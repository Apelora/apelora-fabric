// SPDX-License-Identifier: AGPL-3.0-only
//! The cached answer of the public `GET /api/v1/health`.
//!
//! The public route must not let anyone make the control plane call Incus at will, and must not
//! tell a visitor anything beyond "up, and is Incus reachable". So Incus is asked at most once
//! per [`DEFAULT_INTERVAL`] (on demand: the first request after the interval refreshes, and
//! requests that arrive meanwhile get the previous answer), each probe is bounded by
//! [`PROBE_TIMEOUT`], and only a yes/no is kept. The error text, versions and cluster details
//! stay in the server log and on the authenticated `GET /api/v1/system` and `GET /api/v1/cluster`.

use std::time::{Duration, Instant};

use apelora_domain::ProviderMode;
use apelora_incus_adapter::Provider;

/// Incus is asked at most this often through the public route.
pub const DEFAULT_INTERVAL: Duration = Duration::from_secs(15);
/// A probe that takes longer counts as unreachable.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(5);

/// The last observation.
#[derive(Debug, Clone, Copy)]
pub struct Snapshot {
    /// Talking to a real Incus server that answered (never true for mock data).
    pub connected: bool,
    /// When Incus was last asked (UTC).
    pub checked_at: jiff::Timestamp,
    at: Instant,
}

#[derive(Debug)]
pub struct HealthCache {
    interval: Duration,
    last: std::sync::Mutex<Option<Snapshot>>,
    /// One probe at a time; others use the previous answer meanwhile.
    probing: tokio::sync::Mutex<()>,
}

impl Default for HealthCache {
    fn default() -> Self {
        Self::new(DEFAULT_INTERVAL)
    }
}

impl HealthCache {
    pub fn new(interval: Duration) -> Self {
        Self {
            interval,
            last: std::sync::Mutex::new(None),
            probing: tokio::sync::Mutex::new(()),
        }
    }

    fn cached(&self) -> Option<Snapshot> {
        *self.last.lock().unwrap_or_else(|e| e.into_inner())
    }

    fn fresh(&self) -> Option<Snapshot> {
        self.cached().filter(|s| s.at.elapsed() < self.interval)
    }

    /// The current summary: cached when recent, otherwise probed (once, however many ask).
    pub async fn summary(&self, provider: &Provider) -> Snapshot {
        if let Some(s) = self.fresh() {
            return s;
        }
        let _guard = match (self.probing.try_lock(), self.cached()) {
            (Ok(g), _) => g,
            // Someone is probing: answer with what we had rather than queueing on Incus.
            (Err(_), Some(stale)) => return stale,
            // Nothing yet: wait for the first probe.
            (Err(_), None) => self.probing.lock().await,
        };
        if let Some(s) = self.fresh() {
            return s;
        }
        let previous = self.cached();
        let reachable = match tokio::time::timeout(PROBE_TIMEOUT, provider.server_info()).await {
            Ok(Ok(_)) => Ok(()),
            Ok(Err(e)) => Err(e.to_string()),
            Err(_) => Err(format!("no answer within {} s", PROBE_TIMEOUT.as_secs())),
        };
        let connected = reachable.is_ok() && provider.mode() == ProviderMode::Incus;
        // The detail goes to the log, once per change, never to the public answer.
        match (&reachable, previous.map(|p| p.connected)) {
            (Err(e), Some(true) | None) => {
                tracing::warn!(error = %e, "health: Incus is not reachable")
            }
            (Ok(()), Some(false)) if connected => {
                tracing::info!("health: Incus is reachable again")
            }
            _ => {}
        }
        let snap = Snapshot {
            connected,
            checked_at: jiff::Timestamp::now(),
            at: Instant::now(),
        };
        *self.last.lock().unwrap_or_else(|e| e.into_inner()) = Some(snap);
        snap
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use apelora_incus_adapter::{IncusClient, MockIncus};
    use std::sync::Arc;
    use std::sync::atomic::{AtomicUsize, Ordering};

    /// A stand-in Incus that counts how often `/1.0` is asked.
    async fn counting_incus() -> (Provider, Arc<AtomicUsize>) {
        let hits = Arc::new(AtomicUsize::new(0));
        let counter = hits.clone();
        let app = axum::Router::new().fallback(move || {
            let counter = counter.clone();
            async move {
                counter.fetch_add(1, Ordering::SeqCst);
                axum::Json(serde_json::json!({
                    "type": "sync", "status": "Success", "status_code": 200, "operation": "",
                    "error_code": 0, "error": "",
                    "metadata": { "api_extensions": [], "api_status": "stable", "api_version": "1.0",
                        "auth": "trusted", "auth_methods": ["tls"], "config": {}, "public": false,
                        "environment": { "server_clustered": true, "server_version": "7.0.2",
                            "server_name": "node-1", "kernel_version": "6.12.0", "storage": "zfs" } }
                }))
            }
        });
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        tokio::spawn(async move { axum::serve(listener, app).await.expect("serve") });
        (
            Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}"))),
            hits,
        )
    }

    #[tokio::test]
    async fn incus_is_asked_once_per_interval_however_often_the_route_is_called() {
        let (provider, hits) = counting_incus().await;
        let cache = HealthCache::new(Duration::from_secs(60));
        for _ in 0..20 {
            assert!(cache.summary(&provider).await.connected);
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn concurrent_callers_share_one_probe() {
        let (provider, hits) = counting_incus().await;
        let cache = Arc::new(HealthCache::new(Duration::from_secs(60)));
        let provider = Arc::new(provider);
        let calls: Vec<_> = (0..16)
            .map(|_| {
                let (c, p) = (cache.clone(), provider.clone());
                tokio::spawn(async move { c.summary(&p).await.connected })
            })
            .collect();
        for c in calls {
            assert!(c.await.expect("task"));
        }
        assert_eq!(hits.load(Ordering::SeqCst), 1);
    }

    #[tokio::test]
    async fn an_old_answer_is_refreshed_after_the_interval() {
        let (provider, hits) = counting_incus().await;
        let cache = HealthCache::new(Duration::from_millis(30));
        cache.summary(&provider).await;
        tokio::time::sleep(Duration::from_millis(60)).await;
        cache.summary(&provider).await;
        assert_eq!(hits.load(Ordering::SeqCst), 2);
    }

    #[tokio::test]
    async fn mock_data_is_never_connected_and_an_unreachable_incus_is_not_either() {
        let cache = HealthCache::default();
        assert!(
            !cache
                .summary(&Provider::Mock(MockIncus::default()))
                .await
                .connected
        );
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
            .await
            .expect("bind");
        let addr = listener.local_addr().expect("addr");
        drop(listener);
        let down = Provider::Incus(IncusClient::plain_for_tests(&format!("http://{addr}")));
        assert!(!HealthCache::default().summary(&down).await.connected);
    }
}

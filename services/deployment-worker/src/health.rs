// SPDX-License-Identifier: AGPL-3.0-only
//! Health supervision (Phase 08): decides each checked instance's health with hysteresis
//! (`apelora_domain::health`) and restarts a stateless replica that has stayed unhealthy.
//!
//! The check is the OVN load balancer's port probe of each service backend, read through the
//! load balancer's state; instances without a service address are not checked. Only running
//! instances on online members are observed: a stopped or stranded instance is the replica
//! supervisor's business. At most one replica of an appliance is restarted per pass, so a
//! service whose replicas all fail is not taken down at once; a restart is audited, and the
//! same instance is not restarted again within the cooldown. The decided health is recorded
//! for the status API: the worker is the only place it is decided.

use std::collections::{BTreeMap, HashMap};
use std::time::Instant;

use apelora_domain::Instance;
use apelora_domain::health::{HealthPolicy, HealthState, Probe, Tracker, probe_of};
use apelora_domain::network::network_name;
use apelora_domain::plan::{Availability, DesiredAppliance, replica_name};
use apelora_incus_adapter::Provider;
use apelora_persistence::{DeploymentRecord, NewHealth, Pool, StoreError};
use serde_json::json;
use time::OffsetDateTime;

/// Hysteresis state of every checked instance, across supervision passes.
#[derive(Debug)]
pub struct HealthWatch {
    policy: HealthPolicy,
    epoch: Instant,
    trackers: HashMap<(String, String), Tracker>,
}

impl HealthWatch {
    pub fn new(policy: HealthPolicy) -> Self {
        HealthWatch {
            policy,
            epoch: Instant::now(),
            trackers: HashMap::new(),
        }
    }

    fn now(&self) -> f64 {
        self.epoch.elapsed().as_secs_f64()
    }

    /// Wall-clock time of a tracker timestamp.
    fn wall(&self, t: f64) -> OffsetDateTime {
        OffsetDateTime::now_utc() - time::Duration::seconds_f64((self.now() - t).max(0.0))
    }

    /// Observes one application; returns the instances it queued a restart for.
    pub async fn check(
        &mut self,
        pool: &Pool,
        provider: &Provider,
        d: &DeploymentRecord,
        desired: &[DesiredAppliance],
        observed: &[Instance],
        unavailable: &[String],
    ) -> Result<Vec<String>, StoreError> {
        let project = d.project.as_str();
        let app = d.application.as_str();
        let network = network_name(app);
        // Probe per instance, from each service's load balancer.
        let mut probes: BTreeMap<String, (String, Probe)> = BTreeMap::new();
        for (appliance, address) in
            apelora_persistence::service_addresses(pool, d.application_id, project).await?
        {
            let backends = match provider
                .load_balancer_health(project, &network, &address)
                .await
            {
                Ok(b) => b,
                // The address is allocated before its load balancer exists: no check yet.
                Err(apelora_incus_adapter::AdapterError::NotFound(_)) => {
                    tracing::debug!(application = app, %appliance, "health: no load balancer yet");
                    continue;
                }
                Err(e) => {
                    tracing::warn!(application = app, %appliance, error = %e, "health: cannot read the load balancer's state");
                    continue;
                }
            };
            for (backend, status) in backends {
                if let Some(instance) = backend_instance(app, &appliance, &backend) {
                    probes.insert(instance, (appliance.clone(), probe_of(&status)));
                }
            }
        }
        let now = self.now();
        // Observe every checked instance first, then pick restarts.
        let policy = self.policy;
        let mut seen: Vec<(String, String, Probe)> = Vec::new();
        for i in observed.iter().filter(|i| {
            i.project == project
                && i.status == "Running"
                && !i.unreachable
                && !unavailable.contains(&i.location)
                && i.owner.as_ref().is_some_and(|o| o.application == app)
        }) {
            let Some((appliance, probe)) = probes.get(&i.name) else {
                continue;
            };
            let tracker = self
                .trackers
                .entry((project.to_owned(), i.name.clone()))
                .or_insert_with(|| Tracker::new(now));
            match tracker.observe(*probe, now, &policy) {
                Some(HealthState::Unhealthy) => {
                    tracing::warn!(application = app, instance = %i.name, "health: replica is failing its port check")
                }
                Some(h) => {
                    tracing::info!(application = app, instance = %i.name, health = h.as_str(), "health: replica is passing its port check")
                }
                None => {}
            }
            seen.push((i.name.clone(), appliance.clone(), *probe));
        }
        // Per stateless appliance, at most one restart per pass: the replica restarted least
        // recently (never first), so failing replicas take turns.
        let mut restarted_now: Vec<String> = Vec::new();
        let stateless: Vec<&str> = desired
            .iter()
            .filter(|a| a.availability == Availability::Stateless)
            .map(|a| a.appliance.as_str())
            .collect();
        for appliance in stateless {
            let mut due: Vec<(&String, Option<f64>)> = seen
                .iter()
                .filter(|(_, a, _)| a == appliance)
                .filter_map(|(name, _, _)| {
                    let t = self.trackers.get(&(project.to_owned(), name.clone()))?;
                    t.should_restart(now, &policy)
                        .then(|| (name, t.last_restart()))
                })
                .collect();
            due.sort_by(|x, y| x.1.unwrap_or(f64::MIN).total_cmp(&y.1.unwrap_or(f64::MIN)));
            for (name, _) in due {
                if restart_pending(pool, project, name).await? {
                    continue;
                }
                let Some(tracker) = self.trackers.get_mut(&(project.to_owned(), name.clone()))
                else {
                    continue;
                };
                let job = apelora_persistence::enqueue_job(
                    pool,
                    "instance.restart",
                    Some(d.application_id),
                    &json!({ "project": project, "name": name }),
                    None,
                )
                .await?
                .id();
                let unhealthy_secs = (now - tracker.since).round();
                apelora_persistence::audit(
                    pool,
                    "supervisor",
                    "application.replica.restart-unhealthy",
                    &format!("{project}/{name}"),
                    &json!({ "application": app, "appliance": appliance, "unhealthySeconds": unhealthy_secs, "jobId": job }),
                )
                .await?;
                tracing::warn!(application = app, instance = %name, unhealthy_secs, "health: restarting a replica that keeps failing its port check");
                tracker.restarted(now);
                restarted_now.push(name.clone());
                break;
            }
        }
        let mut checked = Vec::new();
        for (name, appliance, probe) in &seen {
            let Some(tracker) = self.trackers.get(&(project.to_owned(), name.clone())) else {
                continue;
            };
            let detail = match probe {
                Probe::Passing => "port check passing",
                Probe::Failing => "port check failing",
                Probe::Unknown => "port check not reporting",
            };
            apelora_persistence::record_health(
                pool,
                &NewHealth {
                    application_id: d.application_id,
                    project,
                    instance: name,
                    appliance,
                    health: tracker.health.as_str(),
                    since: self.wall(tracker.since),
                    detail,
                    restarted_at: restarted_now.contains(name).then(OffsetDateTime::now_utc),
                },
            )
            .await?;
            checked.push(name.clone());
        }
        // Instances no longer checked (stopped, removed, on an unavailable member) start over.
        self.trackers.retain(|(p, name), _| {
            p != project || checked.contains(name) || !owned_by(observed, project, name, app)
        });
        apelora_persistence::forget_health(pool, d.application_id, project, &checked).await?;
        Ok(restarted_now)
    }
}

/// Whether an instance of `project` belongs to `app` (trackers of other applications stay).
fn owned_by(observed: &[Instance], project: &str, name: &str, app: &str) -> bool {
    observed
        .iter()
        .find(|i| i.project == project && i.name == name)
        .is_none_or(|i| i.owner.as_ref().is_some_and(|o| o.application == app))
}

async fn restart_pending(pool: &Pool, project: &str, name: &str) -> Result<bool, StoreError> {
    for kind in [
        "instance.restart",
        "instance.start",
        "instance.create",
        "instance.delete",
    ] {
        if apelora_persistence::job_in_flight(pool, kind, project, name).await? {
            return Ok(true);
        }
    }
    Ok(false)
}

/// The instance behind a load balancer backend: `r2` → `<app>-<appliance>-r2` (replica 1 has
/// no suffix), `r2-s1` → its substitute.
pub fn backend_instance(app: &str, appliance: &str, backend: &str) -> Option<String> {
    let rest = backend.strip_prefix('r')?;
    let digits: String = rest.chars().take_while(char::is_ascii_digit).collect();
    let replica: u8 = digits.parse().ok()?;
    let suffix = &rest[digits.len()..];
    Some(format!("{}{suffix}", replica_name(app, appliance, replica)))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn backends_map_to_their_instances() {
        assert_eq!(
            backend_instance("shop", "web-01", "r1").as_deref(),
            Some(replica_name("shop", "web-01", 1).as_str())
        );
        assert_eq!(
            backend_instance("shop", "web-01", "r2-s1"),
            Some(format!("{}-s1", replica_name("shop", "web-01", 2)))
        );
        assert_eq!(backend_instance("shop", "web-01", "x1"), None);
    }
}

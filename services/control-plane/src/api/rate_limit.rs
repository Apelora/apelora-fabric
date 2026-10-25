// SPDX-License-Identifier: AGPL-3.0-only
//! Rate limits on the public surface: token buckets in this process's memory.
//!
//! Three limits, each a token bucket (`per_minute` tokens added evenly over a minute, at most
//! `burst` held; `per_minute = 0` turns that limit off):
//!
//! - **public**, per client address: every request to a public route ([`is_public_path`]) and, when
//!   sign-in is required, every request without a bearer token takes one token;
//! - **failed sign-in**, per client address: every refused credential (a 401 to a request that
//!   carried a bearer token) takes one; while it is empty, requests with a bearer token from that
//!   address are refused before the token is even looked at, so guessing tokens is slow;
//! - **writes**, per signed-in caller: every authenticated `POST`/`PUT`/`PATCH`/`DELETE` takes one
//!   (a looser limit that keeps a runaway script from flooding the job queue and the audit log).
//!
//! A refusal is `429 rate_limited` with `Retry-After` (seconds) and a warn line in the request's
//! span (with its request id; never the token). Only the first refusal while a bucket is empty is
//! logged at warn, later ones at debug, so a flood cannot flood the log.
//!
//! **Client address.** The peer address of the connection, unless that peer is a declared
//! trusted proxy (`APELORA_TRUSTED_PROXIES`, CIDR list): then the right-most address in
//! `X-Forwarded-For` that is not itself a trusted proxy. IPv6 clients are grouped by their /64,
//! since one host usually holds a whole /64. Without connection information (only in tests that
//! call the router directly) nothing is limited per address.
//!
//! **Memory** is bounded: at most `max_clients` buckets per limit; when a new key arrives at the
//! limit, buckets that have refilled completely (idle callers, indistinguishable from new ones)
//! are dropped, and if none has, the least recently used one is.
//!
//! **Per process.** With several API processes each keeps its own buckets, so the effective limit
//! is N times the configured one (`docs/architecture/CONNECTIONS-AND-SCALE.md` §4).

use std::collections::HashMap;
use std::hash::Hash;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{Extensions, HeaderMap, HeaderValue, Method, StatusCode, header};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};

use super::{ApiError, AppState, Auth, Principal};

/// One limit: tokens per minute and the most that can be saved up.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Rate {
    pub per_minute: u32,
    pub burst: u32,
}

impl Rate {
    fn off(self) -> bool {
        self.per_minute == 0 || self.burst == 0
    }

    fn per_sec(self) -> f64 {
        f64::from(self.per_minute) / 60.0
    }
}

/// Settings (`APELORA_RATE_LIMIT_*`, `APELORA_TRUSTED_PROXIES`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RateLimitConfig {
    pub public: Rate,
    pub failed_auth: Rate,
    pub writes: Rate,
    pub max_clients: usize,
    pub trusted_proxies: Vec<Cidr>,
}

impl Default for RateLimitConfig {
    fn default() -> Self {
        Self {
            // A browser loading the Console or the Studio makes a handful of public calls; a
            // load balancer probing /health every 2 s makes 30 a minute.
            public: Rate {
                per_minute: 120,
                burst: 60,
            },
            failed_auth: Rate {
                per_minute: 10,
                burst: 20,
            },
            writes: Rate {
                per_minute: 600,
                burst: 200,
            },
            max_clients: 10_000,
            trusted_proxies: Vec::new(),
        }
    }
}

impl RateLimitConfig {
    pub fn from_env(get: impl Fn(&str) -> Option<String>) -> Result<Self, String> {
        let mut c = Self::default();
        let num = |key: &str, max: u64| -> Result<Option<u64>, String> {
            match get(key).filter(|v| !v.trim().is_empty()) {
                None => Ok(None),
                Some(v) => v
                    .trim()
                    .parse::<u64>()
                    .ok()
                    .filter(|n| *n <= max)
                    .map(Some)
                    .ok_or_else(|| format!("{key} must be a whole number from 0 to {max}")),
            }
        };
        let rate = |name: &str, r: &mut Rate| -> Result<(), String> {
            let as_u32 = |n: u64| u32::try_from(n).unwrap_or(u32::MAX);
            if let Some(n) = num(&format!("APELORA_RATE_LIMIT_{name}_PER_MIN"), 1_000_000)? {
                r.per_minute = as_u32(n);
            }
            if let Some(n) = num(&format!("APELORA_RATE_LIMIT_{name}_BURST"), 1_000_000)? {
                r.burst = as_u32(n);
            }
            Ok(())
        };
        rate("PUBLIC", &mut c.public)?;
        rate("FAILED_AUTH", &mut c.failed_auth)?;
        rate("WRITES", &mut c.writes)?;
        if let Some(n) = num("APELORA_RATE_LIMIT_MAX_CLIENTS", 10_000_000)? {
            if n == 0 {
                return Err("APELORA_RATE_LIMIT_MAX_CLIENTS must be at least 1".into());
            }
            c.max_clients = usize::try_from(n).unwrap_or(usize::MAX);
        }
        if let Some(list) = get("APELORA_TRUSTED_PROXIES") {
            c.trusted_proxies = list
                .split(',')
                .map(str::trim)
                .filter(|s| !s.is_empty())
                .map(|s| {
                    Cidr::parse(s).ok_or_else(|| {
                        format!(
                            "APELORA_TRUSTED_PROXIES: {s:?} is not an address or CIDR range (e.g. 10.0.0.5 or 10.0.0.0/24)"
                        )
                    })
                })
                .collect::<Result<_, _>>()?;
        }
        Ok(c)
    }
}

/// An address range, e.g. `10.0.0.0/24` or `fd00::/8`; a bare address is a range of one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Cidr {
    net: IpAddr,
    prefix: u8,
}

impl Cidr {
    pub fn parse(s: &str) -> Option<Self> {
        let (addr, prefix) = match s.split_once('/') {
            Some((a, p)) => (a, Some(p)),
            None => (s, None),
        };
        let net = canonical(addr.trim().parse::<IpAddr>().ok()?);
        let max = if net.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(p) => p.trim().parse::<u8>().ok().filter(|p| *p <= max)?,
            None => max,
        };
        Some(Self { net, prefix })
    }

    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.net, canonical(ip)) {
            (IpAddr::V4(n), IpAddr::V4(a)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(n) & mask == u32::from(a) & mask
            }
            (IpAddr::V6(n), IpAddr::V6(a)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(n) & mask == u128::from(a) & mask
            }
            _ => false,
        }
    }
}

/// IPv4-mapped IPv6 addresses (`::ffff:a.b.c.d`) are the IPv4 address.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map_or(ip, IpAddr::V4),
        v4 => v4,
    }
}

/// The caller's address: the peer, or what a trusted proxy says it forwarded for.
pub fn client_ip(peer: IpAddr, forwarded_for: Option<&str>, trusted: &[Cidr]) -> IpAddr {
    let peer = canonical(peer);
    let is_trusted = |ip: IpAddr| trusted.iter().any(|c| c.contains(ip));
    if !is_trusted(peer) {
        return peer;
    }
    let mut client = peer;
    for part in forwarded_for.unwrap_or_default().rsplit(',') {
        match part.trim().parse::<IpAddr>() {
            Ok(ip) => {
                client = canonical(ip);
                if !is_trusted(client) {
                    return client;
                }
            }
            // Anything unreadable ends the chain: the last address we could trust stands.
            Err(_) => return client,
        }
    }
    client
}

/// The bucket key for an address: IPv4 as is, IPv6 by its /64.
pub(super) fn bucket_key(ip: IpAddr) -> IpAddr {
    match canonical(ip) {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & (u128::MAX << 64))),
        v4 => v4,
    }
}

/// Routes anyone may call without signing in. A new public route belongs here (a test compares
/// this with the OpenAPI document's routes that need no authentication).
pub const PUBLIC_PATHS: &[&str] = &[
    "/api/v1/health",
    "/api/v1/sign-in",
    "/api/v1/openapi.json",
    super::sessions::CONNECT_PATH,
];

pub fn is_public_path(path: &str) -> bool {
    PUBLIC_PATHS.contains(&path) || path.starts_with("/api/v1/sign-in/")
}

#[derive(Debug, Clone, Copy)]
struct Bucket {
    tokens: f64,
    updated: Instant,
    /// Refusing since the last token was taken (the first refusal is logged at warn).
    refusing: bool,
}

/// What a bucket said.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Verdict {
    Allowed,
    /// Refused; wait this long. `first`: the first refusal since the bucket last allowed one.
    Refused {
        retry_after: Duration,
        first: bool,
    },
}

#[derive(Debug)]
struct Buckets<K> {
    rate: Rate,
    max: usize,
    map: std::sync::Mutex<HashMap<K, Bucket>>,
}

impl<K: Hash + Eq + Clone> Buckets<K> {
    fn new(rate: Rate, max: usize) -> Self {
        Self {
            rate,
            max,
            map: std::sync::Mutex::new(HashMap::new()),
        }
    }

    fn refill(&self, b: &mut Bucket, now: Instant) {
        let gained = now.duration_since(b.updated).as_secs_f64() * self.rate.per_sec();
        b.tokens = (b.tokens + gained).min(f64::from(self.rate.burst));
        b.updated = now;
    }

    /// Takes one token (`take`), or only checks that one is there.
    fn ask(&self, key: &K, now: Instant, take: bool) -> Verdict {
        if self.rate.off() {
            return Verdict::Allowed;
        }
        let mut map = self.map.lock().unwrap_or_else(|e| e.into_inner());
        if !map.contains_key(key) {
            if !take {
                return Verdict::Allowed; // a full bucket
            }
            if map.len() >= self.max {
                self.evict(&mut map, now);
            }
        }
        let fresh = Bucket {
            tokens: f64::from(self.rate.burst),
            updated: now,
            refusing: false,
        };
        let b = map.entry(key.clone()).or_insert(fresh);
        self.refill(b, now);
        if b.tokens >= 1.0 {
            if take {
                b.tokens -= 1.0;
                b.refusing = false;
            }
            return Verdict::Allowed;
        }
        let wait = (1.0 - b.tokens) / self.rate.per_sec();
        let first = !b.refusing;
        b.refusing = true;
        Verdict::Refused {
            retry_after: Duration::from_secs_f64(wait.max(0.0)),
            first,
        }
    }

    /// Makes room: drops buckets that have refilled completely, else the least recently used.
    fn evict(&self, map: &mut HashMap<K, Bucket>, now: Instant) {
        let full = f64::from(self.rate.burst);
        let per_sec = self.rate.per_sec();
        map.retain(|_, b| b.tokens + now.duration_since(b.updated).as_secs_f64() * per_sec < full);
        while map.len() >= self.max {
            let Some(oldest) = map
                .iter()
                .min_by_key(|(_, b)| b.updated)
                .map(|(k, _)| k.clone())
            else {
                break;
            };
            map.remove(&oldest);
        }
    }

    #[cfg(test)]
    fn has(&self, key: &K) -> bool {
        self.map
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(key)
    }

    #[cfg(test)]
    fn len(&self) -> usize {
        self.map.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The limits of this process.
#[derive(Debug)]
pub struct Limiter {
    trusted: Vec<Cidr>,
    public: Buckets<IpAddr>,
    failed_auth: Buckets<IpAddr>,
    writes: Buckets<String>,
}

impl Default for Limiter {
    fn default() -> Self {
        Self::new(RateLimitConfig::default())
    }
}

impl Limiter {
    pub fn new(c: RateLimitConfig) -> Self {
        Self {
            trusted: c.trusted_proxies,
            public: Buckets::new(c.public, c.max_clients),
            failed_auth: Buckets::new(c.failed_auth, c.max_clients),
            writes: Buckets::new(c.writes, c.max_clients),
        }
    }

    fn client(&self, req: &Request) -> Option<IpAddr> {
        self.client_of(req.extensions(), req.headers())
    }

    /// The client address of a request (see the module notes): the peer, or what a trusted
    /// proxy forwarded for. `None` without connection information.
    pub fn client_of(&self, extensions: &Extensions, headers: &HeaderMap) -> Option<IpAddr> {
        let peer = extensions.get::<ConnectInfo<SocketAddr>>()?.0.ip();
        let forwarded = headers
            .get_all("x-forwarded-for")
            .iter()
            .filter_map(|v| v.to_str().ok())
            .collect::<Vec<_>>()
            .join(",");
        Some(client_ip(
            peer,
            (!forwarded.is_empty()).then_some(forwarded.as_str()),
            &self.trusted,
        ))
    }
}

fn refused(limit: &'static str, retry_after: Duration, first: bool, who: &str) -> Response {
    let secs = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
    let secs = secs.max(1);
    // Inside the request span: the line carries the request id. Never the token.
    if first {
        tracing::warn!(
            limit,
            client = who,
            retry_after_secs = secs,
            "rate limit: refusing requests"
        );
    } else {
        tracing::debug!(
            limit,
            client = who,
            retry_after_secs = secs,
            "rate limit: refused"
        );
    }
    let mut res = ApiError::new(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limited",
        format!("too many requests; try again in {secs} s"),
    )
    .into_response();
    if let Ok(v) = HeaderValue::from_str(&secs.to_string()) {
        res.headers_mut().insert(header::RETRY_AFTER, v);
    }
    res
}

fn has_bearer(req: &Request) -> bool {
    req.headers()
        .get(header::AUTHORIZATION)
        .and_then(|v| v.to_str().ok())
        .and_then(|v| v.strip_prefix("Bearer "))
        .is_some_and(|t| !t.trim().is_empty())
}

/// Outer layer: the per-address limits (public routes, unauthenticated requests, refused
/// credentials).
pub async fn per_address(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let limits = state.rate_limits.clone();
    let Some(ip) = limits.client(&req) else {
        return next.run(req).await;
    };
    let key = bucket_key(ip);
    let now = Instant::now();
    let bearer = has_bearer(&req);
    let sign_in_required = matches!(state.auth, Auth::Tokens(_));
    let public = is_public_path(req.uri().path()) || (sign_in_required && !bearer);
    let verdict = if public {
        limits.public.ask(&key, now, true)
    } else if bearer {
        limits.failed_auth.ask(&key, now, false)
    } else {
        Verdict::Allowed
    };
    if let Verdict::Refused { retry_after, first } = verdict {
        let limit = if public { "public" } else { "failed-sign-in" };
        return refused(limit, retry_after, first, &ip.to_string());
    }
    let res = next.run(req).await;
    if bearer && res.status() == StatusCode::UNAUTHORIZED {
        limits.failed_auth.ask(&key, Instant::now(), true);
    }
    res
}

/// Inner layer (after authentication): the per-caller limit on writes.
pub async fn per_principal(State(state): State<AppState>, req: Request, next: Next) -> Response {
    let write = !matches!(*req.method(), Method::GET | Method::HEAD | Method::OPTIONS);
    if let (true, Some(p)) = (write, req.extensions().get::<Principal>()) {
        let who = p.actor_ref();
        if let Verdict::Refused { retry_after, first } =
            state.rate_limits.writes.ask(&who, Instant::now(), true)
        {
            return refused("writes", retry_after, first, &who);
        }
    }
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ip(s: &str) -> IpAddr {
        s.parse().expect("ip")
    }

    #[test]
    fn ranges_match_their_addresses_only() {
        let r = Cidr::parse("10.1.0.0/16").expect("cidr");
        assert!(r.contains(ip("10.1.200.3")));
        assert!(r.contains(ip("::ffff:10.1.0.9")), "IPv4-mapped IPv6");
        assert!(!r.contains(ip("10.2.0.1")));
        assert!(!r.contains(ip("fd00::1")));
        let one = Cidr::parse("192.0.2.7").expect("one address");
        assert!(one.contains(ip("192.0.2.7")) && !one.contains(ip("192.0.2.8")));
        let v6 = Cidr::parse("fd00:1::/32").expect("v6");
        assert!(v6.contains(ip("fd00:1:ffff::1")) && !v6.contains(ip("fd00:2::1")));
        assert!(
            Cidr::parse("0.0.0.0/0")
                .expect("all")
                .contains(ip("203.0.113.1"))
        );
        for bad in [
            "10.0.0.0/33",
            "fd00::/129",
            "example",
            "10.0.0/8",
            "10.0.0.0/x",
        ] {
            assert_eq!(Cidr::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn forwarded_for_counts_only_when_the_peer_is_a_trusted_proxy() {
        let trusted = [Cidr::parse("10.0.0.0/24").expect("cidr")];
        // Not a proxy: the header is the caller's own claim and is ignored.
        assert_eq!(
            client_ip(ip("203.0.113.9"), Some("198.51.100.1"), &trusted),
            ip("203.0.113.9")
        );
        // From the proxy: the right-most untrusted address; a client cannot prepend its way out.
        assert_eq!(
            client_ip(ip("10.0.0.2"), Some("1.2.3.4, 198.51.100.7"), &trusted),
            ip("198.51.100.7")
        );
        // Chained proxies are skipped.
        assert_eq!(
            client_ip(ip("10.0.0.2"), Some("198.51.100.7, 10.0.0.3"), &trusted),
            ip("198.51.100.7")
        );
        // A proxy that forwarded nothing, or garbage: the proxy's own address.
        assert_eq!(client_ip(ip("10.0.0.2"), None, &trusted), ip("10.0.0.2"));
        assert_eq!(
            client_ip(ip("10.0.0.2"), Some("unknown"), &trusted),
            ip("10.0.0.2")
        );
        // No proxies declared: always the peer.
        assert_eq!(
            client_ip(ip("10.0.0.2"), Some("198.51.100.7"), &[]),
            ip("10.0.0.2")
        );
    }

    #[test]
    fn ipv6_callers_share_a_bucket_per_64() {
        assert_eq!(
            bucket_key(ip("2001:db8:1:2:aaaa::1")),
            bucket_key(ip("2001:db8:1:2:bbbb::9"))
        );
        assert_ne!(
            bucket_key(ip("2001:db8:1:2::1")),
            bucket_key(ip("2001:db8:1:3::1"))
        );
        assert_eq!(bucket_key(ip("192.0.2.1")), ip("192.0.2.1"));
    }

    #[test]
    fn a_bucket_allows_its_burst_then_refills_at_its_rate() {
        let b = Buckets::new(
            Rate {
                per_minute: 60,
                burst: 3,
            },
            100,
        );
        let t0 = Instant::now();
        for _ in 0..3 {
            assert_eq!(b.ask(&1, t0, true), Verdict::Allowed);
        }
        let Verdict::Refused { retry_after, first } = b.ask(&1, t0, true) else {
            panic!("the fourth request in the same instant is refused");
        };
        assert!(first && retry_after <= Duration::from_secs(1));
        assert_eq!(
            b.ask(&1, t0, true),
            Verdict::Refused {
                retry_after: Duration::from_secs(1),
                first: false
            },
            "later refusals are not the first"
        );
        assert_eq!(
            b.ask(&2, t0, true),
            Verdict::Allowed,
            "other callers are not affected"
        );
        assert_eq!(
            b.ask(&1, t0 + Duration::from_millis(1001), true),
            Verdict::Allowed
        );
    }

    #[test]
    fn checking_does_not_spend_and_an_off_limit_allows_everything() {
        let b = Buckets::new(
            Rate {
                per_minute: 60,
                burst: 1,
            },
            100,
        );
        let t0 = Instant::now();
        for _ in 0..5 {
            assert_eq!(b.ask(&1, t0, false), Verdict::Allowed);
        }
        assert_eq!(b.len(), 0, "a check does not create a bucket");
        let off = Buckets::new(
            Rate {
                per_minute: 0,
                burst: 10,
            },
            100,
        );
        for _ in 0..100 {
            assert_eq!(off.ask(&1, t0, true), Verdict::Allowed);
        }
    }

    #[test]
    fn memory_is_bounded_by_dropping_idle_then_least_recent_buckets() {
        let b = Buckets::new(
            Rate {
                per_minute: 60,
                burst: 2,
            },
            3,
        );
        let t0 = Instant::now();
        for k in 0..3 {
            b.ask(&k, t0 + Duration::from_millis(k), true);
        }
        assert_eq!(b.len(), 3);
        // Nobody has refilled yet: the least recently used (0) makes room.
        b.ask(&10, t0 + Duration::from_millis(10), true);
        assert_eq!(b.len(), 3);
        assert!(!b.has(&0) && b.has(&1) && b.has(&10));
        // Much later every bucket has refilled: all idle ones go at once.
        b.ask(&20, t0 + Duration::from_secs(60), true);
        assert_eq!(b.len(), 1);
        for k in 100..10_000 {
            b.ask(&k, t0 + Duration::from_secs(61), true);
        }
        assert!(b.len() <= 3);
    }

    #[test]
    fn settings_come_from_the_environment_with_safe_defaults() {
        let none = RateLimitConfig::from_env(|_| None).expect("defaults");
        assert_eq!(none, RateLimitConfig::default());
        assert!(none.public.per_minute > 0 && none.failed_auth.per_minute > 0);
        let env = |k: &str| match k {
            "APELORA_RATE_LIMIT_PUBLIC_PER_MIN" => Some("30".to_owned()),
            "APELORA_RATE_LIMIT_PUBLIC_BURST" => Some("5".to_owned()),
            "APELORA_RATE_LIMIT_WRITES_PER_MIN" => Some("0".to_owned()),
            "APELORA_TRUSTED_PROXIES" => Some("10.0.0.0/24, fd00::1".to_owned()),
            _ => None,
        };
        let c = RateLimitConfig::from_env(env).expect("config");
        assert_eq!(
            c.public,
            Rate {
                per_minute: 30,
                burst: 5
            }
        );
        assert!(c.writes.off());
        assert_eq!(c.trusted_proxies.len(), 2);
        for (k, v) in [
            ("APELORA_RATE_LIMIT_PUBLIC_PER_MIN", "many"),
            ("APELORA_RATE_LIMIT_MAX_CLIENTS", "0"),
            ("APELORA_TRUSTED_PROXIES", "10.0.0.0/24,proxy.example"),
        ] {
            let err =
                RateLimitConfig::from_env(|key| (key == k).then(|| v.to_owned())).expect_err(k);
            assert!(err.starts_with(k), "{err}");
        }
    }
}

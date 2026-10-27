//! Rate limits of the session-free endpoints (ADR-0022): pairing mailboxes per client
//! address and requests per mailbox, recovery reads per client address. Sliding
//! windows in memory, on the server's clock; a restart forgets them.
//!
//! The client address is the TCP peer, unless the peer is a trusted proxy
//! (`KB_SERVER_TRUSTED_PROXIES`): then it is the last `X-Forwarded-For` address that is
//! not itself a trusted proxy, else `X-Real-IP`, else the peer. Headers from a peer that
//! is not trusted are ignored (anyone can send them). IPv6 clients are counted per /64
//! (one host usually owns the whole prefix). Header values are never logged.

use std::collections::{HashMap, VecDeque};
use std::convert::Infallible;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::str::FromStr;
use std::sync::{Arc, Mutex, PoisonError};

use axum::extract::{ConnectInfo, FromRequestParts};
use axum::http::HeaderMap;
use axum::http::request::Parts;

use crate::api::AppState;

/// Mailboxes one address may create per window.
pub(crate) const MAILBOXES_PER_ADDRESS: usize = 10;
/// Window of [`MAILBOXES_PER_ADDRESS`].
pub(crate) const MAILBOX_WINDOW_MS: i64 = 10 * 60 * 1000;
/// Requests one mailbox may receive per window (reads, posts, close).
pub(crate) const REQUESTS_PER_MAILBOX: usize = 60;
/// Window of [`REQUESTS_PER_MAILBOX`].
pub(crate) const MAILBOX_REQUEST_WINDOW_MS: i64 = 60 * 1000;
/// Recovery blob reads one address may make per window.
pub(crate) const RECOVERY_READS_PER_ADDRESS: usize = 5;
/// Window of [`RECOVERY_READS_PER_ADDRESS`].
pub(crate) const RECOVERY_READ_WINDOW_MS: i64 = 60 * 1000;
/// `invite_open` calls one address may make per window (`INVITE_OPEN_PER_IP_PER_HOUR`).
pub(crate) const INVITE_OPENS_PER_ADDRESS: usize = 30;
/// Window of [`INVITE_OPENS_PER_ADDRESS`].
pub(crate) const INVITE_OPEN_WINDOW_MS: i64 = 60 * 60 * 1000;
/// Public view-link reads (the publication and its chunks together) one address may make per window
/// (`kb_types::sharing::LINK_GETS_PER_MINUTE_PER_IP`).
pub(crate) const LINK_GET_WINDOW_MS: i64 = 60 * 1000;
/// The longest window: older timestamps can always be forgotten.
const LONGEST_WINDOW_MS: i64 = INVITE_OPEN_WINDOW_MS;
/// Keys tracked before a sweep runs on insertion.
const SWEEP_AT_KEYS: usize = 50_000;

/// The client's address (`None` when the server runs without connection info).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) struct ClientAddr(pub(crate) Option<IpAddr>);

/// IPv4 (also IPv4-mapped IPv6) as is, IPv6 cut to its /64.
fn bucket(ip: IpAddr) -> IpAddr {
    match ip.to_canonical() {
        IpAddr::V4(v4) => IpAddr::V4(v4),
        IpAddr::V6(v6) => {
            let mut segments = v6.segments();
            for segment in &mut segments[4..] {
                *segment = 0;
            }
            IpAddr::V6(Ipv6Addr::from(segments))
        }
    }
}

/// One entry of `KB_SERVER_TRUSTED_PROXIES`: an address (`10.0.0.2`, `fd00::2`) or a
/// network in CIDR notation (`172.30.0.0/24`, `fd00::/64`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxy {
    network: IpAddr,
    prefix: u8,
}

impl TrustedProxy {
    /// Whether `ip` (IPv4-mapped IPv6 counts as IPv4) lies in this network.
    #[must_use]
    pub fn contains(&self, ip: IpAddr) -> bool {
        match (self.network, ip.to_canonical()) {
            (IpAddr::V4(net), IpAddr::V4(ip)) => {
                let mask = u32::MAX
                    .checked_shl(32 - u32::from(self.prefix))
                    .unwrap_or(0);
                u32::from(net) & mask == u32::from(ip) & mask
            }
            (IpAddr::V6(net), IpAddr::V6(ip)) => {
                let mask = u128::MAX
                    .checked_shl(128 - u32::from(self.prefix))
                    .unwrap_or(0);
                u128::from(net) & mask == u128::from(ip) & mask
            }
            _ => false,
        }
    }
}

/// A malformed `KB_SERVER_TRUSTED_PROXIES` entry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TrustedProxyParseError;

impl FromStr for TrustedProxy {
    type Err = TrustedProxyParseError;

    fn from_str(text: &str) -> Result<Self, Self::Err> {
        let (address, prefix) = match text.trim().split_once('/') {
            Some((address, prefix)) => (address, Some(prefix)),
            None => (text.trim(), None),
        };
        let network = IpAddr::from_str(address)
            .map_err(|_| TrustedProxyParseError)?
            .to_canonical();
        let max = if network.is_ipv4() { 32 } else { 128 };
        let prefix = match prefix {
            Some(prefix) => prefix
                .parse::<u8>()
                .ok()
                .filter(|p| *p <= max)
                .ok_or(TrustedProxyParseError)?,
            None => max,
        };
        Ok(Self { network, prefix })
    }
}

fn trusted(ip: IpAddr, proxies: &[TrustedProxy]) -> bool {
    proxies.iter().any(|proxy| proxy.contains(ip))
}

/// An address as proxies write it: a bare IP, or with a port (`1.2.3.4:5`, `[::1]:5`).
fn header_ip(text: &str) -> Option<IpAddr> {
    let text = text.trim();
    IpAddr::from_str(text)
        .ok()
        .or_else(|| SocketAddr::from_str(text).ok().map(|s| s.ip()))
        .map(|ip| ip.to_canonical())
}

/// The address the rate limits count for a request from `peer`.
pub(crate) fn client_ip(peer: IpAddr, headers: &HeaderMap, proxies: &[TrustedProxy]) -> IpAddr {
    let peer = peer.to_canonical();
    if !trusted(peer, proxies) {
        return peer;
    }
    // Every X-Forwarded-For header, in order, then right to left: each proxy appends
    // the address it saw, so the first one that is not ours is the client.
    let forwarded: Vec<&str> = headers
        .get_all("x-forwarded-for")
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .collect();
    for entry in forwarded.iter().rev() {
        match header_ip(entry) {
            Some(ip) if trusted(ip, proxies) => {}
            Some(ip) => return ip,
            None => break,
        }
    }
    headers
        .get("x-real-ip")
        .and_then(|value| value.to_str().ok())
        .and_then(header_ip)
        .unwrap_or(peer)
}

impl FromRequestParts<Arc<AppState>> for ClientAddr {
    type Rejection = Infallible;

    async fn from_request_parts(
        parts: &mut Parts,
        state: &Arc<AppState>,
    ) -> Result<Self, Self::Rejection> {
        Ok(Self(parts.extensions.get::<ConnectInfo<SocketAddr>>().map(
            |info| {
                bucket(client_ip(
                    info.0.ip(),
                    &parts.headers,
                    &state.settings.trusted_proxies,
                ))
            },
        )))
    }
}

/// What a limit counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub(crate) enum LimitKey {
    MailboxCreate(ClientAddr),
    Mailbox([u8; kb_types::devices::MAILBOX_ID_LEN]),
    RecoveryRead(ClientAddr),
    InviteOpen(ClientAddr),
    LinkGet(ClientAddr),
}

/// Sliding-window counters.
#[derive(Default)]
pub(crate) struct RateLimiter {
    windows: Mutex<HashMap<LimitKey, VecDeque<i64>>>,
}

impl RateLimiter {
    /// Counts one request under `key`; `false` (and nothing counted) when `limit`
    /// requests already happened within `window_ms` before `now_ms`.
    pub(crate) fn allow(&self, key: LimitKey, limit: usize, window_ms: i64, now_ms: i64) -> bool {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        if windows.len() >= SWEEP_AT_KEYS {
            sweep(&mut windows, now_ms);
        }
        let times = windows.entry(key).or_default();
        let start = now_ms.saturating_sub(window_ms);
        while times.front().is_some_and(|t| *t <= start) {
            times.pop_front();
        }
        if times.len() >= limit {
            return false;
        }
        times.push_back(now_ms);
        true
    }

    /// Forgets keys with no request inside the longest window.
    pub(crate) fn sweep(&self, now_ms: i64) {
        let mut windows = self.windows.lock().unwrap_or_else(PoisonError::into_inner);
        sweep(&mut windows, now_ms);
    }
}

fn sweep(windows: &mut HashMap<LimitKey, VecDeque<i64>>, now_ms: i64) {
    let start = now_ms.saturating_sub(LONGEST_WINDOW_MS);
    windows.retain(|_, times| times.back().is_some_and(|t| *t > start));
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn a_window_admits_its_limit_then_slides() {
        let limiter = RateLimiter::default();
        let key = LimitKey::RecoveryRead(ClientAddr(None));
        for t in 0..5 {
            assert!(limiter.allow(key, 5, 1000, t));
        }
        assert!(!limiter.allow(key, 5, 1000, 999));
        // Refusals are not counted: the first request leaves the window at 1000.
        assert!(limiter.allow(key, 5, 1000, 1000));
        assert!(!limiter.allow(key, 5, 1000, 1000));
        // Other keys have their own window.
        assert!(limiter.allow(LimitKey::Mailbox([1; 6]), 5, 1000, 1000));
    }

    #[test]
    fn sweeping_forgets_idle_keys_only() {
        let limiter = RateLimiter::default();
        let old = LimitKey::Mailbox([1; 6]);
        let recent = LimitKey::Mailbox([2; 6]);
        assert!(limiter.allow(old, 1, 1000, 0));
        assert!(limiter.allow(recent, 1, 1000, LONGEST_WINDOW_MS));
        limiter.sweep(LONGEST_WINDOW_MS + 1);
        let windows = limiter.windows.lock().unwrap();
        assert!(!windows.contains_key(&old));
        assert!(windows.contains_key(&recent));
    }

    fn headers(pairs: &[(&'static str, &'static str)]) -> HeaderMap {
        let mut map = HeaderMap::new();
        for (name, value) in pairs {
            map.append(*name, value.parse().unwrap());
        }
        map
    }

    fn ip(text: &str) -> IpAddr {
        text.parse().unwrap()
    }

    #[test]
    fn trusted_proxies_parse_addresses_and_networks() {
        let single: TrustedProxy = "10.0.0.2".parse().unwrap();
        assert!(single.contains(ip("10.0.0.2")));
        assert!(!single.contains(ip("10.0.0.3")));
        assert!(single.contains(ip("::ffff:10.0.0.2")));
        let net: TrustedProxy = " 172.30.0.0/24 ".parse().unwrap();
        assert!(net.contains(ip("172.30.0.200")));
        assert!(!net.contains(ip("172.30.1.1")));
        let all: TrustedProxy = "0.0.0.0/0".parse().unwrap();
        assert!(all.contains(ip("8.8.8.8")) && !all.contains(ip("::1")));
        let v6: TrustedProxy = "fd00::/64".parse().unwrap();
        assert!(v6.contains(ip("fd00::1234")) && !v6.contains(ip("fd00:0:0:1::1")));
        for bad in [
            "",
            "10.0.0.1/33",
            "fd00::/129",
            "10.0.0/8",
            "proxy",
            "10.0.0.1/x",
        ] {
            assert!(bad.parse::<TrustedProxy>().is_err(), "{bad}");
        }
    }

    #[test]
    fn forwarded_headers_count_only_from_trusted_peers() {
        let proxies: Vec<TrustedProxy> = vec!["172.30.0.10".parse().unwrap()];
        let caddy = ip("172.30.0.10");
        let forwarded = headers(&[("x-forwarded-for", "203.0.113.7")]);
        assert_eq!(client_ip(caddy, &forwarded, &proxies), ip("203.0.113.7"));
        // An untrusted peer's headers are ignored.
        assert_eq!(
            client_ip(ip("198.51.100.1"), &forwarded, &proxies),
            ip("198.51.100.1")
        );
        // A spoofed entry on the left does not win over what the proxy appended.
        let spoofed = headers(&[("x-forwarded-for", "10.9.9.9, 203.0.113.7")]);
        assert_eq!(client_ip(caddy, &spoofed, &proxies), ip("203.0.113.7"));
        // Trusted hops are skipped, across several headers.
        let chain = headers(&[
            ("x-forwarded-for", "203.0.113.8"),
            ("x-forwarded-for", "172.30.0.10"),
        ]);
        assert_eq!(client_ip(caddy, &chain, &proxies), ip("203.0.113.8"));
        let with_port = headers(&[("x-forwarded-for", "[2001:db8::5]:4433")]);
        assert_eq!(client_ip(caddy, &with_port, &proxies), ip("2001:db8::5"));
        // Fallbacks: X-Real-IP, then the peer.
        let real = headers(&[("x-real-ip", "203.0.113.9")]);
        assert_eq!(client_ip(caddy, &real, &proxies), ip("203.0.113.9"));
        let garbage = headers(&[("x-forwarded-for", "not an address"), ("x-real-ip", "nope")]);
        assert_eq!(client_ip(caddy, &garbage, &proxies), caddy);
        let only_proxies = headers(&[("x-forwarded-for", "172.30.0.10")]);
        assert_eq!(client_ip(caddy, &only_proxies, &proxies), caddy);
        assert_eq!(client_ip(caddy, &HeaderMap::new(), &[]), caddy);
    }

    #[test]
    fn ipv6_counts_per_prefix_and_mapped_ipv4_as_ipv4() {
        let a: IpAddr = "2001:db8:1:2:aaaa::1".parse().unwrap();
        let b: IpAddr = "2001:db8:1:2:bbbb::2".parse().unwrap();
        let c: IpAddr = "2001:db8:1:3::1".parse().unwrap();
        assert_eq!(bucket(a), bucket(b));
        assert_ne!(bucket(a), bucket(c));
        let mapped: IpAddr = "::ffff:192.0.2.7".parse().unwrap();
        assert_eq!(bucket(mapped), "192.0.2.7".parse::<IpAddr>().unwrap());
    }
}

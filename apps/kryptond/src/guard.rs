//! Per-client request budgets and a global concurrency cap for the HTTP API.
//!
//! Clients are told apart by IP (IPv6 by /64). Requests that reach the node from a reverse
//! proxy on this machine carry the real client in `X-Real-IP`; local requests without it
//! (this computer's wallet and miner) are not limited.

use axum::extract::{ConnectInfo, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::Arc;
use std::time::Instant;
use tokio::sync::Semaphore;

/// Requests per minute and burst size for one client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Class {
    /// Cheap reads (info, headers, single lookups).
    Read,
    /// Reads that touch many blocks or tree nodes.
    Scan,
    Template,
    Submit,
    Tx,
    Proof,
    Address,
    Rpc,
}

impl Class {
    fn budget(self) -> (f64, f64) {
        match self {
            Class::Read => (3000.0, 600.0),
            Class::Scan => (600.0, 120.0),
            Class::Template => (120.0, 30.0),
            Class::Submit => (1200.0, 200.0),
            Class::Tx => (30.0, 10.0),
            Class::Proof => (30.0, 10.0),
            Class::Address => (10.0, 5.0),
            Class::Rpc => (300.0, 60.0),
        }
    }

    pub fn of(method: &Method, path: &str) -> Class {
        let p = path.trim_end_matches('/');
        match (method == Method::POST, p) {
            (true, "/v1/tx") => Class::Tx,
            (true, "/v1/verify-payment") => Class::Proof,
            (true, "/v1/address") | (true, "/v1/swap/offers") | (true, "/v1/swap/trade") => {
                Class::Address
            }
            (true, "/v1/mining/template") => Class::Template,
            (true, "/v1/mining/submit") => Class::Submit,
            (true, _) => Class::Rpc,
            _ if p.starts_with("/v1/compact")
                || p.starts_with("/v1/subtree")
                || p.starts_with("/v1/blocks")
                || p.starts_with("/v1/flyclient")
                || p.starts_with("/v1/mmrproof")
                || p.starts_with("/v1/rawblock")
                || p.starts_with("/v1/rawtx")
                || p.starts_with("/v1/witness")
                || p.starts_with("/v1/frontier")
                || p.starts_with("/v1/address-info")
                || p.starts_with("/v1/stats")
                || p.starts_with("/v1/miner")
                || p.starts_with("/v1/block/") =>
            {
                Class::Scan
            }
            _ => Class::Read,
        }
    }
}

struct Bucket {
    tokens: f64,
    at: Instant,
}

pub struct Guard {
    buckets: Mutex<HashMap<(IpAddr, Class), Bucket>>,
    inflight: Semaphore,
    /// Open WebSocket subscriptions.
    pub ws: Arc<Semaphore>,
}

const MAX_INFLIGHT: usize = 512;
const MAX_WEBSOCKETS: usize = 256;
const MAX_CLIENTS: usize = 100_000;

impl Default for Guard {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(HashMap::new()),
            inflight: Semaphore::new(MAX_INFLIGHT),
            ws: Arc::new(Semaphore::new(MAX_WEBSOCKETS)),
        }
    }
}

/// The client a request comes from, or None for this machine's own software.
pub fn client(peer: SocketAddr, headers: &axum::http::HeaderMap) -> Option<IpAddr> {
    let ip = if peer.ip().is_loopback() {
        // The proxy always sets the header; if it can't be read, all such requests share
        // one budget rather than being treated as this machine's own software.
        let h = headers.get("x-real-ip")?;
        h.to_str()
            .ok()
            .and_then(|s| s.trim().parse::<IpAddr>().ok())
            .unwrap_or(IpAddr::V4(std::net::Ipv4Addr::UNSPECIFIED))
    } else {
        peer.ip()
    };
    Some(match ip {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    })
}

impl Guard {
    /// Takes one request from the client's budget for `class`.
    pub fn allow(&self, ip: IpAddr, class: Class) -> bool {
        let (per_min, burst) = class.budget();
        let now = Instant::now();
        let mut b = self.buckets.lock();
        if b.len() >= MAX_CLIENTS {
            // Forget clients whose budget has refilled; they lose nothing.
            b.retain(|_, x| now.duration_since(x.at).as_secs() < 120);
            if b.len() >= MAX_CLIENTS {
                return false;
            }
        }
        let e = b.entry((ip, class)).or_insert(Bucket {
            tokens: burst,
            at: now,
        });
        let refill = now.duration_since(e.at).as_secs_f64() * per_min / 60.0;
        e.tokens = (e.tokens + refill).min(burst);
        e.at = now;
        if e.tokens >= 1.0 {
            e.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

fn busy(m: &str) -> Response {
    (
        StatusCode::TOO_MANY_REQUESTS,
        [("retry-after", "10")],
        axum::Json(serde_json::json!({ "error": m })),
    )
        .into_response()
}

pub async fn limit(
    axum::extract::State(g): axum::extract::State<Arc<Guard>>,
    ConnectInfo(peer): ConnectInfo<SocketAddr>,
    req: Request,
    next: Next,
) -> Response {
    let Some(ip) = client(peer, req.headers()) else {
        return next.run(req).await;
    };
    let class = Class::of(req.method(), req.uri().path());
    if !g.allow(ip, class) {
        return busy("too many requests from your address; slow down");
    }
    let Ok(_permit) = g.inflight.try_acquire() else {
        return busy("the node is busy; try again shortly");
    };
    next.run(req).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn local_software_is_not_limited_but_proxied_clients_are() {
        let h = axum::http::HeaderMap::new();
        assert_eq!(client("127.0.0.1:5000".parse().unwrap(), &h), None);
        let mut h2 = axum::http::HeaderMap::new();
        h2.insert("x-real-ip", "203.0.113.7".parse().unwrap());
        assert_eq!(
            client("127.0.0.1:5000".parse().unwrap(), &h2),
            Some("203.0.113.7".parse().unwrap())
        );
        // A remote peer cannot pick its identity with the header.
        assert_eq!(
            client("198.51.100.1:5000".parse().unwrap(), &h2),
            Some("198.51.100.1".parse().unwrap())
        );
    }

    #[test]
    fn ipv6_clients_are_grouped_by_64() {
        let a = client(
            "[2001:db8:1:2:3:4:5:6]:1".parse().unwrap(),
            &Default::default(),
        );
        let b = client(
            "[2001:db8:1:2:ffff::1]:1".parse().unwrap(),
            &Default::default(),
        );
        assert_eq!(a, b);
    }

    #[test]
    fn budgets_refill_and_are_per_class() {
        let g = Guard::default();
        let ip: IpAddr = "203.0.113.9".parse().unwrap();
        let mut n = 0;
        while g.allow(ip, Class::Address) {
            n += 1;
        }
        assert_eq!(n, 5);
        assert!(g.allow(ip, Class::Read));
        assert_eq!(Class::of(&Method::POST, "/v1/tx"), Class::Tx);
        assert_eq!(Class::of(&Method::GET, "/v1/subtree/3"), Class::Scan);
        assert_eq!(Class::of(&Method::GET, "/v1/info"), Class::Read);
    }
}

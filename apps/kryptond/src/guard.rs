//! Per-client request budgets and a global concurrency cap for the HTTP API.
//!
//! Clients are told apart by IP (IPv6 by /64). Requests that reach the node from a reverse
//! proxy on this machine carry the real client in `X-Real-IP`; local requests without it
//! (this computer's wallet and miner) are not limited.
//!
//! Besides each client's budget, the network it belongs to (IPv4 by /24, IPv6 by /48) has
//! a larger one, so a block of addresses counts as a few clients, not thousands. A full
//! table of clients refuses only new ones; each client has at most a few requests in
//! flight at once.

use axum::extract::{ConnectInfo, Request};
use axum::http::{Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
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
    /// Swap mailboxes and coin offers.
    Board,
    /// Reading the swap board: offers, market, mailboxes, history.
    BoardRead,
    Rpc,
}

impl Class {
    fn budget(self) -> (f64, f64) {
        match self {
            Class::Read => (3000.0, 600.0),
            Class::Scan => (600.0, 120.0),
            Class::Template => (120.0, 30.0),
            // The whole network finds about 32 shares a minute.
            Class::Submit => (240.0, 60.0),
            Class::Tx => (30.0, 10.0),
            Class::Proof => (30.0, 10.0),
            Class::Address => (10.0, 5.0),
            Class::Board => (60.0, 20.0),
            Class::BoardRead => (600.0, 120.0),
            Class::Rpc => (300.0, 60.0),
        }
    }

    /// The budget a JSON-RPC method draws on besides the general one: the same as the
    /// REST endpoint it stands for, so `/rpc` is no way around a limit.
    pub fn rpc_method(method: &str) -> Option<Class> {
        match method {
            "sendrawtransaction" => Some(Class::Tx),
            "getblocktemplate" => Some(Class::Template),
            "submitwork" => Some(Class::Submit),
            _ => None,
        }
    }

    pub fn of(method: &Method, path: &str) -> Class {
        let p = path.trim_end_matches('/');
        match (method == Method::POST, p) {
            (true, "/v1/tx") => Class::Tx,
            (true, "/v1/verify-payment") => Class::Proof,
            (true, "/v1/address")
            | (true, "/v1/swap/offers")
            | (true, "/v1/swap/trade")
            | (true, "/v1/swap/progress") => Class::Address,
            // Sealed swap messages (up to 256 KB each) and coin offers: a swap needs a
            // handful, never hundreds a minute.
            (true, "/v1/swap/inbox")
            | (true, "/v1/swap/coins")
            | (true, "/v1/swap/coin-trade")
            | (true, "/v1/swap/promise")
            | (true, "/v1/swap/promise/release") => Class::Board,
            (true, "/v1/mining/template") => Class::Template,
            (true, "/v1/mining/submit") => Class::Submit,
            (true, _) => Class::Rpc,
            _ if p.starts_with("/v1/swap/") => Class::BoardRead,
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

/// A client's budget, or its network's (level 1).
type Key = (IpAddr, Class, u8);

#[derive(Default)]
struct Buckets {
    map: HashMap<Key, Bucket>,
    /// When the table was last cleared of clients whose budget had refilled.
    swept: Option<Instant>,
}

pub struct Guard {
    buckets: Mutex<Buckets>,
    inflight: Semaphore,
    /// Requests each client has in flight.
    busy: Mutex<HashMap<IpAddr, u32>>,
    /// Open WebSocket subscriptions, in all and per client.
    pub ws: Arc<Semaphore>,
    ws_each: Mutex<HashMap<IpAddr, u32>>,
}

const MAX_INFLIGHT: usize = 512;
const MAX_INFLIGHT_EACH: u32 = 16;
const MAX_WEBSOCKETS: usize = 256;
const MAX_WEBSOCKETS_EACH: u32 = 4;
const MAX_CLIENTS: usize = 100_000;
/// A network's budget is this many clients' worth.
const NETWORK_SHARE: f64 = 8.0;

impl Default for Guard {
    fn default() -> Self {
        Self {
            buckets: Mutex::new(Buckets::default()),
            inflight: Semaphore::new(MAX_INFLIGHT),
            busy: Mutex::new(HashMap::new()),
            ws: Arc::new(Semaphore::new(MAX_WEBSOCKETS)),
            ws_each: Mutex::new(HashMap::new()),
        }
    }
}

/// Who a request is from, for the board's quotas: the client, and the wider network its
/// quotas count by (an IPv6 /48, an IPv4 address). Set by [`limit`] for every request that
/// is not from this machine's own software.
#[derive(Clone, Copy, Debug)]
pub struct ClientKey {
    pub ip: IpAddr,
    pub quota: IpAddr,
}

/// The network a client belongs to for the shared budget: IPv4 by /24, IPv6 by /48.
fn network(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            IpAddr::V4(Ipv4Addr::new(o[0], o[1], o[2], 0))
        }
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], 0, 0, 0, 0, 0))
        }
    }
}

/// The key the board's quotas count by: an IPv4 address, an IPv6 /48.
fn quota_key(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(_) => network(ip),
        v4 => v4,
    }
}

/// The client a request comes from, or None for this machine's own software.
pub fn client(peer: SocketAddr, headers: &axum::http::HeaderMap) -> Option<IpAddr> {
    // An IPv4 client seen through a dual-stack socket is that IPv4 client.
    let ip = if peer.ip().to_canonical().is_loopback() {
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
    Some(match ip.to_canonical() {
        IpAddr::V6(v6) => {
            let s = v6.segments();
            IpAddr::V6(Ipv6Addr::new(s[0], s[1], s[2], s[3], 0, 0, 0, 0))
        }
        v4 => v4,
    })
}

impl Guard {
    /// Takes one request from the client's budget for `class`, and from its network's.
    pub fn allow(&self, ip: IpAddr, class: Class) -> bool {
        let (per_min, burst) = class.budget();
        let now = Instant::now();
        let mut b = self.buckets.lock();
        let keys = [(ip, class, 0u8), (network(ip), class, 1u8)];
        let new = keys.iter().filter(|k| !b.map.contains_key(*k)).count();
        if new > 0 && b.map.len() + new > MAX_CLIENTS {
            // Forget clients whose budget has refilled (they lose nothing), at most once a
            // second. Clients already known are never refused because the table is full.
            if b.swept.is_none_or(|s| now.duration_since(s).as_secs() >= 1) {
                b.swept = Some(now);
                b.map
                    .retain(|_, x| now.duration_since(x.at).as_secs() < 120);
            }
            if b.map.len() + new > MAX_CLIENTS {
                return false;
            }
        }
        let mut ok = true;
        for (k, share) in [(keys[0], 1.0), (keys[1], NETWORK_SHARE)] {
            let (rate, cap) = (per_min * share, burst * share);
            let e = b.map.entry(k).or_insert(Bucket {
                tokens: cap,
                at: now,
            });
            let refill = now.duration_since(e.at).as_secs_f64() * rate / 60.0;
            e.tokens = (e.tokens + refill).min(cap);
            e.at = now;
            ok &= e.tokens >= 1.0;
        }
        if ok {
            for k in keys {
                if let Some(e) = b.map.get_mut(&k) {
                    e.tokens -= 1.0;
                }
            }
        }
        ok
    }

    /// One more request in flight for `ip`, unless it has its share already.
    fn start(self: &Arc<Self>, ip: IpAddr) -> Option<Busy> {
        let mut m = self.busy.lock();
        let n = m.entry(ip).or_insert(0);
        if *n >= MAX_INFLIGHT_EACH {
            return None;
        }
        *n += 1;
        Some(Busy {
            g: self.clone(),
            ip,
        })
    }
}

impl Guard {
    /// One more WebSocket subscription for `ip`, unless it has its share already.
    pub fn ws_slot(self: &Arc<Self>, ip: IpAddr) -> Option<WsSlot> {
        let mut m = self.ws_each.lock();
        let n = m.entry(ip).or_insert(0);
        if *n >= MAX_WEBSOCKETS_EACH {
            return None;
        }
        *n += 1;
        Some(WsSlot {
            g: self.clone(),
            ip,
        })
    }
}

/// An open subscription; its client's count goes down when it closes.
pub struct WsSlot {
    g: Arc<Guard>,
    ip: IpAddr,
}

impl Drop for WsSlot {
    fn drop(&mut self) {
        let mut m = self.g.ws_each.lock();
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
        }
    }
}

/// A request in flight; its client's count goes down when it is done.
struct Busy {
    g: Arc<Guard>,
    ip: IpAddr,
}

impl Drop for Busy {
    fn drop(&mut self) {
        let mut m = self.g.busy.lock();
        if let Some(n) = m.get_mut(&self.ip) {
            *n -= 1;
            if *n == 0 {
                m.remove(&self.ip);
            }
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
    mut req: Request,
    next: Next,
) -> Response {
    let Some(ip) = client(peer, req.headers()) else {
        return next.run(req).await;
    };
    let class = Class::of(req.method(), req.uri().path());
    if !g.allow(ip, class) {
        return busy("too many requests from your address; slow down");
    }
    let Some(_mine) = g.start(ip) else {
        return busy("too many requests at once from your address; slow down");
    };
    let Ok(_permit) = g.inflight.try_acquire() else {
        return busy("the node is busy; try again shortly");
    };
    req.extensions_mut().insert(ClientKey {
        ip,
        quota: quota_key(ip),
    });
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
    fn mapped_ipv4_clients_are_themselves() {
        let mut h = axum::http::HeaderMap::new();
        h.insert("x-real-ip", "203.0.113.9".parse().unwrap());
        assert_eq!(
            client("[::ffff:127.0.0.1]:5000".parse().unwrap(), &h),
            Some("203.0.113.9".parse().unwrap()),
            "the proxy over a dual-stack socket"
        );
        assert_eq!(
            client(
                "[::ffff:198.51.100.4]:1".parse().unwrap(),
                &Default::default()
            ),
            Some("198.51.100.4".parse().unwrap())
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
    fn writes_have_their_own_budgets() {
        let post = Method::POST;
        assert!(matches!(Class::of(&post, "/v1/tx"), Class::Tx));
        for p in [
            "/v1/swap/inbox",
            "/v1/swap/coins",
            "/v1/swap/coin-trade",
            "/v1/swap/promise",
            "/v1/swap/promise/release",
        ] {
            assert!(matches!(Class::of(&post, p), Class::Board), "{p}");
        }
        assert!(matches!(
            Class::rpc_method("sendrawtransaction"),
            Some(Class::Tx)
        ));
        assert!(matches!(
            Class::rpc_method("getblocktemplate"),
            Some(Class::Template)
        ));
        assert!(matches!(
            Class::rpc_method("submitwork"),
            Some(Class::Submit)
        ));
        assert!(Class::rpc_method("getinfo").is_none());
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
        assert_eq!(Class::of(&Method::GET, "/v1/swap/offers"), Class::BoardRead);
        assert_eq!(
            Class::of(&Method::GET, "/v1/swap/history"),
            Class::BoardRead
        );
    }

    #[test]
    fn a_network_of_addresses_shares_one_larger_budget() {
        let g = Guard::default();
        // 64 addresses of one /48 each spend their own budget; together they hit the
        // network's (8 clients' worth) long before 64 budgets are used up.
        let mut n = 0;
        for k in 0..64u16 {
            let ip: IpAddr = format!("2001:db8:7:{k:x}::1").parse().unwrap();
            while g.allow(ip, Class::Address) {
                n += 1;
            }
        }
        assert_eq!(n, 40, "eight clients' worth of the Address burst (5 each)");
        // Another network is not affected.
        assert!(g.allow("2001:db8:8::1".parse().unwrap(), Class::Address));
    }

    #[test]
    fn a_full_table_refuses_only_new_clients() {
        let g = Guard::default();
        let known: IpAddr = "198.51.100.7".parse().unwrap();
        assert!(g.allow(known, Class::Read));
        {
            let mut b = g.buckets.lock();
            let now = Instant::now();
            for k in 0..MAX_CLIENTS as u32 {
                let ip = IpAddr::V4(Ipv4Addr::from(0x0a00_0000 + k));
                b.map.insert(
                    (ip, Class::Read, 0),
                    Bucket {
                        tokens: 0.0,
                        at: now,
                    },
                );
            }
            b.swept = Some(now);
        }
        assert!(
            g.allow(known, Class::Read),
            "a known client keeps its budget"
        );
        assert!(
            !g.allow("203.0.113.200".parse().unwrap(), Class::Read),
            "a new one waits"
        );
    }

    #[test]
    fn a_client_has_a_few_requests_in_flight_at_most() {
        let g = Arc::new(Guard::default());
        let ip: IpAddr = "203.0.113.5".parse().unwrap();
        let held: Vec<_> = (0..MAX_INFLIGHT_EACH)
            .map(|_| g.start(ip).unwrap())
            .collect();
        assert!(g.start(ip).is_none());
        assert!(g.start("203.0.113.6".parse().unwrap()).is_some());
        drop(held);
        let again = g.start(ip);
        assert!(again.is_some() && g.busy.lock().len() == 1);
        drop(again);
        assert!(g.busy.lock().is_empty());
    }
}

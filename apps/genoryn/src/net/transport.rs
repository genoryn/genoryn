//! Centralized outbound P2P transport. Every peer connection is opened here,
//! after `TransportPolicy::route`; there is no other dialing path and no fallback
//! from a failed Tor route to a direct one.
use super::{
    PeerStream,
    addr::PeerAddr,
    policy::{NetworkMode, PolicyViolation, Route, TransportPolicy},
    socks::SocksError,
    tor::TorState,
};
use serde::Serialize;
use std::{
    fmt,
    net::SocketAddr,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};
use tokio::net::TcpStream;

pub const DIRECT_TIMEOUT: Duration = Duration::from_secs(3);
/// First contact with an onion service commonly takes 5–30 seconds.
pub const TOR_TIMEOUT: Duration = Duration::from_secs(45);

/// How a connection reaches its peer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Link {
    /// Direct TCP; the peer sees this node's IP address.
    Clearnet,
    /// Outbound through the Tor SOCKS proxy.
    Tor,
    /// Inbound through this node's onion service.
    OnionInbound,
}
impl Link {
    pub fn via_tor(self) -> bool {
        self != Self::Clearnet
    }
    pub fn name(self) -> &'static str {
        match self {
            Self::Clearnet => "clearnet",
            Self::Tor => "tor",
            Self::OnionInbound => "onion-inbound",
        }
    }
}

#[derive(Debug)]
pub enum ConnectError {
    Policy(PolicyViolation),
    /// Tor is not configured, not reachable or still bootstrapping. Nothing was dialed.
    TorUnavailable,
    Socks(SocksError),
    /// The built-in Tor client could not open the stream.
    BuiltIn {
        local: bool,
        message: String,
    },
    Direct(std::io::Error),
    Timeout,
}
impl ConnectError {
    /// Failures that say nothing about the peer itself, so they must not count
    /// toward forgetting its address.
    pub fn is_local(&self) -> bool {
        match self {
            Self::TorUnavailable | Self::Policy(_) => true,
            Self::Socks(error) => error.proxy_down(),
            Self::BuiltIn { local, .. } => *local,
            _ => false,
        }
    }
}
impl fmt::Display for ConnectError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Policy(violation) => write!(f, "transport policy: {violation}"),
            Self::TorUnavailable => f.write_str("Tor is not ready; connection not attempted"),
            Self::Socks(error) => write!(f, "{error}"),
            Self::BuiltIn { message, .. } => write!(f, "built-in Tor: {message}"),
            Self::Direct(error) => write!(f, "direct connection failed: {}", error.kind()),
            Self::Timeout => f.write_str("connection timed out"),
        }
    }
}
impl std::error::Error for ConnectError {}

#[derive(Default)]
pub struct TransportCounters {
    pub direct_attempts: AtomicU64,
    pub tor_attempts: AtomicU64,
    pub tor_failures: AtomicU64,
    pub tor_unavailable: AtomicU64,
    pub policy_refusals: AtomicU64,
}
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize)]
pub struct CounterSnapshot {
    pub direct_attempts: u64,
    pub tor_attempts: u64,
    pub tor_failures: u64,
    pub tor_unavailable: u64,
    pub policy_refusals: u64,
}
impl TransportCounters {
    pub fn snapshot(&self) -> CounterSnapshot {
        CounterSnapshot {
            direct_attempts: self.direct_attempts.load(Ordering::Relaxed),
            tor_attempts: self.tor_attempts.load(Ordering::Relaxed),
            tor_failures: self.tor_failures.load(Ordering::Relaxed),
            tor_unavailable: self.tor_unavailable.load(Ordering::Relaxed),
            policy_refusals: self.policy_refusals.load(Ordering::Relaxed),
        }
    }
}

pub struct Connector {
    policy: TransportPolicy,
    tor: Option<Arc<TorState>>,
    pub counters: TransportCounters,
    direct_timeout: Duration,
    tor_timeout: Duration,
}
impl Connector {
    pub fn new(policy: TransportPolicy, tor: Option<Arc<TorState>>) -> Self {
        Self {
            policy,
            tor,
            counters: TransportCounters::default(),
            direct_timeout: DIRECT_TIMEOUT,
            tor_timeout: TOR_TIMEOUT,
        }
    }
    pub fn with_timeouts(mut self, direct: Duration, tor: Duration) -> Self {
        self.direct_timeout = direct;
        self.tor_timeout = tor;
        self
    }
    pub fn policy(&self) -> TransportPolicy {
        self.policy
    }
    pub fn route(&self, target: &PeerAddr) -> Result<Route, PolicyViolation> {
        self.policy.route(target)
    }
    pub fn tor_usable(&self) -> bool {
        self.tor.as_ref().is_some_and(|tor| tor.usable())
    }
    /// Whether dialing `target` now could open a socket at all.
    pub fn may_dial(&self, target: &PeerAddr) -> bool {
        match self.policy.route(target) {
            Ok(Route::Direct(_)) => true,
            Ok(Route::Tor(_)) => self.tor_usable(),
            Err(_) => false,
        }
    }
    pub async fn connect(&self, target: &PeerAddr) -> Result<(PeerStream, Link), ConnectError> {
        let route = self.policy.route(target).map_err(|violation| {
            self.counters
                .policy_refusals
                .fetch_add(1, Ordering::Relaxed);
            ConnectError::Policy(violation)
        })?;
        match route {
            Route::Direct(address) => self.direct(address).await,
            Route::Tor(target) => {
                let Some(tor) = self.tor.as_ref().filter(|tor| tor.usable()) else {
                    self.counters
                        .tor_unavailable
                        .fetch_add(1, Ordering::Relaxed);
                    return Err(ConnectError::TorUnavailable);
                };
                self.counters.tor_attempts.fetch_add(1, Ordering::Relaxed);
                match tor.connect(&target, self.tor_timeout).await {
                    Ok(stream) => Ok((stream, Link::Tor)),
                    Err(error) => {
                        self.counters.tor_failures.fetch_add(1, Ordering::Relaxed);
                        Err(error)
                    }
                }
            }
        }
    }
    async fn direct(&self, address: SocketAddr) -> Result<(PeerStream, Link), ConnectError> {
        // `route` cannot produce this in tor-only mode; refuse anyway.
        if self.policy.mode == NetworkMode::TorOnly {
            self.counters
                .policy_refusals
                .fetch_add(1, Ordering::Relaxed);
            return Err(ConnectError::TorUnavailable);
        }
        self.counters
            .direct_attempts
            .fetch_add(1, Ordering::Relaxed);
        match tokio::time::timeout(self.direct_timeout, TcpStream::connect(address)).await {
            Ok(Ok(stream)) => Ok((super::tcp_stream(stream), Link::Clearnet)),
            Ok(Err(error)) => Err(ConnectError::Direct(error)),
            Err(_) => Err(ConnectError::Timeout),
        }
    }
}

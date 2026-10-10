//! The single transport policy that decides how each peer address may be reached.
//! Every outbound P2P connection is routed through `TransportPolicy::route`.
use super::addr::{PeerAddr, is_onion_name};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};
use std::{fmt, net::SocketAddr, str::FromStr};

/// How this node uses the network. Network privacy only concerns who can see
/// this node's IP address; on-chain confidentiality comes from shielded notes.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum NetworkMode {
    /// Standard TCP peers. Peers see this node's public IP address.
    #[default]
    Direct,
    /// Clearnet peers directly and onion peers through Tor.
    Hybrid,
    /// Every peer connection through Tor; no direct P2P TCP and no local DNS.
    TorOnly,
}
impl NetworkMode {
    pub fn name(self) -> &'static str {
        match self {
            Self::Direct => "direct",
            Self::Hybrid => "hybrid",
            Self::TorOnly => "tor-only",
        }
    }
    pub fn uses_tor(self) -> bool {
        self != Self::Direct
    }
    /// What the operator should know before choosing this mode.
    pub fn privacy_notice(self) -> &'static str {
        match self {
            Self::Direct => {
                "Direct: every peer sees this computer's public IP address and can link it to the transactions it relays first. Shielded notes still hide amounts and addresses."
            }
            Self::Hybrid => {
                "Hybrid: direct clearnet connections reveal this computer's public IP address to those peers; only onion peers are reached through Tor."
            }
            Self::TorOnly => {
                "Tor only: peer connections, peer discovery and transaction broadcasts go through Tor. Tor hides your IP address from peers but cannot defeat a compromised computer or global traffic correlation."
            }
        }
    }
}
impl FromStr for NetworkMode {
    type Err = anyhow::Error;
    fn from_str(text: &str) -> Result<Self> {
        match text {
            "direct" => Ok(Self::Direct),
            "hybrid" => Ok(Self::Hybrid),
            "tor-only" => Ok(Self::TorOnly),
            _ => anyhow::bail!("network mode must be direct, hybrid or tor-only"),
        }
    }
}
impl fmt::Display for NetworkMode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.name())
    }
}

/// The destination a SOCKS5 proxy is asked to reach. Names are passed to the
/// proxy verbatim (SOCKS5 ATYP 3), so the proxy, not this host, resolves them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TorTarget {
    Ip(SocketAddr),
    Domain(String, u16),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Route {
    Direct(SocketAddr),
    Tor(TorTarget),
}
impl Route {
    pub fn uses_tor(&self) -> bool {
        matches!(self, Self::Tor(_))
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PolicyViolation {
    /// Onion services are only reachable through Tor.
    OnionNeedsTor,
    /// Host names are resolved before dialing outside Tor-only mode.
    UnresolvedName,
}
impl fmt::Display for PolicyViolation {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::OnionNeedsTor => "onion peers require hybrid or tor-only mode",
            Self::UnresolvedName => "host names must be resolved before a direct connection",
        })
    }
}
impl std::error::Error for PolicyViolation {}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TransportPolicy {
    pub mode: NetworkMode,
}
impl TransportPolicy {
    pub fn new(mode: NetworkMode) -> Self {
        Self { mode }
    }
    /// The only way an outbound P2P route is chosen. Tor-only never yields
    /// `Route::Direct`, whatever address a configuration file or peer supplies.
    pub fn route(&self, target: &PeerAddr) -> std::result::Result<Route, PolicyViolation> {
        match (self.mode, target) {
            (NetworkMode::TorOnly, PeerAddr::Ip(address)) => {
                Ok(Route::Tor(TorTarget::Ip(*address)))
            }
            (_, PeerAddr::Ip(address)) => Ok(Route::Direct(*address)),
            (NetworkMode::Direct, PeerAddr::Onion { .. }) => Err(PolicyViolation::OnionNeedsTor),
            (_, PeerAddr::Onion { key, port }) => {
                Ok(Route::Tor(TorTarget::Domain(key.hostname(), *port)))
            }
            (NetworkMode::TorOnly, PeerAddr::Name { host, port }) => {
                Ok(Route::Tor(TorTarget::Domain(host.clone(), *port)))
            }
            (_, PeerAddr::Name { .. }) => Err(PolicyViolation::UnresolvedName),
        }
    }
    /// Whether this host may ask the operating system's resolver about `host`.
    /// Never in Tor-only mode, and never for an onion name in any mode.
    pub fn may_resolve_locally(&self, host: &str) -> bool {
        self.mode != NetworkMode::TorOnly && !is_onion_name(host)
    }
    /// Whether an address is worth keeping in this mode's discovery cache.
    pub fn reachable(&self, address: &PeerAddr) -> bool {
        match address {
            PeerAddr::Ip(_) => true,
            PeerAddr::Onion { .. } => self.mode.uses_tor(),
            PeerAddr::Name { .. } => false,
        }
    }
    /// Tor-only mode accepts inbound peers only through its onion service. A
    /// clearnet listener is permitted only when bound to loopback (a local lab).
    pub fn validate_clearnet_listener(&self, listen: Option<SocketAddr>) -> Result<()> {
        if self.mode == NetworkMode::TorOnly
            && let Some(listen) = listen
        {
            ensure!(
                listen.ip().is_loopback(),
                "tor-only mode refuses a non-loopback P2P listener; use the onion service for inbound peers"
            );
        }
        Ok(())
    }
    /// Separate discovery caches keep peers learned over one transport from
    /// shaping (and fingerprinting) connections made over another.
    pub fn cache_file(&self) -> &'static str {
        match self.mode {
            NetworkMode::Direct => "peers.json",
            NetworkMode::Hybrid => "peers-hybrid.json",
            NetworkMode::TorOnly => "peers-tor.json",
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::addr::tests::test_onion;

    fn name() -> PeerAddr {
        PeerAddr::Name {
            host: "seed.example.org".into(),
            port: 29610,
        }
    }

    #[test]
    fn tor_only_never_routes_directly() {
        let policy = TransportPolicy::new(NetworkMode::TorOnly);
        let ip: SocketAddr = "203.0.113.5:29610".parse().unwrap();
        assert_eq!(
            policy.route(&PeerAddr::Ip(ip)),
            Ok(Route::Tor(TorTarget::Ip(ip)))
        );
        let onion = test_onion(4);
        assert_eq!(
            policy.route(&PeerAddr::onion(onion, 29610)),
            Ok(Route::Tor(TorTarget::Domain(onion.hostname(), 29610)))
        );
        assert_eq!(
            policy.route(&name()),
            Ok(Route::Tor(TorTarget::Domain(
                "seed.example.org".into(),
                29610
            )))
        );
        for address in [PeerAddr::Ip(ip), PeerAddr::onion(onion, 1), name()] {
            assert!(policy.route(&address).unwrap().uses_tor());
        }
        assert!(!policy.may_resolve_locally("seed.example.org"));
        assert!(policy.validate_clearnet_listener(None).is_ok());
        assert!(
            policy
                .validate_clearnet_listener(Some("127.0.0.1:29610".parse().unwrap()))
                .is_ok()
        );
        assert!(
            policy
                .validate_clearnet_listener(Some("0.0.0.0:29610".parse().unwrap()))
                .is_err()
        );
    }

    #[test]
    fn hybrid_and_direct_route_each_class_by_policy() {
        let ip: SocketAddr = "203.0.113.5:29610".parse().unwrap();
        let onion = PeerAddr::onion(test_onion(5), 29610);
        let hybrid = TransportPolicy::new(NetworkMode::Hybrid);
        assert_eq!(hybrid.route(&PeerAddr::Ip(ip)), Ok(Route::Direct(ip)));
        assert!(hybrid.route(&onion).unwrap().uses_tor());
        assert_eq!(hybrid.route(&name()), Err(PolicyViolation::UnresolvedName));
        assert!(hybrid.reachable(&onion));
        let direct = TransportPolicy::new(NetworkMode::Direct);
        assert_eq!(direct.route(&PeerAddr::Ip(ip)), Ok(Route::Direct(ip)));
        assert_eq!(direct.route(&onion), Err(PolicyViolation::OnionNeedsTor));
        assert!(!direct.reachable(&onion));
        assert!(direct.reachable(&PeerAddr::Ip(ip)));
        for policy in [direct, hybrid] {
            assert!(policy.may_resolve_locally("seed.example.org"));
            assert!(!policy.may_resolve_locally(&test_onion(1).hostname()));
            assert!(!policy.may_resolve_locally("anything.ONION."));
            assert!(!policy.reachable(&name()));
        }
    }

    #[test]
    fn modes_parse_strictly_and_use_separate_caches() {
        assert_eq!(
            "tor-only".parse::<NetworkMode>().unwrap(),
            NetworkMode::TorOnly
        );
        for bad in ["Tor-Only", "tor", "", "onion"] {
            assert!(bad.parse::<NetworkMode>().is_err());
        }
        assert_eq!(NetworkMode::default(), NetworkMode::Direct);
        let files: std::collections::BTreeSet<_> = [
            NetworkMode::Direct,
            NetworkMode::Hybrid,
            NetworkMode::TorOnly,
        ]
        .map(|mode| TransportPolicy::new(mode).cache_file())
        .into_iter()
        .collect();
        assert_eq!(files.len(), 3);
        assert_eq!(
            TransportPolicy::new(NetworkMode::Direct).cache_file(),
            "peers.json"
        );
    }
}

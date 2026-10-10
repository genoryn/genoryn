use anyhow::{Result, ensure};
use serde::Deserialize;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
};

#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct Config {
    pub network: NetworkConfig,
    pub p2p: P2pConfig,
    pub rpc: RpcConfig,
    pub database: DatabaseConfig,
    pub public_rpc: PublicRpcConfig,
    pub mining: MiningConfig,
    pub limits: ResourceLimits,
    pub network_privacy: NetworkPrivacyConfig,
    pub tor: TorFileConfig,
}
/// Network privacy mode. Absent means `direct`, the behavior of earlier
/// releases; Tor modes are never enabled implicitly.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkPrivacyConfig {
    pub mode: Option<crate::net::NetworkMode>,
}
/// Settings for an operator-run Tor daemon. GENORYN never starts or installs Tor.
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct TorFileConfig {
    /// SOCKS port as an IP literal; default 127.0.0.1:9050 (Tor Browser uses 9150).
    pub socks: Option<SocketAddr>,
    /// Control port, needed for bootstrap status and the onion service.
    pub control: Option<SocketAddr>,
    /// File holding the control password (HashedControlPassword). Never logged.
    pub control_password_file: Option<PathBuf>,
    /// Overrides the cookie path Tor reports for SAFECOOKIE authentication.
    pub cookie_file: Option<PathBuf>,
    pub onion_service: bool,
    /// Loopback listener Tor forwards onion connections to; default base port + 3.
    pub onion_listen: Option<SocketAddr>,
    /// Port advertised with the onion address; default the network base port.
    pub onion_port: Option<u16>,
    /// Separate Tor circuits per peer connection; default true.
    pub stream_isolation: Option<bool>,
}
/// Local admission policy. These limits cannot increase consensus maxima.
#[derive(Clone, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ResourceLimits {
    pub connections: usize,
    pub inbound_connections: usize,
    pub outbound_connections: usize,
    pub rpc_concurrency: usize,
    pub rpc_request_bytes: usize,
    pub mempool_transactions: usize,
    pub mempool_bytes: usize,
}
impl Default for ResourceLimits {
    fn default() -> Self {
        Self {
            connections: 24,
            inbound_connections: 16,
            outbound_connections: 8,
            rpc_concurrency: 16,
            rpc_request_bytes: 12_000_000,
            mempool_transactions: genoryn_core::chain::MAX_MEMPOOL_TRANSACTIONS,
            mempool_bytes: genoryn_core::chain::MAX_MEMPOOL_BYTES,
        }
    }
}
impl ResourceLimits {
    /// Inbound peers may use only the slots outbound connections do not need, so
    /// an attacker who fills every inbound slot still cannot block the outbound
    /// peers this node chooses itself (eclipse resistance).
    pub fn effective_inbound(&self) -> usize {
        self.inbound_connections
            .min(self.connections.saturating_sub(self.outbound_connections))
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            (1..=32).contains(&self.connections),
            "connections must be 1..32"
        );
        ensure!(
            self.inbound_connections <= self.connections,
            "inbound limit exceeds total connections"
        );
        ensure!(
            self.outbound_connections <= self.connections.min(8),
            "outbound limit exceeds total connections or 8"
        );
        ensure!(
            self.inbound_connections + self.outbound_connections > 0,
            "at least one peer direction must be enabled"
        );
        ensure!(
            (1..=16).contains(&self.rpc_concurrency),
            "RPC concurrency must be 1..16"
        );
        ensure!(
            (65_536..=12_000_000).contains(&self.rpc_request_bytes),
            "RPC body limit must be 65536..12000000 bytes"
        );
        ensure!(
            (1..=genoryn_core::chain::MAX_MEMPOOL_TRANSACTIONS)
                .contains(&self.mempool_transactions),
            "mempool transaction limit must be 1..256"
        );
        ensure!(
            (genoryn_core::codec::MAX_TX_BYTES..=genoryn_core::chain::MAX_MEMPOOL_BYTES)
                .contains(&self.mempool_bytes),
            "mempool byte limit must be 200000..8000000"
        );
        Ok(())
    }
}
#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct NetworkConfig {
    pub name: String,
}
impl Default for NetworkConfig {
    fn default() -> Self {
        Self {
            name: "mainnet".into(),
        }
    }
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct P2pConfig {
    pub listen: Option<SocketAddr>,
    /// `ip:port`, `[ipv6]:port`, `<v3>.onion:port` or `host:port`.
    pub peers: Vec<String>,
    pub dns_seeds: Vec<String>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct RpcConfig {
    pub admin_listen: Option<SocketAddr>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct DatabaseConfig {
    pub data_dir: Option<PathBuf>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct PublicRpcConfig {
    pub enabled: bool,
    pub listen: Option<SocketAddr>,
}
#[derive(Default, Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct MiningConfig {
    pub enabled: bool,
}
impl Config {
    pub fn load(path: Option<&Path>) -> Result<Self> {
        let c = if let Some(path) = path {
            ensure!(
                std::fs::metadata(path)?.len() <= 65_536,
                "configuration file too large"
            );
            toml::from_str(&std::fs::read_to_string(path)?)?
        } else {
            Self::default()
        };
        ensure!(
            !c.mining.enabled,
            "automatic startup mining is disabled; explicitly run genoryn-miner or use Start Mining"
        );
        ensure!(
            c.p2p.peers.len() <= 128 && c.p2p.dns_seeds.len() <= 16,
            "too many discovery sources"
        );
        c.limits.validate()?;
        Ok(c)
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn strict_configuration() {
        assert!(toml::from_str::<Config>("[rpc]\nadmin_listen = '0.0.0.0:29411'").is_ok());
        assert!(toml::from_str::<Config>("unknown_security_switch = true").is_err());
        assert!(!Config::default().public_rpc.enabled);
        assert!(!Config::default().mining.enabled);
        assert!(
            Config::default().network_privacy.mode.is_none(),
            "Tor modes are never enabled implicitly"
        );
    }
    #[test]
    fn network_privacy_sections_parse_strictly() {
        let config: Config = toml::from_str(
            "[network_privacy]\nmode = 'tor-only'\n[tor]\nsocks = '127.0.0.1:9150'\ncontrol = '127.0.0.1:9151'\nonion_service = true\n[p2p]\npeers = ['203.0.113.1:29610', 'duckduckgogg42xjoc72x3sjasowoarfbgcmvfimaftt6twagswzczad.onion:29610']",
        )
        .unwrap();
        assert_eq!(
            config.network_privacy.mode,
            Some(crate::net::NetworkMode::TorOnly)
        );
        assert_eq!(config.tor.socks, Some("127.0.0.1:9150".parse().unwrap()));
        assert!(config.tor.onion_service);
        assert_eq!(config.p2p.peers.len(), 2);
        for bad in [
            "[network_privacy]\nmode = 'tor'",
            "[tor]\nsocks = 'localhost:9050'",
            "[tor]\ncontrol_password = 'inline secrets are not accepted'",
        ] {
            assert!(toml::from_str::<Config>(bad).is_err(), "{bad}");
        }
        let legacy: Config = toml::from_str("[p2p]\npeers = ['127.0.0.1:29420']").unwrap();
        assert_eq!(legacy.p2p.peers, ["127.0.0.1:29420"]);
    }
    #[test]
    fn shipped_example_configuration_is_valid() {
        let config: Config =
            toml::from_str(include_str!("../../../config/genoryn.example.toml")).unwrap();
        config.limits.validate().unwrap();
        assert_eq!(
            config.network_privacy.mode,
            Some(crate::net::NetworkMode::Direct)
        );
        assert!(!config.tor.onion_service);
        assert_eq!(config.tor.socks, None, "the example uses the built-in Tor");
    }
    #[test]
    fn outbound_slots_are_reserved_from_inbound_peers() {
        let limits = ResourceLimits::default();
        assert_eq!(limits.connections, 24);
        assert_eq!(limits.effective_inbound(), 16);
        assert_eq!(
            limits.effective_inbound() + limits.outbound_connections,
            limits.connections
        );
        let tight: ResourceLimits = toml::from_str::<Config>(
            "[limits]\nconnections=16\ninbound_connections=16\noutbound_connections=8",
        )
        .unwrap()
        .limits;
        assert_eq!(tight.effective_inbound(), 8);
        let inbound_only: ResourceLimits = toml::from_str::<Config>(
            "[limits]\nconnections=1\ninbound_connections=1\noutbound_connections=0",
        )
        .unwrap()
        .limits;
        assert_eq!(inbound_only.effective_inbound(), 1);
    }
    #[test]
    fn operator_limits_are_bounded_without_changing_consensus() {
        assert!(ResourceLimits::default().validate().is_ok());
        for text in [
            "[limits]\nconnections=0",
            "[limits]\nconnections=33",
            "[limits]\ninbound_connections=25",
            "[limits]\noutbound_connections=9",
            "[limits]\ninbound_connections=0\noutbound_connections=0",
            "[limits]\nrpc_concurrency=0",
            "[limits]\nrpc_request_bytes=12000001",
            "[limits]\nmempool_transactions=257",
            "[limits]\nmempool_bytes=199999",
        ] {
            let config: Config = toml::from_str(text).unwrap();
            assert!(config.limits.validate().is_err(), "{text}");
        }
        let config: Config = toml::from_str("[limits]\nconnections=4\ninbound_connections=2\noutbound_connections=2\nrpc_concurrency=4\nmempool_transactions=16\nmempool_bytes=1000000").unwrap();
        assert!(config.limits.validate().is_ok());
    }
}

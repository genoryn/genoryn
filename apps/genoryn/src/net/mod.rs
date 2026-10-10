//! Network privacy: peer addresses, the transport policy, Tor and log redaction.
//!
//! Application-level protections here do not replace operating-system firewall
//! isolation (for example Whonix or Tails), and Tor itself cannot protect against
//! a compromised computer or an adversary who observes both ends of a circuit.
pub mod addr;
pub mod bridge;
pub mod embedded;
pub mod policy;
pub mod redact;
pub mod socks;
pub mod tor;
pub mod transport;

pub use addr::{NetClass, OnionV3, PeerAddr, WireAddr};
pub use policy::{NetworkMode, TransportPolicy};
pub use transport::{Connector, Link};

use anyhow::{Result, ensure};
use genoryn_core::codec::Hash;
use rand08::RngCore;
use std::{future::Future, net::SocketAddr, path::Path, pin::Pin, sync::Arc, time::Duration};
use tor::{TorConfig, TorState};
use zeroize::Zeroizing;

/// Any bidirectional byte stream a peer connection can run over: direct TCP, a
/// SOCKS-proxied TCP stream or a stream of the built-in Tor client.
pub trait AsyncStream: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send {}
impl<T: tokio::io::AsyncRead + tokio::io::AsyncWrite + Send> AsyncStream for T {}
pub type PeerStream = Pin<Box<dyn AsyncStream>>;
pub fn tcp_stream(stream: tokio::net::TcpStream) -> PeerStream {
    let _ = stream.set_nodelay(true);
    Box::pin(stream)
}

/// Operator choices that shape every network path of the node.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct NetworkOptions {
    pub mode: NetworkMode,
    pub tor: Option<TorConfig>,
    /// Clearnet P2P listener; `None` accepts no clearnet inbound peers.
    pub listen: Option<SocketAddr>,
}
impl NetworkOptions {
    pub fn direct(listen: SocketAddr) -> Self {
        Self {
            mode: NetworkMode::Direct,
            tor: None,
            listen: Some(listen),
        }
    }
    pub fn validate(&self) -> Result<()> {
        TransportPolicy::new(self.mode).validate_clearnet_listener(self.listen)?;
        match (&self.tor, self.mode) {
            (None, NetworkMode::Direct) => {}
            (Some(_), NetworkMode::Direct) => {
                anyhow::bail!("Tor settings require hybrid or tor-only network mode")
            }
            (None, _) => anyhow::bail!("hybrid and tor-only modes need a Tor SOCKS proxy"),
            (Some(tor), _) => {
                tor.validate()?;
                ensure!(
                    !tor.onion_service || Some(tor.onion_target) != self.listen,
                    "the onion-service target must differ from the clearnet listener"
                );
            }
        }
        Ok(())
    }
}

/// Network state shared by the P2P layer, RPC status and transaction relay.
pub struct NetContext {
    pub options: NetworkOptions,
    pub policy: TransportPolicy,
    pub connector: Connector,
    pub tor: Option<Arc<TorState>>,
    /// Handshake identity on Tor links, distinct from the clearnet identity and
    /// regenerated every start, so peers cannot link a node across transports.
    pub tor_id: Hash,
    pub redact_logs: bool,
}
impl NetContext {
    pub fn new(options: NetworkOptions, data_dir: &Path) -> Result<Self> {
        options.validate()?;
        let policy = TransportPolicy::new(options.mode);
        let tor = options
            .tor
            .clone()
            .map(|config| TorState::new(config, data_dir.to_path_buf()));
        let mut tor_id = [0u8; 32];
        rand08::rngs::OsRng.fill_bytes(&mut tor_id);
        Ok(Self {
            connector: Connector::new(policy, tor.clone()),
            redact_logs: options.mode.uses_tor(),
            policy,
            tor,
            tor_id,
            options,
        })
    }
    pub fn onion_address(&self) -> Option<PeerAddr> {
        self.tor.as_ref()?.onion_address()
    }
    /// Loopback listener an external Tor forwards onion connections to. The
    /// built-in client delivers onion streams in-process instead.
    pub fn onion_target(&self) -> Option<SocketAddr> {
        self.options
            .tor
            .as_ref()
            .filter(|tor| tor.onion_service && !tor.built_in())
            .map(|tor| tor.onion_target)
    }
    pub fn log_peer<'a>(&self, address: &'a PeerAddr) -> redact::LogPeer<'a> {
        redact::peer(address, self.redact_logs)
    }
    /// SOCKS proxy URL for auxiliary HTTPS (published seed list) in tor-only mode.
    /// For the built-in client it carries the bridge's credentials.
    pub fn auxiliary_proxy(&self) -> Option<Zeroizing<String>> {
        if self.options.mode != NetworkMode::TorOnly {
            return None;
        }
        self.tor.as_ref()?.auxiliary_proxy()
    }
}

/// HTTP client for auxiliary traffic. With a proxy URL (`socks5h://…`), every
/// request goes through it, the proxy resolves host names and no system proxy applies.
pub fn http_client_builder(proxy: Option<&str>) -> Result<reqwest::ClientBuilder> {
    let builder = reqwest::Client::builder();
    Ok(match proxy {
        Some(url) => {
            anyhow::ensure!(
                url.starts_with("socks5h://"),
                "auxiliary proxy must use socks5h"
            );
            builder.proxy(reqwest::Proxy::all(url)?)
        }
        None => builder.no_proxy(),
    })
}

/// Turns operator bootstrap entries (IP literals, host names, onion names) into
/// dial targets. `resolve` is the system resolver: it is never called in
/// tor-only mode (names are dialed through Tor with remote resolution) and never
/// for an onion name in any mode.
pub async fn bootstrap_targets<F, Fut>(
    policy: TransportPolicy,
    seeds: &[String],
    port: u16,
    resolve: F,
) -> Vec<PeerAddr>
where
    F: Fn(String, u16) -> Fut,
    Fut: Future<Output = std::io::Result<Vec<SocketAddr>>>,
{
    let mut targets = Vec::new();
    for seed in seeds {
        match PeerAddr::parse_with_default_port(seed, port) {
            Ok(address @ PeerAddr::Ip(_)) => targets.push(address),
            Ok(address @ PeerAddr::Onion { .. }) if policy.reachable(&address) => {
                targets.push(address)
            }
            Ok(PeerAddr::Onion { .. }) => {
                tracing::warn!(
                    "onion bootstrap entry skipped: direct mode cannot reach onion peers"
                )
            }
            Ok(address @ PeerAddr::Name { .. }) if policy.mode == NetworkMode::TorOnly => {
                targets.push(address)
            }
            Ok(PeerAddr::Name { host, port }) if policy.may_resolve_locally(&host) => {
                match tokio::time::timeout(Duration::from_secs(5), resolve(host.clone(), port))
                    .await
                {
                    Ok(Ok(addresses)) => {
                        targets.extend(addresses.into_iter().take(16).map(PeerAddr::from))
                    }
                    _ => {
                        tracing::warn!(seed = %host, "DNS seed unavailable; other discovery continues")
                    }
                }
            }
            Ok(PeerAddr::Name { .. }) => {}
            Err(error) => tracing::warn!(error = %error, "invalid bootstrap entry skipped"),
        }
    }
    targets
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::net::addr::tests::test_onion;
    use std::sync::{Arc, Mutex};

    fn recording_resolver(
        calls: Arc<Mutex<Vec<String>>>,
    ) -> impl Fn(String, u16) -> std::future::Ready<std::io::Result<Vec<SocketAddr>>> {
        move |host, port| {
            calls.lock().unwrap().push(host);
            std::future::ready(Ok(vec![SocketAddr::from(([198, 51, 100, 4], port))]))
        }
    }

    #[tokio::test]
    async fn tor_only_bootstrap_never_uses_the_system_resolver() {
        let onion = test_onion(2).hostname();
        let seeds = vec![
            "seed.example.org".to_owned(),
            onion.clone(),
            "203.0.113.8".to_owned(),
        ];
        let calls = Arc::new(Mutex::new(Vec::new()));
        let targets = bootstrap_targets(
            TransportPolicy::new(NetworkMode::TorOnly),
            &seeds,
            29610,
            recording_resolver(calls.clone()),
        )
        .await;
        assert!(
            calls.lock().unwrap().is_empty(),
            "no local DNS in tor-only mode"
        );
        assert_eq!(targets.len(), 3);
        assert!(targets.contains(&PeerAddr::Name {
            host: "seed.example.org".into(),
            port: 29610
        }));
        assert!(targets.contains(&PeerAddr::onion(test_onion(2), 29610)));
    }

    #[tokio::test]
    async fn onion_names_never_reach_the_resolver_in_any_mode() {
        let seeds = vec![test_onion(6).hostname(), "seed.example.org".to_owned()];
        for mode in [NetworkMode::Direct, NetworkMode::Hybrid] {
            let calls = Arc::new(Mutex::new(Vec::new()));
            let targets = bootstrap_targets(
                TransportPolicy::new(mode),
                &seeds,
                29610,
                recording_resolver(calls.clone()),
            )
            .await;
            assert_eq!(*calls.lock().unwrap(), ["seed.example.org"]);
            assert!(targets.contains(&PeerAddr::Ip("198.51.100.4:29610".parse().unwrap())));
            assert_eq!(
                targets.contains(&PeerAddr::onion(test_onion(6), 29610)),
                mode == NetworkMode::Hybrid
            );
        }
    }

    #[test]
    fn auxiliary_clients_accept_only_remote_resolution_proxies() {
        assert!(http_client_builder(Some("socks5h://127.0.0.1:9050")).is_ok());
        assert!(http_client_builder(Some("socks5h://genoryn:secret@127.0.0.1:41000")).is_ok());
        assert!(
            http_client_builder(Some("socks5://127.0.0.1:9050")).is_err(),
            "socks5 would resolve names locally"
        );
        assert!(http_client_builder(None).is_ok());
    }

    #[test]
    fn options_validate_mode_and_listener_combinations() {
        let tor = TorConfig::new(None, 29610);
        assert!(
            NetworkOptions::direct("0.0.0.0:29610".parse().unwrap())
                .validate()
                .is_ok()
        );
        let tor_only = NetworkOptions {
            mode: NetworkMode::TorOnly,
            tor: Some(tor.clone()),
            listen: None,
        };
        assert!(tor_only.validate().is_ok());
        assert!(
            NetworkOptions {
                listen: Some("0.0.0.0:29610".parse().unwrap()),
                ..tor_only.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            NetworkOptions {
                tor: None,
                ..tor_only.clone()
            }
            .validate()
            .is_err()
        );
        assert!(
            NetworkOptions {
                mode: NetworkMode::Direct,
                ..tor_only
            }
            .validate()
            .is_err()
        );
    }
}

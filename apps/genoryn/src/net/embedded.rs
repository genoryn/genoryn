//! Built-in Tor: Arti, the Tor Project's Rust implementation of Tor, linked into
//! the node so users install and run nothing else. No Tor program is downloaded
//! or executed; the client fetches only Tor network directory documents and keeps
//! them, its guard state and (with an onion service) its identity keys under
//! `<data dir>/tor`.
use super::{PeerStream, policy::TorTarget};
use anyhow::{Context, Result};
use arti_client::{
    BootstrapBehavior, ErrorKind, HasKind, StreamPrefs, TorAddr, TorClient,
    config::{BoolOrAuto, TorClientConfigBuilder},
};
use futures::StreamExt;
use safelog::DisplayRedacted;
use std::{net::SocketAddr, path::Path, sync::Arc};
use tokio::sync::mpsc;
use tor_rtcompat::PreferredRuntime;

/// Nickname that selects the onion service's persistent identity in the keystore.
const SERVICE_NICKNAME: &str = "genoryn-p2p";
/// Concurrent light-wallet RPC streams through the onion service; the public
/// RPC applies its own request limits behind this.
const MAX_ONION_RPC_STREAMS: usize = 16;

pub struct EmbeddedTor {
    client: Arc<TorClient<PreferredRuntime>>,
}

/// Failure of a connection through the built-in client.
#[derive(Debug)]
pub struct EmbeddedError {
    /// The failure is about this machine's Tor access, not about the peer.
    pub local: bool,
    pub message: String,
}

fn classify(error: &arti_client::Error) -> EmbeddedError {
    let local = matches!(
        error.kind(),
        ErrorKind::TorAccessFailed
            | ErrorKind::BootstrapRequired
            | ErrorKind::DirectoryExpired
            | ErrorKind::LocalNetworkError
            | ErrorKind::LocalResourceExhausted
            | ErrorKind::TorNetworkTimeout
            | ErrorKind::TorDirectoryError
            | ErrorKind::TorDirectoryUnusable
            | ErrorKind::ClockSkew
            | ErrorKind::NoPath
            | ErrorKind::ArtiShuttingDown
            | ErrorKind::ReactorShuttingDown
            | ErrorKind::TransientFailure
            | ErrorKind::Internal
    );
    EmbeddedError {
        local,
        message: format!("{:?}", error.kind()),
    }
}

impl EmbeddedTor {
    /// Creates the client without network activity. Must run inside the Tokio runtime.
    pub fn create(data_dir: &Path) -> Result<Arc<Self>> {
        let root = data_dir.join("tor");
        let state = root.join("state");
        let cache = root.join("cache");
        for directory in [&root, &state, &cache] {
            std::fs::create_dir_all(directory)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(directory, std::fs::Permissions::from_mode(0o700))?;
            }
        }
        let config = TorClientConfigBuilder::from_directories(&state, &cache)
            .build()
            .context("built-in Tor configuration")?;
        let client = TorClient::builder()
            .config(config)
            .bootstrap_behavior(BootstrapBehavior::Manual)
            .create_unbootstrapped()
            .context("built-in Tor could not start")?;
        Ok(Arc::new(Self { client }))
    }
    pub async fn bootstrap(&self) -> Result<(), EmbeddedError> {
        self.client
            .bootstrap()
            .await
            .map_err(|error| classify(&error))
    }
    pub fn ready(&self) -> bool {
        self.client.bootstrap_status().ready_for_traffic()
    }
    /// Progress 0–100 and a short human-readable summary.
    pub fn progress(&self) -> (u8, String) {
        let status = self.client.bootstrap_status();
        let percent = (status.as_frac().clamp(0.0, 1.0) * 100.0).round() as u8;
        let summary = match status.blocked() {
            Some(blockage) => format!("Blocked: {}", blockage.message()),
            None => status.to_string(),
        };
        (percent, summary.chars().take(160).collect())
    }
    pub fn events(&self) -> arti_client::status::BootstrapEvents {
        self.client.bootstrap_events()
    }
    /// Opens a stream through Tor. Peer IP addresses come from configuration or
    /// peer exchange, never from local DNS, so they are passed as addresses;
    /// names are resolved by the exit or reached as onion services.
    pub async fn connect(
        &self,
        target: &TorTarget,
        isolate: bool,
    ) -> Result<PeerStream, EmbeddedError> {
        let mut prefs = StreamPrefs::new();
        prefs.connect_to_onion_services(BoolOrAuto::Explicit(true));
        if isolate {
            prefs.new_isolation_group();
        }
        let stream = match target {
            TorTarget::Ip(address) => {
                let address = TorAddr::dangerously_from(*address).map_err(|_| EmbeddedError {
                    local: false,
                    message: "address not usable through Tor".into(),
                })?;
                self.client.connect_with_prefs(address, &prefs).await
            }
            TorTarget::Domain(host, port) => {
                self.client
                    .connect_with_prefs((host.as_str(), *port), &prefs)
                    .await
            }
        };
        stream
            .map(|stream| Box::pin(stream) as PeerStream)
            .map_err(|error| classify(&error))
    }
    /// Publishes this node's onion service. Its identity key is created on first
    /// use and kept in the built-in client's keystore, so the address survives
    /// restarts. Accepted streams for `port` are delivered to `inbound`; with
    /// `rpc = (virtual port, local listener)` streams on that port are forwarded
    /// to the local public RPC; other ports close the circuit.
    pub fn launch_onion_service(
        &self,
        port: u16,
        inbound: mpsc::Sender<PeerStream>,
        rpc: Option<(u16, SocketAddr)>,
    ) -> Result<OnionHost> {
        let nickname = tor_hsservice::HsNickname::new(SERVICE_NICKNAME.into())
            .map_err(|_| anyhow::anyhow!("invalid onion service nickname"))?;
        let config = tor_hsservice::config::OnionServiceConfigBuilder::default()
            .nickname(nickname)
            .build()
            .context("onion service configuration")?;
        let (service, requests) = self
            .client
            .launch_onion_service(config)
            .context("onion service could not start")?
            .context("onion service disabled")?;
        let rpc_slots = Arc::new(tokio::sync::Semaphore::new(MAX_ONION_RPC_STREAMS));
        tokio::spawn(async move {
            let mut streams = std::pin::pin!(tor_hsservice::handle_rend_requests(requests));
            while let Some(request) = streams.next().await {
                let requested = match request.request() {
                    tor_proto::stream::IncomingStreamRequest::Begin(begin) => Some(begin.port()),
                    _ => None,
                };
                let rpc_target = rpc
                    .filter(|(rpc_port, _)| requested == Some(*rpc_port))
                    .map(|(_, target)| target);
                if let Some(target) = rpc_target {
                    let Ok(slot) = rpc_slots.clone().try_acquire_owned() else {
                        let _ = request.shutdown_circuit();
                        continue;
                    };
                    if let Ok(mut stream) = request
                        .accept(tor_cell::relaycell::msg::Connected::new_empty())
                        .await
                    {
                        tokio::spawn(async move {
                            let _slot = slot;
                            if let Ok(mut local) = tokio::net::TcpStream::connect(target).await {
                                let _ =
                                    tokio::io::copy_bidirectional(&mut stream, &mut local).await;
                            }
                        });
                    }
                    continue;
                }
                if requested != Some(port) {
                    let _ = request.shutdown_circuit();
                    continue;
                }
                if let Ok(stream) = request
                    .accept(tor_cell::relaycell::msg::Connected::new_empty())
                    .await
                {
                    // A full queue drops the stream: inbound slots are bounded downstream.
                    let _ = inbound.try_send(Box::pin(stream) as PeerStream);
                }
            }
        });
        Ok(OnionHost { service })
    }
}

pub struct OnionHost {
    service: Arc<tor_hsservice::RunningOnionService>,
}
impl OnionHost {
    /// The full `<56 characters>.onion` name.
    pub fn hostname(&self) -> Option<String> {
        self.service
            .onion_address()
            .map(|id| id.display_unredacted().to_string())
    }
    /// `registered` once reachable, `starting` while publishing, else `failed`.
    pub fn state(&self) -> (&'static str, Option<String>) {
        use tor_hsservice::status::State;
        let status = self.service.status();
        match status.state() {
            State::Running | State::DegradedReachable => ("registered", None),
            State::Bootstrapping => ("starting", None),
            other => (
                "failed",
                Some(
                    format!("onion service {other:?}")
                        .chars()
                        .take(160)
                        .collect(),
                ),
            ),
        }
    }
}

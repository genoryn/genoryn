use anyhow::{Result, ensure};
use clap::Parser;
use genoryn::net::{NetworkMode, NetworkOptions, PeerAddr, TransportPolicy, tor::TorConfig};
use genoryn_core::params::Network;
use std::{net::SocketAddr, path::PathBuf};
#[derive(Parser)]
#[command(
    name = "genorynd",
    about = "GENORYN experimental private PoW full node"
)]
struct Args {
    #[arg(long)]
    network: Option<String>,
    #[arg(long)]
    data_dir: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    p2p_listen: Option<SocketAddr>,
    #[arg(long)]
    rpc_listen: Option<SocketAddr>,
    /// Explicit peer: IP:PORT, [IPV6]:PORT, <v3 onion>.onion:PORT or HOST:PORT.
    #[arg(long)]
    peer: Vec<String>,
    #[arg(long)]
    dns_seed: Vec<String>,
    /// Do not contact the built-in bootstrap seeds (always skipped on loopback).
    #[arg(long)]
    no_default_seeds: bool,
    #[arg(long)]
    public_rpc: bool,
    #[arg(long)]
    public_rpc_listen: Option<SocketAddr>,
    /// Network privacy: direct (default), hybrid or tor-only.
    #[arg(long)]
    network_mode: Option<NetworkMode>,
    /// Use an external Tor daemon at this SOCKS address (IP:PORT) instead of the
    /// built-in Tor client, for example a system Tor (9050) or Tor Browser (9150).
    #[arg(long)]
    tor_socks: Option<SocketAddr>,
    /// External Tor's control port as IP:PORT, for bootstrap status and its onion service.
    #[arg(long)]
    tor_control: Option<SocketAddr>,
    /// File containing the external Tor's control password; never pass secrets as arguments.
    #[arg(long)]
    tor_control_password_file: Option<PathBuf>,
    /// External Tor's control cookie, if Tor reports a path this process cannot use.
    #[arg(long)]
    tor_cookie_file: Option<PathBuf>,
    /// Accept inbound peers through a persistent Tor v3 onion service.
    #[arg(long)]
    onion_service: bool,
    /// Loopback address an external Tor forwards onion connections to (default base port + 3).
    #[arg(long)]
    onion_listen: Option<SocketAddr>,
    /// Share one Tor circuit among peers instead of isolating each connection.
    #[arg(long)]
    no_stream_isolation: bool,
}
#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .json()
        .with_env_filter(
            tracing_subscriber::EnvFilter::from_default_env()
                .add_directive(tracing::Level::INFO.into()),
        )
        .init();
    let args = Args::parse();
    let config = genoryn::config::Config::load(args.config.as_deref())?;
    let network = Network::parse(args.network.as_deref().unwrap_or(&config.network.name))?;
    network.ensure_runnable()?;
    let base = network.base_port();
    let mode = args
        .network_mode
        .or(config.network_privacy.mode)
        .unwrap_or_default();
    let policy = TransportPolicy::new(mode);
    let configured_listen = args.p2p_listen.or(config.p2p.listen);
    // Tor-only nodes accept inbound peers through the onion service; a clearnet
    // listener exists only if one is configured, and then only on loopback.
    let listen = match mode {
        NetworkMode::TorOnly => configured_listen,
        _ => Some(configured_listen.unwrap_or(([0, 0, 0, 0], base).into())),
    };
    let p2p = listen.unwrap_or(([127, 0, 0, 1], base).into());
    let public_rpc = args.public_rpc || config.public_rpc.enabled;
    let public_rpc_listen = args
        .public_rpc_listen
        .or(config.public_rpc.listen)
        .unwrap_or(([127, 0, 0, 1], base + 2).into());
    let tor = if mode.uses_tor() {
        // Without an external SOCKS address the built-in Tor client is used.
        let mut tor = TorConfig::new(args.tor_socks.or(config.tor.socks), base);
        tor.control = args.tor_control.or(config.tor.control);
        tor.control_password_file = args
            .tor_control_password_file
            .or(config.tor.control_password_file);
        tor.cookie_file = args.tor_cookie_file.or(config.tor.cookie_file);
        tor.onion_service = args.onion_service || config.tor.onion_service;
        if let Some(target) = args.onion_listen.or(config.tor.onion_listen) {
            tor.onion_target = target;
        }
        // Light wallets reach the public RPC through the same onion service.
        if tor.built_in() && tor.onion_service && public_rpc {
            tor.onion_rpc = Some(public_rpc_listen);
        }
        if let Some(port) = config.tor.onion_port {
            tor.onion_port = port;
        }
        tor.stream_isolation =
            !args.no_stream_isolation && config.tor.stream_isolation.unwrap_or(true);
        if tor.socks.is_some_and(|socks| !socks.ip().is_loopback()) {
            tracing::warn!(
                "the Tor SOCKS proxy is not on loopback; traffic to it crosses the local network unencrypted"
            );
        }
        tracing::info!(
            engine = if tor.built_in() {
                "built-in"
            } else {
                "external"
            },
            "Tor engine"
        );
        Some(tor)
    } else {
        ensure!(
            args.tor_socks.is_none()
                && args.tor_control.is_none()
                && args.tor_control_password_file.is_none()
                && args.tor_cookie_file.is_none()
                && !args.onion_service
                && args.onion_listen.is_none(),
            "Tor options need --network-mode hybrid or tor-only"
        );
        None
    };
    let options = NetworkOptions { mode, tor, listen };
    options.validate()?;
    tracing::info!(
        mode = mode.name(),
        notice = mode.privacy_notice(),
        "network privacy mode"
    );
    let rpc = args
        .rpc_listen
        .or(config.rpc.admin_listen)
        .unwrap_or(([127, 0, 0, 1], base + 1).into());
    ensure!(rpc.ip().is_loopback(), "admin RPC must stay on loopback");
    let data_dir = args
        .data_dir
        .or(config.database.data_dir)
        .unwrap_or_else(|| PathBuf::from(format!(".genoryn/{}", network.name())));
    let mut peers = args.peer;
    peers.extend(config.p2p.peers);
    for peer in &peers {
        let address = PeerAddr::parse_with_default_port(peer, base)?;
        ensure!(
            !address.is_onion() || mode.uses_tor(),
            "onion peers need --network-mode hybrid or tor-only"
        );
    }
    let mut seeds = args.dns_seed;
    seeds.extend(config.p2p.dns_seeds);
    for seed in &seeds {
        ensure!(
            genoryn::seeds::valid_seed_name(seed),
            "invalid DNS seed name"
        );
    }
    let node = genoryn::node::Node::open_with_options(
        network,
        data_dir,
        p2p,
        public_rpc,
        config.limits,
        options,
    )?;
    if let Some(tor) = &node.net.tor {
        tor.start();
    }
    let rpc_node = node.clone();
    let rpc_task = tokio::spawn(async move { genoryn::node::serve(rpc_node, rpc, true).await });
    if public_rpc {
        let addr = public_rpc_listen;
        let public_node = node.clone();
        tokio::spawn(async move {
            if let Err(e) = genoryn::node::serve(public_node, addr, false).await {
                tracing::error!(error=%e,"public RPC failed");
            }
        });
    }
    let local_lab = configured_listen.is_some_and(|listen| listen.ip().is_loopback());
    if !local_lab && !args.no_default_seeds {
        seeds.extend(
            genoryn::seeds::default_seeds(network)
                .iter()
                .map(|seed| seed.to_string()),
        );
        if mode.uses_tor() {
            seeds.extend(
                genoryn::seeds::default_onion_seeds(network)
                    .iter()
                    .map(|seed| seed.to_string()),
            );
        }
        if mode == NetworkMode::TorOnly {
            // Published seeds are fetched only through Tor. A first bootstrap of the
            // built-in client downloads the Tor directory, so allow it time.
            for _ in 0..120 {
                if node.net.connector.tor_usable() && node.net.auxiliary_proxy().is_some() {
                    break;
                }
                tokio::time::sleep(std::time::Duration::from_secs(1)).await;
            }
            match node.net.auxiliary_proxy() {
                Some(proxy) if node.net.connector.tor_usable() => {
                    let (published, onion) = tokio::join!(
                        genoryn::seeds::fetch_published(network, Some(proxy.as_str())),
                        genoryn::seeds::fetch_published_onion(network, Some(proxy.as_str())),
                    );
                    seeds.extend(published);
                    seeds.extend(onion);
                }
                _ => tracing::warn!(
                    "Tor not ready; published seed list skipped rather than fetched directly"
                ),
            }
        } else {
            seeds.extend(genoryn::seeds::fetch_published(network, None).await);
            if mode.uses_tor() {
                seeds.extend(genoryn::seeds::fetch_published_onion(network, None).await);
            }
        }
    }
    let mut seen = std::collections::BTreeSet::new();
    let entries = peers
        .into_iter()
        .chain(seeds)
        .filter(|entry| seen.insert(entry.clone()))
        .collect::<Vec<_>>();
    let mut targets =
        genoryn::net::bootstrap_targets(policy, &entries, base, |host, port| async move {
            tokio::net::lookup_host((host.as_str(), port))
                .await
                .map(|addresses| addresses.collect())
        })
        .await;
    let mut unique = std::collections::BTreeSet::new();
    targets.retain(|target| unique.insert(target.clone()));
    targets.truncate(128);
    let p2p_task = tokio::spawn(async move { genoryn::p2p::run(node, targets).await });
    tokio::select! {result=p2p_task=>result??,result=rpc_task=>result??,_=tokio::signal::ctrl_c()=>tracing::info!("shutdown requested")};
    Ok(())
}

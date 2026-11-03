//! kryptond: the Genoryn Krypton full node.

mod mempool;
mod node;
mod rpc;
mod stats;

use anyhow::{Context, Result};
use clap::Parser;
use krypton_core::Network;
use krypton_p2p::{Config, Net};
use krypton_store::Chain;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

#[derive(Parser, Debug)]
#[command(name = "kryptond", version, about = "Genoryn Krypton full node")]
struct Args {
    /// mainnet, testnet or devnet
    #[arg(long, default_value = "mainnet", env = "KRYPTON_NETWORK")]
    network: String,
    /// Data directory (default: ~/.genoryn-krypton/<network>)
    #[arg(long, env = "KRYPTON_DATA_DIR")]
    data_dir: Option<PathBuf>,
    /// P2P listen address (default 0.0.0.0:<network port>)
    #[arg(long)]
    listen: Option<SocketAddr>,
    /// Do not accept inbound peers
    #[arg(long)]
    no_listen: bool,
    /// API listen address (default 127.0.0.1:<network rpc port>)
    #[arg(long)]
    rpc: Option<SocketAddr>,
    /// Extra peers to connect to (host:port), repeatable
    #[arg(long = "peer")]
    peers: Vec<String>,
    /// Do not use the built-in seed nodes
    #[arg(long)]
    no_seeds: bool,
    #[arg(long, default_value_t = 8)]
    max_outbound: usize,
    #[arg(long, default_value_t = 64)]
    max_inbound: usize,
}

pub fn default_data_dir(network: Network) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("GenorynKrypton"))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".genoryn-krypton")))
        .unwrap_or_else(|| PathBuf::from(".genoryn-krypton"));
    base.join(network.as_str())
}

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| "info,redb=warn".into()),
        )
        .init();
    let a = Args::parse();
    let network = Network::parse(&a.network).context("unknown network")?;
    let p = network.params();
    let dir = a
        .data_dir
        .clone()
        .unwrap_or_else(|| default_data_dir(network));
    tracing::info!(
        "{} node {} starting, data in {}",
        p.name,
        env!("CARGO_PKG_VERSION"),
        dir.display()
    );
    let chain = Arc::new(tokio::task::spawn_blocking(move || Chain::open(&dir, network)).await??);
    let tip = chain.tip();
    tracing::info!(height = tip.height(), tip = %hex::encode(tip.hash), genesis = %hex::encode(chain.genesis.hash()), "chain loaded");

    let listen = if a.no_listen {
        None
    } else {
        Some(
            a.listen
                .unwrap_or_else(|| SocketAddr::from(([0, 0, 0, 0], p.p2p_port))),
        )
    };
    let mut seeds: Vec<String> = a.peers.clone();
    if !a.no_seeds {
        seeds.extend(p.seeds.iter().map(|s| s.to_string()));
    }
    let (net, rx) = Net::start(Config {
        magic: p.magic,
        listen,
        seeds,
        max_outbound: a.max_outbound,
        max_inbound: a.max_inbound,
        max_per_ip: 4,
    });
    let node = node::Node::new(chain, net, listen.map(|l| l.port()).unwrap_or(0));
    let n2 = node.clone();
    tokio::spawn(async move { n2.run(rx).await });

    let rpc_addr = a
        .rpc
        .unwrap_or_else(|| SocketAddr::from(([127, 0, 0, 1], p.rpc_port)));
    let listener = tokio::net::TcpListener::bind(rpc_addr)
        .await
        .context("bind API")?;
    tracing::info!("API on http://{rpc_addr} (REST /v1, JSON-RPC /rpc, WebSocket /v1/ws)");
    axum::serve(
        listener,
        rpc::router(node).into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(async {
        let _ = tokio::signal::ctrl_c().await;
        tracing::info!("shutting down");
    })
    .await?;
    Ok(())
}

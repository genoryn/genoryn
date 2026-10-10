//! Light wallet transport: a wallet without a local node reads chain data from
//! remote nodes' public RPC and hands transactions to them, only through Tor.
//!
//! Every downloaded block is fully validated locally by the caller's scan
//! (RandomX work, Halo 2 proofs, state roots) before anything is derived from
//! it, so a remote node can withhold data but cannot make the wallet accept an
//! invalid chain. Several sources are compared to expose withholding, each on
//! its own circuit, and transactions leave through a different source than the
//! one the wallet synced from when there is one.
use crate::{
    client::RpcClient,
    net::{
        bridge::{self, AuxProxy},
        embedded::EmbeddedTor,
    },
};
use anyhow::{Context, Result, ensure};
use genoryn_core::{block::Block, params::Network};
use rand08::{RngCore, seq::SliceRandom};
use serde::Serialize;
use serde_json::Value;
use std::{net::SocketAddr, path::Path, sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// The Tor a light wallet uses: its own built-in client, or an external Tor.
pub struct LightTor {
    _engine: Option<Arc<EmbeddedTor>>,
    bridge: Option<AuxProxy>,
    external: Option<SocketAddr>,
}
impl LightTor {
    /// Starts the built-in client with its state in `state_dir`, or uses the
    /// external Tor at `external`. Nothing is sent before Tor is ready.
    pub async fn start(
        state_dir: &Path,
        external: Option<SocketAddr>,
        timeout: Duration,
    ) -> Result<Self> {
        if let Some(socks) = external {
            return Ok(Self {
                _engine: None,
                bridge: None,
                external: Some(socks),
            });
        }
        let engine = EmbeddedTor::create(state_dir)?;
        tokio::time::timeout(timeout, engine.bootstrap())
            .await
            .context("the built-in Tor did not finish connecting in time")?
            .map_err(|error| {
                anyhow::anyhow!("the built-in Tor could not connect: {}", error.message)
            })?;
        let upstream_engine = engine.clone();
        let upstream: bridge::Upstream = Arc::new(move |target| {
            let engine = upstream_engine.clone();
            Box::pin(async move {
                engine
                    .connect(&target, true)
                    .await
                    .map_err(|error| error.message)
            })
        });
        let bridge = bridge::start(upstream).await?;
        Ok(Self {
            _engine: Some(engine),
            bridge: Some(bridge),
            external: None,
        })
    }
    /// A `socks5h://` proxy URL whose connections get circuits of their own:
    /// the built-in bridge isolates every connection; an external Tor isolates
    /// by the random credentials (IsolateSOCKSAuth).
    pub fn proxy(&self) -> Zeroizing<String> {
        match (&self.bridge, self.external) {
            (Some(bridge), _) => bridge.url(),
            (None, Some(socks)) => {
                let mut secret = [0u8; 12];
                rand08::rngs::OsRng.fill_bytes(&mut secret);
                Zeroizing::new(format!("socks5h://genoryn:{}@{socks}", hex::encode(secret)))
            }
            (None, None) => unreachable!("LightTor always has a route"),
        }
    }
}

/// What one remote source reported and how it fared.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
pub struct SourceReport {
    pub source: String,
    pub height: Option<u64>,
    /// `used`, `agrees`, `behind`, `different branch`, `rejected: …` or `unreachable: …`.
    pub status: String,
}

/// Downloads the chain from the best source that passes `scan` (which must
/// validate it fully) and compares the others with it. Returns the scan
/// result, the index of the source used and a report per source.
pub async fn sync<T>(
    network: Network,
    clients: &[RpcClient],
    labels: &[String],
    mut scan: impl FnMut(&[Block]) -> Result<T>,
) -> Result<(T, usize, Vec<SourceReport>)> {
    ensure!(
        !clients.is_empty() && clients.len() == labels.len(),
        "light sync needs at least one source"
    );
    let mut reports: Vec<SourceReport> = labels
        .iter()
        .map(|label| SourceReport {
            source: label.clone(),
            height: None,
            status: String::new(),
        })
        .collect();
    let mut tips = vec![None; clients.len()];
    for (index, client) in clients.iter().enumerate() {
        match client.get::<Value>("v1/info").await {
            Ok(info)
                if info["chain_id"] == network.chain_id()
                    && info["genesis_hash"] == hex::encode(network.genesis()) =>
            {
                let height = info["height"].as_u64();
                reports[index].height = height;
                tips[index] = height.zip(info["best_hash"].as_str().map(str::to_owned));
            }
            Ok(_) => reports[index].status = "rejected: another network or genesis".into(),
            Err(error) => reports[index].status = format!("unreachable: {}", short(&error)),
        }
    }
    let mut order: Vec<usize> = (0..clients.len()).filter(|i| tips[*i].is_some()).collect();
    order.sort_by_key(|i| std::cmp::Reverse(tips[*i].as_ref().map(|(height, _)| *height)));
    let mut chosen = None;
    for index in order {
        let attempt = async {
            let blocks = clients[index].chain(network).await?;
            let scanned = scan(&blocks)?;
            Ok::<_, anyhow::Error>((blocks, scanned))
        };
        match attempt.await {
            Ok((blocks, scanned)) => {
                reports[index].status = "used".into();
                chosen = Some((index, blocks, scanned));
                break;
            }
            Err(error) => reports[index].status = format!("rejected: {}", short(&error)),
        }
    }
    let (used, blocks, scanned) =
        chosen.context("no source served a chain that passed full validation")?;
    for (index, tip) in tips.iter().enumerate() {
        let Some((height, hash)) = tip else { continue };
        if index == used || !reports[index].status.is_empty() {
            continue;
        }
        let ours = match *height {
            0 => Some(hex::encode(network.genesis())),
            h => blocks
                .get(h as usize - 1)
                .and_then(|block| block.hash().ok())
                .map(hex::encode),
        };
        reports[index].status = if *height as usize > blocks.len() {
            "claims a longer chain; not verified".into()
        } else if ours.as_deref() != Some(hash.as_str()) {
            "different branch".into()
        } else if (*height as usize) < blocks.len() {
            "behind".into()
        } else {
            "agrees".into()
        };
    }
    Ok((scanned, used, reports))
}

/// Hands `tx` to a source other than `synced_from` when possible (in random
/// order), falling back to it last. Returns the index used and its answer.
pub async fn relay(
    clients: &[RpcClient],
    synced_from: usize,
    tx: &impl Serialize,
) -> Result<(usize, Value)> {
    let mut order: Vec<usize> = (0..clients.len()).filter(|i| *i != synced_from).collect();
    order.shuffle(&mut rand08::rngs::OsRng);
    if synced_from < clients.len() {
        order.push(synced_from);
    }
    let mut last = None;
    for index in order {
        match clients[index].relay::<Value>(tx).await {
            Ok(answer) => return Ok((index, answer)),
            Err(error) => last = Some(error),
        }
    }
    Err(last
        .unwrap_or_else(|| anyhow::anyhow!("no source to relay through"))
        .context("no source accepted the transaction"))
}

fn short(error: &anyhow::Error) -> String {
    format!("{error:#}").chars().take(160).collect()
}

//! Light wallet transport: a wallet without a local node reads chain data from
//! remote nodes' public RPC and hands transactions to them, only through Tor.
//!
//! Every downloaded block is fully validated on this computer (RandomX work,
//! Halo 2 proofs, state roots) before anything is derived from it, so a remote
//! node can withhold data but cannot make the wallet accept an invalid chain.
//! The chain to follow is chosen exactly like a full node chooses: by the
//! cumulative work computed locally from validated headers, never by a height
//! or work figure a server advertises, and on equal work the chain already
//! followed is kept. Verified blocks are kept locally, so each sync downloads
//! only what is new. Several sources are compared to expose withholding, each
//! on its own circuit, and transactions leave through a different source than
//! the one the wallet synced from when there is one.
use crate::{
    client::RpcClient,
    net::{
        bridge::{self, AuxProxy},
        embedded::EmbeddedTor,
    },
};
use anyhow::{Context, Result, ensure};
use futures::{StreamExt, stream};
use genoryn_core::{
    block::Block,
    chain::unix_time,
    codec::Hash,
    params::Network,
    state::{ChainState, ProofCache, Verification},
};
use rand08::{RngCore, seq::SliceRandom};
use serde::Serialize;
use serde_json::Value;
use std::{collections::BTreeMap, net::SocketAddr, path::Path, sync::Arc, time::Duration};
use zeroize::Zeroizing;

/// Blocks requested from one source at a time while downloading. Every new Tor
/// connection builds its own circuit to the onion service, so few connections
/// that are kept and reused are faster and far more reliable than many.
const PARALLEL_REQUESTS: usize = 2;
/// How far back a fork point is searched block by block before the source's
/// whole chain is downloaded instead.
const MAX_FORK_SEARCH: u64 = 256;
/// Upper bound on a chain this wallet downloads and keeps.
pub const MAX_CHAIN_BLOCKS: u64 = 1_000_000;
/// Blocks whose work and proofs are checked in parallel before being applied.
pub const PREVERIFY_CHUNK: usize = 1_024;
/// Parallel checking pays for its per-thread RandomX setup (about a second)
/// only on longer downloads; short incremental syncs stay sequential.
pub fn preverify_threads(blocks: usize) -> usize {
    if blocks < 64 {
        return 1;
    }
    std::thread::available_parallelism()
        .map_or(2, usize::from)
        .min(16)
        .min(blocks / 16)
        .max(1)
}

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
    /// The height the source advertised; informational only.
    pub height: Option<u64>,
    /// `used`, `agrees`, `behind`, `less work`, `equal work`, `rejected: …` or
    /// `unreachable: …`.
    pub status: String,
}

/// The chain a light sync settled on.
pub struct Verified {
    /// Every block of the followed chain, validated on this computer.
    pub blocks: Vec<Block>,
    pub state: ChainState,
    /// Blocks above this height differ from the chain followed before the sync
    /// (equal to the previous height when the chain was only extended or kept).
    pub fork: u64,
    /// The source whose blocks were adopted; `None` when the local chain was kept.
    pub used: Option<usize>,
    pub reports: Vec<SourceReport>,
    /// Sources that answered for this network and genesis.
    pub reachable: usize,
}
impl Verified {
    /// No source confirmed the followed chain (it agrees with it, served it, is
    /// behind it or has less or equal work): the result is the last verified
    /// chain and may be out of date, for example when every download failed.
    pub fn is_stale(&self) -> bool {
        !self.reports.iter().any(|report| {
            let status = report.status.as_str();
            status == "agrees"
                || status == "behind"
                || status.starts_with("used")
                || status.starts_with("less work")
                || status.starts_with("equal work")
        })
    }
}

struct Candidate {
    source: usize,
    fork: u64,
    blocks: Vec<Block>,
    state: ChainState,
}

async fn block_at(client: &RpcClient, height: u64) -> Result<Block> {
    let mut page: Vec<Block> = client
        .get(&format!("v1/blocks?from={height}&limit=1"))
        .await?;
    ensure!(
        page.len() == 1,
        "source returned no block at height {height}"
    );
    let block = page.remove(0);
    ensure!(
        block.header.height == height,
        "source returned the wrong block for height {height}"
    );
    Ok(block)
}
/// Attempts per block before a source is given up; Tor circuits fail now and then.
const BLOCK_ATTEMPTS: u32 = 6;
/// Blocks a page asks for over Tor; a 1.1 node answers up to this many at
/// once, an older one a single block (the rest is asked for again).
const PAGE: u64 = 16;
/// Blocks `from..=to`, retried after transport errors (never after a wrong answer).
async fn range_with_retry(client: &RpcClient, from: u64, to: u64) -> Result<Vec<Block>> {
    let mut blocks = Vec::with_capacity((to - from + 1) as usize);
    let mut next = from;
    let mut attempt = 1;
    while next <= to {
        match client.block_page(next, (to - next + 1) as usize).await {
            Ok(page) => {
                next += page.len() as u64;
                blocks.extend(page);
                attempt = 1;
            }
            Err(error)
                if attempt < BLOCK_ATTEMPTS && error.downcast_ref::<reqwest::Error>().is_some() =>
            {
                tokio::time::sleep(Duration::from_millis(500 << attempt)).await;
                attempt += 1;
            }
            Err(error) => return Err(error),
        }
    }
    Ok(blocks)
}
/// Blocks `from..=to` in order: pages of `PAGE` blocks, several in flight.
async fn download(client: &RpcClient, from: u64, to: u64) -> Result<Vec<Block>> {
    let pages: Vec<(u64, u64)> = (from..=to)
        .step_by(PAGE as usize)
        .map(|start| (start, (start + PAGE - 1).min(to)))
        .collect();
    let mut blocks = Vec::with_capacity((to + 1).saturating_sub(from) as usize);
    let mut fetched = stream::iter(pages)
        .map(|(start, end)| range_with_retry(client, start, end))
        .buffered(PARALLEL_REQUESTS);
    while let Some(page) = fetched.next().await {
        blocks.extend(page?);
    }
    Ok(blocks)
}
/// The highest height at which `client`'s chain and `local` share a block.
async fn fork_point(client: &RpcClient, local: &[Block], their_height: u64) -> Result<u64> {
    let top = their_height.min(local.len() as u64);
    let mut height = top;
    while height > 0 && top - height < MAX_FORK_SEARCH {
        let theirs = block_at(client, height).await?.hash()?;
        if theirs == local[height as usize - 1].hash()? {
            return Ok(height);
        }
        height -= 1;
    }
    Ok(0)
}
fn replay(network: Network, blocks: &[Block]) -> Result<ChainState> {
    let mut state = ChainState::new(network);
    let mut cache = ProofCache::default();
    for block in blocks {
        state.apply_block_in_place(block, unix_time(), &mut cache, Verification::Stored)?;
    }
    Ok(state)
}

/// Brings the locally verified chain (`local`, whose validated state is
/// `local_state`) up to date from `clients`, following the valid chain with the
/// most cumulative work. Unreachable sources leave the local chain in place.
pub async fn sync_verified(
    network: Network,
    clients: &[RpcClient],
    labels: &[String],
    local: Vec<Block>,
    local_state: ChainState,
) -> Result<Verified> {
    ensure!(
        !clients.is_empty() && clients.len() == labels.len(),
        "light sync needs at least one source"
    );
    ensure!(
        local_state.height == local.len() as u64,
        "local chain and its state disagree"
    );
    let local_tip = local_state.best_hash;
    let mut reports: Vec<SourceReport> = labels
        .iter()
        .map(|label| SourceReport {
            source: label.clone(),
            height: None,
            status: String::new(),
        })
        .collect();
    let infos =
        futures::future::join_all(clients.iter().map(|client| client.get::<Value>("v1/info")))
            .await;
    let mut tips: Vec<Option<(u64, String)>> = vec![None; clients.len()];
    let mut reachable = 0;
    for (index, info) in infos.into_iter().enumerate() {
        match info {
            Ok(info)
                if info["chain_id"] == network.chain_id()
                    && info["genesis_hash"] == hex::encode(network.genesis()) =>
            {
                match (info["height"].as_u64(), info["best_hash"].as_str()) {
                    (Some(0), Some(best)) if best != hex::encode(network.genesis()) => {
                        reports[index].status = "rejected: tip does not match genesis".into()
                    }
                    (Some(height), Some(best)) if height <= MAX_CHAIN_BLOCKS => {
                        reachable += 1;
                        reports[index].height = Some(height);
                        tips[index] = Some((height, best.to_owned()));
                    }
                    _ => reports[index].status = "rejected: malformed status".into(),
                }
            }
            Ok(_) => reports[index].status = "rejected: another network or genesis".into(),
            Err(error) => reports[index].status = format!("unreachable: {}", short(&error)),
        }
    }
    let mut candidates: Vec<Candidate> = Vec::new();
    // Sources advertising a tip already examined share its verdict.
    let mut examined: BTreeMap<String, usize> = BTreeMap::new();
    let mut fork_states: BTreeMap<u64, ChainState> = BTreeMap::new();
    for index in 0..clients.len() {
        let Some((height, best)) = tips[index].clone() else {
            continue;
        };
        if best == hex::encode(local_tip) {
            reports[index].status = "agrees".into();
            continue;
        }
        if let Some(first) = examined.get(&best) {
            reports[index].status = reports[*first].status.clone();
            continue;
        }
        examined.insert(best.clone(), index);
        let attempt = async {
            let fork = fork_point(&clients[index], &local, height).await?;
            if height <= fork {
                return Ok::<_, anyhow::Error>(None);
            }
            let state = match fork_states.get(&fork) {
                Some(state) => state.clone(),
                None if fork == local.len() as u64 => local_state.clone(),
                None => {
                    let state = replay(network, &local[..fork as usize])?;
                    fork_states.insert(fork, state.clone());
                    state
                }
            };
            let blocks = download(&clients[index], fork + 1, height).await?;
            Ok(Some((fork, state, blocks)))
        };
        let (fork, mut state, blocks) = match attempt.await {
            Ok(Some(found)) => found,
            Ok(None) => {
                reports[index].status = "behind".into();
                continue;
            }
            Err(error) => {
                reports[index].status = format!("rejected: {}", short(&error));
                continue;
            }
        };
        let base = state.clone();
        let mut cache = ProofCache::default();
        let mut valid = Vec::with_capacity(blocks.len());
        let mut failure = None;
        let threads = preverify_threads(blocks.len());
        let mut blocks = blocks.into_iter().peekable();
        'chunks: while blocks.peek().is_some() {
            let chunk: Vec<Block> = blocks.by_ref().take(PREVERIFY_CHUNK).collect();
            // Work and proofs checked in parallel first; every rule is then
            // applied in order below exactly as before.
            cache.preverify(&chunk, threads);
            for block in chunk {
                match state.apply_block_in_place(
                    &block,
                    unix_time(),
                    &mut cache,
                    Verification::Full,
                ) {
                    Ok(()) => valid.push(block),
                    Err(error) => {
                        failure = Some(format!(
                            "invalid block at height {}: {}",
                            block.header.height,
                            short(&error)
                        ));
                        break 'chunks;
                    }
                }
            }
        }
        if failure.is_some() {
            // A failed block leaves the state partly updated; rebuild the valid part.
            state = base;
            for block in &valid {
                state.apply_block_in_place(block, unix_time(), &mut cache, Verification::Stored)?;
            }
        }
        if failure.is_none() && hex::encode(state.best_hash) != best {
            failure = Some("blocks do not lead to the advertised tip".into());
        }
        if let Some(reason) = &failure {
            reports[index].status = format!("rejected: {reason}");
        }
        if !valid.is_empty() {
            candidates.push(Candidate {
                source: index,
                fork,
                blocks: valid,
                state,
            });
        }
    }
    // Most cumulative work wins; on equal work the chain already followed (or
    // the first one examined) is kept, as in the full node.
    let mut best: Option<usize> = None;
    let mut best_work = local_state.work;
    for (position, candidate) in candidates.iter().enumerate() {
        if candidate.state.work > best_work {
            best_work = candidate.state.work;
            best = Some(position);
        }
    }
    // A source whose tip lies on the newly followed chain is merely behind it.
    let chosen_hashes: Option<BTreeMap<u64, Hash>> = best.map(|position| {
        let chosen = &candidates[position];
        chosen
            .blocks
            .iter()
            .filter_map(|block| Some((block.header.height, block.hash().ok()?)))
            .collect()
    });
    let on_chosen = |candidate: &Candidate| {
        let chosen = &candidates[best.expect("checked")];
        let tip = candidate.state.height;
        candidate.fork >= chosen.fork
            && chosen_hashes
                .as_ref()
                .and_then(|hashes| hashes.get(&tip))
                .is_some_and(|hash| *hash == candidate.state.best_hash)
    };
    let behind: Vec<bool> = candidates
        .iter()
        .enumerate()
        .map(|(position, candidate)| {
            best.is_some() && Some(position) != best && on_chosen(candidate)
        })
        .collect();
    for (position, candidate) in candidates.iter().enumerate() {
        let report = &mut reports[candidate.source];
        if behind[position] && report.status.is_empty() {
            report.status = "behind".into();
        } else if Some(position) == best {
            if report.status.is_empty() {
                report.status = "used".into();
            } else {
                report.status = format!("used valid part; {}", report.status);
            }
        } else if report.status.is_empty() {
            report.status = if candidate.state.work == best_work {
                "equal work; current chain kept".into()
            } else {
                "less work; not followed".into()
            };
        }
    }
    Ok(match best {
        Some(position) => {
            let candidate = candidates.swap_remove(position);
            let mut blocks = local;
            blocks.truncate(candidate.fork as usize);
            blocks.extend(candidate.blocks);
            Verified {
                blocks,
                state: candidate.state,
                fork: candidate.fork,
                used: Some(candidate.source),
                reports,
                reachable,
            }
        }
        None => Verified {
            fork: local.len() as u64,
            blocks: local,
            state: local_state,
            used: None,
            reports,
            reachable,
        },
    })
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

/// The id a node acknowledged, or its refusal as text.
pub fn acknowledged(answer: Result<Value>) -> std::result::Result<String, String> {
    match answer {
        Ok(value) => value["txid"]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| "the node answered without a transaction id".to_owned()),
        Err(error) => Err(format!("{error:#}")),
    }
}

/// Local tip hash helper for callers that report the followed chain.
pub fn tip_hash(network: Network, blocks: &[Block]) -> Result<Hash> {
    Ok(match blocks.last() {
        Some(block) => block.hash()?,
        None => network.genesis(),
    })
}

fn short(error: &anyhow::Error) -> String {
    format!("{error:#}").chars().take(160).collect()
}

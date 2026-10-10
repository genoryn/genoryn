use anyhow::{Context, Result};
use clap::{Parser, ValueEnum};
use genoryn_core::{block::Block, codec::Hash, params::Network, pow};
use serde_json::{Value, json};
use std::{
    path::PathBuf,
    sync::{
        Arc, Mutex, PoisonError,
        atomic::{AtomicBool, AtomicU8, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};
#[derive(Clone, Copy, Debug, PartialEq, Eq, ValueEnum)]
enum RandomXMode {
    /// One ~2 GiB dataset shared by all threads, several times faster. Mining
    /// starts in light mode and switches once the dataset is built; it stays in
    /// light mode if the dataset cannot be allocated.
    Auto,
    /// A 256 MiB cache per thread, at most 16 threads.
    Light,
}
#[derive(Parser)]
#[command(
    name = "genoryn-miner",
    about = "Explicit-destination solo RandomX CPU miner"
)]
struct Args {
    #[arg(long, default_value = "mainnet")]
    network: String,
    #[arg(long, default_value = "http://127.0.0.1:29711")]
    node: String,
    #[arg(long)]
    token_file: PathBuf,
    #[arg(long)]
    wallet: String,
    #[arg(long, default_value_t = 1)]
    threads: usize,
    #[arg(long, default_value_t = 0)]
    max_blocks: u64,
    #[arg(long, default_value_t = 0)]
    max_seconds: u64,
    #[arg(long, value_enum, default_value_t = RandomXMode::Auto)]
    randomx_mode: RandomXMode,
}
/// Each light-mode worker holds its own 256MiB RandomX cache.
const MAX_LIGHT_THREADS: usize = 16;
/// Fast-mode workers share the dataset and add only a 2MiB scratchpad each.
const MAX_FAST_THREADS: usize = 256;
const STALE_CHECK: Duration = Duration::from_secs(1);
const TEMPLATE_RETRY_START: Duration = Duration::from_secs(1);
const TEMPLATE_RETRY_MAX: Duration = Duration::from_secs(10);
/// A template searched again after a mode switch starts at a fresh nonce range.
const NONCE_ROUND_SHIFT: u32 = 40;

/// One worker per logical CPU, bounded to keep memory predictable.
fn max_threads(mode: RandomXMode) -> usize {
    let cap = match mode {
        RandomXMode::Auto => MAX_FAST_THREADS,
        RandomXMode::Light => MAX_LIGHT_THREADS,
    };
    std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, cap)
}

/// The full-memory dataset for the current PoW seed. It is built on its own
/// thread, so mining starts at once in light mode and switches when it is ready.
#[derive(Default)]
struct Fast {
    ready: Option<pow::FastDataset>,
    building: Option<Hash>,
    generation: u64,
    unavailable: bool,
}
type SharedFast = Arc<Mutex<Fast>>;

/// The dataset for `seed` if it is built; otherwise starts building it once.
fn fast_dataset(fast: &SharedFast, seed: Hash) -> Option<pow::FastDataset> {
    let mut state = fast.lock().unwrap_or_else(PoisonError::into_inner);
    if let Some(dataset) = state.ready.as_ref().filter(|d| d.seed() == seed) {
        return Some(dataset.clone());
    }
    if state.unavailable || state.building == Some(seed) {
        return None;
    }
    // A new seed retires the old dataset; running workers keep theirs until they stop.
    state.ready = None;
    state.building = Some(seed);
    state.generation += 1;
    let generation = state.generation;
    let fast = fast.clone();
    std::thread::spawn(move || {
        let started = Instant::now();
        let result = pow::FastDataset::new(seed);
        let mut state = fast.lock().unwrap_or_else(PoisonError::into_inner);
        if state.generation != generation {
            return;
        }
        state.building = None;
        match result {
            Ok(dataset) => {
                eprintln!(
                    "RandomX fast mode ready after {:.0}s: one dataset shared by all threads{}",
                    started.elapsed().as_secs_f64(),
                    if dataset.large_pages() {
                        ", in 2 MiB pages"
                    } else {
                        ""
                    }
                );
                state.ready = Some(dataset);
            }
            Err(error) => {
                eprintln!("RandomX fast mode unavailable, mining stays in light mode: {error:#}");
                state.unavailable = true;
            }
        }
    });
    None
}
fn fast_ready(fast: &SharedFast, seed: Hash) -> bool {
    let state = fast.lock().unwrap_or_else(PoisonError::into_inner);
    state.ready.as_ref().is_some_and(|d| d.seed() == seed)
}
fn fast_failed(fast: &SharedFast, error: &anyhow::Error) {
    eprintln!("RandomX fast mode unavailable, mining stays in light mode: {error:#}");
    let mut state = fast.lock().unwrap_or_else(PoisonError::into_inner);
    state.ready = None;
    state.unavailable = true;
}
/// Why a search ended before a worker found a block.
const STOP_STALE: u8 = 1;
const STOP_RESTART: u8 = 2;
/// A template for `height` is stale once the node's tip has reached that height.
fn template_is_stale(info: &Value, height: u64) -> bool {
    info["height"].as_u64().is_some_and(|tip| tip >= height)
}
/// Why the miner stopped, reported to the log and to the wallet as `{"event":"error"}`
/// so it can tell the user the cause and pick a remedy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Failure {
    /// Invalid arguments: thread count, address, network name, token file.
    Setup,
    /// The node never answered within the startup wait.
    NodeUnreachable,
    /// The node refused the admin token: it is a different node than the token's.
    Unauthorized,
    /// The node runs another network or genesis.
    NetworkMismatch,
    /// RandomX could not allocate or initialize a hashing VM.
    RandomX,
}
impl Failure {
    fn wrap(self, detail: String) -> anyhow::Error {
        anyhow::anyhow!(detail).context(self)
    }
    fn kind(self) -> &'static str {
        match self {
            Self::Setup => "setup",
            Self::NodeUnreachable => "node_unreachable",
            Self::Unauthorized => "unauthorized",
            Self::NetworkMismatch => "network_mismatch",
            Self::RandomX => "randomx",
        }
    }
}
impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Setup => "invalid miner settings",
            Self::NodeUnreachable => "the node did not answer",
            Self::Unauthorized => {
                "the node refused this admin token; another node may be using the RPC port"
            }
            Self::NetworkMismatch => "the node runs a different network",
            Self::RandomX => "RandomX could not start on this computer",
        })
    }
}
/// How long the miner waits for a starting node before giving up.
const NODE_WAIT: Duration = Duration::from_secs(120);

/// Only a refused token is fatal; template errors can clear once the node's
/// mempool or tip changes, so they are retried.
fn classify_rpc(error: &anyhow::Error) -> Option<Failure> {
    let rejected = error.downcast_ref::<genoryn::client::Rejected>()?;
    (rejected.is_unauthorized() || rejected.body.contains("authentication"))
        .then_some(Failure::Unauthorized)
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    match run(Args::parse()).await {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let failure = error
                .downcast_ref::<Failure>()
                .copied()
                .unwrap_or(Failure::Setup);
            let reason = format!("{error:#}");
            eprintln!("genoryn-miner stopped [{}]: {reason}", failure.kind());
            println!(
                "{}",
                json!({"event":"error","kind":failure.kind(),"reason":reason})
            );
            std::process::ExitCode::FAILURE
        }
    }
}

async fn run(args: Args) -> Result<()> {
    let network = Network::parse(&args.network).context(Failure::Setup)?;
    network.ensure_runnable().context(Failure::Setup)?;
    let max_threads = max_threads(args.randomx_mode);
    if !(1..=max_threads).contains(&args.threads) {
        return Err(Failure::Setup.wrap(match args.randomx_mode {
            RandomXMode::Auto => format!(
                "mining supports 1-{max_threads} CPU threads on this computer (one per logical CPU)"
            ),
            RandomXMode::Light => format!(
                "light-mode mining supports 1-{max_threads} CPU threads on this computer (one per logical CPU, at most {MAX_LIGHT_THREADS}; 256MiB cache each)"
            ),
        }));
    }
    genoryn_wallet_core::keys::decode_address(network, &args.wallet).context(Failure::Setup)?;
    let client = genoryn::client::RpcClient::new(&args.node, Some(&args.token_file))
        .context(Failure::Setup)?;
    let waiting = Instant::now();
    let mut retry = TEMPLATE_RETRY_START;
    let info: Value = loop {
        match client.get("v1/info").await {
            Ok(info) => break info,
            Err(error) if waiting.elapsed() < NODE_WAIT => {
                eprintln!(
                    "node at {} not answering yet, retrying in {retry:?}: {error}",
                    args.node
                );
                tokio::time::sleep(retry).await;
                retry = (retry * 2).min(TEMPLATE_RETRY_MAX);
            }
            Err(error) => {
                return Err(error
                    .context(format!(
                        "no answer from {} for {}s; is the node running?",
                        args.node,
                        NODE_WAIT.as_secs()
                    ))
                    .context(Failure::NodeUnreachable));
            }
        }
    };
    if info["chain_id"] != network.chain_id()
        || info["genesis_hash"] != hex::encode(network.genesis())
    {
        return Err(Failure::NetworkMismatch.wrap(format!(
            "the node at {} reports chain {}, but this miner is for {}",
            args.node,
            info["chain_id"].as_str().unwrap_or("unknown"),
            network.chain_id()
        )));
    }
    let start = Instant::now();
    let mut accepted = 0u64;
    let mut rejected = 0u64;
    let hashes = Arc::new(AtomicU64::new(0));
    let mut template_seconds = 0.0;
    let mut hashing_seconds = 0.0;
    let mut submission_seconds = 0.0;
    let fast = SharedFast::default();
    loop {
        if args.max_blocks > 0 && accepted >= args.max_blocks
            || args.max_seconds > 0 && start.elapsed().as_secs() >= args.max_seconds
        {
            break;
        }
        let template_start = Instant::now();
        let mut retry = TEMPLATE_RETRY_START;
        let block: Block = loop {
            match client
                .post("v1/mine/template", &json!({"wallet":args.wallet}))
                .await
            {
                Ok(block) => break block,
                Err(error) => {
                    if let Some(failure) = classify_rpc(&error) {
                        return Err(error.context(failure));
                    }
                    // A busy proof worker or a briefly restarting node is temporary.
                    eprintln!("block template unavailable, retrying in {retry:?}: {error}");
                    tokio::time::sleep(retry).await;
                    retry = (retry * 2).min(TEMPLATE_RETRY_MAX);
                }
            }
        };
        template_seconds += template_start.elapsed().as_secs_f64();
        let height = block.header.height;
        let seed = block.header.pow_seed;
        let deadline = if args.max_seconds > 0 {
            Some(start + Duration::from_secs(args.max_seconds))
        } else {
            None
        };
        let mut round = 0u64;
        let (mined, mode) = loop {
            let dataset = match args.randomx_mode {
                RandomXMode::Auto => fast_dataset(&fast, seed),
                RandomXMode::Light => None,
            };
            let threads = if dataset.is_some() {
                args.threads
            } else {
                args.threads.min(MAX_LIGHT_THREADS)
            };
            let mode = if dataset.is_some() { "fast" } else { "light" };
            let hashing_start = Instant::now();
            let found = Arc::new(AtomicBool::new(false));
            let stop = Arc::new(AtomicBool::new(false));
            let reason = Arc::new(AtomicU8::new(0));
            let (send, recv) = std::sync::mpsc::sync_channel(1);
            let mut workers = vec![];
            for index in 0..threads {
                let mut block = block.clone();
                let (found, stop, reason) = (found.clone(), stop.clone(), reason.clone());
                let (send, hashes) = (send.clone(), hashes.clone());
                let (dataset, fast) = (dataset.clone(), fast.clone());
                workers.push(std::thread::spawn(move || -> Result<()> {
                    let vm = match dataset.as_ref().map(pow::FastDataset::vm).transpose() {
                        Ok(vm) => vm,
                        Err(error) => {
                            fast_failed(&fast, &error);
                            reason.store(STOP_RESTART, Ordering::SeqCst);
                            stop.store(true, Ordering::SeqCst);
                            return Ok(());
                        }
                    };
                    block.header.nonce = (round << NONCE_ROUND_SHIFT) + index as u64;
                    while !found.load(Ordering::Relaxed) && !stop.load(Ordering::Relaxed) {
                        if deadline.is_some_and(|d| Instant::now() >= d) {
                            break;
                        }
                        let bytes = block.header.bytes()?;
                        let hash = match &vm {
                            Some(vm) => vm.hash(&bytes),
                            None => pow::randomx(seed, &bytes),
                        }
                        .context(Failure::RandomX)?;
                        hashes.fetch_add(1, Ordering::Relaxed);
                        if pow::meets_difficulty(hash, block.header.difficulty) {
                            if !found.swap(true, Ordering::SeqCst) {
                                let _ = send.try_send(block);
                            }
                            break;
                        }
                        block.header.nonce = block
                            .header
                            .nonce
                            .checked_add(threads as u64)
                            .ok_or_else(|| anyhow::anyhow!("nonce space exhausted"))?;
                    }
                    Ok(())
                }));
            }
            drop(send);
            // Abandon this template as soon as the node accepts a block at its height,
            // whoever mined it; otherwise the search continues on a stale parent.
            // A light-mode search restarts in fast mode once the dataset is ready.
            let watcher = {
                let client = client.clone();
                let (found, stop, reason) = (found.clone(), stop.clone(), reason.clone());
                let (hashes, fast) = (hashes.clone(), fast.clone());
                let upgrade = args.randomx_mode == RandomXMode::Auto && dataset.is_none();
                let totals = Totals {
                    start,
                    template_seconds,
                    hashing_seconds,
                    submission_seconds,
                    accepted,
                    rejected,
                    threads,
                    mode,
                };
                tokio::spawn(async move {
                    let mut reported = Instant::now();
                    loop {
                        tokio::time::sleep(STALE_CHECK).await;
                        if found.load(Ordering::Relaxed) || stop.load(Ordering::Relaxed) {
                            break;
                        }
                        // Long searches still show a live hashrate.
                        if reported.elapsed() >= PROGRESS_EVERY {
                            reported = Instant::now();
                            let hashing =
                                totals.hashing_seconds + hashing_start.elapsed().as_secs_f64();
                            println!(
                                "{}",
                                totals.event(hashes.load(Ordering::Relaxed), height, hashing)
                            );
                        }
                        if upgrade && fast_ready(&fast, seed) {
                            reason.store(STOP_RESTART, Ordering::SeqCst);
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                        if let Ok(info) = client.get::<Value>("v1/info").await
                            && template_is_stale(&info, height)
                        {
                            reason.store(STOP_STALE, Ordering::SeqCst);
                            stop.store(true, Ordering::SeqCst);
                            break;
                        }
                    }
                })
            };
            let mined = tokio::task::spawn_blocking(move || {
                let result = recv.recv().ok();
                for worker in workers {
                    worker
                        .join()
                        .map_err(|_| anyhow::anyhow!("mining worker panicked"))??;
                }
                Ok::<_, anyhow::Error>(result)
            })
            .await??;
            watcher.abort();
            hashing_seconds += hashing_start.elapsed().as_secs_f64();
            match reason.load(Ordering::SeqCst) {
                STOP_STALE => break (None, mode),
                STOP_RESTART if mined.is_none() => round += 1,
                _ => break (mined, mode),
            }
        };
        if let Some(block) = mined {
            let submission_start = Instant::now();
            match client.post::<Value>("v1/mine/submit", &block).await {
                Ok(v) if v["became_best"] == true => accepted += 1,
                Ok(_) => rejected += 1,
                Err(e) => {
                    rejected += 1;
                    eprintln!("block rejected: {e}");
                }
            }
            submission_seconds += submission_start.elapsed().as_secs_f64();
        }
        let totals = Totals {
            start,
            template_seconds,
            hashing_seconds,
            submission_seconds,
            accepted,
            rejected,
            threads: args.threads,
            mode,
        };
        println!(
            "{}",
            totals.event(hashes.load(Ordering::Relaxed), height, hashing_seconds)
        );
    }
    Ok(())
}

/// Interval of progress reports while one template is searched.
const PROGRESS_EVERY: Duration = Duration::from_secs(10);

/// Running totals behind each `{"event":"mining"}` report.
#[derive(Clone, Copy)]
struct Totals {
    start: Instant,
    template_seconds: f64,
    hashing_seconds: f64,
    submission_seconds: f64,
    accepted: u64,
    rejected: u64,
    threads: usize,
    mode: &'static str,
}
impl Totals {
    fn event(&self, hashes: u64, height: u64, hashing_seconds: f64) -> Value {
        json!({"event":"mining","hashrate_hs":hashes as f64/hashing_seconds.max(0.001),"hashrate_basis":"hashes / hashing wall time, including RandomX cache setup; excludes template and submission RPC","elapsed_seconds":self.start.elapsed().as_secs_f64(),"template_seconds":self.template_seconds,"hashing_seconds":hashing_seconds,"submission_seconds":self.submission_seconds,"hashes":hashes,"accepted_blocks":self.accepted,"rejected_blocks":self.rejected,"height":height,"threads":self.threads,"randomx_mode":self.mode})
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn thread_limit_follows_the_cpu_count_within_bounds() {
        let light = max_threads(RandomXMode::Light);
        let fast = max_threads(RandomXMode::Auto);
        assert!((1..=MAX_LIGHT_THREADS).contains(&light));
        assert!((light..=MAX_FAST_THREADS).contains(&fast));
        if let Ok(cpus) = std::thread::available_parallelism() {
            assert_eq!(light, usize::from(cpus).min(MAX_LIGHT_THREADS));
            assert_eq!(fast, usize::from(cpus).min(MAX_FAST_THREADS));
        }
    }

    #[test]
    fn auto_mode_is_the_default_and_light_stays_selectable() {
        let base = ["genoryn-miner", "--token-file", "t", "--wallet", "gnr1"];
        assert_eq!(Args::parse_from(base).randomx_mode, RandomXMode::Auto);
        let light = Args::parse_from(base.iter().copied().chain(["--randomx-mode", "light"]));
        assert_eq!(light.randomx_mode, RandomXMode::Light);
    }

    #[test]
    fn a_failed_fast_mode_is_not_retried_and_never_reported_ready() {
        let fast = SharedFast::default();
        fast_failed(&fast, &anyhow::anyhow!("no memory"));
        assert!(fast_dataset(&fast, [1; 32]).is_none());
        let state = fast.lock().unwrap();
        assert!(state.building.is_none(), "no build starts once unavailable");
        assert!(state.unavailable && state.ready.is_none());
        drop(state);
        assert!(!fast_ready(&fast, [1; 32]));
    }

    #[test]
    fn a_seed_being_built_is_not_built_twice() {
        let fast = SharedFast::default();
        fast.lock().unwrap().building = Some([2; 32]);
        assert!(fast_dataset(&fast, [2; 32]).is_none());
        let state = fast.lock().unwrap();
        assert_eq!(state.generation, 0, "no second build was started");
        assert_eq!(state.building, Some([2; 32]));
    }

    #[test]
    fn failures_keep_their_kind_through_context() {
        let error = Failure::Unauthorized.wrap("401".into()).context("outer");
        assert_eq!(
            error.downcast_ref::<Failure>(),
            Some(&Failure::Unauthorized)
        );
        let error = anyhow::anyhow!("no cache").context(Failure::RandomX);
        assert_eq!(
            format!("{error:#}"),
            "RandomX could not start on this computer: no cache"
        );
    }

    #[test]
    fn rpc_rejections_split_into_fatal_and_temporary() {
        let rejected = |status, body: &str| {
            anyhow::Error::new(genoryn::client::Rejected {
                status,
                body: body.into(),
            })
        };
        assert_eq!(
            classify_rpc(&rejected(401, "")),
            Some(Failure::Unauthorized)
        );
        assert_eq!(
            classify_rpc(&rejected(400, "{\"error\":\"invalid authentication\"}")),
            Some(Failure::Unauthorized),
            "nodes before v0.4.0 answer 400"
        );
        assert_eq!(classify_rpc(&rejected(429, "busy")), None);
        assert_eq!(classify_rpc(&rejected(503, "")), None);
        assert_eq!(classify_rpc(&rejected(400, "template unavailable")), None);
        assert_eq!(classify_rpc(&anyhow::anyhow!("connection refused")), None);
    }

    #[test]
    fn a_template_goes_stale_when_its_height_is_taken() {
        assert!(!template_is_stale(&json!({"height": 9}), 10));
        assert!(template_is_stale(&json!({"height": 10}), 10));
        assert!(template_is_stale(&json!({"height": 11}), 10));
        assert!(
            !template_is_stale(&json!({}), 10),
            "an unreadable reply keeps mining"
        );
    }
}

//! `genoryn-pool`: an open-source, independently operable GENORYN mining pool.
//!
//! - `serve`: miner gateway, job distribution from the operator's own node,
//!   real RandomX share verification, durable share accounting, block
//!   submission, the accountant, and a read-only statistics API.
//! - `account`: the accountant alone (for gateways on several machines that
//!   share one PostgreSQL ledger; run exactly one).
//! - `payout`: pays matured balances with ordinary shielded transfers from the
//!   pool wallet, idempotently and crash-safely.
//! - `status`: the ledger's audit and recent blocks.
//! - `bench`: measured PPLNS, ledger and RandomX verification capacity, and a
//!   connection load generator.
//!
//! The pool is custodial: rewards sit in the operator's wallet between
//! maturity and payout. Solo mining stays available and needs no pool.
use anyhow::{Context, Result, ensure};
use clap::{Args as ClapArgs, Parser, Subcommand};
use genoryn::client::RpcClient;
use genoryn_core::{chain::unix_time, params::Network};
use genoryn_pool::{
    config::PoolPolicy,
    jobs::{Job, JobBook},
    pplns,
    store::{self, ChainSnapshot, Store},
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{
    collections::BTreeMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[path = "pool/bench.rs"]
mod bench;
#[path = "pool/gateway.rs"]
mod gateway;
#[path = "pool/payout.rs"]
mod payout;

#[derive(Parser)]
#[command(name = "genoryn-pool", about = "GENORYN shared mining pool (PPLNS)")]
struct Args {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    Serve(ServeArgs),
    Account(AccountArgs),
    Payout(payout::PayoutArgs),
    Status(LedgerArgs),
    Bench(bench::BenchArgs),
}

#[derive(ClapArgs, Clone)]
struct LedgerArgs {
    #[arg(long, default_value = "mainnet")]
    network: String,
    /// SQLite ledger file (one machine).
    #[arg(long, conflicts_with = "ledger_url")]
    ledger: Option<PathBuf>,
    /// PostgreSQL ledger (`postgresql://…`), shared by several gateways.
    /// Prefer --ledger-url-file when the URL holds a password.
    #[arg(long)]
    ledger_url: Option<String>,
    /// File holding the PostgreSQL URL, so no password appears in argv.
    #[arg(long)]
    ledger_url_file: Option<PathBuf>,
    #[command(flatten)]
    policy: PolicyArgs,
}

#[derive(ClapArgs, Clone)]
struct PolicyArgs {
    /// First height whose pool blocks are shared (mainnet default 1150).
    #[arg(long)]
    activation_height: Option<u64>,
    /// Confirms a new activation height after the default one has passed.
    #[arg(long)]
    confirm_activation_height: bool,
    #[arg(long, default_value_t = 100)]
    fee_bps: u16,
    #[arg(long, default_value_t = 2)]
    window_factor: u32,
    #[arg(long, default_value_t = 100_000_000_000)]
    payout_threshold: u64,
    #[arg(long, default_value_t = 15)]
    payout_batch: usize,
    #[arg(long)]
    initial_share_difficulty: Option<u64>,
    #[arg(long)]
    min_share_difficulty: Option<u64>,
    #[arg(long)]
    share_seconds: Option<u64>,
}

#[derive(ClapArgs, Clone)]
struct NodeArgs {
    /// The operator's own node (admin RPC, loopback only).
    #[arg(long, default_value = "http://127.0.0.1:29711")]
    node: String,
    #[arg(long)]
    token_file: PathBuf,
}

#[derive(ClapArgs)]
struct ServeArgs {
    #[command(flatten)]
    ledger: LedgerArgs,
    #[command(flatten)]
    node: NodeArgs,
    /// The pool wallet's address: every pool block's coinbase pays it.
    #[arg(long)]
    pool_address: String,
    #[arg(long, default_value = "127.0.0.1:29790")]
    listen: Vec<SocketAddr>,
    /// Read-only statistics API; keep it on loopback unless it is meant public.
    #[arg(long, default_value = "127.0.0.1:29791")]
    api: SocketAddr,
    #[arg(long, default_value_t = 100_000)]
    max_connections: usize,
    #[arg(long, default_value_t = 64)]
    max_per_ip: usize,
    /// Share verification threads (each holds a 256 MiB RandomX cache).
    #[arg(long, default_value_t = 4)]
    verify_threads: usize,
    #[arg(long, default_value_t = 4_096)]
    verify_queue: usize,
    /// Verify shares with full-memory RandomX: one shared 2 GiB dataset per
    /// seed, identical hashes, several times the light-mode throughput.
    #[arg(long, default_value_t = false, action = clap::ArgAction::Set)]
    fast_verify: bool,
    /// Name of this gateway in the shared ledger.
    #[arg(long, default_value = "gateway-1")]
    gateway: String,
    /// Run the accountant in this process (exactly one per ledger).
    #[arg(long, default_value_t = true, action = clap::ArgAction::Set)]
    account: bool,
}

#[derive(ClapArgs)]
struct AccountArgs {
    #[command(flatten)]
    ledger: LedgerArgs,
    #[command(flatten)]
    node: NodeArgs,
    #[arg(long, default_value_t = 10)]
    interval_seconds: u64,
    #[arg(long)]
    once: bool,
}

impl LedgerArgs {
    fn network(&self) -> Result<Network> {
        Network::parse(&self.network)
    }
    fn policy(&self) -> Result<PoolPolicy> {
        let network = self.network()?;
        let mut policy = PoolPolicy::defaults(network);
        let p = &self.policy;
        if let Some(height) = p.activation_height {
            policy.activation_height = height;
        }
        policy.fee_bps = p.fee_bps;
        policy.window_factor = p.window_factor;
        policy.payout_threshold = p.payout_threshold;
        policy.payout_batch = p.payout_batch;
        if let Some(value) = p.initial_share_difficulty {
            policy.vardiff.initial = value;
        }
        if let Some(value) = p.min_share_difficulty {
            policy.vardiff.min = value;
        }
        if let Some(value) = p.share_seconds {
            policy.vardiff.target_seconds = value;
            policy.vardiff.retarget_seconds = policy.vardiff.retarget_seconds.max(value);
        }
        policy.validate()?;
        Ok(policy)
    }
    fn open(&self) -> Result<Store> {
        let from_file = self
            .ledger_url_file
            .as_ref()
            .map(|path| -> Result<String> {
                ensure!(
                    std::fs::metadata(path)?.len() <= 4_096,
                    "ledger URL file too large"
                );
                Ok(std::fs::read_to_string(path)?.trim().to_owned())
            })
            .transpose()?;
        match (&self.ledger, self.ledger_url.clone().or(from_file)) {
            (Some(path), None) => Store::open_sqlite(path),
            (None, Some(url)) => Store::open_postgres(&url),
            _ => anyhow::bail!(
                "choose one ledger: --ledger FILE, or --ledger-url / --ledger-url-file for PostgreSQL"
            ),
        }
    }
}

async fn node_client(args: &NodeArgs, network: Network) -> Result<(RpcClient, u64)> {
    let rpc = RpcClient::new(&args.node, Some(&args.token_file))?;
    let info: Value = rpc
        .get("v1/info")
        .await
        .context("the pool's node did not answer")?;
    ensure!(
        info["chain_id"] == network.chain_id()
            && info["genesis_hash"] == hex::encode(network.genesis()),
        "the node runs another network or genesis"
    );
    Ok((rpc, info["height"].as_u64().context("node height")?))
}

async fn blocking<T: Send + 'static>(
    store: &Arc<Mutex<Store>>,
    work: impl FnOnce(&mut Store) -> Result<T> + Send + 'static,
) -> Result<T> {
    let store = store.clone();
    tokio::task::spawn_blocking(move || {
        let mut store = store
            .lock()
            .map_err(|_| anyhow::anyhow!("ledger lock poisoned"))?;
        work(&mut store)
    })
    .await?
}

/// Reads the node's chain at every tracked block's height, then runs one
/// accounting pass on that snapshot.
async fn account_once(
    rpc: &RpcClient,
    store: &Arc<Mutex<Store>>,
    policy: &PoolPolicy,
) -> Result<store::Progress> {
    let info: Value = rpc.get("v1/info").await?;
    let tip = info["height"].as_u64().context("node height")?;
    let watch = policy.reorg_watch;
    let tracked = blocking(store, move |store| store.tracked_blocks(tip, watch)).await?;
    let mut snapshot = ChainSnapshot {
        tip,
        hashes: BTreeMap::new(),
    };
    for height in tracked.iter().map(|block| block.height) {
        if height == 0 || height > tip || snapshot.hashes.contains_key(&height) {
            continue;
        }
        let page: Vec<genoryn_core::block::Block> =
            rpc.get(&format!("v1/blocks?from={height}&limit=1")).await?;
        if let Some(block) = page.first().filter(|b| b.header.height == height) {
            snapshot.hashes.insert(height, hex::encode(block.hash()?));
        }
    }
    let policy = policy.clone();
    blocking(store, move |store| {
        store::account(store, &mut snapshot, &policy, unix_time())
    })
    .await
}

async fn accountant(rpc: RpcClient, store: Arc<Mutex<Store>>, policy: PoolPolicy, every: Duration) {
    loop {
        match account_once(&rpc, &store, &policy).await {
            Ok(progress)
                if !(progress.allocated.is_empty()
                    && progress.matured.is_empty()
                    && progress.orphaned.is_empty()
                    && progress.reversed.is_empty()) =>
            {
                println!("{}", json!({"event": "accounting", "progress": progress}));
            }
            Ok(_) => {}
            Err(error) => eprintln!("accounting pass failed (retrying): {error:#}"),
        }
        tokio::time::sleep(every).await;
    }
}

fn open_ledger(args: &LedgerArgs, tip: u64) -> Result<(Store, PoolPolicy)> {
    let policy = args.policy()?;
    let mut store = args.open()?;
    let has_state = store.has_state()?;
    policy.check_activation(tip, has_state, args.policy.confirm_activation_height)?;
    store.bind(&policy)?;
    Ok((store, policy))
}

async fn serve(args: ServeArgs) -> Result<()> {
    let network = args.ledger.network()?;
    genoryn_wallet_core::keys::decode_address(network, &args.pool_address)
        .context("--pool-address must be a GENORYN address of this network")?;
    let (rpc, tip) = node_client(&args.node, network).await?;
    let (store, policy) = open_ledger(&args.ledger, tip)?;
    let store = Arc::new(Mutex::new(store));
    let (verify_tx, mut found) = gateway::start_workers(
        store.clone(),
        args.gateway.clone(),
        args.verify_threads.clamp(1, 256),
        args.verify_queue.max(16),
        args.fast_verify.then(gateway::SharedFast::default),
    );
    let (job_tx, _) = tokio::sync::watch::channel(None);
    let shared = Arc::new(gateway::Shared {
        network,
        policy: policy.clone(),
        gateway: args.gateway.clone(),
        store: store.clone(),
        jobs: std::sync::RwLock::new(JobBook::default()),
        job_tx,
        next_prefix: std::sync::atomic::AtomicU32::new(rand08::random::<u32>()),
        verify_tx,
        stats: Default::default(),
        open: std::sync::atomic::AtomicBool::new(false),
        max_per_ip: args.max_per_ip.max(1),
        per_ip: Mutex::new(Default::default()),
        refresh: tokio::sync::Notify::new(),
    });
    // Templates from the operator's node; a new tip retires older jobs.
    {
        let (shared, rpc, address) = (shared.clone(), rpc.clone(), args.pool_address.clone());
        tokio::spawn(async move {
            let mut last_tip = None;
            let mut last_template = Instant::now() - Duration::from_secs(3600);
            let mut counter = 0u64;
            loop {
                if let Ok(info) = rpc.get::<Value>("v1/info").await
                    && let Some(tip) = info["height"].as_u64()
                {
                    shared
                        .jobs
                        .write()
                        .unwrap_or_else(|e| e.into_inner())
                        .set_tip(tip);
                    let open = shared.policy.eligible(tip + 1);
                    shared
                        .open
                        .store(open, std::sync::atomic::Ordering::Release);
                    if open
                        && (last_tip != Some(tip)
                            || last_template.elapsed() >= Duration::from_secs(30))
                    {
                        match rpc
                            .post::<genoryn_core::block::Block>(
                                "v1/mine/template",
                                &json!({"wallet": address}),
                            )
                            .await
                        {
                            Ok(block) if block.header.height == tip + 1 => {
                                counter += 1;
                                match Job::new(format!("{}-{counter}", shared.gateway), block, 0) {
                                    Ok(job) => {
                                        let job = Arc::new(job);
                                        shared
                                            .jobs
                                            .write()
                                            .unwrap_or_else(|e| e.into_inner())
                                            .add(job.clone());
                                        let _ = shared.job_tx.send(Some(job));
                                        last_tip = Some(tip);
                                        last_template = Instant::now();
                                    }
                                    Err(error) => eprintln!("template unusable: {error:#}"),
                                }
                            }
                            Ok(_) => {}
                            Err(error) => eprintln!("template unavailable (retrying): {error:#}"),
                        }
                    }
                }
                tokio::select! {
                    _ = tokio::time::sleep(Duration::from_secs(1)) => {}
                    _ = shared.refresh.notified() => {}
                }
            }
        });
    }
    // Found blocks go to the node at once; a refused one is marked rejected.
    {
        let (shared, rpc, store) = (shared.clone(), rpc.clone(), store.clone());
        tokio::spawn(async move {
            while let Some(found) = found.recv().await {
                let height = found.block.header.height;
                let accepted = matches!(
                    rpc.post::<Value>("v1/mine/submit", &found.block).await,
                    Ok(answer) if answer["became_best"] == true
                );
                println!(
                    "{}",
                    json!({"event": "block_found", "height": height, "accepted": accepted, "ledger_id": found.id})
                );
                if !accepted {
                    let id = found.id;
                    let _ =
                        blocking(&store, move |store| store.reject_block(id, unix_time())).await;
                }
                shared.refresh.notify_one();
            }
        });
    }
    if args.account {
        tokio::spawn(accountant(
            rpc.clone(),
            store.clone(),
            policy.clone(),
            Duration::from_secs(5),
        ));
    }
    tokio::spawn(api(shared.clone(), args.api));
    println!(
        "{}",
        json!({"event": "pool_ready", "network": network.name(), "ledger": store.lock().map(|s| s.driver()).unwrap_or("?"), "activation_height": policy.activation_height, "fee_bps": policy.fee_bps, "window_factor": policy.window_factor, "listen": args.listen, "api": args.api})
    );
    let mut listeners = Vec::new();
    for address in &args.listen {
        listeners.push(tokio::spawn(gateway::listen(
            shared.clone(),
            *address,
            args.max_connections,
        )));
    }
    for listener in listeners {
        listener.await??;
    }
    Ok(())
}

#[derive(Serialize)]
struct MinerReport {
    #[serde(flatten)]
    view: store::MinerView,
    /// Expected credit per pool block from the current window (an estimate,
    /// not earnings: nothing is earned until the pool finds a block).
    estimated_per_block_gori: String,
    states: &'static str,
}

async fn api(shared: Arc<gateway::Shared>, address: SocketAddr) {
    use axum::{Json, Router, extract::Query, routing::get};
    let status = {
        let shared = shared.clone();
        move || {
            let shared = shared.clone();
            async move {
                let tip = shared.jobs.read().map(|book| book.tip()).unwrap_or(0);
                let blocks = blocking(&shared.store, |store| store.blocks(20))
                    .await
                    .unwrap_or_default();
                let audit = blocking(&shared.store, |store| store.audit()).await.ok();
                Json(json!({
                    "network": shared.network.name(),
                    "gateway": shared.gateway,
                    "tip": tip,
                    "open": shared.open.load(std::sync::atomic::Ordering::Acquire),
                    "policy": shared.policy,
                    "stats": shared.stats.snapshot(),
                    "recent_blocks": blocks,
                    "audit": audit,
                    "custody": "custodial pool: rewards are held in the operator's wallet until paid",
                }))
            }
        }
    };
    let miner = {
        let shared = shared.clone();
        move |Query(query): Query<BTreeMap<String, String>>| {
            let shared = shared.clone();
            async move {
                let Some(address) = query.get("address").cloned() else {
                    return Json(json!({"error": "address required"}));
                };
                let difficulty = shared
                    .jobs
                    .read()
                    .ok()
                    .and_then(|book| book.latest())
                    .map_or(0, |job| job.difficulty());
                let target = pplns::window_target(difficulty, shared.policy.window_factor);
                let reward = shared
                    .jobs
                    .read()
                    .ok()
                    .and_then(|b| b.latest())
                    .map_or(0, |j| j.reward);
                let fee_bps = shared.policy.fee_bps;
                match blocking(&shared.store, move |store| {
                    store.miner_view(&address, target)
                })
                .await
                {
                    Ok(Some(view)) => {
                        let distributable =
                            u128::from(reward) * u128::from(10_000 - fee_bps) / 10_000;
                        let estimate = if view.pool_window_work == 0 {
                            0
                        } else {
                            distributable * u128::from(view.window_work)
                                / u128::from(view.pool_window_work)
                        };
                        Json(serde_json::to_value(MinerReport {
                            view,
                            estimated_per_block_gori: estimate.to_string(),
                            states: "window_work: verified shares; estimated: per pool block; immature: found, not 60 blocks deep; matured: payable; pending: in a payout transaction; paid: confirmed on chain. Amounts *_fixed are in 2^-32 gori.",
                        }).unwrap_or_default())
                    }
                    Ok(None) => Json(json!({"error": "unknown miner"})),
                    Err(error) => Json(json!({"error": error.to_string()})),
                }
            }
        }
    };
    let router = Router::new()
        .route("/v1/pool/status", get(status))
        .route("/v1/pool/miner", get(miner));
    match tokio::net::TcpListener::bind(address).await {
        Ok(listener) => {
            let _ = axum::serve(listener, router).await;
        }
        Err(error) => eprintln!("statistics API unavailable on {address}: {error}"),
    }
}

async fn account(args: AccountArgs) -> Result<()> {
    let network = args.ledger.network()?;
    let (rpc, tip) = node_client(&args.node, network).await?;
    let (store, policy) = open_ledger(&args.ledger, tip)?;
    let store = Arc::new(Mutex::new(store));
    if args.once {
        let progress = account_once(&rpc, &store, &policy).await?;
        println!("{}", serde_json::to_string(&progress)?);
        return Ok(());
    }
    accountant(
        rpc,
        store,
        policy,
        Duration::from_secs(args.interval_seconds.max(1)),
    )
    .await;
    Ok(())
}

fn status(args: LedgerArgs) -> Result<()> {
    let policy = args.policy()?;
    let mut store = args.open()?;
    store.bind(&policy)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "driver": store.driver(),
            "policy": policy,
            "audit": store.audit()?,
            "recent_blocks": store.blocks(20)?,
            "open_payout_batches": store.open_batches()?,
        }))?
    );
    Ok(())
}

#[tokio::main]
async fn main() -> std::process::ExitCode {
    let result = match Args::parse().command {
        Command::Serve(args) => serve(args).await,
        Command::Account(args) => account(args).await,
        Command::Payout(args) => payout::run(args).await,
        Command::Status(args) => status(args),
        Command::Bench(args) => bench::run(args).await,
    };
    match result {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            eprintln!("genoryn-pool: {error:#}");
            println!(
                "{}",
                json!({"event": "error", "reason": format!("{error:#}")})
            );
            std::process::ExitCode::FAILURE
        }
    }
}

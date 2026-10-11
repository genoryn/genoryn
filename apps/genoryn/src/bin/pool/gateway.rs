//! The mining gateway: miner sessions, job distribution, share verification
//! and the durable share sink.
//!
//! - Sessions speak the line protocol (`genoryn_pool::protocol`); each gets
//!   its own nonce range and VarDiff, and is limited in rate and line size.
//! - Verification runs on a fixed set of threads, each with its own RandomX
//!   VM; the queue in front of them is bounded, so overload answers "busy"
//!   instead of growing memory.
//! - The sink commits verified work in batches and only then acknowledges
//!   the shares; a block a share found is recorded after the work that found
//!   it, then submitted to the node.
use super::*;
use genoryn_pool::{
    jobs::{self, Job, JobBook, SeenShares},
    pplns::MinerId,
    protocol::{self, ClientMessage, Receipt, Reject, Request, ServerMessage},
    vardiff::SessionDifficulty,
};
use std::{
    collections::HashMap,
    net::IpAddr,
    sync::{
        RwLock,
        atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
        mpsc as std_mpsc,
    },
};
use tokio::{
    io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader},
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
};

/// Shares a session may submit per second on average (burst of four times).
const SHARES_PER_SECOND: f64 = 4.0;
/// Invalid shares after which a session is closed.
const MAX_INVALID: u32 = 32;
/// A share at the previous difficulty is still accepted this long after a change.
const DIFFICULTY_GRACE_MS: u64 = 10_000;
const LOGIN_TIMEOUT: Duration = Duration::from_secs(30);
const IDLE_TIMEOUT: Duration = Duration::from_secs(600);
/// The sink commits at least this often while shares arrive.
const SINK_INTERVAL: Duration = Duration::from_millis(200);
const SINK_MAX_BATCH: usize = 4_096;

#[derive(Default)]
pub struct Stats {
    pub connections: AtomicU64,
    pub sessions_total: AtomicU64,
    pub shares_accepted: AtomicU64,
    pub shares_rejected: AtomicU64,
    pub work_accepted: AtomicU64,
    pub blocks_found: AtomicU64,
    pub busy_rejections: AtomicU64,
}
impl Stats {
    pub fn snapshot(&self) -> Value {
        let load = |counter: &AtomicU64| counter.load(Ordering::Relaxed);
        json!({
            "connections": load(&self.connections),
            "sessions_total": load(&self.sessions_total),
            "shares_accepted": load(&self.shares_accepted),
            "shares_rejected": load(&self.shares_rejected),
            "work_accepted": load(&self.work_accepted),
            "blocks_found": load(&self.blocks_found),
            "busy_rejections": load(&self.busy_rejections),
        })
    }
}

pub struct Shared {
    pub network: Network,
    pub policy: PoolPolicy,
    pub gateway: String,
    pub store: Arc<Mutex<Store>>,
    pub jobs: RwLock<JobBook>,
    pub job_tx: watch::Sender<Option<Arc<Job>>>,
    pub next_prefix: AtomicU32,
    pub verify_tx: std_mpsc::SyncSender<VerifyTask>,
    pub stats: Stats,
    pub open: AtomicBool,
    pub max_per_ip: usize,
    pub per_ip: Mutex<HashMap<IpAddr, usize>>,
    pub refresh: tokio::sync::Notify,
}

pub struct VerifyTask {
    job: Arc<Job>,
    nonce: u64,
    difficulty: u64,
    miner: MinerId,
    reply: oneshot::Sender<Result<Receipt, Reject>>,
}

struct SinkItem {
    task: VerifyTask,
    hash: [u8; 32],
    block: bool,
}

pub struct BlockFound {
    pub id: i64,
    pub block: genoryn_core::block::Block,
}

/// The full-memory RandomX dataset for the current seed, built once in the
/// background and shared by every verification thread. Until it is ready
/// (about a minute per seed) shares are verified in light mode.
#[derive(Default)]
pub struct FastDatasets {
    ready: Option<genoryn_core::pow::FastDataset>,
    building: Option<[u8; 32]>,
    unavailable: bool,
}
pub type SharedFast = Arc<Mutex<FastDatasets>>;
fn fast_vm_for(fast: &SharedFast, seed: [u8; 32]) -> Option<genoryn_core::pow::FastVm> {
    let mut state = fast.lock().ok()?;
    if let Some(dataset) = state.ready.as_ref().filter(|d| d.seed() == seed) {
        return dataset.vm().ok();
    }
    if state.unavailable || state.building == Some(seed) {
        return None;
    }
    state.building = Some(seed);
    let fast = fast.clone();
    std::thread::spawn(move || {
        let built = genoryn_core::pow::FastDataset::new(seed);
        if let Ok(mut state) = fast.lock() {
            state.building = None;
            match built {
                Ok(dataset) => state.ready = Some(dataset),
                Err(error) => {
                    eprintln!(
                        "full-memory RandomX unavailable; shares stay verified in light mode: {error:#}"
                    );
                    state.unavailable = true;
                }
            }
        }
    });
    None
}

/// Starts `threads` verification workers and the sink. Returns the queue
/// sender and a receiver of found blocks for the submitter.
pub fn start_workers(
    store: Arc<Mutex<Store>>,
    gateway: String,
    threads: usize,
    queue: usize,
    fast: Option<SharedFast>,
) -> (
    std_mpsc::SyncSender<VerifyTask>,
    mpsc::UnboundedReceiver<BlockFound>,
) {
    let (verify_tx, verify_rx) = std_mpsc::sync_channel::<VerifyTask>(queue);
    let verify_rx = Arc::new(Mutex::new(verify_rx));
    let (sink_tx, sink_rx) = std_mpsc::channel::<SinkItem>();
    let (block_tx, block_rx) = mpsc::unbounded_channel();
    for index in 0..threads {
        let (verify_rx, sink_tx, fast) = (verify_rx.clone(), sink_tx.clone(), fast.clone());
        std::thread::Builder::new()
            .name(format!("verify-{index}"))
            .spawn(move || {
                let mut vm: Option<([u8; 32], genoryn_core::pow::FastVm)> = None;
                loop {
                    let task = match verify_rx.lock().map(|rx| rx.recv()) {
                        Ok(Ok(task)) => task,
                        _ => return,
                    };
                    let seed = task.job.seed();
                    if let Some(fast) = &fast
                        && vm.as_ref().is_none_or(|(known, _)| *known != seed)
                    {
                        vm = fast_vm_for(fast, seed).map(|machine| (seed, machine));
                    }
                    let checked = match &vm {
                        Some((_, machine)) => jobs::verify_share_with(
                            &task.job,
                            task.nonce,
                            task.difficulty,
                            |_, bytes| machine.hash(bytes),
                        ),
                        None => jobs::verify_share(&task.job, task.nonce, task.difficulty),
                    };
                    match checked {
                        Ok(checked) => {
                            let _ = sink_tx.send(SinkItem {
                                task,
                                hash: checked.hash,
                                block: checked.block,
                            });
                        }
                        Err(reject) => {
                            let _ = task.reply.send(Err(reject));
                        }
                    }
                }
            })
            .expect("verification thread");
    }
    drop(sink_tx);
    std::thread::Builder::new()
        .name("share-sink".into())
        .spawn(move || sink(store, gateway, sink_rx, block_tx))
        .expect("sink thread");
    (verify_tx, block_rx)
}

fn sink(
    store: Arc<Mutex<Store>>,
    gateway: String,
    rx: std_mpsc::Receiver<SinkItem>,
    blocks: mpsc::UnboundedSender<BlockFound>,
) {
    loop {
        let Ok(first) = rx.recv() else {
            return;
        };
        let mut batch = vec![first];
        let deadline = Instant::now() + SINK_INTERVAL;
        while batch.len() < SINK_MAX_BATCH {
            let left = deadline.saturating_duration_since(Instant::now());
            match rx.recv_timeout(left) {
                Ok(item) => batch.push(item),
                Err(_) => break,
            }
        }
        let mut per_miner: BTreeMap<MinerId, (u64, u32)> = BTreeMap::new();
        for item in &batch {
            let entry = per_miner.entry(item.task.miner).or_insert((0, 0));
            entry.0 = entry.0.saturating_add(item.task.difficulty);
            entry.1 += 1;
        }
        let rows: Vec<(MinerId, u64, u32)> = per_miner
            .iter()
            .map(|(miner, (work, shares))| (*miner, *work, *shares))
            .collect();
        let now = unix_time();
        let committed = store
            .lock()
            .map_err(|_| anyhow::anyhow!("ledger lock poisoned"))
            .and_then(|mut store| {
                let seqs = store.record_work(&gateway, &rows, now)?;
                let mut found = Vec::new();
                let last = seqs.iter().copied().max().unwrap_or(0);
                for item in batch.iter().filter(|item| item.block) {
                    let block = item.task.job.block_with(item.task.nonce);
                    let id = store.record_block(
                        block.header.height,
                        &hex::encode(block.hash()?),
                        item.task.job.reward,
                        item.task.job.difficulty(),
                        last,
                        &gateway,
                        now,
                    )?;
                    found.push(BlockFound { id, block });
                }
                Ok((seqs, found))
            });
        match committed {
            Ok((seqs, found)) => {
                let sequence: HashMap<MinerId, i64> = rows
                    .iter()
                    .zip(&seqs)
                    .map(|((miner, _, _), seq)| (*miner, *seq))
                    .collect();
                for block in found {
                    let _ = blocks.send(block);
                }
                for item in batch {
                    let receipt = Receipt {
                        job_id: item.task.job.id.clone(),
                        height: item.task.job.height(),
                        nonce: protocol::format_nonce(item.task.nonce),
                        hash: hex::encode(item.hash),
                        work: item.task.difficulty,
                        sequence: sequence.get(&item.task.miner).copied().unwrap_or(0),
                        block_candidate: item.block,
                    };
                    let _ = item.task.reply.send(Ok(receipt));
                }
            }
            Err(error) => {
                // Nothing was committed, so nothing is acknowledged: the miner
                // sees "busy" and no credit is claimed for these shares.
                eprintln!(
                    "share sink could not commit {} shares: {error:#}",
                    batch.len()
                );
                for item in batch {
                    let _ = item.task.reply.send(Err(Reject::Busy));
                }
            }
        }
    }
}

struct TokenBucket {
    tokens: f64,
    at: Instant,
}
impl TokenBucket {
    fn new() -> Self {
        Self {
            tokens: SHARES_PER_SECOND * 4.0,
            at: Instant::now(),
        }
    }
    fn take(&mut self) -> bool {
        let now = Instant::now();
        self.tokens = (self.tokens + now.duration_since(self.at).as_secs_f64() * SHARES_PER_SECOND)
            .min(SHARES_PER_SECOND * 4.0);
        self.at = now;
        if self.tokens >= 1.0 {
            self.tokens -= 1.0;
            true
        } else {
            false
        }
    }
}

fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as u64)
}

/// Reads one line of at most `MAX_LINE_BYTES`; `None` at end of stream.
async fn read_line<R: tokio::io::AsyncBufRead + Unpin>(reader: &mut R) -> Result<Option<String>> {
    let mut bytes = Vec::new();
    let read = reader
        .take(protocol::MAX_LINE_BYTES as u64 + 1)
        .read_until(b'\n', &mut bytes)
        .await?;
    if read == 0 {
        return Ok(None);
    }
    ensure!(
        bytes.len() <= protocol::MAX_LINE_BYTES && bytes.ends_with(b"\n"),
        "line too long"
    );
    Ok(Some(String::from_utf8(bytes)?.trim().to_owned()))
}

pub async fn listen(
    shared: Arc<Shared>,
    address: SocketAddr,
    max_connections: usize,
) -> Result<()> {
    let listener = TcpListener::bind(address)
        .await
        .with_context(|| format!("cannot listen for miners on {address}"))?;
    let permits = Arc::new(tokio::sync::Semaphore::new(max_connections));
    eprintln!("genoryn-pool: miners connect to {address}");
    loop {
        let (stream, peer) = listener.accept().await?;
        let Ok(permit) = permits.clone().try_acquire_owned() else {
            drop(stream);
            continue;
        };
        let ip = peer.ip();
        {
            let mut per_ip = shared.per_ip.lock().unwrap_or_else(|e| e.into_inner());
            let count = per_ip.entry(ip).or_insert(0);
            if *count >= shared.max_per_ip {
                continue;
            }
            *count += 1;
        }
        let shared = shared.clone();
        tokio::spawn(async move {
            shared.stats.connections.fetch_add(1, Ordering::Relaxed);
            shared.stats.sessions_total.fetch_add(1, Ordering::Relaxed);
            let _ = stream.set_nodelay(true);
            let _ = session(shared.clone(), stream).await;
            shared.stats.connections.fetch_sub(1, Ordering::Relaxed);
            let mut per_ip = shared.per_ip.lock().unwrap_or_else(|e| e.into_inner());
            if let Some(count) = per_ip.get_mut(&ip) {
                *count -= 1;
                if *count == 0 {
                    per_ip.remove(&ip);
                }
            }
            drop(permit);
        });
    }
}

struct Session {
    miner: Option<MinerId>,
    prefix: u32,
    difficulty: SessionDifficulty,
    previous: Option<(u64, u64)>,
    seen: SeenShares,
    bucket: TokenBucket,
    invalid: u32,
}

async fn session(shared: Arc<Shared>, stream: TcpStream) -> Result<()> {
    let (read, mut write) = stream.into_split();
    let mut reader = BufReader::new(read);
    let (out_tx, mut out_rx) = mpsc::channel::<ServerMessage>(64);
    let writer = tokio::spawn(async move {
        while let Some(message) = out_rx.recv().await {
            let mut line = serde_json::to_vec(&message).unwrap_or_default();
            line.push(b'\n');
            if write.write_all(&line).await.is_err() {
                break;
            }
        }
    });
    let mut state = Session {
        miner: None,
        prefix: shared.next_prefix.fetch_add(1, Ordering::Relaxed),
        difficulty: SessionDifficulty::new(shared.policy.vardiff, now_ms()),
        previous: None,
        seen: SeenShares::default(),
        bucket: TokenBucket::new(),
        invalid: 0,
    };
    let mut jobs = shared.job_tx.subscribe();
    let mut retarget = tokio::time::interval(Duration::from_secs(15));
    let login_deadline = tokio::time::Instant::now() + LOGIN_TIMEOUT;
    let mut last_input = tokio::time::Instant::now();
    let result: Result<()> = async {
        loop {
            let idle = if state.miner.is_none() {
                login_deadline
            } else {
                last_input + IDLE_TIMEOUT
            };
            tokio::select! {
                line = read_line(&mut reader) => {
                    let Some(line) = line? else { return Ok(()) };
                    last_input = tokio::time::Instant::now();
                    if line.is_empty() { continue; }
                    let message = match protocol::parse_client(&line) {
                        Ok(message) => message,
                        Err(reject) => {
                            let _ = out_tx.send(ServerMessage::reject(0, reject)).await;
                            anyhow::bail!("malformed line");
                        }
                    };
                    handle(&shared, &mut state, message, &out_tx).await?;
                    if state.invalid > MAX_INVALID {
                        anyhow::bail!("too many invalid shares");
                    }
                }
                changed = jobs.changed() => {
                    changed?;
                    if state.miner.is_some() {
                        let job = jobs.borrow_and_update().clone();
                        if let Some(job) = job {
                            state.seen = SeenShares::default();
                            let _ = out_tx.send(ServerMessage::job(job.for_session(state.prefix, state.difficulty.current()))).await;
                        }
                    }
                }
                _ = retarget.tick() => {
                    let before = state.difficulty.current();
                    if let Some(next) = state.difficulty.on_timer(now_ms()) {
                        state.previous = Some((before, now_ms()));
                        send_job(&shared, &state, &out_tx, next).await;
                    }
                }
                _ = tokio::time::sleep_until(idle) => anyhow::bail!("session timed out"),
            }
        }
    }
    .await;
    drop(out_tx);
    let _ = writer.await;
    result
}

async fn send_job(
    shared: &Shared,
    state: &Session,
    out: &mpsc::Sender<ServerMessage>,
    difficulty: u64,
) {
    let job = shared.jobs.read().ok().and_then(|book| book.latest());
    if let Some(job) = job {
        let _ = out
            .send(ServerMessage::job(
                job.for_session(state.prefix, difficulty),
            ))
            .await;
    }
}

async fn handle(
    shared: &Arc<Shared>,
    state: &mut Session,
    message: ClientMessage,
    out: &mpsc::Sender<ServerMessage>,
) -> Result<()> {
    let id = message.id;
    let reply = |result: Result<Value, Reject>| match result {
        Ok(value) => ServerMessage::ok(id, value),
        Err(reject) => ServerMessage::reject(id, reject),
    };
    match message.request {
        Request::Keepalive => {
            let _ = out.send(reply(Ok(json!({"status": "alive"})))).await;
        }
        Request::Login {
            address, worker, ..
        } => {
            if genoryn_wallet_core::keys::decode_address(shared.network, &address).is_err() {
                let _ = out.send(reply(Err(Reject::InvalidAddress))).await;
                return Ok(());
            }
            if !shared.open.load(Ordering::Acquire) {
                let _ = out.send(reply(Err(Reject::NotOpenYet))).await;
                return Ok(());
            }
            let store = shared.store.clone();
            let miner = tokio::task::spawn_blocking(move || {
                store
                    .lock()
                    .map_err(|_| anyhow::anyhow!("ledger lock poisoned"))?
                    .miner(&address, unix_time())
            })
            .await??;
            state.miner = Some(miner);
            let _ = out
                .send(reply(Ok(json!({
                    "protocol": protocol::PROTOCOL,
                    "miner": miner,
                    "worker": worker.chars().take(64).collect::<String>(),
                    "share_difficulty": state.difficulty.current(),
                    "pool_fee_bps": shared.policy.fee_bps,
                    "pplns_window_factor": shared.policy.window_factor,
                    "payout_threshold_gori": shared.policy.payout_threshold.to_string(),
                    "custody": "The pool holds mined rewards in its own wallet until it pays them out. Payouts are ordinary private transfers.",
                }))))
                .await;
            send_job(shared, state, out, state.difficulty.current()).await;
        }
        Request::Submit { job_id, nonce } => {
            let outcome = submit(shared, state, &job_id, &nonce).await;
            match &outcome {
                Ok(receipt) => {
                    shared.stats.shares_accepted.fetch_add(1, Ordering::Relaxed);
                    shared
                        .stats
                        .work_accepted
                        .fetch_add(receipt.work, Ordering::Relaxed);
                    if receipt.block_candidate {
                        shared.stats.blocks_found.fetch_add(1, Ordering::Relaxed);
                    }
                }
                Err(reject) => {
                    shared.stats.shares_rejected.fetch_add(1, Ordering::Relaxed);
                    if matches!(
                        reject,
                        Reject::LowDifficulty | Reject::OutOfRange | Reject::Duplicate
                    ) {
                        state.invalid += 1;
                    }
                    if *reject == Reject::Busy {
                        shared.stats.busy_rejections.fetch_add(1, Ordering::Relaxed);
                    }
                }
            }
            let accepted = outcome.is_ok();
            let _ = out
                .send(reply(
                    outcome.map(|receipt| json!({"accepted": true, "receipt": receipt})),
                ))
                .await;
            if accepted {
                let before = state.difficulty.current();
                if let Some(next) = state.difficulty.on_share(now_ms()) {
                    state.previous = Some((before, now_ms()));
                    send_job(shared, state, out, next).await;
                }
            }
        }
    }
    Ok(())
}

async fn submit(
    shared: &Arc<Shared>,
    state: &mut Session,
    job_id: &str,
    nonce: &str,
) -> Result<Receipt, Reject> {
    let miner = state.miner.ok_or(Reject::NotLoggedIn)?;
    if !state.bucket.take() {
        return Err(Reject::RateLimited);
    }
    let nonce = protocol::parse_nonce(nonce).ok_or(Reject::Malformed)?;
    if !jobs::in_range(state.prefix, nonce) {
        return Err(Reject::OutOfRange);
    }
    let job = shared.jobs.read().map_err(|_| Reject::Busy)?.find(job_id)?;
    if !state.seen.insert(job_id, nonce) {
        return Err(Reject::Duplicate);
    }
    // Shares sent before a difficulty change arrive a little later; within
    // the grace period they are verified at, and credited with, the old one.
    let difficulty = match state.previous {
        Some((old, at)) if now_ms().saturating_sub(at) < DIFFICULTY_GRACE_MS => {
            old.min(state.difficulty.current())
        }
        _ => state.difficulty.current(),
    };
    let (reply, answer) = oneshot::channel();
    shared
        .verify_tx
        .try_send(VerifyTask {
            job,
            nonce,
            difficulty,
            miner,
            reply,
        })
        .map_err(|_| Reject::Busy)?;
    answer.await.map_err(|_| Reject::Busy)?
}

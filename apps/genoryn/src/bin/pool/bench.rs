//! Measured capacity, never assumed.
//!
//! - `verify`: genuine RandomX share verification throughput on this computer.
//! - `ledger`: durable work recording, PPLNS window and division, maturity
//!   crediting and payout planning for N miners, on SQLite or PostgreSQL.
//! - `connections`: a load generator that holds N real TCP sessions against
//!   a running gateway (logins and keepalives; optional real share
//!   submissions). It measures connection capacity of one gateway, not
//!   independent miners.
use super::*;
use genoryn_pool::{
    jobs,
    protocol::{self, NONCE_OFFSET},
};
use std::sync::atomic::{AtomicU64, Ordering};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

#[derive(ClapArgs)]
pub struct BenchArgs {
    #[command(subcommand)]
    kind: BenchKind,
}

#[derive(Subcommand)]
enum BenchKind {
    Verify {
        #[arg(long, default_value_t = 4)]
        threads: usize,
        #[arg(long, default_value_t = 20)]
        seconds: u64,
    },
    Ledger {
        #[command(flatten)]
        ledger: LedgerArgs,
        #[arg(long, default_value_t = 100_000)]
        miners: u64,
        /// Work rows per miner inside the window.
        #[arg(long, default_value_t = 2)]
        rows_per_miner: u64,
    },
    Connections {
        #[arg(long, default_value = "127.0.0.1:29790")]
        target: Vec<SocketAddr>,
        #[arg(long, default_value_t = 10_000)]
        connections: usize,
        #[arg(long, default_value_t = 60)]
        hold_seconds: u64,
        /// Login address (one miner, many sessions: capacity, not fairness).
        #[arg(long)]
        address: String,
        /// Local source addresses to spread connections over (loopback aliases).
        #[arg(long, value_delimiter = ',')]
        source: Vec<std::net::IpAddr>,
        /// Submit this many real shares per session during the hold.
        #[arg(long, default_value_t = 0)]
        shares_per_session: u32,
    },
}

pub async fn run(args: BenchArgs) -> Result<()> {
    let report = match args.kind {
        BenchKind::Verify { threads, seconds } => {
            tokio::task::spawn_blocking(move || verify(threads, seconds)).await??
        }
        BenchKind::Ledger {
            ledger,
            miners,
            rows_per_miner,
        } => {
            tokio::task::spawn_blocking(move || ledger_bench(ledger, miners, rows_per_miner))
                .await??
        }
        BenchKind::Connections {
            target,
            connections,
            hold_seconds,
            address,
            source,
            shares_per_session,
        } => {
            connections_bench(
                target,
                connections,
                hold_seconds,
                address,
                source,
                shares_per_session,
            )
            .await?
        }
    };
    println!("{}", serde_json::to_string_pretty(&report)?);
    Ok(())
}

fn machine() -> Value {
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "logical_cpus": std::thread::available_parallelism().map_or(1, |n| n.get()),
    })
}

/// Real RandomX light-mode verification of share-shaped inputs.
fn verify(threads: usize, seconds: u64) -> Result<Value> {
    let threads = threads.clamp(1, 256);
    let done = Arc::new(AtomicU64::new(0));
    let deadline = Instant::now() + Duration::from_secs(seconds.max(1));
    let started = Instant::now();
    let workers: Vec<_> = (0..threads)
        .map(|index| {
            let done = done.clone();
            std::thread::spawn(move || -> Result<()> {
                let seed = [7u8; 32];
                let mut header = vec![0x5a; 200];
                // Warm the VM (cache construction is not share verification).
                genoryn_core::pow::randomx(seed, &header)?;
                let mut nonce = (index as u64) << 40;
                while Instant::now() < deadline {
                    header[NONCE_OFFSET..NONCE_OFFSET + 8].copy_from_slice(&nonce.to_le_bytes());
                    std::hint::black_box(genoryn_core::pow::randomx(seed, &header)?);
                    done.fetch_add(1, Ordering::Relaxed);
                    nonce += 1;
                }
                Ok(())
            })
        })
        .collect();
    for worker in workers {
        worker
            .join()
            .map_err(|_| anyhow::anyhow!("worker panicked"))??;
    }
    let elapsed = started.elapsed().as_secs_f64();
    let total = done.load(Ordering::Relaxed);
    let per_second = total as f64 / elapsed;
    Ok(json!({
        "benchmark": "genuine RandomX light-mode share verification",
        "machine": machine(),
        "threads": threads,
        "seconds": elapsed,
        "verified_shares": total,
        "shares_per_second": per_second,
        "per_thread_per_second": per_second / threads as f64,
        "memory_note": "about 256 MiB of RandomX cache per verification thread",
        "miners_supported_at_15s_shares": (per_second * 15.0) as u64,
        "scope": "one machine's verification capacity; add verification threads or gateways to scale",
    }))
}

fn ledger_bench(args: LedgerArgs, miners: u64, rows_per_miner: u64) -> Result<Value> {
    let mut policy = args.policy()?;
    policy.payout_threshold = 1_000;
    let mut store = args.open()?;
    store.bind(&policy)?;
    let timing = |label: &str, started: Instant, extra: Value| json!({"step": label, "seconds": started.elapsed().as_secs_f64(), "detail": extra});
    let mut steps = Vec::new();
    let started = Instant::now();
    let mut ids = Vec::with_capacity(miners as usize);
    for chunk in (0..miners).collect::<Vec<_>>().chunks(4_096) {
        let addresses: Vec<String> = chunk.iter().map(|n| format!("bench-miner-{n}")).collect();
        let found = store.tx(|tx| {
            let mut found = Vec::with_capacity(addresses.len());
            for address in &addresses {
                tx.exec(
                    "INSERT INTO miners(address, created_at) VALUES ($1, 0) ON CONFLICT(address) DO NOTHING",
                    &[genoryn_pool::store::Val::Text(address.clone())],
                )?;
                let row = tx.rows(
                    "SELECT id FROM miners WHERE address = $1",
                    &[genoryn_pool::store::Val::Text(address.clone())],
                )?;
                found.push(match row.first().map(|r| r[0].clone()) {
                    Some(genoryn_pool::store::Val::Int(id)) => id,
                    _ => anyhow::bail!("miner id"),
                });
            }
            Ok(found)
        })?;
        ids.extend(found);
    }
    steps.push(timing(
        "register_miners",
        started,
        json!({"miners": miners}),
    ));
    let started = Instant::now();
    let mut rows = 0u64;
    let mut total_work = 0u64;
    for round in 0..rows_per_miner {
        for chunk in ids.chunks(4_096) {
            let batch: Vec<_> = chunk
                .iter()
                .map(|id| {
                    // Hashrates spread from 10 H/s to about 1 MH/s over 60 s.
                    let work = 600 + (*id as u64 % 100_000) * 600 + round;
                    (*id, work, 1u32)
                })
                .collect();
            total_work += batch.iter().map(|(_, w, _)| w).sum::<u64>();
            rows += batch.len() as u64;
            store.record_work("bench", &batch, 0)?;
        }
    }
    let record_seconds = started.elapsed().as_secs_f64();
    steps.push(json!({"step": "record_work_batched_4096", "seconds": record_seconds, "detail": {"rows": rows, "rows_per_second": rows as f64 / record_seconds}}));
    let last = store.last_work_seq()?;
    let started = Instant::now();
    let (window, window_work) = store.window(last, total_work)?;
    steps.push(timing(
        "pplns_window",
        started,
        json!({"miners_in_window": window.len(), "window_work": window_work}),
    ));
    let started = Instant::now();
    let allocation = pplns::allocate(4_825_437_595_129, policy.fee_bps, &window)?;
    steps.push(timing(
        "pplns_division",
        started,
        json!({"credits": allocation.credits.len(), "dust_fixed": allocation.dust.to_string()}),
    ));
    let block = store.record_block(
        1_000_000,
        &format!("bench-{}", unix_time()),
        4_825_437_595_129,
        total_work,
        last,
        "bench",
        0,
    )?;
    let started = Instant::now();
    store.allocate_block(block, &allocation, 0)?;
    steps.push(timing("store_immature_credits", started, json!({})));
    let started = Instant::now();
    store.mature_block(block, 0)?;
    steps.push(timing("mature_and_credit_balances", started, json!({})));
    let started = Instant::now();
    let batches = store.plan_payouts(&policy, u64::MAX, 1_000, 0)?;
    steps.push(timing("plan_1000_payout_batches", started, json!({"batches": batches.len(), "payees": batches.iter().map(|b| b.payouts.len()).sum::<usize>()})));
    let started = Instant::now();
    let audit = store.audit()?;
    steps.push(timing(
        "audit",
        started,
        json!({"balanced": audit.balanced}),
    ));
    Ok(json!({
        "benchmark": "pool ledger at scale",
        "driver": store.driver(),
        "machine": machine(),
        "miners": miners,
        "rows_per_miner": rows_per_miner,
        "steps": steps,
        "balanced": audit.balanced,
        "scope": "accounting path only (no network, no RandomX); simulated miners",
    }))
}

async fn connections_bench(
    targets: Vec<SocketAddr>,
    connections: usize,
    hold_seconds: u64,
    address: String,
    sources: Vec<std::net::IpAddr>,
    shares_per_session: u32,
) -> Result<Value> {
    ensure!(!targets.is_empty(), "at least one --target");
    let connected = Arc::new(AtomicU64::new(0));
    let logged_in = Arc::new(AtomicU64::new(0));
    let failed = Arc::new(AtomicU64::new(0));
    let shares_sent = Arc::new(AtomicU64::new(0));
    let shares_answered = Arc::new(AtomicU64::new(0));
    let latencies = Arc::new(Mutex::new(Vec::with_capacity(connections)));
    let started = Instant::now();
    let hold_until = tokio::time::Instant::now() + Duration::from_secs(hold_seconds);
    let mut tasks = Vec::with_capacity(connections);
    let gate = Arc::new(tokio::sync::Semaphore::new(512));
    for index in 0..connections {
        let target = targets[index % targets.len()];
        let source = (!sources.is_empty()).then(|| sources[index % sources.len()]);
        let (connected, logged_in, failed, latencies, gate, address) = (
            connected.clone(),
            logged_in.clone(),
            failed.clone(),
            latencies.clone(),
            gate.clone(),
            address.clone(),
        );
        let (shares_sent, shares_answered) = (shares_sent.clone(), shares_answered.clone());
        tasks.push(tokio::spawn(async move {
            let opened = Instant::now();
            let attempt = async {
                let permit = gate.acquire().await?;
                let socket = if target.is_ipv4() {
                    tokio::net::TcpSocket::new_v4()?
                } else {
                    tokio::net::TcpSocket::new_v6()?
                };
                if let Some(source) = source {
                    socket.bind(SocketAddr::new(source, 0))?;
                }
                let stream = socket.connect(target).await?;
                connected.fetch_add(1, Ordering::Relaxed);
                let (read, mut write) = stream.into_split();
                let mut lines = BufReader::new(read).lines();
                write
                    .write_all(format!("{}\n", json!({"id": 1, "method": "login", "params": {"address": address, "worker": format!("bench-{index}")}})).as_bytes())
                    .await?;
                let mut job: Option<protocol::Job> = None;
                let mut logged = false;
                while !logged || job.is_none() {
                    let line = tokio::time::timeout(Duration::from_secs(60), lines.next_line())
                        .await??
                        .context("closed")?;
                    let value: Value = serde_json::from_str(&line)?;
                    if value["id"] == 1 {
                        ensure!(value["error"].is_null(), "login refused: {}", value["error"]);
                        logged = true;
                    }
                    if value["method"] == "job" {
                        job = Some(serde_json::from_value(value["params"].clone())?);
                    }
                }
                drop(permit);
                logged_in.fetch_add(1, Ordering::Relaxed);
                latencies
                    .lock()
                    .unwrap_or_else(|e| e.into_inner())
                    .push(opened.elapsed().as_secs_f64() * 1000.0);
                let job = job.context("job")?;
                let start = protocol::parse_nonce(&job.nonce_start).context("nonce start")?;
                let mut next_id = 2u64;
                for share in 0..shares_per_session {
                    // A real nonce in this session's range; the gateway hashes it.
                    write
                        .write_all(format!("{}\n", json!({"id": next_id, "method": "submit", "params": {"job_id": job.job_id, "nonce": protocol::format_nonce(start + u64::from(share))}})).as_bytes())
                        .await?;
                    next_id += 1;
                    shares_sent.fetch_add(1, Ordering::Relaxed);
                    loop {
                        let line = tokio::time::timeout(Duration::from_secs(120), lines.next_line())
                            .await??
                            .context("closed")?;
                        if serde_json::from_str::<Value>(&line)?["id"].as_u64() == Some(next_id - 1) {
                            shares_answered.fetch_add(1, Ordering::Relaxed);
                            break;
                        }
                    }
                    tokio::time::sleep(Duration::from_millis(250)).await;
                }
                while tokio::time::Instant::now() < hold_until {
                    tokio::time::sleep(Duration::from_secs(20).min(hold_until - tokio::time::Instant::now())).await;
                    write.write_all(b"{\"id\":0,\"method\":\"keepalive\"}\n").await?;
                }
                Ok::<_, anyhow::Error>(())
            };
            if attempt.await.is_err() {
                failed.fetch_add(1, Ordering::Relaxed);
            }
        }));
    }
    let mut peak = 0;
    while tokio::time::Instant::now() < hold_until {
        tokio::time::sleep(Duration::from_secs(1)).await;
        peak = peak.max(
            logged_in.load(Ordering::Relaxed)
                - failed
                    .load(Ordering::Relaxed)
                    .min(logged_in.load(Ordering::Relaxed)),
        );
    }
    for task in tasks {
        let _ = task.await;
    }
    let mut latencies = latencies.lock().unwrap_or_else(|e| e.into_inner()).clone();
    latencies.sort_by(|a, b| a.total_cmp(b));
    let percentile = |p: f64| {
        latencies
            .get(((latencies.len() as f64 - 1.0) * p).round() as usize)
            .copied()
    };
    Ok(json!({
        "benchmark": "simulated miner connections against one gateway",
        "machine": machine(),
        "requested": connections,
        "connected": connected.load(Ordering::Relaxed),
        "logged_in_with_job": logged_in.load(Ordering::Relaxed),
        "failed": failed.load(Ordering::Relaxed),
        "peak_concurrent_sessions": peak,
        "login_ms_p50": percentile(0.5),
        "login_ms_p99": percentile(0.99),
        "shares_sent": shares_sent.load(Ordering::Relaxed),
        "shares_answered": shares_answered.load(Ordering::Relaxed),
        "seconds": started.elapsed().as_secs_f64(),
        "scope": "simulated sessions from one client machine, not independent miners; shares are real nonces hashed by the gateway",
    }))
}

#[allow(dead_code)]
fn _protocol_offset_matches(job: &jobs::Job) -> bool {
    job.header_with(1)[NONCE_OFFSET] == 1
}

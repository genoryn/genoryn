//! krypton-miner: mines Genoryn Krypton on the CPU or on GPUs (CUDA / OpenCL).
//!
//! The miner never needs a wallet or keys: it asks a node for work paying to your
//! address, searches nonces against the share target, and submits every share.
//! Rewards are proportional to the shares you find (see docs/spec/rewards.md), so
//! the miner must stay online to deliver them. Every GPU solution is re-verified on
//! the CPU before submission.

use anyhow::{Context, Result, bail};
use clap::{Parser, ValueEnum};
use krypton_pow::cpu::{self, Job};
use krypton_pow::{Cache, Dataset, PowParams};
use serde_json::{Value, json};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

#[derive(Copy, Clone, Debug, PartialEq, Eq, ValueEnum)]
enum Backend {
    /// Use every NVIDIA GPU (CUDA), else OpenCL GPUs, else the CPU
    Auto,
    Cpu,
    Cuda,
    Opencl,
}

#[derive(Parser, Debug)]
#[command(
    name = "krypton-miner",
    version,
    about = "Genoryn Krypton miner (CPU or GPU)"
)]
struct Args {
    /// Node API URL
    #[arg(long, default_value = "http://127.0.0.1:30334", env = "KRYPTON_NODE")]
    node: String,
    /// Payout address: the full gnr1... address, or its short form gnrs1... (resolved
    /// through the node and checked against its hash here)
    #[arg(long, env = "KRYPTON_MINER_ADDRESS")]
    address: Option<String>,
    /// Read the payout address from a file
    #[arg(long)]
    address_file: Option<std::path::PathBuf>,
    #[arg(long, value_enum, default_value_t = Backend::Auto)]
    backend: Backend,
    /// CPU threads (default: from --max-cpu)
    #[arg(long)]
    threads: Option<usize>,
    /// Upper bound on CPU usage in percent when mining on the CPU
    #[arg(long, default_value_t = 100)]
    max_cpu: u32,
    /// GPU device indices (default: all)
    #[arg(long, value_delimiter = ',')]
    devices: Vec<usize>,
    /// Nonces per GPU launch (auto-tuned when omitted)
    #[arg(long)]
    batch: Option<u32>,
    /// Print machine-readable JSON status lines
    #[arg(long)]
    json: bool,
    /// List devices and exit
    #[arg(long)]
    list_devices: bool,
}

struct Shared {
    hashes: AtomicU64,
    accepted: AtomicU64,
    shares: AtomicU64,
    rejected: AtomicU64,
    height: AtomicU64,
    /// When the node last accepted a share or block (unix seconds; 0: not yet).
    last_share: AtomicU64,
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// The full address behind the short address `short`, as a node's directory returned it
/// (`full`). The short form is a hash of the full address, so nothing a node returns can
/// stand in for another address.
fn checked_full(short: &str, full: &str) -> Result<String> {
    use krypton_core::address::Address;
    let (network, id) = Address::parse_short(short).map_err(|e| anyhow::anyhow!("{e}"))?;
    let a = Address::decode(full).map_err(|e| anyhow::anyhow!("payout address: {e}"))?;
    if a.short_id() != id || a.network != network {
        bail!("the node returned another address for {short}: refusing to mine");
    }
    Ok(a.encode())
}

/// The payout address to mine to: a full address as given, or the full address of a short
/// one, from the node's directory (waiting for the node if it cannot be reached yet).
fn payout_address(input: &str, base: &str, http: &reqwest::blocking::Client) -> Result<String> {
    let s = input.trim();
    if krypton_core::address::Address::parse_short(s).is_err() {
        krypton_core::address::Address::decode(s)
            .map_err(|e| anyhow::anyhow!("payout address: {e}"))?;
        return Ok(s.to_string());
    }
    loop {
        let r = http.get(format!("{base}/v1/address/{s}")).send();
        match r {
            Ok(r) if r.status().as_u16() == 404 => bail!(
                "the node does not know the short address {s}: open the wallet once while it is online (it publishes the address), or pass the full address"
            ),
            Ok(r) if r.status().is_success() => {
                let v: Value = r.json()?;
                return checked_full(s, v["address"].as_str().context("node answer")?);
            }
            Ok(r) => eprintln!("waiting for node: {}", r.status()),
            Err(e) => eprintln!("waiting for node: {e}"),
        }
        std::thread::sleep(Duration::from_secs(5));
    }
}

#[derive(Clone)]
struct Work {
    job_id: u64,
    height: u64,
    pow_hash: [u8; 32],
    share_difficulty: u128,
    epoch: u64,
    chain_seed: [u8; 32],
    params: PowParams,
}

/// Network figures for the share-of-network estimate.
#[derive(Default, Clone, Copy)]
struct NetView {
    hashrate: f64,
    subsidy: u64,
    unpaid: u64,
}

/// Turns a node's template into work, trusting nothing but the job number: the header
/// must pay `address`, and the hash to grind, the share target, the epoch and the chain
/// are all derived from that header and from the address's own network. A node can
/// therefore not make this miner work for someone else, on another chain, or on a
/// made-up epoch.
fn verify_work(v: &Value, address: &str) -> Result<Work> {
    use krypton_core::address::Address;
    use krypton_core::types::BlockHeader;
    let addr = Address::decode(address).map_err(|e| anyhow::anyhow!("payout address: {e}"))?;
    let chain = addr.network.params();
    let owner = addr.owner_bytes();
    let raw = hex::decode(v["header"].as_str().context(
        "the node does not show whom its work pays: update the node to 1.0.38 or later",
    )?)?;
    let header = BlockHeader::from_bytes(&raw).map_err(|e| anyhow::anyhow!("header: {e}"))?;
    if header.miner != owner {
        bail!("the node's work pays another address: refusing to mine it");
    }
    if header.height == 0 || header.nonce != 0 {
        bail!("the node sent a malformed header");
    }
    if let Some(n) = v["network"].as_str()
        && n != addr.network.as_str()
    {
        bail!(
            "the node is on {n}, but your address is for {}",
            addr.network.as_str()
        );
    }
    Ok(Work {
        job_id: v["job_id"].as_u64().context("job_id")?,
        height: header.height,
        pow_hash: header.pow_hash(),
        share_difficulty: krypton_core::reward::share_difficulty(header.difficulty),
        epoch: krypton_pow::epoch_of(header.height),
        chain_seed: krypton_core::consensus::genesis_block(&chain).hash(),
        params: chain.pow,
    })
}

struct Client {
    base: String,
    http: reqwest::blocking::Client,
    address: String,
}

impl Client {
    fn template(&self) -> Result<Work> {
        let r = self
            .http
            .post(format!("{}/v1/mining/template", self.base))
            .json(&json!({"address": self.address}))
            .send()?;
        let status = r.status();
        let v: Value = r.json()?;
        if !status.is_success() {
            bail!("node: {}", v["error"].as_str().unwrap_or("error"));
        }
        verify_work(&v, &self.address)
    }
    fn submit(&self, job_id: u64, nonce: u64) -> Result<Value> {
        let r = self
            .http
            .post(format!("{}/v1/mining/submit", self.base))
            .json(&json!({"job_id": job_id, "nonce": nonce}))
            .send()?;
        Ok(r.json()?)
    }
    fn net_view(&self) -> Result<NetView> {
        let i: Value = self
            .http
            .get(format!("{}/v1/info", self.base))
            .send()?
            .json()?;
        let m: Value = self
            .http
            .get(format!("{}/v1/miner/{}", self.base, self.address))
            .send()?
            .json()?;
        Ok(NetView {
            hashrate: i["hashrate"].as_f64().unwrap_or(0.0),
            subsidy: i["next_subsidy"].as_u64().unwrap_or(0),
            unpaid: m["unpaid"].as_u64().unwrap_or(0),
        })
    }
}

/// (fraction of network hashrate, expected GNR per day) for a miner at `rate` H/s.
fn expected(rate: f64, net: &NetView) -> (f64, f64) {
    let total = net.hashrate.max(rate);
    if total <= 0.0 {
        return (0.0, 0.0);
    }
    let frac = (rate / total).clamp(0.0, 1.0);
    (frac, frac * 1440.0 * net.subsidy as f64 / 1e8)
}

enum Engine {
    Cpu {
        threads: usize,
        dataset: Option<Arc<Dataset>>,
    },
    #[cfg(feature = "cuda")]
    Cuda(Vec<krypton_pow::cuda::CudaMiner>),
    #[cfg(feature = "opencl")]
    Opencl(Vec<krypton_pow::opencl::ClMiner>),
}

/// One GPU as the miner sees it, and whether it can mine (with the reason if not).
struct Probe {
    backend: &'static str,
    index: usize,
    name: String,
    memory_mib: u64,
    error: Option<String>,
}

/// Lists every device and tries to set each one up for mining (compiling the kernel),
/// so a device that cannot mine says why.
fn probe_devices() -> (Vec<Probe>, Vec<String>) {
    let mut v = Vec::new();
    let mut notes = Vec::new();
    #[cfg(feature = "cuda")]
    match krypton_pow::cuda::devices() {
        Ok(d) => {
            for x in d {
                let error = krypton_pow::cuda::CudaMiner::new(x.ordinal)
                    .err()
                    .map(|e| e.to_string());
                v.push(Probe {
                    backend: "cuda",
                    index: x.ordinal,
                    name: x.name,
                    memory_mib: (x.memory_bytes >> 20) as u64,
                    error,
                });
            }
        }
        Err(e) => notes.push(format!("CUDA: unavailable ({e})")),
    }
    #[cfg(feature = "opencl")]
    match krypton_pow::opencl::devices() {
        Ok(d) => {
            for (i, n, m) in d {
                let error = krypton_pow::opencl::ClMiner::new(i)
                    .err()
                    .map(|e| e.to_string());
                v.push(Probe {
                    backend: "opencl",
                    index: i,
                    name: n,
                    memory_mib: m >> 20,
                    error,
                });
            }
        }
        Err(e) => notes.push(format!("OpenCL: unavailable ({e})")),
    }
    (v, notes)
}

fn list_devices(json: bool) {
    let (devs, notes) = probe_devices();
    if json {
        let list: Vec<Value> = devs
            .iter()
            .map(|d| {
                json!({"backend": d.backend, "index": d.index, "name": d.name,
                       "memory_mib": d.memory_mib, "usable": d.error.is_none(), "error": d.error})
            })
            .collect();
        println!(
            "{}",
            json!({"cpu_threads": num_cpus::get(), "gpus": list, "notes": notes})
        );
        return;
    }
    println!("CPU: {} logical cores", num_cpus::get());
    for d in &devs {
        let tag = if d.backend == "cuda" {
            "CUDA"
        } else {
            "OpenCL"
        };
        match &d.error {
            None => println!(
                "{tag} {}: {} ({} MiB) - usable",
                d.index, d.name, d.memory_mib
            ),
            Some(e) => println!(
                "{tag} {}: {} ({} MiB) - NOT usable: {e}",
                d.index, d.name, d.memory_mib
            ),
        }
    }
    for n in notes {
        println!("{n}");
    }
}

fn pick_engine(a: &Args) -> Result<Engine> {
    let threads = a
        .threads
        .unwrap_or_else(|| {
            ((num_cpus::get() as u32 * a.max_cpu.clamp(1, 100)).div_ceil(100)) as usize
        })
        .max(1);
    // Why each GPU that was tried could not be used (shown if none works).
    let mut why: Vec<String> = Vec::new();
    #[cfg(feature = "cuda")]
    if matches!(a.backend, Backend::Auto | Backend::Cuda) {
        match krypton_pow::cuda::devices() {
            Ok(devs) => {
                let mut miners = Vec::new();
                for d in devs
                    .iter()
                    .filter(|d| a.devices.is_empty() || a.devices.contains(&d.ordinal))
                {
                    match krypton_pow::cuda::CudaMiner::new(d.ordinal) {
                        Ok(m) => miners.push(m),
                        Err(e) => why.push(format!("CUDA {} ({}): {e}", d.ordinal, d.name)),
                    }
                }
                if !miners.is_empty() {
                    return Ok(Engine::Cuda(miners));
                }
            }
            Err(e) => why.push(format!("CUDA: {e}")),
        }
        if a.backend == Backend::Cuda {
            bail!("no usable NVIDIA GPU found: {}", reasons(&why));
        }
    }
    #[cfg(feature = "opencl")]
    if matches!(a.backend, Backend::Auto | Backend::Opencl) {
        match krypton_pow::opencl::devices() {
            Ok(devs) => {
                let mut miners = Vec::new();
                for (i, n, _) in devs
                    .iter()
                    .filter(|(i, _, _)| a.devices.is_empty() || a.devices.contains(i))
                {
                    match krypton_pow::opencl::ClMiner::new(*i) {
                        Ok(m) => miners.push(m),
                        Err(e) => why.push(format!("OpenCL {i} ({n}): {e}")),
                    }
                }
                if !miners.is_empty() {
                    if !why.is_empty() {
                        eprintln!("skipped: {}", reasons(&why));
                    }
                    return Ok(Engine::Opencl(miners));
                }
            }
            Err(e) => why.push(format!("OpenCL: {e}")),
        }
        if a.backend == Backend::Opencl {
            bail!("no usable OpenCL GPU found: {}", reasons(&why));
        }
    }
    if !why.is_empty() {
        eprintln!("no GPU could mine, using the CPU: {}", reasons(&why));
    }
    Ok(Engine::Cpu {
        threads,
        dataset: None,
    })
}

/// How many devices the engine mines on (CPU: 1).
fn engine_devices(e: &Engine) -> usize {
    match e {
        Engine::Cpu { .. } => 1,
        #[cfg(feature = "cuda")]
        Engine::Cuda(m) => m.len(),
        #[cfg(feature = "opencl")]
        Engine::Opencl(m) => m.len(),
    }
}

fn reasons(why: &[String]) -> String {
    if why.is_empty() {
        "no GPU of this kind was found (see --list-devices)".into()
    } else {
        why.join("; ")
    }
}

fn main() -> Result<()> {
    let a = Args::parse();
    if a.list_devices {
        list_devices(a.json);
        return Ok(());
    }
    let input = match (&a.address, &a.address_file) {
        (Some(x), _) => x.trim().to_string(),
        (None, Some(f)) => std::fs::read_to_string(f)?.trim().to_string(),
        _ => bail!("pass --address <your address> (create one with `krypton-wallet-cli new`)"),
    };
    let base = a.node.trim_end_matches('/').to_string();
    let http = reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(60))
        .build()?;
    let address = payout_address(&input, &base, &http)?;
    let client = Client {
        base,
        http,
        address,
    };
    let started = Instant::now();
    let mut engine = pick_engine(&a)?;
    let shared = Arc::new(Shared {
        hashes: AtomicU64::new(0),
        accepted: AtomicU64::new(0),
        shares: AtomicU64::new(0),
        rejected: AtomicU64::new(0),
        height: AtomicU64::new(0),
        last_share: AtomicU64::new(0),
    });
    let net = Arc::new(std::sync::Mutex::new(NetView::default()));
    {
        let net = net.clone();
        let c = Client {
            base: client.base.clone(),
            http: client.http.clone(),
            address: client.address.clone(),
        };
        std::thread::spawn(move || {
            loop {
                if let Ok(v) = c.net_view() {
                    *net.lock().unwrap() = v;
                }
                std::thread::sleep(Duration::from_secs(30));
            }
        });
    }
    let desc = match &engine {
        Engine::Cpu { threads, .. } => format!("CPU x{threads} threads"),
        #[cfg(feature = "cuda")]
        Engine::Cuda(m) => m
            .iter()
            .map(|x| format!("CUDA {} {}", x.info.ordinal, x.info.name))
            .collect::<Vec<_>>()
            .join(", "),
        #[cfg(feature = "opencl")]
        Engine::Opencl(m) => m
            .iter()
            .map(|x| format!("OpenCL {} {}", x.index, x.name))
            .collect::<Vec<_>>()
            .join(", "),
    };
    eprintln!(
        "krypton-miner {} | {} | node {}",
        env!("CARGO_PKG_VERSION"),
        desc,
        client.base
    );
    if a.json {
        let short = krypton_core::address::Address::decode(&client.address)
            .map(|x| x.short())
            .unwrap_or_default();
        println!(
            "{}",
            json!({"event": "start", "version": env!("CARGO_PKG_VERSION"), "device": desc, "devices": engine_devices(&engine), "node": client.base, "address": short})
        );
    }

    {
        let s = shared.clone();
        let json_out = a.json;
        let desc = desc.clone();
        let net = net.clone();
        std::thread::spawn(move || {
            let mut last = 0u64;
            let mut t = Instant::now();
            loop {
                std::thread::sleep(Duration::from_secs(10));
                let h = s.hashes.load(Ordering::Relaxed);
                let rate = (h - last) as f64 / t.elapsed().as_secs_f64();
                last = h;
                t = Instant::now();
                let (acc, shares, rej, height) = (
                    s.accepted.load(Ordering::Relaxed),
                    s.shares.load(Ordering::Relaxed),
                    s.rejected.load(Ordering::Relaxed),
                    s.height.load(Ordering::Relaxed),
                );
                let nv = *net.lock().unwrap();
                let (frac, per_day) = expected(rate, &nv);
                if json_out {
                    println!(
                        "{}",
                        json!({"event": "status", "version": env!("CARGO_PKG_VERSION"), "uptime_secs": started.elapsed().as_secs(), "last_share_at": s.last_share.load(Ordering::Relaxed), "hashrate_hs": rate, "hashes": h, "accepted_blocks": acc, "accepted_shares": shares, "rejected": rej, "height": height, "device": desc, "network_hashrate_hs": nv.hashrate, "network_fraction": frac, "expected_gnr_per_day": per_day, "unpaid": nv.unpaid})
                    );
                } else {
                    eprintln!(
                        "[{}] {:.2} {} | {:.2}% of network ≈ {:.2} GNR/day | height {} | shares {} | blocks {} | unpaid {} GNR | rejected {}",
                        desc,
                        fmt_rate(rate).0,
                        fmt_rate(rate).1,
                        frac * 100.0,
                        per_day,
                        height,
                        shares,
                        acc,
                        fmt_gnr(nv.unpaid),
                        rej
                    );
                }
            }
        });
    }

    let mut cache: Option<Arc<Cache>> = None;
    loop {
        let work = match client.template() {
            Ok(w) => w,
            Err(e) => {
                eprintln!("waiting for node: {e}");
                std::thread::sleep(Duration::from_secs(5));
                continue;
            }
        };
        // Epochs only move forward, one at a time: anything else is not the chain we mine.
        if let Some(c) = &cache
            && (work.epoch < c.epoch || work.epoch > c.epoch + 1)
        {
            eprintln!(
                "the node's work is for epoch {} while mining epoch {}: ignoring it",
                work.epoch, c.epoch
            );
            std::thread::sleep(Duration::from_secs(5));
            continue;
        }
        shared.height.store(work.height, Ordering::Relaxed);
        if cache.as_ref().map(|c| c.epoch) != Some(work.epoch) {
            eprintln!("preparing epoch {} ...", work.epoch);
            let t = Instant::now();
            let c = Arc::new(Cache::new(&work.params, &work.chain_seed, work.epoch));
            match &mut engine {
                Engine::Cpu { threads, dataset } => {
                    *dataset = Some(Arc::new(Dataset::generate(&work.params, &c, *threads)));
                }
                #[cfg(feature = "cuda")]
                Engine::Cuda(ms) => {
                    for m in ms.iter_mut() {
                        m.prepare(&work.params, &c)?;
                    }
                }
                #[cfg(feature = "opencl")]
                Engine::Opencl(ms) => {
                    for m in ms.iter_mut() {
                        m.prepare(&work.params, &c)?;
                    }
                }
            }
            eprintln!(
                "epoch {} ready in {:.1}s",
                work.epoch,
                t.elapsed().as_secs_f64()
            );
            cache = Some(c);
        }
        let c = cache.clone().expect("cache");
        let job = Job {
            header: krypton_pow::header_words(&work.pow_hash),
            target: krypton_pow::target_from_difficulty(work.share_difficulty),
            start_nonce: rand::random::<u64>() & !0xffff_ffff,
        };
        let found = mine_round(&mut engine, &job, &c, &shared, &client, &work, a.batch)?;
        if let Some(nonce) = found {
            match client.submit(work.job_id, nonce) {
                Ok(v) if v["accepted"].as_bool() == Some(true) => {
                    let block = v["kind"].as_str() == Some("block");
                    if block {
                        shared.accepted.fetch_add(1, Ordering::Relaxed);
                    }
                    shared.shares.fetch_add(1, Ordering::Relaxed);
                    shared.last_share.store(unix_now(), Ordering::Relaxed);
                    if a.json {
                        println!(
                            "{}",
                            json!({"event": if block { "block" } else { "share" }, "height": v["height"], "hash": v["hash"]})
                        );
                    } else if block {
                        eprintln!(
                            "BLOCK FOUND at height {} ({})",
                            v["height"],
                            v["hash"].as_str().unwrap_or("")
                        );
                    }
                }
                Ok(v) => {
                    shared.rejected.fetch_add(1, Ordering::Relaxed);
                    eprintln!("solution not accepted: {v}");
                }
                Err(e) => eprintln!("submit failed: {e}"),
            }
        }
    }
}

fn fmt_gnr(krypton: u64) -> String {
    format!("{}.{:08}", krypton / 100_000_000, krypton % 100_000_000)
}

fn fmt_rate(r: f64) -> (f64, &'static str) {
    if r >= 1e9 {
        (r / 1e9, "GH/s")
    } else if r >= 1e6 {
        (r / 1e6, "MH/s")
    } else if r >= 1e3 {
        (r / 1e3, "kH/s")
    } else {
        (r, "H/s")
    }
}

/// Mines until a solution is found or the node's template changes (checked every 2 s).
fn mine_round(
    engine: &mut Engine,
    job: &Job,
    cache: &Cache,
    shared: &Arc<Shared>,
    client: &Client,
    work: &Work,
    batch: Option<u32>,
) -> Result<Option<u64>> {
    let stale = |client: &Client| -> bool {
        client
            .template()
            .map(|w| w.height != work.height || w.pow_hash != work.pow_hash)
            .unwrap_or(false)
    };
    let _ = cache;
    match engine {
        Engine::Cpu { threads, dataset } => {
            let ds = dataset.clone().expect("dataset");
            let stop = Arc::new(AtomicBool::new(false));
            let hashes = Arc::new(AtomicU64::new(0));
            let (st, hs, j, th) = (stop.clone(), hashes.clone(), *job, *threads);
            let worker = std::thread::spawn(move || cpu::mine(ds, j, th, st, hs));
            let mut seen = 0;
            let mut last_check = Instant::now();
            loop {
                std::thread::sleep(Duration::from_millis(500));
                let h = hashes.load(Ordering::Relaxed);
                shared.hashes.fetch_add(h - seen, Ordering::Relaxed);
                seen = h;
                if worker.is_finished() || stop.load(Ordering::Relaxed) {
                    break;
                }
                if last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    if stale(client) {
                        stop.store(true, Ordering::Relaxed);
                    }
                }
            }
            let sol = worker.join().expect("miner thread");
            let h = hashes.load(Ordering::Relaxed);
            shared
                .hashes
                .fetch_add(h.saturating_sub(seen), Ordering::Relaxed);
            Ok(sol.map(|s| s.nonce))
        }
        #[cfg(feature = "cuda")]
        Engine::Cuda(ms) => {
            let mut nonce = job.start_nonce;
            let mut b = batch.unwrap_or(1 << 22);
            let mut last_check = Instant::now();
            loop {
                for (i, m) in ms.iter_mut().enumerate() {
                    let j = Job {
                        start_nonce: nonce.wrapping_add((i as u64) << 40),
                        ..*job
                    };
                    let t = Instant::now();
                    if let Some(s) = m
                        .search_verified(&j, b, cache)
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    {
                        return Ok(Some(s.nonce));
                    }
                    shared.hashes.fetch_add(b as u64, Ordering::Relaxed);
                    // Keep each launch near 100 ms for responsiveness.
                    if batch.is_none() {
                        let ms_ = t.elapsed().as_millis();
                        if ms_ < 60 && b < 1 << 28 {
                            b *= 2;
                        } else if ms_ > 200 && b > 1 << 16 {
                            b /= 2;
                        }
                    }
                }
                nonce = nonce.wrapping_add(b as u64);
                if last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    if stale(client) {
                        return Ok(None);
                    }
                }
            }
        }
        #[cfg(feature = "opencl")]
        Engine::Opencl(ms) => {
            let mut nonce = job.start_nonce;
            let b = batch.unwrap_or(1 << 21);
            let mut last_check = Instant::now();
            loop {
                for (i, m) in ms.iter_mut().enumerate() {
                    let j = Job {
                        start_nonce: nonce.wrapping_add((i as u64) << 40),
                        ..*job
                    };
                    if let Some(s) = m
                        .search_verified(&j, b, cache)
                        .map_err(|e| anyhow::anyhow!("{e}"))?
                    {
                        return Ok(Some(s.nonce));
                    }
                    shared.hashes.fetch_add(b as u64, Ordering::Relaxed);
                }
                nonce = nonce.wrapping_add(b as u64);
                if last_check.elapsed() > Duration::from_secs(2) {
                    last_check = Instant::now();
                    if stale(client) {
                        return Ok(None);
                    }
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krypton_core::Network;
    use krypton_core::address::Address;
    use krypton_core::consensus::genesis_block;

    fn address(seed: u8) -> Address {
        let sk = krypton_crypto::kem::SecretKey::from_seed(&[seed; 96]);
        Address {
            network: Network::Devnet,
            owner: krypton_crypto::rescue::SpendKeys::from_sk(krypton_crypto::rescue::scalar(
                seed as u64,
            ))
            .owner,
            kem: sk.public().clone(),
        }
    }

    /// A node's template is accepted only if its header pays this miner, and everything
    /// mined is derived from that header, not from what the node says next to it.
    #[test]
    fn work_must_pay_the_miner() {
        let me = address(1);
        let thief = address(2);
        let p = Network::Devnet.params();
        let mut h = genesis_block(&p).header;
        h.height = 40_000;
        h.miner = me.owner_bytes();
        let job = |h: &krypton_core::types::BlockHeader| {
            json!({"job_id": 7, "header": hex::encode(h.to_bytes()), "network": "devnet",
            "pow_hash": "00".repeat(32), "epoch": 999, "chain_seed": "11".repeat(32), "height": 1, "share_difficulty": "1"})
        };
        let w = verify_work(&job(&h), &me.encode()).expect("pays us");
        assert_eq!((w.job_id, w.height), (7, 40_000));
        assert_eq!(w.pow_hash, h.pow_hash(), "the hash comes from the header");
        assert_eq!(
            w.epoch,
            krypton_pow::epoch_of(40_000),
            "the epoch comes from the height"
        );
        assert_eq!(
            w.chain_seed,
            genesis_block(&p).hash(),
            "the chain comes from the address"
        );
        assert_eq!(
            w.share_difficulty,
            krypton_core::reward::share_difficulty(h.difficulty)
        );
        // Work that pays someone else, or no proof of whom it pays, is refused.
        let mut stolen = h.clone();
        stolen.miner = thief.owner_bytes();
        assert!(verify_work(&job(&stolen), &me.encode()).is_err());
        let mut old = job(&h);
        old.as_object_mut().unwrap().remove("header");
        assert!(verify_work(&old, &me.encode()).is_err());
        let mut other = job(&h);
        other["network"] = json!("mainnet");
        assert!(verify_work(&other, &me.encode()).is_err());
    }

    /// A short address is resolved by a node, but only the full address it is the hash of
    /// is accepted: a node cannot make the miner pay anyone else.
    #[test]
    fn a_short_address_resolves_only_to_its_own_full_address() {
        let me = address(1);
        let thief = address(2);
        assert_eq!(
            checked_full(&me.short(), &me.encode()).unwrap(),
            me.encode()
        );
        assert!(checked_full(&me.short(), &thief.encode()).is_err());
        assert!(checked_full(&me.short(), "gnr1nonsense").is_err());
        assert!(checked_full("not-an-address", &me.encode()).is_err());
        // The short form is short enough for any mining dashboard's wallet field.
        assert!(me.short().len() < 80, "{}", me.short().len());
        assert!(me.encode().len() > 1000);
    }
}

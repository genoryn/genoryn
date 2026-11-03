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
    /// Payout address (full gnr1/tgnr1... address)
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

fn h32(v: &Value) -> Result<[u8; 32]> {
    hex::decode(v.as_str().context("hex")?)?
        .try_into()
        .map_err(|_| anyhow::anyhow!("length"))
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
        let network = v["network"].as_str().unwrap_or("mainnet");
        let params = match network {
            "devnet" => PowParams::DEVNET,
            _ => PowParams::STANDARD,
        };
        Ok(Work {
            job_id: v["job_id"].as_u64().context("job_id")?,
            height: v["height"].as_u64().context("height")?,
            pow_hash: h32(&v["pow_hash"])?,
            share_difficulty: v["share_difficulty"]
                .as_str()
                .context("share_difficulty (node too old?)")?
                .parse()?,
            epoch: v["epoch"].as_u64().context("epoch")?,
            chain_seed: h32(&v["chain_seed"])?,
            params,
        })
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

fn list_devices() {
    println!("CPU: {} logical cores", num_cpus::get());
    #[cfg(feature = "cuda")]
    match krypton_pow::cuda::devices() {
        Ok(d) => d.iter().for_each(|x| {
            println!(
                "CUDA {}: {} ({} MiB)",
                x.ordinal,
                x.name,
                x.memory_bytes >> 20
            )
        }),
        Err(e) => println!("CUDA: unavailable ({e})"),
    }
    #[cfg(feature = "opencl")]
    match krypton_pow::opencl::devices() {
        Ok(d) => d
            .iter()
            .for_each(|(i, n, m)| println!("OpenCL {i}: {n} ({} MiB)", m >> 20)),
        Err(e) => println!("OpenCL: unavailable ({e})"),
    }
}

fn pick_engine(a: &Args) -> Result<Engine> {
    let threads = a
        .threads
        .unwrap_or_else(|| {
            ((num_cpus::get() as u32 * a.max_cpu.clamp(1, 100)).div_ceil(100)) as usize
        })
        .max(1);
    let want_gpu = matches!(a.backend, Backend::Auto | Backend::Cuda);
    #[cfg(feature = "cuda")]
    if want_gpu {
        if let Ok(devs) = krypton_pow::cuda::devices() {
            let ids: Vec<usize> = devs
                .iter()
                .map(|d| d.ordinal)
                .filter(|o| a.devices.is_empty() || a.devices.contains(o))
                .collect();
            let miners: Vec<_> = ids
                .iter()
                .filter_map(|i| krypton_pow::cuda::CudaMiner::new(*i).ok())
                .collect();
            if !miners.is_empty() {
                return Ok(Engine::Cuda(miners));
            }
        }
        if a.backend == Backend::Cuda {
            bail!("no usable NVIDIA GPU found");
        }
    }
    #[cfg(feature = "opencl")]
    if matches!(a.backend, Backend::Auto | Backend::Opencl) {
        if let Ok(devs) = krypton_pow::opencl::devices() {
            let miners: Vec<_> = devs
                .iter()
                .filter(|(i, _, _)| a.devices.is_empty() || a.devices.contains(i))
                .filter_map(|(i, _, _)| krypton_pow::opencl::ClMiner::new(*i).ok())
                .collect();
            if !miners.is_empty() {
                return Ok(Engine::Opencl(miners));
            }
        }
        if a.backend == Backend::Opencl {
            bail!("no usable OpenCL GPU found");
        }
    }
    let _ = want_gpu;
    Ok(Engine::Cpu {
        threads,
        dataset: None,
    })
}

fn main() -> Result<()> {
    let a = Args::parse();
    if a.list_devices {
        list_devices();
        return Ok(());
    }
    let address = match (&a.address, &a.address_file) {
        (Some(x), _) => x.trim().to_string(),
        (None, Some(f)) => std::fs::read_to_string(f)?.trim().to_string(),
        _ => bail!("pass --address <your address> (create one with `krypton-wallet-cli new`)"),
    };
    let client = Client {
        base: a.node.trim_end_matches('/').to_string(),
        http: reqwest::blocking::Client::builder()
            .timeout(Duration::from_secs(60))
            .build()?,
        address,
    };
    let mut engine = pick_engine(&a)?;
    let shared = Arc::new(Shared {
        hashes: AtomicU64::new(0),
        accepted: AtomicU64::new(0),
        shares: AtomicU64::new(0),
        rejected: AtomicU64::new(0),
        height: AtomicU64::new(0),
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
            .map(|x| x.info.name.clone())
            .collect::<Vec<_>>()
            .join(", "),
        #[cfg(feature = "opencl")]
        Engine::Opencl(m) => m
            .iter()
            .map(|x| x.name.clone())
            .collect::<Vec<_>>()
            .join(", "),
    };
    eprintln!(
        "krypton-miner {} | {} | node {}",
        env!("CARGO_PKG_VERSION"),
        desc,
        client.base
    );

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
                        json!({"event": "status", "hashrate_hs": rate, "hashes": h, "accepted_blocks": acc, "accepted_shares": shares, "rejected": rej, "height": height, "device": desc, "network_hashrate_hs": nv.hashrate, "network_fraction": frac, "expected_gnr_per_day": per_day, "unpaid": nv.unpaid})
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

fn fmt_gnr(gori: u64) -> String {
    format!("{}.{:08}", gori / 100_000_000, gori % 100_000_000)
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

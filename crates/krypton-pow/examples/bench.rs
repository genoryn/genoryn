//! KryptoHash benchmark with production parameters (2 GiB dataset).
//! `cargo run --release -p krypton-pow --example bench --features cuda -- [cpu THREADS] [gpu]`

use krypton_pow::cpu::{self, Job};
use krypton_pow::{Cache, Dataset, PowParams, header_words, target_from_difficulty};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, Instant};

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let p = PowParams::STANDARD;
    let t = Instant::now();
    let cache = Cache::new(&p, &[7; 32], 0);
    println!("cache: {:?}", t.elapsed());
    let hdr = header_words(&[1; 32]);
    let t = Instant::now();
    let mut n = 0;
    for nonce in 0..200 {
        n ^= krypton_pow::hash_light(&cache, p.dataset_pairs(0), &hdr, nonce)[0];
    }
    println!("light verify: {:?} per hash ({n})", t.elapsed() / 200);

    if let Some(i) = args.iter().position(|a| a == "cpu") {
        let threads: usize = args
            .get(i + 1)
            .and_then(|s| s.parse().ok())
            .unwrap_or_else(num_cpus::get);
        let t = Instant::now();
        let ds = Arc::new(Dataset::generate(&p, &cache, threads));
        println!(
            "cpu dataset ({} MiB): {:?}",
            (ds.items.len() * 64) >> 20,
            t.elapsed()
        );
        let stop = Arc::new(AtomicBool::new(false));
        let hashes = Arc::new(AtomicU64::new(0));
        let job = Job {
            header: hdr,
            target: [0; 4],
            start_nonce: 0,
        };
        let (s2, h2) = (stop.clone(), hashes.clone());
        let handle = std::thread::spawn(move || cpu::mine(ds, job, threads, s2, h2));
        std::thread::sleep(Duration::from_secs(10));
        stop.store(true, Ordering::Relaxed);
        handle.join().unwrap();
        let hs = hashes.load(Ordering::Relaxed) as f64 / 10.0;
        println!("cpu {threads} threads: {:.3} MH/s", hs / 1e6);
    }

    #[cfg(feature = "cuda")]
    if args.iter().any(|a| a == "gpu") {
        let mut m = krypton_pow::cuda::CudaMiner::new(0).expect("cuda");
        let t = Instant::now();
        m.prepare(&p, &cache).unwrap();
        println!("gpu {} dataset: {:?}", m.info.name, t.elapsed());
        let job = Job {
            header: hdr,
            target: target_from_difficulty(u128::MAX),
            start_nonce: 0,
        };
        let batch = 1u32 << 24;
        m.search(&job, batch).unwrap();
        let t = Instant::now();
        let rounds = 20;
        for r in 0..rounds {
            let j = Job {
                start_nonce: r as u64 * batch as u64,
                ..job
            };
            m.search(&j, batch).unwrap();
        }
        let hs = (rounds as f64 * batch as f64) / t.elapsed().as_secs_f64();
        println!("gpu: {:.2} MH/s", hs / 1e6);
    }
}

//! Measures full-memory RandomX hashing speed, exactly as `genoryn-miner` hashes
//! in fast mode: one shared dataset, one VM per thread.
//!
//! cargo run --release --example randomx_bench -- 6 30
//! (threads, seconds). Set GENORYN_RANDOMX_LARGE_PAGES=1 to try 2 MiB pages.
use genoryn_core::pow::FastDataset;
use std::{
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

fn main() -> anyhow::Result<()> {
    let mut args = std::env::args().skip(1);
    let threads: usize = args.next().map_or(Ok(1), |a| a.parse())?;
    let seconds: u64 = args.next().map_or(Ok(30), |a| a.parse())?;
    let started = Instant::now();
    let dataset = FastDataset::new([7u8; 32])?;
    let build = started.elapsed().as_secs_f64();
    let stop = Arc::new(AtomicBool::new(false));
    let hashes = Arc::new(AtomicU64::new(0));
    let workers: Vec<_> = (0..threads)
        .map(|index| {
            let (dataset, stop, hashes) = (dataset.clone(), stop.clone(), hashes.clone());
            std::thread::spawn(move || -> anyhow::Result<()> {
                let vm = dataset.vm()?;
                let mut input = [0u8; 76];
                input[..8].copy_from_slice(&(index as u64).to_le_bytes());
                let mut nonce = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    input[8..16].copy_from_slice(&nonce.to_le_bytes());
                    vm.hash(&input)?;
                    nonce += 1;
                    if nonce.is_multiple_of(64) {
                        hashes.fetch_add(64, Ordering::Relaxed);
                    }
                }
                Ok(())
            })
        })
        .collect();
    // Exclude VM creation and warm-up from the measurement window.
    std::thread::sleep(Duration::from_secs(2));
    let begin = hashes.load(Ordering::Relaxed);
    let window = Instant::now();
    std::thread::sleep(Duration::from_secs(seconds));
    let counted = hashes.load(Ordering::Relaxed) - begin;
    let elapsed = window.elapsed().as_secs_f64();
    stop.store(true, Ordering::Relaxed);
    for worker in workers {
        worker.join().expect("worker panicked")?;
    }
    println!(
        "{{\"threads\":{threads},\"seconds\":{elapsed:.1},\"hashes\":{counted},\"hashrate_hs\":{:.1},\"dataset_build_seconds\":{build:.1},\"large_pages\":{}}}",
        counted as f64 / elapsed,
        dataset.large_pages()
    );
    Ok(())
}

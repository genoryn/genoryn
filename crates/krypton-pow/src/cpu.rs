//! Multi-threaded CPU miner. Each thread runs `LANES` independent hashes in lockstep
//! so their random dataset loads overlap (memory-level parallelism).

use crate::{
    ACCESSES, Dataset, HeaderWords, U256, access_index, finish, hash_le_cmp_target, mix_in,
    seed_state,
};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};

const LANES: usize = 8;

#[derive(Clone, Copy, Debug)]
pub struct Job {
    pub header: HeaderWords,
    pub target: U256,
    pub start_nonce: u64,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Solution {
    pub nonce: u64,
    pub hash: [u8; 32],
}

fn search_batch(ds: &Dataset, job: &Job, base: u64) -> Option<Solution> {
    let mut seeds = [[0u32; 16]; LANES];
    let mut mixes = [[0u32; 32]; LANES];
    for l in 0..LANES {
        let (s, m) = seed_state(&job.header, base.wrapping_add(l as u64));
        seeds[l] = s;
        mixes[l] = m;
    }
    for a in 0..ACCESSES {
        let mut idx = [0usize; LANES];
        for l in 0..LANES {
            idx[l] = access_index(a, seeds[l][0], &mixes[l], ds.pairs) as usize;
        }
        for l in 0..LANES {
            let p = idx[l];
            mix_in(a, &mut mixes[l], &ds.items[2 * p], &ds.items[2 * p + 1]);
        }
    }
    for l in 0..LANES {
        let h = finish(&seeds[l], &mixes[l]);
        if hash_le_cmp_target(&h, &job.target) {
            return Some(Solution {
                nonce: base.wrapping_add(l as u64),
                hash: h,
            });
        }
    }
    None
}

/// Searches nonces on `threads` threads until a solution is found or `stop` is set.
/// Thread `t` scans `start + t*LANES + k*threads*LANES`.
pub fn mine(
    ds: Arc<Dataset>,
    job: Job,
    threads: usize,
    stop: Arc<AtomicBool>,
    hashes: Arc<AtomicU64>,
) -> Option<Solution> {
    let threads = threads.max(1);
    let found = Arc::new(parking_lot::Mutex::new(None));
    std::thread::scope(|scope| {
        for t in 0..threads {
            let ds = ds.clone();
            let stop = stop.clone();
            let hashes = hashes.clone();
            let found = found.clone();
            scope.spawn(move || {
                let stride = (threads * LANES) as u64;
                let mut base = job.start_nonce.wrapping_add((t * LANES) as u64);
                let mut local = 0u64;
                while !stop.load(Ordering::Relaxed) {
                    if let Some(sol) = search_batch(&ds, &job, base) {
                        *found.lock() = Some(sol);
                        stop.store(true, Ordering::Relaxed);
                        break;
                    }
                    base = base.wrapping_add(stride);
                    local += LANES as u64;
                    if local >= 1024 {
                        hashes.fetch_add(local, Ordering::Relaxed);
                        local = 0;
                    }
                }
                hashes.fetch_add(local, Ordering::Relaxed);
            });
        }
    });
    *found.lock()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{
        Cache, PowParams, hash_light, header_words, meets_difficulty, target_from_difficulty,
    };
    #[test]
    fn cpu_miner_finds_valid_solution() {
        let p = PowParams::DEVNET;
        let cache = Cache::new(&p, &[5; 32], 0);
        let ds = Arc::new(Dataset::generate(&p, &cache, 4));
        let job = Job {
            header: header_words(&[3; 32]),
            target: target_from_difficulty(500),
            start_nonce: 0,
        };
        let sol = mine(
            ds.clone(),
            job,
            4,
            Arc::new(AtomicBool::new(false)),
            Arc::new(AtomicU64::new(0)),
        )
        .unwrap();
        let h = hash_light(&cache, ds.pairs, &job.header, sol.nonce);
        assert_eq!(h, sol.hash);
        assert!(meets_difficulty(&h, 500));
    }
}

//! Network statistics for dashboards: a compact per-block summary kept in memory
//! (built once, extended block by block, rolled back on reorganizations) and
//! aggregated into at most `MAX_POINTS` buckets per request.

use krypton_core::Hash32;
use krypton_core::emission::{MAX_SUPPLY, issued_through};
use krypton_core::reward::share_difficulty;
use krypton_store::Chain;
use serde_json::{Value, json};
use std::collections::HashSet;

const MAX_POINTS: usize = 240;
/// Hashrate of one RTX 5090 on KryptoHash, for the "GPU equivalents" estimate.
pub const REFERENCE_GPU_HS: f64 = 74e6;
/// Payout owners seen in shares over this many blocks count as active miners.
pub const ACTIVE_WINDOW: u64 = 360;

#[derive(Clone)]
struct BlockStat {
    hash: Hash32,
    /// Owner whose header became this block.
    finder: Hash32,
    /// Payouts made by this block: (owner, value).
    payouts: Vec<(Hash32, u64)>,
    time: u64,
    difficulty: u128,
    shares: u32,
    txs: u32,
    /// Distinct miner owners (first 4 bytes) credited by this block.
    owners: Vec<u32>,
}

#[derive(Default)]
pub struct Stats {
    blocks: Vec<BlockStat>,
}

fn short(o: &Hash32) -> u32 {
    u32::from_le_bytes([o[0], o[1], o[2], o[3]])
}

impl Stats {
    /// Brings the cache in line with the main chain (cheap when nothing changed).
    pub fn refresh(&mut self, chain: &Chain) {
        let tip = chain.tip().height();
        while let Some(last) = self.blocks.last() {
            let h = self.blocks.len() as u64 - 1;
            if h <= tip && chain.main_hash(h).ok().flatten() == Some(last.hash) {
                break;
            }
            self.blocks.pop();
        }
        for h in self.blocks.len() as u64..=tip {
            let Ok(Some(b)) = chain.block_at(h) else {
                break;
            };
            let mut owners: Vec<u32> = std::iter::once(&b.header.miner)
                .chain(b.shares.iter().map(|s| &s.miner))
                .map(short)
                .collect();
            owners.sort_unstable();
            owners.dedup();
            if h == 0 {
                owners.clear();
            }
            let payouts = chain
                .payouts_at(h)
                .unwrap_or_default()
                .into_iter()
                .map(|p| (p.owner, p.value))
                .collect();
            self.blocks.push(BlockStat {
                hash: b.hash(),
                finder: b.header.miner,
                payouts,
                time: b.header.timestamp,
                difficulty: b.header.difficulty,
                shares: b.shares.len() as u32 + (h > 0) as u32,
                txs: b.txs.len() as u32,
                owners,
            });
        }
    }

    /// Public mining activity of one owner: blocks found, blocks with its shares,
    /// payouts (height, value) and the first and last height it was credited.
    pub fn activity(&self, owner: &Hash32) -> Value {
        let s = short(owner);
        let mut found = 0u64;
        let mut active = 0u64;
        let (mut first, mut last) = (None, None);
        let mut payouts = Vec::new();
        for (h, b) in self.blocks.iter().enumerate().skip(1) {
            if b.finder == *owner {
                found += 1;
            }
            if b.owners.contains(&s) {
                active += 1;
                first.get_or_insert(h);
                last = Some(h);
            }
            for (o, v) in &b.payouts {
                if o == owner {
                    payouts.push(json!({"height": h, "value": v, "time": b.time}));
                }
            }
        }
        json!({
            "blocks_found": found,
            "blocks_with_shares": active,
            "first_seen": first,
            "last_seen": last,
            "payouts": payouts,
        })
    }

    /// Distinct payout owners credited over the last `window` blocks.
    pub fn active_miners(&self, window: u64) -> usize {
        let n = self.blocks.len();
        let from = n.saturating_sub(window as usize).max(1);
        self.blocks[from.min(n)..]
            .iter()
            .flat_map(|b| b.owners.iter())
            .collect::<HashSet<_>>()
            .len()
    }

    /// Hashrate over the last `window` blocks, estimated from credited shares.
    pub fn hashrate(&self, window: usize) -> f64 {
        let n = self.blocks.len();
        if n < 3 {
            return 0.0;
        }
        let from = n.saturating_sub(window).max(2);
        let s = &self.blocks[from..];
        let dt = s
            .last()
            .map(|b| b.time)
            .unwrap_or(0)
            .saturating_sub(self.blocks[from - 1].time);
        let work: f64 = s
            .iter()
            .map(|b| share_difficulty(b.difficulty) as f64 * b.shares as f64)
            .sum();
        work / dt.max(1) as f64
    }

    pub fn summary(&self) -> Value {
        let hr = self.hashrate(60);
        let height = self.blocks.len().saturating_sub(1) as u64;
        json!({
            "estimated_miners": self.active_miners(ACTIVE_WINDOW),
            "miners_last_hour": self.active_miners(60),
            "gpu_equivalents": (hr / REFERENCE_GPU_HS).max(0.0),
            "reference_gpu": "RTX 5090 ≈ 74 MH/s",
            "hashrate": hr,
            "issued": issued_through(height),
            "remaining": MAX_SUPPLY - issued_through(height),
            "max_supply": MAX_SUPPLY,
        })
    }

    /// Bucketed series for blocks whose time is within `seconds` of the tip (0 = all).
    pub fn series(&self, seconds: u64) -> Value {
        let n = self.blocks.len();
        if n < 2 {
            return json!([]);
        }
        let tip_time = self.blocks[n - 1].time;
        // Block 1's interval includes the wait before launch: start after it.
        let first_useful = 2.min(n - 1);
        let start = if seconds == 0 {
            first_useful
        } else {
            let cut = tip_time.saturating_sub(seconds);
            self.blocks
                .partition_point(|b| b.time < cut)
                .max(first_useful)
        };
        let range = &self.blocks[start..];
        if range.is_empty() {
            return json!([]);
        }
        let per = range.len().div_ceil(MAX_POINTS).max(1);
        let mut out = Vec::new();
        let mut first = start;
        for chunk in range.chunks(per) {
            let prev_time = self.blocks[first - 1].time;
            let last = chunk.last().expect("chunk");
            let dt = last.time.saturating_sub(prev_time).max(1);
            // Hashrate over at least 10 blocks: single blocks are too noisy to plot.
            let end = first + chunk.len();
            let from = end.saturating_sub(chunk.len().max(10)).max(1);
            let hr_dt = last.time.saturating_sub(self.blocks[from - 1].time).max(1);
            let work: f64 = self.blocks[from..end]
                .iter()
                .map(|b| share_difficulty(b.difficulty) as f64 * b.shares as f64)
                .sum();
            let miners: HashSet<&u32> = chunk.iter().flat_map(|b| b.owners.iter()).collect();
            let height = (first + chunk.len() - 1) as u64;
            out.push(json!({
                "t": last.time,
                "h": height,
                "hashrate": work / hr_dt as f64,
                "difficulty": chunk.iter().map(|b| b.difficulty as f64).sum::<f64>() / chunk.len() as f64,
                "block_time": dt as f64 / chunk.len() as f64,
                "shares": chunk.iter().map(|b| b.shares as f64).sum::<f64>() / chunk.len() as f64,
                "txs": chunk.iter().map(|b| b.txs as u64).sum::<u64>(),
                "miners": miners.len(),
                "issued": issued_through(height),
            }));
            first += chunk.len();
        }
        json!(out)
    }
}

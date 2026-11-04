//! Transaction pool: fee-rate ordered, nullifier-conflict free, bounded.

use krypton_core::Hash32;
use krypton_core::params::MIN_FEE_PER_KB;
use krypton_core::types::Transaction;
use std::collections::HashMap;

pub const MAX_POOL_BYTES: usize = 256 * 1024 * 1024;

pub struct Entry {
    pub tx: Transaction,
    pub size: usize,
    pub added: u64,
}

impl Entry {
    fn rate(&self) -> u128 {
        self.tx.fee as u128 * 1_000_000 / self.size.max(1) as u128
    }
}

#[derive(Default)]
pub struct Mempool {
    pub txs: HashMap<Hash32, Entry>,
    nfs: HashMap<Hash32, Hash32>,
    bytes: usize,
    pub revision: u64,
}

pub fn min_fee(size: usize) -> u64 {
    size.div_ceil(1024) as u64 * MIN_FEE_PER_KB
}

impl Mempool {
    pub fn contains(&self, id: &Hash32) -> bool {
        self.txs.contains_key(id)
    }

    /// Checks pool-level policy (validity against the chain is checked by the caller).
    pub fn precheck(&self, tx: &Transaction) -> Result<(), String> {
        let size = tx.size();
        if tx.fee < min_fee(size) {
            return Err(format!(
                "fee below minimum relay fee of {} gori",
                min_fee(size)
            ));
        }
        if tx.nullifiers.iter().any(|n| self.nfs.contains_key(&n.0)) {
            return Err("conflicts with a pending transaction".into());
        }
        Ok(())
    }

    pub fn insert(&mut self, tx: Transaction) -> Hash32 {
        let id = tx.txid();
        let size = tx.size();
        for n in &tx.nullifiers {
            self.nfs.insert(n.0, id);
        }
        self.bytes += size;
        self.txs.insert(
            id,
            Entry {
                tx,
                size,
                added: krypton_core::now(),
            },
        );
        self.revision += 1;
        while self.bytes > MAX_POOL_BYTES {
            let Some(worst) = self
                .txs
                .iter()
                .min_by_key(|(_, e)| e.rate())
                .map(|(k, _)| *k)
            else {
                break;
            };
            self.remove(&worst);
        }
        id
    }

    pub fn remove(&mut self, id: &Hash32) {
        if let Some(e) = self.txs.remove(id) {
            self.bytes -= e.size;
            for n in &e.tx.nullifiers {
                self.nfs.remove(&n.0);
            }
            self.revision += 1;
        }
    }

    /// Highest fee rate first, within `max_bytes`.
    pub fn select(&self, max_bytes: usize) -> Vec<Transaction> {
        let mut v: Vec<&Entry> = self.txs.values().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.rate()));
        let mut out = Vec::new();
        let mut used = 0;
        for e in v {
            if used + e.size + 16 > max_bytes {
                continue;
            }
            used += e.size + 16;
            out.push(e.tx.clone());
        }
        out
    }

    /// Drops transactions that the new chain state makes invalid.
    pub fn retain_valid(&mut self, mut still_valid: impl FnMut(&Transaction) -> bool) {
        let ids: Vec<Hash32> = self.txs.keys().copied().collect();
        for id in ids {
            let ok = self
                .txs
                .get(&id)
                .map(|e| still_valid(&e.tx))
                .unwrap_or(true);
            if !ok {
                self.remove(&id);
            }
        }
    }

    pub fn bytes(&self) -> usize {
        self.bytes
    }

    /// Fee per KB that should get a transaction mined within `blocks` blocks: the floor
    /// while the pool fits, otherwise just above the rate at that depth of the backlog.
    pub fn fee_per_kb_within(&self, blocks: usize, block_bytes: usize) -> u64 {
        let mut v: Vec<&Entry> = self.txs.values().collect();
        v.sort_by_key(|e| std::cmp::Reverse(e.rate()));
        let room = blocks.saturating_mul(block_bytes);
        let mut used = 0usize;
        for e in v {
            used += e.size + 16;
            if used > room {
                // rate is fee per byte × 10^6; one more gori per KB outbids it.
                let per_kb = (e.rate() * 1024 / 1_000_000) as u64 + 1;
                return per_kb.max(MIN_FEE_PER_KB);
            }
        }
        MIN_FEE_PER_KB
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn empty_pool_suggests_the_floor() {
        let m = Mempool::default();
        assert_eq!(m.fee_per_kb_within(1, 1 << 20), MIN_FEE_PER_KB);
        assert_eq!(min_fee(100 * 1024), 100 * MIN_FEE_PER_KB);
    }
}

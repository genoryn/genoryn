//! Mining jobs and share verification.
//!
//! A job is a node template whose coinbase pays the pool. Each session mines
//! its own slice of the 64-bit nonce space (`NONCE_BITS` low bits; the high
//! bits are the session's prefix), so two sessions never repeat each other's
//! work and a share cannot be resubmitted by another session. Every share is
//! hashed again here with real RandomX before it is credited.
use crate::protocol::{self, Reject};
use anyhow::{Context, Result, ensure};
use genoryn_core::{block::Block, codec::Hash, pow};
use std::{
    collections::{HashSet, VecDeque},
    sync::Arc,
};

/// Low nonce bits a session owns: 2^40 hashes, about 127 days at 100 kH/s.
pub const NONCE_BITS: u8 = 40;
/// High nonce bits that number sessions: 16.7 million per gateway before reuse.
pub const PREFIX_BITS: u8 = 64 - NONCE_BITS;
/// Jobs kept answerable at once (refreshed templates of the same height).
pub const MAX_JOBS: usize = 8;

#[derive(Debug)]
pub struct Job {
    pub id: String,
    pub block: Block,
    pub header: Vec<u8>,
    /// The coinbase amount this block pays the pool: reward plus fees.
    pub reward: u64,
    pub created_ms: u64,
}
impl Job {
    pub fn new(id: String, block: Block, created_ms: u64) -> Result<Self> {
        let mut header = block.header.clone();
        header.nonce = 0;
        let bytes = header.bytes()?;
        ensure!(
            bytes.len() >= protocol::NONCE_OFFSET + 8,
            "unexpected header encoding"
        );
        let reward = u64::try_from(
            block
                .body
                .miner
                .bundle
                .value_balance
                .checked_neg()
                .context("coinbase value")?,
        )
        .context("coinbase pays a negative amount")?;
        Ok(Self {
            id,
            block,
            header: bytes,
            reward,
            created_ms,
        })
    }
    pub fn height(&self) -> u64 {
        self.block.header.height
    }
    pub fn difficulty(&self) -> u64 {
        self.block.header.difficulty
    }
    pub fn seed(&self) -> Hash {
        self.block.header.pow_seed
    }
    /// The canonical header bytes with `nonce`.
    pub fn header_with(&self, nonce: u64) -> Vec<u8> {
        let mut bytes = self.header.clone();
        bytes[protocol::NONCE_OFFSET..protocol::NONCE_OFFSET + 8]
            .copy_from_slice(&nonce.to_le_bytes());
        bytes
    }
    /// The full block for `nonce`, ready to submit.
    pub fn block_with(&self, nonce: u64) -> Block {
        let mut block = self.block.clone();
        block.header.nonce = nonce;
        block
    }
    pub fn for_session(&self, prefix: u32, share_difficulty: u64) -> protocol::Job {
        protocol::Job {
            job_id: self.id.clone(),
            height: self.height(),
            header: hex::encode(&self.header),
            seed: hex::encode(self.seed()),
            share_difficulty,
            network_difficulty: self.difficulty(),
            nonce_start: protocol::format_nonce(nonce_start(prefix)),
            nonce_bits: NONCE_BITS,
        }
    }
}

pub fn nonce_start(prefix: u32) -> u64 {
    u64::from(prefix & ((1 << PREFIX_BITS) - 1)) << NONCE_BITS
}
pub fn in_range(prefix: u32, nonce: u64) -> bool {
    nonce >> NONCE_BITS == u64::from(prefix & ((1 << PREFIX_BITS) - 1))
}

/// The jobs a gateway currently answers for.
#[derive(Default)]
pub struct JobBook {
    jobs: VecDeque<Arc<Job>>,
    /// The node's tip: jobs at or below it can no longer become blocks.
    tip: u64,
}
impl JobBook {
    pub fn add(&mut self, job: Arc<Job>) {
        // A template of a new height retires the older heights at once.
        let height = job.height();
        self.jobs.retain(|old| old.height() >= height);
        self.jobs.push_back(job);
        while self.jobs.len() > MAX_JOBS {
            self.jobs.pop_front();
        }
    }
    pub fn latest(&self) -> Option<Arc<Job>> {
        self.jobs.back().cloned()
    }
    pub fn set_tip(&mut self, tip: u64) {
        self.tip = self.tip.max(tip);
    }
    pub fn tip(&self) -> u64 {
        self.tip
    }
    pub fn find(&self, id: &str) -> Result<Arc<Job>, Reject> {
        let job = self
            .jobs
            .iter()
            .find(|job| job.id == id)
            .cloned()
            .ok_or(Reject::UnknownJob)?;
        if job.height() <= self.tip {
            return Err(Reject::Stale);
        }
        Ok(job)
    }
}

/// Shares one session already submitted, so none is credited twice.
#[derive(Default)]
pub struct SeenShares {
    seen: HashSet<(String, u64)>,
}
impl SeenShares {
    /// Records `(job, nonce)`; `false` when it was already there.
    pub fn insert(&mut self, job: &str, nonce: u64) -> bool {
        if self.seen.len() > 100_000 {
            // Old jobs are stale anyway; keep memory bounded.
            self.seen.clear();
        }
        self.seen.insert((job.to_owned(), nonce))
    }
    pub fn forget_job(&mut self, job: &str) {
        self.seen.retain(|(id, _)| id != job);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CheckedShare {
    pub hash: Hash,
    pub work: u64,
    pub block: bool,
}

/// Verifies one submission with real RandomX. Range and duplicate checks are
/// the caller's (they need the session); this is the expensive part.
pub fn verify_share(job: &Job, nonce: u64, share_difficulty: u64) -> Result<CheckedShare, Reject> {
    if share_difficulty == 0 {
        return Err(Reject::LowDifficulty);
    }
    let hash = pow::randomx(job.seed(), &job.header_with(nonce)).map_err(|_| Reject::Busy)?;
    if !pow::meets_difficulty(hash, share_difficulty) {
        return Err(Reject::LowDifficulty);
    }
    Ok(CheckedShare {
        hash,
        // A share above the network target is still worth its share difficulty:
        // a lucky share earns no more than an ordinary one.
        work: share_difficulty,
        block: pow::meets_difficulty(hash, job.difficulty()),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use genoryn_core::{
        block::{Body, Header},
        params::Network,
        privacy::{BundleWire, ShieldedTx, TxContext},
    };

    pub(crate) fn fake_block(height: u64, difficulty: u64) -> Block {
        let tx = ShieldedTx {
            context: TxContext {
                protocol: 1,
                network: Network::Devnet,
                kind: 1,
                height,
                expires: 0,
                nonce: [0; 32],
            },
            bundle: BundleWire {
                actions: vec![],
                flags: 6,
                value_balance: -5_000,
                anchor: [0; 32],
                proof: vec![],
                binding_signature: vec![],
            },
        };
        Block {
            header: Header {
                version: 1,
                network: Network::Devnet,
                height,
                previous: [1; 32],
                timestamp: 1_000,
                difficulty,
                pow_seed: [2; 32],
                nonce: 0,
                body_hash: [3; 32],
                notes_root: [4; 32],
                nullifiers_root: [5; 32],
            },
            body: Body {
                miner: tx,
                treasury: None,
                transactions: vec![],
            },
        }
    }

    #[test]
    fn the_nonce_sits_where_the_protocol_says() {
        let job = Job::new("j".into(), fake_block(7, 1), 0).unwrap();
        assert_eq!(job.reward, 5_000);
        let nonce = 0x1122_3344_5566_7788u64;
        let mut header = job.block.header.clone();
        header.nonce = nonce;
        assert_eq!(job.header_with(nonce), header.bytes().unwrap());
        assert_eq!(job.block_with(nonce).header, header);
    }

    #[test]
    fn sessions_own_disjoint_nonce_ranges() {
        assert_eq!(nonce_start(0), 0);
        assert_eq!(nonce_start(1), 1 << 40);
        assert!(in_range(5, nonce_start(5)));
        assert!(in_range(5, nonce_start(6) - 1));
        assert!(!in_range(5, nonce_start(6)));
        assert!(!in_range(5, nonce_start(4)));
        assert_eq!(
            nonce_start(1 << PREFIX_BITS),
            0,
            "prefixes wrap within 24 bits"
        );
    }

    #[test]
    fn stale_unknown_and_duplicate_shares_are_refused() {
        let mut book = JobBook::default();
        let first = Arc::new(Job::new("a".into(), fake_block(10, 1), 0).unwrap());
        book.add(first.clone());
        book.add(Arc::new(
            Job::new("b".into(), fake_block(10, 1), 1).unwrap(),
        ));
        assert!(
            book.find("a").is_ok(),
            "same-height refresh keeps the older job"
        );
        book.add(Arc::new(
            Job::new("c".into(), fake_block(11, 1), 2).unwrap(),
        ));
        assert_eq!(book.find("a").unwrap_err(), Reject::UnknownJob);
        assert_eq!(book.latest().unwrap().id, "c");
        book.set_tip(11);
        assert_eq!(book.find("c").unwrap_err(), Reject::Stale);
        assert_eq!(book.find("zzz").unwrap_err(), Reject::UnknownJob);
        let mut seen = SeenShares::default();
        assert!(seen.insert("c", 9));
        assert!(!seen.insert("c", 9));
        assert!(seen.insert("c", 10));
        seen.forget_job("c");
        assert!(seen.insert("c", 9));
    }

    #[test]
    fn real_randomx_decides_every_share() {
        let job = Job::new("j".into(), fake_block(3, 1_000_000), 0).unwrap();
        let mut accepted = None;
        for nonce in 0..400u64 {
            match verify_share(&job, nonce, 8) {
                Ok(share) => {
                    let hash = pow::randomx(job.seed(), &job.header_with(nonce)).unwrap();
                    assert_eq!(share.hash, hash);
                    assert!(pow::meets_difficulty(hash, 8));
                    assert_eq!(share.work, 8);
                    accepted = Some(nonce);
                    break;
                }
                Err(reject) => assert_eq!(reject, Reject::LowDifficulty),
            }
        }
        let nonce = accepted.expect("a difficulty-8 share within 400 hashes");
        let low = (0..200u64)
            .map(|n| nonce + 1 + n)
            .find(|n| verify_share(&job, *n, 8).is_err())
            .unwrap();
        assert_eq!(verify_share(&job, low, 8), Err(Reject::LowDifficulty));
        assert_eq!(verify_share(&job, nonce, 0), Err(Reject::LowDifficulty));
        let easy = Job::new("e".into(), fake_block(3, 1), 0).unwrap();
        assert!(
            verify_share(&easy, 0, 1).unwrap().block,
            "difficulty 1 is always a block"
        );
    }
}

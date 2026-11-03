//! Light client protocol.
//!
//! A light client never trusts a server for validity, only for availability:
//!
//! 1. **Tip**: the tip header commits (MMR root) to every earlier block. The client
//!    samples headers across the whole chain, checks each one's proof of work and
//!    its MMR inclusion proof, and fully checks the most recent headers
//!    ([`verify_flyclient`]). Cost grows with log(chain length), so even a chain of
//!    billions of blocks is verified in seconds.
//! 2. **Blocks since the wallet birthday** arrive as [`CompactBlock`]s whose outputs,
//!    nullifiers and note-tree updates are each checked against the block header
//!    ([`CompactVerifier`]).
//! 3. Keys, trial decryption and proof generation never leave the device.

#![forbid(unsafe_code)]

#[cfg(feature = "client")]
pub mod client;

use anyhow::{Result, bail, ensure};
use krypton_core::codec::{Reader, Writer};
use krypton_core::mmr::{self, MmrNode, MmrProof};
use krypton_core::reward::{Payout, payout_digest};
use krypton_core::tree::Frontier;
use krypton_core::types::{BlockHeader, Output, nullifier_digest_raw, output_digest_raw};
use krypton_core::{ChainParams, Hash32};
use krypton_crypto::field::{digest_from_bytes, digest_to_bytes};
use krypton_pow::Verifier;

/// Everything a wallet needs from one block.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CompactBlock {
    pub header: BlockHeader,
    /// Note tree size before this block.
    pub leaf_start: u64,
    /// All transaction outputs of the block, in order.
    pub outputs: Vec<Output>,
    /// All nullifiers of the block in transaction order.
    pub nullifiers: Vec<Hash32>,
    /// Reward payouts made by this block (committed by `header.payout_digest`).
    pub payouts: Vec<Payout>,
    /// Payouts of block `matured_from` maturing into the note tree in this block.
    pub matured: Vec<Payout>,
    pub matured_from: u64,
}

fn write_payouts(w: &mut Writer, v: &[Payout]) {
    w.u32(v.len() as u32);
    for p in v {
        w.raw(&p.owner).u64(p.value);
    }
}

fn read_payouts(r: &mut Reader) -> Result<Vec<Payout>> {
    let n = r.u32()? as usize;
    ensure!(
        n <= krypton_core::params::MAX_PAYOUTS_PER_BLOCK,
        "too many payouts"
    );
    (0..n)
        .map(|_| {
            Ok(Payout {
                owner: r.array()?,
                value: r.u64()?,
            })
        })
        .collect()
}

impl CompactBlock {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(self.header.to_bytes());
        w.u64(self.leaf_start).u32(self.outputs.len() as u32);
        for o in &self.outputs {
            w.raw(&o.cm).bytes(&o.ciphertext);
        }
        w.u32(self.nullifiers.len() as u32);
        for n in &self.nullifiers {
            w.raw(n);
        }
        write_payouts(&mut w, &self.payouts);
        write_payouts(&mut w, &self.matured);
        w.u64(self.matured_from);
        w.0
    }
    pub fn from_reader(r: &mut Reader) -> Result<Self> {
        let header = BlockHeader::from_reader(r)?;
        let leaf_start = r.u64()?;
        let n = r.u32()? as usize;
        ensure!(n <= 20_000, "too many outputs");
        let mut outputs = Vec::with_capacity(n);
        for _ in 0..n {
            let cm = r.array()?;
            let ciphertext = r.bytes(krypton_core::types::NOTE_CIPHERTEXT_LEN)?;
            outputs.push(Output { cm, ciphertext });
        }
        let n = r.u32()? as usize;
        ensure!(n <= 20_000, "too many nullifiers");
        let nullifiers = (0..n)
            .map(|_| r.array())
            .collect::<krypton_core::Result<_>>()?;
        let payouts = read_payouts(r)?;
        let matured = read_payouts(r)?;
        let matured_from = r.u64()?;
        Ok(Self {
            header,
            leaf_start,
            outputs,
            nullifiers,
            payouts,
            matured,
            matured_from,
        })
    }
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }
    /// Commitments of the matured payouts, recomputed from their public data.
    pub fn matured_cms(&self) -> Result<Vec<Hash32>> {
        Ok(krypton_core::consensus::payout_cms(
            self.matured_from,
            &self.matured,
        )?)
    }
    /// Leaves appended to the note tree, in order.
    pub fn appended(&self) -> Result<Vec<Hash32>> {
        let mut v = self.matured_cms()?;
        v.extend(self.outputs.iter().map(|o| o.cm));
        Ok(v)
    }
}

pub fn encode_compact_list(v: &[CompactBlock]) -> Vec<u8> {
    let mut w = Writer::default();
    w.u32(v.len() as u32);
    for b in v {
        w.bytes(&b.to_bytes());
    }
    w.0
}

pub fn decode_compact_list(b: &[u8]) -> Result<Vec<CompactBlock>> {
    let mut r = Reader::new(b);
    let n = r.u32()? as usize;
    ensure!(n <= 10_000, "too many blocks");
    let mut v = Vec::with_capacity(n);
    for _ in 0..n {
        let raw = r.bytes(64 << 20)?;
        let mut rr = Reader::new(&raw);
        v.push(CompactBlock::from_reader(&mut rr)?);
        rr.finish()?;
    }
    r.finish()?;
    Ok(v)
}

/// Checks a stream of compact blocks against their headers and the note tree.
pub struct CompactVerifier {
    pub last: BlockHeader,
    pub frontier: Frontier,
}

impl CompactVerifier {
    /// Starts from a trusted header (from [`verify_flyclient`]) and the frontier at
    /// that height, which is checked against the header's note root.
    pub fn new(start: BlockHeader, frontier: Frontier) -> Result<Self> {
        ensure!(
            digest_to_bytes(&frontier.root()) == start.note_root,
            "frontier does not match header note root"
        );
        Ok(Self {
            last: start,
            frontier,
        })
    }
    pub fn verify(&mut self, b: &CompactBlock) -> Result<()> {
        let h = &b.header;
        ensure!(
            h.height == self.last.height + 1 && h.prev == self.last.hash(),
            "compact block does not extend chain"
        );
        ensure!(b.leaf_start == self.frontier.size, "wrong leaf start");
        ensure!(
            output_digest_raw(&b.outputs) == h.output_digest,
            "outputs do not match header"
        );
        ensure!(
            nullifier_digest_raw(&b.nullifiers) == h.nullifier_digest,
            "nullifiers do not match header"
        );
        ensure!(
            payout_digest(&b.payouts) == h.payout_digest,
            "payouts do not match header"
        );
        ensure!(
            b.matured.is_empty() || b.matured_from < h.height,
            "matured payouts must come from an earlier block"
        );
        let mut f = self.frontier.clone();
        for cm in b.appended()? {
            f.append(digest_from_bytes(&cm)?);
        }
        ensure!(
            digest_to_bytes(&f.root()) == h.note_root,
            "note tree update does not match header"
        );
        self.frontier = f;
        self.last = h.clone();
        Ok(())
    }
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlySample {
    pub header: BlockHeader,
    pub proof: MmrProof,
}

#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct FlyProof {
    pub tip: BlockHeader,
    /// The most recent headers, oldest first, ending with `tip`'s parent.
    pub recent: Vec<BlockHeader>,
    pub samples: Vec<FlySample>,
}

/// Heights sampled for a FlyClient check: denser near the tip (where an attacker's
/// fork would start), sparse but present across the whole history.
pub fn sample_heights(tip_height: u64, count: usize, seed: u64) -> Vec<u64> {
    use rand::{Rng, SeedableRng};
    if tip_height <= 1 {
        return vec![];
    }
    let mut rng = rand::rngs::StdRng::seed_from_u64(seed);
    let mut v: Vec<u64> = (0..count)
        .map(|_| {
            // x in [0,1): density proportional to 1/(1-x) (FlyClient-like).
            let u: f64 = rng.r#gen();
            let delta: f64 = 1.0 / tip_height as f64;
            let x = 1.0 - delta.powf(u);
            ((x * tip_height as f64) as u64).min(tip_height - 1)
        })
        .collect();
    v.sort_unstable();
    v.dedup();
    v
}

/// Verifies a tip: sampled headers carry valid PoW and are committed by the tip's MMR,
/// recent headers link up to the tip. Returns the cumulative work implied.
pub fn verify_flyclient(
    params: &ChainParams,
    genesis_hash: &Hash32,
    pow: &Verifier,
    p: &FlyProof,
    expected: &[u64],
) -> Result<u128> {
    let tip = &p.tip;
    ensure!(
        pow.verify(tip.height, &tip.pow_hash(), tip.nonce, tip.difficulty),
        "tip proof of work"
    );
    let leaves = tip.height;
    let mut found = std::collections::BTreeSet::new();
    for s in &p.samples {
        let h = &s.header;
        ensure!(
            s.proof.leaves == leaves && s.proof.leaf_index == h.height,
            "sample proof shape"
        );
        let hash = h.hash();
        if h.height == 0 {
            ensure!(hash == *genesis_hash, "wrong genesis");
        } else {
            ensure!(
                pow.verify(h.height, &h.pow_hash(), h.nonce, h.difficulty),
                "sample proof of work at {}",
                h.height
            );
        }
        ensure!(
            mmr::verify(&tip.mmr_root, &MmrNode::leaf(&hash, h.difficulty), &s.proof),
            "sample not in tip MMR"
        );
        found.insert(h.height);
    }
    for e in expected {
        if !found.contains(e) {
            bail!("server omitted sampled height {e}");
        }
    }
    // Recent headers: full linkage, proof of work and difficulty rule.
    let mut prev: Option<&BlockHeader> = None;
    let mut history: Vec<(u64, u128)> = Vec::new();
    for h in p.recent.iter().chain(std::iter::once(tip)) {
        if let Some(pr) = prev {
            ensure!(
                h.prev == pr.hash() && h.height == pr.height + 1,
                "recent headers do not link"
            );
            if history.len() > krypton_core::difficulty::WINDOW {
                ensure!(
                    h.difficulty == krypton_core::consensus::expected_difficulty(params, &history),
                    "recent difficulty rule"
                );
            }
        }
        if h.height > 0 {
            ensure!(
                pow.verify(h.height, &h.pow_hash(), h.nonce, h.difficulty),
                "recent proof of work"
            );
        }
        history.push((h.timestamp, h.difficulty));
        prev = Some(h);
    }
    // The MMR root carries the total work of all committed blocks.
    Ok(p.samples.iter().map(|s| s.header.difficulty).sum())
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn sampling_covers_history_and_recent_blocks() {
        let s = sample_heights(1_000_000_000, 80, 42);
        assert!(s.len() > 50);
        assert!(s.iter().any(|&h| h < 500_000_000), "old history sampled");
        assert!(s.iter().any(|&h| h > 999_000_000), "recent blocks sampled");
        assert!(s.iter().all(|&h| h < 1_000_000_000));
    }
}

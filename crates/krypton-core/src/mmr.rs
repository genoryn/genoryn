//! Work-weighted Merkle Mountain Range over all block hashes.
//!
//! Every header commits to the MMR of all previous blocks, so a light client can
//! check any historical header against the tip with a logarithmic proof
//! (FlyClient-style sampling), however long the chain becomes.

use crate::Hash32;
use krypton_crypto::hash::tagged_parts;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MmrNode {
    pub hash: Hash32,
    pub work: u128,
}

impl MmrNode {
    pub fn leaf(block_hash: &Hash32, difficulty: u128) -> Self {
        Self {
            hash: tagged_parts("Krypton/mmr/leaf", &[block_hash, &difficulty.to_le_bytes()]),
            work: difficulty,
        }
    }
    pub fn parent(l: &Self, r: &Self) -> Self {
        let work = l.work.saturating_add(r.work);
        Self {
            hash: tagged_parts("Krypton/mmr/node", &[&l.hash, &r.hash, &work.to_le_bytes()]),
            work,
        }
    }
    pub fn to_bytes(&self) -> [u8; 48] {
        let mut b = [0u8; 48];
        b[..32].copy_from_slice(&self.hash);
        b[32..].copy_from_slice(&self.work.to_le_bytes());
        b
    }
    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        Some(Self {
            hash: b.get(..32)?.try_into().ok()?,
            work: u128::from_le_bytes(b.get(32..48)?.try_into().ok()?),
        })
    }
}

pub trait MmrStore {
    fn mmr_get(&self, pos: u64) -> Option<MmrNode>;
}

pub fn node_count(leaves: u64) -> u64 {
    2 * leaves - leaves.count_ones() as u64
}

pub fn leaf_pos(leaf: u64) -> u64 {
    2 * leaf - leaf.count_ones() as u64
}

/// Peaks of an MMR with `leaves` leaves: (position, height, first leaf), left to right.
pub fn peaks(leaves: u64) -> Vec<(u64, u32, u64)> {
    let mut v = Vec::new();
    let mut cum_nodes = 0u64;
    let mut first_leaf = 0u64;
    for h in (0..64u32).rev() {
        if leaves >> h & 1 == 1 {
            let size = (1u64 << (h + 1)) - 1;
            v.push((cum_nodes + size - 1, h, first_leaf));
            cum_nodes += size;
            first_leaf += 1 << h;
        }
    }
    v
}

/// Appends a leaf to an MMR with `leaves` leaves; returns nodes to persist (position, node).
pub fn append(store: &impl MmrStore, leaves: u64, leaf: MmrNode) -> Vec<(u64, MmrNode)> {
    let mut out = Vec::new();
    let mut pos = node_count(leaves);
    out.push((pos, leaf));
    let mut node = leaf;
    let mut height = 0u32;
    // The new leaf merges with left siblings while the leaf count has trailing ones.
    let mut n = leaves;
    while n & 1 == 1 {
        let left_pos = pos + 1 - (1u64 << (height + 1));
        let left = out
            .iter()
            .find(|(p, _)| *p == left_pos)
            .map(|x| x.1)
            .or_else(|| store.mmr_get(left_pos))
            .expect("left sibling stored");
        node = MmrNode::parent(&left, &node);
        pos += 1;
        height += 1;
        out.push((pos, node));
        n >>= 1;
    }
    out
}

/// Root committing to all peaks, the leaf count and the total work.
pub fn root(store: &impl MmrStore, leaves: u64) -> Hash32 {
    let ps: Vec<MmrNode> = peaks(leaves)
        .iter()
        .map(|(p, _, _)| store.mmr_get(*p).expect("peak stored"))
        .collect();
    root_from_peaks(&ps, leaves)
}

pub fn root_from_peaks(peaks: &[MmrNode], leaves: u64) -> Hash32 {
    let mut acc: Option<MmrNode> = None;
    for p in peaks.iter().rev() {
        acc = Some(match acc {
            None => *p,
            Some(a) => {
                let work = p.work.saturating_add(a.work);
                MmrNode {
                    hash: tagged_parts("Krypton/mmr/bag", &[&p.hash, &a.hash, &work.to_le_bytes()]),
                    work,
                }
            }
        });
    }
    let (h, w) = acc.map(|a| (a.hash, a.work)).unwrap_or(([0; 32], 0));
    tagged_parts(
        "Krypton/mmr/root",
        &[&h, &w.to_le_bytes(), &leaves.to_le_bytes()],
    )
}

/// Inclusion proof of one leaf.
#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct MmrProof {
    pub leaves: u64,
    pub leaf_index: u64,
    /// Sibling nodes from the leaf up to its peak.
    #[serde(with = "nodes_hex")]
    pub siblings: Vec<MmrNode>,
    /// All peaks, with the leaf's own peak replaced by a placeholder.
    #[serde(with = "nodes_hex")]
    pub peaks: Vec<MmrNode>,
    pub peak_index: u32,
}

pub fn prove(store: &impl MmrStore, leaves: u64, leaf_index: u64) -> Option<MmrProof> {
    if leaf_index >= leaves {
        return None;
    }
    let ps = peaks(leaves);
    let (pi, &(peak_pos, height, first)) = ps
        .iter()
        .enumerate()
        .find(|(_, (_, h, f))| leaf_index >= *f && leaf_index < f + (1u64 << h))?;
    // Walk down from the peak, recording siblings.
    let mut siblings = Vec::new();
    let mut pos = peak_pos;
    let mut h = height;
    let mut start = first;
    while h > 0 {
        let right = pos - 1;
        let left = pos - (1u64 << h);
        let half = 1u64 << (h - 1);
        if leaf_index < start + half {
            siblings.push(store.mmr_get(right)?);
            pos = left;
        } else {
            siblings.push(store.mmr_get(left)?);
            pos = right;
            start += half;
        }
        h -= 1;
    }
    siblings.reverse();
    let mut peak_nodes = Vec::new();
    for (i, (p, _, _)) in ps.iter().enumerate() {
        peak_nodes.push(if i == pi {
            MmrNode {
                hash: [0; 32],
                work: 0,
            }
        } else {
            store.mmr_get(*p)?
        });
    }
    Some(MmrProof {
        leaves,
        leaf_index,
        siblings,
        peaks: peak_nodes,
        peak_index: pi as u32,
    })
}

/// Verifies that `leaf` is leaf `proof.leaf_index` of the MMR with root `root`.
pub fn verify(root: &Hash32, leaf: &MmrNode, proof: &MmrProof) -> bool {
    let ps = peaks(proof.leaves);
    let Some(&(_, height, first)) = ps.get(proof.peak_index as usize) else {
        return false;
    };
    if proof.peaks.len() != ps.len()
        || proof.siblings.len() != height as usize
        || proof.leaf_index < first
        || proof.leaf_index >= first + (1u64 << height)
    {
        return false;
    }
    let mut node = *leaf;
    let offset = proof.leaf_index - first;
    for (k, s) in proof.siblings.iter().enumerate() {
        node = if (offset >> k) & 1 == 0 {
            MmrNode::parent(&node, s)
        } else {
            MmrNode::parent(s, &node)
        };
    }
    let mut all = proof.peaks.clone();
    all[proof.peak_index as usize] = node;
    root_from_peaks(&all, proof.leaves) == *root
}

/// Like [`verify`], and on success returns the work committed by the root: (work of all
/// leaves before this one, work of all leaves). Both are authenticated by the root, since
/// every node hashes the work beneath it.
pub fn verify_work(root: &Hash32, leaf: &MmrNode, proof: &MmrProof) -> Option<(u128, u128)> {
    if !verify(root, leaf, proof) {
        return None;
    }
    let first = peaks(proof.leaves)[proof.peak_index as usize].2;
    let offset = proof.leaf_index - first;
    let mut node = *leaf;
    let mut before = 0u128;
    for (k, s) in proof.siblings.iter().enumerate() {
        node = if (offset >> k) & 1 == 0 {
            MmrNode::parent(&node, s)
        } else {
            before = before.saturating_add(s.work);
            MmrNode::parent(s, &node)
        };
    }
    let pi = proof.peak_index as usize;
    let left: u128 = proof.peaks[..pi].iter().map(|p| p.work).sum();
    let right: u128 = proof.peaks[pi + 1..].iter().map(|p| p.work).sum();
    Some((
        left.saturating_add(before),
        left.saturating_add(node.work).saturating_add(right),
    ))
}

mod nodes_hex {
    use super::MmrNode;
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &[MmrNode], s: S) -> Result<S::Ok, S::Error> {
        let strs: Vec<String> = v.iter().map(|n| hex::encode(n.to_bytes())).collect();
        serde::Serialize::serialize(&strs, s)
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Vec<MmrNode>, D::Error> {
        let strs = Vec::<String>::deserialize(d)?;
        strs.iter()
            .map(|s| {
                hex::decode(s)
                    .ok()
                    .and_then(|b| MmrNode::from_bytes(&b))
                    .ok_or_else(|| serde::de::Error::custom("bad mmr node"))
            })
            .collect()
    }
}

/// In-memory MMR (tests and light clients).
#[derive(Default, Clone)]
pub struct MemMmr {
    pub nodes: Vec<MmrNode>,
    pub leaves: u64,
}

impl MmrStore for MemMmr {
    fn mmr_get(&self, pos: u64) -> Option<MmrNode> {
        self.nodes.get(pos as usize).copied()
    }
}

impl MemMmr {
    pub fn push(&mut self, leaf: MmrNode) {
        for (p, n) in append(self, self.leaves, leaf) {
            assert_eq!(p as usize, self.nodes.len());
            self.nodes.push(n);
        }
        self.leaves += 1;
    }
    pub fn root(&self) -> Hash32 {
        root(self, self.leaves)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn proofs_for_every_leaf_and_size() {
        let mut m = MemMmr::default();
        let empty = m.root();
        for i in 0..40u64 {
            m.push(MmrNode::leaf(&[i as u8; 32], 10 + i as u128));
            assert_eq!(m.nodes.len() as u64, node_count(m.leaves));
            let r = m.root();
            assert_ne!(r, empty);
            for j in 0..=i {
                let p = prove(&m, m.leaves, j).unwrap();
                let leaf = MmrNode::leaf(&[j as u8; 32], 10 + j as u128);
                assert!(verify(&r, &leaf, &p), "leaf {j} of {}", m.leaves);
                let wrong = MmrNode::leaf(&[j as u8; 32], 11 + j as u128);
                assert!(!verify(&r, &wrong, &p));
                // The work before leaf j and the total, both proven by the root.
                let before: u128 = (0..j as u128).map(|k| 10 + k).sum();
                let all: u128 = (0..=i as u128).map(|k| 10 + k).sum();
                assert_eq!(verify_work(&r, &leaf, &p), Some((before, all)));
                assert_eq!(verify_work(&r, &wrong, &p), None);
            }
        }
        let total: u128 = (0..40u128).map(|i| 10 + i).sum();
        let ps: Vec<_> = peaks(m.leaves)
            .iter()
            .map(|(p, _, _)| m.nodes[*p as usize])
            .collect();
        assert_eq!(ps.iter().map(|p| p.work).sum::<u128>(), total);
    }
}

//! Append-only note commitment tree (depth 40, Rescue-Prime nodes).
//!
//! Completed internal nodes are persisted through [`NodeStore`]; the tree can then
//! serve authentication paths for any leaf and recompute the root after appends.

use krypton_crypto::field::{Digest, ZERO_DIGEST};
use krypton_crypto::rescue::merkle_node;
use krypton_zk::layout::DEPTH;
use std::sync::OnceLock;

/// Roots of empty subtrees: `EMPTY[0]` is the empty leaf.
pub fn empty_roots() -> &'static [Digest; DEPTH + 1] {
    static E: OnceLock<[Digest; DEPTH + 1]> = OnceLock::new();
    E.get_or_init(|| {
        let mut e = [ZERO_DIGEST; DEPTH + 1];
        for k in 1..=DEPTH {
            e[k] = merkle_node(&e[k - 1], &e[k - 1]);
        }
        e
    })
}

pub trait NodeStore {
    /// Completed node at (`level`, `index`); level 0 are leaves.
    fn get(&self, level: u8, index: u64) -> Option<Digest>;
}

/// Rightmost frontier of the tree: enough to append leaves and compute roots.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub struct Frontier {
    pub size: u64,
    /// `ommers[k]` is the completed left node at level k waiting for a right sibling.
    pub ommers: Vec<Option<Digest>>,
}

impl Frontier {
    pub fn new() -> Self {
        Self {
            size: 0,
            ommers: vec![None; DEPTH],
        }
    }

    /// Appends a leaf; returns the completed nodes (level, index, digest) created,
    /// including the leaf itself.
    pub fn append(&mut self, leaf: Digest) -> Vec<(u8, u64, Digest)> {
        assert!(self.size < 1u64 << DEPTH, "note tree full");
        if self.ommers.len() != DEPTH {
            self.ommers.resize(DEPTH, None);
        }
        let mut created = vec![(0u8, self.size, leaf)];
        let mut node = leaf;
        let mut index = self.size;
        for level in 0..DEPTH {
            if index & 1 == 0 {
                self.ommers[level] = Some(node);
                break;
            }
            let left = self.ommers[level]
                .take()
                .expect("left ommer exists for a right child");
            node = merkle_node(&left, &node);
            index >>= 1;
            created.push((level as u8 + 1, index, node));
        }
        self.size += 1;
        created
    }

    pub fn root(&self) -> Digest {
        let e = empty_roots();
        let mut node = e[0];
        let mut have = false;
        let mut index = self.size;
        // Fold from the bottom: combine ommers with the running right-side node.
        for level in 0..DEPTH {
            let bit = index & 1;
            let right = if have { node } else { e[level] };
            node = match (bit, self.ommers.get(level).copied().flatten()) {
                (1, Some(left)) => {
                    have = true;
                    merkle_node(&left, &right)
                }
                _ => {
                    if have {
                        merkle_node(&node, &e[level])
                    } else {
                        e[level + 1]
                    }
                }
            };
            index >>= 1;
        }
        node
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut v = self.size.to_le_bytes().to_vec();
        for o in &self.ommers {
            match o {
                Some(d) => {
                    v.push(1);
                    v.extend_from_slice(&krypton_crypto::field::digest_to_bytes(d));
                }
                None => v.push(0),
            }
        }
        v
    }

    pub fn from_bytes(b: &[u8]) -> Option<Self> {
        let size = u64::from_le_bytes(b.get(..8)?.try_into().ok()?);
        let mut ommers = Vec::with_capacity(DEPTH);
        let mut p = 8;
        for _ in 0..DEPTH {
            match *b.get(p)? {
                0 => {
                    ommers.push(None);
                    p += 1;
                }
                1 => {
                    let d: [u8; 32] = b.get(p + 1..p + 33)?.try_into().ok()?;
                    ommers.push(Some(krypton_crypto::field::digest_from_bytes(&d).ok()?));
                    p += 33;
                }
                _ => return None,
            }
        }
        (p == b.len()).then_some(Self { size, ommers })
    }
}

/// Node at (`level`, `index`) of a tree holding `size` leaves.
pub fn node_at(store: &impl NodeStore, size: u64, level: u8, index: u64) -> Digest {
    let e = empty_roots();
    let first_leaf = index << level;
    if first_leaf >= size {
        return e[level as usize];
    }
    let complete = (index + 1)
        .checked_shl(level as u32)
        .is_some_and(|end| end <= size);
    if complete && let Some(d) = store.get(level, index) {
        return d;
    }
    if level == 0 {
        return e[0];
    }
    merkle_node(
        &node_at(store, size, level - 1, 2 * index),
        &node_at(store, size, level - 1, 2 * index + 1),
    )
}

/// Nodes completed by appending leaves `start..end` (to delete when undoing them).
pub fn nodes_completed_by(start: u64, end: u64) -> Vec<(u8, u64)> {
    let mut v = Vec::new();
    for p in start..end {
        for level in 0..=DEPTH as u8 {
            if level > 0 && (p + 1) % (1u64 << level) != 0 {
                break;
            }
            v.push((level, p >> level));
        }
    }
    v
}

/// Authentication path (siblings from the leaf upward) of leaf `pos`.
pub fn auth_path(store: &impl NodeStore, size: u64, pos: u64) -> Vec<Digest> {
    (0..DEPTH as u8)
        .map(|level| node_at(store, size, level, (pos >> level) ^ 1))
        .collect()
}

/// In-memory node store (tests, wallets).
#[derive(Default, Clone)]
pub struct MemTree {
    pub nodes: std::collections::HashMap<(u8, u64), Digest>,
    pub frontier: Frontier,
}

impl MemTree {
    pub fn new() -> Self {
        Self {
            nodes: Default::default(),
            frontier: Frontier::new(),
        }
    }
    pub fn append(&mut self, leaf: Digest) -> u64 {
        let pos = self.frontier.size;
        for (l, i, d) in self.frontier.append(leaf) {
            self.nodes.insert((l, i), d);
        }
        pos
    }
    pub fn root(&self) -> Digest {
        self.frontier.root()
    }
    pub fn path(&self, pos: u64) -> Vec<Digest> {
        auth_path(self, self.frontier.size, pos)
    }
}

impl NodeStore for MemTree {
    fn get(&self, level: u8, index: u64) -> Option<Digest> {
        self.nodes.get(&(level, index)).copied()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use krypton_crypto::rescue::scalar;
    use krypton_zk::layout::DEPTH;

    fn naive_root(leaves: &[Digest]) -> Digest {
        let e = empty_roots();
        let mut level: Vec<Digest> = leaves.to_vec();
        for k in 0..DEPTH {
            let mut next = Vec::new();
            for pair in level.chunks(2) {
                let r = if pair.len() == 2 { pair[1] } else { e[k] };
                next.push(merkle_node(&pair[0], &r));
            }
            if next.is_empty() {
                return e[DEPTH];
            }
            level = next;
        }
        level[0]
    }

    #[test]
    fn frontier_root_matches_naive_and_paths_verify() {
        let mut t = MemTree::new();
        assert_eq!(t.root(), empty_roots()[DEPTH]);
        let mut leaves = Vec::new();
        for i in 0..37u64 {
            let leaf = scalar(1000 + i);
            leaves.push(leaf);
            t.append(leaf);
            assert_eq!(t.root(), naive_root(&leaves), "size {}", i + 1);
        }
        for pos in [0u64, 1, 2, 17, 36] {
            let path = t.path(pos);
            let mut cur = leaves[pos as usize];
            for (k, sib) in path.iter().enumerate() {
                cur = if (pos >> k) & 1 == 0 {
                    merkle_node(&cur, sib)
                } else {
                    merkle_node(sib, &cur)
                };
            }
            assert_eq!(cur, t.root());
        }
        let f2 = Frontier::from_bytes(&t.frontier.to_bytes()).unwrap();
        assert_eq!(f2, t.frontier);
    }
}

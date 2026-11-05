//! Domain-separated two-to-one Rescue-Prime hashing.
//!
//! `H(domain; a, b)` runs one Rp64_256 permutation over the state
//! `[domain, 0, 0, 0, a0, a1, a2, a3, b0, b1, b2, b3]` and returns state[4..8].
//! The same function is enforced inside the spend STARK, so every value derived
//! here (keys, notes, nullifiers, tree nodes) is provable without elliptic curves.

use crate::field::{Digest, Felt, ZERO_DIGEST};
use winterfell::crypto::hashers::Rp64_256;
use winterfell::math::FieldElement;

/// Domain tags. They occupy the first capacity element of the permutation.
pub mod domain {
    pub const OWNER: u64 = 0x4b52_0001;
    pub const NULLIFIER: u64 = 0x4b52_0002;
    pub const NULLIFIER_KEY: u64 = 0x4b52_0003;
    pub const AUTH_KEY: u64 = 0x4b52_0004;
    pub const NOTE_COMMIT: u64 = 0x4b52_0005;
    pub const NOTE_BODY: u64 = 0x4b52_0006;
    pub const MERKLE: u64 = 0x4b52_0007;
    pub const SIGHASH: u64 = 0x4b52_0008;
    /// Owner tag of a lock note (atomic swaps). Ordinary spends hash owners under
    /// `OWNER`, so they can never spend a lock note.
    pub const LOCK_OWNER: u64 = 0x4b52_0009;
    /// Added to `NULLIFIER` for dummy (zero-value) inputs so they can never
    /// collide with the nullifier of a real note.
    pub const DUMMY_OFFSET: u64 = 0x0100_0000;
}

pub fn permute(state: &mut [Felt; 12]) {
    Rp64_256::apply_permutation(state);
}

pub fn hash2(domain: u64, a: &Digest, b: &Digest) -> Digest {
    let mut s = [Felt::ZERO; 12];
    s[0] = Felt::new(domain);
    s[4..8].copy_from_slice(a);
    s[8..12].copy_from_slice(b);
    permute(&mut s);
    [s[4], s[5], s[6], s[7]]
}

pub fn scalar(v: u64) -> Digest {
    [Felt::new(v), Felt::ZERO, Felt::ZERO, Felt::ZERO]
}

/// Spending key material derived from the secret `sk`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SpendKeys {
    pub sk: Digest,
    /// Authorization key: proves possession of `sk` without revealing it.
    pub ak: Digest,
    /// Nullifier key: lets a view-only wallet detect spends, but not spend.
    pub nk: Digest,
    /// The public owner tag committed inside every note sent to this wallet.
    pub owner: Digest,
}

impl SpendKeys {
    pub fn from_sk(sk: Digest) -> Self {
        let ak = hash2(domain::AUTH_KEY, &sk, &ZERO_DIGEST);
        let nk = hash2(domain::NULLIFIER_KEY, &sk, &ZERO_DIGEST);
        let owner = owner_of(&ak, &nk);
        Self { sk, ak, nk, owner }
    }
}

pub fn owner_of(ak: &Digest, nk: &Digest) -> Digest {
    hash2(domain::OWNER, ak, nk)
}

pub fn lock_owner_of(ak: &Digest, nk: &Digest) -> Digest {
    hash2(domain::LOCK_OWNER, ak, nk)
}

/// Note commitment: `cm = H(CM; H(BODY; owner, [value,0,0,0]), rcm)`.
pub fn note_commitment(owner: &Digest, value: u64, rcm: &Digest) -> Digest {
    let body = hash2(domain::NOTE_BODY, owner, &scalar(value));
    hash2(domain::NOTE_COMMIT, &body, rcm)
}

/// Nullifier of the note at tree position `pos` owned by `nk`.
pub fn nullifier(nk: &Digest, pos: u64) -> Digest {
    hash2(domain::NULLIFIER, nk, &scalar(pos))
}

pub fn dummy_nullifier(nk: &Digest, pos: u64) -> Digest {
    hash2(domain::NULLIFIER + domain::DUMMY_OFFSET, nk, &scalar(pos))
}

pub fn merkle_node(left: &Digest, right: &Digest) -> Digest {
    hash2(domain::MERKLE, left, right)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn domains_separate_outputs() {
        let a = scalar(1);
        let b = scalar(2);
        assert_ne!(hash2(domain::OWNER, &a, &b), hash2(domain::MERKLE, &a, &b));
        assert_ne!(nullifier(&a, 5), dummy_nullifier(&a, 5));
        assert_ne!(merkle_node(&a, &b), merkle_node(&b, &a));
    }
    #[test]
    fn spend_keys_are_deterministic() {
        let k1 = SpendKeys::from_sk(scalar(42));
        let k2 = SpendKeys::from_sk(scalar(42));
        assert_eq!(k1, k2);
        assert_ne!(k1.owner, SpendKeys::from_sk(scalar(43)).owner);
    }
}

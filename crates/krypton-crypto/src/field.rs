//! The Goldilocks prime field (p = 2^64 - 2^32 + 1) and 4-element digests.

use crate::{CryptoError, Result};
pub use winterfell::math::fields::f64::BaseElement as Felt;
use winterfell::math::{FieldElement, StarkField};

/// A Rescue-Prime digest: four field elements (about 256 bits).
pub type Digest = [Felt; 4];

pub const ZERO_DIGEST: Digest = [Felt::ZERO; 4];
pub const MODULUS: u64 = Felt::MODULUS;

/// Canonical little-endian encoding of a digest (32 bytes).
pub fn digest_to_bytes(d: &Digest) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (i, e) in d.iter().enumerate() {
        out[i * 8..i * 8 + 8].copy_from_slice(&e.as_int().to_le_bytes());
    }
    out
}

/// Strict decoding: every limb must be a canonical field element (< p).
pub fn digest_from_bytes(b: &[u8; 32]) -> Result<Digest> {
    let mut d = ZERO_DIGEST;
    for (i, e) in d.iter_mut().enumerate() {
        let v = u64::from_le_bytes(b[i * 8..i * 8 + 8].try_into().expect("8 bytes"));
        if v >= MODULUS {
            return Err(CryptoError::Encoding("non-canonical field element"));
        }
        *e = Felt::new(v);
    }
    Ok(d)
}

/// Maps 32 uniformly random bytes onto a digest (bias < 2^-32 per limb).
pub fn digest_from_uniform(b: &[u8; 32]) -> Digest {
    let mut d = ZERO_DIGEST;
    for (i, e) in d.iter_mut().enumerate() {
        *e = Felt::new(u64::from_le_bytes(
            b[i * 8..i * 8 + 8].try_into().expect("8 bytes"),
        ));
    }
    d
}

pub fn felt(v: u64) -> Felt {
    Felt::new(v)
}

pub fn felt_u64(f: Felt) -> u64 {
    f.as_int()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn digest_roundtrip_and_canonical_check() {
        let d = [felt(1), felt(2), felt(MODULUS - 1), felt(0)];
        assert_eq!(digest_from_bytes(&digest_to_bytes(&d)).unwrap(), d);
        let mut bad = [0u8; 32];
        bad[..8].copy_from_slice(&MODULUS.to_le_bytes());
        assert!(digest_from_bytes(&bad).is_err());
    }
}

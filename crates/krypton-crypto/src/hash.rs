//! Domain-separated byte-oriented hash functions.

use sha3::digest::{ExtendableOutput, Update, XofReader};
use sha3::{Digest as _, Sha3_256, Shake256};

pub type Hash32 = [u8; 32];

/// `SHA3-256(len(tag) || tag || data)`: the identifier hash of the protocol.
pub fn tagged(tag: &str, data: &[u8]) -> Hash32 {
    let mut h = Sha3_256::new();
    sha3::Digest::update(&mut h, (tag.len() as u16).to_le_bytes());
    sha3::Digest::update(&mut h, tag.as_bytes());
    sha3::Digest::update(&mut h, data);
    h.finalize().into()
}

/// Tagged hash over several parts, each length-prefixed (no ambiguity).
pub fn tagged_parts(tag: &str, parts: &[&[u8]]) -> Hash32 {
    let mut h = Sha3_256::new();
    sha3::Digest::update(&mut h, (tag.len() as u16).to_le_bytes());
    sha3::Digest::update(&mut h, tag.as_bytes());
    for p in parts {
        sha3::Digest::update(&mut h, (p.len() as u64).to_le_bytes());
        sha3::Digest::update(&mut h, p);
    }
    h.finalize().into()
}

/// SHAKE256 expansion with a domain tag, used for deterministic key derivation.
pub fn shake(tag: &str, data: &[u8], out: &mut [u8]) {
    let mut x = Shake256::default();
    x.update(&(tag.len() as u16).to_le_bytes());
    x.update(tag.as_bytes());
    x.update(data);
    x.finalize_xof().read(out);
}

pub fn blake3(data: &[u8]) -> Hash32 {
    *::blake3::hash(data).as_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn tags_separate() {
        assert_ne!(tagged("a", b"x"), tagged("b", b"x"));
        assert_ne!(
            tagged_parts("t", &[b"ab", b"c"]),
            tagged_parts("t", &[b"a", b"bc"])
        );
    }
}

//! Consensus encoding: bincode 1 fixed-width little endian, no trailing bytes.
use anyhow::{Result, ensure};
use bincode::Options;
use serde::{Serialize, de::DeserializeOwned};
use sha2::{Digest, Sha256};

pub type Hash = [u8; 32];
pub const MAX_BLOCK_BYTES: usize = 2_000_000;
pub const MAX_TX_BYTES: usize = 200_000;
pub const MAX_ACTIONS: usize = 16;

pub fn encode<T: Serialize>(value: &T) -> Result<Vec<u8>> {
    Ok(bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .serialize(value)?)
}
pub fn decode<T: Serialize + DeserializeOwned>(bytes: &[u8], max: usize) -> Result<T> {
    ensure!(bytes.len() <= max, "encoded object exceeds limit");
    let result = bincode::DefaultOptions::new()
        .with_fixint_encoding()
        .with_little_endian()
        .with_limit(max as u64)
        .reject_trailing_bytes()
        .deserialize(bytes)?;
    ensure!(encode(&result)? == bytes, "noncanonical encoding");
    Ok(result)
}
pub fn hash(domain: &[u8], bytes: &[u8]) -> Hash {
    let mut digest = Sha256::new();
    digest.update((domain.len() as u64).to_le_bytes());
    digest.update(domain);
    digest.update((bytes.len() as u64).to_le_bytes());
    digest.update(bytes);
    digest.finalize().into()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn canonical_vector() {
        assert_eq!(
            hex::encode(encode(&(1u16, 0x01020304u32)).unwrap()),
            "010004030201"
        );
        assert!(decode::<u16>(&[1, 0, 0], 10).is_err());
        assert!(decode::<Vec<u8>>(&[255; 16], 100).is_err());
    }
}

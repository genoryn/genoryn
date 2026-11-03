//! Post-quantum cryptographic primitives used by Genoryn Krypton.
//!
//! Every primitive here is either hash-based or a NIST post-quantum standard:
//!
//! * [`field`] / [`rescue`]: the Goldilocks field and the Rescue-Prime (Rp64_256)
//!   permutation used inside STARK proofs for keys, notes, nullifiers and the
//!   note commitment tree.
//! * [`hash`]: domain-separated SHA3-256 / SHAKE256 / BLAKE3.
//! * [`kem`]: hybrid X25519 + ML-KEM-1024 (FIPS 203) key encapsulation.
//! * [`sign`]: hybrid ML-DSA-87 (FIPS 204) + SLH-DSA-SHAKE-256s (FIPS 205) signatures.
//! * [`aead`]: XChaCha20-Poly1305 authenticated encryption.
//! * [`kdf`]: Argon2id password hashing and SHAKE256 key derivation.
//!
//! All algorithms are addressed by a versioned [`AlgorithmId`] so that the protocol
//! can migrate to new primitives through a soft fork (crypto-agility).

#![forbid(unsafe_code)]

pub mod aead;
pub mod field;
pub mod hash;
pub mod kdf;
pub mod kem;
pub mod rescue;
pub mod sign;

pub use field::{Digest, Felt};

/// Versioned identifiers of every cryptographic algorithm the protocol uses.
/// New identifiers may be added by a soft fork; existing ones never change meaning.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, serde::Serialize, serde::Deserialize)]
#[repr(u16)]
pub enum AlgorithmId {
    /// Rescue-Prime Rp64_256 over Goldilocks, 7 rounds, used in circuits.
    Rp64_256 = 0x0001,
    /// SHA3-256 for block, transaction and header identifiers.
    Sha3_256 = 0x0002,
    /// BLAKE3 inside KryptoHash proof of work.
    Blake3 = 0x0003,
    /// Hybrid X25519 + ML-KEM-1024 key encapsulation.
    HybridKemX25519MlKem1024 = 0x0101,
    /// Hybrid ML-DSA-87 + SLH-DSA-SHAKE-256s signature.
    HybridSigMlDsa87SlhDsa256s = 0x0201,
    /// ML-DSA-87 alone (fast signatures, e.g. node identity).
    MlDsa87 = 0x0202,
    /// Winterfell STARK, Goldilocks with cubic extension, Rp64_256 commitments v1.
    StarkSpendV1 = 0x0301,
}

#[derive(Debug, thiserror::Error)]
pub enum CryptoError {
    #[error("invalid encoding: {0}")]
    Encoding(&'static str),
    #[error("cryptographic operation failed: {0}")]
    Failed(&'static str),
    #[error("authentication failed")]
    Auth,
}

pub type Result<T> = core::result::Result<T, CryptoError>;

//! Consensus types and rules of Genoryn Krypton.

#![forbid(unsafe_code)]

pub mod address;
pub mod codec;
pub mod consensus;
pub mod difficulty;
pub mod emission;
pub mod lock;
pub mod mmr;
pub mod params;
pub mod reward;
pub mod tree;
pub mod types;

pub use params::{ChainParams, Network};
pub use types::{Block, BlockHeader, Transaction};

pub type Hash32 = [u8; 32];

#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum CoreError {
    #[error("decode: {0}")]
    Decode(&'static str),
    #[error("invalid: {0}")]
    Invalid(&'static str),
    #[error("double spend")]
    DoubleSpend,
    #[error("timestamp too far in the future")]
    Future,
    #[error("proof: {0}")]
    Proof(String),
}

pub type Result<T> = core::result::Result<T, CoreError>;

pub fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

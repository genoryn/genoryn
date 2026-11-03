//! Post-quantum zk-STARK spend proofs (2-in / 2-out shielded transfers).
//!
//! Soundness rests only on collision-resistant hashing (SHA3-256 Merkle commitments,
//! Rescue-Prime inside the statement) and the FRI low-degree test: no elliptic
//! curves, pairings or lattices, so Shor's algorithm does not apply.
//!
//! Zero knowledge: Winterfell does not mask traces, so the prover does it itself:
//! every column carries [`layout::RANDOM_ROWS`] uniformly random rows outside the
//! constrained region (more than the number of evaluations any column opens), and
//! one fully random column masks the DEEP/FRI polynomial. See
//! `docs/spec/spend-circuit.md` for the argument and its review status.

#![forbid(unsafe_code)]
// AIR and trace code indexes columns and rows explicitly; that is clearer than iterators here.
#![allow(clippy::needless_range_loop)]

pub mod air;
pub mod layout;
mod prover;

pub use air::{SpendAir, SpendPublic};
pub use prover::{InputWitness, OutputWitness, SpendWitness, proof_options, prove, verify};

#[derive(Debug, thiserror::Error)]
pub enum ZkError {
    #[error("invalid witness: {0}")]
    Witness(&'static str),
    #[error("proving failed: {0}")]
    Prove(String),
    #[error("invalid proof: {0}")]
    Verify(String),
}

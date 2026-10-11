#![forbid(unsafe_code)]
//! GENORYN shared mining (pay per last N shares).
//!
//! Nothing here changes consensus. A block a pool finds pays its one coinbase
//! output to the pool's own address, exactly like a solo block; the pool then
//! shares the matured reward among the miners whose verified work found it and
//! pays them with ordinary shielded transfers.
//!
//! - [`pplns`]: exact integer reward division over the last N units of work.
//! - [`vardiff`]: per-miner share difficulty; a share is worth its difficulty,
//!   so changing it never changes what equivalent work earns.
//! - [`jobs`]: mining jobs, per-session nonce ranges and share verification
//!   with real RandomX.
//! - [`protocol`]: the line-delimited JSON miner protocol.
//! - [`store`]: the durable ledger (SQLite for one machine, PostgreSQL for
//!   horizontally scaled gateways), with block maturity, reorganizations and
//!   idempotent payouts.
pub mod config;
pub mod jobs;
pub mod pplns;
pub mod protocol;
pub mod store;
pub mod vardiff;

/// Credits are kept in units of 2^-32 gori, so the smallest contribution
/// still counts; only whole gori are ever paid.
pub const FRACTION_BITS: u32 = 32;
pub const ONE_GORI: u128 = 1 << FRACTION_BITS;

/// Whole gori in a fixed-point amount.
pub fn whole_gori(fixed: u128) -> u128 {
    fixed >> FRACTION_BITS
}

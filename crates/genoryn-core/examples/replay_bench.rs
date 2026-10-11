//! Full validation of a real chain, sequential and with parallel
//! pre-verification, on the same blocks; both must reach the same state.
//!
//! `cargo run --release -p genoryn-core --example replay_bench -- blocks.json mainnet`
//! where `blocks.json` holds blocks 1..=n as returned by `v1/blocks`.
use anyhow::{Context, Result, ensure};
use genoryn_core::{
    block::Block,
    chain::unix_time,
    params::Network,
    state::{ChainState, ProofCache, Verification},
};
use serde_json::json;
use std::time::Instant;

fn replay(
    network: Network,
    blocks: &[Block],
    verification: Verification,
    threads: usize,
) -> Result<(ChainState, f64)> {
    let started = Instant::now();
    let mut state = ChainState::new(network);
    let mut cache = ProofCache::default();
    for chunk in blocks.chunks(1_024) {
        cache.preverify(chunk, threads);
        for block in chunk {
            state.apply_block_in_place(block, unix_time(), &mut cache, verification)?;
        }
    }
    Ok((state, started.elapsed().as_secs_f64()))
}

fn main() -> Result<()> {
    let mut args = std::env::args().skip(1);
    let path = args.next().context("blocks file")?;
    let network = Network::parse(&args.next().unwrap_or_else(|| "mainnet".into()))?;
    let blocks: Vec<Block> = serde_json::from_slice(&std::fs::read(&path)?)?;
    ensure!(!blocks.is_empty(), "no blocks");
    let threads = std::thread::available_parallelism()
        .map_or(2, usize::from)
        .min(16);
    let transactions: usize = blocks
        .iter()
        .map(|b| b.body.all_transactions().count())
        .sum();
    // Verifier key setup happens once per process, not per block.
    genoryn_core::privacy::verifying_key();
    let (sequential, sequential_s) = replay(network, &blocks, Verification::Full, 1)?;
    let (parallel, parallel_s) = replay(network, &blocks, Verification::Full, threads)?;
    let (stored, stored_s) = replay(network, &blocks, Verification::Stored, 1)?;
    let started = Instant::now();
    for _ in 0..20 {
        std::hint::black_box(stored.notes_root());
    }
    let notes_root_ms = started.elapsed().as_secs_f64() * 1000.0 / 20.0;
    let started = Instant::now();
    for block in &blocks {
        std::hint::black_box(block.check_shape()?);
    }
    let shape_ms = started.elapsed().as_secs_f64() * 1000.0 / blocks.len() as f64;
    eprintln!("notes_root {notes_root_ms:.3} ms; check_shape {shape_ms:.3} ms per block");
    // A snapshot round trip must restore exactly the same state.
    let started = Instant::now();
    let bytes = genoryn_core::codec::encode(&stored.snapshot())?;
    let restored = ChainState::from_snapshot(genoryn_core::codec::decode(&bytes, usize::MAX)?)?;
    let snapshot_ms = started.elapsed().as_secs_f64() * 1000.0;
    ensure!(
        restored.best_hash == stored.best_hash
            && restored.notes_root() == stored.notes_root()
            && restored.nullifiers_root() == stored.nullifiers_root()
            && restored.next_difficulty() == stored.next_difficulty()
            && restored.next_pow_seed() == stored.next_pow_seed()
            && restored.mature.len() == stored.mature.len(),
        "snapshot does not restore the state"
    );
    eprintln!(
        "snapshot {} bytes, encode+restore {snapshot_ms:.1} ms",
        bytes.len()
    );
    for state in [&parallel, &stored] {
        ensure!(
            state.best_hash == sequential.best_hash
                && state.work == sequential.work
                && state.notes_root() == sequential.notes_root()
                && state.nullifiers_root() == sequential.nullifiers_root(),
            "replays disagree"
        );
    }
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema": 1,
            "benchmark": "full consensus validation of a real chain",
            "network": network.name(),
            "blocks": blocks.len(),
            "transactions": transactions,
            "tip": hex::encode(sequential.best_hash),
            "logical_cpus": std::thread::available_parallelism().map_or(1, usize::from),
            "parallel_threads": threads,
            "full_sequential_seconds": sequential_s,
            "full_parallel_preverify_seconds": parallel_s,
            "speedup": sequential_s / parallel_s,
            "stored_replay_seconds": stored_s,
            "blocks_per_second_sequential": blocks.len() as f64 / sequential_s,
            "blocks_per_second_parallel": blocks.len() as f64 / parallel_s,
            "identical_final_state": true,
        }))?
    );
    Ok(())
}

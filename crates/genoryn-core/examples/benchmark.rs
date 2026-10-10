//! Small, honest local measurements. Run on an idle machine for comparable numbers.
use anyhow::Result;
use genoryn_core::{
    block::Block,
    chain::Chain,
    params::Network,
    pow,
    privacy::{self, TxContext},
};
use orchard::keys::{FullViewingKey, Scope};
use serde_json::json;
use std::time::Instant;

fn main() -> Result<()> {
    let fixture = include_bytes!("../../../tests/vectors/devnet-block-1.bin");
    let block = Block::decode(fixture)?;
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(Block::decode(fixture)?);
    }
    let parse_us = start.elapsed().as_secs_f64() * 1e6 / 1000.0;
    let start = Instant::now();
    privacy::verifying_key();
    let verifier_setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    block.body.miner.verify_crypto()?;
    let verify_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    pow::randomx(block.header.pow_seed, &block.header.bytes()?)?;
    let randomx_cold_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for _ in 0..10 {
        pow::randomx(block.header.pow_seed, &block.header.bytes()?)?;
    }
    let randomx_warm_ms = start.elapsed().as_secs_f64() * 1000.0 / 10.0;
    // The fixture's coinbase pays the public devnet test address, so its viewing key
    // decrypts the output exactly as a wallet scanning its own notes would.
    let coinbase = block.body.miner.bundle.to_bundle()?;
    let ivk = FullViewingKey::from(&privacy::treasury_key(Network::Devnet)).to_ivk(Scope::External);
    anyhow::ensure!(
        coinbase.decrypt_output_with_key(0, &ivk).is_some(),
        "fixture output must be decryptable by the test key"
    );
    let start = Instant::now();
    for _ in 0..1000 {
        std::hint::black_box(coinbase.decrypt_output_with_key(0, &ivk));
    }
    let scan_us = start.elapsed().as_secs_f64() * 1e6 / 1000.0;
    let start = Instant::now();
    privacy::proving_key();
    let prover_setup_ms = start.elapsed().as_secs_f64() * 1000.0;
    let context = TxContext {
        protocol: 1,
        network: Network::Devnet,
        kind: 2,
        height: 1,
        expires: 0,
        nonce: Network::Devnet.genesis(),
    };
    let start = Instant::now();
    let minted = privacy::mint(
        context,
        privacy::treasury_address(Network::Devnet),
        1_000_000_000_000,
        true,
    )?;
    let proof_create_ms = start.elapsed().as_secs_f64() * 1000.0;
    minted.verify_crypto()?;
    let directory = tempfile::tempdir()?;
    let database = directory.path().join("benchmark.sqlite");
    let start = Instant::now();
    let mut chain = Chain::open(&database, Network::Devnet)?;
    let database_initialize_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    let acceptance = chain.accept(block.clone())?;
    anyhow::ensure!(
        acceptance.became_best,
        "real fixture must advance the chain"
    );
    let block_validate_and_persist_ms = start.elapsed().as_secs_f64() * 1000.0;
    let start = Instant::now();
    for _ in 0..1000 {
        anyhow::ensure!(chain.knows(acceptance.hash)?, "persisted fixture missing");
    }
    let database_lookup_us = start.elapsed().as_secs_f64() * 1e6 / 1000.0;
    drop(chain);
    let start = Instant::now();
    let reopened = Chain::open(&database, Network::Devnet)?;
    anyhow::ensure!(
        reopened.state.height == 1 && reopened.state.best_hash == acceptance.hash,
        "replay differs"
    );
    let database_reopen_and_replay_ms = start.elapsed().as_secs_f64() * 1000.0;
    drop(reopened);
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "schema":1,"profile":"development opt-level=1 dependencies=2","os":std::env::consts::OS,"arch":std::env::consts::ARCH,
            "logical_cpus":std::thread::available_parallelism().map(|n|n.get()).unwrap_or(1),
            "fixture_bytes":fixture.len(),"proof_bytes":minted.bundle.proof.len(),
            "canonical_block_parse_us_mean_1000":parse_us,"verifier_key_setup_ms":verifier_setup_ms,
            "one_action_proof_and_signature_verify_ms":verify_ms,"randomx_cold_cache_and_hash_ms":randomx_cold_ms,
            "randomx_warm_hash_ms_mean_10":randomx_warm_ms,"single_note_trial_decryption_us_mean_1000":scan_us,
            "prover_key_setup_ms":prover_setup_ms,"one_action_mint_proof_create_and_sign_ms":proof_create_ms,
            "database_initialize_ms":database_initialize_ms,"real_block_validate_and_durable_write_ms":block_validate_and_persist_ms,
            "indexed_block_lookup_us_mean_1000":database_lookup_us,"one_block_database_reopen_and_full_replay_ms":database_reopen_and_replay_ms,
            "database_scope":"One actual committed proof-of-work/proof fixture; warm process crypto keys/cache. Not long-history capacity.",
            "warning":"Single local run; includes scheduler noise. No hardware or throughput guarantee."
        }))?
    );
    Ok(())
}

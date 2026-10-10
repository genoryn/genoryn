use genoryn_core::{
    block::Block,
    chain::Chain,
    codec,
    params::Network,
    pow, privacy,
    state::{ChainState, ProofCache},
};

/// Writes a freshly mined devnet block 1 to tests/vectors when
/// GENORYN_WRITE_VECTORS=1. Run only when consensus rules change on purpose.
#[test]
#[ignore = "regenerates the committed vector; explicit maintenance only"]
fn regenerate_devnet_block_vector() {
    if std::env::var("GENORYN_WRITE_VECTORS").as_deref() != Ok("1") {
        return;
    }
    let dir = tempfile::tempdir().unwrap();
    let mut chain = Chain::open(&dir.path().join("chain.sqlite"), Network::Devnet).unwrap();
    let mut block = chain
        .template(privacy::treasury_address(Network::Devnet))
        .unwrap();
    while !pow::meets_difficulty(
        pow::randomx(block.header.pow_seed, &block.header.bytes().unwrap()).unwrap(),
        block.header.difficulty,
    ) {
        block.header.nonce += 1;
    }
    assert!(chain.accept(block.clone()).unwrap().became_best);
    let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../tests/vectors");
    std::fs::write(root.join("devnet-block-1.bin"), block.bytes().unwrap()).unwrap();
    std::fs::write(
        root.join("devnet-block-1.sha256"),
        format!("{}\n", hex::encode(block.hash().unwrap())),
    )
    .unwrap();
}

/// A real block of the retired pre-1.0 mainnet (height 494, with one private
/// transfer), taken from that chain's public block store.
const RETIRED_MAINNET_BLOCK: &[u8] =
    include_bytes!("../../../tests/vectors/retired-mainnet-block-494.bin");

#[test]
fn retired_mainnet_blocks_and_transactions_are_rejected() {
    let block = Block::decode(RETIRED_MAINNET_BLOCK).unwrap();
    assert_eq!(block.header.height, 494);
    assert_eq!(
        block.body.transactions.len(),
        1,
        "fixture carries a transfer"
    );
    // Its ancestry does not lead to the 1.0 genesis.
    let state = ChainState::new(Network::Mainnet);
    let mut cache = ProofCache::default();
    assert!(
        state
            .apply_block(&block, block.header.timestamp, &mut cache)
            .is_err()
    );
    // Replay: every transaction's signatures commit to the retired genesis, so
    // none verifies under 1.0 even before anchors or nullifiers are checked.
    for tx in block.body.all_transactions() {
        assert_eq!(tx.context.network, Network::Mainnet);
        let error = tx.verify_crypto().unwrap_err();
        assert!(!error.to_string().is_empty());
    }
}

#[test]
#[ignore = "real Halo 2 verification of the retired transfer under 1.0 rules"]
fn a_retired_transfer_cannot_enter_a_1_0_mempool() {
    let block = Block::decode(RETIRED_MAINNET_BLOCK).unwrap();
    let dir = tempfile::tempdir().unwrap();
    let mut chain = Chain::open(&dir.path().join("chain.sqlite"), Network::Mainnet).unwrap();
    let transfer = block.body.transactions[0].clone();
    assert!(chain.submit(transfer).is_err());
}

#[test]
fn block_one_pays_the_early_subsidy_and_no_treasury() {
    let block = Block::decode(include_bytes!("../../../tests/vectors/devnet-block-1.bin")).unwrap();
    assert!(block.body.treasury.is_none(), "the treasury starts at 501");
    assert_eq!(
        block.body.miner.bundle.value_balance,
        -(genoryn_core::emission::EARLY_REWARD as i64),
        "the miner issues exactly 1,000 GNR"
    );
}

#[test]
fn committed_block_roundtrips_canonically() {
    let bytes = include_bytes!("../../../tests/vectors/devnet-block-1.bin");
    let block = Block::decode(bytes).unwrap();
    assert_eq!(block.bytes().unwrap(), bytes);
    assert_eq!(
        hex::encode(block.hash().unwrap()),
        include_str!("../../../tests/vectors/devnet-block-1.sha256").trim()
    );
    block.check_shape().unwrap();
    let mut trailing = bytes.to_vec();
    trailing.push(0);
    assert!(Block::decode(&trailing).is_err());
    let mut changed = block;
    changed.header.body_hash[0] ^= 1;
    assert!(changed.check_shape().is_err());
}

#[test]
#[ignore = "genuine shielded block proof + PoW validation; explicit full suite"]
fn committed_block_validates_real_proofs_and_rejects_modified_state() {
    let block = Block::decode(include_bytes!("../../../tests/vectors/devnet-block-1.bin")).unwrap();
    let state = ChainState::new(Network::Devnet);
    let mut cache = ProofCache::default();
    let next = state
        .apply_block(&block, block.header.timestamp, &mut cache)
        .unwrap();
    assert_eq!(next.height, 1);
    let mut wrong = block.clone();
    wrong.header.notes_root[0] ^= 1;
    assert!(
        state
            .apply_block(&wrong, block.header.timestamp, &mut cache)
            .is_err()
    );
    let mut wrong = block.clone();
    wrong.body.miner.bundle.value_balance -= 1;
    wrong.header.body_hash = wrong.body.hash().unwrap();
    assert!(
        state
            .apply_block(&wrong, block.header.timestamp, &mut cache)
            .is_err()
    );
    let mut wrong = block.clone();
    wrong.header.network = Network::Testnet;
    assert!(
        state
            .apply_block(&wrong, block.header.timestamp, &mut cache)
            .is_err()
    );
    assert!(codec::decode::<Block>(&vec![255; 2_000_001], 2_000_000).is_err());
}

use genoryn_core::{
    block::Block,
    codec,
    params::Network,
    state::{ChainState, ProofCache},
};

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

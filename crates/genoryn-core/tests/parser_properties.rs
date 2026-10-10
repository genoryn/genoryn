use genoryn_core::{
    block::Block,
    codec::{self, MAX_BLOCK_BYTES, MAX_TX_BYTES},
    privacy::ShieldedTx,
};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(512))]
    #[test]
    fn arbitrary_network_bytes_never_panic(bytes in proptest::collection::vec(any::<u8>(),0..8192)) {
        let _=Block::decode(&bytes);
        let _:Result<ShieldedTx,_>=codec::decode(&bytes,MAX_TX_BYTES);
    }
    #[test]
    fn canonical_valid_fixture_mutations_are_bounded(index in 0usize..12_375, byte in any::<u8>()) {
        let mut bytes=include_bytes!("../../../tests/vectors/devnet-block-1.bin").to_vec();
        let index=index%bytes.len();bytes[index]=byte;
        if let Ok(block)=codec::decode::<Block>(&bytes,MAX_BLOCK_BYTES) {prop_assert_eq!(block.bytes().unwrap(),bytes);}
    }
}

#[test]
fn oversized_input_and_attacker_lengths_fail_closed() {
    assert!(Block::decode(&vec![0; MAX_BLOCK_BYTES + 1]).is_err());
    for n in [u64::MAX, u64::MAX - 1, 1 << 48, 1 << 32] {
        let mut bytes = n.to_le_bytes().to_vec();
        bytes.extend([0; 64]);
        assert!(codec::decode::<Vec<ShieldedTx>>(&bytes, MAX_TX_BYTES).is_err());
    }
}

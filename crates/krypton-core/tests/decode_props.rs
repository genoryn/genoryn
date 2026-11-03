//! Decoders of untrusted data must never panic and must reject trailing bytes.
use krypton_core::address::Address;
use krypton_core::types::{Block, BlockHeader, Transaction};
use proptest::prelude::*;

proptest! {
    #![proptest_config(ProptestConfig::with_cases(2000))]
    #[test]
    fn block_decoder_never_panics(b in proptest::collection::vec(any::<u8>(), 0..4096)) {
        let _ = Block::from_bytes(&b);
    }
    #[test]
    fn tx_decoder_never_panics(b in proptest::collection::vec(any::<u8>(), 0..8192)) {
        let _ = Transaction::from_bytes(&b);
    }
    #[test]
    fn header_decoder_never_panics(b in proptest::collection::vec(any::<u8>(), 0..400)) {
        if let Ok(h) = BlockHeader::from_bytes(&b) {
            prop_assert_eq!(h.to_bytes(), b);
        }
    }
    #[test]
    fn address_decoder_never_panics(s in "\\PC{0,3000}") {
        let _ = Address::decode(&s);
        let _ = Address::parse_short(&s);
    }
}

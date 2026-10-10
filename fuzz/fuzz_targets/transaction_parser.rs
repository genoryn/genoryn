#![no_main]
libfuzzer_sys::fuzz_target!(|data:&[u8]| {
    if let Ok(tx)=genoryn_core::codec::decode::<genoryn_core::privacy::ShieldedTx>(data,genoryn_core::codec::MAX_TX_BYTES){let _=tx.bundle.to_bundle();}
});

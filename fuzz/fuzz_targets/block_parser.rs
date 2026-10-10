#![no_main]
libfuzzer_sys::fuzz_target!(|data:&[u8]| {let _=genoryn_core::block::Block::decode(data);});

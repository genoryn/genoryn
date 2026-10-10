#![no_main]
libfuzzer_sys::fuzz_target!(|data:&[u8]| {
    if let Ok(text)=std::str::from_utf8(data){let _=genoryn_wallet_core::username::canonicalize(text);}
    let _:Result<genoryn_wallet_core::username::ReceiverBundle,_>=serde_json::from_slice(data);
});

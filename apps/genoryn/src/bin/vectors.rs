use anyhow::Result;
use genoryn_core::{block::Block, codec, params::Network, pow};
use serde_json::json;
fn main() -> Result<()> {
    let block = Block::decode(include_bytes!(
        "../../../../tests/vectors/devnet-block-1.bin"
    ))?;
    let pow_hash = pow::randomx(block.header.pow_seed, &block.header.bytes()?)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "protocol":1,"devnet_genesis":hex::encode(Network::Devnet.genesis()),
            "testnet_genesis":hex::encode(Network::Testnet.genesis()),
            "header_bytes":hex::encode(block.header.bytes()?),
            "block_hash":hex::encode(block.hash()?),"randomx_hash":hex::encode(pow_hash),
            "canonical_u16_u32":hex::encode(codec::encode(&(1u16,0x01020304u32))?),
            "notes_root":hex::encode(block.header.notes_root),
            "nullifiers_root":hex::encode(block.header.nullifiers_root),
            "body_hash":hex::encode(block.body.hash()?)
        }))?
    );
    Ok(())
}

//! Explicitly configured discovery smoke test; prints only public network data.
use anyhow::{Result, ensure};
use genoryn_connect::{Connect, ConnectConfig};

#[tokio::main]
async fn main() -> Result<()> {
    let args: Vec<_> = std::env::args().skip(1).collect();
    ensure!(
        args.len() >= 3,
        "usage: probe EXPECTED_CHAIN_ID EXPECTED_GENESIS_HASH RPC_ORIGIN [RPC_ORIGIN ...]"
    );
    let mut client = Connect::new(ConnectConfig::devnet(
        &args[0],
        &args[1],
        args[2..].to_vec(),
    ))?;
    let selected = client.select().await?;
    println!("{}", serde_json::to_string_pretty(client.last_report())?);
    println!("selected endpoint: {}", selected.endpoint);
    let supply: serde_json::Value = client.get_json("/v1/supply").await?;
    println!("{}", serde_json::to_string_pretty(&supply)?);
    Ok(())
}

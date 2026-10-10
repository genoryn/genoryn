use anyhow::Result;
use clap::{Parser, Subcommand};
use serde_json::Value;
#[derive(Parser)]
struct Args {
    #[arg(long, default_value = "http://127.0.0.1:29611")]
    node: String,
    #[command(subcommand)]
    command: Command,
}
#[derive(Subcommand)]
enum Command {
    #[command(name = "get-network-info")]
    NetworkInfo,
    #[command(name = "get-genesis")]
    Genesis,
    #[command(name = "get-supply")]
    Supply,
    #[command(name = "get-peers")]
    Peers,
    /// Network privacy mode, Tor status and onion service (loopback admin RPC).
    #[command(name = "get-network-privacy")]
    NetworkPrivacy,
    /// Re-check Tor now and retry peers immediately (needs the admin token).
    #[command(name = "reconnect-network")]
    ReconnectNetwork {
        #[arg(long)]
        token_file: std::path::PathBuf,
    },
    #[command(name = "get-blocks")]
    Blocks {
        #[arg(long, default_value_t = 1)]
        from: u64,
    },
    VerifyRelease {
        #[arg(long)]
        manifest: std::path::PathBuf,
        #[arg(long)]
        signature: std::path::PathBuf,
        #[arg(long)]
        public_key: String,
        #[arg(long)]
        directory: std::path::PathBuf,
    },
}
#[tokio::main]
async fn main() -> Result<()> {
    let a = Args::parse();
    if let Command::VerifyRelease {
        manifest,
        signature,
        public_key,
        directory,
    } = &a.command
    {
        anyhow::ensure!(
            std::fs::metadata(manifest)?.len() <= 2_000_000,
            "release manifest too large"
        );
        anyhow::ensure!(
            std::fs::metadata(signature)?.len() <= 256,
            "signature file too large"
        );
        let key: [u8; 32] = hex::decode(public_key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("public key must be 32 bytes"))?;
        let sig = hex::decode(std::fs::read_to_string(signature)?.trim())?;
        let manifest =
            genoryn_wallet_core::release::verify_metadata(&std::fs::read(manifest)?, &sig, key)?;
        genoryn_wallet_core::release::verify_files(&manifest, directory)?;
        println!(
            "{}",
            serde_json::json!({"verified":true,"version":manifest.version,"files":manifest.files.len(),"installed":false})
        );
        return Ok(());
    }
    if let Command::ReconnectNetwork { token_file } = &a.command {
        let client = genoryn::client::RpcClient::new(&a.node, Some(token_file))?;
        let value: Value = client
            .post("v1/network/reconnect", &serde_json::json!({}))
            .await?;
        println!("{}", serde_json::to_string_pretty(&value)?);
        return Ok(());
    }
    let client = genoryn::client::RpcClient::new(&a.node, None)?;
    let path = match a.command {
        Command::NetworkInfo => "v1/info".into(),
        Command::Genesis => "v1/genesis".into(),
        Command::Supply => "v1/supply".into(),
        Command::Peers => "v1/peers".into(),
        Command::NetworkPrivacy => "v1/network".into(),
        Command::Blocks { from } => format!("v1/blocks?from={from}&limit=8"),
        Command::VerifyRelease { .. } | Command::ReconnectNetwork { .. } => {
            unreachable!("handled before creating the read-only RPC client")
        }
    };
    let value: Value = client.get(&path).await?;
    println!("{}", serde_json::to_string_pretty(&value)?);
    Ok(())
}

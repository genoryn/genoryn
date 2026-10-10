//! Standalone diagnostic for the built-in Tor client, independent of the node.
//! Usage: `cargo run -p genoryn --example arti_probe -- <state-dir> [seconds]`
//! with `RUST_LOG=info,tor_dirmgr=debug,tor_dirclient=debug` for detail.
//! Arti redacts sensitive values in its logs by default. Bootstraps, then opens
//! one exit stream to example.com:80; exits non-zero if either step fails.
use anyhow::{Context, Result, bail};
use arti_client::{TorClient, config::TorClientConfigBuilder};
use futures::StreamExt;
use std::time::{Duration, Instant};

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_writer(std::io::stderr)
        .init();
    let mut args = std::env::args().skip(1);
    let directory = std::path::PathBuf::from(args.next().unwrap_or_else(|| "arti-probe".into()));
    let seconds: u64 = args.next().map(|s| s.parse()).transpose()?.unwrap_or(300);
    let config =
        TorClientConfigBuilder::from_directories(directory.join("state"), directory.join("cache"))
            .build()
            .context("configuration")?;
    let client = TorClient::builder()
        .config(config)
        .create_unbootstrapped()
        .context("client creation")?;
    let started = Instant::now();
    let mut events = client.bootstrap_events();
    tokio::spawn(async move {
        let mut last = String::new();
        while let Some(status) = events.next().await {
            let line = format!("{status}");
            if line != last {
                println!("[{:>6.1}s] {line}", started.elapsed().as_secs_f64());
                last = line;
            }
        }
    });
    match tokio::time::timeout(Duration::from_secs(seconds), client.bootstrap()).await {
        Ok(Ok(())) => println!("BOOTSTRAPPED after {:.1}s", started.elapsed().as_secs_f64()),
        Ok(Err(error)) => bail!(
            "bootstrap failed after {:.1}s: {error}",
            started.elapsed().as_secs_f64()
        ),
        Err(_) => bail!(
            "bootstrap timeout after {seconds}s: {}",
            client.bootstrap_status()
        ),
    }
    let connected = Instant::now();
    match tokio::time::timeout(Duration::from_secs(60), client.connect(("example.com", 80))).await {
        Ok(Ok(_)) => println!(
            "EXIT STREAM OK in {:.1}s",
            connected.elapsed().as_secs_f64()
        ),
        Ok(Err(error)) => bail!("exit stream failed: {error}"),
        Err(_) => bail!("exit stream timeout"),
    }
    Ok(())
}

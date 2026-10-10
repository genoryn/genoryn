//! Measured registered-identity dataset; no network, funds or production secrets.
use anyhow::{Context, Result, ensure};
use clap::Parser;
use genoryn_core::{chain::unix_time, params::Network};
use genoryn_wallet_core::{
    keys::Wallet,
    registry_store::{MAX_REGISTERED_IDENTITIES, RegistryStore},
};
use serde_json::json;
use std::{
    collections::BTreeSet,
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    time::Instant,
};

#[derive(Parser)]
#[command(about = "Generate and validate bounded valueless local identity-capacity fixtures")]
struct Args {
    #[arg(long, default_value_t = 50_000)]
    count: usize,
    #[arg(long)]
    database: PathBuf,
    #[arg(long, default_value_t = 4)]
    workers: usize,
}
fn percentile(values: &mut [u128], percent: usize) -> u128 {
    values.sort_unstable();
    values[(values.len() - 1) * percent / 100]
}
fn main() -> Result<()> {
    let args = Args::parse();
    ensure!(
        args.count > 0 && args.count <= MAX_REGISTERED_IDENTITIES as usize,
        "count exceeds bounded registry capacity"
    );
    ensure!((1..=4).contains(&args.workers), "choose 1-4 workers");
    ensure!(!args.database.exists(), "benchmark database must be new");
    let mut store = RegistryStore::open(&args.database, Network::Devnet, None)?;
    let count = args.count;
    let next = Arc::new(AtomicUsize::new(0));
    let now = unix_time();
    let (send, receive) = mpsc::sync_channel(64);
    let started = Instant::now();
    let mut handles = Vec::new();
    for _ in 0..args.workers {
        let send = send.clone();
        let next = next.clone();
        handles.push(std::thread::spawn(move || -> Result<()> {
            loop {
                let i = next.fetch_add(1, Ordering::Relaxed);
                if i >= count {
                    break;
                }
                let wallet = Wallet::create(Network::Devnet)?;
                let bundle = wallet.receiver_bundle(&format!("capacity_{i:06}"), 0, now)?;
                send.send((i, bundle, if i < 10 { Some(wallet) } else { None }))
                    .map_err(|_| anyhow::anyhow!("benchmark channel closed"))?;
            }
            Ok(())
        }));
    }
    drop(send);
    let mut owners = BTreeSet::new();
    let mut receivers = BTreeSet::new();
    let mut retained = Vec::new();
    let mut completed = 0;
    for (i, bundle, wallet) in receive {
        ensure!(owners.insert(bundle.owner_key), "duplicate owner fixture");
        ensure!(
            receivers.insert(bundle.receiver.clone()),
            "duplicate receiving identity fixture"
        );
        store.register(bundle, Network::Devnet, now)?;
        if let Some(w) = wallet {
            retained.push((i, w));
        }
        completed += 1;
        if completed % 10_000 == 0 {
            eprintln!("validated {completed} signed registered identities");
        }
    }
    for handle in handles {
        handle
            .join()
            .map_err(|_| anyhow::anyhow!("identity worker panicked"))??;
    }
    ensure!(
        completed == count && store.len()? == count as u64,
        "registry count mismatch"
    );
    let registration_seconds = started.elapsed().as_secs_f64();
    store.checkpoint()?;
    drop(store);
    let opened = Instant::now();
    let mut store = RegistryStore::open(&args.database, Network::Devnet, None)?;
    let reopen_ms = opened.elapsed().as_secs_f64() * 1000.0;
    ensure!(store.len()? == count as u64, "persistent count mismatch");
    let mut lookup = Vec::new();
    for sample in 0..1000usize.min(count) {
        let i = sample * count / (1000usize.min(count));
        let name = format!("capacity_{i:06}");
        let t = Instant::now();
        let bundle = store.resolve(&name, Network::Devnet, unix_time())?;
        lookup.push(t.elapsed().as_micros());
        ensure!(bundle.username == name, "indexed lookup mismatch");
    }
    let p50 = percentile(&mut lookup, 50);
    let p95 = percentile(&mut lookup, 95);
    let p99 = percentile(&mut lookup, 99);
    for (i, wallet) in retained {
        let name = format!("capacity_{i:06}");
        store.register(
            wallet.receiver_bundle(&name, 1, unix_time())?,
            Network::Devnet,
            unix_time(),
        )?;
        ensure!(
            store.resolve(&name, Network::Devnet, unix_time())?.sequence == 1,
            "renewal failed"
        );
    }
    ensure!(store.len()? == count as u64, "renewal changed record count");
    store.checkpoint()?;
    let bytes = std::fs::metadata(&args.database)
        .context("benchmark database missing")?
        .len();
    println!(
        "{}",
        serde_json::to_string_pretty(
            &json!({"schema":1,"scope":"distinct local wallet receiving identities and signed username owners; not concurrent wallets, payments or chain TPS","registered_identities":count,"distinct_receivers":receivers.len(),"distinct_owners":owners.len(),"generation_and_durable_registration_seconds":registration_seconds,"registrations_per_second":count as f64/registration_seconds,"database_bytes":bytes,"reopen_integrity_check_ms":reopen_ms,"verified_indexed_lookup_p50_us":p50,"verified_indexed_lookup_p95_us":p95,"verified_indexed_lookup_p99_us":p99,"renewals_verified":count.min(10),"persistent_record_count":store.len()?,"passed":true})
        )?
    );
    Ok(())
}

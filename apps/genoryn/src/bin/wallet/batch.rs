//! Many wallets in one pass: `sync_batch` derives every wallet's keys in
//! parallel, reads the chain once (new blocks from the user's own node, or the
//! locally verified chain in Quick mode) and advances every wallet's encrypted
//! scan cache from it in parallel. A wrong password or damaged file is reported
//! for that wallet only; an unreachable chain source leaves every wallet at its
//! last verified state, marked stale. Built for up to 10,000 wallets, which the
//! GUI sends in chunks.
use super::*;
use std::sync::{
    Mutex,
    atomic::{AtomicUsize, Ordering},
};

/// Wallets one request may carry.
pub const MAX_BATCH_WALLETS: usize = 10_000;
/// Key derivation uses 64 MiB per wallet; this bounds memory and keeps the
/// computer responsive.
const MAX_WORKERS: usize = 12;
/// Blocks fetched below the lowest cached height, so a short reorganization
/// rolls caches back instead of rescanning them from genesis.
const REORG_LOOKBACK: u64 = 16;

/// Validated blocks `start..=start + len - 1`.
struct BlockRange {
    start: u64,
    blocks: Vec<Block>,
}
impl BlockRange {
    fn get(&self, height: u64) -> Option<&Block> {
        height
            .checked_sub(self.start)
            .and_then(|index| self.blocks.get(index as usize))
    }
    fn tip(&self) -> u64 {
        self.start + self.blocks.len() as u64 - 1
    }
}

/// Runs `work(index)` for `0..count` on a bounded set of threads.
fn parallel<T: Send>(count: usize, work: impl Fn(usize) -> T + Sync) -> Vec<T> {
    let workers = std::thread::available_parallelism()
        .map_or(4, usize::from)
        .clamp(1, MAX_WORKERS)
        .min(count.max(1));
    let next = AtomicUsize::new(0);
    let results: Mutex<Vec<Option<T>>> = Mutex::new((0..count).map(|_| None).collect());
    std::thread::scope(|scope| {
        for _ in 0..workers {
            scope.spawn(|| {
                loop {
                    let index = next.fetch_add(1, Ordering::Relaxed);
                    if index >= count {
                        break;
                    }
                    let value = work(index);
                    results.lock().expect("results lock")[index] = Some(value);
                }
            });
        }
    });
    results
        .into_inner()
        .expect("results lock")
        .into_iter()
        .map(|value| value.expect("every index processed"))
        .collect()
}

/// Brings `cache` to the tip of `range`. `false` when the range does not reach
/// back to where this cache shares the chain (it then needs blocks from genesis).
fn advance_from_range(
    cache: &mut sync::ScanCache,
    keys: &sync::ScanKeys,
    range: &BlockRange,
) -> Result<bool> {
    if cache.height > 0 {
        let shared = cache
            .recent
            .iter()
            .rev()
            .find(|(height, hash)| range.get(*height).and_then(|b| b.hash().ok()) == Some(*hash))
            .map(|(height, _)| *height);
        match shared {
            Some(height) if height == cache.height => {}
            Some(height) if cache.can_roll_back_to(height) => cache.rollback_to(height),
            _ => return Ok(false),
        }
    }
    if cache.height + 1 < range.start {
        return Ok(false);
    }
    for height in cache.height + 1..=range.tip() {
        let block = range.get(height).context("block range has a gap")?;
        cache.apply_block(keys, block)?;
    }
    Ok(true)
}

/// Blocks `from..=to` from the node, validated only by the node itself (the
/// user's own loopback node) — the same trust as a single wallet's sync.
async fn node_range(rpc: &RpcClient, from: u64, to: u64) -> Result<BlockRange> {
    let mut blocks = Vec::with_capacity(to.saturating_sub(from) as usize + 1);
    for height in from..=to {
        blocks.push(block_at(rpc, height).await?);
    }
    Ok(BlockRange {
        start: from,
        blocks,
    })
}

struct Opened {
    path: String,
    wallet: Wallet,
    cache: sync::ScanCache,
}

pub async fn sync_batch(request: &Value, network: Network) -> Result<Value> {
    let entries = request["wallets"].as_array().context("wallets required")?;
    ensure!(
        (1..=MAX_BATCH_WALLETS).contains(&entries.len()),
        "a batch carries 1 to {MAX_BATCH_WALLETS} wallets"
    );
    let items: Vec<(String, Zeroizing<String>)> = entries
        .iter()
        .map(|entry| {
            (
                entry["wallet_path"].as_str().unwrap_or_default().to_owned(),
                Zeroizing::new(entry["password"].as_str().unwrap_or_default().to_owned()),
            )
        })
        .collect();
    // Key derivation dominates; every wallet opens in parallel.
    let opened: Vec<Result<Opened, String>> = parallel(items.len(), |index| {
        let (path, password) = &items[index];
        let result = (|| -> Result<Opened> {
            let wallet = Wallet::open(Path::new(path), password)?;
            ensure!(wallet.network == network, "wallet network mismatch");
            let cache = sync::load_cache(&wallet, &sync::cache_path(Path::new(path)));
            Ok(Opened {
                path: path.clone(),
                wallet,
                cache,
            })
        })();
        result.map_err(|error| format!("{error:#}").chars().take(200).collect())
    });
    drop(items);

    let light = request["light"].as_bool() == Some(true);
    let rpc_url = request["rpc_url"]
        .as_str()
        .unwrap_or("http://127.0.0.1:29711")
        .to_owned();
    let mut stale = false;
    let mut chain_error: Option<String> = None;
    let mut light_clients: Option<(genoryn::light::LightTor, Vec<RpcClient>, usize)> = None;
    let rpc = if light {
        None
    } else {
        Some(RpcClient::new(&rpc_url, None)?)
    };
    let min_height = opened
        .iter()
        .filter_map(|wallet| wallet.as_ref().ok())
        .map(|wallet| wallet.cache.height)
        .min();
    // The chain is read once for the whole batch.
    let range: Option<BlockRange> = match min_height {
        None => None,
        Some(_) if light => match light_chain(request, network).await {
            Ok((blocks, is_stale, tor, clients, synced_from)) => {
                stale = is_stale;
                light_clients = Some((tor, clients, synced_from));
                (!blocks.is_empty()).then_some(BlockRange { start: 1, blocks })
            }
            Err(error) => {
                stale = true;
                chain_error = Some(format!("{error:#}"));
                None
            }
        },
        Some(min_height) => {
            let rpc = rpc.as_ref().expect("node client");
            let own_node =
                request["full_validation"].as_bool() != Some(true) && is_loopback_url(&rpc_url);
            let attempt = async {
                let info: Value = rpc.get("v1/info").await?;
                ensure!(
                    info["chain_id"] == network.chain_id()
                        && info["genesis_hash"] == hex::encode(network.genesis()),
                    "RPC network/genesis mismatch"
                );
                let tip = info["height"].as_u64().context("RPC height missing")?;
                if tip == 0 {
                    return Ok(None);
                }
                if own_node {
                    let from = min_height.saturating_sub(REORG_LOOKBACK).clamp(1, tip);
                    node_range(rpc, from, tip).await.map(Some)
                } else {
                    // A remote node's blocks are validated here in full.
                    let blocks = rpc.chain(network).await?;
                    genoryn_wallet_core::scan::replay_chain(
                        network,
                        &blocks,
                        genoryn_core::state::Verification::Full,
                    )?;
                    Ok(Some(BlockRange { start: 1, blocks }))
                }
            };
            match attempt.await {
                Ok(range) => range,
                Err(error) => {
                    stale = true;
                    chain_error = Some(format!("{error:#}"));
                    None
                }
            }
        }
    };

    // Every wallet advances in parallel from the same blocks.
    let advance = |opened: &mut Opened, range: &BlockRange| -> Result<bool> {
        let keys = sync::ScanKeys::new(&opened.wallet)?;
        advance_from_range(&mut opened.cache, &keys, range)
    };
    let mut opened = opened;
    let mut needs_genesis = Vec::new();
    if let Some(range) = &range {
        let cells: Vec<Mutex<Option<Result<Opened, String>>>> = opened
            .drain(..)
            .map(|item| Mutex::new(Some(item)))
            .collect();
        let outcomes = parallel(cells.len(), |index| {
            let mut slot = cells[index].lock().expect("cell lock");
            match slot.as_mut().expect("wallet present") {
                Ok(wallet) => match advance(wallet, range) {
                    Ok(true) => Ok(()),
                    Ok(false) => Err(None),
                    Err(error) => Err(Some(format!("{error:#}"))),
                },
                Err(_) => Ok(()),
            }
        });
        opened = cells
            .into_iter()
            .map(|cell| {
                cell.into_inner()
                    .expect("cell lock")
                    .expect("wallet present")
            })
            .collect();
        for (index, outcome) in outcomes.into_iter().enumerate() {
            match outcome {
                Ok(()) => {}
                Err(None) => needs_genesis.push(index),
                Err(Some(error)) => opened[index] = Err(error.chars().take(200).collect()),
            }
        }
    }
    // Wallets whose fork point lies before the fetched range rescan from genesis.
    if !needs_genesis.is_empty()
        && let (Some(rpc), Some(range)) = (rpc.as_ref(), range.as_ref())
        && range.start > 1
    {
        match node_range(rpc, 1, range.tip()).await {
            Ok(full) => {
                for index in needs_genesis.drain(..) {
                    if let Ok(wallet) = &mut opened[index] {
                        wallet.cache = wallet.cache.restart();
                        if let Err(error) = advance(wallet, &full) {
                            opened[index] = Err(format!("{error:#}"));
                        }
                    }
                }
            }
            Err(error) => {
                stale = true;
                chain_error.get_or_insert(format!("{error:#}"));
            }
        }
    }
    for index in needs_genesis {
        if let Ok(wallet) = &mut opened[index] {
            // Quick mode always holds the chain from genesis; this is a reset.
            if let Some(range) = &range {
                wallet.cache = wallet.cache.restart();
                if let Err(error) = advance(wallet, range) {
                    opened[index] = Err(format!("{error:#}"));
                }
            }
        }
    }

    // Persist caches, settle outboxes and collect the summaries.
    let mut summaries = Vec::with_capacity(opened.len());
    let mut due = Vec::new();
    for (index, entry) in opened.iter().enumerate() {
        let path = entries[index]["wallet_path"].as_str().unwrap_or_default();
        match entry {
            Err(error) => summaries.push(json!({"wallet_path": path, "ok": false, "error": error})),
            Ok(wallet) => {
                let file = Path::new(&wallet.path);
                let result = (|| -> Result<Value> {
                    if range.is_some() {
                        sync::save_cache(&wallet.wallet, &sync::cache_path(file), &wallet.cache)?;
                    }
                    let outbox_file = outbox::outbox_path(file);
                    let mut outbox = outbox::load(&wallet.wallet, &outbox_file)?;
                    if outbox.reconcile(&wallet.cache) {
                        outbox::save(&wallet.wallet, &outbox_file, &outbox)?;
                    }
                    if !stale && !outbox.to_relay(unix_time()).is_empty() {
                        due.push(index);
                    }
                    let mut summary =
                        wallet_summary(&wallet.wallet, &wallet.cache, &outbox, request)?;
                    let object = summary.as_object_mut().context("summary object")?;
                    let mined = object
                        .get("mined")
                        .and_then(Value::as_array)
                        .map_or(0, |rewards| rewards.len());
                    let open = outbox.entries.iter().filter(|e| e.status.is_open()).count();
                    for key in ["history", "mined", "outbox", "name_owner", "name_receiver"] {
                        object.remove(key);
                    }
                    object.insert("wallet_path".into(), json!(wallet.path));
                    object.insert("ok".into(), json!(true));
                    object.insert("mined_count".into(), json!(mined));
                    object.insert("outbox_open".into(), json!(open));
                    Ok(summary)
                })();
                summaries.push(result.unwrap_or_else(|error| {
                    json!({"wallet_path": path, "ok": false, "error": format!("{error:#}")})
                }));
            }
        }
    }
    // Pending payments of these wallets are relayed again with their stored bytes.
    for index in due {
        let Ok(wallet) = &opened[index] else { continue };
        let file = outbox::outbox_path(Path::new(&wallet.path));
        let Ok(mut outbox) = outbox::load(&wallet.wallet, &file) else {
            continue;
        };
        match (&rpc, &light_clients) {
            (Some(rpc), _) => relay_due(&mut outbox, &Relayer::Node(rpc)).await,
            (None, Some((_, clients, synced_from))) => {
                relay_due(&mut outbox, &Relayer::Light(clients, *synced_from)).await
            }
            _ => continue,
        }
        let _ = outbox::save(&wallet.wallet, &file, &outbox);
    }
    Ok(json!({
        "wallets": summaries,
        "height": range.as_ref().map(BlockRange::tip),
        "stale": stale,
        "chain_error": chain_error,
        "mode": if light { "quick" } else { "full" },
    }))
}

/// Quick mode: the chain once, verified on this computer and kept in the
/// verified chain of the wallet given at the top level of the request (the
/// active wallet), so every chunk reuses it instead of downloading again.
async fn light_chain(
    request: &Value,
    network: Network,
) -> Result<(
    Vec<Block>,
    bool,
    genoryn::light::LightTor,
    Vec<RpcClient>,
    usize,
)> {
    let path = Path::new(field(request, "wallet_path")?);
    let password = Zeroizing::new(field(request, "password")?.to_owned());
    let primary = Wallet::open(path, &password)?;
    ensure!(primary.network == network, "wallet network mismatch");
    let sources: Vec<String> = request["sources"]
        .as_array()
        .context("Quick mode needs remote nodes")?
        .iter()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect();
    ensure!(
        (1..=8).contains(&sources.len()),
        "Quick mode needs one to eight remote nodes"
    );
    let external = request["tor_socks"]
        .as_str()
        .filter(|socks| !socks.is_empty())
        .map(str::parse::<std::net::SocketAddr>)
        .transpose()
        .context("tor_socks must be an IP address and port")?;
    let tor_dir = request["tor_dir"]
        .as_str()
        .map(PathBuf::from)
        .unwrap_or_else(|| path.with_extension("tor"));
    let store = chain_store::chain_path(path);
    let mut chain = chain_store::load(&primary, &store);
    let local_state = chain.replay(network);
    let tor =
        genoryn::light::LightTor::start(&tor_dir, external, std::time::Duration::from_secs(300))
            .await?;
    let clients = sources
        .iter()
        .map(|url| RpcClient::through_tor(url, &tor.proxy()))
        .collect::<Result<Vec<_>>>()?;
    let verified = genoryn::light::sync_verified(
        network,
        &clients,
        &sources,
        std::mem::take(&mut chain.blocks),
        local_state,
    )
    .await?;
    let stale = verified.is_stale();
    let synced_from = verified.used.unwrap_or_else(|| {
        verified
            .reports
            .iter()
            .position(|report| report.height.is_some())
            .unwrap_or(0)
    });
    chain.blocks = verified.blocks;
    if verified.used.is_some() {
        chain_store::save(&primary, &store, &chain)?;
    }
    Ok((chain.blocks, stale, tor, clients, synced_from))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parallel_work_keeps_order_and_visits_every_index() {
        let squares = parallel(1000, |index| index * index);
        assert_eq!(squares.len(), 1000);
        assert!(squares.iter().enumerate().all(|(i, value)| *value == i * i));
        assert!(parallel(0, |index| index).is_empty());
    }

    #[test]
    fn empty_and_oversized_batches_are_refused() {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let empty = json!({"op": "sync_batch", "wallets": []});
        assert!(
            runtime
                .block_on(sync_batch(&empty, Network::Devnet))
                .is_err()
        );
        let many: Vec<Value> = (0..=MAX_BATCH_WALLETS)
            .map(|i| json!({"wallet_path": format!("w{i}.gnr"), "password": "x"}))
            .collect();
        let oversized = json!({"op": "sync_batch", "wallets": many});
        assert!(
            runtime
                .block_on(sync_batch(&oversized, Network::Devnet))
                .is_err()
        );
    }

    #[test]
    fn wrong_passwords_and_missing_files_fail_only_their_wallet() {
        let directory = tempfile::tempdir().unwrap();
        let good = directory.path().join("good.gnr");
        let wallet = Wallet::create(Network::Devnet).unwrap();
        wallet.save_new(&good, "correct-password-1").unwrap();
        let request = json!({
            "op": "sync_batch",
            "network": "devnet",
            "rpc_url": "http://127.0.0.1:1",
            "wallets": [
                {"wallet_path": good.display().to_string(), "password": "correct-password-1"},
                {"wallet_path": good.display().to_string(), "password": "wrong-password-22"},
                {"wallet_path": directory.path().join("missing.gnr").display().to_string(), "password": "x"},
            ],
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .unwrap();
        let result = runtime
            .block_on(sync_batch(&request, Network::Devnet))
            .unwrap();
        let wallets = result["wallets"].as_array().unwrap();
        assert_eq!(wallets.len(), 3);
        assert_eq!(wallets[0]["ok"], true, "{}", wallets[0]);
        assert_eq!(wallets[0]["address"], wallet.receiving_identity().unwrap());
        assert_eq!(wallets[0]["balance_atomic"], "0");
        assert!(
            wallets[0].get("history").is_none(),
            "compact summaries only"
        );
        assert_eq!(wallets[1]["ok"], false);
        assert_eq!(wallets[2]["ok"], false);
        assert_eq!(
            result["stale"], true,
            "no node answered: last state, marked stale"
        );
    }

    #[test]
    fn caches_advance_from_a_shared_range_and_detect_gaps() {
        let wallet = Wallet::create(Network::Devnet).unwrap();
        let keys = sync::ScanKeys::new(&wallet).unwrap();
        let block = Block::decode(include_bytes!(
            "../../../../../tests/vectors/devnet-block-1.bin"
        ))
        .unwrap();
        let range = BlockRange {
            start: 1,
            blocks: vec![block.clone()],
        };
        let mut cache = sync::ScanCache::new(Network::Devnet, 0);
        assert!(advance_from_range(&mut cache, &keys, &range).unwrap());
        assert_eq!(cache.height, 1);
        assert!(
            advance_from_range(&mut cache, &keys, &range).unwrap(),
            "already at the tip"
        );
        let mut fresh = sync::ScanCache::new(Network::Devnet, 0);
        let later = BlockRange {
            start: 2,
            blocks: vec![block],
        };
        assert!(
            !advance_from_range(&mut fresh, &keys, &later).unwrap(),
            "a range that does not reach back needs blocks from genesis"
        );
    }
}

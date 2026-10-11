use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use genoryn::client::RpcClient;
use genoryn_core::{block::Block, names};
use genoryn_core::{chain::unix_time, emission::parse_gnr, params::Network};
use genoryn_wallet_core::{
    chain_store,
    keys::{Wallet, decode_address},
    outbox,
    scan::{NoteProof, verify_payment_proof},
    sync,
    username::ReceiverBundle,
};
use serde_json::{Value, json};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};
use zeroize::{Zeroize, Zeroizing};

#[path = "wallet/batch.rs"]
mod batch;
#[path = "wallet/payments.rs"]
mod payments;
#[path = "wallet/proofs.rs"]
mod proofs;

/// Operations that read a payment on the verified chain.
const PROOF_OPS: [&str; 4] = [
    "export_proof",
    "verify_proof",
    "answer_payment_challenge",
    "verify_payment_challenge",
];

/// A batch request carries up to 10,000 wallet paths with their passwords.
const MAX_REQUEST_BYTES: u64 = 16 * 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "genoryn-wallet-cli",
    about = "GENORYN encrypted local wallet; passwords and seeds never belong in argv"
)]
struct Args {
    #[arg(long)]
    json_stdio: bool,
    #[arg(long, default_value = "mainnet")]
    network: String,
    #[arg(long, default_value = "wallet.wallet")]
    wallet: PathBuf,
    #[arg(long, default_value = "http://127.0.0.1:29711")]
    node: String,
    #[arg(long)]
    token_file: Option<PathBuf>,
    #[arg(long, default_value_t = 10)]
    confirmation_target: u64,
    /// ZIP32 account for create/restore; existing files retain their stored account.
    #[arg(long, default_value_t = 0)]
    account: u32,
    #[command(subcommand)]
    command: Option<Command>,
}
#[derive(Subcommand)]
enum Command {
    Create,
    Restore,
    Address {
        #[arg(long, default_value_t = 0)]
        index: u32,
    },
    DeriveAccount {
        account: u32,
        output: PathBuf,
    },
    Balance,
    /// Fixed local fee policy; no congestion or confirmation-time guarantee.
    EstimateFee {
        #[arg(default_value = "standard")]
        priority: String,
    },
    Send {
        to: String,
        amount: String,
        #[arg(long, default_value = "0.000010000000")]
        fee: String,
        #[arg(long, default_value = "")]
        memo: String,
    },
    RegisterUsername {
        username: String,
    },
    ResolveUsername {
        username: String,
    },
    Proof {
        txid: String,
        action: u32,
        output: PathBuf,
    },
    VerifyProof {
        file: PathBuf,
    },
}
fn field<'a>(v: &'a Value, k: &str) -> Result<&'a str> {
    v.get(k)
        .and_then(Value::as_str)
        .with_context(|| format!("missing field {k}"))
}
fn account_field(request: &Value) -> Result<u32> {
    match request.get("account") {
        None => Ok(0),
        Some(value) => u32::try_from(
            value
                .as_u64()
                .context("account must be a nonnegative integer")?,
        )
        .context("account index exceeds u32"),
    }
}
fn is_loopback_url(url: &str) -> bool {
    reqwest::Url::parse(url).is_ok_and(|url| {
        url.host_str()
            .and_then(|host| {
                host.trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .ok()
            })
            .is_some_and(|ip| ip.is_loopback())
    })
}
async fn block_at(rpc: &RpcClient, height: u64) -> Result<Block> {
    let mut page: Vec<Block> = rpc.get(&format!("v1/blocks?from={height}&limit=1")).await?;
    ensure!(page.len() == 1, "node returned no block at height {height}");
    let block = page.remove(0);
    ensure!(
        block.header.height == height,
        "node returned the wrong block"
    );
    Ok(block)
}
/// Blocks below a new wallet's birthday are validated but not decrypted. The
/// margin keeps a short reorganization around its creation harmless.
const BIRTHDAY_MARGIN: u64 = 100;

/// An outbox left at this path by a different wallet would block every sync;
/// it is renamed aside (never deleted) so its owner could still recover it.
fn set_aside_foreign_files(wallet: &Wallet, path: &Path) {
    let file = outbox::outbox_path(path);
    if file.exists() && outbox::load(wallet, &file).is_err() {
        let mut aside = file.as_os_str().to_owned();
        aside.push(format!(".foreign-{}", unix_time()));
        let _ = std::fs::rename(&file, aside);
    }
}

/// The scan cache to continue from; `full_rescan` starts over (keeping the
/// mining history), optionally from `rescan_height`.
fn open_cache(wallet: &Wallet, path: &Path, request: &Value) -> sync::ScanCache {
    let cache = sync::load_cache(wallet, &sync::cache_path(path));
    if request["full_rescan"].as_bool() != Some(true) {
        return cache;
    }
    let mut fresh = cache.restart();
    if let Some(height) = request["rescan_height"].as_u64() {
        fresh.birthday = height;
    }
    fresh
}
/// Brings `cache` to the tip of `blocks`, which are already validated, rolling
/// back to the last shared block when the chain forked.
fn advance_cache(
    cache: &mut sync::ScanCache,
    keys: &sync::ScanKeys,
    blocks: &[Block],
) -> Result<()> {
    let shared = if cache.height == 0 {
        Some(0)
    } else {
        cache
            .recent
            .iter()
            .rev()
            .find(|(height, hash)| {
                *height as usize <= blocks.len()
                    && blocks[*height as usize - 1].hash().ok() == Some(*hash)
            })
            .map(|(height, _)| *height)
    };
    match shared {
        Some(height) if height == cache.height => {}
        Some(height) if cache.can_roll_back_to(height) => cache.rollback_to(height),
        _ => *cache = cache.restart(),
    }
    for block in &blocks[cache.height as usize..] {
        cache.apply_block(keys, block)?;
    }
    Ok(())
}
/// Advances `cache` block by block from the wallet's own loopback node, which
/// has already validated every block.
async fn advance_cache_from_node(
    rpc: &RpcClient,
    network: Network,
    cache: &mut sync::ScanCache,
    keys: &sync::ScanKeys,
) -> Result<()> {
    let info: Value = rpc.get("v1/info").await?;
    ensure!(
        info["chain_id"] == network.chain_id()
            && info["genesis_hash"] == hex::encode(network.genesis()),
        "RPC network/genesis mismatch"
    );
    let tip = info["height"].as_u64().context("RPC height missing")?;
    // Find the newest cached block that is still on the node's chain.
    if cache.height > 0 {
        let mut shared = None;
        for (height, hash) in cache.recent.iter().rev().filter(|(h, _)| *h <= tip) {
            if block_at(rpc, *height).await?.hash()? == *hash {
                shared = Some(*height);
                break;
            }
        }
        match shared {
            Some(height) if height == cache.height => {}
            Some(height) if cache.can_roll_back_to(height) => cache.rollback_to(height),
            _ => *cache = cache.restart(),
        }
    }
    while cache.height < tip {
        let block = block_at(rpc, cache.height + 1).await?;
        cache.apply_block(keys, &block)?;
    }
    Ok(())
}
/// How a payment reaches the network.
enum Relayer<'a> {
    /// The wallet's own or configured node.
    Node(&'a RpcClient),
    /// Remote light-wallet sources over Tor, preferring one other than the
    /// source the chain was synced from.
    Light(&'a [RpcClient], usize),
}
impl Relayer<'_> {
    async fn send(&self, tx: &genoryn_core::privacy::ShieldedTx) -> Result<Value> {
        match self {
            Self::Node(rpc) if rpc.has_token() => rpc.post("v1/transactions", tx).await,
            Self::Node(rpc) => rpc.relay(tx).await,
            Self::Light(clients, synced_from) => genoryn::light::relay(clients, *synced_from, tx)
                .await
                .map(|(_, answer)| answer),
        }
    }
}
/// Relays every outbox entry that is due, with its exact stored bytes.
async fn relay_due(outbox: &mut outbox::Outbox, relayer: &Relayer<'_>) {
    let now = unix_time();
    for index in outbox.to_relay(now) {
        let entry = &outbox.entries[index];
        let txid = hex::encode(entry.txid);
        let answer = match entry.transaction() {
            Ok(tx) => match genoryn::light::acknowledged(relayer.send(&tx).await) {
                Err(error) if outbox::is_already_known(&error) => Ok(txid),
                other => other,
            },
            Err(error) => Err(error.to_string()),
        };
        outbox.record_relay(index, now, answer);
    }
}
/// What a payment pays, as recorded in the outbox.
struct Payee {
    recipient: String,
    amount: u64,
    fee: u64,
}
/// Stores the signed `tx` durably first, then relays it. The id the node
/// returns must match the locally computed one.
async fn submit_durably(
    wallet: &Wallet,
    path: &Path,
    outbox: &mut outbox::Outbox,
    tx: &genoryn_core::privacy::ShieldedTx,
    payee: Payee,
    relayer: &Relayer<'_>,
) -> Result<Value> {
    let entry =
        outbox::OutboxEntry::new(tx, payee.recipient, payee.amount, payee.fee, unix_time())?;
    let txid = hex::encode(entry.txid);
    outbox.add(entry)?;
    let file = outbox::outbox_path(path);
    outbox::save(wallet, &file, outbox)
        .context("the payment was not sent: it could not be stored safely first")?;
    relay_due(outbox, relayer).await;
    outbox::save(wallet, &file, outbox)?;
    let entry = outbox
        .entries
        .iter()
        .find(|entry| hex::encode(entry.txid) == txid)
        .context("stored payment missing")?;
    Ok(json!({
        "txid": txid,
        "status": entry.status.label(),
        "acknowledged": entry.status == outbox::OutboxStatus::Submitted,
        "relay_error": entry.last_error,
        "note": "Stored on this computer and handed to the network. It is confirmed only when a block includes it; until then it is retried automatically with the same signed bytes.",
    }))
}
/// Notes whose spend the chain has not yet settled are not available again.
fn exclude_reserved(
    wallet: &Wallet,
    outbox: &outbox::Outbox,
    scanned: &mut genoryn_wallet_core::scan::ScannedWallet,
) -> Result<()> {
    let reserved = outbox.reserved();
    if reserved.is_empty() {
        return Ok(());
    }
    let fvk = wallet.viewing_key()?;
    for note in scanned.notes.iter_mut().filter(|note| !note.spent) {
        if reserved.contains(&note.note.nullifier(&fvk).to_bytes()) {
            note.spent = true;
        }
    }
    Ok(())
}
/// Balance, history, mining rewards and outgoing payments for the GUI.
fn wallet_summary(
    wallet: &Wallet,
    cache: &sync::ScanCache,
    outbox: &outbox::Outbox,
    request: &Value,
) -> Result<Value> {
    let mut summary = serde_json::to_value(cache.summary(
        wallet,
        request["confirmation_target"].as_u64().unwrap_or(10),
    )?)?;
    let reserved = outbox.reserved();
    let held = |mature_only: bool| -> u128 {
        cache
            .notes
            .iter()
            .filter(|note| note.spent_height.is_none() && reserved.contains(&note.nullifier))
            .filter(|note| !mature_only || note.unlock_height <= cache.height)
            .map(|note| note.value as u128)
            .sum()
    };
    let spendable: u128 = summary["spendable_atomic"]
        .as_str()
        .unwrap_or("0")
        .parse()
        .unwrap_or(0);
    summary["spendable_atomic"] = json!(spendable.saturating_sub(held(true)).to_string());
    summary["reserved_atomic"] = json!(held(false).to_string());
    // One payment spends at most 16 notes; above this a "combine notes" step is needed.
    let notes: Vec<u64> = cache
        .notes
        .iter()
        .filter(|note| {
            note.spent_height.is_none()
                && note.unlock_height <= cache.height
                && !reserved.contains(&note.nullifier)
        })
        .map(|note| note.value)
        .collect();
    summary["spendable_notes"] = json!(notes.len());
    summary["max_single_payment_atomic"] =
        json!(genoryn_wallet_core::scan::max_single_payment(notes).to_string());
    summary["mined"] = serde_json::to_value(cache.mined_summary())?;
    summary["outbox"] = serde_json::to_value(outbox.summary())?;
    summary["birthday"] = json!(cache.birthday);
    with_name_identity(summary, wallet)
}
/// Full mode: updates the scan cache from the configured node, settles the
/// outbox against the chain and relays what is due. Returns the validated
/// chain too when `need_chain` (for building a payment).
async fn refresh_from_node(
    rpc: &RpcClient,
    network: Network,
    wallet: &Wallet,
    path: &Path,
    request: &Value,
    need_chain: bool,
) -> Result<(
    sync::ScanCache,
    outbox::Outbox,
    Option<(Vec<Block>, genoryn_core::state::ChainState)>,
)> {
    let own_node = request["full_validation"].as_bool() != Some(true)
        && is_loopback_url(
            request["rpc_url"]
                .as_str()
                .unwrap_or("http://127.0.0.1:29711"),
        );
    let keys = sync::ScanKeys::new(wallet)?;
    let mut cache = open_cache(wallet, path, request);
    let mut chain = None;
    let progress = if own_node && !need_chain {
        advance_cache_from_node(rpc, network, &mut cache, &keys).await
    } else {
        // A remote node's blocks are validated here in full; the wallet's own
        // node already validated them.
        let blocks = rpc.chain(network).await?;
        let verification = if own_node {
            genoryn_core::state::Verification::Stored
        } else {
            genoryn_core::state::Verification::Full
        };
        let state = genoryn_wallet_core::scan::replay_chain(network, &blocks, verification)?;
        let advanced = advance_cache(&mut cache, &keys, &blocks);
        chain = Some((blocks, state));
        advanced
    };
    // Keep verified progress even when the chain moved during the sync.
    sync::save_cache(wallet, &sync::cache_path(path), &cache)?;
    progress.context("the chain changed during sync; the next sync continues")?;
    let outbox_file = outbox::outbox_path(path);
    let mut outbox = outbox::load(wallet, &outbox_file)?;
    outbox.reconcile(&cache);
    relay_due(&mut outbox, &Relayer::Node(rpc)).await;
    outbox::save(wallet, &outbox_file, &outbox)?;
    Ok((cache, outbox, chain))
}
/// Public username identity of a wallet, so the GUI can look up its names.
fn with_name_identity(mut summary: Value, wallet: &Wallet) -> Result<Value> {
    summary["name_owner"] = json!(hex::encode(
        wallet.identity_key()?.verifying_key().to_bytes()
    ));
    summary["name_receiver"] = json!(genoryn_wallet_core::keys::encode_address(
        wallet.network,
        wallet.name_receiver()?
    )?);
    Ok(summary)
}
fn names_rpc(result: Result<Value>) -> Result<Value> {
    result.map_err(|error| {
        if error.to_string().contains("404") {
            anyhow::anyhow!("this node does not support on-chain usernames yet; update GENORYN")
        } else {
            error
        }
    })
}
/// The chain's answer for `@name`: registration, pending registration and price.
async fn chain_name_status(rpc: &RpcClient, name: &str) -> Result<Value> {
    let name = names::canonical_name(name)?;
    names_rpc(rpc.get(&format!("v1/names?name={name}")).await)
}
/// `@name`, or a bare name that cannot be an address. A bare name starting like
/// an address of this network needs the `@`, so a cut-off address is never
/// mistaken for someone's name.
fn names_a_username(network: Network, to: &str) -> bool {
    to.starts_with('@')
        || (!to
            .to_ascii_lowercase()
            .starts_with(&format!("{}1", network.hrp()))
            && decode_address(network, to).is_err()
            && names::canonical_name(to).is_ok())
}
fn not_a_recipient(network: Network) -> String {
    format!(
        "the recipient is neither a GENORYN {} address nor an @username",
        network.name()
    )
}
fn zeroize_value(v: &mut Value) {
    match v {
        Value::String(s) => s.zeroize(),
        Value::Array(a) => a.iter_mut().for_each(zeroize_value),
        Value::Object(o) => o.values_mut().for_each(zeroize_value),
        _ => (),
    }
}

async fn dispatch(request: &Value) -> Result<Value> {
    let op = field(request, "op")?;
    let network = Network::parse(request["network"].as_str().unwrap_or("mainnet"))?;
    network.ensure_runnable()?;
    if op == "estimate_fee" {
        let priority = serde_json::from_value(
            request
                .get("priority")
                .cloned()
                .unwrap_or_else(|| json!("standard")),
        )?;
        return Ok(serde_json::to_value(
            genoryn_wallet_core::fees::estimate_fee(priority),
        )?);
    }
    if op == "sync_batch" {
        return batch::sync_batch(request, network).await;
    }
    if op == "create_payment_challenge" {
        return proofs::create_challenge(request, network);
    }
    let path = Path::new(request["wallet_path"].as_str().unwrap_or("wallet.wallet"));
    let password = Zeroizing::new(request["password"].as_str().unwrap_or("").to_owned());
    if op == "create" {
        let wallet = Wallet::create(network)?.for_account(account_field(request)?)?;
        wallet.save_new(path, &password)?;
        set_aside_foreign_files(&wallet, path);
        // A new seed cannot have received anything before the verified height
        // known at creation; unknown (offline) means scanning from genesis.
        let birthday = request["birthday_height"]
            .as_u64()
            .map_or(0, |height| height.saturating_sub(BIRTHDAY_MARGIN));
        sync::save_cache(
            &wallet,
            &sync::cache_path(path),
            &sync::ScanCache::with_birthday(network, wallet.account, birthday),
        )?;
        let phrase = wallet.phrase()?;
        return Ok(
            json!({"address":wallet.receiving_identity()?,"seed_phrase":phrase.as_str(),"network":network.name(),"account":wallet.account,"birthday":birthday}),
        );
    }
    if op == "restore" {
        let wallet = Wallet::restore(network, field(request, "seed_phrase")?)?
            .for_account(account_field(request)?)?;
        // The GUI lists the wallets already open; the same one is never written twice.
        let address = wallet.receiving_identity()?;
        ensure!(
            !request["refuse_addresses"]
                .as_array()
                .is_some_and(|open| open.iter().any(|known| known.as_str() == Some(&address))),
            "this recovery phrase belongs to a wallet that is already open; it was not added a second time"
        );
        wallet.save_new(path, &password)?;
        set_aside_foreign_files(&wallet, path);
        // A restored seed scans its whole history unless told where it began.
        // Local state this same seed left at this path (pending payments,
        // mining history) is kept.
        let birthday = request["restore_height"].as_u64().unwrap_or(0);
        let mut cache = sync::load_cache(&wallet, &sync::cache_path(path));
        cache.birthday = cache.birthday.min(birthday);
        if cache.height == 0 && cache.mined.is_empty() {
            cache.birthday = birthday;
        }
        sync::save_cache(&wallet, &sync::cache_path(path), &cache)?;
        return Ok(
            json!({"address":wallet.receiving_identity()?,"network":network.name(),"account":wallet.account,"birthday":birthday}),
        );
    }
    let wallet = if ["resolve_username", "name_status"].contains(&op) {
        None
    } else if op == "verify_proof" || op == "verify_payment_challenge" {
        // Optional: with a wallet, the result also tells whether it was paid to it.
        (!password.is_empty() && path.is_file())
            .then(|| Wallet::open(path, &password).ok())
            .flatten()
            .filter(|wallet| wallet.network == network)
    } else {
        let w = Wallet::open(path, &password)?;
        ensure!(w.network == network, "wallet network mismatch");
        Some(w)
    };
    if op == "open" || op == "address" {
        let wallet = wallet.as_ref().context("wallet required")?;
        let index = match request.get("address_index") {
            None => 0,
            Some(value) => u32::try_from(
                value
                    .as_u64()
                    .context("address index must be a nonnegative integer")?,
            )
            .context("address index exceeds u32")?,
        };
        return Ok(
            json!({"address":genoryn_wallet_core::keys::encode_address(network,wallet.address(index)?)?,"network":network.name(),"account":wallet.account,"address_index":index}),
        );
    }
    if op == "derive_account" {
        let account = wallet
            .as_ref()
            .context("wallet required")?
            .for_account(account_field(request)?)?;
        account.save_new(Path::new(field(request, "output")?), &password)?;
        return Ok(
            json!({"address":account.receiving_identity()?,"account":account.account,"network":network.name()}),
        );
    }
    if request["light"].as_bool() == Some(true) {
        return light_dispatch(op, request, network, wallet, path).await;
    }
    let token_path = request["token_file"]
        .as_str()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    let rpc = RpcClient::new(
        request["rpc_url"]
            .as_str()
            .unwrap_or("http://127.0.0.1:29711"),
        token_path.as_deref(),
    )?;
    if op == "resolve_username" {
        let name = genoryn_wallet_core::username::canonicalize(field(request, "username")?)?;
        let bundle: ReceiverBundle = rpc.get(&format!("v1/usernames?username={name}")).await?;
        bundle.verify(network, unix_time())?;
        ensure!(
            bundle.username == name,
            "resolver returned a different username"
        );
        return Ok(
            json!({"address":bundle.receiver,"username":name,"verified":true,"owner_fingerprint":bundle.fingerprint(),"trust":"signed local resolver record; confirm owner fingerprint on first use"}),
        );
    }
    if op == "register_username" {
        let w = wallet.as_ref().context("wallet required")?;
        let name = genoryn_wallet_core::username::canonicalize(field(request, "username")?)?;
        let sequence = match rpc
            .get::<ReceiverBundle>(&format!("v1/usernames/record?username={name}"))
            .await
        {
            Ok(b) => {
                b.verify_for_renewal(network, unix_time())?;
                ensure!(b.username == name, "resolver returned a different username");
                ensure!(
                    b.owner_key == w.identity_key()?.verifying_key().to_bytes(),
                    "username belongs to another owner"
                );
                b.sequence
                    .checked_add(1)
                    .context("username sequence overflow")?
            }
            Err(_) => 0,
        };
        return rpc
            .post(
                "v1/usernames",
                &w.receiver_bundle(&name, sequence, unix_time())?,
            )
            .await;
    }
    if op == "name_status" {
        return chain_name_status(&rpc, field(request, "name")?).await;
    }
    if op == "my_names" {
        let w = wallet.as_ref().context("wallet required")?;
        let owner = hex::encode(w.identity_key()?.verifying_key().to_bytes());
        return names_rpc(rpc.get(&format!("v1/names/owner?owner={owner}")).await);
    }
    if op == "sync" {
        let wallet = wallet.as_ref().context("wallet required")?;
        let (cache, outbox, _) =
            refresh_from_node(&rpc, network, wallet, path, request, false).await?;
        let mut summary = wallet_summary(wallet, &cache, &outbox, request)?;
        summary["mode"] = json!("full");
        summary["verified_by"] = json!(if is_loopback_url(
            request["rpc_url"]
                .as_str()
                .unwrap_or("http://127.0.0.1:29711")
        ) {
            "your own node"
        } else {
            "this computer (every block validated)"
        });
        return Ok(summary);
    }
    if matches!(
        op,
        "send" | "send_many" | "register_name" | "update_name" | "combine_notes"
    ) {
        let wallet = wallet.as_ref().context("wallet required")?;
        if let Some(existing) = payments::already_sent(wallet, path, request)? {
            return Ok(existing);
        }
        let (_, mut outbox, chain) =
            refresh_from_node(&rpc, network, wallet, path, request, true).await?;
        let (blocks, state) = chain.context("validated chain required")?;
        let mut scanned = wallet.scan_with_state(state, &blocks)?;
        exclude_reserved(wallet, &outbox, &mut scanned)?;
        exclude_pending_spends(&rpc, wallet, &mut scanned).await?;
        return build_and_submit(
            op,
            request,
            network,
            wallet,
            path,
            &rpc,
            &scanned,
            &mut outbox,
            &Relayer::Node(&rpc),
        )
        .await;
    }
    if PROOF_OPS.contains(&op) {
        let blocks = rpc.chain(network).await?;
        // The user's own node already validated its blocks; a remote node's are
        // validated here in full.
        let own_node = request["full_validation"].as_bool() != Some(true)
            && is_loopback_url(
                request["rpc_url"]
                    .as_str()
                    .unwrap_or("http://127.0.0.1:29711"),
            );
        let verification = if own_node {
            genoryn_core::state::Verification::Stored
        } else {
            genoryn_core::state::Verification::Full
        };
        genoryn_wallet_core::scan::replay_chain(network, &blocks, verification)?;
        return proof_op(op, request, network, wallet.as_ref(), path, &blocks);
    }
    anyhow::bail!("unsupported wallet operation")
}
/// What an outbox entry for merging notes shows as its recipient.
const COMBINE_RECIPIENT: &str = "yourself (combining notes)";
/// Payment-proof operations against an already validated chain.
fn proof_op(
    op: &str,
    request: &Value,
    network: Network,
    wallet: Option<&Wallet>,
    path: &Path,
    blocks: &[Block],
) -> Result<Value> {
    let find = |txid: &str| proofs::find_on_chain(blocks, txid);
    if op == "export_proof" {
        let wallet = wallet.context("wallet required")?;
        let txid = field(request, "txid")?;
        let prove = |tx: &genoryn_core::privacy::ShieldedTx| match request["action"].as_u64() {
            Some(action) => wallet.payment_proof(tx, action as usize),
            None => wallet.payment_proof_auto(tx),
        };
        let (proof, mined_at) = match find(txid) {
            Some((height, tx)) => (prove(tx)?, Some(height)),
            None => {
                // Not mined yet: the exact signed bytes are in the outbox.
                let outbox = outbox::load(wallet, &outbox::outbox_path(path))?;
                let entry = outbox
                    .entries
                    .iter()
                    .find(|entry| hex::encode(entry.txid) == txid)
                    .context(
                        "this transaction is neither in the chain nor among this wallet's payments",
                    )?;
                (prove(&entry.transaction()?)?, None)
            }
        };
        let mut value = serde_json::to_value(&proof)?;
        value["mined_at"] = json!(mined_at);
        return Ok(value);
    }
    match op {
        "answer_payment_challenge" => proofs::answer_challenge(
            request,
            network,
            wallet.context("open the wallet that made the payment")?,
            path,
            blocks,
        ),
        "verify_payment_challenge" => proofs::verify_challenge(request, network, wallet, blocks),
        _ => proofs::verify_proof(request, network, wallet, blocks),
    }
}
/// Builds the payment or name transaction from `scanned` (reserved and
/// mempool-spent notes already excluded) and submits it through the outbox.
#[allow(clippy::too_many_arguments)]
async fn build_and_submit(
    op: &str,
    request: &Value,
    network: Network,
    wallet: &Wallet,
    path: &Path,
    names_rpc_client: &RpcClient,
    scanned: &genoryn_wallet_core::scan::ScannedWallet,
    outbox: &mut outbox::Outbox,
    relayer: &Relayer<'_>,
) -> Result<Value> {
    let fee = request["fee_atomic"]
        .as_str()
        .unwrap_or("10000000")
        .parse::<u64>()?;
    if op == "combine_notes" {
        let max = request["max_transactions"]
            .as_u64()
            .unwrap_or(8)
            .clamp(1, 16) as usize;
        let mut txids = Vec::new();
        let mut combined = 0;
        for (tx, merged, kept) in wallet.combine_notes(scanned, fee, max)? {
            let payee = Payee {
                recipient: COMBINE_RECIPIENT.into(),
                amount: kept,
                fee,
            };
            let submitted = submit_durably(wallet, path, outbox, &tx, payee, relayer).await?;
            txids.push(submitted["txid"].clone());
            combined += merged;
        }
        return Ok(json!({
            "transactions": txids.len(),
            "notes_combined": combined,
            "txids": txids,
            "note": "Each transfer to yourself merges up to 16 notes into one. After the next block, larger payments need fewer notes.",
        }));
    }
    if op == "register_name" || op == "update_name" {
        let status = chain_name_status(names_rpc_client, field(request, "name")?).await?;
        let name = status["name"]
            .as_str()
            .context("name status missing")?
            .to_owned();
        let owner = hex::encode(wallet.identity_key()?.verifying_key().to_bytes());
        let (kind, sequence) = if op == "register_name" {
            ensure!(
                status["pending"] != true,
                "@{name} is already being registered; wait for the next block"
            );
            ensure!(status["available"] == true, "@{name} is already taken");
            (names::OP_REGISTER, 0)
        } else {
            let entry = &status["registered"];
            ensure!(!entry.is_null(), "@{name} is not registered");
            ensure!(entry["owner"] == owner, "@{name} belongs to another wallet");
            let sequence = entry["sequence"].as_u64().context("name sequence")? as u32;
            (names::OP_UPDATE, sequence + 1)
        };
        let tx = wallet.name_transaction(scanned, kind, &name, sequence, fee)?;
        let burned = if kind == names::OP_REGISTER {
            names::registration_price(&name)
        } else {
            names::UPDATE_PRICE
        };
        let payee = Payee {
            recipient: format!("@{name}"),
            amount: burned,
            fee,
        };
        let mut submitted = submit_durably(wallet, path, outbox, &tx, payee, relayer).await?;
        submitted["name"] = json!(name);
        submitted["burned_atomic"] = json!(burned.to_string());
        submitted["receiver"] = json!(genoryn_wallet_core::keys::encode_address(
            network,
            wallet.name_receiver()?
        )?);
        return Ok(submitted);
    }
    if op == "send_many" {
        return payments::send_many(request, network, wallet, path, scanned, outbox, relayer).await;
    }
    let to = field(request, "to")?.trim();
    let named = names_a_username(network, to);
    let address = if !named {
        decode_address(network, to).with_context(|| not_a_recipient(network))?
    } else if matches!(relayer, Relayer::Light(..)) {
        anyhow::bail!("usernames are not resolved in Quick mode yet; use an address")
    } else {
        resolve_username(names_rpc_client, network, path, to).await?
    };
    let amount = field(request, "amount_atomic")?.parse::<u64>()?;
    // New payments carry a sender-verification key unless switched off.
    let tx = wallet.send(
        scanned,
        address,
        amount,
        fee,
        request["memo"].as_str().unwrap_or(""),
        request["sender_auth"].as_bool() != Some(false),
    )?;
    let payee = Payee {
        recipient: to.to_owned(),
        amount,
        fee,
    };
    submit_durably(wallet, path, outbox, &tx, payee, relayer).await
}
/// The receiving address an `@username` points to: the on-chain name first,
/// then a signed resolver record pinned to its owner on first use.
async fn resolve_username(
    rpc: &RpcClient,
    network: Network,
    path: &Path,
    to: &str,
) -> Result<orchard::Address> {
    if let Some(receiver) = chain_name_status(rpc, to)
        .await
        .ok()
        .and_then(|status| status["registered"]["receiver"].as_str().map(str::to_owned))
    {
        return decode_address(network, &receiver);
    }
    let name = genoryn_wallet_core::username::canonicalize(to)?;
    let b: ReceiverBundle = rpc.get(&format!("v1/usernames?username={name}")).await?;
    b.verify(network, unix_time())?;
    ensure!(b.username == name, "resolver returned a different username");
    // Owner pinning is local and contains only public identity material. A changed key fails closed.
    let pinpath = path.with_extension("username-pins.json");
    let mut pins: std::collections::BTreeMap<String, String> = if pinpath.exists() {
        ensure!(
            std::fs::metadata(&pinpath)?.len() <= 100_000,
            "pin file too large"
        );
        serde_json::from_slice(&std::fs::read(&pinpath)?)?
    } else {
        Default::default()
    };
    if let Some(old) = pins.get(&name) {
        ensure!(
            *old == b.fingerprint(),
            "username owner fingerprint changed; verify independently before updating trust"
        );
    } else {
        pins.insert(name, b.fingerprint());
        std::fs::write(&pinpath, serde_json::to_vec(&pins)?)?;
    }
    decode_address(network, &b.receiver)
}
/// Quick mode: no local node. Chain data comes from remote nodes' public RPC
/// (`sources`, onion services or HTTPS) and transactions go out through them,
/// only over Tor: the built-in client (state beside the wallet) or `tor_socks`.
/// Every new block is validated here in full and kept in the wallet's verified
/// chain, so later syncs fetch only what is new. When no source answers, the
/// last verified state is returned and marked stale; it is never presented as
/// current.
async fn light_dispatch(
    op: &str,
    request: &Value,
    network: Network,
    wallet: Option<Wallet>,
    path: &Path,
) -> Result<Value> {
    ensure!(
        matches!(op, "sync" | "send" | "combine_notes") || PROOF_OPS.contains(&op),
        "{op} is not available in Quick mode yet"
    );
    let wallet = wallet.context("wallet required")?;
    let sources: Vec<String> = match request["sources"].as_array() {
        Some(list) => list
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        None => vec![field(request, "rpc_url")?.to_owned()],
    };
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
    let started = std::time::Instant::now();
    let store = chain_store::chain_path(path);
    let mut chain = chain_store::load(&wallet, &store);
    let local_state = chain.replay(network);
    let replayed_ms = started.elapsed().as_millis() as u64;
    let keys = sync::ScanKeys::new(&wallet)?;
    let mut cache = open_cache(&wallet, path, request);
    let outbox_file = outbox::outbox_path(path);
    let mut outbox = outbox::load(&wallet, &outbox_file)?;
    let tor =
        genoryn::light::LightTor::start(&tor_dir, external, std::time::Duration::from_secs(300))
            .await;
    let (verified, clients, offline) = match tor {
        Ok(tor) => {
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
            (verified, Some((tor, clients)), None)
        }
        Err(error) => {
            let height = local_state.height;
            (
                genoryn::light::Verified {
                    blocks: std::mem::take(&mut chain.blocks),
                    state: local_state,
                    fork: height,
                    used: None,
                    reports: Vec::new(),
                    reachable: 0,
                },
                None,
                Some(format!("{error:#}")),
            )
        }
    };
    let stale = verified.is_stale();
    let previous_height = chain_height_before(&verified);
    chain.blocks = verified.blocks;
    if verified.used.is_some() {
        chain_store::save(&wallet, &store, &chain)?;
    }
    advance_cache(&mut cache, &keys, &chain.blocks)?;
    sync::save_cache(&wallet, &sync::cache_path(path), &cache)?;
    outbox.reconcile(&cache);
    // The source the chain came from, or the first that answered.
    let synced_from = verified.used.unwrap_or_else(|| {
        verified
            .reports
            .iter()
            .position(|report| report.height.is_some())
            .unwrap_or(0)
    });
    if let Some((_, clients)) = &clients
        && !stale
    {
        relay_due(&mut outbox, &Relayer::Light(clients, synced_from)).await;
    }
    outbox::save(&wallet, &outbox_file, &outbox)?;
    let reports = serde_json::to_value(&verified.reports)?;
    if op == "sync" {
        let mut summary = wallet_summary(&wallet, &cache, &outbox, request)?;
        summary["mode"] = json!("quick");
        summary["verified_by"] = json!("this computer (Quick mode, every block validated)");
        summary["light_sources"] = reports;
        summary["stale"] = json!(stale);
        if let Some(reason) = offline {
            summary["offline_reason"] = json!(reason);
        }
        summary["sync_stats"] = json!({
            "replayed_local_blocks": previous_height,
            "replay_ms": replayed_ms,
            "fork_height": verified.fork,
            "total_ms": started.elapsed().as_millis() as u64,
        });
        return Ok(summary);
    }
    if PROOF_OPS.contains(&op) {
        // A check against a chain no source confirmed now may miss a newer
        // block or a reorganization; it is reported as unavailable.
        if stale
            && op.starts_with("verify")
            && request["structured_outcomes"].as_bool() == Some(true)
        {
            return Ok(json!({
                "outcome": "unavailable",
                "last_verified_height": chain.blocks.len(),
                "offline_reason": offline,
                "light_sources": reports,
            }));
        }
        // Every block of this chain was validated on this computer.
        let mut result = proof_op(op, request, network, Some(&wallet), path, &chain.blocks)?;
        result["chain_stale"] = json!(stale);
        return Ok(result);
    }
    let Some((_tor, clients)) = clients else {
        anyhow::bail!(
            "no payment was created: Tor is not connected ({})",
            offline.unwrap_or_default()
        );
    };
    ensure!(
        !stale,
        "no payment was created: no remote node answered, so the balance may be out of date"
    );
    let mut scanned = wallet.scan_with_state(verified.state, &chain.blocks)?;
    exclude_reserved(&wallet, &outbox, &mut scanned)?;
    exclude_pending_spends(&clients[synced_from], &wallet, &mut scanned).await?;
    let mut submitted = build_and_submit(
        op,
        request,
        network,
        &wallet,
        path,
        &clients[synced_from],
        &scanned,
        &mut outbox,
        &Relayer::Light(&clients, synced_from),
    )
    .await?;
    submitted["light_sources"] = reports;
    Ok(submitted)
}
/// Height of the local chain before the sync replaced anything above `fork`.
fn chain_height_before(verified: &genoryn::light::Verified) -> u64 {
    if verified.used.is_some() {
        verified.fork
    } else {
        verified.blocks.len() as u64
    }
}
/// Treats coins spent by this wallet's own unconfirmed transfers as spent, so a
/// second transfer before the next block uses other coins instead of being refused
/// as a double spend. Nodes before 0.5.0 have no such list; nothing is excluded then.
async fn exclude_pending_spends(
    rpc: &genoryn::client::RpcClient,
    wallet: &Wallet,
    scanned: &mut genoryn_wallet_core::scan::ScannedWallet,
) -> Result<()> {
    let Ok(pending) = rpc.get::<Value>("v1/mempool/nullifiers").await else {
        return Ok(());
    };
    let pending: std::collections::BTreeSet<&str> = pending["nullifiers"]
        .as_array()
        .map(|list| list.iter().filter_map(Value::as_str).collect())
        .unwrap_or_default();
    if pending.is_empty() {
        return Ok(());
    }
    let fvk = wallet.viewing_key()?;
    for note in scanned.notes.iter_mut().filter(|note| !note.spent) {
        if pending.contains(hex::encode(note.note.nullifier(&fvk).to_bytes()).as_str()) {
            note.spent = true;
        }
    }
    Ok(())
}
#[tokio::main]
async fn main() -> Result<()> {
    let args = Args::parse();
    if args.json_stdio {
        let mut input = Zeroizing::new(String::new());
        std::io::stdin()
            .take(MAX_REQUEST_BYTES + 1)
            .read_to_string(&mut input)?;
        ensure!(
            input.len() as u64 <= MAX_REQUEST_BYTES,
            "wallet request too large"
        );
        let mut request: Value = serde_json::from_str(&input)?;
        let result = dispatch(&request).await;
        zeroize_value(&mut request);
        let mut response = match result {
            Ok(result) => json!({"ok":true,"result":result}),
            Err(error) => json!({"ok":false,"error":error.to_string()}),
        };
        let encoded = Zeroizing::new(serde_json::to_string(&response)?);
        println!("{}", encoded.as_str());
        zeroize_value(&mut response);
        return Ok(());
    }
    let mut request = json!({"network":args.network,"wallet_path":args.wallet,"rpc_url":args.node,"token_file":args.token_file,"confirmation_target":args.confirmation_target,"account":args.account});
    let command = args.command.context("choose a command or --json-stdio")?;
    let needs_password = !matches!(
        command,
        Command::ResolveUsername { .. } | Command::VerifyProof { .. } | Command::EstimateFee { .. }
    );
    if needs_password {
        let password = Zeroizing::new(rpassword::prompt_password("Wallet password: ")?);
        request["password"] = json!(password.as_str());
    }
    match command {
        Command::Create => request["op"] = json!("create"),
        Command::Restore => {
            request["op"] = json!("restore");
            let phrase = Zeroizing::new(rpassword::prompt_password("24-word recovery phrase: ")?);
            request["seed_phrase"] = json!(phrase.as_str());
        }
        Command::Address { index } => {
            request["op"] = json!("address");
            request["address_index"] = json!(index);
        }
        Command::DeriveAccount { account, output } => {
            request["op"] = json!("derive_account");
            request["account"] = json!(account);
            request["output"] = json!(output);
        }
        Command::Balance => request["op"] = json!("sync"),
        Command::EstimateFee { priority } => {
            request["op"] = json!("estimate_fee");
            request["priority"] = json!(priority);
        }
        Command::Send {
            to,
            amount,
            fee,
            memo,
        } => {
            request["op"] = json!("send");
            request["to"] = json!(to);
            request["amount_atomic"] =
                json!(parse_gnr(&amount).map_err(anyhow::Error::msg)?.to_string());
            request["fee_atomic"] = json!(parse_gnr(&fee).map_err(anyhow::Error::msg)?.to_string());
            request["memo"] = json!(memo);
        }
        Command::RegisterUsername { username } => {
            request["op"] = json!("register_username");
            request["username"] = json!(username);
        }
        Command::ResolveUsername { username } => {
            request["op"] = json!("resolve_username");
            request["username"] = json!(username);
        }
        Command::Proof {
            txid,
            action,
            output,
        } => {
            request["op"] = json!("export_proof");
            request["txid"] = json!(txid);
            request["action"] = json!(action);
            let proof = dispatch(&request).await?;
            zeroize_value(&mut request);
            let mut file = std::fs::OpenOptions::new()
                .create_new(true)
                .write(true)
                .open(output)?;
            file.write_all(&serde_json::to_vec_pretty(&proof)?)?;
            println!(
                "Payment proof saved. Share only with your chosen verifier; it discloses one output."
            );
            return Ok(());
        }
        Command::VerifyProof { file } => {
            request["op"] = json!("verify_proof");
            ensure!(
                std::fs::metadata(&file)?.len() < 65_536,
                "proof file too large"
            );
            request["proof"] = serde_json::from_slice(&std::fs::read(file)?)?;
        }
    }
    let result = dispatch(&request).await;
    zeroize_value(&mut request);
    let mut result = result?;
    let output = Zeroizing::new(serde_json::to_string_pretty(&result)?);
    println!("{}", output.as_str());
    zeroize_value(&mut result);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_names_are_usernames_but_whole_or_cut_off_addresses_are_not() {
        let network = Network::Mainnet;
        let address =
            "gnr1he05z70c97e4tcrkyj0q3upms7cdt9hzzc8cdh7ets0sg5un390kt0caa0jmhtutzeyr76k9tyl";
        assert!(decode_address(network, address).is_ok());
        assert!(!names_a_username(network, address));
        for name in ["@genorynadmin", "genorynadmin", "GenorynAdmin", "@anything"] {
            assert!(names_a_username(network, name), "{name}");
        }
        for not_name in [
            "gnr1he05z70c",
            "GNR1HE05Z70C",
            "ab",
            "two words",
            "",
            "genoryn",
        ] {
            assert!(!names_a_username(network, not_name), "{not_name}");
        }
        assert!(
            names_a_username(Network::Testnet, "gnr1he05z70c"),
            "only this network's address prefix needs the @"
        );
        let error = decode_address(network, "genoryn")
            .with_context(|| not_a_recipient(network))
            .unwrap_err();
        assert_eq!(
            error.to_string(),
            "the recipient is neither a GENORYN mainnet address nor an @username"
        );
    }
}

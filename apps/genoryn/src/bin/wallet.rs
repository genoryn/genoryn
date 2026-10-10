use anyhow::{Context, Result, ensure};
use clap::{Parser, Subcommand};
use genoryn::client::RpcClient;
use genoryn_core::{block::Block, names};
use genoryn_core::{chain::unix_time, emission::parse_gnr, params::Network};
use genoryn_wallet_core::{
    keys::{Wallet, decode_address},
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
    #[arg(long, default_value = "http://127.0.0.1:29611")]
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
/// Balance and history from the encrypted scan cache plus any new blocks. Used only
/// with the wallet's own loopback node, which has already validated every block.
async fn incremental_sync(
    rpc: &RpcClient,
    network: Network,
    wallet: &Wallet,
    path: &Path,
    request: &Value,
) -> Result<Value> {
    let info: Value = rpc.get("v1/info").await?;
    ensure!(
        info["chain_id"] == network.chain_id()
            && info["genesis_hash"] == hex::encode(network.genesis()),
        "RPC network/genesis mismatch"
    );
    let tip = info["height"].as_u64().context("RPC height missing")?;
    let file = sync::cache_path(path);
    let mut cache = if request["full_rescan"].as_bool() == Some(true) {
        sync::ScanCache::new(network, wallet.account)
    } else {
        sync::load_cache(wallet, &file)
    };
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
            Some(height) if cache.can_roll_back_to(height) => cache.rollback_to(height),
            _ => cache = sync::ScanCache::new(network, wallet.account),
        }
    }
    let keys = sync::ScanKeys::new(wallet)?;
    let mut progress = Ok(());
    while cache.height < tip {
        match block_at(rpc, cache.height + 1).await {
            Ok(block) => {
                if let Err(error) = cache.apply_block(&keys, &block) {
                    progress = Err(error);
                    break;
                }
            }
            Err(error) => {
                progress = Err(error);
                break;
            }
        }
    }
    // Keep verified progress even when the chain moved during the sync.
    sync::save_cache(wallet, &file, &cache)?;
    progress.context("the chain changed during sync; the next sync continues")?;
    with_name_identity(
        serde_json::to_value(cache.summary(
            wallet,
            request["confirmation_target"].as_u64().unwrap_or(10),
        )?)?,
        wallet,
    )
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
    let path = Path::new(request["wallet_path"].as_str().unwrap_or("wallet.wallet"));
    let password = Zeroizing::new(request["password"].as_str().unwrap_or("").to_owned());
    if op == "create" {
        let wallet = Wallet::create(network)?.for_account(account_field(request)?)?;
        wallet.save_new(path, &password)?;
        let phrase = wallet.phrase()?;
        return Ok(
            json!({"address":wallet.receiving_identity()?,"seed_phrase":phrase.as_str(),"network":network.name(),"account":wallet.account}),
        );
    }
    if op == "restore" {
        let wallet = Wallet::restore(network, field(request, "seed_phrase")?)?
            .for_account(account_field(request)?)?;
        wallet.save_new(path, &password)?;
        return Ok(
            json!({"address":wallet.receiving_identity()?,"network":network.name(),"account":wallet.account}),
        );
    }
    let wallet = if ["resolve_username", "verify_proof", "name_status"].contains(&op) {
        None
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
            .unwrap_or("http://127.0.0.1:29611"),
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
    if op == "sync"
        && request["full_validation"].as_bool() != Some(true)
        && is_loopback_url(
            request["rpc_url"]
                .as_str()
                .unwrap_or("http://127.0.0.1:29611"),
        )
    {
        let wallet = wallet.as_ref().context("wallet required")?;
        return incremental_sync(&rpc, network, wallet, path, request).await;
    }
    let blocks = rpc.chain(network).await?;
    if matches!(op, "verify_proof" | "export_proof") {
        genoryn_wallet_core::scan::validate_chain(network, &blocks)?;
    }
    if op == "verify_proof" {
        let proof: NoteProof = serde_json::from_value(request["proof"].clone())?;
        let tx = blocks
            .iter()
            .flat_map(|b| b.body.all_transactions())
            .find(|t| t.id().map(hex::encode).ok() == Some(proof.txid.clone()))
            .context("payment transaction not in selected chain")?;
        return Ok(serde_json::to_value(verify_payment_proof(&proof, tx)?)?);
    }
    let wallet = wallet.context("wallet required")?;
    if op == "export_proof" {
        let txid = field(request, "txid")?;
        let index = request["action"].as_u64().unwrap_or(0) as usize;
        let tx = blocks
            .iter()
            .flat_map(|b| b.body.all_transactions())
            .find(|t| t.id().map(hex::encode).ok().as_deref() == Some(txid))
            .context("transaction not found")?;
        return Ok(serde_json::to_value(wallet.payment_proof(tx, index)?)?);
    }
    let mut scanned = wallet.scan(&blocks)?;
    if matches!(op, "send" | "register_name" | "update_name") {
        exclude_pending_spends(&rpc, &wallet, &mut scanned).await?;
    }
    if op == "sync" {
        return with_name_identity(
            serde_json::to_value(wallet.summary_with_confirmation_target(
                &scanned,
                request["confirmation_target"].as_u64().unwrap_or(10),
            )?)?,
            &wallet,
        );
    }
    if op == "register_name" || op == "update_name" {
        let status = chain_name_status(&rpc, field(request, "name")?).await?;
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
        let fee = request["fee_atomic"]
            .as_str()
            .unwrap_or("10000000")
            .parse::<u64>()?;
        let tx = wallet.name_transaction(&scanned, kind, &name, sequence, fee)?;
        let submitted: Value = rpc.post("v1/transactions", &tx).await?;
        let burned = if kind == names::OP_REGISTER {
            names::registration_price(&name)
        } else {
            names::UPDATE_PRICE
        };
        return Ok(json!({
            "name": name,
            "txid": submitted["txid"].clone(),
            "burned_atomic": burned.to_string(),
            "receiver": genoryn_wallet_core::keys::encode_address(network, wallet.name_receiver()?)?,
        }));
    }
    if op == "send" {
        let to = field(request, "to")?.trim();
        let named = names_a_username(network, to);
        let on_chain = if named {
            chain_name_status(&rpc, to)
                .await
                .ok()
                .and_then(|status| status["registered"]["receiver"].as_str().map(str::to_owned))
        } else {
            None
        };
        let address = if let Some(receiver) = on_chain {
            decode_address(network, &receiver)?
        } else if named {
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
            decode_address(network, &b.receiver)?
        } else {
            decode_address(network, to).with_context(|| not_a_recipient(network))?
        };
        let amount = field(request, "amount_atomic")?.parse::<u64>()?;
        let fee = request["fee_atomic"]
            .as_str()
            .unwrap_or("10000000")
            .parse::<u64>()?;
        let tx = wallet.send(
            &scanned,
            address,
            amount,
            fee,
            request["memo"].as_str().unwrap_or(""),
        )?;
        return rpc.post("v1/transactions", &tx).await;
    }
    anyhow::bail!("unsupported wallet operation")
}
/// Light wallet: no local node. Chain data comes from remote nodes' public RPC
/// (`sources`, onion services or HTTPS) and transactions go out through them,
/// only over Tor: the built-in client (state beside the wallet) or `tor_socks`.
/// The chain is fully validated here before it is scanned.
async fn light_dispatch(
    op: &str,
    request: &Value,
    network: Network,
    wallet: Option<Wallet>,
    path: &Path,
) -> Result<Value> {
    ensure!(
        matches!(op, "sync" | "send"),
        "{op} is not available in light mode yet"
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
        "light mode needs one to eight sources"
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
    let tor =
        genoryn::light::LightTor::start(&tor_dir, external, std::time::Duration::from_secs(300))
            .await?;
    let clients = sources
        .iter()
        .map(|url| RpcClient::through_tor(url, &tor.proxy()))
        .collect::<Result<Vec<_>>>()?;
    let (mut scanned, synced_from, reports) =
        genoryn::light::sync(network, &clients, &sources, |blocks| wallet.scan(blocks)).await?;
    let reports = serde_json::to_value(&reports)?;
    if op == "sync" {
        let mut summary = with_name_identity(
            serde_json::to_value(wallet.summary_with_confirmation_target(
                &scanned,
                request["confirmation_target"].as_u64().unwrap_or(10),
            )?)?,
            &wallet,
        )?;
        summary["light_sources"] = reports;
        return Ok(summary);
    }
    exclude_pending_spends(&clients[synced_from], &wallet, &mut scanned).await?;
    let to = field(request, "to")?.trim();
    ensure!(
        !names_a_username(network, to),
        "usernames are not resolved in light mode yet; use an address"
    );
    let address = decode_address(network, to).with_context(|| not_a_recipient(network))?;
    let amount = field(request, "amount_atomic")?.parse::<u64>()?;
    let fee = request["fee_atomic"]
        .as_str()
        .unwrap_or("10000000")
        .parse::<u64>()?;
    let tx = wallet.send(
        &scanned,
        address,
        amount,
        fee,
        request["memo"].as_str().unwrap_or(""),
    )?;
    let (via, answer) = genoryn::light::relay(&clients, synced_from, &tx).await?;
    Ok(json!({
        "txid": answer["txid"].clone(),
        "status": "relayed through a remote node; unconfirmed",
        "relayed_via": sources[via],
        "light_sources": reports,
    }))
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
        std::io::stdin().take(65_537).read_to_string(&mut input)?;
        ensure!(input.len() <= 65_536, "wallet request too large");
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

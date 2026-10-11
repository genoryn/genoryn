//! Payouts: matured balances leave the pool wallet in ordinary shielded
//! transfers of up to 15 recipients each.
//!
//! The pool wallet is used only through `genoryn-wallet-cli` in its own
//! process: the gateway never holds its keys or password. Each batch is
//! planned in the ledger first (balances move to pending in the same
//! transaction), then paid with `send_many` under the batch id as its
//! reference: the wallet pays a reference at most once, so a crash or restart
//! at any step re-sends the same stored transaction instead of paying again.
//! A batch counts as paid only when the wallet sees its transaction confirmed
//! on the validated chain, and returns to the balances only when the wallet
//! sees that it can never confirm (expired, or its notes spent elsewhere).
use super::*;
use zeroize::Zeroizing;

#[derive(ClapArgs)]
pub struct PayoutArgs {
    #[command(flatten)]
    ledger: LedgerArgs,
    #[command(flatten)]
    node: NodeArgs,
    /// The pool wallet that receives every pool block's coinbase.
    #[arg(long)]
    wallet: PathBuf,
    /// File holding the pool wallet's password (readable only by the operator).
    #[arg(long)]
    password_file: PathBuf,
    #[arg(long, default_value = "")]
    bin_dir: String,
    #[arg(long, default_value_t = 60)]
    interval_seconds: u64,
    /// Open (unconfirmed) payout transactions at most.
    #[arg(long, default_value_t = 8)]
    max_open_batches: usize,
    #[arg(long)]
    once: bool,
}

struct WalletCli {
    binary: PathBuf,
    network: Network,
    wallet: PathBuf,
    password: Zeroizing<String>,
    node: String,
    token: PathBuf,
}
impl WalletCli {
    fn call(&self, op: &str, extra: Value) -> Result<Value> {
        let mut request = json!({
            "op": op,
            "network": self.network.name(),
            "wallet_path": self.wallet,
            "password": self.password.as_str(),
            "rpc_url": self.node,
            "token_file": self.token,
        });
        if let (Some(target), Value::Object(fields)) = (request.as_object_mut(), extra) {
            target.extend(fields);
        }
        let input = Zeroizing::new(serde_json::to_vec(&request)?);
        let mut child = std::process::Command::new(&self.binary)
            .arg("--json-stdio")
            .stdin(std::process::Stdio::piped())
            .stdout(std::process::Stdio::piped())
            .stderr(std::process::Stdio::piped())
            .spawn()
            .context("cannot start genoryn-wallet-cli")?;
        std::io::Write::write_all(child.stdin.as_mut().context("wallet stdin")?, &input)?;
        drop(child.stdin.take());
        let output = child.wait_with_output()?;
        let answer: Value = serde_json::from_slice(&output.stdout)
            .with_context(|| format!("wallet {op} gave no answer"))?;
        ensure!(
            answer["ok"] == true,
            "wallet {op} refused: {}",
            answer["error"].as_str().unwrap_or("unknown error")
        );
        Ok(answer["result"].clone())
    }
}

fn wallet_binary(bin_dir: &str) -> PathBuf {
    let name = format!("genoryn-wallet-cli{}", std::env::consts::EXE_SUFFIX);
    if !bin_dir.is_empty() {
        return PathBuf::from(bin_dir).join(name);
    }
    std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(&name)))
        .unwrap_or_else(|| PathBuf::from(name))
}

/// One pass: settle open batches, plan new ones, pay planned ones.
fn pass(
    store: &mut Store,
    policy: &PoolPolicy,
    wallet: &WalletCli,
    max_open: usize,
) -> Result<Value> {
    let now = unix_time();
    let synced = wallet.call("sync", json!({}))?;
    let outbox: Vec<Value> = synced["outbox"].as_array().cloned().unwrap_or_default();
    let mut settled = Vec::new();
    for batch in store.open_batches()? {
        let Some(txid) = &batch.txid else { continue };
        let Some(entry) = outbox.iter().find(|entry| entry["txid"] == *txid) else {
            continue;
        };
        match entry["status"].as_str() {
            Some("Confirmed") => {
                let height = entry["confirmed_height"].as_u64().unwrap_or(0);
                store.batch_confirmed(&batch.id, height, now)?;
                settled.push(json!({"batch": batch.id, "confirmed_height": height}));
            }
            Some("Expired" | "Conflicted") => {
                store.batch_failed(&batch.id, now)?;
                settled.push(json!({"batch": batch.id, "failed": entry["status"]}));
            }
            _ => {}
        }
    }
    let open = store.open_batches()?;
    let mut planned = Vec::new();
    if open.len() < max_open {
        let spendable: u64 = synced["spendable_atomic"]
            .as_str()
            .unwrap_or("0")
            .parse()
            .unwrap_or(0);
        // Submitted batches already hold their notes (the wallet's spendable
        // amount excludes them); planned ones not yet sent still need funds.
        let unsent: u64 = open
            .iter()
            .filter(|batch| batch.status == "planned")
            .map(|batch| batch.fee + batch.payouts.iter().map(|(_, amount)| amount).sum::<u64>())
            .sum();
        planned = store.plan_payouts(
            policy,
            spendable.saturating_sub(unsent),
            max_open - open.len(),
            now,
        )?;
    }
    let mut sent = Vec::new();
    for batch in store
        .open_batches()?
        .into_iter()
        .filter(|b| b.status == "planned")
    {
        let outputs: Vec<Value> = batch
            .payouts
            .iter()
            .map(|(to, amount)| json!({"to": to, "amount_atomic": amount.to_string(), "memo": "GENORYN pool payout"}))
            .collect();
        match wallet.call(
            "send_many",
            json!({"outputs": outputs, "fee_atomic": batch.fee.to_string(), "reference": batch.id}),
        ) {
            Ok(answer) => {
                let txid = answer["txid"].as_str().context("payout txid")?;
                store.batch_submitted(&batch.id, txid, now)?;
                sent.push(json!({"batch": batch.id, "txid": txid, "recipients": batch.payouts.len(), "reused": answer["reused"]}));
            }
            // Not paid and not stored: it stays planned and is tried again.
            Err(error) => {
                // A transfer spends at most 16 notes and every block reward is
                // one: merge the pool wallet's notes, then retry next pass.
                if error.to_string().contains("more than 16") {
                    let _ = wallet.call("combine_notes", json!({"max_transactions": 4}));
                }
                sent.push(json!({"batch": batch.id, "deferred": error.to_string()}))
            }
        }
    }
    Ok(
        json!({"event": "payouts", "settled": settled, "planned": planned.len(), "sent": sent, "audit": store.audit()?}),
    )
}

pub async fn run(args: PayoutArgs) -> Result<()> {
    let network = args.ledger.network()?;
    let (_rpc, tip) = node_client(&args.node, network).await?;
    let (mut store, policy) = open_ledger(&args.ledger, tip)?;
    ensure!(
        std::fs::metadata(&args.password_file)?.len() <= 4_096,
        "password file too large"
    );
    let password = Zeroizing::new(
        std::fs::read_to_string(&args.password_file)?
            .trim_end_matches(['\r', '\n'])
            .to_owned(),
    );
    let wallet = WalletCli {
        binary: wallet_binary(&args.bin_dir),
        network,
        wallet: args.wallet.clone(),
        password,
        node: args.node.node.clone(),
        token: args.node.token_file.clone(),
    };
    loop {
        let result = tokio::task::block_in_place(|| {
            pass(&mut store, &policy, &wallet, args.max_open_batches.max(1))
        });
        match result {
            Ok(report) => println!("{report}"),
            Err(error) => eprintln!("payout pass failed (nothing is lost; retrying): {error:#}"),
        }
        if args.once {
            return Ok(());
        }
        tokio::time::sleep(Duration::from_secs(args.interval_seconds.max(5))).await;
    }
}

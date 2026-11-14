//! krypton-wallet-cli: command-line wallet. Light mode by default: it talks to
//! one or more nodes, verifies everything it receives, and keeps keys local.

use anyhow::{Context, Result, bail};
use clap::{Parser, Subcommand};
use krypton_core::Network;
use krypton_core::address::Address;
use krypton_core::emission::{format_amount, parse_amount};
use krypton_crypto::kdf::Argon2Params;
use krypton_light::client::NodeClient;
use krypton_wallet_core::{ChainSource, DEFAULT_FEE, Wallet, keys};
use std::path::PathBuf;

#[derive(Parser)]
#[command(
    name = "krypton-wallet-cli",
    version,
    about = "Genoryn Krypton wallet (quantum-resistant, private by default)"
)]
struct Cli {
    /// Wallet file
    #[arg(long, global = true, env = "KRYPTON_WALLET")]
    wallet: Option<PathBuf>,
    /// Node API URL(s). Several sources are cross-checked; the first is used for sync.
    #[arg(
        long = "node",
        global = true,
        env = "KRYPTON_NODE",
        value_delimiter = ','
    )]
    nodes: Vec<String>,
    #[arg(
        long,
        global = true,
        default_value = "mainnet",
        env = "KRYPTON_NETWORK"
    )]
    network: String,
    #[command(subcommand)]
    cmd: Cmd,
}

#[derive(Subcommand)]
enum Cmd {
    /// Create a new wallet and show its 24-word recovery phrase once
    New,
    /// Restore a wallet from a recovery phrase (asks for the words)
    Restore {
        /// First block to scan (0 = full history)
        #[arg(long, default_value_t = 0)]
        birthday: u64,
    },
    /// Show the receiving address
    Address {
        /// Publish the address so the short form resolves on that node
        #[arg(long)]
        publish: bool,
    },
    /// Synchronize (verified light sync)
    Sync,
    /// Show the balance
    Balance,
    /// Send GNR: <address or short address> <amount in GNR>
    Send {
        to: String,
        amount: String,
        #[arg(long, default_value = "")]
        memo: String,
        /// Fee in GNR (default: the node's "normal" estimate for the current backlog)
        #[arg(long)]
        fee: Option<String>,
    },
    /// Merge the two largest notes (a transaction spends at most two)
    Consolidate,
    /// Transaction history
    History,
    /// Owned notes
    Notes,
    /// Show the recovery phrase (asks for the password)
    ShowPhrase,
    /// Forget unconfirmed spends (after they expired)
    ResetPending,
    /// Node status of every configured source
    Status,
    /// Create a payment proof for one of your sends (txid from `history`)
    ProvePayment { txid: String },
    /// Check a payment proof against the chain
    VerifyPayment { proof: String },
}

fn default_wallet(network: Network) -> PathBuf {
    let base = std::env::var_os("APPDATA")
        .map(|a| PathBuf::from(a).join("GenorynKrypton"))
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".genoryn-krypton")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join(network.as_str()).join("wallet.krypton")
}

fn password(prompt: &str) -> Result<String> {
    if let Ok(p) = std::env::var("KRYPTON_WALLET_PASSWORD") {
        return Ok(p);
    }
    Ok(rpassword::prompt_password(prompt)?)
}

fn main() -> Result<()> {
    let cli = Cli::parse();
    let network = Network::parse(&cli.network).context("unknown network")?;
    let path = cli
        .wallet
        .clone()
        .unwrap_or_else(|| default_wallet(network));
    let nodes: Vec<String> = if cli.nodes.is_empty() {
        vec![format!("http://127.0.0.1:{}", network.params().rpc_port)]
    } else {
        cli.nodes.clone()
    };
    let clients: Vec<NodeClient> = nodes
        .iter()
        .map(|n| NodeClient::new(n))
        .collect::<Result<_>>()?;
    let src = &clients[0];

    match cli.cmd {
        Cmd::New | Cmd::Restore { .. } => {
            if let Some(d) = path.parent() {
                std::fs::create_dir_all(d)?;
            }
            let (mnemonic, birthday) = if let Cmd::Restore { birthday } = cli.cmd {
                let m = rpassword::prompt_password("Recovery phrase (24 words): ")?;
                keys::validate_mnemonic(&m)?;
                (zeroize::Zeroizing::new(m), birthday)
            } else {
                let tip = src.tip_height().unwrap_or(0);
                (keys::new_mnemonic(), tip)
            };
            let p1 = password("New wallet password: ")?;
            if std::env::var("KRYPTON_WALLET_PASSWORD").is_err()
                && password("Repeat password: ")? != p1
            {
                bail!("passwords do not match");
            }
            let kdf = if std::env::var("KRYPTON_FAST_KDF").is_ok() {
                Argon2Params::TEST
            } else {
                Argon2Params::STRONG
            };
            let w = Wallet::create(&path, &p1, &mnemonic, "", network, birthday, kdf)?;
            if matches!(cli.cmd, Cmd::New) {
                println!("\nRECOVERY PHRASE: write these 24 words on paper and keep them offline.");
                println!("Anyone with them controls the funds. They are shown only now.\n");
                for (i, word) in mnemonic.split_whitespace().enumerate() {
                    print!("{:>2}. {:<12}", i + 1, word);
                    if i % 4 == 3 {
                        println!();
                    }
                }
                println!();
            }
            println!("wallet: {}", path.display());
            println!("short address: {}", w.address().short());
            println!("address:\n{}", w.address().encode());
        }
        Cmd::Address { publish } => {
            let w = Wallet::open(&path, &password("Wallet password: ")?)?;
            let a = w.address();
            if publish {
                for c in &clients {
                    match c.publish_address(&a.encode()) {
                        Ok(s) => println!("published on {} as {s}", c.base),
                        Err(e) => eprintln!("{}: {e}", c.base),
                    }
                }
            }
            println!("short: {}", a.short());
            println!("{}", a.encode());
        }
        Cmd::Sync | Cmd::Balance | Cmd::History | Cmd::Notes => {
            let mut w = Wallet::open(&path, &password("Wallet password: ")?)?;
            if matches!(cli.cmd, Cmd::Sync | Cmd::Balance) {
                cross_check(&clients)?;
                let r = w.sync(src, |h, tip| {
                    if h % 500 == 0 || h == tip {
                        eprintln!("  verified {h}/{tip}");
                    }
                })?;
                if r.reset {
                    eprintln!(
                        "  the network runs a different genesis than this wallet last saw; rescanned from the birthday"
                    );
                }
                eprintln!(
                    "synced {} -> {} ({} new notes, {} spent, reorg {})",
                    r.from, r.to, r.new_notes, r.spent_notes, r.reorged
                );
            }
            match cli.cmd {
                Cmd::History => {
                    for h in w.history() {
                        let sign = if h.amount < 0 { "-" } else { "+" };
                        println!(
                            "{:>10}  {}{:<18} {:<12} {}{}",
                            h.height.map(|x| x.to_string()).unwrap_or("pending".into()),
                            sign,
                            format_amount(h.amount.unsigned_abs() as u64),
                            h.to.as_deref().map(|s| &s[..s.len().min(12)]).unwrap_or(""),
                            h.memo,
                            if h.disclosure.is_some() {
                                format!("  [provable: {}]", h.txid)
                            } else {
                                String::new()
                            }
                        );
                    }
                }
                Cmd::Notes => {
                    for n in w.notes() {
                        println!(
                            "{} value {} height {} pos {:?} spent {:?} {}",
                            &hex::encode(n.cm)[..12],
                            format_amount(n.value),
                            n.height,
                            n.position,
                            n.spent_height,
                            if n.coinbase { "coinbase" } else { "" }
                        );
                    }
                }
                _ => {
                    let b = w.balance();
                    println!("spendable  {} GNR", format_amount(b.spendable));
                    println!("pending    {} GNR", format_amount(b.pending));
                    println!(
                        "immature   {} GNR (mining rewards mature after {} blocks)",
                        format_amount(b.immature),
                        network.params().coinbase_maturity
                    );
                    println!("total      {} GNR", format_amount(b.total));
                }
            }
        }
        Cmd::Send {
            to,
            amount,
            memo,
            fee,
        } => {
            let mut w = Wallet::open(&path, &password("Wallet password: ")?)?;
            cross_check(&clients)?;
            w.sync(src, |_, _| {})?;
            let to_full = if to.contains("s1") && to.len() < 120 {
                src.resolve_short(&to)?
            } else {
                to
            };
            let addr = Address::decode(&to_full)?;
            let amount = parse_amount(&amount).context("invalid amount")?;
            let fee = fee
                .map(|f| parse_amount(&f).context("invalid fee"))
                .transpose()?
                .unwrap_or_else(|| suggested_fee(&clients));
            eprintln!(
                "proving shielded transfer of {} GNR to {} (fee {} GNR) ...",
                format_amount(amount),
                addr.short(),
                format_amount(fee)
            );
            let id = w.send_auto(&Multi(&clients), &addr, amount, fee, &memo, |s| {
                eprintln!("{s}")
            })?;
            println!("sent: {}", hex::encode(id));
        }
        Cmd::Consolidate => {
            let mut w = Wallet::open(&path, &password("Wallet password: ")?)?;
            w.sync(src, |_, _| {})?;
            for id in w.consolidate(&Multi(&clients), suggested_fee(&clients), 64)? {
                println!("sent: {}", hex::encode(id));
            }
        }
        Cmd::ShowPhrase => {
            let w = Wallet::open(&path, &password("Wallet password: ")?)?;
            println!("{}", w.mnemonic().as_str());
        }
        Cmd::ResetPending => {
            let mut w = Wallet::open(&path, &password("Wallet password: ")?)?;
            w.reset_pending()?;
            println!("pending spends cleared");
        }
        Cmd::ProvePayment { txid } => {
            let w = Wallet::open(&path, &password("Wallet password: ")?)?;
            println!("{}", w.payment_proof(txid.trim())?);
            eprintln!(
                "give this proof to anyone who should check the payment; it reveals only this one payment"
            );
        }
        Cmd::VerifyPayment { proof } => {
            cross_check(&clients)?;
            let p = krypton_wallet_core::proof::PaymentProof::decode(&proof)?;
            let me = Wallet::open(&path, &password("Wallet password (Enter to skip): ")?)
                .ok()
                .map(|w| w.address());
            let params = p.network.params();
            let pow = krypton_pow::Verifier::new(
                params.pow,
                krypton_core::consensus::genesis_block(&params).hash(),
            );
            let r = krypton_wallet_core::proof::verify_for(&p, src, me.as_ref(), &pow)?;
            println!("VALID payment proof");
            println!("amount         {} GNR", format_amount(r.value));
            println!("recipient      {}", r.recipient);
            println!("memo           {}", r.memo);
            println!(
                "transaction    {} (block {}, {} confirmations)",
                r.txid, r.height, r.confirmations
            );
            match (r.recipient_confirmed, r.recipient_verified) {
                (Some(true), _) => println!("paid to        THIS wallet"),
                (_, true) => println!("paid to        the address above (verified)"),
                _ => println!(
                    "paid to        UNVERIFIED: the payment is real, but the recipient address above\n               \
                     is only a claim. The recipient can confirm it with their wallet, or publish\n               \
                     their address (`address --publish`) so anyone can check it."
                ),
            }
        }
        Cmd::Status => {
            for c in &clients {
                match c.info() {
                    Ok(i) => println!(
                        "{}: {} height {} tip {} peers {} mempool {}",
                        c.base,
                        i.network,
                        i.height,
                        &i.tip[..16],
                        i.peers,
                        i.mempool
                    ),
                    Err(e) => println!("{}: unreachable ({e})", c.base),
                }
            }
        }
    }
    Ok(())
}

/// The node's "normal" fee for one transfer, or the built-in default.
fn suggested_fee(clients: &[NodeClient]) -> u64 {
    clients
        .iter()
        .find_map(|c| c.fee_rates().ok().flatten())
        .map(|r| krypton_wallet_core::transfer_fee(r.normal_per_kb))
        .unwrap_or(DEFAULT_FEE)
}

/// With several sources, refuse to proceed if they disagree on a common height.
fn cross_check(clients: &[NodeClient]) -> Result<()> {
    if clients.len() < 2 {
        return Ok(());
    }
    let infos: Vec<_> = clients
        .iter()
        .filter_map(|c| c.info().ok().map(|i| (c, i)))
        .collect();
    let Some(min) = infos.iter().map(|(_, i)| i.height).min() else {
        return Ok(());
    };
    let check = min.saturating_sub(2);
    let mut hashes = std::collections::HashSet::new();
    for (c, _) in &infos {
        if let Ok(h) = c.header(check) {
            hashes.insert(h.hash());
        }
    }
    if hashes.len() > 1 {
        bail!("configured nodes disagree about block {check}: possible attack or network split");
    }
    Ok(())
}

/// Reads from the first node, submits transactions through every node.
struct Multi<'a>(&'a [NodeClient]);

impl ChainSource for Multi<'_> {
    fn tip_height(&self) -> Result<u64> {
        self.0[0].tip_height()
    }
    fn header(&self, h: u64) -> Result<krypton_core::BlockHeader> {
        ChainSource::header(&self.0[0], h)
    }
    fn compact_blocks(&self, from: u64, count: u32) -> Result<Vec<krypton_light::CompactBlock>> {
        ChainSource::compact_blocks(&self.0[0], from, count)
    }
    fn frontier(&self, h: u64) -> Result<krypton_core::tree::Frontier> {
        ChainSource::frontier(&self.0[0], h)
    }
    fn witness(&self, pos: u64) -> Result<(Vec<krypton_crypto::Digest>, krypton_crypto::Digest)> {
        ChainSource::witness(&self.0[0], pos)
    }
    fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        known: Option<&krypton_core::BlockHeader>,
    ) -> Result<Option<krypton_core::BlockHeader>> {
        ChainSource::verified_tip(&self.0[0], params, pow, known)
    }
    fn header_in(
        &self,
        h: &krypton_core::BlockHeader,
        tip: &krypton_core::BlockHeader,
    ) -> Result<bool> {
        ChainSource::header_in(&self.0[0], h, tip)
    }
    fn tip_work(&self, tip: &krypton_core::BlockHeader) -> Result<Option<u128>> {
        ChainSource::tip_work(&self.0[0], tip)
    }
    fn submit(&self, tx: &krypton_core::Transaction) -> Result<krypton_core::Hash32> {
        let mut last_err = None;
        let mut ok = None;
        for c in self.0 {
            match ChainSource::submit(c, tx) {
                Ok(id) => ok = Some(id),
                Err(e) => last_err = Some(e),
            }
        }
        ok.ok_or_else(|| {
            last_err.unwrap_or_else(|| anyhow::anyhow!("no node accepted the transaction"))
        })
    }
}

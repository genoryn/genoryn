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
    /// Krypton coins: private coins issued on Genoryn Krypton (fees are paid in GNR)
    Coin {
        #[command(subcommand)]
        cmd: CoinCmd,
    },
}

#[derive(Subcommand)]
enum CoinCmd {
    /// Every coin on chain
    List,
    /// One coin (ticker or id): its manifest, supply and risks
    Info { coin: String },
    /// This wallet's coin balances
    Balance,
    /// Send a coin: <coin> <address or short address> <amount>
    Send {
        coin: String,
        to: String,
        amount: String,
        #[arg(long, default_value = "")]
        memo: String,
        #[arg(long)]
        fee: Option<String>,
    },
    /// Write the manifest of a new coin, with this wallet's issuer key, to a file
    New {
        /// Name ending in " Krypton", e.g. "Dollar Krypton"
        #[arg(long)]
        name: String,
        /// Ticker ending in K, e.g. USDK
        #[arg(long)]
        ticker: String,
        #[arg(long, default_value_t = 2)]
        decimals: u8,
        /// fixed (issued once, exactly --cap) or mintable
        #[arg(long, default_value = "mintable")]
        supply: String,
        /// Cap in whole coins (none: no cap; required for fixed)
        #[arg(long)]
        cap: Option<String>,
        /// Holders may burn their coins
        #[arg(long)]
        burn: bool,
        /// The issuer may pause all transfers (makes transfers show the coin)
        #[arg(long)]
        pause: bool,
        /// none, issuer-reserves or bridge
        #[arg(long, default_value = "none")]
        backing: String,
        #[arg(long)]
        origin: Option<String>,
        #[arg(long)]
        reserves: Option<String>,
        #[arg(long)]
        website: Option<String>,
        #[arg(long)]
        terms: Option<String>,
        /// SHA-256 of the logo file (hex)
        #[arg(long)]
        logo: Option<String>,
        #[arg(long)]
        out: PathBuf,
    },
    /// Deploy a coin from its manifest file (pays the 10 GNR deploy fee)
    Deploy { manifest: PathBuf },
    /// Issue new units of a coin you issue: <coin> <address> <amount>
    Issue {
        coin: String,
        to: String,
        amount: String,
    },
    /// Burn units of a coin you hold: <coin> <amount>
    Burn { coin: String, amount: String },
    /// Stop every transfer of a pausable coin you issue
    Pause { coin: String },
    /// Resume the transfers of a paused coin you issue
    Unpause { coin: String },
    /// This wallet's issuer key: its fingerprint (as reserved tickers pin it) and the key
    Issuer,
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
        Cmd::Coin { cmd } => coin_cmd(cmd, &path, &clients)?,
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

fn resolve_to(src: &NodeClient, to: String) -> Result<Address> {
    let full = if to.contains("s1") && to.len() < 120 {
        src.resolve_short(&to)?
    } else {
        to
    };
    Ok(Address::decode(&full)?)
}

fn coin_cmd(cmd: CoinCmd, path: &std::path::Path, clients: &[NodeClient]) -> Result<()> {
    use krypton_assets::{format_units, parse_units};
    use krypton_core::asset::{Backing, Manifest, Supply};
    let src = &clients[0];
    let open_synced = || -> Result<Wallet> {
        let mut w = Wallet::open(path, &password("Wallet password: ")?)?;
        cross_check(clients)?;
        w.sync(src, |_, _| {})?;
        Ok(w)
    };
    let fee = suggested_fee(clients);
    match cmd {
        CoinCmd::List => {
            let v = src.assets()?;
            if v["active"] != serde_json::json!(true) {
                println!("Krypton coins are not active on this network yet.");
            }
            for a in v["assets"].as_array().cloned().unwrap_or_default() {
                let d = a["decimals"].as_u64().unwrap_or(0) as u8;
                println!(
                    "{:<8} {:<28} outstanding {:>22}  {}{}",
                    a["ticker"].as_str().unwrap_or(""),
                    a["name"].as_str().unwrap_or(""),
                    format_units(a["outstanding"].as_u64().unwrap_or(0), d),
                    a["id"].as_str().unwrap_or(""),
                    if a["paused"] == serde_json::json!(true) {
                        "  PAUSED"
                    } else {
                        ""
                    }
                );
            }
        }
        CoinCmd::Info { coin } => {
            let (id, m, v) = src.asset(&coin)?;
            println!("{} ({})  id {}", m.name, m.ticker, id);
            println!(
                "issued {}  burned {}  outstanding {}{}",
                format_units(v["issued"].as_u64().unwrap_or(0), m.decimals),
                format_units(v["burned"].as_u64().unwrap_or(0), m.decimals),
                format_units(v["outstanding"].as_u64().unwrap_or(0), m.decimals),
                if v["paused"] == serde_json::json!(true) {
                    "  (PAUSED)"
                } else {
                    ""
                }
            );
            println!("\nWhat the issuer can do (declared at deploy, unchangeable):");
            for r in m.risk_labels(false) {
                println!("  - {r}");
            }
            println!(
                "\nmanifest:\n{}",
                m.canonical()
                    .lines()
                    .filter(|l| !l.starts_with("issuer:"))
                    .collect::<Vec<_>>()
                    .join("\n")
            );
        }
        CoinCmd::Balance => {
            let w = open_synced()?;
            let bs = w.coin_balances();
            if bs.is_empty() {
                println!("no coins");
            }
            for b in bs {
                match src.asset(&b.coin.to_hex()) {
                    Ok((_, m, _)) => println!(
                        "{:<8} {:>22} spendable  {:>18} pending  ({})",
                        m.ticker,
                        format_units(b.spendable, m.decimals),
                        format_units(b.pending, m.decimals),
                        m.name
                    ),
                    Err(_) => println!(
                        "{}  {} base units (coin unknown to the node)",
                        b.coin, b.spendable
                    ),
                }
            }
        }
        CoinCmd::Send {
            coin,
            to,
            amount,
            memo,
            fee: f,
        } => {
            let (id, m, _) = src.asset(&coin)?;
            let amount = parse_units(&amount, m.decimals).context("invalid amount")?;
            let f = f
                .map(|f| parse_amount(&f).context("invalid fee"))
                .transpose()?
                .unwrap_or(fee);
            let to = resolve_to(src, to)?;
            let mut w = open_synced()?;
            eprintln!(
                "proving a private transfer of {} {} (fee {} GNR) ...",
                format_units(amount, m.decimals),
                m.ticker,
                format_amount(f)
            );
            let id = w.send_coin(&Multi(clients), id, &to, amount, f, &memo)?;
            println!("sent: {}", hex::encode(id));
        }
        CoinCmd::New {
            name,
            ticker,
            decimals,
            supply,
            cap,
            burn,
            pause,
            backing,
            origin,
            reserves,
            website,
            terms,
            logo,
            out,
        } => {
            let w = Wallet::open(path, &password("Wallet password: ")?)?;
            let supply = match supply.as_str() {
                "fixed" => Supply::Fixed,
                "mintable" => Supply::Mintable,
                _ => bail!("--supply is fixed or mintable"),
            };
            let backing = match backing.as_str() {
                "none" => Backing::None,
                "issuer-reserves" => Backing::IssuerReserves,
                "bridge" => Backing::Bridge,
                _ => bail!("--backing is none, issuer-reserves or bridge"),
            };
            let cap = cap
                .map(|c| parse_units(&c, decimals).context("invalid cap"))
                .transpose()?;
            let logo = logo
                .map(|h| -> Result<[u8; 32]> {
                    hex::decode(h.trim())?
                        .try_into()
                        .map_err(|_| anyhow::anyhow!("--logo is 64 hex characters"))
                })
                .transpose()?;
            let m = Manifest {
                name,
                ticker,
                decimals,
                supply,
                cap,
                burn,
                pause,
                backing,
                origin,
                reserves,
                website,
                terms,
                logo,
                issuer: w.issuer_key(),
            };
            m.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
            std::fs::write(&out, m.canonical())?;
            println!("manifest written to {} (coin id {})", out.display(), m.id());
            println!("\nAnyone holding this coin will see, before receiving it:");
            for r in m.risk_labels(false) {
                println!("  - {r}");
            }
            println!(
                "\nNothing in it can ever change. Deploy it with `coin deploy {}`.",
                out.display()
            );
        }
        CoinCmd::Deploy { manifest } => {
            let text = std::fs::read_to_string(&manifest)?;
            let m = Manifest::parse(&text).map_err(|e| anyhow::anyhow!("{e}"))?;
            m.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
            let mut w = open_synced()?;
            eprintln!(
                "deploying {} ({}), paying the 10 GNR deploy fee ...",
                m.name, m.ticker
            );
            let id = w.deploy_coin(&Multi(clients), &m, fee)?;
            println!("sent: {}  (coin id {})", hex::encode(id), m.id());
        }
        CoinCmd::Issue { coin, to, amount } => {
            let (id, m, _) = src.asset(&coin)?;
            let amount = parse_units(&amount, m.decimals).context("invalid amount")?;
            let to = resolve_to(src, to)?;
            let mut w = open_synced()?;
            let txid = w.issue_coin(&Multi(clients), id, &to, amount, fee)?;
            println!(
                "issued {} {}: {}",
                format_units(amount, m.decimals),
                m.ticker,
                hex::encode(txid)
            );
        }
        CoinCmd::Burn { coin, amount } => {
            let (id, m, _) = src.asset(&coin)?;
            let amount = parse_units(&amount, m.decimals).context("invalid amount")?;
            let mut w = open_synced()?;
            let txid = w.burn_coin(&Multi(clients), id, amount, fee)?;
            println!(
                "burned {} {}: {}",
                format_units(amount, m.decimals),
                m.ticker,
                hex::encode(txid)
            );
        }
        CoinCmd::Issuer => {
            let w = Wallet::open(path, &password("Wallet password: ")?)?;
            let key = w.issuer_key();
            println!(
                "fingerprint {}",
                hex::encode(krypton_core::asset::issuer_fingerprint(&key))
            );
            println!("key {}", hex::encode(key));
        }
        CoinCmd::Pause { coin } => {
            let (id, m, _) = src.asset(&coin)?;
            let mut w = open_synced()?;
            println!(
                "paused {}: {}",
                m.ticker,
                hex::encode(w.set_coin_paused(&Multi(clients), id, true, fee)?)
            );
        }
        CoinCmd::Unpause { coin } => {
            let (id, m, _) = src.asset(&coin)?;
            let mut w = open_synced()?;
            println!(
                "resumed {}: {}",
                m.ticker,
                hex::encode(w.set_coin_paused(&Multi(clients), id, false, fee)?)
            );
        }
    }
    Ok(())
}

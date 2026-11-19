//! The Console tab: questions to the chain and to the open wallet, typed as commands.
//!
//! Every command only reads. None can send GNR, show a recovery phrase or a key, or change
//! a setting, so a command that someone tells a user to paste here cannot take their coins.
//! Arguments are checked before they become part of a request to a node, and a node's
//! answer is cut to size here and shown by the page as plain text.

use crate::{App, S, active_view, blocking, check, err, resolve_short_any, vault};
use anyhow::{Context, Result, anyhow, bail, ensure};
use axum::Json;
use axum::extract::State;
use axum::http::HeaderMap;
use axum::response::{IntoResponse, Response};
use krypton_core::address::Address;
use krypton_core::emission::format_amount;
use serde::Deserialize;
use serde_json::{Value, json};
use std::sync::Arc;

/// The longest line taken: a full address is about 2,600 characters.
const MAX_LINE: usize = 4_096;
/// The most a node may answer to one command.
const MAX_ANSWER: u64 = 2 << 20;

pub struct Cmd {
    pub name: &'static str,
    pub args: &'static str,
    pub group: &'static str,
    pub about: &'static str,
}

const fn cmd(
    name: &'static str,
    args: &'static str,
    group: &'static str,
    about: &'static str,
) -> Cmd {
    Cmd {
        name,
        args,
        group,
        about,
    }
}

pub const COMMANDS: &[Cmd] = &[
    cmd(
        "getblockchaininfo",
        "",
        "Blockchain",
        "Height, tip, difficulty, supply and hashrate, as the node reports them.",
    ),
    cmd(
        "getblockcount",
        "",
        "Blockchain",
        "The height of the newest block.",
    ),
    cmd(
        "getbestblockhash",
        "",
        "Blockchain",
        "The hash of the newest block.",
    ),
    cmd(
        "getblockhash",
        "<height>",
        "Blockchain",
        "The hash of the block at a height.",
    ),
    cmd(
        "getblock",
        "<height|hash>",
        "Blockchain",
        "A block: header, transactions, shares and payouts.",
    ),
    cmd(
        "getblockheader",
        "<height>",
        "Blockchain",
        "The header of the block at a height.",
    ),
    cmd(
        "getrecentblocks",
        "[count]",
        "Blockchain",
        "The newest blocks, newest first (10 unless you say how many, at most 100).",
    ),
    cmd(
        "getdifficulty",
        "",
        "Blockchain",
        "The difficulty of the next block.",
    ),
    cmd(
        "getsupply",
        "",
        "Blockchain",
        "GNR issued so far and what is still to be mined.",
    ),
    cmd(
        "gettransaction",
        "<txid>",
        "Transactions",
        "Whether a transaction is confirmed, its block and its fee. Amounts and parties are hidden on chain.",
    ),
    cmd(
        "getmempoolinfo",
        "",
        "Transactions",
        "Transactions waiting for a block.",
    ),
    cmd(
        "getnullifier",
        "<nullifier>",
        "Transactions",
        "Whether a nullifier is spent, and in which block.",
    ),
    cmd(
        "estimatefee",
        "",
        "Transactions",
        "The fee rates the node suggests, in krypton per KB.",
    ),
    cmd(
        "getnetworkhashps",
        "",
        "Mining",
        "The network's estimated hashes per second.",
    ),
    cmd(
        "getshares",
        "",
        "Mining",
        "Shares waiting for the next block, and the share rules.",
    ),
    cmd(
        "getminerinfo",
        "[address|owner]",
        "Mining",
        "Unpaid rewards and recent shares of a miner: this wallet when you name none. Mining income is public per miner.",
    ),
    cmd(
        "getmininginfo",
        "",
        "Mining",
        "This computer's miner: running or not, device and hashrate.",
    ),
    cmd(
        "getpeerinfo",
        "",
        "Network",
        "The peers of the node this wallet asks.",
    ),
    cmd(
        "getconnectioncount",
        "",
        "Network",
        "How many peers that node has.",
    ),
    cmd(
        "getnetworkinfo",
        "",
        "Network",
        "How this wallet is connected: light or full node, which nodes, their version.",
    ),
    cmd(
        "validateaddress",
        "<address>",
        "Addresses",
        "Checks an address (full or short) and shows its network and short form.",
    ),
    cmd(
        "resolveaddress",
        "<short address>",
        "Addresses",
        "The full address behind a short one, checked against it.",
    ),
    cmd(
        "getaddressinfo",
        "<address>",
        "Addresses",
        "What the chain shows publicly about an address: whether its short form is published and its mining.",
    ),
    cmd(
        "getmarketinfo",
        "",
        "Market",
        "The market price and the offer board in numbers.",
    ),
    cmd(
        "getwalletinfo",
        "",
        "Wallet",
        "The open wallet: name, short address, synced height, swaps in progress.",
    ),
    cmd(
        "getbalance",
        "",
        "Wallet",
        "The open wallet's balance in GNR.",
    ),
    cmd(
        "getaddress",
        "",
        "Wallet",
        "The open wallet's short and full address.",
    ),
    cmd(
        "listtransactions",
        "[count]",
        "Wallet",
        "The open wallet's latest payments (10 unless you say how many, at most 100).",
    ),
    cmd(
        "listwallets",
        "",
        "Wallet",
        "The wallets in this app and whether each is unlocked.",
    ),
    cmd("getversion", "", "Console", "The version of this wallet."),
    cmd(
        "help",
        "[command]",
        "Console",
        "Lists the commands, or explains one.",
    ),
    cmd("clear", "", "Console", "Empties the window."),
];

fn find(name: &str) -> Option<&'static Cmd> {
    let name = match name {
        "getinfo" => "getblockchaininfo",
        "getblocks" => "getrecentblocks",
        "getfee" | "estimatesmartfee" => "estimatefee",
        "getpeers" => "getpeerinfo",
        "getmarket" => "getmarketinfo",
        "getnewaddress" => "getaddress",
        "version" => "getversion",
        "?" => "help",
        other => other,
    };
    COMMANDS.iter().find(|c| c.name == name)
}

/// The command list, for the page's help and completion.
pub async fn commands(State(app): S, h: HeaderMap) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    Json(json!({
        "commands": COMMANDS.iter().map(|c| json!({"name": c.name, "args": c.args, "group": c.group, "about": c.about})).collect::<Vec<_>>(),
    }))
    .into_response()
}

#[derive(Deserialize)]
pub struct Req {
    line: String,
}

pub async fn run(State(app): S, h: HeaderMap, Json(r): Json<Req>) -> Response {
    if let Err(x) = check(&app, &h) {
        return x;
    }
    let a = app.clone();
    match blocking(move || exec(&a, &r.line)).await {
        Ok(v) => Json(v).into_response(),
        Err(e) => err(e),
    }
}

fn height(s: &str) -> Result<u64> {
    ensure!(
        !s.is_empty() && s.len() <= 12 && s.bytes().all(|b| b.is_ascii_digit()),
        "a block height is a number, for example 21000"
    );
    Ok(s.parse()?)
}

fn hash(s: &str, what: &str) -> Result<String> {
    ensure!(
        s.len() == 64 && s.bytes().all(|b| b.is_ascii_hexdigit()),
        "{what} is 64 hexadecimal characters"
    );
    Ok(s.to_ascii_lowercase())
}

fn count(s: Option<&str>) -> Result<usize> {
    let Some(s) = s else { return Ok(10) };
    let n = height(s).map_err(|_| anyhow!("how many: a number from 1 to 100"))?;
    ensure!((1..=100).contains(&n), "how many: a number from 1 to 100");
    Ok(n as usize)
}

/// An address as typed: letters and digits only, so it can stand in a request path.
fn address(s: &str) -> Result<&str> {
    ensure!(
        s.len() >= 8 && s.len() <= 3_600 && s.bytes().all(|b| b.is_ascii_alphanumeric()),
        "this is not an address"
    );
    Ok(s)
}

/// A node's answer made safe to show: bounded in depth, length and count, and free of
/// control characters. The page prints it as text, never as markup.
fn tidy(v: Value, depth: usize) -> Value {
    match v {
        Value::String(s) => {
            Value::String(s.chars().filter(|c| !c.is_control()).take(4_096).collect())
        }
        Value::Array(a) if depth < 12 => Value::Array(
            a.into_iter()
                .take(2_000)
                .map(|x| tidy(x, depth + 1))
                .collect(),
        ),
        Value::Object(o) if depth < 12 => Value::Object(
            o.into_iter()
                .take(512)
                .map(|(k, x)| {
                    (
                        k.chars().filter(|c| !c.is_control()).take(64).collect(),
                        tidy(x, depth + 1),
                    )
                })
                .collect(),
        ),
        Value::Array(_) | Value::Object(_) => Value::String("…".into()),
        other => other,
    }
}

fn gnr(krypton: u64) -> String {
    format!("{} GNR", format_amount(krypton))
}

/// The open wallet's last snapshot: wallet commands answer from it, so they never wait
/// for a sync and never touch the keys.
fn wallet(a: &Arc<App>) -> Result<Value> {
    let id = a.vault.active_id();
    ensure!(
        a.vault.locks_in(&id).is_some() && !vault::expired(a, &id),
        "unlock the wallet first"
    );
    active_view(a).context("the wallet is still opening; try again in a moment")
}

fn exec(a: &Arc<App>, line: &str) -> Result<Value> {
    ensure!(line.len() <= MAX_LINE, "this line is too long");
    let mut words = line.split_whitespace();
    let name = words.next().unwrap_or("").to_ascii_lowercase();
    let args: Vec<&str> = words.collect();
    let c = find(&name).with_context(|| {
        format!(
            "unknown command \"{}\": type help for the list",
            name.chars().take(40).collect::<String>()
        )
    })?;
    let most = c.args.split_whitespace().count();
    ensure!(
        args.len() <= most,
        "{} takes {}: {} {}",
        c.name,
        if most == 0 {
            "no arguments".to_string()
        } else {
            format!("at most {most}")
        },
        c.name,
        c.args
    );
    let need = |i: usize| -> Result<&str> {
        args.get(i)
            .copied()
            .with_context(|| format!("usage: {} {}", c.name, c.args))
    };
    // One node answers: the wallet's own in full-node mode, else the first configured.
    let node = || -> Result<krypton_light::client::NodeClient> {
        a.sources()?
            .into_iter()
            .next()
            .context("no node configured")
    };
    let ask = |path: String| -> Result<(Value, Option<String>)> {
        let n = node()?;
        let v = n.view(&path, MAX_ANSWER)?;
        Ok((tidy(v, 0), Some(n.base_url().to_string())))
    };
    let (result, from): (Value, Option<String>) = match c.name {
        "getblockchaininfo" => ask("/v1/info".into())?,
        "getblockcount" => {
            let (v, n) = ask("/v1/info".into())?;
            (v["height"].clone(), n)
        }
        "getbestblockhash" => {
            let (v, n) = ask("/v1/info".into())?;
            (v["tip"].clone(), n)
        }
        "getdifficulty" => {
            let (v, n) = ask("/v1/info".into())?;
            (v["difficulty"].clone(), n)
        }
        "getnetworkhashps" => {
            let (v, n) = ask("/v1/info".into())?;
            (v["hashrate"].clone(), n)
        }
        "getblockhash" => {
            let (v, n) = ask(format!("/v1/header/{}", height(need(0)?)?))?;
            (v["hash"].clone(), n)
        }
        "getblockheader" => ask(format!("/v1/header/{}", height(need(0)?)?))?,
        "getblock" => {
            let id = need(0)?;
            let id = match height(id) {
                Ok(h) => h.to_string(),
                Err(_) => hash(id, "a block is named by its height or its hash; a hash")?,
            };
            ask(format!("/v1/block/{id}"))?
        }
        "getrecentblocks" => ask(format!(
            "/v1/blocks?count={}",
            count(args.first().copied())?
        ))?,
        "getsupply" => ask("/v1/supply".into())?,
        "gettransaction" => ask(format!("/v1/tx/{}", hash(need(0)?, "a transaction id")?))?,
        "getmempoolinfo" => ask("/v1/mempool".into())?,
        "getnullifier" => ask(format!("/v1/nullifier/{}", hash(need(0)?, "a nullifier")?))?,
        "estimatefee" => ask("/v1/fee".into())?,
        "getshares" => ask("/v1/shares".into())?,
        "getminerinfo" => {
            let who = match args.first() {
                Some(x) if x.len() == 64 => hash(x, "an owner id")?,
                Some(x) => {
                    let x = address(x)?;
                    if x.contains("s1") && x.len() < 120 {
                        resolve_short_any(a, x)?
                    } else {
                        x.to_string()
                    }
                }
                None => wallet(a)?["owner"]
                    .as_str()
                    .context("unlock the wallet first, or name a miner")?
                    .to_string(),
            };
            ask(format!("/v1/miner/{who}"))?
        }
        "getmininginfo" => {
            let running = a.miner_running();
            let ms = a.miner_stats.lock().clone();
            (
                json!({
                    "running": running,
                    "backend": ms.backend,
                    "device": ms.device,
                    "hashrate": if running { ms.hashrate } else { 0.0 },
                    "shares_found": ms.shares,
                    "blocks_found": ms.blocks,
                    "share_of_network": ms.network_fraction,
                    "expected_gnr_per_day": ms.expected_per_day,
                }),
                None,
            )
        }
        "getpeerinfo" => ask("/v1/peers".into())?,
        "getconnectioncount" => {
            let (v, n) = ask("/v1/peers".into())?;
            (json!(v.as_array().map_or(0, Vec::len)), n)
        }
        "getnetworkinfo" => {
            let s = a.settings.lock().clone();
            let nodes: Vec<Value> = a
                .sources()?
                .iter()
                .map(|n| match n.info() {
                    Ok(i) => json!({"node": n.base_url(), "reachable": true, "version": i.version, "height": i.height, "peers": i.peers}),
                    Err(_) => json!({"node": n.base_url(), "reachable": false}),
                })
                .collect();
            (
                tidy(
                    json!({
                        "network": a.network.as_str(),
                        "mode": if s.mode == "full" { "full node (this computer validates every block)" } else { "light (nodes provide data; this computer verifies it)" },
                        "nodes": nodes,
                    }),
                    0,
                ),
                None,
            )
        }
        "validateaddress" => {
            let x = address(need(0)?)?;
            if x.contains("s1") && x.len() < 120 {
                match Address::parse_short(x) {
                    Ok((net, _)) => (
                        json!({"valid": net == a.network, "network": net.as_str(), "short": x, "form": "short"}),
                        None,
                    ),
                    Err(e) => (json!({"valid": false, "error": e.to_string()}), None),
                }
            } else {
                match Address::decode(x) {
                    Ok(d) => (
                        json!({"valid": d.network == a.network, "network": d.network.as_str(), "short": d.short(), "form": "full"}),
                        None,
                    ),
                    Err(e) => (json!({"valid": false, "error": e.to_string()}), None),
                }
            }
        }
        "resolveaddress" => {
            let x = address(need(0)?)?;
            ensure!(
                x.contains("s1") && x.len() < 120,
                "resolveaddress takes a short address (it has s1 in it)"
            );
            let full = resolve_short_any(a, x)?;
            (json!({"short": x, "address": full}), None)
        }
        "getaddressinfo" => ask(format!("/v1/address-info/{}", address(need(0)?)?))?,
        "getmarketinfo" => {
            let (v, n) = ask("/v1/swap/market".into())?;
            (v["market"].clone(), n)
        }
        "getwalletinfo" => {
            let v = wallet(a)?;
            let reg = a.vault.reg.lock().clone();
            (
                json!({
                    "name": reg.get(&reg.active).map(|e| e.name.clone()),
                    "network": a.network.as_str(),
                    "short_address": v["short"],
                    "synced_height": v["synced_height"],
                    "payments": v["history"].as_array().map_or(0, Vec::len),
                    "swaps_in_progress": v["swaps_open"],
                    "locks_itself_in_seconds": a.vault.locks_in(&reg.active),
                }),
                None,
            )
        }
        "getbalance" => {
            let v = wallet(a)?;
            let b = &v["balance"];
            let k = |f: &str| gnr(b[f].as_u64().unwrap_or(0));
            (
                json!({
                    "spendable": k("spendable"),
                    "incoming": k("pending"),
                    "maturing_mining_rewards": k("immature"),
                    "set_aside_for_offers": k("held"),
                    "locked_in_swaps": gnr(v["swap_locked"].as_u64().unwrap_or(0)),
                    "total": k("total"),
                }),
                None,
            )
        }
        "getaddress" => {
            let v = wallet(a)?;
            (json!({"short": v["short"], "address": v["address"]}), None)
        }
        "listtransactions" => {
            let n = count(args.first().copied())?;
            let v = wallet(a)?;
            let rows: Vec<Value> = v["history"]
                .as_array()
                .map(|h| {
                    h.iter()
                        .filter(|e| !e["txid"].as_str().unwrap_or("").starts_with("self:"))
                        .take(n)
                        .map(|e| {
                            let amount = e["amount"].as_i64().unwrap_or(0);
                            json!({
                                "block": e["height"],
                                "amount": format!("{}{}", if amount < 0 { "-" } else { "+" }, gnr(amount.unsigned_abs())),
                                "fee": gnr(e["fee"].as_u64().unwrap_or(0)),
                                "memo": e["memo"],
                                "txid": e["txid"],
                            })
                        })
                        .collect()
                })
                .unwrap_or_default();
            (tidy(Value::Array(rows), 0), None)
        }
        "listwallets" => {
            let reg = a.vault.reg.lock().clone();
            (
                Value::Array(
                    reg.wallets
                        .iter()
                        .map(|e| json!({"name": e.name, "active": e.id == reg.active, "unlocked": a.vault.locks_in(&e.id).is_some()}))
                        .collect(),
                ),
                None,
            )
        }
        "getversion" => (
            json!({"wallet": env!("CARGO_PKG_VERSION"), "network": a.network.as_str()}),
            None,
        ),
        "help" => match args.first() {
            Some(x) => {
                let c = find(&x.to_ascii_lowercase())
                    .with_context(|| "no such command: type help for the list".to_string())?;
                (
                    Value::String(
                        format!("{} {}\n\n{}", c.name, c.args, c.about).replace(" \n", "\n"),
                    ),
                    None,
                )
            }
            None => {
                let mut out = String::new();
                let mut group = "";
                for c in COMMANDS {
                    if c.group != group {
                        group = c.group;
                        out.push_str(&format!("\n== {group} ==\n"));
                    }
                    out.push_str(&format!("{} {}\n", c.name, c.args).replace(" \n", "\n"));
                }
                out.push_str("\nhelp <command> explains one. Tab completes a name; the arrow keys bring back earlier lines.");
                (Value::String(out.trim_start().to_string()), None)
            }
        },
        "clear" => (Value::Null, None),
        _ => bail!("unknown command"),
    };
    Ok(json!({"command": c.name, "result": result, "node": from}))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn arguments_cannot_leave_their_place_in_a_request() {
        for bad in ["1/../../x", "12?x=1", "-1", "1e3", "", "99999999999999"] {
            assert!(height(bad).is_err(), "{bad}");
        }
        assert_eq!(height("21000").unwrap(), 21_000);
        let h = "Ab".repeat(32);
        assert_eq!(hash(&h, "x").unwrap(), "ab".repeat(32));
        for bad in ["ab", &"zz".repeat(32), &format!("{}/", "a".repeat(63))] {
            assert!(hash(bad, "x").is_err());
        }
        for bad in ["gnrs1abc/../x", "gnrs1abc?x", "gnr 1", "short"] {
            assert!(address(bad).is_err(), "{bad}");
        }
        assert!(address("gnrs1qqqqqqqqqqqq").is_ok());
        assert!(count(Some("0")).is_err() && count(Some("101")).is_err());
        assert_eq!(count(None).unwrap(), 10);
    }

    #[test]
    fn no_command_spends_reveals_or_changes() {
        for c in COMMANDS {
            for word in [
                "send", "dump", "import", "sign", "key", "phrase", "password", "set", "unlock",
                "backup", "export",
            ] {
                assert!(
                    !c.name.contains(word),
                    "{} must not be a console command",
                    c.name
                );
            }
            assert!(find(c.name).is_some());
        }
    }

    #[test]
    fn an_answer_is_cut_to_size() {
        let deep = (0..40).fold(json!("x"), |v, _| json!([v]));
        let s = serde_json::to_string(&tidy(deep, 0)).unwrap();
        assert!(s.len() < 200);
        let long = tidy(
            json!({"a": "y".repeat(10_000), "b\u{7}": "ok\u{1b}[31m"}),
            0,
        );
        assert_eq!(long["a"].as_str().unwrap().len(), 4_096);
        assert_eq!(long["b"], "ok[31m");
    }
}

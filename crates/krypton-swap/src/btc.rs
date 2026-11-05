//! Reading and writing Bitcoin for swaps: an Esplora HTTP API (mempool.space, Blockstream
//! or your own) or a Bitcoin Core node over JSON-RPC.

use anyhow::{Context, Result, anyhow, bail};
use bitcoin::consensus::encode::serialize_hex;
use bitcoin::{Address, Amount, FeeRate, Network, OutPoint, Transaction, Txid};
use serde_json::{Value, json};
use std::str::FromStr;
use std::time::Duration;

/// An output paying a watched address.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Utxo {
    pub outpoint: OutPoint,
    pub value: Amount,
    /// Height of the block that confirmed it (`None` while in the mempool).
    pub height: Option<u32>,
}

pub trait BtcChain {
    fn tip(&self) -> Result<u32>;
    /// Outputs paying `address`: at least every unspent one, confirmed or not.
    fn outputs(&self, address: &Address) -> Result<Vec<Utxo>>;
    fn is_spent(&self, op: &OutPoint) -> Result<bool>;
    fn broadcast(&self, tx: &Transaction) -> Result<Txid>;
    /// A fee rate that confirms within about half an hour.
    fn fee_rate(&self) -> Result<FeeRate>;
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("genoryn-krypton-wallet")
        .build()?)
}

/// Default public Esplora API for a network.
pub fn default_esplora(n: Network) -> &'static str {
    match n {
        Network::Bitcoin => "https://mempool.space/api",
        Network::Testnet => "https://mempool.space/testnet/api",
        Network::Testnet4 => "https://mempool.space/testnet4/api",
        Network::Signet => "https://mempool.space/signet/api",
        _ => "http://127.0.0.1:3002",
    }
}

/// `esplora:<url>` or `rpc:<url with user:password>`; a bare URL means Esplora.
pub fn connect(spec: &str) -> Result<Box<dyn BtcChain + Send + Sync>> {
    let spec = spec.trim();
    if let Some(u) = spec.strip_prefix("rpc:") {
        Ok(Box::new(CoreRpc::new(u)?))
    } else {
        Ok(Box::new(Esplora::new(
            spec.strip_prefix("esplora:").unwrap_or(spec),
        )?))
    }
}

pub struct Esplora {
    base: String,
    http: reqwest::blocking::Client,
}

impl Esplora {
    pub fn new(base: &str) -> Result<Self> {
        let base = base.trim().trim_end_matches('/').to_string();
        anyhow::ensure!(
            base.starts_with("https://")
                || base.starts_with("http://127.0.0.1")
                || base.starts_with("http://localhost"),
            "the Bitcoin API must use https (or run on this computer)"
        );
        Ok(Self {
            base,
            http: http()?,
        })
    }
    fn get(&self, path: &str) -> Result<String> {
        let r = self.http.get(format!("{}{path}", self.base)).send()?;
        let status = r.status();
        let body = r.text()?;
        if !status.is_success() {
            bail!(
                "Bitcoin API {path}: {status} {}",
                body.chars().take(200).collect::<String>()
            );
        }
        Ok(body)
    }
}

impl BtcChain for Esplora {
    fn tip(&self) -> Result<u32> {
        Ok(self.get("/blocks/tip/height")?.trim().parse()?)
    }
    fn outputs(&self, address: &Address) -> Result<Vec<Utxo>> {
        let txs: Value = serde_json::from_str(&self.get(&format!("/address/{address}/txs"))?)?;
        let spk = address.script_pubkey().to_hex_string();
        let mut out = Vec::new();
        for tx in txs.as_array().context("unexpected Bitcoin API reply")? {
            let txid = Txid::from_str(tx["txid"].as_str().context("txid")?)?;
            let height = tx["status"]["block_height"].as_u64().map(|h| h as u32);
            for (i, o) in tx["vout"].as_array().into_iter().flatten().enumerate() {
                if o["scriptpubkey"].as_str() == Some(spk.as_str()) {
                    out.push(Utxo {
                        outpoint: OutPoint::new(txid, i as u32),
                        value: Amount::from_sat(o["value"].as_u64().context("value")?),
                        height,
                    });
                }
            }
        }
        Ok(out)
    }
    fn is_spent(&self, op: &OutPoint) -> Result<bool> {
        let v: Value =
            serde_json::from_str(&self.get(&format!("/tx/{}/outspend/{}", op.txid, op.vout))?)?;
        Ok(v["spent"].as_bool().unwrap_or(false))
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        let r = self
            .http
            .post(format!("{}/tx", self.base))
            .body(serialize_hex(tx))
            .send()?;
        let status = r.status();
        let body = r.text()?;
        if !status.is_success() {
            bail!(
                "Bitcoin broadcast refused: {}",
                body.chars().take(300).collect::<String>()
            );
        }
        Ok(Txid::from_str(body.trim())?)
    }
    fn fee_rate(&self) -> Result<FeeRate> {
        let v: Value = serde_json::from_str(&self.get("/fee-estimates")?)?;
        let r = v["3"].as_f64().or_else(|| v["6"].as_f64()).unwrap_or(2.0);
        FeeRate::from_sat_per_vb(r.ceil().clamp(1.0, 2_000.0) as u64).context("fee rate")
    }
}

/// Bitcoin Core over JSON-RPC (`http://user:password@127.0.0.1:8332`).
pub struct CoreRpc {
    url: reqwest::Url,
    user: String,
    pass: String,
    http: reqwest::blocking::Client,
}

impl CoreRpc {
    pub fn new(url: &str) -> Result<Self> {
        let mut u = reqwest::Url::parse(url.trim())?;
        let user = u.username().to_string();
        let pass = u.password().unwrap_or("").to_string();
        u.set_username("").map_err(|_| anyhow!("bad RPC URL"))?;
        u.set_password(None).map_err(|_| anyhow!("bad RPC URL"))?;
        Ok(Self {
            url: u,
            user,
            pass,
            http: http()?,
        })
    }
    fn call(&self, method: &str, params: Value) -> Result<Value> {
        let r: Value = self
            .http
            .post(self.url.clone())
            .basic_auth(&self.user, Some(&self.pass))
            .json(&json!({"jsonrpc": "1.0", "id": "gk", "method": method, "params": params}))
            .send()?
            .json()?;
        if !r["error"].is_null() {
            bail!(
                "bitcoind {method}: {}",
                r["error"]["message"].as_str().unwrap_or("error")
            );
        }
        Ok(r["result"].clone())
    }
}

impl BtcChain for CoreRpc {
    fn tip(&self) -> Result<u32> {
        Ok(self
            .call("getblockcount", json!([]))?
            .as_u64()
            .context("height")? as u32)
    }
    fn outputs(&self, address: &Address) -> Result<Vec<Utxo>> {
        let r = self.call(
            "scantxoutset",
            json!(["start", [format!("addr({address})")]]),
        )?;
        r["unspents"]
            .as_array()
            .context("unexpected scantxoutset reply")?
            .iter()
            .map(|u| {
                Ok(Utxo {
                    outpoint: OutPoint::new(
                        Txid::from_str(u["txid"].as_str().context("txid")?)?,
                        u["vout"].as_u64().context("vout")? as u32,
                    ),
                    value: Amount::from_btc(u["amount"].as_f64().context("amount")?)?,
                    height: u["height"].as_u64().map(|h| h as u32),
                })
            })
            .collect()
    }
    fn is_spent(&self, op: &OutPoint) -> Result<bool> {
        Ok(self
            .call("gettxout", json!([op.txid.to_string(), op.vout, true]))?
            .is_null())
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        let r = self.call("sendrawtransaction", json!([serialize_hex(tx)]))?;
        Ok(Txid::from_str(r.as_str().context("txid")?)?)
    }
    fn fee_rate(&self) -> Result<FeeRate> {
        let r = self.call("estimatesmartfee", json!([3])).ok();
        let btc_per_kvb = r.and_then(|v| v["feerate"].as_f64()).unwrap_or(0.00002);
        let sat_vb = (btc_per_kvb * 1e8 / 1000.0).ceil().clamp(1.0, 2_000.0) as u64;
        FeeRate::from_sat_per_vb(sat_vb).context("fee rate")
    }
}

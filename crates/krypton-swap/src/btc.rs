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
    /// The transaction spending `op`, confirmed or in the mempool. `from` is a block height
    /// at or below the one that confirmed `op` (where to start looking, if needed).
    fn spender(&self, op: &OutPoint, from: u32) -> Result<Option<Transaction>>;
    fn broadcast(&self, tx: &Transaction) -> Result<Txid>;
    /// A fee rate that confirms within about half an hour.
    fn fee_rate(&self) -> Result<FeeRate>;
    /// Transaction `txid`, checked against its id, when the server gives it; `None` when it
    /// cannot. A signature commits to the amount of the output it spends only, so the amounts
    /// a wallet signs for are read from the transactions themselves where possible.
    fn tx(&self, _txid: &Txid) -> Result<Option<Transaction>> {
        Ok(None)
    }
    /// The outputs paying `address` that nothing has spent, confirmed or not.
    fn unspent(&self, address: &Address) -> Result<Vec<Utxo>> {
        unspent_of(self, address)
    }
}

/// How far back a swap's payment is looked for (blocks, four days): reports arrive within
/// hours of the swap, and a server that scans blocks for the spender (Bitcoin Core) must
/// not be sent far back for a payment that is not there.
pub const CLAIM_LOOKBACK: u32 = 576;

/// What the chain shows was taken out of the swap contract `htlc` with `secret`: the
/// bitcoin (sat) that the transaction spending `funding` pays out, if that transaction
/// spends nothing else, spends it by the contract's script and reveals the secret.
/// 0 when there is no such transaction (yet).
pub fn claimed_from(
    chain: &(impl BtcChain + ?Sized),
    htlc: &crate::Htlc,
    funding: &OutPoint,
    funded_height: u32,
    secret: &[u8; 32],
) -> Result<u64> {
    let tip = chain.tip()?;
    anyhow::ensure!(
        funded_height > 0
            && funded_height <= tip
            && funded_height.saturating_add(CLAIM_LOOKBACK) >= tip,
        "the payment of this swap is not in a recent Bitcoin block"
    );
    let Some(tx) = chain.spender(funding, funded_height)? else {
        return Ok(0);
    };
    let script = htlc.script();
    let by_contract = tx.input.len() == 1
        && tx.input[0].previous_output == *funding
        && tx.input[0].witness.last() == Some(script.as_bytes())
        && tx.input[0].witness.iter().any(|w| w == secret);
    Ok(if by_contract {
        tx.output
            .iter()
            .fold(0u64, |a, o| a.saturating_add(o.value.to_sat()))
    } else {
        0
    })
}

/// The unspent outputs of `address`, from its outputs and one look at each.
fn unspent_of<C: BtcChain + ?Sized>(chain: &C, address: &Address) -> Result<Vec<Utxo>> {
    let mut coins: Vec<Utxo> = Vec::new();
    for u in chain.outputs(address)? {
        if !coins.iter().any(|c| c.outpoint == u.outpoint) && !chain.is_spent(&u.outpoint)? {
            coins.push(u);
        }
    }
    Ok(coins)
}

fn http() -> Result<reqwest::blocking::Client> {
    Ok(reqwest::blocking::Client::builder()
        .timeout(Duration::from_secs(20))
        .user_agent("genoryn-krypton-wallet")
        .build()?)
}

/// The highest fee rate (sat/vB) taken from a Bitcoin server: above any rate the network has
/// needed to confirm within hours, so a broken or hostile server cannot make a payment burn its
/// value in fees.
pub const MAX_FEE_RATE: f64 = 500.0;

/// Whether `url` is plain http to this computer: the host is exactly a loopback name or
/// address, with no user name in front of it (so `http://127.0.0.1.example.com` or
/// `http://127.0.0.1:1@example.com` is not).
pub fn is_local_http(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| {
        u.scheme() == "http" && u.username().is_empty() && u.password().is_none() && loopback(&u)
    })
}

/// Whether `url` is a Bitcoin Core RPC address on this computer: plain http to a loopback
/// host, with the RPC user name and password in front of it allowed.
pub fn is_local_rpc(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| u.scheme() == "http" && loopback(&u))
}

/// Whether `url` is https to a named host, with no user name in front of it.
pub fn is_remote_https(url: &str) -> bool {
    reqwest::Url::parse(url.trim()).is_ok_and(|u| {
        u.scheme() == "https"
            && u.username().is_empty()
            && u.password().is_none()
            && u.host_str().is_some_and(|h| !h.is_empty())
    })
}

fn loopback(u: &reqwest::Url) -> bool {
    u.host_str().is_some_and(|h| {
        h.eq_ignore_ascii_case("localhost")
            || h.parse::<std::net::Ipv4Addr>()
                .is_ok_and(|a| a.is_loopback())
            || h.trim_start_matches('[')
                .trim_end_matches(']')
                .parse::<std::net::Ipv6Addr>()
                .is_ok_and(|a| a.is_loopback())
    })
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
            is_remote_https(&base) || is_local_http(&base),
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
    /// The server's own list of the unspent outputs of an address: complete, however
    /// many other payments the address has seen (its transaction list is one page).
    fn utxo(&self, address: &Address) -> Result<Vec<Utxo>> {
        let v: Value = serde_json::from_str(&self.get(&format!("/address/{address}/utxo"))?)?;
        v.as_array()
            .context("unexpected Bitcoin API reply")?
            .iter()
            .map(|u| {
                Ok(Utxo {
                    outpoint: OutPoint::new(
                        Txid::from_str(u["txid"].as_str().context("txid")?)?,
                        u["vout"].as_u64().context("vout")? as u32,
                    ),
                    value: Amount::from_sat(u["value"].as_u64().context("value")?),
                    height: u["status"]["block_height"].as_u64().map(|h| h as u32),
                })
            })
            .collect()
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
    fn spender(&self, op: &OutPoint, _from: u32) -> Result<Option<Transaction>> {
        let v: Value =
            serde_json::from_str(&self.get(&format!("/tx/{}/outspend/{}", op.txid, op.vout))?)?;
        let Some(id) = v["txid"]
            .as_str()
            .filter(|_| v["spent"].as_bool() == Some(true))
        else {
            return Ok(None);
        };
        let id = Txid::from_str(id)?;
        let raw = self.get(&format!("/tx/{id}/hex"))?;
        let tx: Transaction = bitcoin::consensus::encode::deserialize_hex(raw.trim())?;
        anyhow::ensure!(
            tx.compute_txid() == id,
            "the Bitcoin API returned another transaction"
        );
        Ok(Some(tx))
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
    /// The server's own list of unspent outputs; if it does not give one (some refuse it
    /// for busy addresses), the outputs are looked at one by one instead.
    fn unspent(&self, address: &Address) -> Result<Vec<Utxo>> {
        self.utxo(address).or_else(|_| unspent_of(self, address))
    }
    fn fee_rate(&self) -> Result<FeeRate> {
        let v: Value = serde_json::from_str(&self.get("/fee-estimates")?)?;
        let r = v["3"].as_f64().or_else(|| v["6"].as_f64()).unwrap_or(2.0);
        FeeRate::from_sat_per_vb(r.ceil().clamp(1.0, MAX_FEE_RATE) as u64).context("fee rate")
    }
    fn tx(&self, txid: &Txid) -> Result<Option<Transaction>> {
        let raw = self.get(&format!("/tx/{txid}/hex"))?;
        let tx: Transaction = bitcoin::consensus::encode::deserialize_hex(raw.trim())?;
        anyhow::ensure!(
            tx.compute_txid() == *txid,
            "the Bitcoin API returned another transaction"
        );
        Ok(Some(tx))
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
        // The RPC password goes with every call: in the clear only to this computer.
        anyhow::ensure!(
            u.scheme() == "https" || (u.scheme() == "http" && loopback(&u)),
            "a Bitcoin Core node on this computer (http://user:pass@127.0.0.1:8332) or over https"
        );
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
    fn spender(&self, op: &OutPoint, from: u32) -> Result<Option<Transaction>> {
        let spends = |tx: &Transaction| tx.input.iter().any(|i| i.previous_output == *op);
        let decode = |hex: &str| -> Result<Transaction> {
            Ok(bitcoin::consensus::encode::deserialize_hex(hex)?)
        };
        // Mempool first (Bitcoin Core 24+), then the blocks since `from`.
        if let Ok(r) = self.call(
            "gettxspendingprevout",
            json!([[{"txid": op.txid.to_string(), "vout": op.vout}]]),
        ) && let Some(id) = r[0]["spendingtxid"].as_str()
        {
            let raw = self.call("getrawtransaction", json!([id]))?;
            let tx = decode(raw.as_str().context("raw tx")?)?;
            if spends(&tx) {
                return Ok(Some(tx));
            }
        }
        let tip = self.tip()?;
        for h in from.max(tip.saturating_sub(2_000))..=tip {
            let hash = self.call("getblockhash", json!([h]))?;
            let raw = self.call("getblock", json!([hash, 0]))?;
            let block: bitcoin::Block =
                bitcoin::consensus::encode::deserialize_hex(raw.as_str().context("block")?)?;
            if let Some(tx) = block.txdata.into_iter().find(|t| spends(t)) {
                return Ok(Some(tx));
            }
        }
        Ok(None)
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        let r = self.call("sendrawtransaction", json!([serialize_hex(tx)]))?;
        Ok(Txid::from_str(r.as_str().context("txid")?)?)
    }
    fn fee_rate(&self) -> Result<FeeRate> {
        let r = self.call("estimatesmartfee", json!([3])).ok();
        let btc_per_kvb = r.and_then(|v| v["feerate"].as_f64()).unwrap_or(0.00002);
        let sat_vb = (btc_per_kvb * 1e8 / 1000.0).ceil().clamp(1.0, MAX_FEE_RATE) as u64;
        FeeRate::from_sat_per_vb(sat_vb).context("fee rate")
    }
    fn tx(&self, txid: &Txid) -> Result<Option<Transaction>> {
        // Without -txindex Bitcoin Core finds only mempool and wallet transactions.
        let Ok(raw) = self.call("getrawtransaction", json!([txid.to_string()])) else {
            return Ok(None);
        };
        let tx: Transaction =
            bitcoin::consensus::encode::deserialize_hex(raw.as_str().context("raw tx")?)?;
        anyhow::ensure!(
            tx.compute_txid() == *txid,
            "bitcoind returned another transaction"
        );
        Ok(Some(tx))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_this_computer_counts_as_local() {
        for u in [
            "http://127.0.0.1:3002",
            "http://localhost:3002/api",
            "http://[::1]:3002",
            "http://127.0.0.2:8080",
        ] {
            assert!(is_local_http(u), "{u}");
        }
        for u in [
            "http://127.0.0.1.example.com/api",
            "http://127.0.0.1:1@example.com/",
            "http://localhost.example.com",
            "http://user@127.0.0.1:3002",
            "https://127.0.0.1:3002",
            "http://10.0.0.1:3002",
            "ftp://127.0.0.1",
        ] {
            assert!(!is_local_http(u), "{u}");
        }
        assert!(is_local_rpc("http://user:pass@127.0.0.1:8332"));
        assert!(!is_local_rpc("http://user:pass@127.0.0.1.example.com:8332"));
        assert!(!is_local_rpc("http://127.0.0.1:1@example.com:8332"));
        assert!(is_remote_https("https://mempool.space/api"));
        assert!(!is_remote_https("https://user@mempool.space/api"));
        assert!(!is_remote_https("http://mempool.space/api"));
        assert!(Esplora::new("http://127.0.0.1.example.com/api").is_err());
        assert!(CoreRpc::new("http://u:p@example.com:8332").is_err());
        assert!(CoreRpc::new("http://u:p@127.0.0.1:8332").is_ok());
    }
}

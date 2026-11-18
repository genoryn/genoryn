//! The offer board: public swap offers, an encrypted mailbox between wallets, and the
//! market price from completed swaps. The board (any node, genoryn.io by default) only
//! ever sees offers, ciphertext and trade reports: it holds no keys and no coins.
//!
//! * Every offer has its own hybrid X25519 + ML-KEM-1024 key, derived from the wallet's
//!   swap seed, so offers cannot be linked to a wallet address or to each other.
//! * Messages are sealed to the recipient's offer key and carry the sender's key for the
//!   reply; the board stores them for a day under `inbox = H(key)`.
//! * Offers, messages and trade reports carry a small proof of work against spam.

use anyhow::{Context, Result, anyhow, ensure};
use krypton_crypto::hash::{tagged, tagged_parts};
use krypton_crypto::{aead, kem};
use serde::{Deserialize, Serialize};

/// Proof-of-work bits for anything posted to the board (about a second on one core).
pub const POW_BITS: u32 = 18;
/// Longest an offer stays up: a year (and a day for leap years).
pub const MAX_OFFER_SECS: u64 = 366 * 24 * 3600;

/// Proof-of-work bits for an offer that stays up `secs`: one more bit for every doubling
/// beyond a day, so keeping junk on the board for long costs as much more as it lasts
/// (a year: 9 more bits, about half a minute on one core).
pub fn offer_bits(secs: u64) -> u32 {
    if secs <= 24 * 3600 {
        return POW_BITS;
    }
    let days = secs.div_ceil(24 * 3600);
    POW_BITS + (64 - (days - 1).leading_zeros())
}
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
/// Coin swaps send a proven transaction (about 100 KB, 160 KB in base32) in one message;
/// larger messages need 4 more bits of work and count against the board's byte budget.
pub const MAX_LARGE_MESSAGE_BYTES: usize = 256 * 1024;
pub const LARGE_POW_BITS: u32 = POW_BITS + 4;

fn message_bits(hex_len: usize) -> u32 {
    if hex_len <= 2 * MAX_MESSAGE_BYTES {
        POW_BITS
    } else {
        LARGE_POW_BITS
    }
}
/// How long trades count for the market price.
pub const PRICE_WINDOW_SECS: u64 = 24 * 3600;

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Side {
    /// The maker sells GNR for bitcoin (a buyer of GNR takes it).
    SellGnr,
    /// The maker buys GNR with bitcoin (a seller of GNR takes it).
    BuyGnr,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Offer {
    pub id: String,
    pub network: String,
    pub btc_network: String,
    pub side: Side,
    /// GNR (krypton) and bitcoin (sat) of the whole offer.
    pub gnr: u64,
    pub sats: u64,
    /// The maker's key for this offer (hex).
    pub key: String,
    pub expires: u64,
    /// SHA3 of the secret that cancels the offer (hex).
    pub cancel: String,
    pub nonce: u64,
}

fn leading_zero_bits(h: &[u8; 32]) -> u32 {
    let mut n = 0;
    for b in h {
        if *b == 0 {
            n += 8;
        } else {
            n += b.leading_zeros();
            break;
        }
    }
    n
}

fn pow_hash(tag: &str, body: &[u8], nonce: u64) -> [u8; 32] {
    tagged_parts(tag, &[body, &nonce.to_le_bytes()])
}

/// Finds a nonce with `bits` leading zero bits, on every core. The hash state after the
/// tag and the body is computed once; each try only adds the nonce, giving exactly
/// [`pow_hash`].
pub fn mine(tag: &str, body: &[u8], bits: u32) -> u64 {
    use sha3::{Digest, Sha3_256};
    use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
    let mut base = Sha3_256::new();
    base.update((tag.len() as u16).to_le_bytes());
    base.update(tag.as_bytes());
    base.update((body.len() as u64).to_le_bytes());
    base.update(body);
    let threads = std::thread::available_parallelism().map_or(1, |n| n.get()) as u64;
    let done = AtomicBool::new(false);
    let found = AtomicU64::new(0);
    std::thread::scope(|s| {
        for t in 0..threads {
            let (base, done, found) = (&base, &done, &found);
            s.spawn(move || {
                let mut n = t;
                while !done.load(Ordering::Relaxed) {
                    let mut h = base.clone();
                    h.update(8u64.to_le_bytes());
                    h.update(n.to_le_bytes());
                    let d: [u8; 32] = h.finalize().into();
                    if leading_zero_bits(&d) >= bits {
                        found.store(n, Ordering::Relaxed);
                        done.store(true, Ordering::Relaxed);
                        break;
                    }
                    n += threads;
                }
            });
        }
    });
    found.load(Ordering::Relaxed)
}

pub fn pow_ok(tag: &str, body: &[u8], nonce: u64, bits: u32) -> bool {
    leading_zero_bits(&pow_hash(tag, body, nonce)) >= bits
}

impl Offer {
    fn body(&self) -> Vec<u8> {
        let mut o = self.clone();
        o.nonce = 0;
        serde_json::to_vec(&o).expect("serializable")
    }
    pub fn seal_pow(&mut self) {
        let bits = offer_bits(self.expires.saturating_sub(krypton_core::now()));
        self.nonce = mine("Krypton/board/offer", &self.body(), bits);
    }
    /// Whether the offer carries enough work for the time it still has to run at `now`.
    pub fn pow_ok_at(&self, now: u64) -> bool {
        let bits = offer_bits(self.expires.saturating_sub(now));
        pow_ok("Krypton/board/offer", &self.body(), self.nonce, bits)
    }
    pub fn public_key(&self) -> Result<kem::PublicKey> {
        Ok(kem::PublicKey::from_bytes(&hex::decode(&self.key)?)?)
    }
    pub fn inbox(&self) -> Result<String> {
        Ok(inbox_of(&self.public_key()?))
    }
    /// Bitcoin per GNR, in sat per whole GNR.
    pub fn price(&self) -> f64 {
        self.sats as f64 / (self.gnr as f64 / 1e8)
    }
    /// Checks everything a board checks before listing an offer.
    pub fn validate(&self, network: &str, now: u64) -> Result<()> {
        ensure!(
            self.id.len() == 32 && hex::decode(&self.id).is_ok(),
            "bad offer id"
        );
        ensure!(self.network == network, "the offer is for another network");
        ensure!(self.btc_network.len() <= 16, "bad Bitcoin network");
        ensure!(
            self.gnr > 0 && self.gnr <= 21_000_000 * 100_000_000,
            "bad GNR amount"
        );
        ensure!(
            self.sats >= 10_000 && self.sats <= MAX_OFFER_SATS,
            "an offer is for 10,000 sat to 1,000 BTC"
        );
        ensure!(
            self.expires > now && self.expires <= now + MAX_OFFER_SECS + 60,
            "bad expiry"
        );
        ensure!(
            self.cancel.len() == 64 && hex::decode(&self.cancel).is_ok(),
            "bad cancel hash"
        );
        self.public_key().context("bad offer key")?;
        ensure!(self.pow_ok_at(now), "missing proof of work");
        Ok(())
    }
}

/// The most bitcoin one offer (or reported swap) may be for.
pub const MAX_OFFER_SATS: u64 = 1_000 * 100_000_000;

pub fn cancel_hash(secret: &[u8; 32]) -> String {
    hex::encode(tagged("Krypton/board/cancel", secret))
}

pub fn inbox_of(pk: &kem::PublicKey) -> String {
    hex::encode(tagged("Krypton/board/inbox", &pk.to_bytes()))
}

/// The key of offer (or take) number `n` of a wallet.
pub fn offer_key(swap_seed: &[u8; 64], n: u32) -> kem::SecretKey {
    let mut ikm = zeroize::Zeroizing::new(swap_seed.to_vec());
    ikm.extend_from_slice(&n.to_le_bytes());
    let seed = krypton_crypto::kdf::derive::<96>(&ikm, "Krypton/board/key");
    kem::SecretKey::from_seed(&seed)
}

/// A message on its way to an inbox.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Envelope {
    pub to: String,
    /// Hex: KEM ciphertext ‖ nonce ‖ sealed (sender key ‖ body).
    pub data: String,
    pub nonce: u64,
}

impl Envelope {
    fn body(&self) -> Vec<u8> {
        [self.to.as_bytes(), b"|", self.data.as_bytes()].concat()
    }
    pub fn pow_ok(&self) -> bool {
        pow_ok(
            "Krypton/board/msg",
            &self.body(),
            self.nonce,
            message_bits(self.data.len()),
        )
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.to.len() == 64 && hex::decode(&self.to).is_ok(),
            "bad inbox"
        );
        ensure!(
            self.data.len() <= 2 * MAX_LARGE_MESSAGE_BYTES,
            "message too large"
        );
        hex::decode(&self.data).context("bad message encoding")?;
        ensure!(self.pow_ok(), "missing proof of work");
        Ok(())
    }
}

/// Seals `body` to `to` on behalf of `from` (whose public key goes inside, for replies).
pub fn seal(to: &kem::PublicKey, from: &kem::SecretKey, body: &[u8]) -> Result<Envelope> {
    ensure!(
        body.len() <= MAX_LARGE_MESSAGE_BYTES - kem::CIPHERTEXT_LEN - kem::PUBLIC_KEY_LEN - 64,
        "message too large"
    );
    let (ct, ss) = kem::encapsulate(to)?;
    let nonce: [u8; 24] = rand::random();
    let mut pt = from.public().to_bytes();
    pt.extend_from_slice(body);
    let aad = tagged("Krypton/board/aad", &to.to_bytes());
    let mut data = ct;
    data.extend_from_slice(&nonce);
    data.extend_from_slice(&aead::seal(&ss, &nonce, &aad, &pt));
    let mut e = Envelope {
        to: inbox_of(to),
        data: hex::encode(data),
        nonce: 0,
    };
    e.nonce = mine("Krypton/board/msg", &e.body(), message_bits(e.data.len()));
    Ok(e)
}

/// Opens a message sent to `me`: (sender's key, body).
pub fn open(me: &kem::SecretKey, data_hex: &str) -> Result<(kem::PublicKey, Vec<u8>)> {
    let data = hex::decode(data_hex)?;
    ensure!(
        data.len() > kem::CIPHERTEXT_LEN + 24 + 16 + kem::PUBLIC_KEY_LEN,
        "message too short"
    );
    let (ct, rest) = data.split_at(kem::CIPHERTEXT_LEN);
    let (nonce, sealed) = rest.split_at(24);
    let ss = me.decapsulate(ct)?;
    let aad = tagged("Krypton/board/aad", &me.public().to_bytes());
    let pt = aead::open(&ss, nonce.try_into().expect("24"), &aad, sealed)
        .map_err(|_| anyhow!("not for this key"))?;
    let (from, body) = pt.split_at(kem::PUBLIC_KEY_LEN);
    Ok((kem::PublicKey::from_bytes(from)?, body.to_vec()))
}

/// What travels inside a message between the two wallets of a swap.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "kind", rename_all = "snake_case")]
pub enum Note {
    /// A seller of GNR takes a buy offer: the maker answers with a request.
    Take {
        offer: String,
    },
    /// A swap code (request, accept or deposit).
    Code {
        offer: String,
        code: String,
    },
    /// The offer is gone (taken by someone else, expired or cancelled).
    Gone {
        offer: String,
    },
    /// Coin swaps (one transaction, `krypton_wallet_core::coin_swap`): a taker asks for
    /// the swap and gives its address; a buyer sends its unproven GNR half (JSON); the
    /// seller returns the transaction with its half proven (base32 of its bytes).
    CoinTake {
        offer: String,
        address: String,
    },
    CoinHalf {
        offer: String,
        half: String,
    },
    CoinTx {
        offer: String,
        tx: String,
    },
}

/// A standing offer to swap GNR for a Krypton coin. `side` is the maker's side of the
/// GNR: `sell_gnr` sells GNR for the coin, `buy_gnr` buys GNR with the coin.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoinOffer {
    pub id: String,
    pub network: String,
    /// The coin's id (hex).
    pub coin: String,
    pub side: Side,
    /// GNR (krypton) and coin base units of the whole offer.
    pub gnr: u64,
    pub units: u64,
    pub key: String,
    pub expires: u64,
    pub cancel: String,
    pub nonce: u64,
}

impl CoinOffer {
    fn body(&self) -> Vec<u8> {
        let mut o = self.clone();
        o.nonce = 0;
        serde_json::to_vec(&o).expect("serializable")
    }
    pub fn seal_pow(&mut self) {
        let bits = offer_bits(self.expires.saturating_sub(krypton_core::now()));
        self.nonce = mine("Krypton/board/coin-offer", &self.body(), bits);
    }
    pub fn pow_ok_at(&self, now: u64) -> bool {
        let bits = offer_bits(self.expires.saturating_sub(now));
        pow_ok("Krypton/board/coin-offer", &self.body(), self.nonce, bits)
    }
    pub fn public_key(&self) -> Result<kem::PublicKey> {
        Ok(kem::PublicKey::from_bytes(&hex::decode(&self.key)?)?)
    }
    pub fn inbox(&self) -> Result<String> {
        Ok(inbox_of(&self.public_key()?))
    }
    pub fn coin_id(&self) -> Result<krypton_core::asset::AssetId> {
        krypton_core::asset::AssetId::from_hex(&self.coin).context("bad coin id")
    }
    /// GNR (krypton) per base unit of the coin.
    pub fn price(&self) -> f64 {
        self.gnr as f64 / self.units as f64
    }
    pub fn validate(&self, network: &str, now: u64) -> Result<()> {
        ensure!(
            self.id.len() == 32 && hex::decode(&self.id).is_ok(),
            "bad offer id"
        );
        ensure!(self.network == network, "the offer is for another network");
        let coin = self.coin_id()?;
        ensure!(!coin.is_gnr(), "GNR is not a Krypton coin");
        ensure!(
            self.gnr > 0 && self.gnr <= 21_000_000 * 100_000_000,
            "bad GNR amount"
        );
        ensure!(
            self.units > 0 && self.units <= krypton_core::asset::MAX_COIN_UNITS,
            "bad coin amount"
        );
        ensure!(
            self.expires > now && self.expires <= now + MAX_OFFER_SECS + 60,
            "bad expiry"
        );
        ensure!(
            self.cancel.len() == 64 && hex::decode(&self.cancel).is_ok(),
            "bad cancel hash"
        );
        self.public_key().context("bad offer key")?;
        ensure!(self.pow_ok_at(now), "missing proof of work");
        Ok(())
    }
}

/// Opens one output of a transaction: its one-time key and owner tag (as in a payment
/// proof), so the board can confirm what a swap paid without trusting the reporter.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Opening {
    /// Index among the transaction's outputs (`Transaction::all_outputs`).
    pub output: u8,
    pub key: String,
    pub owner: String,
}

impl Opening {
    fn open(&self, tx: &krypton_core::Transaction) -> Result<krypton_core::address::NotePlain> {
        let outs = tx.all_outputs();
        let o = outs.get(self.output as usize).context("no such output")?;
        let key: [u8; 32] = hex::decode(&self.key)?
            .try_into()
            .map_err(|_| anyhow!("bad key"))?;
        let owner: [u8; 32] = hex::decode(&self.owner)?
            .try_into()
            .map_err(|_| anyhow!("bad owner"))?;
        let owner =
            krypton_crypto::field::digest_from_bytes(&owner).map_err(|_| anyhow!("bad owner"))?;
        krypton_core::address::open_disclosed_note(&key, &owner, &o.cm, &o.ciphertext)
            .context("the opening does not match the output")
    }
}

/// A completed coin swap, reported for that coin's market price. The board checks it
/// against the mined transaction: the coin output must hold `units` of `coin` and the
/// GNR output `gnr`, so a report cannot invent a price.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct CoinTrade {
    pub network: String,
    pub coin: String,
    pub txid: String,
    pub gnr: u64,
    pub units: u64,
    pub coin_output: Opening,
    pub gnr_output: Opening,
    pub nonce: u64,
}

impl CoinTrade {
    fn body(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.nonce = 0;
        serde_json::to_vec(&t).expect("serializable")
    }
    pub fn seal_pow(&mut self) {
        self.nonce = mine("Krypton/board/coin-trade", &self.body(), POW_BITS);
    }
    pub fn price(&self) -> f64 {
        self.gnr as f64 / self.units as f64
    }
    pub fn validate(&self, network: &str) -> Result<()> {
        ensure!(self.network == network, "the trade is for another network");
        krypton_core::asset::AssetId::from_hex(&self.coin).context("bad coin id")?;
        ensure!(
            self.txid.len() == 64 && hex::decode(&self.txid).is_ok(),
            "bad txid"
        );
        ensure!(self.gnr > 0 && self.units > 0, "bad amounts");
        ensure!(
            pow_ok(
                "Krypton/board/coin-trade",
                &self.body(),
                self.nonce,
                POW_BITS
            ),
            "missing proof of work"
        );
        Ok(())
    }
    /// Checks the report against the mined transaction `tx`.
    pub fn verify(&self, tx: &krypton_core::Transaction) -> Result<()> {
        ensure!(hex::encode(tx.txid()) == self.txid, "another transaction");
        let a = tx.asset.as_ref().context("not a coin transaction")?;
        ensure!(
            a.op == krypton_core::asset::AssetOp::Transfer && a.fee_action.is_some(),
            "not a coin swap"
        );
        // The coin moves in the transaction's action, the GNR in its fee action.
        ensure!(
            self.coin_output.output < 2 && (2..4).contains(&self.gnr_output.output),
            "the outputs are not the swap's"
        );
        let coin = self.coin_output.open(tx)?;
        let gnr = self.gnr_output.open(tx)?;
        ensure!(
            coin.coin().map(|c| c.to_hex()).as_deref() == Some(self.coin.as_str())
                && coin.value == self.units,
            "the coin output does not match"
        );
        ensure!(
            gnr.coin().is_none() && gnr.value == self.gnr,
            "the GNR output does not match"
        );
        Ok(())
    }
}

/// A coin's market price in krypton per base unit: the average of its completed swaps in
/// the last [`MARKET_WINDOW_SECS`] (or of all if there are none), without outliers more
/// than [`OUTLIER_FACTOR`] from the median once there are three. `None` with no swaps.
pub fn coin_market(trades: &[(CoinTrade, u64)], now: u64) -> Option<(f64, usize)> {
    let recent: Vec<f64> = trades
        .iter()
        .filter(|(_, at)| at + MARKET_WINDOW_SECS > now)
        .map(|(t, _)| t.price())
        .collect();
    let mut v = if recent.is_empty() {
        trades.iter().map(|(t, _)| t.price()).collect()
    } else {
        recent
    };
    v.retain(|p| p.is_finite() && *p > 0.0);
    if v.len() >= 3 {
        let mut s = v.clone();
        s.sort_by(f64::total_cmp);
        let med = s[s.len() / 2];
        v.retain(|p| *p <= med * OUTLIER_FACTOR && *p >= med / OUTLIER_FACTOR);
    }
    if v.is_empty() {
        return None;
    }
    Some((v.iter().sum::<f64>() / v.len() as f64, v.len()))
}

/// A completed swap, reported for the market price.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Trade {
    pub network: String,
    /// The swap's hash lock (hex): one trade per hash, and the board finds its claim on chain.
    pub hash: String,
    pub gnr: u64,
    pub sats: u64,
    pub nonce: u64,
}

impl Trade {
    fn body(&self) -> Vec<u8> {
        let mut t = self.clone();
        t.nonce = 0;
        serde_json::to_vec(&t).expect("serializable")
    }
    pub fn seal_pow(&mut self) {
        self.nonce = mine("Krypton/board/trade", &self.body(), POW_BITS);
    }
    pub fn validate(&self, network: &str) -> Result<()> {
        ensure!(self.network == network, "the trade is for another network");
        ensure!(
            self.hash.len() == 64 && hex::decode(&self.hash).is_ok(),
            "bad hash"
        );
        ensure!(
            self.gnr > 0 && self.sats > 0 && self.sats <= MAX_OFFER_SATS,
            "bad amounts"
        );
        ensure!(
            pow_ok("Krypton/board/trade", &self.body(), self.nonce, POW_BITS),
            "missing proof of work"
        );
        Ok(())
    }
    pub fn price(&self) -> f64 {
        self.sats as f64 / (self.gnr as f64 / 1e8)
    }
}

/// The market from the board's data.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Market {
    /// The market price in sat per GNR: the average of completed swaps (see
    /// [`market`]). `None` while the board is empty.
    pub price: Option<f64>,
    /// The strongest kind of data the price rests on: `swaps` or `offers`.
    pub source: Option<String>,
    /// How much the price can be trusted, 0 to 1: grows with the (time-weighted) GNR volume
    /// behind it, `w / (w + DEPTH_GNR)`.
    #[serde(default)]
    pub confidence: f64,
    /// `thin`, `moderate` or `deep`.
    #[serde(default)]
    pub depth: String,
    /// The order book's price (best ask and bid), shown only; it never sets the price.
    pub offers_price: Option<f64>,
    pub asks: usize,
    pub bids: usize,
    /// Swaps completed in the last 24 hours and their GNR.
    pub trades: usize,
    pub volume_gnr: u64,
    /// Swaps in progress (offers taken, not finished yet) and their GNR.
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub pending_gnr: u64,
    pub last_price: Option<f64>,
    /// How many completed swaps the price is the average of.
    #[serde(default)]
    pub priced: usize,
    /// Cheapest sell offer and highest buy offer (sat per GNR).
    pub best_ask: Option<f64>,
    pub best_bid: Option<f64>,
    pub offers: usize,
}

/// Volume-weighted median of (price, weight): robust against a few odd entries. With
/// exactly half the weight on each side it is the midpoint of the two middle prices.
pub fn weighted_median(v: Vec<(f64, u64)>) -> Option<f64> {
    weighted_median_f(v.into_iter().map(|(p, w)| (p, w as f64)).collect())
}

fn weighted_median_f(mut v: Vec<(f64, f64)>) -> Option<f64> {
    v.retain(|(p, w)| p.is_finite() && *p > 0.0 && w.is_finite() && *w > 0.0);
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: f64 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for (i, (p, w)) in v.iter().enumerate() {
        acc += w;
        let rel = (acc * 2.0 - total) / total;
        if rel.abs() < 1e-9
            && let Some((next, _)) = v.get(i + 1)
        {
            return Some((p + next) / 2.0);
        }
        if rel >= 0.0 {
            return Some(*p);
        }
    }
    v.last().map(|x| x.0)
}

/// The price is the average of the completed swaps of the last 72 hours (all kept swaps
/// while there are none that recent).
pub const MARKET_WINDOW_SECS: u64 = 72 * 3600;
/// With three or more swaps, a swap more than this factor away from their median is left
/// out: a swap reports its own amounts (they are private on chain), so a typo or a fake
/// report must not move the average.
pub const OUTLIER_FACTOR: f64 = 4.0;
/// GNR swapped behind the price at which confidence reaches one half.
pub const DEPTH_GNR: f64 = 25.0;
/// The book's best price is where at least this much GNR can be bought (or sold) at once,
/// so a dust offer does not set it.
pub const BOOK_MIN_GNR: f64 = 1.0;

/// The best price on one side of the book at which `BOOK_MIN_GNR` is available, and the
/// side's volume. `v` is (price, GNR) sorted best first.
fn inside(v: &[(f64, f64)]) -> Option<(f64, f64)> {
    let total: f64 = v.iter().map(|x| x.1).sum();
    let mut acc = 0.0;
    for (p, g) in v {
        acc += g;
        if acc >= BOOK_MIN_GNR {
            return Some((*p, total));
        }
    }
    v.last().map(|x| (x.0, total))
}

/// The order book as one price, for display only: the best ask and the best bid with at
/// least 1 GNR behind them (their midpoint when both exist).
pub fn book_price(offers: &[Offer]) -> Option<(f64, f64)> {
    let side = |s: Side, asc: bool| {
        let mut v: Vec<(f64, f64)> = offers
            .iter()
            .filter(|o| o.side == s)
            .map(|o| (o.price(), o.gnr as f64 / 1e8))
            .filter(|(p, g)| p.is_finite() && *p > 0.0 && *g > 0.0)
            .collect();
        v.sort_by(|a, b| {
            if asc {
                a.0.total_cmp(&b.0)
            } else {
                b.0.total_cmp(&a.0)
            }
        });
        inside(&v)
    };
    match (side(Side::SellGnr, true), side(Side::BuyGnr, false)) {
        (Some((a, va)), Some((b, vb))) => Some(((a + b) / 2.0, va + vb)),
        (Some((a, va)), None) => Some((a, va)),
        (None, Some((b, vb))) => Some((b, vb)),
        (None, None) => None,
    }
}

/// The swaps the price is the average of: those of the last 72 hours (all of them while
/// none is that recent), without typos and fakes (more than 4× from the median, once
/// there are three or more).
pub fn priced_swaps(trades: &[(Trade, u64)], now: u64) -> Vec<&Trade> {
    let ok = |t: &&Trade| t.price().is_finite() && t.price() > 0.0;
    let mut v: Vec<&Trade> = trades
        .iter()
        .filter(|(_, at)| *at + MARKET_WINDOW_SECS > now)
        .map(|(t, _)| t)
        .filter(ok)
        .collect();
    if v.is_empty() {
        v = trades.iter().map(|(t, _)| t).filter(ok).collect();
    }
    if v.len() >= 3
        && let Some(m) = weighted_median_f(v.iter().map(|t| (t.price(), 1.0)).collect())
    {
        v.retain(|t| t.price() <= m * OUTLIER_FACTOR && t.price() >= m / OUTLIER_FACTOR);
    }
    v
}

/// The market price: the arithmetic mean of the prices of completed swaps (each one's
/// claim is on the GNR chain), every swap counting the same; see [`priced_swaps`].
///
/// Open offers and swaps in progress are counted and shown, but never move the price:
/// anyone can post an offer at any price, and nothing proves a swap in progress runs.
pub fn market(
    trades: &[(Trade, u64)],
    offers: &[Offer],
    pending: &[(Offer, u64)],
    now: u64,
) -> Market {
    let day: Vec<&Trade> = trades
        .iter()
        .filter(|(_, at)| *at + PRICE_WINDOW_SECS > now)
        .map(|(t, _)| t)
        .collect();
    let last = trades
        .iter()
        .max_by_key(|(_, at)| *at)
        .map(|(t, _)| t.price());
    let ask = offers
        .iter()
        .filter(|o| o.side == Side::SellGnr)
        .map(Offer::price)
        .min_by(f64::total_cmp);
    let bid = offers
        .iter()
        .filter(|o| o.side == Side::BuyGnr)
        .map(Offer::price)
        .max_by(f64::total_cmp);
    let swaps = priced_swaps(trades, now);
    let price = (!swaps.is_empty())
        .then(|| swaps.iter().map(|t| t.price()).sum::<f64>() / swaps.len() as f64);
    let w: f64 = swaps.iter().map(|t| t.gnr as f64 / 1e8).sum();
    let confidence = if w > 0.0 { w / (w + DEPTH_GNR) } else { 0.0 };
    Market {
        price,
        source: price.map(|_| "swaps".to_string()),
        confidence: (confidence * 1000.0).round() / 1000.0,
        depth: if confidence >= 0.8 {
            "deep"
        } else if confidence >= 0.4 {
            "moderate"
        } else {
            "thin"
        }
        .into(),
        offers_price: book_price(offers).map(|b| b.0),
        asks: offers.iter().filter(|o| o.side == Side::SellGnr).count(),
        bids: offers.iter().filter(|o| o.side == Side::BuyGnr).count(),
        trades: day.len(),
        volume_gnr: day.iter().map(|t| t.gnr).sum(),
        pending: pending.len(),
        pending_gnr: pending.iter().map(|(o, _)| o.gnr).sum(),
        last_price: last,
        priced: swaps.len(),
        best_ask: ask,
        best_bid: bid,
        offers: offers.len(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn offer(side: Side, gnr: u64, sats: u64) -> Offer {
        let k = offer_key(&[1; 64], 1);
        let mut o = Offer {
            id: "ab".repeat(16),
            network: "devnet".into(),
            btc_network: "regtest".into(),
            side,
            gnr,
            sats,
            key: hex::encode(k.public().to_bytes()),
            expires: 2_000,
            cancel: cancel_hash(&[2; 32]),
            nonce: 0,
        };
        o.seal_pow();
        o
    }

    #[test]
    fn mining_matches_the_plain_hash() {
        for bits in [4, 10, 16] {
            let n = mine("Krypton/test", b"some body", bits);
            assert!(pow_ok("Krypton/test", b"some body", n, bits));
        }
    }

    #[test]
    fn longer_offers_need_more_work() {
        assert_eq!(offer_bits(3600), POW_BITS);
        assert_eq!(offer_bits(24 * 3600), POW_BITS);
        assert_eq!(offer_bits(2 * 24 * 3600), POW_BITS + 1);
        assert_eq!(offer_bits(7 * 24 * 3600), POW_BITS + 3);
        assert_eq!(offer_bits(30 * 24 * 3600), POW_BITS + 5);
        assert_eq!(offer_bits(MAX_OFFER_SECS), POW_BITS + 9);
    }

    #[test]
    fn offers_need_valid_fields_and_work() {
        let o = offer(Side::SellGnr, 100_000_000, 50_000);
        assert!(o.validate("devnet", 1_000).is_ok());
        assert!(o.validate("mainnet", 1_000).is_err());
        assert!(o.validate("devnet", 2_000).is_err(), "expired");
        let mut t = o.clone();
        t.sats += 1;
        assert!(
            t.validate("devnet", 1_000).is_err(),
            "work covers every field"
        );
        assert_eq!(o.price(), 50_000.0);
    }

    #[test]
    fn messages_reach_only_their_recipient() {
        let a = offer_key(&[1; 64], 1);
        let b = offer_key(&[9; 64], 4);
        let e = seal(a.public(), &b, b"hello swap").unwrap();
        assert!(e.validate().is_ok());
        assert_eq!(e.to, inbox_of(a.public()));
        let (from, body) = open(&a, &e.data).unwrap();
        assert_eq!(&from, b.public());
        assert_eq!(body, b"hello swap");
        assert!(open(&b, &e.data).is_err());
        let mut bad = e.clone();
        bad.data.replace_range(bad.data.len() - 2.., "00");
        assert!(open(&a, &bad.data).is_err());
        assert_ne!(
            offer_key(&[1; 64], 1).public(),
            offer_key(&[1; 64], 2).public()
        );
    }

    fn trade(sats: u64, gnr: u64) -> Trade {
        Trade {
            network: "devnet".into(),
            hash: "00".repeat(32),
            gnr,
            sats,
            nonce: 0,
        }
    }

    fn close(a: Option<f64>, b: f64) -> bool {
        a.is_some_and(|a| (a - b).abs() < 0.01)
    }

    #[test]
    fn market_price_is_the_average_of_completed_swaps() {
        let now = 100 * 3600;
        let trades = vec![
            (trade(5_000, 100_000_000), now - 600),
            (trade(52_000, 1_000_000_000), now - 500),
            (trade(5_100, 100_000_000), now - 400),
            // A fat-finger (or fake) report of 500 GNR at 400,000× the price is left out.
            (trade(1_000_000_000_000, 50_000_000_000), now - 300),
        ];
        // Every swap counts the same, whatever its size: (5,000 + 5,200 + 5,100) / 3.
        let m = market(&trades, &[], &[], now);
        assert_eq!(m.trades, 4);
        assert!(close(m.price, 5_100.0), "{m:?}");
        assert_eq!(m.source.as_deref(), Some("swaps"));
        assert!(
            (m.confidence - 12.0 / 37.0).abs() < 0.001,
            "{}",
            m.confidence
        );
        // Two swaps: their plain average (no outlier test below three).
        let two = vec![
            (trade(1_000, 100_000_000), now),
            (trade(3_000, 100_000_000), now),
        ];
        assert!(close(market(&two, &[], &[], now).price, 2_000.0));
        // No swap at all: no price, however many offers.
        let offers = vec![
            offer(Side::SellGnr, 100_000_000, 6_000),
            offer(Side::BuyGnr, 100_000_000, 4_000),
        ];
        let m = market(&[], &offers, &[], now);
        assert_eq!((m.price, m.source), (None, None));
        assert_eq!((m.best_ask, m.best_bid), (Some(6_000.0), Some(4_000.0)));
        assert_eq!(m.offers_price, Some(5_000.0));
        assert_eq!((m.asks, m.bids), (1, 1));
        assert_eq!(weighted_median(vec![]), None);
        assert_eq!(
            weighted_median(vec![(1_000.0, 10), (3_000.0, 10)]),
            Some(2_000.0)
        );
    }

    #[test]
    fn offers_and_swaps_in_progress_never_move_the_price() {
        let now = 300 * 3600;
        let swaps: Vec<_> = (0..3)
            .map(|i| (trade(5_000, 100_000_000), now - 60 * i))
            .collect();
        let mut spam: Vec<_> = (0..40)
            .map(|_| offer(Side::SellGnr, 100_000_000, 100_000_000))
            .collect();
        spam.extend((0..40).map(|_| offer(Side::BuyGnr, 100_000_000, 9_999)));
        let fake: Vec<_> = (0..100)
            .map(|_| (offer(Side::SellGnr, 100_000_000_000, 100_000_000_000), now))
            .collect();
        let m = market(&swaps, &spam, &fake, now);
        assert!(close(m.price, 5_000.0), "{m:?}");
        assert_eq!((m.pending, m.offers), (100, 80));
    }

    #[test]
    fn recent_swaps_count_older_ones_only_when_there_are_none() {
        let now = 200 * 3600;
        let trades = vec![
            (trade(40_000, 1_000_000_000), now - 80 * 3600),
            (trade(6_000, 100_000_000), now - 60),
        ];
        // The swap of 80 h ago is outside the 72 h window while a recent one exists.
        assert!(close(market(&trades, &[], &[], now).price, 6_000.0));
        // With only old swaps, their average.
        let old = vec![
            (trade(4_000, 100_000_000), now - 100 * 3600),
            (trade(6_000, 100_000_000), now - 90 * 3600),
        ];
        assert!(close(market(&old, &[], &[], now).price, 5_000.0));
        assert_eq!(market(&[], &[], &[], now).price, None);
        let deep = vec![
            (trade(500_000, 100 * 100_000_000), now - 60),
            (trade(500_000, 100 * 100_000_000), now - 30),
        ];
        let m = market(&deep, &[], &[], now);
        assert_eq!(m.depth, "deep");
        assert!(m.confidence > 0.85);
    }

    #[test]
    fn the_book_price_is_shown_but_not_used() {
        let g = |x: f64| (x * 1e8) as u64;
        let ask = |gnr: f64, per: u64| offer(Side::SellGnr, g(gnr), g(gnr) / 100_000_000 * per);
        // A dust ask below 1 GNR does not set the book; buy and sell sides meet midway.
        let mut b2 = vec![offer(Side::SellGnr, g(0.3), 300), ask(2.0, 10_000)];
        assert_eq!(book_price(&b2).map(|b| b.0), Some(10_000.0));
        b2.push(ask_bid(g(2.0), 16_000));
        assert_eq!(book_price(&b2).map(|b| b.0), Some(9_000.0));
    }

    fn ask_bid(gnr: u64, sats: u64) -> Offer {
        offer(Side::BuyGnr, gnr, sats)
    }

    fn coin_offer(gnr: u64, units: u64) -> CoinOffer {
        let k = kem::SecretKey::from_seed(&[3; 96]);
        let mut o = CoinOffer {
            id: "ab".repeat(16),
            network: "devnet".into(),
            coin: krypton_core::asset::AssetId {
                a0: 5,
                a1: 6,
                pausable: false,
            }
            .to_hex(),
            side: Side::SellGnr,
            gnr,
            units,
            key: hex::encode(k.public().to_bytes()),
            expires: krypton_core::now() + 3600,
            cancel: cancel_hash(&[1; 32]),
            nonce: 0,
        };
        o.seal_pow();
        o
    }

    #[test]
    fn coin_offers_validate() {
        let now = krypton_core::now();
        let o = coin_offer(100_000_000, 5_000);
        o.validate("devnet", now).unwrap();
        assert!((o.price() - 20_000.0).abs() < 1e-9);
        let mut bad = o.clone();
        bad.units = 6_000;
        assert!(
            bad.validate("devnet", now).is_err(),
            "work covers every field"
        );
        let mut gnr = o.clone();
        gnr.coin = krypton_core::asset::GNR.to_hex();
        gnr.seal_pow();
        assert!(gnr.validate("devnet", now).is_err(), "GNR is not a coin");
        assert!(o.validate("mainnet", now).is_err());
    }

    #[test]
    fn large_messages_need_more_work() {
        let a = kem::SecretKey::from_seed(&[1; 96]);
        let b = kem::SecretKey::from_seed(&[2; 96]);
        // A coin swap's transaction: about 100 KB, sent in base32.
        let body = vec![7u8; 170 * 1024];
        let e = seal(b.public(), &a, &body).unwrap();
        e.validate().unwrap();
        assert!(e.data.len() > 2 * MAX_MESSAGE_BYTES);
        assert!(
            !pow_ok("Krypton/board/msg", &e.body(), e.nonce, LARGE_POW_BITS + 6),
            "sanity: not absurdly lucky"
        );
        let (_, got) = open(&b, &e.data).unwrap();
        assert_eq!(got, body);
        // Work for a small message does not carry a large one.
        let small = seal(b.public(), &a, b"hi").unwrap();
        let forged = Envelope {
            to: e.to.clone(),
            data: e.data.clone(),
            nonce: small.nonce,
        };
        assert!(forged.validate().is_err());
    }

    fn coin_trade(gnr: u64, units: u64) -> CoinTrade {
        let op = Opening {
            output: 0,
            key: "00".repeat(32),
            owner: "00".repeat(32),
        };
        CoinTrade {
            network: "devnet".into(),
            coin: krypton_core::asset::AssetId {
                a0: 5,
                a1: 6,
                pausable: false,
            }
            .to_hex(),
            txid: "11".repeat(32),
            gnr,
            units,
            coin_output: op.clone(),
            gnr_output: Opening { output: 2, ..op },
            nonce: 0,
        }
    }

    #[test]
    fn coin_market_is_the_average_of_swaps_without_outliers() {
        let now = 1_000_000;
        assert_eq!(coin_market(&[], now), None);
        let v = vec![
            (coin_trade(100, 10), now - 10),
            (coin_trade(300, 10), now - 20),
            (coin_trade(1_000_000, 10), now - 30),
        ];
        let (p, n) = coin_market(&v, now).unwrap();
        assert_eq!(n, 2, "the outlier is dropped");
        assert!((p - 20.0).abs() < 1e-9);
        // Old swaps count only when there are no recent ones.
        let old = vec![(coin_trade(500, 10), 0), (coin_trade(100, 10), now - 5)];
        assert!((coin_market(&old, now).unwrap().0 - 10.0).abs() < 1e-9);
        let only_old = vec![(coin_trade(500, 10), 0)];
        assert!((coin_market(&only_old, now).unwrap().0 - 50.0).abs() < 1e-9);
    }
}

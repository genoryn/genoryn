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
/// Longest an offer stays up.
pub const MAX_OFFER_SECS: u64 = 24 * 3600;
pub const MAX_MESSAGE_BYTES: usize = 8 * 1024;
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
    /// GNR (gori) and bitcoin (sat) of the whole offer.
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

/// Finds a nonce with `bits` leading zero bits.
pub fn mine(tag: &str, body: &[u8], bits: u32) -> u64 {
    (0u64..)
        .find(|n| leading_zero_bits(&pow_hash(tag, body, *n)) >= bits)
        .expect("found")
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
        self.nonce = mine("Krypton/board/offer", &self.body(), POW_BITS);
    }
    pub fn pow_ok(&self) -> bool {
        pow_ok("Krypton/board/offer", &self.body(), self.nonce, POW_BITS)
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
            self.sats >= 10_000 && self.sats <= 21_000_000 * 100_000_000,
            "bad bitcoin amount"
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
        ensure!(self.pow_ok(), "missing proof of work");
        Ok(())
    }
}

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
        pow_ok("Krypton/board/msg", &self.body(), self.nonce, POW_BITS)
    }
    pub fn validate(&self) -> Result<()> {
        ensure!(
            self.to.len() == 64 && hex::decode(&self.to).is_ok(),
            "bad inbox"
        );
        ensure!(
            self.data.len() <= 2 * MAX_MESSAGE_BYTES,
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
        body.len() <= MAX_MESSAGE_BYTES - kem::CIPHERTEXT_LEN - kem::PUBLIC_KEY_LEN - 64,
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
    e.nonce = mine("Krypton/board/msg", &e.body(), POW_BITS);
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
    Take { offer: String },
    /// A swap code (request, accept or deposit).
    Code { offer: String, code: String },
    /// The offer is gone (taken by someone else, expired or cancelled).
    Gone { offer: String },
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
        ensure!(self.gnr > 0 && self.sats > 0, "bad amounts");
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
    /// The market price (sat per GNR): a volume-weighted median over the completed swaps of
    /// the window, the swaps in progress and the open offers, in that order of weight.
    pub price: Option<f64>,
    /// The strongest kind of data the price rests on: `swaps`, `pending` or `offers`.
    pub source: Option<String>,
    /// Volume-weighted median of the open offers alone.
    pub offers_price: Option<f64>,
    pub asks: usize,
    pub bids: usize,
    pub trades: usize,
    pub volume_gnr: u64,
    /// Swaps in progress (offers taken, not finished yet) and their GNR.
    #[serde(default)]
    pub pending: usize,
    #[serde(default)]
    pub pending_gnr: u64,
    pub last_price: Option<f64>,
    /// Cheapest sell offer and highest buy offer (sat per GNR).
    pub best_ask: Option<f64>,
    pub best_bid: Option<f64>,
    pub offers: usize,
}

/// Volume-weighted median of (price, volume): robust against a few odd trades.
pub fn weighted_median(mut v: Vec<(f64, u64)>) -> Option<f64> {
    v.retain(|(p, w)| p.is_finite() && *p > 0.0 && *w > 0);
    if v.is_empty() {
        return None;
    }
    v.sort_by(|a, b| a.0.total_cmp(&b.0));
    let total: u128 = v.iter().map(|x| x.1 as u128).sum();
    let mut acc = 0u128;
    for (p, w) in &v {
        acc += *w as u128;
        if acc * 2 >= total {
            return Some(*p);
        }
    }
    v.last().map(|x| x.0)
}

/// Weights of the three kinds of market data: what people paid counts most, what they are
/// paying right now next, what they ask or bid least.
pub const WEIGHT_SWAP: u64 = 3;
pub const WEIGHT_PENDING: u64 = 2;
pub const WEIGHT_OFFER: u64 = 1;

/// `pending`: offers taken whose swaps are still in progress.
pub fn market(trades: &[(Trade, u64)], offers: &[Offer], pending: &[Offer], now: u64) -> Market {
    let recent: Vec<&Trade> = trades
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
    let mut all: Vec<(f64, u64)> = Vec::new();
    all.extend(
        recent
            .iter()
            .map(|t| (t.price(), t.gnr.saturating_mul(WEIGHT_SWAP))),
    );
    all.extend(
        pending
            .iter()
            .map(|o| (o.price(), o.gnr.saturating_mul(WEIGHT_PENDING))),
    );
    all.extend(
        offers
            .iter()
            .map(|o| (o.price(), o.gnr.saturating_mul(WEIGHT_OFFER))),
    );
    let source = if !recent.is_empty() {
        Some("swaps")
    } else if !pending.is_empty() {
        Some("pending")
    } else if !offers.is_empty() {
        Some("offers")
    } else {
        None
    };
    Market {
        price: weighted_median(all),
        source: source.map(str::to_string),
        offers_price: weighted_median(offers.iter().map(|o| (o.price(), o.gnr)).collect()),
        asks: offers.iter().filter(|o| o.side == Side::SellGnr).count(),
        bids: offers.iter().filter(|o| o.side == Side::BuyGnr).count(),
        trades: recent.len(),
        volume_gnr: recent.iter().map(|t| t.gnr).sum(),
        pending: pending.len(),
        pending_gnr: pending.iter().map(|o| o.gnr).sum(),
        last_price: last,
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

    #[test]
    fn market_price_is_a_robust_median() {
        let t = |sats: u64, gnr: u64| Trade {
            network: "devnet".into(),
            hash: "00".repeat(32),
            gnr,
            sats,
            nonce: 0,
        };
        let trades = vec![
            (t(5_000, 100_000_000), 900),
            (t(5_200, 100_000_000), 950),
            (t(5_100, 300_000_000 / 3), 960),
            // One absurd trade does not move a median.
            (t(1_000_000, 10_000_000), 970),
            // Outside the 24 h window.
            (t(9_000, 100_000_000), 10),
        ];
        let offers = vec![
            offer(Side::SellGnr, 100_000_000, 6_000),
            offer(Side::BuyGnr, 100_000_000, 4_000),
        ];
        let m = market(&trades, &offers, &[], 850 + PRICE_WINDOW_SECS);
        assert_eq!(m.trades, 4);
        assert_eq!(m.price, Some(5_100.0));
        assert_eq!(m.best_ask, Some(6_000.0));
        assert_eq!(m.best_bid, Some(4_000.0));
        assert_eq!(m.source.as_deref(), Some("swaps"));
        assert_eq!((m.asks, m.bids), (1, 1));
        assert_eq!(weighted_median(vec![]), None);
        // No swaps yet: the first offer already sets a price, more offers a median.
        let one = market(&[], &offers[..1], &[], 1_000);
        assert_eq!(
            (one.price, one.source.as_deref()),
            (Some(6_000.0), Some("offers"))
        );
        let mut book = offers.clone();
        book.push(offer(Side::SellGnr, 300_000_000, 15_000));
        assert_eq!(market(&[], &book, &[], 1_000).price, Some(5_000.0));
        assert_eq!(market(&[], &[], &[], 1_000).price, None);
        // A swap in progress counts twice as much as an offer of the same size.
        let taken = vec![offer(Side::SellGnr, 100_000_000, 8_000)];
        let m = market(
            &[],
            &[offer(Side::SellGnr, 100_000_000, 2_000)],
            &taken,
            1_000,
        );
        assert_eq!(
            (m.price, m.source.as_deref(), m.pending),
            (Some(8_000.0), Some("pending"), 1)
        );
        assert_eq!(market(&[], &[], &taken, 1_000).price, Some(8_000.0));
    }
}

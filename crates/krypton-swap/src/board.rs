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
    /// The market price in sat per GNR: one robust median over what people do (see
    /// [`market`]). `None` while the board is empty.
    pub price: Option<f64>,
    /// The strongest kind of data the price rests on: `swaps`, `pending` or `offers`.
    pub source: Option<String>,
    /// How much the price can be trusted, 0 to 1: grows with the (time-weighted) GNR volume
    /// behind it, `w / (w + DEPTH_GNR)`.
    #[serde(default)]
    pub confidence: f64,
    /// `thin`, `moderate` or `deep`.
    #[serde(default)]
    pub depth: String,
    /// Volume-weighted median of the open offers alone.
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

/// What people paid counts most, what they are paying right now next, what they ask or
/// bid least.
pub const WEIGHT_SWAP: f64 = 3.0;
pub const WEIGHT_PENDING: f64 = 2.0;
pub const WEIGHT_OFFER: f64 = 1.0;
/// Older data fades: half its weight every 12 hours, gone after 72.
pub const HALF_LIFE_SECS: f64 = 12.0 * 3600.0;
pub const MARKET_WINDOW_SECS: u64 = 72 * 3600;
/// Prices more than this factor away from the plain median are left out (fat fingers,
/// joke offers, attempts to drag the price).
pub const OUTLIER_FACTOR: f64 = 4.0;
/// Time-weighted GNR behind the price at which confidence reaches one half.
pub const DEPTH_GNR: f64 = 25.0;

fn fade(age: u64) -> f64 {
    0.5f64.powf(age as f64 / HALF_LIFE_SECS)
}

/// The market price:
///
/// 1. Every completed swap (last 72 h), swap in progress and open offer becomes a price
///    point weighted by its GNR × kind (swap 3, in progress 2, offer 1) × age fade (half
///    every 12 h; offers are current and do not fade).
/// 2. Points more than 4× away from the plain median are dropped.
/// 3. The price is the weighted median of the rest; confidence grows with their weight.
///
/// `pending` are taken offers with the time they were taken.
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

    // (price, weight in GNR-equivalents, kind)
    let mut pts: Vec<(f64, f64, u8)> = Vec::new();
    for (t, at) in trades
        .iter()
        .filter(|(_, at)| *at + MARKET_WINDOW_SECS > now)
    {
        pts.push((
            t.price(),
            t.gnr as f64 / 1e8 * WEIGHT_SWAP * fade(now.saturating_sub(*at)),
            0,
        ));
    }
    for (o, at) in pending {
        pts.push((
            o.price(),
            o.gnr as f64 / 1e8 * WEIGHT_PENDING * fade(now.saturating_sub(*at)),
            1,
        ));
    }
    for o in offers {
        pts.push((o.price(), o.gnr as f64 / 1e8 * WEIGHT_OFFER, 2));
    }
    // The centre for the outlier cut counts points, not GNR (a made-up giant offer cannot
    // move it), each by its kind (cheap offers cannot outvote swaps). Two points have no
    // outlier: the weighted median alone decides between them.
    if pts.len() >= 3
        && let Some(m0) = weighted_median_f(
            pts.iter()
                .map(|p| {
                    (
                        p.0,
                        [WEIGHT_SWAP, WEIGHT_PENDING, WEIGHT_OFFER][p.2 as usize],
                    )
                })
                .collect(),
        )
    {
        pts.retain(|p| p.0 <= m0 * OUTLIER_FACTOR && p.0 >= m0 / OUTLIER_FACTOR);
    }
    let w: f64 = pts.iter().map(|p| p.1).sum();
    let kind = pts.iter().map(|p| p.2).min();
    let confidence = if w > 0.0 { w / (w + DEPTH_GNR) } else { 0.0 };
    Market {
        price: weighted_median_f(pts.iter().map(|p| (p.0, p.1)).collect()),
        source: kind.map(|k| ["swaps", "pending", "offers"][k as usize].to_string()),
        confidence: (confidence * 1000.0).round() / 1000.0,
        depth: if confidence >= 0.8 {
            "deep"
        } else if confidence >= 0.4 {
            "moderate"
        } else {
            "thin"
        }
        .into(),
        offers_price: weighted_median(offers.iter().map(|o| (o.price(), o.gnr)).collect()),
        asks: offers.iter().filter(|o| o.side == Side::SellGnr).count(),
        bids: offers.iter().filter(|o| o.side == Side::BuyGnr).count(),
        trades: day.len(),
        volume_gnr: day.iter().map(|t| t.gnr).sum(),
        pending: pending.len(),
        pending_gnr: pending.iter().map(|(o, _)| o.gnr).sum(),
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
        let now = 100 * 3600;
        let trades = vec![
            (t(5_000, 100_000_000), now - 600),
            (t(5_200, 100_000_000), now - 500),
            (t(5_100, 100_000_000), now - 400),
            // A fat-finger trade of 500 GNR at 400,000× the price is left out entirely.
            (t(1_000_000_000_000, 50_000_000_000), now - 300),
        ];
        let offers = vec![
            offer(Side::SellGnr, 100_000_000, 6_000),
            offer(Side::BuyGnr, 100_000_000, 4_000),
        ];
        let m = market(&trades, &offers, &[], now);
        assert_eq!(m.trades, 4);
        assert_eq!(m.price, Some(5_100.0));
        assert_eq!((m.best_ask, m.best_bid), (Some(6_000.0), Some(4_000.0)));
        assert_eq!(m.source.as_deref(), Some("swaps"));
        assert_eq!((m.asks, m.bids), (1, 1));
        assert!(m.confidence > 0.2 && m.confidence < 0.5, "{}", m.confidence);
        assert_eq!(weighted_median(vec![]), None);
        // No swaps yet: the first offer already sets a price, more offers a median.
        let one = market(&[], &offers[..1], &[], now);
        assert_eq!(
            (one.price, one.source.as_deref(), one.depth.as_str()),
            (Some(6_000.0), Some("offers"), "thin")
        );
        let mut book = offers.clone();
        book.push(offer(Side::SellGnr, 300_000_000, 15_000));
        assert_eq!(market(&[], &book, &[], now).price, Some(5_000.0));
        assert_eq!(market(&[], &[], &[], now).price, None);
        // A swap in progress counts twice as much as an offer of the same size.
        let taken = vec![(offer(Side::SellGnr, 100_000_000, 8_000), now)];
        let m = market(
            &[],
            &[offer(Side::SellGnr, 100_000_000, 5_000)],
            &taken,
            now,
        );
        assert_eq!(
            (m.price, m.source.as_deref(), m.pending),
            (Some(8_000.0), Some("pending"), 1)
        );
        // Two equal swaps at 1,000 and 3,000 sat: the midpoint, not the lower one.
        assert_eq!(
            weighted_median(vec![(1_000.0, 10), (3_000.0, 10)]),
            Some(2_000.0)
        );
        assert_eq!(
            weighted_median(vec![(1_000.0, 10), (3_000.0, 11)]),
            Some(3_000.0)
        );
    }

    #[test]
    fn old_data_fades_and_volume_builds_confidence() {
        let t = |sats: u64, gnr: u64| Trade {
            network: "devnet".into(),
            hash: "00".repeat(32),
            gnr,
            sats,
            nonce: 0,
        };
        let now = 200 * 3600;
        // A big swap two days ago and a smaller one now: the recent one wins.
        // (sats are the whole bitcoin amount: price × GNR)
        let trades = vec![
            (t(40_000, 1_000_000_000), now - 48 * 3600),
            (t(24_000, 400_000_000), now - 60),
        ];
        assert_eq!(market(&trades, &[], &[], now).price, Some(6_000.0));
        // Beyond 72 hours a swap no longer counts at all.
        let old = vec![(t(40_000, 1_000_000_000), now - 73 * 3600)];
        assert_eq!(market(&old, &[], &[], now).price, None);
        // Confidence: thin with a few GNR, deep with a lot.
        let deep = vec![
            (t(500_000, 100 * 100_000_000), now - 60),
            (t(500_000, 100 * 100_000_000), now - 30),
        ];
        let m = market(&deep, &[], &[], now);
        assert_eq!(m.depth, "deep");
        assert!(m.confidence > 0.9);
    }

    #[test]
    fn a_real_swap_is_not_cut_by_one_offer() {
        let t = |sats: u64, gnr: u64| Trade {
            network: "devnet".into(),
            hash: "00".repeat(32),
            gnr,
            sats,
            nonce: 0,
        };
        let now = 300 * 3600;
        // 10 GNR swapped at 1,000 sat five hours ago, one 10 GNR offer at 10,000 sat:
        // two points, no outlier; the swap weighs more.
        let trades = vec![(t(10_000, 1_000_000_000), now - 5 * 3600)];
        let offers = vec![offer(Side::SellGnr, 1_000_000_000, 100_000)];
        let m = market(&trades, &offers, &[], now);
        assert_eq!(
            (m.price, m.source.as_deref()),
            (Some(1_000.0), Some("swaps"))
        );
        // A day later the old swap has faded below the offer.
        let m = market(&trades, &offers, &[], now + 19 * 3600);
        assert_eq!(m.price, Some(10_000.0));
        // Many cheap offers cannot cut the swaps: kinds count in the centre.
        let swaps: Vec<_> = (0..3)
            .map(|i| (t(5_000, 100_000_000), now - 60 * i))
            .collect();
        let spam: Vec<_> = (0..8)
            .map(|_| offer(Side::SellGnr, 100_000_000, 1_000_000))
            .collect();
        let m = market(&swaps, &spam, &[], now);
        assert_eq!(m.source.as_deref(), Some("swaps"), "{m:?}");
    }
}

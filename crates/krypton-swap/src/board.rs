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

// Proof of funds for sell offers.
//
// A wallet can only list GNR it holds: with a sell offer it sends the board a proof that
// it can spend a note worth the offer and the fee of the lock that a take needs. The
// proof is an ordinary shielded transfer of the offer's amount to a note that is fixed
// by the offer itself, so it cannot back any other offer, and it can never be mined: it
// is expired from the start. Nothing moves. The board learns the nullifiers of the notes
// behind the offer, watches them, and takes the offer down the moment one is spent.

/// The height a proof of funds "expires" at: before every block that will ever be mined.
pub const RESERVE_EXPIRY: u64 = 1;
/// What the proof must leave for the lock transaction of a take.
pub const RESERVE_FEE: u64 = krypton_wallet_core::DEFAULT_FEE;
/// The board's reply to a sell offer without a proof (wallets before 1.0.40).
pub const RESERVE_MISSING: &str = "sell offers must prove their GNR since 1.0.40: update your wallet (About, Check for updates) and publish the offer again";

/// What a board keeps of a proof of funds: the notes behind an offer, by their
/// nullifiers, and the note tree root they were proven against.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Backing {
    pub nullifiers: [String; 2],
    pub anchor: String,
}

/// The note a proof of funds for `o` must pay: its owner and randomness follow from the
/// offer (id, key, cancel hash, amount, network), so the proof fits this offer only.
pub fn reserve_output(o: &Offer) -> (krypton_crypto::field::Digest, krypton_crypto::field::Digest) {
    use krypton_crypto::field::digest_from_uniform;
    let gnr = o.gnr.to_le_bytes();
    let parts: [&[u8]; 5] = [
        o.id.as_bytes(),
        o.key.as_bytes(),
        o.cancel.as_bytes(),
        &gnr,
        o.network.as_bytes(),
    ];
    (
        digest_from_uniform(&tagged_parts("Krypton/board/reserve/owner", &parts)),
        digest_from_uniform(&tagged_parts("Krypton/board/reserve/rcm", &parts)),
    )
}

/// Commitment of that note.
pub fn reserve_cm(o: &Offer) -> krypton_core::Hash32 {
    let (owner, rcm) = reserve_output(o);
    krypton_crypto::field::digest_to_bytes(&krypton_crypto::rescue::note_commitment_asset(
        &owner, o.gnr, &[0; 3], &rcm,
    ))
}

/// Checks a sell offer's proof of funds as far as it can be checked without the chain:
/// its shape, that it pays this offer's note the offer's amount and leaves the lock fee,
/// and the zero-knowledge proof itself. The caller must still check, against its own
/// chain, that the anchor is a root of the main chain and that neither nullifier is
/// spent, and must not let the same nullifier back two offers.
pub fn check_reserve(o: &Offer, tx: &krypton_core::types::Transaction) -> Result<Backing> {
    use krypton_core::types::TX_VERSION;
    ensure!(o.side == Side::SellGnr, "only sell offers prove funds");
    // Plain GNR notes only: a lock note or a coin is not free to sell.
    ensure!(
        tx.version == TX_VERSION && tx.lock.is_none() && tx.asset.is_none(),
        "the proof of funds must be a plain GNR transfer"
    );
    ensure!(
        tx.expiry_height == RESERVE_EXPIRY,
        "the proof of funds must not be a transaction that can be mined"
    );
    ensure!(tx.mint == 0, "the proof of funds mints");
    ensure!(
        (RESERVE_FEE..=krypton_wallet_core::MAX_FEE).contains(&tx.fee),
        "the proof of funds leaves no room for the fee of the lock"
    );
    ensure!(
        tx.outputs[0].cm == reserve_cm(o),
        "the proof of funds is for another offer or another amount"
    );
    let st = krypton_core::consensus::check_tx_structure(tx)
        .map_err(|e| anyhow!("bad proof of funds: {e}"))?;
    krypton_core::consensus::verify_proof(tx, &st)
        .map_err(|e| anyhow!("bad proof of funds: {e}"))?;
    Ok(Backing {
        nullifiers: [
            hex::encode(tx.nullifiers[0].0),
            hex::encode(tx.nullifiers[1].0),
        ],
        anchor: hex::encode(tx.anchor),
    })
}

// Bitcoin behind buy offers.
//
// A buy offer can come with the bitcoin already paid to an address of the buyer's own
// wallet (`crate::deposit`). The wallet proves that the address is its own by signing the
// offer with the address's key. A board that can read the Bitcoin chain then checks that
// the offer's amount is there, unspent, and marks the offer; it keeps the address to
// itself. From that address the buyer's wallet pays the swap contract by itself.

/// What a wallet sends with a buy offer whose bitcoin is deposited: the public key of the
/// deposit address and its signature over the offer (both hex).
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Funding {
    pub key: String,
    pub sig: String,
}

fn funding_digest(o: &Offer) -> [u8; 32] {
    use bitcoin::hashes::{Hash, sha256};
    let (gnr, sats) = (o.gnr.to_le_bytes(), o.sats.to_le_bytes());
    let parts: [&[u8]; 7] = [
        o.id.as_bytes(),
        o.key.as_bytes(),
        o.cancel.as_bytes(),
        &gnr,
        &sats,
        o.network.as_bytes(),
        o.btc_network.as_bytes(),
    ];
    sha256::Hash::hash(&tagged_parts("Krypton/board/funded", &parts)).to_byte_array()
}

/// Signs `o` with the key of the deposit address that holds its bitcoin.
pub fn sign_funding(o: &Offer, key: &bitcoin::secp256k1::SecretKey) -> Funding {
    use bitcoin::secp256k1::{Message, Secp256k1};
    let secp = Secp256k1::signing_only();
    let sig = secp.sign_ecdsa(&Message::from_digest(funding_digest(o)), key);
    Funding {
        key: hex::encode(key.public_key(&secp).serialize()),
        sig: hex::encode(sig.serialize_compact()),
    }
}

/// Checks that `f` was made for the buy offer `o` by the owner of a deposit address, and
/// returns that address. Whether the bitcoin is there is for the caller to look up on the
/// Bitcoin chain: unspent outputs of that address worth at least `o.sats`.
pub fn check_funding(o: &Offer, f: &Funding) -> Result<bitcoin::Address> {
    use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, ecdsa::Signature};
    ensure!(o.side == Side::BuyGnr, "only buy offers deposit bitcoin");
    let network = bitcoin::Network::from_core_arg(&o.btc_network)
        .map_err(|_| anyhow!("unknown Bitcoin network"))?;
    ensure!(f.key.len() == 66 && f.sig.len() == 128, "bad deposit proof");
    let key = PublicKey::from_slice(&hex::decode(&f.key).context("bad deposit key")?)
        .map_err(|_| anyhow!("bad deposit key"))?;
    let sig = Signature::from_compact(&hex::decode(&f.sig).context("bad deposit signature")?)
        .map_err(|_| anyhow!("bad deposit signature"))?;
    Secp256k1::verification_only()
        .verify_ecdsa(&Message::from_digest(funding_digest(o)), &sig, &key)
        .map_err(|_| anyhow!("the deposit proof is not for this offer"))?;
    Ok(bitcoin::Address::p2wpkh(
        &bitcoin::CompressedPublicKey(key),
        network,
    ))
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
    /// A seller of GNR takes a buy offer: the maker answers with a request. From 1.0.46
    /// it may name a part of a Quick Swap offer ([`Auto`]): the GNR it sells and the
    /// bitcoin it asks for them ([`part_ok`]). Without them it takes the whole offer.
    Take {
        offer: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        gnr: Option<u64>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        sats: Option<u64>,
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

/// What makes a trade report count by its bitcoin (1.0.44): the swap's bitcoin contract,
/// and the signature of one of the contract's two keys over the report. Only the two
/// wallets of a swap hold those keys, so nobody else can report it; and the contract
/// follows from the keys and the trade's hash lock, so the board can look up on the
/// Bitcoin chain how much was taken out of it with the swap's secret.
/// It travels next to the trade, not inside it: older boards read the trade as before.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Paid {
    pub btc_network: String,
    /// The contract's keys (hex) and refund height.
    pub claim: String,
    pub refund: String,
    pub refund_height: u32,
    /// The payment into the contract (`txid:vout`) and the Bitcoin block that holds it.
    pub funding: String,
    pub funded_height: u32,
    /// The key that signed, one of the two above, and its signature.
    pub key: String,
    pub sig: String,
}

fn paid_digest(t: &Trade, p: &Paid) -> [u8; 32] {
    use bitcoin::hashes::{Hash, sha256};
    let (gnr, sats, height, funded) = (
        t.gnr.to_le_bytes(),
        t.sats.to_le_bytes(),
        p.refund_height.to_le_bytes(),
        p.funded_height.to_le_bytes(),
    );
    let parts: [&[u8]; 10] = [
        t.network.as_bytes(),
        t.hash.as_bytes(),
        &gnr,
        &sats,
        p.btc_network.as_bytes(),
        p.claim.as_bytes(),
        p.refund.as_bytes(),
        &height,
        p.funding.as_bytes(),
        &funded,
    ];
    sha256::Hash::hash(&tagged_parts("Krypton/board/paid", &parts)).to_byte_array()
}

/// Signs the report `t` of a swap whose bitcoin contract is `htlc`, paid by `funding`
/// in the Bitcoin block `funded_height`, with this wallet's key of that contract.
pub fn sign_paid(
    t: &Trade,
    htlc: &crate::Htlc,
    network: bitcoin::Network,
    funding: &bitcoin::OutPoint,
    funded_height: u32,
    key: &bitcoin::secp256k1::SecretKey,
) -> Paid {
    use bitcoin::secp256k1::{Message, Secp256k1};
    let secp = Secp256k1::signing_only();
    let mut p = Paid {
        btc_network: network.to_core_arg().into(),
        claim: htlc.claim.to_string(),
        refund: htlc.refund.to_string(),
        refund_height: htlc.refund_height,
        funding: funding.to_string(),
        funded_height,
        key: hex::encode(key.public_key(&secp).serialize()),
        sig: String::new(),
    };
    let sig = secp.sign_ecdsa(&Message::from_digest(paid_digest(t, &p)), key);
    p.sig = hex::encode(sig.serialize_compact());
    p
}

/// Checks that `p` was made for the report `t` by one of the two keys of the bitcoin
/// contract it names, and returns that contract and the payment into it. What the
/// Bitcoin chain shows of them is for the caller to look up
/// ([`crate::btc::claimed_from`]).
pub fn check_paid(t: &Trade, p: &Paid) -> Result<(crate::Htlc, bitcoin::OutPoint)> {
    use bitcoin::secp256k1::{Message, PublicKey, Secp256k1, ecdsa::Signature};
    use std::str::FromStr;
    bitcoin::Network::from_core_arg(&p.btc_network)
        .map_err(|_| anyhow!("unknown Bitcoin network"))?;
    ensure!(
        p.claim.len() == 66
            && p.refund.len() == 66
            && p.key.len() == 66
            && p.sig.len() == 128
            && p.funding.len() <= 80,
        "bad swap proof"
    );
    let funding = bitcoin::OutPoint::from_str(&p.funding).map_err(|_| anyhow!("bad payment"))?;
    ensure!(
        p.key.eq_ignore_ascii_case(&p.claim) || p.key.eq_ignore_ascii_case(&p.refund),
        "the swap proof is not signed by a key of its contract"
    );
    let hash: [u8; 32] = hex::decode(&t.hash)
        .ok()
        .and_then(|h| h.try_into().ok())
        .context("bad hash")?;
    let key = PublicKey::from_slice(&hex::decode(&p.key).context("bad key")?)
        .map_err(|_| anyhow!("bad key"))?;
    let sig = Signature::from_compact(&hex::decode(&p.sig).context("bad signature")?)
        .map_err(|_| anyhow!("bad signature"))?;
    Secp256k1::verification_only()
        .verify_ecdsa(&Message::from_digest(paid_digest(t, p)), &sig, &key)
        .map_err(|_| anyhow!("the swap proof is not for this report"))?;
    let htlc = crate::Htlc::new(
        hash,
        bitcoin::PublicKey::from_str(&p.claim).map_err(|_| anyhow!("bad claim key"))?,
        bitcoin::PublicKey::from_str(&p.refund).map_err(|_| anyhow!("bad refund key"))?,
        p.refund_height,
    )
    .map_err(|e| anyhow!("bad contract: {e}"))?;
    Ok((htlc, funding))
}

/// The market from the board's data.
#[derive(Clone, Debug, Default, Serialize, Deserialize, PartialEq)]
pub struct Market {
    /// The market price in sat per GNR, from completed swaps (see [`market`]). `None`
    /// while the board has none.
    pub price: Option<f64>,
    /// What the price rests on: `verified swaps` (their bitcoin was found on the Bitcoin
    /// chain; the price is their volume-weighted median) or `swaps` (too few of those
    /// yet: the plain average of the reported ones).
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
    /// How many completed swaps the price is computed from.
    #[serde(default)]
    pub priced: usize,
    /// Swaps of the price window whose bitcoin the board found on the Bitcoin chain, and
    /// that bitcoin (sat).
    #[serde(default)]
    pub verified: usize,
    #[serde(default)]
    pub verified_sats: u64,
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

/// The price comes from the completed swaps of the last 72 hours (all kept swaps while
/// there are none that recent).
pub const MARKET_WINDOW_SECS: u64 = 72 * 3600;
/// With three or more swaps, a swap more than this factor away from their median is left
/// out: a swap reports its own GNR amount (it is private on chain), so a typo or a fake
/// report must not move the price.
pub const OUTLIER_FACTOR: f64 = 4.0;
/// From this many verified swaps in the window the price rests on them alone.
pub const MIN_VERIFIED: usize = 3;
/// No single swap carries more than this share of the weight behind the price.
pub const MAX_SHARE: f64 = 0.2;
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

/// The completed swaps of the price window: those of the last 72 hours, or all of them
/// while none is that recent.
fn window(trades: &[(Trade, u64)], now: u64) -> Vec<&Trade> {
    let ok = |t: &&Trade| t.price().is_finite() && t.price() > 0.0;
    let v: Vec<&Trade> = trades
        .iter()
        .filter(|(_, at)| *at + MARKET_WINDOW_SECS > now)
        .map(|(t, _)| t)
        .filter(ok)
        .collect();
    if v.is_empty() {
        trades.iter().map(|(t, _)| t).filter(ok).collect()
    } else {
        v
    }
}

/// Caps every weight so that none is more than [`MAX_SHARE`] of the total (with five
/// swaps or fewer that means: all the same).
fn cap_shares(w: &mut [f64]) {
    if w.is_empty() {
        return;
    }
    let least = w.iter().copied().fold(f64::INFINITY, f64::min);
    if (w.len() as f64) * MAX_SHARE <= 1.0 {
        w.iter_mut().for_each(|x| *x = least);
        return;
    }
    // The largest cap c with c = MAX_SHARE × Σ min(w, c), approached from above.
    let mut c = w.iter().copied().fold(0.0, f64::max);
    for _ in 0..64 {
        let next = MAX_SHARE * w.iter().map(|x| x.min(c)).sum::<f64>();
        if next >= c * (1.0 - 1e-9) {
            break;
        }
        c = next.max(least);
    }
    w.iter_mut().for_each(|x| *x = x.min(c));
}

/// The swaps the price is computed from, each with its weight, and whether they are the
/// verified ones.
///
/// * **Verified swaps** (`verified`: hash lock → sat found on the Bitcoin chain, paid into
///   the swap's contract and taken out with its secret), once the window has
///   [`MIN_VERIFIED`] of them: each weighs the bitcoin it moved, but never more than its
///   GNR is worth at the going price (so an odd price cannot buy weight with one large
///   side), and never more than [`MAX_SHARE`] of the total. One or two large swaps
///   therefore cannot set the price; it takes several that together outweigh the rest.
/// * Otherwise every reported swap, each weighing the same, as before 1.0.44.
///
/// Either way a swap more than [`OUTLIER_FACTOR`] from the median is left out once there
/// are three.
pub fn priced_swaps<'a>(
    trades: &'a [(Trade, u64)],
    verified: &std::collections::HashMap<String, u64>,
    now: u64,
) -> (Vec<(&'a Trade, f64)>, bool) {
    let all = window(trades, now);
    let proven: Vec<(&Trade, f64)> = all
        .iter()
        .filter_map(|t| {
            verified
                .get(&t.hash)
                .map(|sats| (*t, (*sats).min(t.sats) as f64))
        })
        .filter(|(_, w)| *w > 0.0)
        .collect();
    let by_volume = proven.len() >= MIN_VERIFIED;
    let mut v: Vec<(&Trade, f64)> = if by_volume {
        proven
    } else {
        all.into_iter().map(|t| (t, 1.0)).collect()
    };
    // The going price is taken with the shares already capped: one huge swap at an odd
    // price cannot make itself the yardstick and the others the outliers.
    let capped = |v: &[(&Trade, f64)]| -> Vec<f64> {
        let mut w: Vec<f64> = v.iter().map(|x| x.1).collect();
        if by_volume {
            cap_shares(&mut w);
        }
        w
    };
    if v.len() >= 3
        && let Some(m) = weighted_median_f(
            v.iter()
                .zip(capped(&v))
                .map(|((t, _), w)| (t.price(), w))
                .collect(),
        )
    {
        v.retain(|(t, _)| t.price() <= m * OUTLIER_FACTOR && t.price() >= m / OUTLIER_FACTOR);
        if by_volume {
            for (t, w) in v.iter_mut() {
                *w = w.min(t.gnr as f64 / 1e8 * m);
            }
        }
    }
    let w = capped(&v);
    for (x, w) in v.iter_mut().zip(w) {
        x.1 = w;
    }
    (v, by_volume)
}

/// The market price, from completed swaps (each one's claim is on the GNR chain).
///
/// With enough swaps whose bitcoin the board found on the Bitcoin chain, the price is
/// their **volume-weighted median**: half of the bitcoin that changed hands did so below
/// it, half above. Moving it takes more bitcoin, really paid through swap contracts, than
/// everyone else traded in the window; a report that is not one of a swap's two wallets'
/// or that names no payment weighs nothing. See [`priced_swaps`]. Until there are enough
/// such swaps it is the plain average of the reported ones, as before 1.0.44.
///
/// Open offers and swaps in progress are counted and shown, but never move the price:
/// anyone can post an offer at any price, and nothing proves a swap in progress runs.
pub fn market(
    trades: &[(Trade, u64)],
    verified: &std::collections::HashMap<String, u64>,
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
    let (swaps, by_volume) = priced_swaps(trades, verified, now);
    let price = if swaps.is_empty() {
        None
    } else if by_volume {
        weighted_median_f(swaps.iter().map(|(t, w)| (t.price(), *w)).collect())
    } else {
        Some(swaps.iter().map(|(t, _)| t.price()).sum::<f64>() / swaps.len() as f64)
    };
    let w: f64 = swaps.iter().map(|(t, _)| t.gnr as f64 / 1e8).sum();
    let confidence = if w > 0.0 { w / (w + DEPTH_GNR) } else { 0.0 };
    let proven: Vec<u64> = window(trades, now)
        .iter()
        .filter_map(|t| verified.get(&t.hash).map(|s| (*s).min(t.sats)))
        .collect();
    Market {
        price,
        source: price.map(|_| if by_volume { "verified swaps" } else { "swaps" }.to_string()),
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
        verified: proven.len(),
        verified_sats: proven.iter().sum(),
        best_ask: ask,
        best_bid: bid,
        offers: offers.len(),
    }
}

// Quick Swap (1.0.46).
//
// An Auto-swap offer is one its maker's wallet sells (or buys) in parts, to anyone, by
// itself: a taker names how much of it, at the offer's price, and the maker's wallet
// swaps exactly that. The offer stands for what it always did (GNR proven to the board,
// or bitcoin deposited): a part takes some of it, and the maker's wallet lists the rest
// again once its coins are back in a block. A taker's wallet buys or sells any amount
// across up to [`MAX_PARTS`] such offers at once, each part its own atomic swap.

/// What the board shows of an Auto-swap offer next to it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Auto {
    /// The least GNR (krypton) one part may be for. The whole offer can always be taken.
    pub min: u64,
}

/// The most offers one Quick Swap is spread over.
pub const MAX_PARTS: usize = 3;
/// The least bitcoin a part may be for (a smaller swap would pay most of itself in fees).
pub const MIN_PART_SATS: u64 = 10_000;
/// The least GNR a part may be for: more than its two fees.
pub const MIN_PART_GNR: u64 = 3 * krypton_wallet_core::DEFAULT_FEE;

/// Whether `a` is a sensible Auto-swap setting for `o`.
pub fn auto_ok(o: &Offer, a: &Auto) -> Result<()> {
    ensure!(
        a.min >= MIN_PART_GNR && a.min <= o.gnr,
        "the smallest part must be at least a few fees and at most the whole offer"
    );
    Ok(())
}

/// The bitcoin for `gnr` of the offer `o` at its price, in whole sat: rounded up when
/// the maker sells (it never gets less than its price) and down when it buys (it never
/// pays more). `None` for more GNR than the offer has.
pub fn part_sats(o: &Offer, gnr: u64) -> Option<u64> {
    if gnr == 0 || gnr > o.gnr || o.gnr == 0 {
        return None;
    }
    if gnr == o.gnr {
        return Some(o.sats);
    }
    let num = gnr as u128 * o.sats as u128;
    let d = o.gnr as u128;
    let s = match o.side {
        Side::SellGnr => num.div_ceil(d),
        Side::BuyGnr => num / d,
    };
    u64::try_from(s).ok()
}

/// Whether a request for `gnr` GNR and `sats` bitcoin is a part of the Auto-swap offer
/// `o` that its maker's wallet swaps: the whole offer, or at least its smallest part, at
/// no worse a price for the maker than the offer's own.
pub fn part_ok(o: &Offer, a: Option<&Auto>, gnr: u64, sats: u64) -> Result<()> {
    if (gnr, sats) == (o.gnr, o.sats) {
        return Ok(());
    }
    let a = a.context("this offer is taken whole only")?;
    ensure!(gnr > 0 && gnr < o.gnr, "a part is more than the offer");
    ensure!(
        sats <= o.sats,
        "a part asks for more bitcoin than the offer"
    );
    ensure!(
        gnr >= a.min.max(MIN_PART_GNR),
        "a part smaller than the offer's smallest part"
    );
    ensure!(sats >= MIN_PART_SATS, "a part of less than 10,000 sat");
    let (s, g) = (sats as u128 * o.gnr as u128, gnr as u128 * o.sats as u128);
    match o.side {
        Side::SellGnr => ensure!(s >= g, "a part below the offer's price"),
        Side::BuyGnr => ensure!(s <= g, "a part above the offer's price"),
    }
    Ok(())
}

/// One part of a Quick Swap: an offer and how much of it.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Part {
    pub id: String,
    pub gnr: u64,
    pub sats: u64,
}

/// What a Quick Swap is for: GNR, or bitcoin (to spend when buying, to receive when
/// selling).
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Want {
    Gnr(u64),
    Sats(u64),
}

/// The best way to buy (from the sell offers among `offers`, `buying`) or sell (to the
/// buy offers) `want` across at most `max_parts` Auto-swap offers: best price first,
/// whole offers where they fit, and one part each. Offers whose smallest part is more
/// than what is left are passed over. The plan comes short of `want` when there is not
/// enough on the board.
pub fn plan(offers: &[(Offer, Auto)], buying: bool, want: Want, max_parts: usize) -> Vec<Part> {
    let mut v: Vec<&(Offer, Auto)> = offers
        .iter()
        .filter(|(o, a)| o.gnr > 0 && o.sats > 0 && auto_ok(o, a).is_ok())
        .filter(|(o, _)| (o.side == Side::SellGnr) == buying)
        .collect();
    // Best first: cheapest to buy from, dearest to sell to. Larger offers first at the
    // same price, so fewer parts are needed.
    v.sort_by(|a, b| {
        let (pa, pb) = (a.0.price(), b.0.price());
        let by = if buying {
            pa.total_cmp(&pb)
        } else {
            pb.total_cmp(&pa)
        };
        by.then(b.0.gnr.cmp(&a.0.gnr))
    });
    let mut left = match want {
        Want::Gnr(g) | Want::Sats(g) => g,
    };
    let mut parts = Vec::new();
    for (o, a) in v {
        if parts.len() >= max_parts || left == 0 {
            break;
        }
        // The GNR of this part: all that is left of what is wanted, at most the offer.
        let gnr = match want {
            Want::Gnr(_) => left.min(o.gnr),
            Want::Sats(_) if left >= o.sats => o.gnr,
            Want::Sats(_) => {
                // As much GNR as `left` sat pay for (buying) or bring (selling).
                let mut g = (left as u128 * o.gnr as u128 / o.sats as u128) as u64;
                while g > 0 && part_sats(o, g).is_some_and(|s| s > left) {
                    g -= 1;
                }
                g
            }
        };
        let Some(sats) = part_sats(o, gnr) else {
            continue;
        };
        if part_ok(o, Some(a), gnr, sats).is_err() {
            continue;
        }
        left -= match want {
            Want::Gnr(_) => gnr,
            Want::Sats(_) => sats.min(left),
        };
        parts.push(Part {
            id: o.id.clone(),
            gnr,
            sats,
        });
    }
    parts
}

/// What a seller's wallet shows the board about a buyer's deposit before it locks GNR
/// for that buyer (1.0.46): the buyer's own signed word from the request
/// (`engine::Prepaid`), so the board can add up what one deposit address is promised to
/// across all sellers. A buyer with bitcoin for one swap then cannot have the GNR of many
/// sellers locked for it. `release` is the hash of a secret only the seller's wallet
/// knows; it takes the promise back when the swap is paid or over.
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Promise {
    pub id: String,
    pub gnr: u64,
    pub sats: u64,
    pub refund_pub: String,
    pub btc_network: String,
    pub key: String,
    pub sig: String,
    pub release: String,
}

/// How long a board counts a promise: a GNR-first swap is paid within two hours of its
/// lock, which confirms within the first hour.
pub const PROMISE_SECS: u64 = 4 * 3600;

pub fn release_hash(secret: &[u8; 32]) -> String {
    hex::encode(tagged("Krypton/board/promise-release", secret))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_deposit_proof_fits_one_buy_offer_and_one_key() {
        let offer = |id: u8, side: Side, sats: u64| Offer {
            id: hex::encode([id; 16]),
            network: "mainnet".into(),
            btc_network: "main".into(),
            side,
            gnr: 5 * 100_000_000,
            sats,
            key: "aa".repeat(8),
            expires: 2_000_000_000,
            cancel: "bb".repeat(32),
            nonce: 0,
        };
        let seed = [3u8; 64];
        let key = crate::deposit::key(&seed, 2);
        let o = offer(1, Side::BuyGnr, 50_000);
        let f = sign_funding(&o, &key);
        let address = check_funding(&o, &f).unwrap();
        assert_eq!(
            address,
            crate::deposit::address(&seed, 2, bitcoin::Network::Bitcoin)
        );
        // The proof of work is not part of it: sealing the offer keeps the proof valid.
        let mut sealed = o.clone();
        sealed.nonce = 77;
        assert!(check_funding(&sealed, &f).is_ok());
        // Another offer, another amount, a sell offer: not this proof.
        assert!(check_funding(&offer(2, Side::BuyGnr, 50_000), &f).is_err());
        assert!(check_funding(&offer(1, Side::BuyGnr, 60_000), &f).is_err());
        assert!(check_funding(&offer(1, Side::SellGnr, 50_000), &f).is_err());
        // Someone else's address cannot be claimed: the key must have signed.
        let other = sign_funding(&o, &crate::deposit::key(&seed, 3));
        let forged = Funding {
            key: f.key.clone(),
            sig: other.sig,
        };
        assert!(check_funding(&o, &forged).is_err());
        for bad in [
            Funding {
                key: "00".repeat(33),
                sig: f.sig.clone(),
            },
            Funding {
                key: f.key.clone(),
                sig: "zz".repeat(64),
            },
            Funding {
                key: String::new(),
                sig: String::new(),
            },
        ] {
            assert!(check_funding(&o, &bad).is_err());
        }
        let mut regtest = o.clone();
        regtest.btc_network = "regtest".into();
        let f2 = sign_funding(&regtest, &key);
        assert!(
            check_funding(&regtest, &f2)
                .unwrap()
                .to_string()
                .starts_with("bcrt1q")
        );
    }

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

    fn none() -> std::collections::HashMap<String, u64> {
        Default::default()
    }

    /// A swap with its own hash lock, and the same swap as the Bitcoin chain shows it.
    fn swap(n: u8, sats: u64, gnr: u64) -> (Trade, (String, u64)) {
        let mut t = trade(sats, gnr);
        t.hash = hex::encode([n; 32]);
        let seen = (t.hash.clone(), sats);
        (t, seen)
    }

    #[test]
    fn the_price_of_verified_swaps_is_where_half_the_bitcoin_changed_hands() {
        let now = 500 * 3600;
        let g = 100_000_000u64;
        // Five swaps around 5,000 sat per GNR, 0.001 to 0.004 BTC each.
        let honest = [
            swap(1, 100_000, 20 * g),
            swap(2, 204_000, 40 * g),
            swap(3, 400_000, 80 * g),
            swap(4, 245_000, 50 * g),
            swap(5, 153_000, 30 * g),
        ];
        let mut trades: Vec<(Trade, u64)> =
            honest.iter().map(|x| (x.0.clone(), now - 60)).collect();
        let mut seen: std::collections::HashMap<String, u64> =
            honest.iter().map(|x| x.1.clone()).collect();
        let m = market(&trades, &seen, &[], &[], now);
        assert_eq!(m.source.as_deref(), Some("verified swaps"));
        assert_eq!((m.priced, m.verified), (5, 5));
        let fair = m.price.unwrap();
        assert!((4_900.0..=5_100.0).contains(&fair), "{fair}");

        // Reports nobody proved weigh nothing, however many and whatever they say.
        for i in 0..50u8 {
            let mut t = trade(1_000_000, g);
            t.hash = hex::encode([100 + i; 32]);
            trades.push((t, now - 30));
        }
        assert_eq!(market(&trades, &seen, &[], &[], now).price, Some(fair));

        // One real swap of 1 BTC for 2 GNR (a hundred times the price): it is left out as
        // an outlier; at three times the price it stays, but weighs only what its 2 GNR
        // are worth at the going price, not its bitcoin.
        let (far, far_seen) = swap(60, 100_000_000, 2 * g);
        let (odd, odd_seen) = swap(61, 30_000, 2 * g);
        let mut with = trades.clone();
        let mut seen_with = seen.clone();
        with.push((far, now - 10));
        with.push((odd, now - 10));
        seen_with.extend([far_seen, odd_seen]);
        let m = market(&with, &seen_with, &[], &[], now);
        assert_eq!(m.priced, 6);
        assert!((4_900.0..=5_100.0).contains(&m.price.unwrap()), "{m:?}");

        // One whale cannot be the price: a single swap is at most a fifth of the weight.
        let (whale, whale_seen) = swap(70, 6_000 * 2_000, 2_000 * g);
        trades.push((whale, now - 5));
        seen.insert(whale_seen.0, whale_seen.1);
        let m = market(&trades, &seen, &[], &[], now);
        assert!((4_900.0..=5_100.0).contains(&m.price.unwrap()), "{m:?}");

        // Nor can two. It takes several large swaps that together outweigh the rest.
        let mut more = trades.clone();
        let mut seen_more = seen.clone();
        let (t, s) = swap(71, 6_000 * 2_000, 2_000 * g);
        more.push((t, now - 4));
        seen_more.insert(s.0, s.1);
        let two = market(&more, &seen_more, &[], &[], now).price.unwrap();
        assert!(two < 5_200.0, "{two}");
        let (t, s) = swap(72, 6_000 * 2_000, 2_000 * g);
        more.push((t, now - 3));
        seen_more.insert(s.0, s.1);
        assert!(close(
            market(&more, &seen_more, &[], &[], now).price,
            6_000.0
        ));
    }

    #[test]
    fn too_few_verified_swaps_leave_the_price_as_it_was() {
        let now = 600 * 3600;
        let g = 100_000_000u64;
        let (a, sa) = swap(1, 5_000, g);
        let (b, sb) = swap(2, 7_000, g);
        let (c, _) = swap(3, 6_000, g);
        let trades = vec![(a, now - 3), (b, now - 2), (c, now - 1)];
        let seen: std::collections::HashMap<String, u64> = [sa, sb].into_iter().collect();
        let m = market(&trades, &seen, &[], &[], now);
        assert_eq!(m.source.as_deref(), Some("swaps"));
        assert_eq!((m.priced, m.verified, m.verified_sats), (3, 2, 12_000));
        assert!(close(m.price, 6_000.0));
        // A weight is never more than what the report itself says was paid.
        let (d, mut sd) = swap(4, 5_000, g);
        sd.1 = 9_000_000;
        let same = [(d.clone(), now), (d.clone(), now), (d, now)];
        let (v, by_volume) = priced_swaps(&same, &[sd].into_iter().collect(), now);
        assert!(by_volume && v.iter().all(|(_, w)| *w <= 5_000.0));
    }

    #[test]
    fn shares_are_capped_without_losing_the_weight() {
        let mut w = vec![1.0, 1.0, 1.0, 1.0, 1.0, 1.0, 100.0];
        cap_shares(&mut w);
        let total: f64 = w.iter().sum();
        assert!(w.iter().all(|x| *x <= total * MAX_SHARE + 1e-6), "{w:?}");
        assert!((w[6] - 1.5).abs() < 1e-6 && w[0] == 1.0, "{w:?}");
        let mut few = vec![5.0, 1.0, 3.0];
        cap_shares(&mut few);
        assert_eq!(few, vec![1.0, 1.0, 1.0]);
        let mut even = vec![2.0; 8];
        cap_shares(&mut even);
        assert_eq!(even, vec![2.0; 8]);
    }

    #[test]
    fn a_swap_proof_is_one_partys_word_for_one_report() {
        use bitcoin::secp256k1::{Secp256k1, SecretKey};
        let secp = Secp256k1::signing_only();
        let (claim, refund, other) = (
            SecretKey::from_slice(&[7; 32]).unwrap(),
            SecretKey::from_slice(&[8; 32]).unwrap(),
            SecretKey::from_slice(&[9; 32]).unwrap(),
        );
        let key = |k: &SecretKey| bitcoin::PublicKey::new(k.public_key(&secp));
        let (t, _) = swap(5, 150_000, 30 * 100_000_000);
        let htlc = crate::Htlc::new([5; 32], key(&claim), key(&refund), 900_000).unwrap();
        let net = bitcoin::Network::Bitcoin;
        let paid = bitcoin::OutPoint::new("aa".repeat(32).parse().unwrap(), 1);
        for k in [&claim, &refund] {
            let p = sign_paid(&t, &htlc, net, &paid, 880_000, k);
            assert_eq!(check_paid(&t, &p).unwrap(), (htlc.clone(), paid));
            let mut moved = p.clone();
            moved.funding = format!("{}:2", "aa".repeat(32));
            assert!(check_paid(&t, &moved).is_err());
            // Another amount, another swap, another contract: not this proof.
            let mut more = t.clone();
            more.gnr += 1;
            assert!(check_paid(&more, &p).is_err());
            let mut elsewhere = t.clone();
            elsewhere.hash = hex::encode([6; 32]);
            assert!(check_paid(&elsewhere, &p).is_err());
            let mut later = p.clone();
            later.refund_height += 1;
            assert!(check_paid(&t, &later).is_err());
        }
        // A key that is not in the contract cannot speak for it, even with a good
        // signature of its own.
        let mut p = sign_paid(&t, &htlc, net, &paid, 880_000, &other);
        assert!(check_paid(&t, &p).is_err());
        p.key = p.claim.clone();
        assert!(check_paid(&t, &p).is_err());
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
        let m = market(&trades, &none(), &[], &[], now);
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
        assert!(close(market(&two, &none(), &[], &[], now).price, 2_000.0));
        // No swap at all: no price, however many offers.
        let offers = vec![
            offer(Side::SellGnr, 100_000_000, 6_000),
            offer(Side::BuyGnr, 100_000_000, 4_000),
        ];
        let m = market(&[], &none(), &offers, &[], now);
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
        let m = market(&swaps, &none(), &spam, &fake, now);
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
        assert!(close(
            market(&trades, &none(), &[], &[], now).price,
            6_000.0
        ));
        // With only old swaps, their average.
        let old = vec![
            (trade(4_000, 100_000_000), now - 100 * 3600),
            (trade(6_000, 100_000_000), now - 90 * 3600),
        ];
        assert!(close(market(&old, &none(), &[], &[], now).price, 5_000.0));
        assert_eq!(market(&[], &none(), &[], &[], now).price, None);
        let deep = vec![
            (trade(500_000, 100 * 100_000_000), now - 60),
            (trade(500_000, 100 * 100_000_000), now - 30),
        ];
        let m = market(&deep, &none(), &[], &[], now);
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

#[cfg(test)]
mod quick_tests {
    use super::*;

    const GNR: u64 = 100_000_000;

    fn offer(id: &str, side: Side, gnr: u64, sats: u64) -> Offer {
        Offer {
            id: id.into(),
            network: "devnet".into(),
            btc_network: "regtest".into(),
            side,
            gnr,
            sats,
            key: String::new(),
            expires: 0,
            cancel: String::new(),
            nonce: 0,
        }
    }

    fn auto(min: u64) -> Auto {
        Auto { min }
    }

    #[test]
    fn a_part_is_priced_in_the_makers_favour() {
        // 3 GNR for 100,001 sat: a third of the GNR.
        let sell = offer("s", Side::SellGnr, 3 * GNR, 100_001);
        let buy = offer("b", Side::BuyGnr, 3 * GNR, 100_001);
        assert_eq!(part_sats(&sell, GNR), Some(33_334), "the seller rounds up");
        assert_eq!(part_sats(&buy, GNR), Some(33_333), "the buyer rounds down");
        assert_eq!(
            part_sats(&sell, 3 * GNR),
            Some(100_001),
            "the whole offer is exact"
        );
        assert_eq!(part_sats(&sell, 3 * GNR + 1), None);
        assert_eq!(part_sats(&sell, 0), None);
        // No overflow with the largest amounts.
        let big = offer("x", Side::SellGnr, 21_000_000 * GNR, MAX_OFFER_SATS);
        assert_eq!(part_sats(&big, 21_000_000 * GNR - 1), Some(MAX_OFFER_SATS));
    }

    #[test]
    fn a_maker_swaps_parts_only_at_its_price_and_size() {
        let a = auto(GNR / 2);
        let sell = offer("s", Side::SellGnr, 4 * GNR, 400_000);
        let buy = offer("b", Side::BuyGnr, 4 * GNR, 400_000);
        // The whole offer, Auto-swap or not.
        assert!(part_ok(&sell, None, 4 * GNR, 400_000).is_ok());
        // Parts only of Auto-swap offers.
        assert!(part_ok(&sell, None, GNR, 100_000).is_err());
        assert!(part_ok(&sell, Some(&a), GNR, 100_000).is_ok());
        assert!(part_ok(&buy, Some(&a), GNR, 100_000).is_ok());
        // A seller does not sell cheaper, a buyer does not pay more.
        assert!(part_ok(&sell, Some(&a), GNR, 99_999).is_err());
        assert!(part_ok(&sell, Some(&a), GNR, 100_001).is_ok());
        assert!(part_ok(&buy, Some(&a), GNR, 100_001).is_err());
        assert!(part_ok(&buy, Some(&a), GNR, 99_999).is_ok());
        // Not below the smallest part, not below 10,000 sat, never more than the offer.
        assert!(part_ok(&sell, Some(&a), GNR / 2 - 1, 50_000).is_err());
        assert!(part_ok(&sell, Some(&a), GNR / 2, 50_000).is_ok());
        let cheap = offer("c", Side::SellGnr, 4 * GNR, 40_000);
        assert!(part_ok(&cheap, Some(&a), GNR, 10_000).is_ok());
        assert!(part_ok(&cheap, Some(&a), GNR / 2, 5_000).is_err());
        assert!(part_ok(&sell, Some(&a), 5 * GNR, 500_000).is_err());
        assert!(part_ok(&sell, Some(&a), 4 * GNR - 1, 400_001).is_err());
        assert!(part_ok(&sell, Some(&a), 0, 0).is_err());
        // The setting itself: a few fees at least, the offer at most.
        assert!(auto_ok(&sell, &auto(MIN_PART_GNR - 1)).is_err());
        assert!(auto_ok(&sell, &auto(4 * GNR + 1)).is_err());
        assert!(auto_ok(&sell, &auto(4 * GNR)).is_ok());
    }

    #[test]
    fn a_quick_swap_takes_the_best_prices_first_in_at_most_three_parts() {
        let book = vec![
            (offer("dear", Side::SellGnr, 10 * GNR, 2_000_000), auto(GNR)),
            (
                offer("cheap", Side::SellGnr, 2 * GNR, 200_000),
                auto(GNR / 10),
            ),
            (
                offer("mid", Side::SellGnr, 3 * GNR, 450_000),
                auto(GNR / 10),
            ),
            (offer("bid", Side::BuyGnr, 50 * GNR, 500_000), auto(GNR)),
            (offer("tiny", Side::SellGnr, GNR, 120_000), auto(GNR)),
        ];
        let p = plan(&book, true, Want::Gnr(4 * GNR), MAX_PARTS);
        assert_eq!(
            p,
            vec![
                Part {
                    id: "cheap".into(),
                    gnr: 2 * GNR,
                    sats: 200_000
                },
                Part {
                    id: "tiny".into(),
                    gnr: GNR,
                    sats: 120_000
                },
                Part {
                    id: "mid".into(),
                    gnr: GNR,
                    sats: 150_000
                },
            ]
        );
        // More than three parts are never planned: the plan comes short.
        let p = plan(&book, true, Want::Gnr(20 * GNR), MAX_PARTS);
        assert_eq!(p.len(), 3);
        assert_eq!(p.iter().map(|x| x.gnr).sum::<u64>(), 6 * GNR);
        // Spending bitcoin: as much GNR as it buys, never more bitcoin than given.
        let p = plan(&book, true, Want::Sats(250_000), MAX_PARTS);
        assert_eq!(
            p[0],
            Part {
                id: "cheap".into(),
                gnr: 2 * GNR,
                sats: 200_000
            }
        );
        let spent: u64 = p.iter().map(|x| x.sats).sum();
        assert!(spent <= 250_000 && spent > 240_000, "{p:?}");
        // A rest smaller than every offer's smallest part is left out.
        let p = plan(&book, true, Want::Gnr(2 * GNR + 1), MAX_PARTS);
        assert_eq!(p.len(), 1);
        // Selling goes to the buy offers only, dearest first.
        let p = plan(&book, false, Want::Gnr(2 * GNR), MAX_PARTS);
        assert_eq!(
            p,
            vec![Part {
                id: "bid".into(),
                gnr: 2 * GNR,
                sats: 20_000
            }]
        );
        // Every planned part is one its maker swaps.
        for (want, buying) in [
            (Want::Gnr(7 * GNR + 12_345), true),
            (Want::Sats(777_777), true),
            (Want::Sats(123_456), false),
        ] {
            for x in plan(&book, buying, want, MAX_PARTS) {
                let (o, a) = book.iter().find(|(o, _)| o.id == x.id).unwrap();
                assert!(part_ok(o, Some(a), x.gnr, x.sats).is_ok(), "{x:?}");
            }
        }
        assert!(plan(&[], true, Want::Gnr(GNR), MAX_PARTS).is_empty());
    }

    #[test]
    fn a_take_without_amounts_reads_as_before() {
        let old: Note = serde_json::from_str(r#"{"kind":"take","offer":"ab"}"#).unwrap();
        assert_eq!(
            old,
            Note::Take {
                offer: "ab".into(),
                gnr: None,
                sats: None
            }
        );
        let new = Note::Take {
            offer: "ab".into(),
            gnr: Some(5),
            sats: Some(10_000),
        };
        let s = serde_json::to_string(&new).unwrap();
        assert_eq!(serde_json::from_str::<Note>(&s).unwrap(), new);
        // A wallet before 1.0.46 reads the take as one for the whole offer.
        #[derive(Deserialize)]
        #[serde(tag = "kind", rename_all = "snake_case")]
        enum OldNote {
            Take { offer: String },
        }
        let OldNote::Take { offer } = serde_json::from_str::<OldNote>(&s).unwrap();
        assert_eq!(offer, "ab");
    }
}

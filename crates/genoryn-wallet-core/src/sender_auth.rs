//! Payment Proof 2.0: fresh sender authentication for a specific payment.
//!
//! Three levels of assurance are kept apart:
//!
//! - **Payment verification** (`scan::verify_payment_proof`): an output of a
//!   valid transaction pays this amount to this address.
//! - **Sender-origin disclosure** (the proof's outgoing cipher key): evidence
//!   made by the paying wallet once. It can be forwarded like any file.
//! - **Fresh sender authentication** (this module): a signature over a fresh
//!   challenge with a key the payment itself committed to.
//!
//! When a wallet pays someone, it writes a payment-specific Ed25519 public key
//! into the end of that output's 512-byte memo (`MemoTrailer`). The memo is
//! part of the note ciphertext that the spend authorization signs, so only the
//! creator of the transaction chose that key. The matching secret is derived
//! from the recovery seed (never from viewing keys), the payment's own spent
//! nullifier, the recipient, the amount and the output's position; a restored
//! wallet derives it again. A recipient, a watch-only holder or anyone holding
//! a forwarded proof sees the public key but cannot sign with it.
//!
//! A successful response establishes control of that payment's credential, not
//! a legal identity: secrets can be shared or stolen, and the verifier must
//! link the exchange to a person through a channel they already trust.
use crate::keys::Wallet;
use anyhow::{Context, Result, bail, ensure};
use data_encoding::BASE64URL_NOPAD;
use ed25519_dalek::{Signature, Signer, SigningKey, VerifyingKey};
use genoryn_core::{
    codec::{self, Hash},
    params::Network,
};
use orchard::Address;
use rand08::RngCore;
use serde::Serialize;
use zeroize::Zeroizing;

/// Bytes at the end of the memo that hold the commitment.
pub const TRAILER_LEN: usize = 40;
/// Where the commitment starts; the byte before it is always zero.
pub const TRAILER_START: usize = 512 - TRAILER_LEN;
/// Longest memo text that still leaves room for the commitment. Older wallets
/// read memo text up to the first zero byte, so they never show the trailer.
pub const MAX_MEMO_WITH_AUTH: usize = TRAILER_START - 1;
const TRAILER_MAGIC: &[u8; 4] = b"GNRA";
const TRAILER_VERSION: u8 = 1;
/// One transfer has at most 16 actions, so at most 16 outputs.
const MAX_ORDINAL: u16 = 16;

const ROOT_DOMAIN: &[u8] = b"GENORYN/sender-auth/root/v1";
const KEY_DOMAIN: &[u8] = b"GENORYN/sender-auth/payment/v1";
const MESSAGE_DOMAIN: &[u8] = b"GENORYN/payment-challenge/response/v1";
const CHALLENGE_ID_DOMAIN: &[u8] = b"GENORYN/payment-challenge/id/v1";
const CHECKSUM_DOMAIN: &[u8] = b"GENORYN/payment-challenge/checksum/v1";

pub const CHALLENGE_PREFIX: &str = "GNRCHAL1-";
pub const RESPONSE_PREFIX: &str = "GNRRESP1-";
const CHALLENGE_BYTES: usize = 1 + 1 + 32 + 2 + 43 + 8 + 32 + 8;
const RESPONSE_BYTES: usize = 1 + 32 + 64;
const CHECKSUM_BYTES: usize = 4;
/// A challenge can be answered for at most a day.
pub const MAX_CHALLENGE_SECONDS: u64 = 86_400;
pub const DEFAULT_CHALLENGE_SECONDS: u64 = 3_600;
/// Clock differences between the two computers that are tolerated.
const CLOCK_SKEW_SECONDS: u64 = 300;

/// The payment-specific public key committed at the end of an output's memo.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MemoTrailer {
    pub key: [u8; 32],
}
impl MemoTrailer {
    /// Writes the commitment into `memo`, which must hold at most
    /// `MAX_MEMO_WITH_AUTH` bytes of text followed by zeros.
    pub fn write(&self, memo: &mut [u8; 512]) -> Result<()> {
        ensure!(
            memo[MAX_MEMO_WITH_AUTH..].iter().all(|byte| *byte == 0),
            "memo text is too long to add sender verification ({MAX_MEMO_WITH_AUTH} bytes at most)"
        );
        memo[TRAILER_START..TRAILER_START + 4].copy_from_slice(TRAILER_MAGIC);
        memo[TRAILER_START + 4] = TRAILER_VERSION;
        memo[TRAILER_START + 8..].copy_from_slice(&self.key);
        Ok(())
    }
    /// The commitment in `memo`, if it carries a well-formed one.
    pub fn read(memo: &[u8; 512]) -> Option<Self> {
        let trailer = &memo[TRAILER_START..];
        if memo[MAX_MEMO_WITH_AUTH] != 0
            || &trailer[..4] != TRAILER_MAGIC
            || trailer[4] != TRAILER_VERSION
            || trailer[5..8] != [0, 0, 0]
        {
            return None;
        }
        let key: [u8; 32] = trailer[8..].try_into().ok()?;
        VerifyingKey::from_bytes(&key).ok()?;
        Some(Self { key })
    }
}

/// What a payment-specific key is bound to besides the wallet's seed.
pub struct PaymentBinding<'a> {
    /// The smallest nullifier among the paying wallet's own spends in the
    /// transaction: public once mined, unique because a note is spent once.
    pub anchor_nullifier: Hash,
    pub recipient: &'a Address,
    pub amount: u64,
    /// The output's position in the list of payments the wallet requested.
    pub ordinal: u16,
}

fn authentication_root(wallet: &Wallet) -> Result<Zeroizing<[u8; 32]>> {
    wallet.storage_key(ROOT_DOMAIN)
}

/// The secret key of one payment. Derived from the seed, so watch-only and
/// receiving keys cannot produce it.
pub fn payment_key(wallet: &Wallet, binding: &PaymentBinding<'_>) -> Result<SigningKey> {
    let root = authentication_root(wallet)?;
    let hk = hkdf::Hkdf::<sha2::Sha256>::new(Some(KEY_DOMAIN), &*root);
    let mut info = Vec::with_capacity(32 + 32 + 43 + 8 + 2);
    info.extend_from_slice(&wallet.network.genesis());
    info.extend_from_slice(&binding.anchor_nullifier);
    info.extend_from_slice(&binding.recipient.to_raw_address_bytes());
    info.extend_from_slice(&binding.amount.to_le_bytes());
    info.extend_from_slice(&binding.ordinal.to_le_bytes());
    let mut secret = Zeroizing::new([0u8; 32]);
    hk.expand(&info, &mut *secret)
        .map_err(|_| anyhow::anyhow!("sender key derivation failed"))?;
    Ok(SigningKey::from_bytes(&secret))
}

/// The commitment for a new payment output.
pub fn commitment(wallet: &Wallet, binding: &PaymentBinding<'_>) -> Result<MemoTrailer> {
    Ok(MemoTrailer {
        key: payment_key(wallet, binding)?.verifying_key().to_bytes(),
    })
}

/// The key a wallet committed to in one of its payments, found by trying each
/// output position. `None` when this wallet did not create the commitment.
pub fn find_payment_key(
    wallet: &Wallet,
    anchor_nullifier: Hash,
    recipient: &Address,
    amount: u64,
    committed: &MemoTrailer,
) -> Result<Option<SigningKey>> {
    for ordinal in 0..MAX_ORDINAL {
        let key = payment_key(
            wallet,
            &PaymentBinding {
                anchor_nullifier,
                recipient,
                amount,
                ordinal,
            },
        )?;
        if key.verifying_key().to_bytes() == committed.key {
            return Ok(Some(key));
        }
    }
    Ok(None)
}

/// A verifier's fresh question about one payment output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
pub struct Challenge {
    pub network: Network,
    #[serde(with = "hex_hash")]
    pub txid: Hash,
    pub action: u16,
    #[serde(skip)]
    pub recipient: [u8; 43],
    pub amount: u64,
    #[serde(with = "hex_hash")]
    pub nonce: Hash,
    /// Unix seconds after which the challenge is no longer answered or accepted.
    pub expires: u64,
}
mod hex_hash {
    pub fn serialize<S: serde::Serializer>(hash: &[u8; 32], s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&hex::encode(hash))
    }
}
fn network_from_id(id: u8) -> Result<Network> {
    [Network::Devnet, Network::Testnet, Network::Mainnet]
        .into_iter()
        .find(|network| network.id() == id)
        .context("the challenge names an unknown network")
}
fn checksum(bytes: &[u8]) -> [u8; CHECKSUM_BYTES] {
    let hash = codec::hash(CHECKSUM_DOMAIN, bytes);
    hash[..CHECKSUM_BYTES].try_into().expect("fixed length")
}
fn encode_code(prefix: &str, bytes: &[u8]) -> String {
    let mut body = bytes.to_vec();
    body.extend_from_slice(&checksum(bytes));
    format!("{prefix}{}", BASE64URL_NOPAD.encode(&body))
}
/// The bytes of a code; spaces and line breaks inside it (from a message or a
/// PDF viewer) are ignored, and a mistyped code fails its checksum.
fn decode_code(prefix: &str, text: &str, length: usize) -> Result<Vec<u8>> {
    let text = text.trim();
    ensure!(text.len() <= 1024, "this code is too long");
    let body = text
        .strip_prefix(prefix)
        .with_context(|| format!("this is not a {prefix}… code"))?;
    let body: String = body.chars().filter(|c| !c.is_whitespace()).collect();
    let bytes = BASE64URL_NOPAD
        .decode(body.as_bytes())
        .ok()
        .filter(|bytes| bytes.len() == length + CHECKSUM_BYTES)
        .context("this code is incomplete or damaged")?;
    let (payload, check) = bytes.split_at(length);
    ensure!(
        checksum(payload) == check,
        "this code is damaged (checksum mismatch)"
    );
    Ok(payload.to_vec())
}

impl Challenge {
    /// A new challenge about a payment the verifier has already checked.
    pub fn new(
        network: Network,
        txid: Hash,
        action: u16,
        recipient: &Address,
        amount: u64,
        now: u64,
        valid_seconds: u64,
    ) -> Result<Self> {
        ensure!(
            (60..=MAX_CHALLENGE_SECONDS).contains(&valid_seconds),
            "a challenge stays valid between one minute and one day"
        );
        let mut nonce = [0u8; 32];
        rand08::rngs::OsRng.fill_bytes(&mut nonce);
        Ok(Self {
            network,
            txid,
            action,
            recipient: recipient.to_raw_address_bytes(),
            amount,
            nonce,
            expires: now.checked_add(valid_seconds).context("time overflow")?,
        })
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(CHALLENGE_BYTES);
        bytes.push(1);
        bytes.push(self.network.id());
        bytes.extend_from_slice(&self.txid);
        bytes.extend_from_slice(&self.action.to_le_bytes());
        bytes.extend_from_slice(&self.recipient);
        bytes.extend_from_slice(&self.amount.to_le_bytes());
        bytes.extend_from_slice(&self.nonce);
        bytes.extend_from_slice(&self.expires.to_le_bytes());
        bytes
    }
    pub fn from_bytes(bytes: &[u8]) -> Result<Self> {
        ensure!(bytes.len() == CHALLENGE_BYTES, "challenge length");
        ensure!(bytes[0] == 1, "unsupported challenge version");
        let take = |from: usize, to: usize| &bytes[from..to];
        let challenge = Self {
            network: network_from_id(bytes[1])?,
            txid: take(2, 34).try_into()?,
            action: u16::from_le_bytes(take(34, 36).try_into()?),
            recipient: take(36, 79).try_into()?,
            amount: u64::from_le_bytes(take(79, 87).try_into()?),
            nonce: take(87, 119).try_into()?,
            expires: u64::from_le_bytes(take(119, 127).try_into()?),
        };
        ensure!(
            (challenge.action as usize) < genoryn_core::codec::MAX_ACTIONS,
            "the challenge names an output beyond the protocol limit"
        );
        challenge.recipient()?;
        Ok(challenge)
    }
    pub fn recipient(&self) -> Result<Address> {
        Option::<Address>::from(Address::from_raw_address_bytes(&self.recipient))
            .context("the challenge names an invalid receiving address")
    }
    pub fn code(&self) -> String {
        encode_code(CHALLENGE_PREFIX, &self.to_bytes())
    }
    pub fn parse(text: &str) -> Result<Self> {
        Self::from_bytes(&decode_code(CHALLENGE_PREFIX, text, CHALLENGE_BYTES)?)
    }
    /// Identifies this exact challenge, nonce and expiry included.
    pub fn id(&self) -> Hash {
        codec::hash(CHALLENGE_ID_DOMAIN, &self.to_bytes())
    }
    /// What the response signs: the network's genesis and chain id, the whole
    /// challenge and the committed key. A response to any other challenge,
    /// payment, recipient, amount, network or key does not verify.
    fn message(&self, committed: &MemoTrailer) -> Hash {
        let mut bytes = Vec::new();
        bytes.extend_from_slice(&self.network.genesis());
        bytes.extend_from_slice(self.network.chain_id().as_bytes());
        bytes.extend_from_slice(&self.to_bytes());
        bytes.extend_from_slice(&committed.key);
        codec::hash(MESSAGE_DOMAIN, &bytes)
    }
    pub fn ensure_open(&self, now: u64) -> Result<()> {
        ensure!(
            now <= self.expires.saturating_add(CLOCK_SKEW_SECONDS),
            "this challenge has expired; ask for a new one"
        );
        ensure!(
            self.expires <= now.saturating_add(MAX_CHALLENGE_SECONDS + CLOCK_SKEW_SECONDS),
            "this challenge claims to stay valid for more than a day"
        );
        Ok(())
    }
}

/// The paying wallet's signed answer to one challenge.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Response {
    pub challenge_id: Hash,
    pub signature: [u8; 64],
}
impl Response {
    pub fn code(&self) -> String {
        let mut bytes = Vec::with_capacity(RESPONSE_BYTES);
        bytes.push(1);
        bytes.extend_from_slice(&self.challenge_id);
        bytes.extend_from_slice(&self.signature);
        encode_code(RESPONSE_PREFIX, &bytes)
    }
    pub fn parse(text: &str) -> Result<Self> {
        let bytes = decode_code(RESPONSE_PREFIX, text, RESPONSE_BYTES)?;
        ensure!(bytes[0] == 1, "unsupported response version");
        Ok(Self {
            challenge_id: bytes[1..33].try_into()?,
            signature: bytes[33..].try_into()?,
        })
    }
}

/// Signs `challenge` with the key of a payment the wallet made. The caller has
/// already found the payment and checked that its recipient and amount match.
pub fn answer(
    challenge: &Challenge,
    committed: &MemoTrailer,
    key: &SigningKey,
    now: u64,
) -> Result<Response> {
    challenge.ensure_open(now)?;
    ensure!(
        key.verifying_key().to_bytes() == committed.key,
        "this key is not the one the payment committed to"
    );
    Ok(Response {
        challenge_id: challenge.id(),
        signature: key.sign(&challenge.message(committed)).to_bytes(),
    })
}

/// Checks a response against the challenge the verifier issued and the key
/// read from the payment's own memo on the verified chain.
pub fn verify(
    challenge: &Challenge,
    response: &Response,
    committed: &MemoTrailer,
    now: u64,
) -> Result<()> {
    if response.challenge_id != challenge.id() {
        bail!("this response answers a different challenge (or an older one)");
    }
    challenge.ensure_open(now)?;
    let key = VerifyingKey::from_bytes(&committed.key).context("invalid committed key")?;
    key.verify_strict(
        &challenge.message(committed),
        &Signature::from_bytes(&response.signature),
    )
    .map_err(|_| anyhow::anyhow!("the sender signature is not valid"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wallet() -> Wallet {
        Wallet::create(Network::Devnet).unwrap()
    }
    fn binding(recipient: &Address, ordinal: u16) -> PaymentBinding<'_> {
        PaymentBinding {
            anchor_nullifier: [7; 32],
            recipient,
            amount: 10_000_000_000_000,
            ordinal,
        }
    }

    #[test]
    fn the_trailer_fits_behind_the_longest_allowed_text_and_old_readers_ignore_it() {
        let alice = wallet();
        let bob = wallet().address(0).unwrap();
        let trailer = commitment(&alice, &binding(&bob, 0)).unwrap();
        let mut memo = [0u8; 512];
        let text = "x".repeat(MAX_MEMO_WITH_AUTH);
        memo[..text.len()].copy_from_slice(text.as_bytes());
        trailer.write(&mut memo).unwrap();
        assert_eq!(MemoTrailer::read(&memo), Some(trailer));
        // How every wallet so far reads memo text: up to the first zero byte.
        let shown = &memo[..memo.iter().position(|b| *b == 0).unwrap()];
        assert_eq!(shown, text.as_bytes());
        let mut long = [0u8; 512];
        long[..MAX_MEMO_WITH_AUTH + 1].fill(b'y');
        assert!(trailer.write(&mut long).is_err(), "never cuts user text");
        assert_eq!(MemoTrailer::read(&[0; 512]), None);
        let mut damaged = memo;
        damaged[TRAILER_START + 5] = 1;
        assert_eq!(MemoTrailer::read(&damaged), None);
        let mut text_runs_into_it = memo;
        text_runs_into_it[MAX_MEMO_WITH_AUTH] = b'z';
        assert_eq!(MemoTrailer::read(&text_runs_into_it), None);
    }

    #[test]
    fn keys_are_per_payment_recoverable_from_the_seed_and_not_from_viewing_material() {
        let alice = wallet();
        let restored = Wallet::restore(Network::Devnet, &alice.phrase().unwrap()).unwrap();
        let bob = wallet().address(0).unwrap();
        let carol = wallet().address(0).unwrap();
        let key = payment_key(&alice, &binding(&bob, 0)).unwrap();
        assert_eq!(
            key.to_bytes(),
            payment_key(&restored, &binding(&bob, 0))
                .unwrap()
                .to_bytes()
        );
        let distinct = [
            payment_key(&alice, &binding(&bob, 1)).unwrap(),
            payment_key(&alice, &binding(&carol, 0)).unwrap(),
            payment_key(
                &alice,
                &PaymentBinding {
                    amount: 1,
                    ..binding(&bob, 0)
                },
            )
            .unwrap(),
            payment_key(
                &alice,
                &PaymentBinding {
                    anchor_nullifier: [8; 32],
                    ..binding(&bob, 0)
                },
            )
            .unwrap(),
            payment_key(&alice.for_account(1).unwrap(), &binding(&bob, 0)).unwrap(),
            payment_key(
                &Wallet::restore(Network::Testnet, &alice.phrase().unwrap()).unwrap(),
                &binding(&bob, 0),
            )
            .unwrap(),
        ];
        for other in &distinct {
            assert_ne!(other.to_bytes(), key.to_bytes());
        }
        let committed = MemoTrailer {
            key: key.verifying_key().to_bytes(),
        };
        let found = find_payment_key(&restored, [7; 32], &bob, 10_000_000_000_000, &committed)
            .unwrap()
            .unwrap();
        assert_eq!(found.to_bytes(), key.to_bytes());
        // The recipient knows the committed key and its own keys, nothing more.
        let bob_wallet = wallet();
        assert!(
            find_payment_key(&bob_wallet, [7; 32], &bob, 10_000_000_000_000, &committed)
                .unwrap()
                .is_none()
        );
    }

    fn challenge_for(alice: &Wallet, bob: &Address) -> (Challenge, MemoTrailer, SigningKey) {
        let key = payment_key(alice, &binding(bob, 0)).unwrap();
        let committed = MemoTrailer {
            key: key.verifying_key().to_bytes(),
        };
        let challenge = Challenge::new(
            Network::Devnet,
            [3; 32],
            1,
            bob,
            10_000_000_000_000,
            1_000_000,
            3_600,
        )
        .unwrap();
        (challenge, committed, key)
    }

    #[test]
    fn codes_round_trip_and_damage_is_detected() {
        let alice = wallet();
        let bob = wallet().address(0).unwrap();
        let (challenge, committed, key) = challenge_for(&alice, &bob);
        let code = challenge.code();
        assert!(
            code.starts_with(CHALLENGE_PREFIX) && code.len() < 200,
            "{code}"
        );
        assert_eq!(Challenge::parse(&code).unwrap(), challenge);
        let wrapped = format!("  {}\n{}  ", &code[..40], &code[40..]);
        assert_eq!(Challenge::parse(&wrapped).unwrap(), challenge);
        let response = answer(&challenge, &committed, &key, 1_000_100).unwrap();
        let reply = response.code();
        assert!(
            reply.starts_with(RESPONSE_PREFIX) && reply.len() < 150,
            "{reply}"
        );
        assert_eq!(Response::parse(&reply).unwrap(), response);
        for position in [12, 60, code.len() - 3] {
            let mut damaged = code.clone().into_bytes();
            damaged[position] = if damaged[position] == b'A' {
                b'B'
            } else {
                b'A'
            };
            assert!(Challenge::parse(&String::from_utf8(damaged).unwrap()).is_err());
        }
        assert!(
            Challenge::parse(&reply).is_err(),
            "a response is not a challenge"
        );
        assert!(Response::parse(&code).is_err());
        assert!(Challenge::parse(&format!("{CHALLENGE_PREFIX}{}", "A".repeat(2000))).is_err());
    }

    #[test]
    fn only_the_committed_key_answers_only_this_fresh_challenge() {
        let alice = wallet();
        let bob_wallet = wallet();
        let bob = bob_wallet.address(0).unwrap();
        let (challenge, committed, key) = challenge_for(&alice, &bob);
        let now = 1_000_100;
        let response = answer(&challenge, &committed, &key, now).unwrap();
        verify(&challenge, &response, &committed, now).unwrap();
        // A copied old response does not satisfy a new challenge.
        let fresh = Challenge::new(
            Network::Devnet,
            challenge.txid,
            challenge.action,
            &bob,
            challenge.amount,
            now,
            3_600,
        )
        .unwrap();
        assert!(verify(&fresh, &response, &committed, now).is_err());
        // A signature by any other key (Bob's own, another wallet's) fails.
        for impostor in [
            payment_key(&bob_wallet, &binding(&bob, 0)).unwrap(),
            payment_key(&wallet(), &binding(&bob, 0)).unwrap(),
        ] {
            assert!(answer(&challenge, &committed, &impostor, now).is_err());
            let forged = Response {
                challenge_id: challenge.id(),
                signature: impostor.sign(&challenge.message(&committed)).to_bytes(),
            };
            assert!(verify(&challenge, &forged, &committed, now).is_err());
        }
        // A different payment, recipient, amount, output or network.
        let carol = wallet().address(0).unwrap();
        let changed = [
            Challenge {
                txid: [4; 32],
                ..challenge.clone()
            },
            Challenge {
                recipient: carol.to_raw_address_bytes(),
                ..challenge.clone()
            },
            Challenge {
                amount: challenge.amount + 1,
                ..challenge.clone()
            },
            Challenge {
                action: 0,
                ..challenge.clone()
            },
            Challenge {
                network: Network::Testnet,
                ..challenge.clone()
            },
        ];
        for other in &changed {
            assert!(verify(other, &response, &committed, now).is_err());
            let resigned = Response {
                challenge_id: other.id(),
                signature: response.signature,
            };
            assert!(verify(other, &resigned, &committed, now).is_err());
        }
        // A modified signature or a key other than the committed one.
        let mut bad = response.clone();
        bad.signature[10] ^= 1;
        assert!(verify(&challenge, &bad, &committed, now).is_err());
        let other_key = MemoTrailer {
            key: payment_key(&alice, &binding(&bob, 1))
                .unwrap()
                .verifying_key()
                .to_bytes(),
        };
        assert!(verify(&challenge, &response, &other_key, now).is_err());
        // Expired challenges are neither answered nor accepted.
        let late = challenge.expires + CLOCK_SKEW_SECONDS + 1;
        assert!(answer(&challenge, &committed, &key, late).is_err());
        assert!(verify(&challenge, &response, &committed, late).is_err());
        assert!(Challenge::new(Network::Devnet, [0; 32], 0, &bob, 1, now, 10).is_err());
        assert!(Challenge::new(Network::Devnet, [0; 32], 0, &bob, 1, now, 2 * 86_400).is_err());
    }

    #[test]
    fn hostile_bytes_never_panic() {
        for length in [0, 1, CHALLENGE_BYTES - 1, CHALLENGE_BYTES + 1] {
            assert!(Challenge::from_bytes(&vec![1; length]).is_err());
        }
        let mut bytes = vec![0u8; CHALLENGE_BYTES];
        bytes[0] = 1;
        bytes[1] = 9;
        assert!(Challenge::from_bytes(&bytes).is_err(), "unknown network");
        bytes[1] = Network::Devnet.id();
        assert!(Challenge::from_bytes(&bytes).is_err(), "invalid recipient");
        for text in ["", "GNRCHAL1-", "GNRCHAL1-!!!", "GNRRESP1-AAAA", "hello"] {
            assert!(Challenge::parse(text).is_err());
            assert!(Response::parse(text).is_err());
        }
    }
}

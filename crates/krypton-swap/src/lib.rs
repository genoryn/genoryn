//! BTC ⇄ GNR atomic swaps, Bitcoin side (see `docs/spec/atomic-swaps.md`).
//!
//! The BTC seller locks bitcoin in a hash time-locked contract (HTLC): the GNR seller can
//! take it with the 32-byte secret behind `hash`, the BTC seller can take it back after
//! `refund_height`. The GNR seller learns the secret when the BTC seller claims the GNR,
//! so either both sides get paid or both get their coins back.
//!
//! Terms are exchanged, never scripts: each side rebuilds the script from the terms and
//! checks the funding output itself.

use bitcoin::absolute::LockTime;
use bitcoin::blockdata::script::Builder;
use bitcoin::hashes::{Hash, sha256};
use bitcoin::opcodes::all::{
    OP_CHECKSIG, OP_CLTV, OP_DROP, OP_ELSE, OP_ENDIF, OP_EQUALVERIFY, OP_IF, OP_SHA256, OP_SIZE,
};
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, FeeRate, Network, OutPoint, PublicKey, Script, ScriptBuf, Sequence,
    Transaction, TxIn, TxOut, Witness,
};
use rand_core::{OsRng, RngCore};
use zeroize::{Zeroize, ZeroizeOnDrop};

pub use bitcoin;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum SwapError {
    #[error("the refund height must be a block height above zero")]
    BadLockTime,
    #[error("this key does not match the contract")]
    WrongKey,
    #[error("this secret does not match the contract's hash")]
    WrongSecret,
    #[error("the fee leaves less than the dust limit of {0} sat")]
    Dust(u64),
    #[error("fee rate overflow")]
    Fee,
    #[error("signing failed: {0}")]
    Sign(String),
    #[error("unsafe timelocks: {0}")]
    Timelocks(&'static str),
}

pub type Result<T> = std::result::Result<T, SwapError>;

/// The swap secret. Its SHA-256 is the hash lock on both chains.
#[derive(Clone, Zeroize, ZeroizeOnDrop, PartialEq, Eq)]
pub struct Secret([u8; 32]);

impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Secret(..)")
    }
}

impl Secret {
    pub fn generate() -> Self {
        let mut s = [0u8; 32];
        OsRng.fill_bytes(&mut s);
        Self(s)
    }
    pub fn from_bytes(b: [u8; 32]) -> Self {
        Self(b)
    }
    pub fn as_bytes(&self) -> &[u8; 32] {
        &self.0
    }
    pub fn hash(&self) -> [u8; 32] {
        sha256::Hash::hash(&self.0).to_byte_array()
    }
}

fn tagged(tag: &str, seed: &[u8], swap: u32, counter: u32) -> [u8; 32] {
    let t = sha256::Hash::hash(tag.as_bytes()).to_byte_array();
    let mut m = Vec::with_capacity(64 + seed.len() + 8);
    m.extend_from_slice(&t);
    m.extend_from_slice(&t);
    m.extend_from_slice(seed);
    m.extend_from_slice(&swap.to_le_bytes());
    m.extend_from_slice(&counter.to_le_bytes());
    let h = sha256::Hash::hash(&m).to_byte_array();
    m.zeroize();
    h
}

/// The Bitcoin key of swap number `swap`, derived from the wallet seed so a swap in
/// progress can be finished (claimed or refunded) after restoring from the 24 words.
pub fn derive_key(seed: &[u8], swap: u32) -> SecretKey {
    (0u32..)
        .find_map(|c| SecretKey::from_slice(&tagged("Genoryn/swap/btc-key", seed, swap, c)).ok())
        .expect("a valid key is found within a few tries")
}

/// The secret of swap number `swap` when this wallet is the BTC seller.
pub fn derive_secret(seed: &[u8], swap: u32) -> Secret {
    Secret(tagged("Genoryn/swap/secret", seed, swap, 0))
}

pub fn public_key(sk: &SecretKey) -> PublicKey {
    PublicKey::new(sk.public_key(&Secp256k1::signing_only()))
}

/// Terms of the Bitcoin HTLC.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Htlc {
    /// SHA-256 of the secret.
    pub hash: [u8; 32],
    /// The GNR seller's key: spends with the secret.
    pub claim: PublicKey,
    /// The BTC seller's key: spends from `refund_height` on.
    pub refund: PublicKey,
    pub refund_height: u32,
}

/// Where the BTC seller's deposit landed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Funding {
    pub outpoint: OutPoint,
    pub value: Amount,
}

enum Path<'a> {
    Claim(&'a Secret),
    Refund,
}

impl Htlc {
    pub fn new(
        hash: [u8; 32],
        claim: PublicKey,
        refund: PublicKey,
        refund_height: u32,
    ) -> Result<Self> {
        if refund_height == 0 || LockTime::from_height(refund_height).is_err() {
            return Err(SwapError::BadLockTime);
        }
        if !claim.compressed || !refund.compressed {
            return Err(SwapError::WrongKey);
        }
        Ok(Self {
            hash,
            claim,
            refund,
            refund_height,
        })
    }

    fn lock_time(&self) -> LockTime {
        LockTime::from_height(self.refund_height).expect("checked in new")
    }

    /// `IF SIZE 32 EQUALVERIFY SHA256 <hash> EQUALVERIFY <claim>
    ///  ELSE <refund_height> CLTV DROP <refund> ENDIF CHECKSIG`
    pub fn script(&self) -> ScriptBuf {
        Builder::new()
            .push_opcode(OP_IF)
            .push_opcode(OP_SIZE)
            .push_int(32)
            .push_opcode(OP_EQUALVERIFY)
            .push_opcode(OP_SHA256)
            .push_slice(self.hash)
            .push_opcode(OP_EQUALVERIFY)
            .push_key(&self.claim)
            .push_opcode(OP_ELSE)
            .push_lock_time(self.lock_time())
            .push_opcode(OP_CLTV)
            .push_opcode(OP_DROP)
            .push_key(&self.refund)
            .push_opcode(OP_ENDIF)
            .push_opcode(OP_CHECKSIG)
            .into_script()
    }

    pub fn script_pubkey(&self) -> ScriptBuf {
        ScriptBuf::new_p2wsh(&self.script().wscript_hash())
    }

    /// The P2WSH address the BTC seller pays into.
    pub fn address(&self, network: Network) -> Address {
        Address::p2wsh(&self.script(), network)
    }

    /// Outputs of `tx` that fund this contract.
    pub fn find_funding(&self, tx: &Transaction) -> Vec<Funding> {
        let spk = self.script_pubkey();
        let txid = tx.compute_txid();
        tx.output
            .iter()
            .enumerate()
            .filter(|(_, o)| o.script_pubkey == spk)
            .map(|(i, o)| Funding {
                outpoint: OutPoint::new(txid, i as u32),
                value: o.value,
            })
            .collect()
    }

    /// The GNR seller takes the bitcoin with the secret (learned from the GNR claim).
    pub fn claim(
        &self,
        funding: Funding,
        dest: &Script,
        rate: FeeRate,
        key: &SecretKey,
        secret: &Secret,
    ) -> Result<Transaction> {
        if public_key(key) != self.claim {
            return Err(SwapError::WrongKey);
        }
        if secret.hash() != self.hash {
            return Err(SwapError::WrongSecret);
        }
        self.spend(funding, dest, rate, key, Path::Claim(secret))
    }

    /// The BTC seller takes the bitcoin back. Final (minable) once the chain is at
    /// `refund_height`, i.e. in blocks above it.
    pub fn refund(
        &self,
        funding: Funding,
        dest: &Script,
        rate: FeeRate,
        key: &SecretKey,
    ) -> Result<Transaction> {
        if public_key(key) != self.refund {
            return Err(SwapError::WrongKey);
        }
        self.spend(funding, dest, rate, key, Path::Refund)
    }

    fn spend(
        &self,
        funding: Funding,
        dest: &Script,
        rate: FeeRate,
        key: &SecretKey,
        path: Path,
    ) -> Result<Transaction> {
        // Sign once to learn the size, then again with the fee for that size (+1 vB for
        // a signature that comes out a byte longer).
        // CLTV needs a non-final sequence and nLockTime at or above the script's.
        let lock_time = match path {
            Path::Claim(_) => LockTime::ZERO,
            Path::Refund => self.lock_time(),
        };
        let seq = Sequence::ENABLE_RBF_NO_LOCKTIME;
        let draft = self.signed(funding, dest, Amount::ZERO, key, &path, lock_time, seq)?;
        let fee = rate
            .fee_vb(draft.vsize() as u64 + 1)
            .ok_or(SwapError::Fee)?;
        self.signed(funding, dest, fee, key, &path, lock_time, seq)
    }

    #[allow(clippy::too_many_arguments)]
    fn signed(
        &self,
        funding: Funding,
        dest: &Script,
        fee: Amount,
        key: &SecretKey,
        path: &Path,
        lock_time: LockTime,
        sequence: Sequence,
    ) -> Result<Transaction> {
        let dust = dest.minimal_non_dust();
        let value = funding
            .value
            .checked_sub(fee)
            .filter(|v| *v >= dust)
            .ok_or(SwapError::Dust(dust.to_sat()))?;
        let mut tx = Transaction {
            version: Version::TWO,
            lock_time,
            input: vec![TxIn {
                previous_output: funding.outpoint,
                script_sig: ScriptBuf::new(),
                sequence,
                witness: Witness::new(),
            }],
            output: vec![TxOut {
                value,
                script_pubkey: dest.to_owned(),
            }],
        };
        let script = self.script();
        let sighash = SighashCache::new(&tx)
            .p2wsh_signature_hash(0, &script, funding.value, EcdsaSighashType::All)
            .map_err(|e| SwapError::Sign(e.to_string()))?;
        let sig = Secp256k1::signing_only()
            .sign_ecdsa(&Message::from_digest(sighash.to_byte_array()), key);
        let sig = bitcoin::ecdsa::Signature {
            signature: sig,
            sighash_type: EcdsaSighashType::All,
        }
        .to_vec();
        tx.input[0].witness = match path {
            Path::Claim(s) => {
                Witness::from_slice(&[sig, s.as_bytes().to_vec(), vec![1], script.to_bytes()])
            }
            Path::Refund => Witness::from_slice(&[sig, Vec::new(), script.to_bytes()]),
        };
        Ok(tx)
    }

    /// The secret, if `tx` claims this contract (the BTC seller watches for it when it
    /// claims on Bitcoin first in the reverse flow, and for the record otherwise).
    pub fn find_secret(&self, tx: &Transaction) -> Option<Secret> {
        let script = self.script().to_bytes();
        tx.input.iter().find_map(|i| {
            let w: Vec<&[u8]> = i.witness.iter().collect();
            if w.len() != 4 || w[2] != [1] || w[3] != script.as_slice() {
                return None;
            }
            let s = Secret(w[1].try_into().ok()?);
            (s.hash() == self.hash).then_some(s)
        })
    }
}

/// Refund heights of one swap.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Timelocks {
    pub btc_refund_height: u32,
    pub gnr_refund_height: u64,
}

/// Timelock and confirmation policy of a network pair.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Timing {
    /// How long the bitcoin stays locked.
    pub btc_lock_blocks: u32,
    /// How long the GNR stays locked.
    pub gnr_lock_blocks: u64,
    /// Seconds per block for the timelock check: Bitcoin blocks counted fast and GNR
    /// blocks counted slow, so the GNR seller's window to claim the bitcoin is never
    /// overestimated.
    pub btc_fast_secs: u64,
    pub gnr_slow_secs: u64,
    /// After the GNR lock expires, the GNR seller has at least this long to claim the bitcoin.
    pub min_window_secs: u64,
    /// The BTC seller claims the GNR at least this many blocks before the GNR lock expires.
    pub claim_margin: u64,
    /// Confirmations the GNR seller waits for on the bitcoin deposit before locking GNR.
    pub btc_confirmations: u32,
    /// Confirmations the BTC seller waits for on the GNR lock before revealing the secret.
    pub gnr_confirmations: u64,
}

impl Timing {
    /// Bitcoin mainnet: 288 blocks (about 48 h) against 720 GNR blocks (about 12 h).
    pub const MAINNET: Self = Self {
        btc_lock_blocks: 288,
        gnr_lock_blocks: 720,
        btc_fast_secs: 480,
        gnr_slow_secs: 90,
        min_window_secs: 12 * 3600,
        claim_margin: 120,
        btc_confirmations: 2,
        gnr_confirmations: 30,
    };
    /// Short locks for test networks (signet, regtest, GNR devnet).
    pub const TEST: Self = Self {
        btc_lock_blocks: 40,
        gnr_lock_blocks: 30,
        btc_fast_secs: 60,
        gnr_slow_secs: 10,
        min_window_secs: 600,
        claim_margin: 5,
        btc_confirmations: 1,
        gnr_confirmations: 2,
    };

    pub fn for_network(n: Network) -> Self {
        if n == Network::Bitcoin {
            Self::MAINNET
        } else {
            Self::TEST
        }
    }

    pub fn plan(&self, btc_tip: u32, gnr_tip: u64) -> Timelocks {
        Timelocks {
            btc_refund_height: btc_tip + self.btc_lock_blocks,
            gnr_refund_height: gnr_tip + self.gnr_lock_blocks,
        }
    }

    /// The GNR seller's check before locking GNR: the BTC seller still has time to claim
    /// after the confirmations, and once the GNR lock expires there is still at least
    /// [`Timing::min_window_secs`] left on the bitcoin lock.
    pub fn check(&self, t: &Timelocks, btc_tip: u32, gnr_tip: u64) -> Result<()> {
        if t.gnr_refund_height <= gnr_tip + self.gnr_confirmations + self.claim_margin {
            return Err(SwapError::Timelocks("the GNR lock expires too soon"));
        }
        if t.btc_refund_height <= btc_tip {
            return Err(SwapError::Timelocks("the bitcoin lock has already expired"));
        }
        let btc = u64::from(t.btc_refund_height - btc_tip) * self.btc_fast_secs;
        let gnr = (t.gnr_refund_height - gnr_tip) * self.gnr_slow_secs;
        if btc < gnr + self.min_window_secs {
            return Err(SwapError::Timelocks(
                "the bitcoin lock must outlast the GNR lock by a safe margin",
            ));
        }
        Ok(())
    }

    /// Whether the BTC seller may still claim the GNR (reveal the secret) at `gnr_tip`.
    pub fn may_claim_gnr(&self, t: &Timelocks, gnr_tip: u64) -> bool {
        gnr_tip + self.claim_margin < t.gnr_refund_height
    }
}

pub mod board;
pub mod btc;
pub mod engine;

#[cfg(test)]
mod tests;

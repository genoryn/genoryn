//! Blocks, headers and transactions with canonical encodings.

use crate::codec::{Reader, Writer};
use crate::params::{MAX_BLOCK_BYTES, MAX_PROOF_BYTES};
use crate::{CoreError, Hash32, Result};
use krypton_crypto::field::{Digest, digest_from_bytes, digest_from_uniform};
use krypton_crypto::hash::{tagged, tagged_parts};
use krypton_zk::SpendPublic;

pub const TX_VERSION: u16 = 1;
pub const HEADER_VERSION: u32 = 1;
pub const MEMO_LEN: usize = 128;
pub const NOTE_PLAINTEXT_LEN: usize = 8 + 32 + MEMO_LEN;
pub const NOTE_CIPHERTEXT_LEN: usize =
    krypton_crypto::kem::CIPHERTEXT_LEN + NOTE_PLAINTEXT_LEN + 16;
pub const HEADER_LEN: usize = 4 + 8 + 32 * 9 + 8 + 16 + 8;

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct BlockHeader {
    pub version: u32,
    pub height: u64,
    #[serde(with = "hex::serde")]
    pub prev: Hash32,
    /// MMR root over all previous blocks (FlyClient commitment).
    #[serde(with = "hex::serde")]
    pub mmr_root: Hash32,
    #[serde(with = "hex::serde")]
    pub tx_root: Hash32,
    /// Commitment to the shares included in this block.
    #[serde(with = "hex::serde")]
    pub share_root: Hash32,
    /// Note commitment tree root after this block.
    #[serde(with = "hex::serde")]
    pub note_root: Hash32,
    #[serde(with = "hex::serde")]
    pub nullifier_digest: Hash32,
    #[serde(with = "hex::serde")]
    pub output_digest: Hash32,
    /// Commitment to the reward payouts this block makes.
    #[serde(with = "hex::serde")]
    pub payout_digest: Hash32,
    /// Payout owner of whoever mined this header (as a block or as a share).
    #[serde(with = "hex::serde")]
    pub miner: Hash32,
    pub timestamp: u64,
    #[serde(with = "u128_str")]
    pub difficulty: u128,
    pub nonce: u64,
}

impl BlockHeader {
    fn encode_without_nonce(&self, w: &mut Writer) {
        w.u32(self.version)
            .u64(self.height)
            .raw(&self.prev)
            .raw(&self.mmr_root)
            .raw(&self.tx_root)
            .raw(&self.share_root)
            .raw(&self.note_root)
            .raw(&self.nullifier_digest)
            .raw(&self.output_digest)
            .raw(&self.payout_digest)
            .raw(&self.miner)
            .u64(self.timestamp)
            .u128(self.difficulty);
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        self.encode_without_nonce(&mut w);
        w.u64(self.nonce);
        w.0
    }
    pub fn from_reader(r: &mut Reader) -> Result<Self> {
        Ok(Self {
            version: r.u32()?,
            height: r.u64()?,
            prev: r.array()?,
            mmr_root: r.array()?,
            tx_root: r.array()?,
            share_root: r.array()?,
            note_root: r.array()?,
            nullifier_digest: r.array()?,
            output_digest: r.array()?,
            payout_digest: r.array()?,
            miner: r.array()?,
            timestamp: r.u64()?,
            difficulty: r.u128()?,
            nonce: r.u64()?,
        })
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        let h = Self::from_reader(&mut r)?;
        r.finish()?;
        Ok(h)
    }
    /// Input to KryptoHash (everything except the nonce).
    pub fn pow_hash(&self) -> Hash32 {
        let mut w = Writer::default();
        self.encode_without_nonce(&mut w);
        tagged("Krypton/header/pow", &w.0)
    }
    pub fn hash(&self) -> Hash32 {
        tagged("Krypton/block", &self.to_bytes())
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Output {
    #[serde(with = "hex::serde")]
    pub cm: Hash32,
    #[serde(with = "hex::serde")]
    pub ciphertext: Vec<u8>,
}

/// Mining rewards are paid by consensus (see [`crate::reward`]), so every transaction
/// is a shielded transfer.
#[derive(Clone, Copy, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
#[repr(u8)]
pub enum TxKind {
    Transfer = 0,
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Transaction {
    pub version: u16,
    pub kind: TxKind,
    #[serde(with = "hex::serde")]
    pub anchor: Hash32,
    pub nullifiers: [HexHash; 2],
    pub outputs: [Output; 2],
    pub fee: u64,
    pub mint: u64,
    /// Last height at which the transaction may be mined (coinbase: its height).
    pub expiry_height: u64,
    #[serde(with = "hex::serde")]
    pub proof: Vec<u8>,
}

#[derive(
    Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord, serde::Serialize, serde::Deserialize,
)]
pub struct HexHash(#[serde(with = "hex::serde")] pub Hash32);

impl Transaction {
    fn encode_body(&self, w: &mut Writer) {
        w.u16(self.version).u8(self.kind as u8).raw(&self.anchor);
        for n in &self.nullifiers {
            w.raw(&n.0);
        }
        for o in &self.outputs {
            w.raw(&o.cm).bytes(&o.ciphertext);
        }
        w.u64(self.fee).u64(self.mint).u64(self.expiry_height);
    }
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer::default();
        self.encode_body(&mut w);
        w.bytes(&self.proof);
        w.0
    }
    pub fn from_reader(r: &mut Reader) -> Result<Self> {
        let version = r.u16()?;
        let kind = match r.u8()? {
            0 => TxKind::Transfer,
            _ => return Err(CoreError::Decode("unknown transaction kind")),
        };
        let anchor = r.array()?;
        let nullifiers = [HexHash(r.array()?), HexHash(r.array()?)];
        let mut out = || -> Result<Output> {
            let cm = r.array()?;
            let ciphertext = r.bytes(NOTE_CIPHERTEXT_LEN)?;
            Ok(Output { cm, ciphertext })
        };
        let outputs = [out()?, out()?];
        Ok(Self {
            version,
            kind,
            anchor,
            nullifiers,
            outputs,
            fee: r.u64()?,
            mint: r.u64()?,
            expiry_height: r.u64()?,
            proof: r.bytes(MAX_PROOF_BYTES)?,
        })
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        let mut r = Reader::new(b);
        let t = Self::from_reader(&mut r)?;
        r.finish()?;
        Ok(t)
    }
    pub fn txid(&self) -> Hash32 {
        tagged("Krypton/tx", &self.to_bytes())
    }
    /// Hash of everything except the proof; the proof commits to it.
    pub fn sighash_bytes(&self) -> Hash32 {
        let mut w = Writer::default();
        self.encode_body(&mut w);
        tagged("Krypton/sighash", &w.0)
    }
    pub fn sighash(&self) -> Digest {
        digest_from_uniform(&self.sighash_bytes())
    }
    /// The public statement the proof must satisfy (strict field decoding).
    pub fn statement(&self) -> Result<SpendPublic> {
        let d = |b: &Hash32| {
            digest_from_bytes(b).map_err(|_| CoreError::Invalid("non-canonical field encoding"))
        };
        Ok(SpendPublic {
            root: d(&self.anchor)?,
            nullifiers: [d(&self.nullifiers[0].0)?, d(&self.nullifiers[1].0)?],
            output_cms: [d(&self.outputs[0].cm)?, d(&self.outputs[1].cm)?],
            fee: self.fee,
            mint: self.mint,
            sighash: self.sighash(),
        })
    }
    pub fn size(&self) -> usize {
        self.to_bytes().len()
    }
}

#[derive(Clone, Debug, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
pub struct Block {
    pub header: BlockHeader,
    /// Shares (headers meeting the share target) credited by this block, ascending by hash.
    pub shares: Vec<BlockHeader>,
    pub txs: Vec<Transaction>,
}

impl Block {
    pub fn to_bytes(&self) -> Vec<u8> {
        let mut w = Writer(self.header.to_bytes());
        w.u32(self.shares.len() as u32);
        for s in &self.shares {
            w.raw(&s.to_bytes());
        }
        w.u32(self.txs.len() as u32);
        for t in &self.txs {
            w.bytes(&t.to_bytes());
        }
        w.0
    }
    pub fn from_bytes(b: &[u8]) -> Result<Self> {
        if b.len() > MAX_BLOCK_BYTES + 1024 {
            return Err(CoreError::Decode("block too large"));
        }
        let mut r = Reader::new(b);
        let header = BlockHeader::from_reader(&mut r)?;
        let n = r.u32()? as usize;
        if n > crate::params::MAX_SHARES_PER_BLOCK {
            return Err(CoreError::Decode("too many shares"));
        }
        let shares = (0..n)
            .map(|_| BlockHeader::from_reader(&mut r))
            .collect::<Result<Vec<_>>>()?;
        let n = r.u32()? as usize;
        if n > MAX_BLOCK_BYTES / 1024 {
            return Err(CoreError::Decode("too many transactions"));
        }
        let mut txs = Vec::with_capacity(n);
        for _ in 0..n {
            let raw = r.bytes(crate::params::MAX_TX_BYTES)?;
            txs.push(Transaction::from_bytes(&raw)?);
        }
        r.finish()?;
        Ok(Self {
            header,
            shares,
            txs,
        })
    }
    pub fn hash(&self) -> Hash32 {
        self.header.hash()
    }
}

/// Binary Merkle root of transaction ids (odd nodes are promoted, never duplicated).
pub fn tx_root(txs: &[Transaction]) -> Hash32 {
    let mut level: Vec<Hash32> = txs.iter().map(|t| t.txid()).collect();
    if level.is_empty() {
        return tagged("Krypton/txroot/empty", &[]);
    }
    while level.len() > 1 {
        level = level
            .chunks(2)
            .map(|c| {
                if c.len() == 2 {
                    tagged_parts("Krypton/txroot/node", &[&c[0], &c[1]])
                } else {
                    c[0]
                }
            })
            .collect();
    }
    tagged_parts(
        "Krypton/txroot",
        &[&level[0], &(txs.len() as u64).to_le_bytes()],
    )
}

pub fn nullifier_digest(txs: &[Transaction]) -> Hash32 {
    let v: Vec<Hash32> = txs
        .iter()
        .flat_map(|t| t.nullifiers.iter().map(|n| n.0))
        .collect();
    nullifier_digest_raw(&v)
}

pub fn nullifier_digest_raw(nfs: &[Hash32]) -> Hash32 {
    let mut w = Writer::default();
    for n in nfs {
        w.raw(n);
    }
    tagged("Krypton/nullifiers", &w.0)
}

pub fn output_digest(txs: &[Transaction]) -> Hash32 {
    let v: Vec<Output> = txs.iter().flat_map(|t| t.outputs.iter().cloned()).collect();
    output_digest_raw(&v)
}

pub fn output_digest_raw(outputs: &[Output]) -> Hash32 {
    let mut w = Writer::default();
    for o in outputs {
        w.raw(&o.cm)
            .raw(&tagged("Krypton/ciphertext", &o.ciphertext));
    }
    tagged("Krypton/outputs", &w.0)
}

mod u128_str {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &u128, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(&v.to_string())
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<u128, D::Error> {
        String::deserialize(d)?
            .parse()
            .map_err(serde::de::Error::custom)
    }
}

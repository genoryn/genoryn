use crate::{
    codec::{self, Hash, MAX_BLOCK_BYTES},
    params::{MAX_TRANSACTIONS, Network},
    privacy::ShieldedTx,
};
use anyhow::{Result, ensure};
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Header {
    pub version: u16,
    pub network: Network,
    pub height: u64,
    pub previous: Hash,
    pub timestamp: u64,
    pub difficulty: u64,
    pub pow_seed: Hash,
    pub nonce: u64,
    pub body_hash: Hash,
    pub notes_root: Hash,
    pub nullifiers_root: Hash,
}
impl Header {
    pub fn bytes(&self) -> Result<Vec<u8>> {
        codec::encode(self)
    }
    pub fn hash(&self) -> Result<Hash> {
        Ok(codec::hash(b"GENORYN/block/v1", &self.bytes()?))
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Body {
    pub miner: ShieldedTx,
    pub treasury: Option<ShieldedTx>,
    pub transactions: Vec<ShieldedTx>,
}
impl Body {
    pub fn hash(&self) -> Result<Hash> {
        Ok(codec::hash(b"GENORYN/body/v1", &codec::encode(self)?))
    }
    pub fn all_transactions(&self) -> impl Iterator<Item = &ShieldedTx> {
        std::iter::once(&self.miner)
            .chain(self.treasury.iter())
            .chain(self.transactions.iter())
    }
}
#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Block {
    pub header: Header,
    pub body: Body,
}
impl Block {
    pub fn hash(&self) -> Result<Hash> {
        self.header.hash()
    }
    pub fn bytes(&self) -> Result<Vec<u8>> {
        codec::encode(self)
    }
    pub fn decode(bytes: &[u8]) -> Result<Self> {
        codec::decode(bytes, MAX_BLOCK_BYTES)
    }
    pub fn check_shape(&self) -> Result<()> {
        ensure!(self.header.version == 1, "unsupported block version");
        ensure!(self.bytes()?.len() <= MAX_BLOCK_BYTES, "oversized block");
        ensure!(
            self.body.transactions.len() <= MAX_TRANSACTIONS,
            "too many transactions"
        );
        ensure!(
            self.header.body_hash == self.body.hash()?,
            "body commitment mismatch"
        );
        Ok(())
    }
}

//! Payment proofs: the sender of a shielded payment can prove to anyone that a
//! confirmed transaction paid a given amount (and memo) to a given address.
//!
//! The proof discloses the one-time key of exactly one output. A verifier fetches the
//! transaction's block, checks it against the chain, decrypts that single output and
//! checks that it commits to the recipient's owner tag. Nothing else is revealed: not
//! the sender's other notes, balance, change or keys. Only the sender (who created the
//! key) and the recipient can produce a proof, so it settles "did you pay me?".

use anyhow::{Context, Result, bail, ensure};
use krypton_core::address::{Address, base32_decode, base32_encode, open_disclosed_note};
use krypton_core::types::{Block, BlockHeader, tx_root};
use krypton_core::{ChainParams, Hash32, Network};
use krypton_crypto::field::digest_from_bytes;
use krypton_crypto::hash::tagged;
use krypton_pow::Verifier;

const VERSION: u8 = 1;
const CHECKSUM_LEN: usize = 16;
const PAYLOAD_LEN: usize = 2 + 32 + 1 + 32 + 32 + 32;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PaymentProof {
    pub network: Network,
    pub txid: Hash32,
    /// Output index within the transaction.
    pub output: u8,
    /// One-time key of that output.
    pub key: Hash32,
    /// Recipient owner tag and short-address id.
    pub owner: Hash32,
    pub recipient: Hash32,
}

impl PaymentProof {
    pub fn encode(&self) -> String {
        let mut p = vec![VERSION, self.network.id()];
        p.extend_from_slice(&self.txid);
        p.push(self.output);
        p.extend_from_slice(&self.key);
        p.extend_from_slice(&self.owner);
        p.extend_from_slice(&self.recipient);
        let c = tagged("Krypton/payment-proof", &p);
        p.extend_from_slice(&c[..CHECKSUM_LEN]);
        format!(
            "{}pay1{}",
            self.network.params().address_prefix,
            base32_encode(&p)
        )
    }

    pub fn decode(s: &str) -> Result<Self> {
        let (prefix, data) = s.trim().split_once("pay1").context("not a payment proof")?;
        let b = base32_decode(data).context("payment proof encoding")?;
        ensure!(
            b.len() == PAYLOAD_LEN + CHECKSUM_LEN,
            "payment proof length"
        );
        let (p, c) = b.split_at(PAYLOAD_LEN);
        ensure!(
            tagged("Krypton/payment-proof", p)[..CHECKSUM_LEN] == *c,
            "payment proof checksum (copied incompletely?)"
        );
        ensure!(p[0] == VERSION, "unsupported payment proof version");
        let network = Network::from_id(p[1]).context("payment proof network")?;
        ensure!(
            network.params().address_prefix == prefix,
            "payment proof prefix does not match its network"
        );
        let h = |o: usize| -> Hash32 { p[o..o + 32].try_into().expect("32") };
        ensure!(p[34] < 2, "output index");
        Ok(Self {
            network,
            txid: h(2),
            output: p[34],
            key: h(35),
            owner: h(67),
            recipient: h(99),
        })
    }

    /// Short address of the recipient.
    pub fn recipient_short(&self) -> String {
        format!(
            "{}s1{}",
            self.network.params().address_prefix,
            base32_encode(&self.recipient)
        )
    }
}

/// Chain access needed to check a proof.
pub trait ProofSource {
    fn tx_height(&self, txid: &Hash32) -> Result<Option<u64>>;
    fn raw_block(&self, height: u64) -> Result<Block>;
    /// The main-chain header at `height`, proven by proof of work back to the tip
    /// (a trusted local store may return its own header).
    fn proven_header(
        &self,
        height: u64,
        params: &ChainParams,
        pow: &Verifier,
    ) -> Result<BlockHeader>;
    fn tip_height(&self) -> Result<u64>;
    /// The full address published for a short address, if the source keeps a directory.
    /// The caller checks that it hashes to the short address.
    fn resolve_recipient(&self, _short: &str) -> Result<Option<String>> {
        Ok(None)
    }
}

#[derive(Clone, Debug, serde::Serialize)]
pub struct PaymentReport {
    pub txid: String,
    pub height: u64,
    pub confirmations: u64,
    pub value: u64,
    pub memo: String,
    pub recipient: String,
    /// Some(true/false) when the verifier supplied the recipient's address.
    pub recipient_confirmed: Option<bool>,
    /// `recipient` provably belongs to the owner the payment went to (checked against
    /// the verifier's own address or the published address the short form hashes to).
    /// The short address is chosen by the prover, so it means nothing when false.
    pub recipient_verified: bool,
    pub time: u64,
}

pub fn verify(
    proof: &PaymentProof,
    src: &impl ProofSource,
    recipient: Option<&Address>,
    pow: &Verifier,
) -> Result<PaymentReport> {
    let params = proof.network.params();
    let height = src
        .tx_height(&proof.txid)?
        .context("the transaction is not confirmed on this chain")?;
    let block = src.raw_block(height)?;
    let header = src.proven_header(height, &params, pow)?;
    ensure!(
        block.header == header && tx_root(&block.txs) == header.tx_root,
        "block data does not match the chain"
    );
    let tx = block
        .txs
        .iter()
        .find(|t| t.txid() == proof.txid)
        .context("transaction not in its block")?;
    let out = &tx.outputs[proof.output as usize];
    let owner = digest_from_bytes(&proof.owner).context("owner tag")?;
    let Some(note) = open_disclosed_note(&proof.key, &owner, &out.cm, &out.ciphertext) else {
        bail!("the proof does not open this payment: it is forged or for another output");
    };
    let paid = |a: &Address| {
        krypton_crypto::field::digest_to_bytes(&a.owner) == proof.owner
            && a.short_id() == proof.recipient
            && a.network == proof.network
    };
    let recipient_confirmed = recipient.map(paid);
    let recipient_verified = recipient_confirmed == Some(true)
        || src
            .resolve_recipient(&proof.recipient_short())
            .ok()
            .flatten()
            .and_then(|s| Address::decode(&s).ok())
            .is_some_and(|a| paid(&a));
    let tip = src.tip_height()?;
    Ok(PaymentReport {
        txid: hex::encode(proof.txid),
        height,
        confirmations: tip.saturating_sub(height) + 1,
        value: note.value,
        memo: note.memo_text(),
        recipient: proof.recipient_short(),
        recipient_confirmed,
        recipient_verified,
        time: header.timestamp,
    })
}

/// [`verify`] from the point of view of the wallet `me` (if any): a proof that names
/// `me` as the recipient but paid someone else is rejected as forged; for proofs about
/// other people's payments `recipient_confirmed` is None.
pub fn verify_for(
    proof: &PaymentProof,
    src: &impl ProofSource,
    me: Option<&Address>,
    pow: &Verifier,
) -> Result<PaymentReport> {
    let mut r = verify(proof, src, me, pow)?;
    if r.recipient_confirmed == Some(false) {
        ensure!(
            me.is_none_or(|a| a.short_id() != proof.recipient),
            "this proof names this wallet as the recipient, but the payment went to someone else: it is forged"
        );
        r.recipient_confirmed = None;
    }
    Ok(r)
}

impl ProofSource for krypton_light::client::NodeClient {
    fn tx_height(&self, txid: &Hash32) -> Result<Option<u64>> {
        self.tx_height(txid)
    }
    fn raw_block(&self, height: u64) -> Result<Block> {
        self.raw_block(height)
    }
    fn proven_header(
        &self,
        height: u64,
        params: &ChainParams,
        pow: &Verifier,
    ) -> Result<BlockHeader> {
        let tip = self.verified_tip(params, pow, None)?;
        let h = krypton_light::client::NodeClient::header(self, height)?;
        ensure!(
            self.header_in(&h, &tip)?,
            "the node's block is not part of the chain proven by proof of work"
        );
        Ok(h)
    }
    fn tip_height(&self) -> Result<u64> {
        Ok(self.info()?.height)
    }
    fn resolve_recipient(&self, short: &str) -> Result<Option<String>> {
        Ok(self.resolve_short(short).ok())
    }
}

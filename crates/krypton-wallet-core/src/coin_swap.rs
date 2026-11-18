//! Swaps of GNR for a Krypton coin in one transaction (`docs/spec/assets.md`).
//!
//! The coin and GNR live on the same chain, so a swap needs no lock notes: the coin
//! seller's coin spend is the transaction's action and the buyer's GNR spend is its fee
//! action. Both proofs bind the same sighash, which covers every output, so neither side
//! can change what the other receives, and the transaction is valid only whole.
//!
//! 1. The buyer builds an unproven GNR spend paying the seller (and the fee): a
//!    [`GnrHalf`].
//! 2. The seller checks that it pays her the agreed GNR, adds her coin spend paying the
//!    buyer, proves her half and returns the transaction.
//! 3. The buyer checks that it pays him the agreed coin, proves his half and submits.
//!
//! Nobody can lose anything: until the buyer's proof exists the transaction is invalid,
//! and it expires a few blocks later. Amounts and the coin stay hidden on chain (unless
//! the coin is pausable).

use super::*;
use krypton_core::address::try_decrypt_note;
use krypton_core::asset::{AssetOp, AssetPart, FeeAction};

/// Blocks a swap transaction stays valid: short, so neither side holds a free option.
pub const SWAP_EXPIRY_BLOCKS: u64 = 20;

/// What is exchanged: `units` base units of `coin` for `gnr` krypton.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct SwapTerms {
    pub coin: AssetId,
    pub units: u64,
    pub gnr: u64,
}

/// The buyer's half: an unproven GNR spend paying the seller, and where the coin goes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct GnrHalf {
    pub terms: SwapTerms,
    /// The buyer's address (receives the coin).
    pub buyer: String,
    pub action: FeeAction,
    pub expiry: u64,
}

/// A half this wallet built and has not finished yet (kept in memory only: a swap
/// interrupted by a restart simply expires).
pub(crate) struct OpenHalf {
    half: GnrHalf,
    witness: SpendWitness,
    notes: Vec<OwnedNote>,
    /// One-time key and owner of the output paying the seller (for the trade report).
    paid_key: Zeroizing<[u8; 32]>,
    seller_owner: Hash32,
}

/// What a finished swap can prove to a board: the one-time keys and owners of the two
/// outputs that cross, at their indices among the transaction's outputs.
#[derive(Clone, Debug, Serialize)]
pub struct SwapReceipt {
    pub txid: Hash32,
    pub terms: SwapTerms,
    pub coin_output: (u8, [u8; 32], Hash32),
    pub gnr_output: (u8, [u8; 32], Hash32),
}

fn swap_id(cm: &Hash32) -> String {
    format!("swap:{}", hex::encode(cm))
}

impl Wallet {
    fn tip(&self) -> Result<u64> {
        Ok(self
            .verifier
            .as_ref()
            .context("sync the wallet first")?
            .last
            .height)
    }

    /// Buyer, step 1: an unproven GNR spend paying `terms.gnr` to `seller` plus `fee`.
    pub fn coin_swap_half(
        &mut self,
        src: &impl ChainSource,
        terms: SwapTerms,
        seller: &Address,
        fee: u64,
    ) -> Result<GnrHalf> {
        ensure!(
            seller.network == self.network(),
            "the seller is on another network"
        );
        ensure!(terms.units > 0 && terms.gnr > 0, "empty swap");
        self.check_fee(fee)?;
        let fee = super::coins::two_proof_fee(fee);
        let need = terms.gnr.checked_add(fee).context("amount overflow")?;
        let notes = self.select_notes(need)?;
        let total: u64 = notes.iter().map(|n| n.value).sum();
        let me = self.address();
        let outs = [
            self.build_out(seller, terms.gnr, "swap", None)?,
            self.build_out(&me, total - need, "change", None)?,
        ];
        let s = self.spend(src, notes, outs, fee, 0, None)?;
        let paid_key = Zeroizing::new(*s.outs[0].key);
        let half = GnrHalf {
            terms,
            buyer: me.encode(),
            action: FeeAction {
                anchor: digest_to_bytes(&s.witness.root),
                nullifiers: [
                    HexHash(digest_to_bytes(&s.witness.inputs[0].nullifier())),
                    HexHash(digest_to_bytes(&s.witness.inputs[1].nullifier())),
                ],
                outputs: [s.outs[0].output.clone(), s.outs[1].output.clone()],
                fee,
                proof: vec![],
            },
            expiry: self.tip()? + SWAP_EXPIRY_BLOCKS,
        };
        self.open_halves.push(OpenHalf {
            half: half.clone(),
            witness: s.witness,
            notes: s.notes,
            paid_key,
            seller_owner: digest_to_bytes(&seller.owner),
        });
        Ok(half)
    }

    /// Seller, step 2: checks that `half` pays us `expect.gnr` and returns the swap
    /// transaction with our coin spend proven. Our coin notes stay reserved until the
    /// swap is mined or expires.
    pub fn coin_swap_accept(
        &mut self,
        src: &impl ChainSource,
        half: &GnrHalf,
        expect: SwapTerms,
    ) -> Result<Transaction> {
        ensure!(half.terms == expect, "the buyer changed the terms");
        let tip = self.tip()?;
        ensure!(
            half.expiry > tip && half.expiry <= tip + 2 * SWAP_EXPIRY_BLOCKS,
            "the swap's expiry is out of range"
        );
        ensure!(half.action.fee >= MIN_FEE, "the buyer's fee is too low");
        ensure!(half.action.proof.is_empty(), "unexpected proof");
        let buyer = Address::decode(&half.buyer)?;
        ensure!(
            buyer.network == self.network(),
            "the buyer is on another network"
        );
        let owner = self.keys.spend.owner;
        let paid = half.action.outputs.iter().any(|o| {
            try_decrypt_note(&self.keys.kem, &owner, &o.cm, &o.ciphertext)
                .is_some_and(|n| n.coin().is_none() && n.value == expect.gnr)
        });
        ensure!(paid, "the buyer's half does not pay the agreed GNR");
        let coin = expect.coin;
        let notes = self.select_notes_of(Some(coin), expect.units)?;
        let total: u64 = notes.iter().map(|n| n.value).sum();
        let me = self.address();
        let outs = [
            self.build_out(&buyer, expect.units, "swap", Some(coin))?,
            self.build_out(&me, total - expect.units, "change", Some(coin))?,
        ];
        let s = self.spend(
            src,
            notes,
            outs,
            0,
            0,
            Some(krypton_zk::AssetWitness {
                id: coin.elements(),
                burn: 0,
                reveal: coin.pausable,
            }),
        )?;
        let mut tx = Transaction {
            version: TX_VERSION_ASSET,
            kind: TxKind::Transfer,
            anchor: digest_to_bytes(&s.witness.root),
            nullifiers: [
                HexHash(digest_to_bytes(&s.witness.inputs[0].nullifier())),
                HexHash(digest_to_bytes(&s.witness.inputs[1].nullifier())),
            ],
            outputs: [s.outs[0].output.clone(), s.outs[1].output.clone()],
            fee: 0,
            mint: 0,
            expiry_height: half.expiry,
            lock: None,
            proof: vec![],
            asset: Some(AssetPart {
                op: AssetOp::Transfer,
                coin: coin.pausable.then_some(coin),
                burn: 0,
                manifest: None,
                issuer_sig: None,
                fee_action: Some(half.action.clone()),
            }),
        };
        let mut w = s.witness.clone();
        w.sighash = tx.sighash();
        tx.proof = krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?.1;
        let id = swap_id(&s.outs[1].output.cm);
        for n in self.state.notes.iter_mut() {
            if s.notes.iter().any(|c| c.cm == n.cm) {
                n.pending_spend = Some(id.clone());
            }
        }
        let mut e = self.entry(
            -(expect.units as i128),
            "swap",
            Some(buyer.short()),
            Some(coin),
        );
        e.txid = id;
        e.change_cm = Some(hex::encode(s.outs[1].output.cm));
        e.expiry = Some(half.expiry);
        self.state.history.push(e);
        self.save()?;
        Ok(tx)
    }

    /// Buyer, step 3: checks that the seller's transaction pays us the agreed coin and
    /// keeps our half unchanged, proves our half and submits.
    pub fn coin_swap_finish(
        &mut self,
        src: &impl ChainSource,
        mut tx: Transaction,
    ) -> Result<SwapReceipt> {
        let a = tx.asset.as_ref().context("not a coin transaction")?;
        let fa = a.fee_action.as_ref().context("no GNR half")?;
        let i = self
            .open_halves
            .iter()
            .position(|h| {
                let mut ours = h.half.action.clone();
                ours.proof = fa.proof.clone();
                ours == *fa
            })
            .context("this transaction does not carry one of our halves unchanged")?;
        let open = &self.open_halves[i];
        let terms = open.half.terms;
        ensure!(
            tx.version == TX_VERSION_ASSET
                && a.op == AssetOp::Transfer
                && a.burn == 0
                && tx.mint == 0
                && tx.fee == 0
                && tx.lock.is_none()
                && a.issuer_sig.is_none()
                && fa.proof.is_empty()
                && tx.expiry_height == open.half.expiry,
            "the seller's transaction is not a plain swap"
        );
        let owner = self.keys.spend.owner;
        let got = tx
            .outputs
            .iter()
            .position(|o| {
                try_decrypt_note(&self.keys.kem, &owner, &o.cm, &o.ciphertext)
                    .is_some_and(|n| n.coin() == Some(terms.coin) && n.value == terms.units)
            })
            .context("the seller's transaction does not pay the agreed coin")?;
        let got_key = krypton_core::address::note_key(&self.keys.kem, &tx.outputs[got].ciphertext)
            .context("cannot open our output")?;
        krypton_zk::verify(&tx.statement()?, &tx.proof)
            .map_err(|_| anyhow::anyhow!("the seller's proof is invalid"))?;
        let mut w = open.witness.clone();
        w.sighash = tx.sighash();
        let proof = krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?.1;
        tx.asset
            .as_mut()
            .and_then(|a| a.fee_action.as_mut())
            .expect("fee action")
            .proof = proof;
        let txid = src.submit(&tx)?;
        let open = self.open_halves.remove(i);
        let id = hex::encode(txid);
        for n in self.state.notes.iter_mut() {
            if open.notes.iter().any(|c| c.cm == n.cm) {
                n.pending_spend = Some(id.clone());
            }
        }
        let fee = open.half.action.fee;
        let mut e = self.entry(-((terms.gnr + fee) as i128), "swap", None, None);
        e.txid = id;
        e.fee = fee;
        e.change_cm = Some(hex::encode(open.half.action.outputs[1].cm));
        e.expiry = Some(tx.expiry_height);
        self.state.history.push(e);
        self.save()?;
        Ok(SwapReceipt {
            txid,
            terms,
            coin_output: (got as u8, *got_key, digest_to_bytes(&owner)),
            gnr_output: (2, *open.paid_key, open.seller_owner),
        })
    }

    /// Forgets an unfinished half (the swap was abandoned).
    pub fn coin_swap_drop(&mut self, half: &GnrHalf) {
        self.open_halves.retain(|h| h.half != *half);
    }
}

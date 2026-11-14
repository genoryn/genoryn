//! Krypton coins in the wallet (`docs/spec/assets.md`): balances, sends and the issuer
//! operations. A coin note is an ordinary note whose commitment includes its coin; the
//! recipient learns the coin from the encrypted memo. Fees are always GNR, paid by a
//! second spend bound to the same transaction.

use super::*;
use krypton_core::address::encrypt_note_disclosable;
use krypton_core::asset::{AssetOp, AssetPart, DEPLOY_FEE, FeeAction, Manifest, SIG_CTX};
use krypton_zk::AssetWitness;

/// Balance of one Krypton coin, in its base units.
#[derive(Debug, Clone, Copy, Serialize)]
pub struct CoinBalance {
    pub coin: AssetId,
    pub spendable: u64,
    pub pending: u64,
    pub immature: u64,
    pub total: u64,
}

/// An output being built: its on-chain form, its witness and its one-time key.
pub(crate) struct Built {
    pub(crate) output: Output,
    pub(crate) witness: OutputWitness,
    pub(crate) key: Zeroizing<[u8; 32]>,
}

/// A spend being built: what it spends and creates.
pub(crate) struct Spend {
    pub(crate) notes: Vec<OwnedNote>,
    pub(crate) witness: SpendWitness,
    pub(crate) outs: [Built; 2],
}

/// A coin transaction carries two proofs, about twice the size of a transfer, so it pays
/// twice the fee of one transfer (`fee` arguments are the fee of one ordinary transfer).
pub fn two_proof_fee(fee: u64) -> u64 {
    fee.saturating_mul(2)
}

fn fresh_rcm() -> Digest {
    let mut b = [0u8; 32];
    rand_core::OsRng.fill_bytes(&mut b);
    digest_from_uniform(&b)
}

fn coin_witness(coin: AssetId, burn: u64, reveal: bool) -> Option<AssetWitness> {
    Some(AssetWitness {
        id: coin.elements(),
        burn,
        reveal,
    })
}

impl Wallet {
    /// Balances of every coin this wallet holds or held (GNR excluded).
    pub fn coin_balances(&self) -> Vec<CoinBalance> {
        let mut v: Vec<CoinBalance> = Vec::new();
        for n in self.state.notes.iter().filter(|n| n.spent_height.is_none()) {
            let Some(coin) = n.coin else { continue };
            let i = match v.iter().position(|b| b.coin == coin) {
                Some(i) => i,
                None => {
                    v.push(CoinBalance {
                        coin,
                        spendable: 0,
                        pending: 0,
                        immature: 0,
                        total: 0,
                    });
                    v.len() - 1
                }
            };
            let b = &mut v[i];
            b.total += n.value;
            if n.position.is_none() {
                b.immature += n.value;
            } else if n.pending_spend.is_some() {
                b.pending += n.value;
            } else {
                b.spendable += n.value;
            }
        }
        v.sort_by_key(|b| b.coin);
        v
    }

    pub fn coin_balance(&self, coin: AssetId) -> u64 {
        self.coin_balances()
            .iter()
            .find(|b| b.coin == coin)
            .map_or(0, |b| b.spendable)
    }

    /// The public key this wallet issues coins with (goes into a manifest's `issuer`).
    pub fn issuer_key(&self) -> Vec<u8> {
        self.keys.issuer.public_bytes()
    }

    pub(crate) fn build_out(
        &self,
        to: &Address,
        value: u64,
        memo: &str,
        coin: Option<AssetId>,
    ) -> Result<Built> {
        let mut plain = NotePlain {
            value,
            rcm: fresh_rcm(),
            memo: memo_from_str(memo),
        };
        plain.set_coin(coin);
        let (cm, ciphertext, key) = encrypt_note_disclosable(to, &plain)?;
        Ok(Built {
            output: Output { cm, ciphertext },
            witness: OutputWitness {
                owner: to.owner,
                value,
                rcm: plain.rcm,
            },
            key,
        })
    }

    fn verified_root(&self) -> Result<Digest> {
        let v = self.verifier.as_ref().context("sync the wallet first")?;
        Ok(digest_from_bytes(&v.last.note_root)?)
    }

    /// A spend of `notes` (one or two, or none for an issue) to `outs`.
    pub(crate) fn spend(
        &self,
        src: &impl ChainSource,
        notes: Vec<OwnedNote>,
        outs: [Built; 2],
        fee: u64,
        mint: u64,
        asset: Option<AssetWitness>,
    ) -> Result<Spend> {
        let (inputs, root) = if notes.is_empty() {
            (
                vec![InputWitness::dummy(), InputWitness::dummy()],
                self.verified_root()?,
            )
        } else {
            self.input_witnesses(src, &notes)?
        };
        let witness = SpendWitness {
            inputs: [inputs[0].clone(), inputs[1].clone()],
            outputs: [outs[0].witness.clone(), outs[1].witness.clone()],
            root,
            fee,
            mint,
            sighash: [krypton_crypto::field::Felt::new(0); 4],
            asset,
        };
        Ok(Spend {
            notes,
            witness,
            outs,
        })
    }

    /// A GNR spend paying `fee` (plus `extra` burned into the fee, e.g. a deploy fee)
    /// with the change back to us.
    fn gnr_spend(&self, src: &impl ChainSource, fee: u64) -> Result<Spend> {
        let notes = self.select_notes(fee)?;
        let total: u64 = notes.iter().map(|n| n.value).sum();
        let me = self.address();
        let outs = [
            self.build_out(&me, total - fee, "change", None)?,
            self.build_out(&me, 0, "", None)?,
        ];
        self.spend(src, notes, outs, fee, 0, None)
    }

    pub(crate) fn check_fee(&self, fee: u64) -> Result<()> {
        ensure!(
            (MIN_FEE..=MAX_FEE).contains(&fee),
            "the fee must be between {} and {} GNR",
            emission::format_amount(MIN_FEE),
            emission::format_amount(MAX_FEE)
        );
        ensure!(self.verifier.is_some(), "sync the wallet first");
        Ok(())
    }

    /// Signs (issuer operations), proves and submits a version-3 transaction, then
    /// records it. `entry` is the history record (its txid is filled in here).
    fn submit_coin_tx(
        &mut self,
        src: &impl ChainSource,
        main: Spend,
        mut part: AssetPart,
        fee: Option<Spend>,
        sign: bool,
        mut entry: HistoryEntry,
    ) -> Result<Hash32> {
        let tip = self
            .verifier
            .as_ref()
            .context("sync the wallet first")?
            .last
            .height;
        let nfs = |s: &SpendWitness| {
            [
                HexHash(digest_to_bytes(&s.inputs[0].nullifier())),
                HexHash(digest_to_bytes(&s.inputs[1].nullifier())),
            ]
        };
        part.fee_action = fee.as_ref().map(|f| FeeAction {
            anchor: digest_to_bytes(&f.witness.root),
            nullifiers: nfs(&f.witness),
            outputs: [f.outs[0].output.clone(), f.outs[1].output.clone()],
            fee: f.witness.fee,
            proof: vec![],
        });
        let mut tx = Transaction {
            version: TX_VERSION_ASSET,
            kind: TxKind::Transfer,
            anchor: digest_to_bytes(&main.witness.root),
            nullifiers: nfs(&main.witness),
            outputs: [main.outs[0].output.clone(), main.outs[1].output.clone()],
            fee: main.witness.fee,
            mint: main.witness.mint,
            expiry_height: tip + 120,
            lock: None,
            proof: vec![],
            asset: Some(part),
        };
        let sighash = tx.sighash();
        if sign {
            let sig = self
                .keys
                .issuer
                .sign(&tx.sighash_bytes(), SIG_CTX)
                .map_err(|e| anyhow::anyhow!("{e}"))?;
            tx.asset.as_mut().expect("coin part").issuer_sig = Some(sig);
        }
        let prove = |mut w: SpendWitness| -> Result<Vec<u8>> {
            w.sighash = sighash;
            Ok(krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?.1)
        };
        tx.proof = prove(main.witness.clone())?;
        if let Some(f) = &fee {
            let p = prove(f.witness.clone())?;
            tx.asset
                .as_mut()
                .and_then(|a| a.fee_action.as_mut())
                .expect("fee action")
                .proof = p;
        }
        let txid = src.submit(&tx)?;
        let id = hex::encode(txid);
        let spent: Vec<Hash32> = main
            .notes
            .iter()
            .chain(fee.iter().flat_map(|f| f.notes.iter()))
            .map(|n| n.cm)
            .collect();
        for n in self.state.notes.iter_mut() {
            if spent.contains(&n.cm) {
                n.pending_spend = Some(id.clone());
            }
        }
        // The transaction confirms when one of our GNR change outputs appears.
        let change = fee.as_ref().map_or(&main.outs[0], |f| &f.outs[0]);
        entry.txid = id;
        entry.change_cm = Some(hex::encode(change.output.cm));
        entry.expiry = Some(tx.expiry_height);
        entry.fee = tx.total_fee();
        self.state.history.push(entry);
        self.save()?;
        Ok(txid)
    }

    pub(crate) fn entry(
        &self,
        amount: i128,
        memo: &str,
        to: Option<String>,
        coin: Option<AssetId>,
    ) -> HistoryEntry {
        HistoryEntry {
            txid: String::new(),
            height: None,
            amount,
            fee: 0,
            memo: memo.to_string(),
            to,
            time: krypton_core::now(),
            change_cm: None,
            disclosure: None,
            expiry: None,
            coin,
        }
    }

    /// Sends `amount` base units of `coin`. The coin stays hidden on chain unless it is
    /// a pausable coin, whose transfers always show which coin moves.
    pub fn send_coin(
        &mut self,
        src: &impl ChainSource,
        coin: AssetId,
        to: &Address,
        amount: u64,
        fee: u64,
        memo: &str,
    ) -> Result<Hash32> {
        ensure!(
            to.network == self.network(),
            "recipient address is for {}",
            to.network.as_str()
        );
        ensure!(amount > 0, "amount must be positive");
        ensure!(!coin.is_gnr(), "GNR is sent with send");
        self.check_fee(fee)?;
        let notes = self.select_notes_of(Some(coin), amount)?;
        let total: u64 = notes.iter().map(|n| n.value).sum();
        let me = self.address();
        let outs = [
            self.build_out(to, amount, memo, Some(coin))?,
            self.build_out(&me, total - amount, "change", Some(coin))?,
        ];
        let main = self.spend(src, notes, outs, 0, 0, coin_witness(coin, 0, coin.pausable))?;
        let fee_spend = self.gnr_spend(src, two_proof_fee(fee))?;
        let to_self = to.owner == me.owner;
        let mut entry = self.entry(
            if to_self { 0 } else { -(amount as i128) },
            memo,
            Some(to.short()),
            Some(coin),
        );
        entry.disclosure = (!to_self).then(|| Disclosure {
            output: 0,
            key: main.outs[0].key.to_vec(),
            owner: digest_to_bytes(&to.owner).to_vec(),
            recipient: to.short_id().to_vec(),
        });
        let mut part = AssetPart {
            op: AssetOp::Transfer,
            coin: coin.pausable.then_some(coin),
            burn: 0,
            manifest: None,
            issuer_sig: None,
            fee_action: None,
        };
        part.coin = coin.pausable.then_some(coin);
        let cm0 = main.outs[0].output.cm;
        let txid = self.submit_coin_tx(src, main, part, Some(fee_spend), false, entry)?;
        if to_self {
            // The payment is ours too: it must not show as income.
            let mut e = self.entry(0, "internal", None, Some(coin));
            e.txid = format!("self:{}", hex::encode(cm0));
            e.change_cm = Some(hex::encode(cm0));
            self.state.history.push(e);
            self.save()?;
        }
        Ok(txid)
    }

    /// Issues `amount` new base units of `coin` (which this wallet's issuer key controls)
    /// to `to`. Issues are public: anyone can audit a coin's supply.
    pub fn issue_coin(
        &mut self,
        src: &impl ChainSource,
        coin: AssetId,
        to: &Address,
        amount: u64,
        fee: u64,
    ) -> Result<Hash32> {
        ensure!(
            to.network == self.network(),
            "recipient address is for another network"
        );
        ensure!(amount > 0, "amount must be positive");
        self.check_fee(fee)?;
        let me = self.address();
        let outs = [
            self.build_out(to, amount, "issue", Some(coin))?,
            self.build_out(&me, 0, "", Some(coin))?,
        ];
        let main = self.spend(src, vec![], outs, 0, amount, coin_witness(coin, 0, true))?;
        let fee_spend = self.gnr_spend(src, two_proof_fee(fee))?;
        let part = AssetPart {
            op: AssetOp::Issue,
            coin: Some(coin),
            burn: 0,
            manifest: None,
            issuer_sig: None,
            fee_action: None,
        };
        let entry = self.entry(0, "issue", Some(to.short()), Some(coin));
        self.submit_coin_tx(src, main, part, Some(fee_spend), true, entry)
    }

    /// Destroys `amount` base units of `coin` from this wallet (public, recorded).
    pub fn burn_coin(
        &mut self,
        src: &impl ChainSource,
        coin: AssetId,
        amount: u64,
        fee: u64,
    ) -> Result<Hash32> {
        ensure!(amount > 0, "amount must be positive");
        self.check_fee(fee)?;
        let notes = self.select_notes_of(Some(coin), amount)?;
        let total: u64 = notes.iter().map(|n| n.value).sum();
        let me = self.address();
        let outs = [
            self.build_out(&me, total - amount, "change", Some(coin))?,
            self.build_out(&me, 0, "", Some(coin))?,
        ];
        let main = self.spend(src, notes, outs, 0, 0, coin_witness(coin, amount, true))?;
        let fee_spend = self.gnr_spend(src, two_proof_fee(fee))?;
        let part = AssetPart {
            op: AssetOp::Burn,
            coin: Some(coin),
            burn: amount,
            manifest: None,
            issuer_sig: None,
            fee_action: None,
        };
        let entry = self.entry(-(amount as i128), "burn", None, Some(coin));
        self.submit_coin_tx(src, main, part, Some(fee_spend), false, entry)
    }

    /// Deploys a coin whose manifest names this wallet's issuer key, paying the deploy
    /// fee (10 GNR) plus `fee`.
    pub fn deploy_coin(
        &mut self,
        src: &impl ChainSource,
        manifest: &Manifest,
        fee: u64,
    ) -> Result<Hash32> {
        manifest.validate().map_err(|e| anyhow::anyhow!("{e}"))?;
        ensure!(
            manifest.issuer == self.issuer_key(),
            "the manifest's issuer key is not this wallet's"
        );
        self.check_fee(fee)?;
        let main = self.gnr_spend(src, DEPLOY_FEE + fee)?;
        let part = AssetPart {
            op: AssetOp::Deploy,
            coin: None,
            burn: 0,
            manifest: Some(manifest.canonical()),
            issuer_sig: None,
            fee_action: None,
        };
        let entry = self.entry(
            -((DEPLOY_FEE + fee) as i128),
            &format!("deploy {}", manifest.ticker),
            None,
            None,
        );
        self.submit_coin_tx(src, main, part, None, true, entry)
    }

    /// Pauses or resumes every transfer of a pausable coin this wallet issues.
    pub fn set_coin_paused(
        &mut self,
        src: &impl ChainSource,
        coin: AssetId,
        paused: bool,
        fee: u64,
    ) -> Result<Hash32> {
        ensure!(coin.pausable, "this coin was declared without pause");
        self.check_fee(fee)?;
        let main = self.gnr_spend(src, fee)?;
        let op = if paused {
            AssetOp::Pause
        } else {
            AssetOp::Unpause
        };
        let part = AssetPart {
            op,
            coin: Some(coin),
            burn: 0,
            manifest: None,
            issuer_sig: None,
            fee_action: None,
        };
        let entry = self.entry(-(fee as i128), op.as_str(), None, None);
        self.submit_coin_tx(src, main, part, None, true, entry)
    }
}

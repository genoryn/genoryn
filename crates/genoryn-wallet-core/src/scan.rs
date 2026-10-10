use crate::keys::{Wallet, encode_address};
use anyhow::{Context, Result, ensure};
use genoryn_core::{
    block::Block,
    chain::unix_time,
    codec::Hash,
    names,
    params::MIN_FEE,
    privacy::{self, BUNDLE_VERSION, ShieldedTx, TxContext},
    state::{ChainState, ProofCache, Verification},
};
use orchard::{
    Address, Note,
    builder::{Builder, BundleType},
    keys::{Scope, SpendAuthorizingKey},
    value::NoteValue,
};
use rand08::RngCore;
use serde::Serialize;
use std::collections::BTreeMap;

/// Spends per transfer; each costs a Halo 2 action, bounding proof time and size.
pub const MAX_INPUTS: usize = 16;

fn format_gnr(amount: u128) -> String {
    genoryn_core::emission::format_gnr(amount)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

pub struct OwnedNote {
    pub note: Note,
    pub txid: Hash,
    pub action: usize,
    pub height: u64,
    pub timestamp: u64,
    pub position: Option<usize>,
    pub spent: bool,
    pub memo: String,
}
#[derive(Clone, Serialize)]
pub struct HistoryEntry {
    pub txid: String,
    pub direction: String,
    pub amount_atomic: String,
    pub height: u64,
    pub timestamp: u64,
    pub status: String,
    pub confirmations: u64,
    pub memo: String,
}
pub struct ScannedWallet {
    pub state: ChainState,
    pub notes: Vec<OwnedNote>,
    pub history: Vec<HistoryEntry>,
}
#[derive(Serialize)]
pub struct WalletSummary {
    pub address: String,
    pub balance_atomic: String,
    pub spendable_atomic: String,
    pub pending_atomic: String,
    pub height: u64,
    pub history: Vec<HistoryEntry>,
}

pub fn validate_chain(
    network: genoryn_core::params::Network,
    blocks: &[Block],
) -> Result<ChainState> {
    replay_chain(network, blocks, Verification::Full)
}
/// Applies `blocks` from genesis. `Verification::Stored` is only for blocks this
/// computer (or the user's own node) already validated in full.
pub fn replay_chain(
    network: genoryn_core::params::Network,
    blocks: &[Block],
    verification: Verification,
) -> Result<ChainState> {
    let mut state = ChainState::new(network);
    let mut cache = ProofCache::default();
    for block in blocks {
        state.apply_block_in_place(block, unix_time(), &mut cache, verification)?;
    }
    Ok(state)
}

impl Wallet {
    /// Fully validate the downloaded chain before reconstructing any private balance.
    /// All scanning happens locally, never by requesting address-specific outputs.
    pub fn scan(&self, blocks: &[Block]) -> Result<ScannedWallet> {
        self.scan_with_state(validate_chain(self.network, blocks)?, blocks)
    }
    /// Scans `blocks`, whose validated state is `state` (from `replay_chain` or
    /// the light wallet's verified chain).
    pub fn scan_with_state(&self, state: ChainState, blocks: &[Block]) -> Result<ScannedWallet> {
        ensure!(
            state.height == blocks.len() as u64
                && state.best_hash
                    == blocks
                        .last()
                        .map(|b| b.hash())
                        .transpose()?
                        .unwrap_or_else(|| self.network.genesis()),
            "validated state does not match the scanned blocks"
        );
        let fvk = self.viewing_key()?;
        let ivks = [fvk.to_ivk(Scope::External), fvk.to_ivk(Scope::Internal)];
        let ovk = fvk.to_ovk(Scope::External);
        let positions: BTreeMap<_, _> = state
            .mature
            .iter()
            .enumerate()
            .map(|(i, n)| ((n.txid, n.action as usize), i))
            .collect();
        let mut notes = Vec::new();
        let mut history = Vec::new();
        for block in blocks {
            for tx in block.body.all_transactions() {
                let bundle = tx.bundle.to_bundle()?;
                let txid = tx.id()?;
                let incoming = bundle.decrypt_outputs_with_keys(&ivks);
                let outgoing = bundle.recover_outputs_with_ovks(std::slice::from_ref(&ovk));
                let mut received = 0u128;
                for (action, _, note, _, memo) in incoming {
                    if note.value().inner() == 0 {
                        continue;
                    }
                    let spent = state.nullifiers.contains(&note.nullifier(&fvk).to_bytes());
                    received += note.value().inner() as u128;
                    let memo = decode_memo(&memo);
                    notes.push(OwnedNote {
                        note,
                        txid,
                        action,
                        height: block.header.height,
                        timestamp: block.header.timestamp,
                        position: positions.get(&(txid, action)).copied(),
                        spent,
                        memo,
                    });
                }
                let mut sent = 0u128;
                let mut sent_memo = String::new();
                let mut burned = 0u128;
                let mut burned_memo = String::new();
                for (_, _, note, recipient, memo) in outgoing {
                    if note.value().inner() == 0 {
                        continue;
                    }
                    // Recognize change by attempting this bundle's incoming decryption, not an address graph.
                    let own =
                        bundle
                            .decrypt_outputs_with_keys(&ivks)
                            .iter()
                            .any(|(_, _, n, _, _)| {
                                orchard::note::ExtractedNoteCommitment::from(n.commitment())
                                    == orchard::note::ExtractedNoteCommitment::from(
                                        note.commitment(),
                                    )
                            });
                    if own {
                        continue;
                    }
                    if names::is_burn_address(self.network, &recipient) {
                        burned += note.value().inner() as u128;
                        burned_memo = name_memo(&memo);
                    } else {
                        sent += note.value().inner() as u128;
                        sent_memo = decode_memo(&memo);
                    }
                }
                let conf = state.height - block.header.height + 1;
                if burned > 0 {
                    history.push(HistoryEntry {
                        txid: hex::encode(txid),
                        direction: "name".into(),
                        amount_atomic: burned.to_string(),
                        height: block.header.height,
                        timestamp: block.header.timestamp,
                        status: confirmation_status(conf),
                        confirmations: conf,
                        memo: burned_memo,
                    });
                }
                if sent > 0 {
                    history.push(HistoryEntry {
                        txid: hex::encode(txid),
                        direction: "sent".into(),
                        amount_atomic: sent.to_string(),
                        height: block.header.height,
                        timestamp: block.header.timestamp,
                        status: confirmation_status(conf),
                        confirmations: conf,
                        memo: sent_memo,
                    });
                } else if received > 0 && burned == 0 {
                    history.push(HistoryEntry {
                        txid: hex::encode(txid),
                        direction: if tx.context.kind == 1 {
                            "mined"
                        } else if tx.context.kind == 2 {
                            "treasury"
                        } else {
                            "received"
                        }
                        .into(),
                        amount_atomic: received.to_string(),
                        height: block.header.height,
                        timestamp: block.header.timestamp,
                        status: confirmation_status(conf),
                        confirmations: conf,
                        memo: notes.last().map(|n| n.memo.clone()).unwrap_or_default(),
                    });
                }
            }
        }
        history.reverse();
        Ok(ScannedWallet {
            state,
            notes,
            history,
        })
    }
    pub fn summary(&self, scan: &ScannedWallet) -> Result<WalletSummary> {
        self.summary_with_confirmation_target(scan, 10)
    }
    pub fn summary_with_confirmation_target(
        &self,
        scan: &ScannedWallet,
        target: u64,
    ) -> Result<WalletSummary> {
        ensure!(
            (1..=10_000).contains(&target),
            "confirmation target must be 1-10000"
        );
        let balance = scan
            .notes
            .iter()
            .filter(|n| !n.spent)
            .map(|n| n.note.value().inner() as u128)
            .sum::<u128>();
        let available = scan
            .notes
            .iter()
            .filter(|n| !n.spent && n.position.is_some())
            .map(|n| n.note.value().inner() as u128)
            .sum::<u128>();
        Ok(WalletSummary {
            address: self.receiving_identity()?,
            balance_atomic: balance.to_string(),
            spendable_atomic: available.to_string(),
            pending_atomic: (balance - available).to_string(),
            height: scan.state.height,
            history: scan
                .history
                .iter()
                .cloned()
                .map(|mut entry| {
                    entry.status = if entry.confirmations < target {
                        format!(
                            "{} of {target} confirmations; reorg risk remains",
                            entry.confirmations
                        )
                    } else {
                        format!(
                            "{target}-confirmation display policy reached; reorgs remain possible"
                        )
                    };
                    entry
                })
                .collect(),
        })
    }
    pub fn send(
        &self,
        scan: &ScannedWallet,
        recipient: Address,
        amount: u64,
        fee: u64,
        memo: &str,
    ) -> Result<ShieldedTx> {
        ensure!(amount > 0, "amount must be positive");
        ensure!(memo.len() <= 480, "memo exceeds 480 UTF-8 bytes");
        ensure!(
            !memo.contains('\0'),
            "memo must not contain a NUL character"
        );
        let mut encoded_memo = [0u8; 512];
        encoded_memo[..memo.len()].copy_from_slice(memo.as_bytes());
        self.send_outputs(scan, &[(recipient, amount, encoded_memo)], fee)
    }
    /// The receiving address a username points to: separate from address 0, so
    /// publishing a name does not publish the wallet's main receiving identity.
    pub fn name_receiver(&self) -> Result<Address> {
        self.address(1)
    }
    pub fn name_record(&self, op: u8, name: &str, sequence: u32) -> Result<names::NameRecord> {
        use ed25519_dalek::Signer;
        let name = names::canonical_name(name)?;
        let key = self.identity_key()?;
        let mut record = names::NameRecord {
            op,
            name,
            receiver: self.name_receiver()?.to_raw_address_bytes(),
            owner: key.verifying_key().to_bytes(),
            sequence,
            signature: [0; 64],
        };
        record.signature = key.sign(&record.signing_message(self.network)).to_bytes();
        Ok(record)
    }
    /// A transfer that burns the name's price with its signed record attached.
    pub fn name_transaction(
        &self,
        scan: &ScannedWallet,
        op: u8,
        name: &str,
        sequence: u32,
        fee: u64,
    ) -> Result<ShieldedTx> {
        let record = self.name_record(op, name, sequence)?;
        let price = if op == names::OP_REGISTER {
            names::registration_price(&record.name)
        } else {
            names::UPDATE_PRICE
        };
        self.send_outputs(
            scan,
            &[(names::burn_address(self.network), price, record.encode())],
            fee,
        )
    }
    /// One private transfer paying every `(recipient, amount, memo)` output, with change.
    pub fn send_outputs(
        &self,
        scan: &ScannedWallet,
        outputs: &[(Address, u64, [u8; 512])],
        fee: u64,
    ) -> Result<ShieldedTx> {
        ensure!(!outputs.is_empty(), "a transfer needs an output");
        ensure!(
            outputs.iter().all(|(_, amount, _)| *amount > 0),
            "amount must be positive"
        );
        ensure!(fee >= MIN_FEE, "fee below consensus minimum");
        let total = outputs
            .iter()
            .try_fold(fee, |sum, (_, amount, _)| sum.checked_add(*amount))
            .context("amount overflow")?;
        ensure!(
            total <= i64::MAX as u64,
            "transaction value too large; split the payment"
        );
        let fvk = self.viewing_key()?;
        let anchor = orchard::Anchor::from(scan.state.tree.root());
        let mut builder = Builder::new(
            BundleType::DEFAULT,
            BUNDLE_VERSION,
            BUNDLE_VERSION.default_flags(),
            anchor,
        )?;
        // Largest notes first, so the input limit is reached only when it must be.
        let mut candidates: Vec<_> = scan
            .notes
            .iter()
            .filter(|n| !n.spent && n.position.is_some())
            .collect();
        candidates.sort_by_key(|n| std::cmp::Reverse(n.note.value().inner()));
        let spendable = candidates
            .iter()
            .try_fold(0u64, |sum, n| sum.checked_add(n.note.value().inner()))
            .context("balance overflow")?;
        ensure!(
            spendable >= total,
            "insufficient matured funds: {} GNR needed including the fee, {} GNR spendable. Mined coins become spendable 60 blocks after they are found",
            format_gnr(total.into()),
            format_gnr(spendable.into())
        );
        let mut selected = 0u64;
        for owned in candidates.iter().take(MAX_INPUTS) {
            builder.add_spend(
                fvk.clone(),
                owned.note,
                scan.state
                    .witness(owned.position.context("immature note")?)?,
            )?;
            selected = selected
                .checked_add(owned.note.value().inner())
                .context("input sum overflow")?;
            if selected >= total {
                break;
            }
        }
        ensure!(
            selected >= total,
            "this payment needs more than {MAX_INPUTS} of your notes: send yourself a smaller amount first to combine them, then retry"
        );
        for (recipient, amount, memo) in outputs {
            builder.add_output(
                Some(fvk.to_ovk(Scope::External)),
                *recipient,
                NoteValue::from_raw(*amount),
                *memo,
            )?;
        }
        if selected > total {
            builder.add_output(
                Some(fvk.to_ovk(Scope::External)),
                fvk.address_at(0u32, Scope::Internal),
                NoteValue::from_raw(selected - total),
                [0; 512],
            )?;
        }
        let mut nonce = [0; 32];
        rand08::rngs::OsRng.fill_bytes(&mut nonce);
        let context = TxContext {
            protocol: 1,
            network: self.network,
            kind: 0,
            height: 0,
            expires: scan.state.height + 120,
            nonce,
        };
        privacy::build_bundle(
            context,
            builder,
            &[SpendAuthorizingKey::from(&self.spending_key()?)],
        )
    }
}
fn confirmation_status(confirmations: u64) -> String {
    if confirmations < 10 {
        format!("{confirmations} confirmations; reorg risk remains")
    } else {
        "confirmed (probabilistic finality)".into()
    }
}
/// What a burn output's memo means to its sender: the username it registered or updated.
pub fn name_memo(bytes: &[u8; 512]) -> String {
    match names::NameRecord::decode(bytes) {
        Some(record) if record.op == names::OP_UPDATE => format!("Updated @{}", record.name),
        Some(record) => format!("Registered @{}", record.name),
        None => "Burned without a valid username record".into(),
    }
}
fn decode_memo(bytes: &[u8; 512]) -> String {
    String::from_utf8_lossy(&bytes[..bytes.iter().position(|b| *b == 0).unwrap_or(512)])
        .into_owned()
}

#[derive(Serialize)]
pub struct ProvenPayment {
    pub recipient: String,
    pub amount_atomic: String,
    pub memo: String,
}

impl Wallet {
    // A selective proof is built by recovering a note, exposing only its opening.
    pub fn payment_proof(&self, tx: &ShieldedTx, action: usize) -> Result<NoteProof> {
        let bundle = tx.bundle.to_bundle()?;
        let fvk = self.viewing_key()?;
        let recovered = bundle
            .recover_output_with_ovk(action, &fvk.to_ovk(Scope::External))
            .or_else(|| bundle.decrypt_output_with_key(action, &fvk.to_ivk(Scope::External)))
            .or_else(|| bundle.decrypt_output_with_key(action, &fvk.to_ivk(Scope::Internal)))
            .context("wallet cannot disclose this output")?;
        Ok(NoteProof {
            version: 1,
            network: self.network,
            txid: hex::encode(tx.id()?),
            action: action as u32,
            recipient: encode_address(self.network, recovered.1)?,
            amount: recovered.0.value().inner(),
            rho: recovered.0.rho().to_bytes(),
            rseed: *recovered.0.rseed().as_bytes(),
            cmx: tx.bundle.actions.get(action).context("action index")?.cmx,
        })
    }
}
#[derive(serde::Serialize, serde::Deserialize)]
pub struct NoteProof {
    pub version: u16,
    pub network: genoryn_core::params::Network,
    pub txid: String,
    pub action: u32,
    pub recipient: String,
    pub amount: u64,
    pub rho: Hash,
    pub rseed: Hash,
    pub cmx: Hash,
}
pub fn verify_payment_proof(proof: &NoteProof, tx: &ShieldedTx) -> Result<ProvenPayment> {
    ensure!(
        proof.version == 1
            && proof.network == tx.context.network
            && proof.txid == hex::encode(tx.id()?),
        "payment proof context mismatch"
    );
    tx.verify_crypto()?;
    let action = tx
        .bundle
        .actions
        .get(proof.action as usize)
        .context("payment proof action")?;
    ensure!(
        action.cmx == proof.cmx,
        "payment proof commitment not in transaction"
    );
    let address = crate::keys::decode_address(proof.network, &proof.recipient)?;
    let rho = Option::<orchard::note::Rho>::from(orchard::note::Rho::from_bytes(&proof.rho))
        .context("invalid rho")?;
    let rseed = Option::<orchard::note::RandomSeed>::from(orchard::note::RandomSeed::from_bytes(
        proof.rseed,
        &rho,
    ))
    .context("invalid randomness")?;
    let note = Option::<Note>::from(Note::from_parts(
        address,
        NoteValue::from_raw(proof.amount),
        rho,
        rseed,
        BUNDLE_VERSION.note_version(),
    ))
    .context("invalid note opening")?;
    let cmx = orchard::note::ExtractedNoteCommitment::from(note.commitment()).to_bytes();
    ensure!(cmx == proof.cmx, "invalid payment proof opening");
    Ok(ProvenPayment {
        recipient: proof.recipient.clone(),
        amount_atomic: proof.amount.to_string(),
        memo: String::new(),
    })
}

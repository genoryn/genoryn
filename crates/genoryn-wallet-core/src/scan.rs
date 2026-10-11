use crate::keys::{Wallet, encode_address};
use crate::sender_auth;
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
use orchard::note_encryption::IronwoodDomain;
use orchard::{
    Address, Note,
    builder::{Builder, BundleType},
    keys::{Scope, SpendAuthorizingKey},
    value::NoteValue,
};
use rand08::RngCore;
use serde::Serialize;
use std::collections::BTreeMap;
use zcash_note_encryption::{
    Domain, EphemeralKeyBytes, OutgoingCipherKey, try_output_recovery_with_ock,
};

/// Spends per transfer; each costs a Halo 2 action, bounding proof time and size.
pub const MAX_INPUTS: usize = 16;

fn format_gnr(amount: u128) -> String {
    genoryn_core::emission::format_gnr(amount)
        .trim_end_matches('0')
        .trim_end_matches('.')
        .to_owned()
}

/// One output of a transfer this wallet builds.
#[derive(Clone, Copy)]
pub struct PaymentOutput {
    pub recipient: Address,
    pub amount: u64,
    pub memo: [u8; 512],
    /// Commit a payment-specific sender-verification key in the memo.
    pub sender_auth: bool,
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
    /// One payment with change. With `sender_auth`, a memo of at most
    /// `sender_auth::MAX_MEMO_WITH_AUTH` bytes also carries this payment's
    /// sender-verification key; a longer memo is sent without one.
    pub fn send(
        &self,
        scan: &ScannedWallet,
        recipient: Address,
        amount: u64,
        fee: u64,
        memo: &str,
        sender_auth: bool,
    ) -> Result<ShieldedTx> {
        ensure!(amount > 0, "amount must be positive");
        ensure!(memo.len() <= 480, "memo exceeds 480 UTF-8 bytes");
        ensure!(
            !memo.contains('\0'),
            "memo must not contain a NUL character"
        );
        let mut encoded_memo = [0u8; 512];
        encoded_memo[..memo.len()].copy_from_slice(memo.as_bytes());
        self.send_payments(
            scan,
            &[PaymentOutput {
                recipient,
                amount,
                memo: encoded_memo,
                sender_auth: sender_auth && memo.len() <= sender_auth::MAX_MEMO_WITH_AUTH,
            }],
            fee,
        )
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
        let outputs: Vec<PaymentOutput> = outputs
            .iter()
            .map(|(recipient, amount, memo)| PaymentOutput {
                recipient: *recipient,
                amount: *amount,
                memo: *memo,
                sender_auth: false,
            })
            .collect();
        self.send_payments(scan, &outputs, fee)
    }
    /// One private transfer paying every output, with change. Outputs with
    /// `sender_auth` get this payment's sender-verification key in their memo.
    pub fn send_payments(
        &self,
        scan: &ScannedWallet,
        outputs: &[PaymentOutput],
        fee: u64,
    ) -> Result<ShieldedTx> {
        ensure!(!outputs.is_empty(), "a transfer needs an output");
        ensure!(
            outputs.len() < genoryn_core::codec::MAX_ACTIONS,
            "a transfer pays at most {} recipients besides its change",
            genoryn_core::codec::MAX_ACTIONS - 1
        );
        ensure!(
            outputs.iter().all(|output| output.amount > 0),
            "amount must be positive"
        );
        ensure!(fee >= MIN_FEE, "fee below consensus minimum");
        let total = outputs
            .iter()
            .try_fold(fee, |sum, output| sum.checked_add(output.amount))
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
        let mut anchor_nullifier: Option<[u8; 32]> = None;
        for owned in candidates.iter().take(MAX_INPUTS) {
            builder.add_spend(
                fvk.clone(),
                owned.note,
                scan.state
                    .witness(owned.position.context("immature note")?)?,
            )?;
            let nullifier = owned.note.nullifier(&fvk).to_bytes();
            anchor_nullifier = Some(anchor_nullifier.map_or(nullifier, |n| n.min(nullifier)));
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
        for (ordinal, output) in outputs.iter().enumerate() {
            let mut memo = output.memo;
            if output.sender_auth {
                sender_auth::commitment(
                    self,
                    &sender_auth::PaymentBinding {
                        anchor_nullifier: anchor_nullifier.context("a payment spends a note")?,
                        recipient: &output.recipient,
                        amount: output.amount,
                        ordinal: ordinal as u16,
                    },
                )?
                .write(&mut memo)?;
            }
            builder.add_output(
                Some(fvk.to_ovk(Scope::External)),
                output.recipient,
                NoteValue::from_raw(output.amount),
                memo,
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
    /// Self-transfers that each merge up to `MAX_INPUTS` spendable notes into one,
    /// smallest notes first, so a later payment needs fewer inputs. Their inputs
    /// are disjoint, so all of them can enter the same block. Returns each
    /// transaction with the number of notes it merges and the value it keeps.
    pub fn combine_notes(
        &self,
        scan: &ScannedWallet,
        fee: u64,
        max_transactions: usize,
    ) -> Result<Vec<(ShieldedTx, usize, u64)>> {
        ensure!(fee >= MIN_FEE, "fee below consensus minimum");
        let mut candidates: Vec<_> = scan
            .notes
            .iter()
            .filter(|n| !n.spent && n.position.is_some())
            .collect();
        ensure!(
            candidates.len() > 1,
            "there is nothing to combine: this wallet has {} spendable note(s)",
            candidates.len()
        );
        candidates.sort_by_key(|n| n.note.value().inner());
        let fvk = self.viewing_key()?;
        let anchor = orchard::Anchor::from(scan.state.tree.root());
        let mut built = Vec::new();
        for group in candidates.chunks(MAX_INPUTS).take(max_transactions) {
            let total = group
                .iter()
                .try_fold(0u64, |sum, n| sum.checked_add(n.note.value().inner()))
                .context("input sum overflow")?;
            if group.len() < 2 || total <= fee {
                continue;
            }
            let mut builder = Builder::new(
                BundleType::DEFAULT,
                BUNDLE_VERSION,
                BUNDLE_VERSION.default_flags(),
                anchor,
            )?;
            for owned in group {
                builder.add_spend(
                    fvk.clone(),
                    owned.note,
                    scan.state
                        .witness(owned.position.context("immature note")?)?,
                )?;
            }
            builder.add_output(
                Some(fvk.to_ovk(Scope::External)),
                fvk.address_at(0u32, Scope::Internal),
                NoteValue::from_raw(total - fee),
                [0; 512],
            )?;
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
            let tx = privacy::build_bundle(
                context,
                builder,
                &[SpendAuthorizingKey::from(&self.spending_key()?)],
            )?;
            built.push((tx, group.len(), total - fee));
        }
        ensure!(!built.is_empty(), "these notes are too small to combine");
        Ok(built)
    }
    /// Whether `address` is one of this wallet's own addresses (any index,
    /// receiving or change).
    pub fn owns_address(&self, address: &Address) -> Result<bool> {
        Ok(self.viewing_key()?.scope_for_address(address).is_some())
    }
}
/// The most one payment can spend: its `MAX_INPUTS` largest notes.
pub fn max_single_payment(mut values: Vec<u64>) -> u64 {
    values.sort_unstable_by(|a, b| b.cmp(a));
    values
        .into_iter()
        .take(MAX_INPUTS)
        .fold(0u64, u64::saturating_add)
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
    /// The proof carries this output's outgoing cipher key, which only the
    /// paying wallet can derive: the sender made this proof. It can be
    /// forwarded, so it is sender-origin evidence, not fresh authentication.
    pub created_by_sender: bool,
    /// The payment-specific sender-verification key committed in this
    /// output's memo (hex), when the proof opens the memo and it carries one.
    pub sender_auth_key: Option<String>,
}

/// What a wallet that made or received a payment can read about one output.
pub struct OutputView {
    pub note: Note,
    pub recipient: Address,
    pub memo: [u8; 512],
}

impl Wallet {
    // A selective proof is built by recovering a note, exposing only its opening.
    // The paying wallet also adds the output's outgoing cipher key (`ock`): it
    // opens only this output's sender ciphertext and is derived from the
    // sender's outgoing viewing key, so a recipient cannot produce it.
    pub fn payment_proof(&self, tx: &ShieldedTx, action: usize) -> Result<NoteProof> {
        let bundle = tx.bundle.to_bundle()?;
        let fvk = self.viewing_key()?;
        let ovk = fvk.to_ovk(Scope::External);
        let (recovered, sender_key) = match bundle.recover_output_with_ovk(action, &ovk) {
            Some(recovered) => {
                let output = bundle.actions().get(action).context("action index")?;
                let ock = <IronwoodDomain as Domain>::derive_ock(
                    &ovk,
                    output.cv_net(),
                    &output.cmx().to_bytes(),
                    &EphemeralKeyBytes(output.encrypted_note().epk_bytes),
                );
                (recovered, Some(ock.0))
            }
            None => (
                bundle
                    .decrypt_output_with_key(action, &fvk.to_ivk(Scope::External))
                    .or_else(|| {
                        bundle.decrypt_output_with_key(action, &fvk.to_ivk(Scope::Internal))
                    })
                    .context("wallet cannot disclose this output")?,
                None,
            ),
        };
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
            sender_key,
        })
    }
    /// The proof of the payment itself in `tx`: the first output this wallet
    /// sent to someone else (never its own change). Without one, an output it
    /// sent to itself, else an output it received.
    pub fn payment_proof_auto(&self, tx: &ShieldedTx) -> Result<NoteProof> {
        let bundle = tx.bundle.to_bundle()?;
        let fvk = self.viewing_key()?;
        let ovk = fvk.to_ovk(Scope::External);
        let count = bundle.actions().len();
        let mut own_external = None;
        for action in 0..count {
            if let Some((_, address, _)) = bundle.recover_output_with_ovk(action, &ovk) {
                match fvk.scope_for_address(&address) {
                    None => return self.payment_proof(tx, action),
                    Some(Scope::External) => {
                        own_external.get_or_insert(action);
                    }
                    Some(Scope::Internal) => {}
                }
            }
        }
        if let Some(action) = own_external {
            return self.payment_proof(tx, action);
        }
        (0..count)
            .find_map(|action| self.payment_proof(tx, action).ok())
            .context("this transaction has no output this wallet can prove")
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
    /// Outgoing cipher key of this output, present when the sender made the proof.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub sender_key: Option<Hash>,
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
    let mut memo = String::new();
    let mut sender_auth_key = None;
    if let Some(ock) = proof.sender_key {
        let bundle = tx.bundle.to_bundle()?;
        let output = bundle
            .actions()
            .get(proof.action as usize)
            .context("payment proof action")?;
        let (sent, recipient, sent_memo) = try_output_recovery_with_ock(
            &IronwoodDomain::for_action(output),
            &OutgoingCipherKey(ock),
            output,
            &output.encrypted_note().out_ciphertext,
        )
        .context("the sender part of this payment proof is not valid")?;
        ensure!(
            recipient == address
                && sent.value().inner() == proof.amount
                && sent.rho().to_bytes() == proof.rho
                && *sent.rseed().as_bytes() == proof.rseed,
            "the sender part of this payment proof does not match the payment"
        );
        memo = decode_memo(&sent_memo);
        sender_auth_key = sender_auth::MemoTrailer::read(&sent_memo).map(|t| hex::encode(t.key));
    }
    Ok(ProvenPayment {
        recipient: proof.recipient.clone(),
        amount_atomic: proof.amount.to_string(),
        memo,
        created_by_sender: proof.sender_key.is_some(),
        sender_auth_key,
    })
}

impl Wallet {
    /// Output `action` of `tx` as its sender sees it (through the outgoing
    /// viewing key): only outputs this wallet paid.
    pub fn sent_output(&self, tx: &ShieldedTx, action: usize) -> Result<Option<OutputView>> {
        let bundle = tx.bundle.to_bundle()?;
        let ovk = self.viewing_key()?.to_ovk(Scope::External);
        Ok(bundle
            .recover_output_with_ovk(action, &ovk)
            .map(|(note, recipient, memo)| OutputView {
                note,
                recipient,
                memo,
            }))
    }
    /// Output `action` of `tx` as its recipient sees it: only outputs paid to
    /// this wallet.
    pub fn received_output(&self, tx: &ShieldedTx, action: usize) -> Result<Option<OutputView>> {
        let bundle = tx.bundle.to_bundle()?;
        let fvk = self.viewing_key()?;
        Ok(bundle
            .decrypt_output_with_key(action, &fvk.to_ivk(Scope::External))
            .or_else(|| bundle.decrypt_output_with_key(action, &fvk.to_ivk(Scope::Internal)))
            .map(|(note, recipient, memo)| OutputView {
                note,
                recipient,
                memo,
            }))
    }
    /// The smallest nullifier among this wallet's own spends in `tx`, given
    /// the nullifiers of every note the wallet holds or held.
    pub fn anchor_nullifier(
        tx: &ShieldedTx,
        own_nullifiers: &std::collections::BTreeSet<[u8; 32]>,
    ) -> Option<[u8; 32]> {
        tx.bundle
            .actions
            .iter()
            .map(|action| action.nullifier)
            .filter(|nullifier| own_nullifiers.contains(nullifier))
            .min()
    }
}

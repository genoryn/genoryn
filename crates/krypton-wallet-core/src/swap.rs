//! GNR side of BTC ⇄ GNR atomic swaps: lock notes (`docs/spec/atomic-swaps.md`).
//!
//! * The GNR seller ([`LockRole::Refunder`]) locks GNR with [`Wallet::lock_funds`]
//!   and can take it back with [`Wallet::refund_lock`] from the refund height on.
//! * The BTC seller ([`LockRole::Claimer`]) registers the agreed terms with
//!   [`Wallet::watch_lock`] before the lock is mined, and takes the GNR with the
//!   secret through [`Wallet::claim_lock`], which publishes the secret.
//!
//! Every output either side may receive is derived from the wallet's swap seed and
//! the swap number, and the lock note's memo carries the rest of the terms to the
//! refunder, so a refund can always be finished from the recovery phrase.

use super::*;
use krypton_core::address::encrypt_note_for;
use krypton_core::lock::{HexCm, LockMode, LockSpend, LockTerms, sha256};
use krypton_core::types::{MEMO_LEN, TX_VERSION_LOCK};

const MEMO_TAG: &[u8; 4] = b"GKL1";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum LockRole {
    /// Locked the GNR; refunds it after the refund height.
    Refunder,
    /// Receives the GNR by revealing the secret.
    Claimer,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct LockRecord {
    pub swap: u32,
    pub role: LockRole,
    pub terms: LockTerms,
    pub value: u64,
    /// Fee of this wallet's own spend of the lock (its claim or its refund).
    pub fee: u64,
    #[serde(with = "hex::serde")]
    pub cm: Hash32,
    /// Height of the block that created the lock note.
    pub height: Option<u64>,
    pub position: Option<u64>,
    pub spent_height: Option<u64>,
    pub pending_spend: Option<String>,
    /// The swap secret, once seen in a claim (in a block or the mempool).
    #[serde(default, skip_serializing_if = "Option::is_none", with = "opt_hex32")]
    pub secret: Option<[u8; 32]>,
}

mod opt_hex32 {
    use serde::{Deserialize, Deserializer, Serializer};
    pub fn serialize<S: Serializer>(v: &Option<[u8; 32]>, s: S) -> Result<S::Ok, S::Error> {
        match v {
            Some(b) => s.serialize_str(&hex::encode(b)),
            None => s.serialize_none(),
        }
    }
    pub fn deserialize<'de, D: Deserializer<'de>>(d: D) -> Result<Option<[u8; 32]>, D::Error> {
        let Some(s) = Option::<String>::deserialize(d)? else {
            return Ok(None);
        };
        let b = hex::decode(s).map_err(serde::de::Error::custom)?;
        b.try_into()
            .map(Some)
            .map_err(|_| serde::de::Error::custom("expected 32 bytes"))
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Kind {
    Claim = 0,
    Refund = 1,
}

fn cms(o: &[OutputWitness; 2]) -> [HexCm; 2] {
    [0, 1].map(|i| HexCm(digest_to_bytes(&o[i].commitment())))
}

impl Wallet {
    /// Seed of this wallet's swap keys (Bitcoin keys and secrets use it too).
    pub fn swap_seed(&self) -> &[u8; 64] {
        &self.keys.swap_seed
    }

    pub fn locks(&self) -> &[LockRecord] {
        &self.state.locks
    }

    /// GNR this wallet has locked in swaps that are neither refunded nor claimed.
    pub fn locked_balance(&self) -> u64 {
        self.state
            .locks
            .iter()
            .filter(|l| l.role == LockRole::Refunder && l.spent_height.is_none())
            .map(|l| l.value)
            .sum()
    }

    fn swap_rcm(&self, swap: u32, n: u8) -> Digest {
        let mut m = Zeroizing::new(Vec::with_capacity(69));
        m.extend_from_slice(self.keys.swap_seed.as_ref());
        m.extend_from_slice(&swap.to_le_bytes());
        m.push(n);
        digest_from_uniform(&krypton_crypto::hash::tagged("Krypton/swap/rcm", &m))
    }

    /// The two outputs this wallet's claim or refund of swap `swap` creates: the value
    /// minus the fee, and a zero-value note, both to itself.
    fn own_outputs(
        &self,
        swap: u32,
        kind: Kind,
        value: u64,
        fee: u64,
    ) -> Result<[OutputWitness; 2]> {
        ensure!(
            (MIN_FEE..=MAX_FEE).contains(&fee),
            "the swap fee must be between {} and {} GNR",
            emission::format_amount(MIN_FEE),
            emission::format_amount(MAX_FEE)
        );
        ensure!(value > fee, "the amount must be larger than the fee");
        let me = self.keys.spend.owner;
        let n = 2 * kind as u8;
        Ok([
            OutputWitness {
                owner: me,
                value: value - fee,
                rcm: self.swap_rcm(swap, n),
            },
            OutputWitness {
                owner: me,
                value: 0,
                rcm: self.swap_rcm(swap, n + 1),
            },
        ])
    }

    /// Outputs a claim must create (the BTC seller sends these to the GNR seller).
    pub fn claim_outputs(&self, swap: u32, value: u64, fee: u64) -> Result<[HexCm; 2]> {
        Ok(cms(&self.own_outputs(swap, Kind::Claim, value, fee)?))
    }

    /// Outputs a refund must create.
    pub fn refund_outputs(&self, swap: u32, value: u64, fee: u64) -> Result<[HexCm; 2]> {
        Ok(cms(&self.own_outputs(swap, Kind::Refund, value, fee)?))
    }

    fn lock_active(&self) -> Result<u64> {
        let tip = self
            .verifier
            .as_ref()
            .context("sync the wallet first")?
            .last
            .height;
        let at = self.network().params().lock_height;
        ensure!(
            tip.saturating_add(1) >= at,
            "atomic swaps are not active on {} yet",
            self.network().as_str()
        );
        Ok(tip)
    }

    /// GNR seller: locks `value` (which includes the claimer's fee) for swap `swap`.
    /// `claim_outputs` come from the BTC seller's wallet. Returns the txid and the
    /// terms to send back to the BTC seller.
    #[allow(clippy::too_many_arguments)]
    pub fn lock_funds(
        &mut self,
        src: &impl ChainSource,
        swap: u32,
        hash: [u8; 32],
        refund_height: u64,
        claim_outputs: [HexCm; 2],
        value: u64,
        refund_fee: u64,
        fee: u64,
    ) -> Result<(Hash32, LockTerms)> {
        let tip = self.lock_active()?;
        ensure!(
            !self
                .state
                .locks
                .iter()
                .any(|l| l.swap == swap && l.role == LockRole::Refunder),
            "swap {swap} already has a lock"
        );
        ensure!(
            refund_height > tip + 20,
            "the refund height must be at least 20 blocks ahead"
        );
        ensure!(
            (MIN_FEE..=MAX_FEE).contains(&fee),
            "the fee must be between {} and {} GNR",
            emission::format_amount(MIN_FEE),
            emission::format_amount(MAX_FEE)
        );
        let refund_outputs = self.refund_outputs(swap, value, refund_fee)?;
        ensure!(
            claim_outputs[0] != claim_outputs[1]
                && !claim_outputs.iter().any(|c| refund_outputs.contains(c)),
            "the claim outputs must be two distinct new commitments"
        );
        let terms = LockTerms {
            hash,
            refund_height,
            claim_outputs,
            refund_outputs,
        };
        let chosen = self.select_notes(value.checked_add(fee).context("amount overflow")?)?;
        let (inputs, root) = self.input_witnesses(src, &chosen)?;
        let change = chosen.iter().map(|n| n.value).sum::<u64>() - value - fee;
        let me = self.address();
        let lock_owner = terms.owner();
        let plain = NotePlain {
            value,
            rcm: terms.rcm(),
            memo: lock_memo(swap, &terms, refund_fee),
        };
        let (cm0, ct0, _) = encrypt_note_for(self.keys.kem.public(), &lock_owner, &plain)?;
        ensure!(cm0 == terms.commitment(value), "lock commitment mismatch");
        let mut b = [0u8; 32];
        rand_core::OsRng.fill_bytes(&mut b);
        let chg = NotePlain {
            value: change,
            rcm: digest_from_uniform(&b),
            memo: memo_from_str("change"),
        };
        let (cm1, ct1) = encrypt_note(&me, &chg)?;
        let mut tx = Transaction {
            version: TX_VERSION,
            kind: TxKind::Transfer,
            anchor: digest_to_bytes(&root),
            nullifiers: [
                HexHash(digest_to_bytes(&inputs[0].nullifier())),
                HexHash(digest_to_bytes(&inputs[1].nullifier())),
            ],
            outputs: [
                Output {
                    cm: cm0,
                    ciphertext: ct0,
                },
                Output {
                    cm: cm1,
                    ciphertext: ct1,
                },
            ],
            fee,
            mint: 0,
            expiry_height: tip + 120,
            lock: None,
            proof: vec![],
        };
        let w = SpendWitness {
            inputs: [inputs[0].clone(), inputs[1].clone()],
            outputs: [
                OutputWitness {
                    owner: lock_owner,
                    value,
                    rcm: plain.rcm,
                },
                OutputWitness {
                    owner: me.owner,
                    value: change,
                    rcm: chg.rcm,
                },
            ],
            root,
            fee,
            mint: 0,
            sighash: tx.sighash(),
        };
        tx.proof = krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?.1;
        let txid = src.submit(&tx)?;
        let id = hex::encode(txid);
        for n in self.state.notes.iter_mut() {
            if chosen.iter().any(|c| c.cm == n.cm) {
                n.pending_spend = Some(id.clone());
            }
        }
        self.state.history.push(HistoryEntry {
            txid: id,
            height: None,
            amount: -(value as i128) - fee as i128,
            fee,
            memo: format!("swap {swap}: GNR locked"),
            to: None,
            time: krypton_core::now(),
            change_cm: Some(hex::encode(cm1)),
            disclosure: None,
            expiry: Some(tx.expiry_height),
        });
        self.state.locks.push(LockRecord {
            swap,
            role: LockRole::Refunder,
            terms: terms.clone(),
            value,
            fee: refund_fee,
            cm: cm0,
            height: None,
            position: None,
            spent_height: None,
            pending_spend: None,
            secret: None,
        });
        self.save()?;
        Ok((txid, terms))
    }

    /// BTC seller: remembers the agreed lock so this wallet finds it on chain. Call this
    /// before the GNR seller locks: the wallet only sees lock notes mined after it.
    pub fn watch_lock(
        &mut self,
        swap: u32,
        terms: LockTerms,
        value: u64,
        claim_fee: u64,
    ) -> Result<Hash32> {
        ensure!(
            terms.claim_outputs == self.claim_outputs(swap, value, claim_fee)?,
            "these terms do not pay this wallet's claim outputs"
        );
        let cm = terms.commitment(value);
        if let Some(l) = self.state.locks.iter().find(|l| l.cm == cm) {
            ensure!(
                l.role == LockRole::Claimer && l.swap == swap,
                "this lock is already known"
            );
            return Ok(cm);
        }
        self.state.locks.push(LockRecord {
            swap,
            role: LockRole::Claimer,
            terms,
            value,
            fee: claim_fee,
            cm,
            height: None,
            position: None,
            spent_height: None,
            pending_spend: None,
            secret: None,
        });
        self.save()?;
        Ok(cm)
    }

    /// BTC seller: takes the GNR with the secret (which becomes public).
    pub fn claim_lock(
        &mut self,
        src: &impl ChainSource,
        cm: &Hash32,
        secret: [u8; 32],
    ) -> Result<Hash32> {
        self.spend_lock(src, cm, LockMode::Claim { secret })
    }

    /// GNR seller: takes the GNR back, from the refund height on.
    pub fn refund_lock(&mut self, src: &impl ChainSource, cm: &Hash32) -> Result<Hash32> {
        self.spend_lock(src, cm, LockMode::Refund)
    }

    fn spend_lock(
        &mut self,
        src: &impl ChainSource,
        cm: &Hash32,
        mode: LockMode,
    ) -> Result<Hash32> {
        let tip = self.lock_active()?;
        let rec = self
            .state
            .locks
            .iter()
            .find(|l| l.cm == *cm)
            .cloned()
            .context("no such lock in this wallet")?;
        ensure!(
            rec.spent_height.is_none(),
            "this lock has already been spent"
        );
        ensure!(
            rec.pending_spend.is_none(),
            "a spend of this lock is already pending"
        );
        let pos = rec.position.context("the lock is not in a block yet")?;
        let t = &rec.terms;
        let (kind, expiry) = match &mode {
            LockMode::Claim { secret } => {
                ensure!(
                    rec.role == LockRole::Claimer,
                    "only the claimer can claim this lock"
                );
                ensure!(
                    sha256(secret) == t.hash,
                    "this secret does not open the lock"
                );
                ensure!(
                    tip + 1 < t.refund_height,
                    "too late to claim: the lock refunds from block {}",
                    t.refund_height
                );
                (Kind::Claim, (tip + 120).min(t.refund_height - 1))
            }
            LockMode::Refund => {
                ensure!(
                    rec.role == LockRole::Refunder,
                    "only the refunder can refund this lock"
                );
                ensure!(
                    tip + 1 >= t.refund_height,
                    "the refund is possible from block {} (now {})",
                    t.refund_height,
                    tip
                );
                (Kind::Refund, tip + 120)
            }
        };
        let (path, root) = src.witness(pos)?;
        ensure!(
            digest_to_bytes(&root) == self.verifier.as_ref().expect("synced").last.note_root,
            "the node's tree root differs from our verified chain; sync and retry"
        );
        let input = InputWitness {
            sk: t.keys().sk,
            value: rec.value,
            rcm: t.rcm(),
            pos,
            path,
            dummy: false,
            lock: true,
        };
        ensure!(
            digest_to_bytes(&input.commitment()) == rec.cm,
            "lock commitment mismatch"
        );
        let outs = self.own_outputs(rec.swap, kind, rec.value, rec.fee)?;
        let pinned = match kind {
            Kind::Claim => &t.claim_outputs,
            Kind::Refund => &t.refund_outputs,
        };
        ensure!(
            cms(&outs) == *pinned,
            "the lock pays other outputs than this wallet's"
        );
        let me = self.address();
        let label = match kind {
            Kind::Claim => format!("swap {}: GNR received", rec.swap),
            Kind::Refund => format!("swap {}: GNR refunded", rec.swap),
        };
        let (cm0, ct0) = encrypt_note(
            &me,
            &NotePlain {
                value: outs[0].value,
                rcm: outs[0].rcm,
                memo: memo_from_str(&label),
            },
        )?;
        let (cm1, ct1) = encrypt_note(
            &me,
            &NotePlain {
                value: 0,
                rcm: outs[1].rcm,
                memo: memo_from_str(""),
            },
        )?;
        let dummy = InputWitness::dummy();
        let mut tx = Transaction {
            version: TX_VERSION_LOCK,
            kind: TxKind::Transfer,
            anchor: digest_to_bytes(&root),
            nullifiers: [
                HexHash(digest_to_bytes(&input.nullifier())),
                HexHash(digest_to_bytes(&dummy.nullifier())),
            ],
            outputs: [
                Output {
                    cm: cm0,
                    ciphertext: ct0,
                },
                Output {
                    cm: cm1,
                    ciphertext: ct1,
                },
            ],
            fee: rec.fee,
            mint: 0,
            expiry_height: expiry,
            lock: Some(LockSpend {
                terms: t.clone(),
                mode,
            }),
            proof: vec![],
        };
        let w = SpendWitness {
            inputs: [input, dummy],
            outputs: outs,
            root,
            fee: rec.fee,
            mint: 0,
            sighash: tx.sighash(),
        };
        tx.proof = krypton_zk::prove(&w).map_err(|e| anyhow::anyhow!("{e}"))?.1;
        let txid = src.submit(&tx)?;
        let id = hex::encode(txid);
        if let Some(l) = self.state.locks.iter_mut().find(|l| l.cm == *cm) {
            l.pending_spend = Some(id.clone());
        }
        self.state.history.push(HistoryEntry {
            txid: id,
            height: None,
            amount: (rec.value - rec.fee) as i128,
            fee: rec.fee,
            memo: label,
            to: None,
            time: krypton_core::now(),
            change_cm: Some(hex::encode(cm0)),
            disclosure: None,
            expiry: Some(expiry),
        });
        self.save()?;
        Ok(txid)
    }

    /// Picks up swap secrets from claims of our locks (mempool transactions or the
    /// transactions of a block). Returns how many secrets were new.
    pub fn learn_secrets(&mut self, txs: &[Transaction]) -> usize {
        let mut new = 0;
        for tx in txs {
            let Some(LockSpend {
                terms,
                mode: LockMode::Claim { secret },
            }) = &tx.lock
            else {
                continue;
            };
            if sha256(secret) != terms.hash {
                continue;
            }
            for l in self.state.locks.iter_mut() {
                if l.terms.hash == terms.hash && l.secret.is_none() {
                    l.secret = Some(*secret);
                    new += 1;
                }
            }
        }
        new
    }

    /// A created lock note was mined.
    pub(crate) fn lock_mined(&mut self, cm: &Hash32, height: u64) {
        if let Some(l) = self
            .state
            .locks
            .iter_mut()
            .find(|l| l.cm == *cm && l.height.is_none())
        {
            l.height = Some(height);
        }
    }

    pub(crate) fn lock_position(&mut self, cm: &Hash32, pos: u64) {
        if let Some(l) = self
            .state
            .locks
            .iter_mut()
            .find(|l| l.cm == *cm && l.position.is_none())
        {
            l.position = Some(pos);
        }
    }

    /// Marks locks spent by `nullifiers`; returns how many.
    pub(crate) fn lock_spent(&mut self, nullifiers: &[Hash32], height: u64) -> usize {
        let mut n = 0;
        for l in self
            .state
            .locks
            .iter_mut()
            .filter(|l| l.spent_height.is_none())
        {
            if let Some(p) = l.position
                && nullifiers.contains(&l.terms.nullifier(p))
            {
                l.spent_height = Some(height);
                l.pending_spend = None;
                n += 1;
            }
        }
        n
    }

    /// Forgets chain facts about locks above `fork` (all of them if `all`).
    pub(crate) fn lock_rollback(&mut self, fork: u64, tree_size: u64, all: bool) {
        for l in self.state.locks.iter_mut() {
            if all || l.height.is_some_and(|h| h > fork) {
                l.height = None;
            }
            if all || l.position.is_some_and(|p| p >= tree_size) {
                l.position = None;
            }
            if all || l.spent_height.is_some_and(|h| h > fork) {
                l.spent_height = None;
            }
        }
    }

    /// A note encrypted to us under another owner: our own lock note if its memo
    /// carries terms that rebuild exactly this commitment.
    pub(crate) fn recover_lock(&mut self, note: &NotePlain, cm: &Hash32, height: u64) {
        let m = &note.memo;
        if &m[..4] != MEMO_TAG || self.state.locks.iter().any(|l| l.cm == *cm) {
            return;
        }
        let u32_at = |i: usize| u32::from_le_bytes(m[i..i + 4].try_into().expect("4"));
        let u64_at = |i: usize| u64::from_le_bytes(m[i..i + 8].try_into().expect("8"));
        let h32 = |i: usize| -> [u8; 32] { m[i..i + 32].try_into().expect("32") };
        let swap = u32_at(4);
        let refund_fee = u64_at(16);
        let Ok(refund_outputs) = self.refund_outputs(swap, note.value, refund_fee) else {
            return;
        };
        let terms = LockTerms {
            hash: h32(24),
            refund_height: u64_at(8),
            claim_outputs: [HexCm(h32(56)), HexCm(h32(88))],
            refund_outputs,
        };
        if terms.commitment(note.value) != *cm || note.rcm != terms.rcm() {
            return;
        }
        self.state.locks.push(LockRecord {
            swap,
            role: LockRole::Refunder,
            terms,
            value: note.value,
            fee: refund_fee,
            cm: *cm,
            height: Some(height),
            position: None,
            spent_height: None,
            pending_spend: None,
            secret: None,
        });
    }
}

/// Memo of a lock note (120 of 128 bytes): tag, swap number, refund height, refund
/// fee, hash, claim outputs. The refund outputs follow from the seed.
fn lock_memo(swap: u32, t: &LockTerms, refund_fee: u64) -> [u8; MEMO_LEN] {
    let mut m = [0u8; MEMO_LEN];
    m[..4].copy_from_slice(MEMO_TAG);
    m[4..8].copy_from_slice(&swap.to_le_bytes());
    m[8..16].copy_from_slice(&t.refund_height.to_le_bytes());
    m[16..24].copy_from_slice(&refund_fee.to_le_bytes());
    m[24..56].copy_from_slice(&t.hash);
    m[56..88].copy_from_slice(&t.claim_outputs[0].0);
    m[88..120].copy_from_slice(&t.claim_outputs[1].0);
    m
}

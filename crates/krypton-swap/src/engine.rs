//! The swap protocol between two wallets and the steps each side takes on its own.
//!
//! **GNR first** (from 1.0.23, flow 2): the GNR is locked before anyone pays, so a buyer
//! never sends bitcoin to someone who does not have the GNR. Three codes pass between the
//! two people (copy and paste, or the offer board):
//!
//! 1. **Request** (buyer → GNR seller): amounts, the buyer's bitcoin refund key and its
//!    GNR claim outputs.
//! 2. **Accept** (GNR seller → buyer): the hash lock (the seller holds the secret), the
//!    seller's bitcoin claim key and the full GNR lock terms. The seller's wallet locks
//!    the GNR at once; the buyer's wallet knows the lock note and spots it on chain.
//! 3. **Funded** (buyer → GNR seller): sent once the buyer has paid. The buyer's wallet
//!    shows where to pay only after the lock has confirmed with exactly the agreed amount,
//!    payable only to the buyer, and lasting long enough.
//!
//! Then the seller takes the bitcoin with the secret, which the buyer's wallet reads from
//! that bitcoin transaction to claim the GNR. Timeouts refund both: the bitcoin after
//! about 6 hours, the GNR after about 32. No swap may last longer than a week.
//!
//! Swaps started before 1.0.23 (flow 0: bitcoin first, the buyer holds the secret) finish
//! as they began; new ones are never started that way.

use crate::btc::BtcChain;
use crate::{
    Funding, Htlc, MAX_SWAP_SECS, Secret, Timelocks, Timing, derive_key, derive_secret, public_key,
};
use anyhow::{Context, Result, anyhow, bail, ensure};
use bitcoin::{Address, Amount, OutPoint, PublicKey, Txid};
use krypton_core::Network;
use krypton_core::lock::{HexCm, LockTerms};
use krypton_core::types::Transaction;
use krypton_wallet_core::swap::LockRecord;
use krypton_wallet_core::{ChainSource, DEFAULT_FEE, Wallet};
use serde::{Deserialize, Serialize};
use std::str::FromStr;

pub const CODE_PREFIX: &str = "gks1";
const STORE_KEY: &str = "swaps";
/// A swap whose bitcoin deposit has not arrived this long after it started is given up.
pub const DEPOSIT_TIMEOUT_SECS: u64 = 3 * 3600;
/// The GNR-first flow.
pub const FLOW_GNR_FIRST: u8 = 2;
/// Wallets older than this answer only the bitcoin-first flow.
pub const UPDATE_HINT: &str = "this swap code comes from a wallet older than 1.0.23; ask the other side to update (now the GNR is locked first, so nobody pays before the coins are there)";

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Role {
    BtcSeller,
    GnrSeller,
}

#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "snake_case")]
pub enum Stage {
    /// BTC seller: request sent, waiting for the accept code.
    Requested,
    /// GNR seller: accept sent, waiting for the funded code.
    Accepted,
    /// BTC seller: waiting for the bitcoin deposit into the HTLC address.
    AwaitDeposit,
    /// Deposit made: the GNR seller waits for its confirmations, the BTC seller for the lock.
    Deposited,
    /// GNR locked: the BTC seller waits for confirmations, the GNR seller for the secret.
    Locked,
    /// Our claim was sent (GNR claim, or bitcoin claim with the secret).
    Claiming,
    Done,
    /// Our refund was sent (GNR or bitcoin).
    Refunding,
    Refunded,
    /// Stopped before anything of ours was locked.
    Failed,
}

impl Stage {
    pub fn finished(self) -> bool {
        matches!(self, Stage::Done | Stage::Refunded | Stage::Failed)
    }
}

impl Swap {
    /// Whether this side can still walk away: nothing of its own is locked yet.
    pub fn cancellable(&self) -> bool {
        match (self.role, self.flow) {
            (Role::BtcSeller, FLOW_GNR_FIRST) => matches!(
                self.stage,
                Stage::Requested | Stage::Accepted | Stage::AwaitDeposit
            ),
            (Role::GnrSeller, FLOW_GNR_FIRST) => {
                self.stage == Stage::Accepted && self.lock_cm.is_none()
            }
            (Role::BtcSeller, _) => matches!(self.stage, Stage::Requested | Stage::AwaitDeposit),
            (Role::GnrSeller, _) => self.stage == Stage::Accepted,
        }
    }
    /// Started in the GNR-first flow.
    pub fn gnr_first(&self) -> bool {
        self.flow == FLOW_GNR_FIRST
    }
    /// Ended (cancelled or expired) before this side locked anything: no bitcoin deposit
    /// (BTC seller), no GNR lock (GNR seller).
    pub fn ended_unlocked(&self) -> bool {
        self.stage == Stage::Failed
            && match self.role {
                Role::BtcSeller => self.deposit.is_none(),
                Role::GnrSeller => self.lock_cm.is_none(),
            }
    }
    /// A given-up BTC seller swap whose contract address is still watched, in case the
    /// bitcoin is paid anyway: it is then refunded at the timelock.
    fn watching(&self) -> bool {
        self.role == Role::BtcSeller
            && self.stage == Stage::Failed
            && self.deposit.is_none()
            && self.btc_refund_height.is_some()
            && self.btc_claim_pub.is_some()
    }
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
pub struct Deposit {
    pub txid: String,
    pub vout: u32,
    pub sats: u64,
    /// The bitcoin block that confirmed it, once known.
    #[serde(default)]
    pub height: Option<u32>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Swap {
    pub id: String,
    /// Swap number in this wallet: derives its keys, secret and outputs.
    pub index: u32,
    pub role: Role,
    pub gnr_network: Network,
    /// Bitcoin network name (`bitcoin`, `signet`, `regtest`, ...).
    pub btc_network: String,
    /// GNR in the lock (gori), including the claim fee.
    pub gnr: u64,
    pub sats: u64,
    #[serde(with = "hex::serde")]
    pub hash: [u8; 32],
    pub claim_fee: u64,
    pub claim_outputs: [HexCm; 2],
    pub btc_refund_pub: String,
    pub btc_claim_pub: Option<String>,
    pub gnr_refund_height: Option<u64>,
    pub refund_outputs: Option<[HexCm; 2]>,
    pub refund_fee: Option<u64>,
    pub btc_refund_height: Option<u32>,
    pub deposit: Option<Deposit>,
    /// This wallet's bitcoin address: payout (GNR seller) or refund (BTC seller).
    pub btc_address: String,
    pub stage: Stage,
    /// The latest code to give the other side.
    pub code: Option<String>,
    pub lock_cm: Option<String>,
    /// Our bitcoin claim or refund.
    pub btc_spend: Option<String>,
    /// What is happening now, in one sentence.
    pub status: String,
    pub log: Vec<(u64, String)>,
    pub created: u64,
    /// 0: bitcoin first (before 1.0.23); 2: GNR first.
    #[serde(default)]
    pub flow: u8,
    /// GNR-first buyer: pay before this time (unix seconds), or the swap ends unpaid.
    #[serde(default)]
    pub pay_by: Option<u64>,
    /// GNR-first buyer: the secret, read from the seller's bitcoin claim.
    #[serde(default)]
    pub learned: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
    /// GNR first: the buyer asks; no hash lock yet (the seller holds the secret).
    Request2 {
        id: String,
        gnr_network: Network,
        btc_network: String,
        gnr: u64,
        sats: u64,
        claim_fee: u64,
        claim_outputs: [HexCm; 2],
        refund_pub: String,
    },
    /// GNR first: the seller's hash lock, bitcoin claim key and GNR lock terms.
    Accept2 {
        id: String,
        #[serde(with = "hex::serde")]
        hash: [u8; 32],
        claim_pub: String,
        gnr_refund_height: u64,
        refund_outputs: [HexCm; 2],
        refund_fee: u64,
    },
    Request {
        id: String,
        gnr_network: Network,
        btc_network: String,
        gnr: u64,
        sats: u64,
        #[serde(with = "hex::serde")]
        hash: [u8; 32],
        claim_fee: u64,
        claim_outputs: [HexCm; 2],
        refund_pub: String,
    },
    Accept {
        id: String,
        claim_pub: String,
        gnr_refund_height: u64,
        refund_outputs: [HexCm; 2],
        refund_fee: u64,
    },
    Funded {
        id: String,
        btc_refund_height: u32,
        txid: String,
        vout: u32,
    },
}

impl Msg {
    pub fn encode(&self) -> String {
        let json = serde_json::to_vec(self).expect("serializable");
        format!(
            "{CODE_PREFIX}{}",
            krypton_core::address::base32_encode(&json)
        )
    }
    pub fn decode(code: &str) -> Result<Self> {
        let c: String = code.split_whitespace().collect::<String>().to_lowercase();
        let body = c
            .strip_prefix(CODE_PREFIX)
            .context("this is not a Genoryn swap code (it starts with gks1)")?;
        let json =
            krypton_core::address::base32_decode(body).context("the swap code is damaged")?;
        serde_json::from_slice(&json).context("the swap code is damaged or from a newer wallet")
    }
    pub fn id(&self) -> &str {
        match self {
            Msg::Request { id, .. }
            | Msg::Accept { id, .. }
            | Msg::Funded { id, .. }
            | Msg::Request2 { id, .. }
            | Msg::Accept2 { id, .. } => id,
        }
    }
}

pub fn load(w: &Wallet) -> Vec<Swap> {
    w.app_data(STORE_KEY)
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default()
}

pub fn save(w: &mut Wallet, swaps: &[Swap]) -> Result<()> {
    w.set_app_data(STORE_KEY, serde_json::to_value(swaps)?)
}

fn now() -> u64 {
    krypton_core::now()
}

fn parse_net(s: &str) -> Result<bitcoin::Network> {
    bitcoin::Network::from_core_arg(s)
        .or_else(|_| bitcoin::Network::from_str(s))
        .map_err(|_| anyhow!("unknown Bitcoin network {s}"))
}

fn net_name(n: bitcoin::Network) -> String {
    n.to_core_arg().to_string()
}

pub fn btc_address(s: &str, n: bitcoin::Network) -> Result<Address> {
    Address::from_str(s.trim())
        .map_err(|e| anyhow!("not a Bitcoin address: {e}"))?
        .require_network(n)
        .map_err(|_| {
            anyhow!(
                "this Bitcoin address is for another network than {}",
                net_name(n)
            )
        })
}

impl Swap {
    fn say(&mut self, msg: impl Into<String>) {
        let m = msg.into();
        if self.status != m {
            self.log.push((now(), m.clone()));
            self.status = m;
        }
    }
    fn timelocks(&self) -> Option<Timelocks> {
        Some(Timelocks {
            btc_refund_height: self.btc_refund_height?,
            gnr_refund_height: self.gnr_refund_height?,
        })
    }
    pub fn terms(&self) -> Option<LockTerms> {
        Some(LockTerms {
            hash: self.hash,
            refund_height: self.gnr_refund_height?,
            claim_outputs: self.claim_outputs,
            refund_outputs: self.refund_outputs?,
        })
    }
    /// The bitcoin contract, once both keys and the refund height are known.
    pub fn htlc(&self) -> Result<Htlc> {
        let claim =
            PublicKey::from_str(self.btc_claim_pub.as_deref().context("no claim key yet")?)?;
        let refund = PublicKey::from_str(&self.btc_refund_pub)?;
        Ok(Htlc::new(
            self.hash,
            claim,
            refund,
            self.btc_refund_height.context("no refund height yet")?,
        )?)
    }
    pub fn htlc_address(&self) -> Result<Address> {
        Ok(self.htlc()?.address(parse_net(&self.btc_network)?))
    }
    fn funding(&self) -> Result<Funding> {
        let d = self.deposit.as_ref().context("no deposit yet")?;
        Ok(Funding {
            outpoint: OutPoint::new(Txid::from_str(&d.txid)?, d.vout),
            value: Amount::from_sat(d.sats),
        })
    }
}

/// One wallet's view of its swaps, with the GNR chain and a Bitcoin backend.
pub struct Engine<'a, G: ChainSource> {
    pub wallet: &'a mut Wallet,
    pub gnr: &'a G,
    pub btc: &'a dyn BtcChain,
    pub btc_network: bitcoin::Network,
    pub timing: Timing,
    /// Transactions of a GNR block (to read a secret from a claim that never passed
    /// through our mempool view).
    pub block_txs: &'a dyn Fn(u64) -> Result<Vec<Transaction>>,
}

impl<G: ChainSource> Engine<'_, G> {
    fn gnr_tip(&self) -> Result<u64> {
        self.wallet.synced_height().context("sync the wallet first")
    }
    fn check_active(&self) -> Result<()> {
        let p = self.wallet.network().params();
        ensure!(
            self.gnr_tip()?.saturating_add(1) >= p.lock_height,
            "atomic swaps are not active on {} yet: they need the network upgrade",
            self.wallet.network().as_str()
        );
        Ok(())
    }
    /// A new swap number. Its secret, keys and outputs all derive from it and the wallet
    /// seed, so it must never repeat, even in a wallet restored from its recovery phrase
    /// (which has no swap records) or the same phrase in a second wallet: a repeated
    /// secret already revealed by an earlier swap would let anyone claim the new lock.
    /// Random numbers from the upper half never meet the counted ones of older versions.
    fn next_index(swaps: &[Swap]) -> u32 {
        new_index(swaps.iter().map(|s| s.index))
    }

    fn lock(&self, s: &Swap) -> Option<LockRecord> {
        let cm = s.lock_cm.as_deref()?;
        self.wallet
            .locks()
            .iter()
            .find(|l| hex::encode(l.cm) == cm)
            .cloned()
    }

    /// BTC seller: starts a swap of `sats` bitcoin for `gnr` GNR (gori). Refunds go to
    /// `refund_address`. Returns the request code for the GNR seller.
    pub fn request(&mut self, gnr: u64, sats: u64, refund_address: &str) -> Result<Swap> {
        self.check_active()?;
        btc_address(refund_address, self.btc_network)?;
        ensure!(sats >= 10_000, "swap at least 10,000 sat");
        ensure!(gnr > 2 * DEFAULT_FEE, "the GNR amount is too small");
        let mut swaps = load(self.wallet);
        let index = Self::next_index(&swaps);
        let seed = *self.wallet.swap_seed();
        let refund_pub = public_key(&derive_key(&seed, index));
        let claim_fee = DEFAULT_FEE;
        let claim_outputs = self.wallet.claim_outputs(index, gnr, claim_fee)?;
        let id = hex::encode(rand::random::<[u8; 16]>());
        let msg = Msg::Request2 {
            id: id.clone(),
            gnr_network: self.wallet.network(),
            btc_network: net_name(self.btc_network),
            gnr,
            sats,
            claim_fee,
            claim_outputs,
            refund_pub: refund_pub.to_string(),
        };
        let mut s = Swap {
            id,
            index,
            role: Role::BtcSeller,
            gnr_network: self.wallet.network(),
            btc_network: net_name(self.btc_network),
            gnr,
            sats,
            hash: [0; 32],
            claim_fee,
            claim_outputs,
            btc_refund_pub: refund_pub.to_string(),
            btc_claim_pub: None,
            gnr_refund_height: None,
            refund_outputs: None,
            refund_fee: None,
            btc_refund_height: None,
            deposit: None,
            btc_address: refund_address.trim().to_string(),
            stage: Stage::Requested,
            code: Some(msg.encode()),
            lock_cm: None,
            btc_spend: None,
            status: String::new(),
            log: vec![],
            created: now(),
            flow: FLOW_GNR_FIRST,
            pay_by: None,
            learned: None,
        };
        s.say("Send the request code to the GNR seller and paste their answer here. You pay nothing until their GNR is locked for you.");
        swaps.push(s.clone());
        save(self.wallet, &swaps)?;
        Ok(s)
    }

    /// GNR seller: accepts a request; the bitcoin will be paid to `payout_address`.
    /// Returns the swap with the accept code for the BTC seller.
    pub fn accept(&mut self, code: &str, payout_address: &str) -> Result<Swap> {
        self.check_active()?;
        let (id, gnr_network, btc_network, gnr, sats, claim_fee, claim_outputs, refund_pub) =
            match Msg::decode(code)? {
                Msg::Request2 {
                    id,
                    gnr_network,
                    btc_network,
                    gnr,
                    sats,
                    claim_fee,
                    claim_outputs,
                    refund_pub,
                } => (
                    id,
                    gnr_network,
                    btc_network,
                    gnr,
                    sats,
                    claim_fee,
                    claim_outputs,
                    refund_pub,
                ),
                Msg::Request { .. } => bail!(UPDATE_HINT),
                _ => bail!("this is not a swap request"),
            };
        ensure!(sats >= 10_000, "a swap of at least 10,000 sat");
        ensure!(gnr > 2 * DEFAULT_FEE, "the GNR amount is too small");
        ensure!(
            gnr_network == self.wallet.network(),
            "the request is for GNR {}",
            gnr_network.as_str()
        );
        ensure!(
            parse_net(&btc_network)? == self.btc_network,
            "the request is for Bitcoin {btc_network}, this wallet uses {}",
            net_name(self.btc_network)
        );
        btc_address(payout_address, self.btc_network)?;
        PublicKey::from_str(&refund_pub).context("bad refund key in the request")?;
        ensure!(
            (krypton_wallet_core::MIN_FEE..=krypton_wallet_core::MAX_FEE).contains(&claim_fee),
            "unusual claim fee in the request"
        );
        let mut swaps = load(self.wallet);
        ensure!(
            !swaps.iter().any(|s| s.id == id),
            "this swap is already in the wallet"
        );
        let spendable = self.wallet.balance().spendable;
        ensure!(
            spendable >= gnr + DEFAULT_FEE,
            "this wallet has {} GNR spendable; the swap needs {}",
            krypton_core::emission::format_amount(spendable),
            krypton_core::emission::format_amount(gnr + DEFAULT_FEE)
        );
        let index = Self::next_index(&swaps);
        let seed = *self.wallet.swap_seed();
        let claim_pub = public_key(&derive_key(&seed, index));
        let refund_fee = DEFAULT_FEE;
        let refund_outputs = self.wallet.refund_outputs(index, gnr, refund_fee)?;
        ensure!(
            claim_outputs[0] != claim_outputs[1]
                && !claim_outputs.iter().any(|c| refund_outputs.contains(c)),
            "the request's claim outputs are invalid"
        );
        // We hold the secret and lock first; the lock lasts through the buyer's payment,
        // the bitcoin lock and the buyer's window to claim.
        let hash = derive_secret(&seed, index).hash();
        let gnr_refund_height = self.gnr_tip()? + self.timing.gnr_lock2_blocks;
        let msg = Msg::Accept2 {
            id: id.clone(),
            hash,
            claim_pub: claim_pub.to_string(),
            gnr_refund_height,
            refund_outputs,
            refund_fee,
        };
        let mut s = Swap {
            id,
            index,
            role: Role::GnrSeller,
            gnr_network,
            btc_network,
            gnr,
            sats,
            hash,
            claim_fee,
            claim_outputs,
            btc_refund_pub: refund_pub,
            btc_claim_pub: Some(claim_pub.to_string()),
            gnr_refund_height: Some(gnr_refund_height),
            refund_outputs: Some(refund_outputs),
            refund_fee: Some(refund_fee),
            btc_refund_height: None,
            deposit: None,
            btc_address: payout_address.trim().to_string(),
            stage: Stage::Accepted,
            code: Some(msg.encode()),
            lock_cm: None,
            btc_spend: None,
            status: String::new(),
            log: vec![],
            created: now(),
            flow: FLOW_GNR_FIRST,
            pay_by: None,
            learned: None,
        };
        s.say("Locking the GNR for the buyer. Send them the accept code; they pay once the lock has confirmed.");
        swaps.push(s.clone());
        save(self.wallet, &swaps)?;
        Ok(s)
    }

    /// Takes the other side's next code (accept or funded).
    pub fn receive(&mut self, code: &str) -> Result<Swap> {
        let msg = Msg::decode(code)?;
        if matches!(msg, Msg::Request { .. } | Msg::Request2 { .. }) {
            bail!("REQUEST: this is a new swap request; accept it to sell GNR");
        }
        let mut swaps = load(self.wallet);
        let i = swaps
            .iter()
            .position(|s| s.id == msg.id())
            .context("no swap in this wallet matches the code")?;
        let mut s = swaps[i].clone();
        match msg {
            Msg::Accept2 {
                hash,
                claim_pub,
                gnr_refund_height,
                refund_outputs,
                refund_fee,
                ..
            } => {
                ensure!(
                    s.role == Role::BtcSeller && s.gnr_first() && s.stage == Stage::Requested,
                    "this swap does not expect an accept code"
                );
                PublicKey::from_str(&claim_pub).context("bad claim key")?;
                ensure!(hash != [0; 32], "bad hash lock");
                let gnr_tip = self.gnr_tip()?;
                let t = self.timing;
                ensure!(
                    gnr_refund_height >= gnr_tip + t.gnr_lock2_blocks * 9 / 10,
                    "the GNR lock in the accept code expires too soon"
                );
                ensure!(
                    (gnr_refund_height - gnr_tip) * 60 <= MAX_SWAP_SECS,
                    "the GNR lock in the accept code lasts longer than a week"
                );
                ensure!(
                    (krypton_wallet_core::MIN_FEE..=krypton_wallet_core::MAX_FEE)
                        .contains(&refund_fee),
                    "unusual refund fee in the accept code"
                );
                ensure!(
                    !refund_outputs.iter().any(|c| s.claim_outputs.contains(c)),
                    "the accept code's refund outputs are invalid"
                );
                let terms = LockTerms {
                    hash,
                    refund_height: gnr_refund_height,
                    claim_outputs: s.claim_outputs,
                    refund_outputs,
                };
                // Only a lock with exactly this amount, payable to our outputs, matches.
                let cm = self.wallet.watch_lock(s.index, terms, s.gnr, s.claim_fee)?;
                s.hash = hash;
                s.btc_claim_pub = Some(claim_pub);
                s.gnr_refund_height = Some(gnr_refund_height);
                s.refund_outputs = Some(refund_outputs);
                s.refund_fee = Some(refund_fee);
                s.lock_cm = Some(hex::encode(cm));
                s.stage = Stage::Accepted;
                s.code = None;
                s.say("Waiting for the seller to lock the GNR for you. You pay nothing before it is locked on chain.");
            }
            Msg::Accept { .. } if s.gnr_first() => bail!(UPDATE_HINT),
            Msg::Funded {
                btc_refund_height,
                txid,
                vout,
                ..
            } if s.gnr_first() => {
                ensure!(
                    s.role == Role::GnrSeller && matches!(s.stage, Stage::Accepted | Stage::Locked),
                    "this swap does not expect a deposit code"
                );
                Txid::from_str(&txid).context("bad deposit txid")?;
                let btc_tip = self.btc.tip()?;
                ensure!(
                    u64::from(btc_refund_height.saturating_sub(btc_tip)) * 600 <= MAX_SWAP_SECS,
                    "the bitcoin lock lasts longer than a week"
                );
                s.btc_refund_height = Some(btc_refund_height);
                s.htlc()?;
                s.deposit = Some(Deposit {
                    txid,
                    vout,
                    sats: s.sats,
                    height: None,
                });
                s.stage = Stage::Deposited;
                s.code = None;
                s.say("The buyer paid. Checking the bitcoin deposit.");
            }
            Msg::Accept {
                claim_pub,
                gnr_refund_height,
                refund_outputs,
                refund_fee,
                ..
            } => {
                ensure!(
                    s.role == Role::BtcSeller && s.stage == Stage::Requested,
                    "this swap does not expect an accept code"
                );
                PublicKey::from_str(&claim_pub).context("bad claim key")?;
                let gnr_tip = self.gnr_tip()?;
                let t = self.timing;
                ensure!(
                    gnr_refund_height
                        > gnr_tip + t.gnr_confirmations + t.claim_margin + t.gnr_lock_blocks / 2,
                    "the GNR lock in the accept code expires too soon"
                );
                ensure!(
                    gnr_refund_height <= gnr_tip + t.gnr_lock_blocks * 2,
                    "the GNR lock in the accept code lasts unusually long"
                );
                let terms = LockTerms {
                    hash: s.hash,
                    refund_height: gnr_refund_height,
                    claim_outputs: s.claim_outputs,
                    refund_outputs,
                };
                let cm = self.wallet.watch_lock(s.index, terms, s.gnr, s.claim_fee)?;
                // The bitcoin lock must outlast the GNR lock by the safety window.
                let btc_tip = self.btc.tip()?;
                let gnr_left = (gnr_refund_height - gnr_tip) * t.gnr_slow_secs + t.min_window_secs;
                let need = (gnr_left / t.btc_fast_secs + 1) as u32;
                let btc_refund_height =
                    btc_tip + t.btc_lock_blocks.max(need + t.btc_confirmations + 2);
                s.btc_claim_pub = Some(claim_pub);
                s.gnr_refund_height = Some(gnr_refund_height);
                s.refund_outputs = Some(refund_outputs);
                s.refund_fee = Some(refund_fee);
                s.btc_refund_height = Some(btc_refund_height);
                s.lock_cm = Some(hex::encode(cm));
                t.check(&s.timelocks().expect("set"), btc_tip, gnr_tip)?;
                s.stage = Stage::AwaitDeposit;
                s.code = None;
                let addr = s.htlc_address()?;
                s.say(format!(
                    "Pay exactly {} BTC to {addr} from any Bitcoin wallet.",
                    Amount::from_sat(s.sats).to_btc()
                ));
            }
            Msg::Funded {
                btc_refund_height,
                txid,
                vout,
                ..
            } => {
                ensure!(
                    s.role == Role::GnrSeller && s.stage == Stage::Accepted,
                    "this swap does not expect a deposit code"
                );
                Txid::from_str(&txid).context("bad deposit txid")?;
                s.btc_refund_height = Some(btc_refund_height);
                s.htlc()?;
                s.deposit = Some(Deposit {
                    txid,
                    vout,
                    sats: s.sats,
                    height: None,
                });
                s.stage = Stage::Deposited;
                s.code = None;
                s.say("Checking the bitcoin deposit.");
            }
            Msg::Request { .. } | Msg::Request2 { .. } => unreachable!("handled above"),
        }
        swaps[i] = s.clone();
        save(self.wallet, &swaps)?;
        Ok(s)
    }

    /// Gives up a swap before anything is locked. `why` is shown to the user. A BTC seller
    /// whose deposit has already arrived cannot cancel: that bitcoin refunds at the timelock.
    pub fn cancel(&mut self, id: &str, why: &str) -> Result<Swap> {
        let mut swaps = load(self.wallet);
        let s = swaps
            .iter_mut()
            .find(|s| s.id == id)
            .context("no such swap")?;
        ensure!(
            s.cancellable(),
            "this swap can no longer be cancelled: coins are locked, and the timelocks return them"
        );
        if s.role == Role::BtcSeller
            && s.stage == Stage::AwaitDeposit
            && let Ok(addr) = s.htlc_address()
            && !self.btc.outputs(&addr)?.is_empty()
        {
            bail!(
                "the bitcoin deposit has already arrived; the swap continues, or the bitcoin refunds after block {}",
                s.btc_refund_height.unwrap_or(0)
            );
        }
        s.stage = Stage::Failed;
        s.code = None;
        s.say(why);
        let out = s.clone();
        save(self.wallet, &swaps)?;
        Ok(out)
    }

    /// Advances every swap as far as the chains allow. `mempool` are pending GNR
    /// transactions (claims there reveal secrets early).
    pub fn step(&mut self, mempool: &[Transaction]) -> Result<Vec<Swap>> {
        self.wallet.learn_secrets(mempool);
        let mut swaps = load(self.wallet);
        if swaps.iter().all(|s| s.stage.finished() && !s.watching()) {
            return Ok(swaps);
        }
        let btc_tip = self.btc.tip()?;
        let gnr_tip = self.gnr_tip()?;
        let t = now();
        let mut changed = false;
        for s in swaps.iter_mut() {
            let before = (s.stage, s.status.clone());
            // Nothing locked and no deposit for hours: give up (the GNR seller's lock window
            // would run out anyway).
            // The GNR seller waits an hour longer, so a deposit made at the last minute still
            // finds it.
            let limit = DEPOSIT_TIMEOUT_SECS + if s.role == Role::GnrSeller { 3600 } else { 0 };
            if s.cancellable() && s.created + limit < t {
                s.stage = Stage::Failed;
                s.code = None;
                s.say(if s.gnr_first() && s.role == Role::BtcSeller {
                    "Expired before you paid: nothing was paid."
                } else {
                    "Expired: no bitcoin deposit within 3 hours. Nothing was locked."
                });
            }
            // GNR first: the buyer pays within the window it was given, or not at all.
            if s.gnr_first()
                && s.stage == Stage::AwaitDeposit
                && s.pay_by.is_some_and(|p| p < t)
                && s.deposit.is_none()
            {
                s.stage = Stage::Failed;
                s.code = None;
                s.say("Expired: the payment window closed before you paid. Nothing was paid; if you pay now anyway, the bitcoin refunds.");
            }
            // Paid after all, into a given-up swap: refund it at the timelock.
            if s.watching()
                && btc_tip < s.btc_refund_height.unwrap_or(0) + 1_000
                && let Ok(addr) = s.htlc_address()
                && let Some(u) = self
                    .btc
                    .outputs(&addr)
                    .ok()
                    .and_then(|v| v.into_iter().next())
            {
                s.deposit = Some(Deposit {
                    txid: u.outpoint.txid.to_string(),
                    vout: u.outpoint.vout,
                    sats: u.value.to_sat(),
                    height: u.height,
                });
                s.stage = Stage::Deposited;
                // Without the seller's claim this only refunds (the GNR-first buyer also
                // claims the GNR if the seller takes the bitcoin after all).
                s.say(format!(
                    "A deposit arrived after the swap ended; it refunds after block {}.",
                    s.btc_refund_height.unwrap_or(0)
                ));
            }
            if s.stage.finished() {
                changed |= before != (s.stage, s.status.clone());
                continue;
            }
            let r = match (s.role, s.gnr_first()) {
                (Role::BtcSeller, true) => self.step2_buyer(s, btc_tip, gnr_tip),
                (Role::GnrSeller, true) => self.step2_seller(s, btc_tip, gnr_tip),
                (Role::BtcSeller, false) => self.step_btc_seller(s, btc_tip, gnr_tip),
                (Role::GnrSeller, false) => self.step_gnr_seller(s, btc_tip, gnr_tip),
            };
            if let Err(e) = r {
                s.say(format!("Waiting: {e}"));
            }
            changed |= before != (s.stage, s.status.clone());
        }
        if changed {
            save(self.wallet, &swaps)?;
        }
        Ok(swaps)
    }

    /// Whether our claim of the GNR was mined: the first claim output is our note. The
    /// lock being spent is not enough, a refund spends it too.
    fn claim_received(&self, s: &Swap) -> bool {
        self.wallet
            .notes()
            .iter()
            .any(|n| n.cm == s.claim_outputs[0].0)
    }

    fn step_btc_seller(&mut self, s: &mut Swap, btc_tip: u32, gnr_tip: u64) -> Result<()> {
        let t = self.timing;
        let Some(tl) = s.timelocks() else {
            return Ok(());
        };
        let lock = self.lock(s);
        let claimed = self.claim_received(s);
        let refunded = !claimed && lock.as_ref().is_some_and(|l| l.spent_height.is_some());
        match s.stage {
            Stage::AwaitDeposit => {
                let addr = s.htlc_address()?;
                if let Some(u) = self
                    .btc
                    .outputs(&addr)?
                    .into_iter()
                    .find(|u| u.value.to_sat() >= s.sats)
                {
                    s.deposit = Some(Deposit {
                        txid: u.outpoint.txid.to_string(),
                        vout: u.outpoint.vout,
                        sats: u.value.to_sat(),
                        height: u.height,
                    });
                    s.code = Some(
                        Msg::Funded {
                            id: s.id.clone(),
                            btc_refund_height: tl.btc_refund_height,
                            txid: u.outpoint.txid.to_string(),
                            vout: u.outpoint.vout,
                        }
                        .encode(),
                    );
                    s.stage = Stage::Deposited;
                    s.say("Deposit seen. Send the deposit code to the GNR seller.");
                }
            }
            Stage::Deposited | Stage::Locked => {
                if let Some(l) = &lock
                    && let Some(h) = l.height
                {
                    if s.stage == Stage::Deposited {
                        s.stage = Stage::Locked;
                        s.say("The GNR seller locked the GNR. Waiting for confirmations.");
                    }
                    let confs = gnr_tip + 1 - h;
                    if refunded {
                        s.say(format!(
                            "The GNR seller took the GNR back; the bitcoin refunds after block {}.",
                            tl.btc_refund_height
                        ));
                    } else if l.spent_height.is_none()
                        && confs >= t.gnr_confirmations
                        && t.may_claim_gnr(&tl, gnr_tip)
                    {
                        let secret = derive_secret(self.wallet.swap_seed(), s.index);
                        let cm = l.cm;
                        self.wallet.claim_lock(self.gnr, &cm, *secret.as_bytes())?;
                        s.stage = Stage::Claiming;
                        s.say("Claiming the GNR (this reveals the secret to the GNR seller).");
                        return Ok(());
                    }
                    if !refunded && !t.may_claim_gnr(&tl, gnr_tip) {
                        s.say(format!(
                            "Too late to claim the GNR safely; the bitcoin refunds after block {}.",
                            tl.btc_refund_height
                        ));
                    }
                }
            }
            Stage::Claiming => {
                if claimed {
                    s.stage = Stage::Done;
                    s.say(format!(
                        "Done: received {} GNR.",
                        krypton_core::emission::format_amount(s.gnr - s.claim_fee)
                    ));
                    return Ok(());
                }
                let pending = lock.as_ref().is_some_and(|l| l.pending_spend.is_some());
                if refunded || (!pending && t.may_claim_gnr(&tl, gnr_tip)) {
                    // Refunded under our claim, or the claim expired unmined: resend it or
                    // fall back to the bitcoin refund below.
                    s.stage = Stage::Locked;
                }
            }
            Stage::Refunding => {
                if s.deposit.is_some() && self.btc.is_spent(&s.funding()?.outpoint)? {
                    s.stage = Stage::Refunded;
                    s.say("Refunded: the bitcoin is back.");
                }
                return Ok(());
            }
            _ => {}
        }
        // The bitcoin refund, unless the GNR was claimed (then the seller has the secret).
        // nLockTime = T is final only in blocks above T, so wait until the tip reaches T.
        if matches!(
            s.stage,
            Stage::Deposited | Stage::Locked | Stage::AwaitDeposit
        ) && s.deposit.is_some()
            && !claimed
            && btc_tip >= tl.btc_refund_height
        {
            self.btc_refund(s)?;
        }
        Ok(())
    }

    fn btc_refund(&mut self, s: &mut Swap) -> Result<()> {
        let htlc = s.htlc()?;
        let f = s.funding()?;
        if self.btc.is_spent(&f.outpoint)? {
            s.stage = Stage::Refunded;
            s.say("The deposit has already been spent.");
            return Ok(());
        }
        let dest = btc_address(&s.btc_address, self.btc_network)?.script_pubkey();
        let key = derive_key(self.wallet.swap_seed(), s.index);
        let tx = htlc.refund(f, &dest, self.btc.fee_rate()?, &key)?;
        let id = self.btc.broadcast(&tx)?;
        s.btc_spend = Some(id.to_string());
        s.stage = Stage::Refunding;
        s.say(format!("Bitcoin refund sent ({id})."));
        Ok(())
    }

    /// GNR first, the buyer: waits for the GNR lock, only then shows where to pay, reads
    /// the secret from the seller's bitcoin claim and claims the GNR with it.
    fn step2_buyer(&mut self, s: &mut Swap, btc_tip: u32, gnr_tip: u64) -> Result<()> {
        let t = self.timing;
        let lock = self.lock(s);
        let claimed = self.claim_received(s);
        let Some(gnr_refund) = s.gnr_refund_height else {
            return Ok(()); // Requested: waiting for the accept code.
        };
        match s.stage {
            Stage::Accepted => {
                let Some(l) = &lock else { return Ok(()) };
                if l.spent_height.is_some() {
                    s.stage = Stage::Failed;
                    s.say("The seller took the GNR back before you paid. Nothing was paid.");
                    return Ok(());
                }
                let Some(h) = l.height else {
                    s.say("Waiting for the seller to lock the GNR for you. You pay nothing before it is locked on chain.");
                    return Ok(());
                };
                let confs = gnr_tip + 1 - h;
                if confs < t.gnr_confs_for(s.gnr) {
                    s.say(format!(
                        "The seller locked {} GNR for you at block {h}: {confs} of {} confirmations before you pay.",
                        krypton_core::emission::format_amount(s.gnr - s.claim_fee),
                        t.gnr_confs_for(s.gnr)
                    ));
                    return Ok(());
                }
                let tl = Timelocks {
                    btc_refund_height: btc_tip + t.btc_lock2_blocks,
                    gnr_refund_height: gnr_refund,
                };
                if let Err(e) = t.check_pay(&tl, btc_tip, gnr_tip) {
                    s.stage = Stage::Failed;
                    s.say(format!(
                        "Not paying: {e}. Nothing was paid; the seller's GNR returns to them."
                    ));
                    return Ok(());
                }
                s.btc_refund_height = Some(tl.btc_refund_height);
                s.pay_by = Some(now() + t.pay_window_secs);
                s.stage = Stage::AwaitDeposit;
                let addr = s.htlc_address()?;
                s.say(format!(
                    "{} GNR are locked for you (block {h}, {confs} confirmations). Pay exactly {} BTC to {addr} within {} minutes.",
                    krypton_core::emission::format_amount(s.gnr - s.claim_fee),
                    Amount::from_sat(s.sats).to_btc(),
                    t.pay_window_secs / 60
                ));
            }
            Stage::AwaitDeposit => {
                let addr = s.htlc_address()?;
                if let Some(u) = self
                    .btc
                    .outputs(&addr)?
                    .into_iter()
                    .find(|u| u.value.to_sat() >= s.sats)
                {
                    let tl = s.timelocks().context("timelocks")?;
                    s.deposit = Some(Deposit {
                        txid: u.outpoint.txid.to_string(),
                        vout: u.outpoint.vout,
                        sats: u.value.to_sat(),
                        height: u.height,
                    });
                    s.code = Some(
                        Msg::Funded {
                            id: s.id.clone(),
                            btc_refund_height: tl.btc_refund_height,
                            txid: u.outpoint.txid.to_string(),
                            vout: u.outpoint.vout,
                        }
                        .encode(),
                    );
                    s.stage = Stage::Deposited;
                    s.say("Paid. The seller takes the bitcoin with the secret, and your wallet then claims the GNR with it.");
                }
            }
            Stage::Deposited | Stage::Claiming => {
                if claimed {
                    s.stage = Stage::Done;
                    s.say(format!(
                        "Done: received {} GNR.",
                        krypton_core::emission::format_amount(s.gnr - s.claim_fee)
                    ));
                    return Ok(());
                }
                let tl = s.timelocks().context("timelocks")?;
                let f = s.funding()?;
                if s.deposit.as_ref().is_some_and(|d| d.height.is_none())
                    && let Some(u) = self
                        .btc
                        .outputs(&s.htlc_address()?)?
                        .into_iter()
                        .find(|u| u.outpoint == f.outpoint)
                    && let Some(d) = s.deposit.as_mut()
                {
                    d.height = u.height;
                }
                if s.learned.is_none() && self.btc.is_spent(&f.outpoint)? {
                    let from = s
                        .deposit
                        .as_ref()
                        .and_then(|d| d.height)
                        .unwrap_or(btc_tip.saturating_sub(t.btc_lock2_blocks + 10));
                    if let Some(tx) = self.btc.spender(&f.outpoint, from)? {
                        match s.htlc()?.find_secret(&tx) {
                            Some(secret) => s.learned = Some(hex::encode(secret.as_bytes())),
                            None if s.btc_spend.is_some() => {
                                s.stage = Stage::Refunding;
                                return Ok(());
                            }
                            None => {}
                        }
                    }
                }
                if let Some(secret) = s.learned.clone() {
                    let secret: [u8; 32] = hex::decode(secret)?
                        .try_into()
                        .map_err(|_| anyhow!("bad secret"))?;
                    let Some(l) = &lock else {
                        bail!("the GNR lock is not in the wallet");
                    };
                    if l.spent_height.is_some() {
                        s.say("The GNR lock was spent by someone else (refunded?).");
                        return Ok(());
                    }
                    if l.pending_spend.is_some() {
                        return Ok(());
                    }
                    if !t.may_claim_gnr(&tl, gnr_tip) {
                        s.say("Too late to claim the GNR: its lock has expired.");
                        return Ok(());
                    }
                    let cm = l.cm;
                    self.wallet.claim_lock(self.gnr, &cm, secret)?;
                    s.stage = Stage::Claiming;
                    s.say("The seller took the bitcoin; claiming your GNR with the secret it revealed.");
                    return Ok(());
                }
                // The seller never took the bitcoin: it comes back at the timelock.
                if !self.btc.is_spent(&f.outpoint)? && btc_tip >= tl.btc_refund_height {
                    self.btc_refund(s)?;
                }
            }
            Stage::Refunding
                if s.deposit.is_some() && self.btc.is_spent(&s.funding()?.outpoint)? =>
            {
                s.stage = Stage::Refunded;
                s.say("Refunded: the bitcoin is back.");
            }
            _ => {}
        }
        Ok(())
    }

    /// GNR first, the seller: locks the GNR at once, takes the bitcoin with its secret once
    /// the payment has confirmed, and refunds the GNR if nobody paid in time.
    fn step2_seller(&mut self, s: &mut Swap, btc_tip: u32, gnr_tip: u64) -> Result<()> {
        let t = self.timing;
        let gnr_refund = s.gnr_refund_height.context("no GNR refund height")?;
        if s.stage == Stage::Accepted && s.lock_cm.is_none() {
            if gnr_tip + t.gnr_lock2_blocks * 9 / 10 > gnr_refund {
                s.stage = Stage::Failed;
                s.say("Too late to lock the GNR for this swap; nothing was locked.");
                return Ok(());
            }
            let terms = s.terms().context("terms")?;
            // The terms are checked before anything is sent: a mismatch must not lock coins.
            ensure!(
                Some(self.wallet.refund_outputs(
                    s.index,
                    s.gnr,
                    s.refund_fee.unwrap_or(DEFAULT_FEE)
                )?) == s.refund_outputs,
                "the lock would not match the agreed terms"
            );
            let (_, got) = self.wallet.lock_funds(
                self.gnr,
                s.index,
                s.hash,
                gnr_refund,
                s.claim_outputs,
                s.gnr,
                s.refund_fee.unwrap_or(DEFAULT_FEE),
                DEFAULT_FEE,
            )?;
            ensure!(got == terms, "lock terms differ from the accepted ones");
            s.lock_cm = Some(hex::encode(got.commitment(s.gnr)));
            s.stage = Stage::Locked;
            s.say("GNR locked for the buyer. They pay once the lock has confirmed; if they do not, the GNR comes back to you.");
            return Ok(());
        }
        let lock = self.lock(s);
        // The buyer never paid (or we could not take the bitcoin in time): refund the GNR.
        let refund_due = |l: &LockRecord| {
            l.spent_height.is_none() && l.pending_spend.is_none() && gnr_tip + 1 >= gnr_refund
        };
        match s.stage {
            Stage::Locked | Stage::Accepted => {
                if let Some(l) = &lock
                    && refund_due(l)
                {
                    let cm = l.cm;
                    self.wallet.refund_lock(self.gnr, &cm)?;
                    s.stage = Stage::Refunding;
                    s.say("Nobody paid in time: refunding the GNR.");
                }
            }
            Stage::Deposited => {
                let tl = s.timelocks().context("timelocks")?;
                let f = s.funding()?;
                let found = self
                    .btc
                    .outputs(&s.htlc_address()?)?
                    .into_iter()
                    .find(|u| u.outpoint == f.outpoint);
                let paid = match found {
                    None => {
                        s.say("Waiting for the buyer's bitcoin payment to appear.");
                        false
                    }
                    Some(u) if u.value.to_sat() < s.sats => {
                        s.say("The payment is smaller than agreed; not taking it. The GNR refunds at its timelock.");
                        false
                    }
                    Some(u) => {
                        let confs = u.height.map(|h| btc_tip + 1 - h).unwrap_or(0);
                        if confs < t.btc_confs_for(s.sats) {
                            s.say(format!(
                                "Bitcoin payment seen, {confs} of {} confirmations.",
                                t.btc_confs_for(s.sats)
                            ));
                            false
                        } else if !t.may_claim_btc(&tl, btc_tip) {
                            s.say(format!(
                                "Too late to take the bitcoin safely; the GNR refunds after block {gnr_refund}."
                            ));
                            false
                        } else {
                            true
                        }
                    }
                };
                if paid {
                    self.btc_claim(s)?;
                    s.stage = Stage::Claiming;
                } else if let Some(l) = &lock
                    && refund_due(l)
                {
                    let cm = l.cm;
                    self.wallet.refund_lock(self.gnr, &cm)?;
                    s.stage = Stage::Refunding;
                    s.say("The bitcoin was not taken in time: refunding the GNR.");
                }
            }
            Stage::Claiming => {
                let tl = s.timelocks().context("timelocks")?;
                let f = s.funding()?;
                if self.btc.is_spent(&f.outpoint)? {
                    // Ours if it reveals the secret (any of our fee bumps); a refund does not.
                    let from = btc_tip.saturating_sub(t.btc_lock2_blocks + 10);
                    let htlc = s.htlc()?;
                    let spender = self.btc.spender(&f.outpoint, from)?;
                    if spender.is_some_and(|tx| htlc.find_secret(&tx).is_some()) {
                        s.stage = Stage::Done;
                        s.say(format!(
                            "Done: received {} BTC.",
                            Amount::from_sat(s.sats).to_btc()
                        ));
                    } else {
                        s.say("The payment was spent by another transaction (refunded?).");
                    }
                    return Ok(());
                }
                // Still unconfirmed: send it again at today's fee (it replaces the old one).
                if t.may_claim_btc(&tl, btc_tip) {
                    let _ = self.btc_claim(s);
                }
            }
            Stage::Done => {
                // The buyer never claimed before the lock ended: only a refund can move it.
                if let Some(l) = &lock
                    && refund_due(l)
                {
                    let cm = l.cm;
                    self.wallet.refund_lock(self.gnr, &cm)?;
                    s.say("The buyer did not claim the GNR before its lock ended; refunded it to you.");
                }
            }
            Stage::Refunding => {
                if let Some(l) = &lock
                    && l.spent_height.is_some()
                {
                    s.stage = Stage::Refunded;
                    s.say("Refunded: the GNR is back.");
                }
            }
            _ => {}
        }
        Ok(())
    }

    /// GNR first, the seller takes the bitcoin with its secret.
    fn btc_claim(&mut self, s: &mut Swap) -> Result<()> {
        let f = s.funding()?;
        let dest = btc_address(&s.btc_address, self.btc_network)?.script_pubkey();
        let key = derive_key(self.wallet.swap_seed(), s.index);
        let secret = derive_secret(self.wallet.swap_seed(), s.index);
        let tx = s
            .htlc()?
            .claim(f, &dest, self.btc.fee_rate()?, &key, &secret)?;
        let id = tx.compute_txid();
        match self.btc.broadcast(&tx) {
            Ok(_) => {}
            // Resent unchanged, or a replacement at the same fee: the first one stands.
            Err(e) if s.btc_spend.is_some() => {
                let _ = e;
                return Ok(());
            }
            Err(e) => return Err(e),
        }
        s.btc_spend = Some(id.to_string());
        s.say(format!("Taking the bitcoin ({id}); this reveals the secret, and the buyer's wallet claims the GNR with it."));
        Ok(())
    }

    fn step_gnr_seller(&mut self, s: &mut Swap, btc_tip: u32, gnr_tip: u64) -> Result<()> {
        let t = self.timing;
        let Some(tl) = s.timelocks() else {
            // Accepted: no deposit code yet. Give up once the lock could not start in time.
            if gnr_tip + t.gnr_confirmations + t.claim_margin + t.gnr_lock_blocks / 4
                >= s.gnr_refund_height.unwrap_or(0)
            {
                s.stage = Stage::Failed;
                s.say("No deposit arrived in time; nothing was locked.");
            }
            return Ok(());
        };
        match s.stage {
            Stage::Deposited => {
                let f = s.funding()?;
                let addr = s.htlc_address()?;
                let found = self
                    .btc
                    .outputs(&addr)?
                    .into_iter()
                    .find(|u| u.outpoint == f.outpoint);
                let Some(u) = found else {
                    s.say("Waiting for the bitcoin deposit to appear.");
                    return Ok(());
                };
                ensure!(
                    u.value.to_sat() >= s.sats,
                    "the deposit is smaller than agreed"
                );
                let confs = u.height.map(|h| btc_tip + 1 - h).unwrap_or(0);
                if confs < t.btc_confirmations {
                    s.say(format!(
                        "Bitcoin deposit seen, {confs} of {} confirmations.",
                        t.btc_confirmations
                    ));
                    return Ok(());
                }
                if let Err(e) = t.check(&tl, btc_tip, gnr_tip) {
                    s.stage = Stage::Failed;
                    s.say(format!(
                        "Not locking GNR: {e}. The BTC seller gets the bitcoin back."
                    ));
                    return Ok(());
                }
                let terms = s.terms().context("terms")?;
                // The terms are checked before anything is sent: a mismatch must not lock coins.
                ensure!(
                    Some(self.wallet.refund_outputs(
                        s.index,
                        s.gnr,
                        s.refund_fee.unwrap_or(DEFAULT_FEE)
                    )?) == s.refund_outputs,
                    "the lock would not match the agreed terms"
                );
                let (_, got) = self.wallet.lock_funds(
                    self.gnr,
                    s.index,
                    s.hash,
                    tl.gnr_refund_height,
                    s.claim_outputs,
                    s.gnr,
                    s.refund_fee.unwrap_or(DEFAULT_FEE),
                    DEFAULT_FEE,
                )?;
                ensure!(got == terms, "lock terms differ from the accepted ones");
                s.lock_cm = Some(hex::encode(got.commitment(s.gnr)));
                s.stage = Stage::Locked;
                s.say("GNR locked. Waiting for the BTC seller to claim it.");
            }
            Stage::Locked | Stage::Claiming => {
                let Some(l) = self.lock(s) else { return Ok(()) };
                if l.secret.is_none()
                    && let Some(h) = l.spent_height
                {
                    let txs = (self.block_txs)(h)?;
                    self.wallet.learn_secrets(&txs);
                }
                let l = self.lock(s).expect("lock");
                if let Some(secret) = l.secret {
                    let f = s.funding()?;
                    if self.btc.is_spent(&f.outpoint)? {
                        if s.btc_spend.is_some() {
                            s.stage = Stage::Done;
                            s.say(format!(
                                "Done: received {} BTC.",
                                Amount::from_sat(s.sats).to_btc()
                            ));
                        } else {
                            s.say("The deposit was spent by someone else (refunded?).");
                        }
                        return Ok(());
                    }
                    let dest = btc_address(&s.btc_address, self.btc_network)?.script_pubkey();
                    let key = derive_key(self.wallet.swap_seed(), s.index);
                    let tx = s.htlc()?.claim(
                        f,
                        &dest,
                        self.btc.fee_rate()?,
                        &key,
                        &Secret::from_bytes(secret),
                    )?;
                    let id = self.btc.broadcast(&tx)?;
                    s.btc_spend = Some(id.to_string());
                    s.stage = Stage::Claiming;
                    s.say(format!("Claiming the bitcoin ({id})."));
                    return Ok(());
                }
                if l.spent_height.is_none()
                    && l.pending_spend.is_none()
                    && gnr_tip + 1 >= tl.gnr_refund_height
                {
                    self.wallet.refund_lock(self.gnr, &l.cm)?;
                    s.stage = Stage::Refunding;
                    s.say("Nobody claimed the GNR in time: refunding it.");
                }
            }
            Stage::Refunding => {
                if let Some(l) = self.lock(s)
                    && l.spent_height.is_some()
                {
                    if l.secret.is_some() {
                        // Claimed after all: take the bitcoin.
                        s.stage = Stage::Locked;
                    } else {
                        s.stage = Stage::Refunded;
                        s.say("Refunded: the GNR is back.");
                    }
                }
            }
            _ => {}
        }
        Ok(())
    }
}

fn new_index(used: impl Iterator<Item = u32> + Clone) -> u32 {
    loop {
        let i = rand::random::<u32>() | (1 << 31);
        if !used.clone().any(|u| u == i) {
            return i;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn swap_numbers_never_repeat_after_a_restore() {
        // Older versions counted 1, 2, 3, ...; new numbers come from the upper half.
        let mut used: Vec<u32> = (1..=3).collect();
        for _ in 0..1_000 {
            let i = new_index(used.iter().copied());
            assert!(i >= 1 << 31 && !used.contains(&i));
            used.push(i);
        }
        // Two wallets from the same recovery phrase, without each other's records: their
        // first swaps get different numbers, hence different secrets.
        let seed = [7u8; 64];
        let a = derive_secret(&seed, new_index(std::iter::empty()));
        let b = derive_secret(&seed, new_index(std::iter::empty()));
        assert_ne!(a.hash(), b.hash());
    }
}

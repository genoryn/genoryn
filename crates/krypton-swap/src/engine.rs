//! The swap protocol between two wallets and the steps each side takes on its own.
//!
//! Three codes pass between the two people (copy and paste, or a relay):
//!
//! 1. **Request** (BTC seller → GNR seller): amounts, the hash lock, the BTC seller's
//!    refund key and its GNR claim outputs.
//! 2. **Accept** (GNR seller → BTC seller): the GNR seller's bitcoin claim key and the
//!    full GNR lock terms (refund height and refund outputs). The BTC seller's wallet now
//!    knows the lock note and spots it on chain by itself.
//! 3. **Funded** (BTC seller → GNR seller): the bitcoin refund height and the deposit.
//!
//! After that both wallets act alone: the GNR seller locks GNR once the deposit has
//! confirmed, the BTC seller claims it (publishing the secret) once the lock has
//! confirmed, the GNR seller claims the bitcoin with the secret. Timeouts refund both.

use crate::btc::BtcChain;
use crate::{Funding, Htlc, Secret, Timelocks, Timing, derive_key, derive_secret, public_key};
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
    /// Whether this side can still walk away: nothing of anyone's is locked yet.
    pub fn cancellable(&self) -> bool {
        match self.role {
            Role::BtcSeller => matches!(self.stage, Stage::Requested | Stage::AwaitDeposit),
            Role::GnrSeller => self.stage == Stage::Accepted,
        }
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
}

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Msg {
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
            Msg::Request { id, .. } | Msg::Accept { id, .. } | Msg::Funded { id, .. } => id,
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
    fn next_index(swaps: &[Swap]) -> u32 {
        swaps.iter().map(|s| s.index).max().unwrap_or(0) + 1
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
        let secret = derive_secret(&seed, index);
        let refund_pub = public_key(&derive_key(&seed, index));
        let claim_fee = DEFAULT_FEE;
        let claim_outputs = self.wallet.claim_outputs(index, gnr, claim_fee)?;
        let id = hex::encode(rand::random::<[u8; 16]>());
        let msg = Msg::Request {
            id: id.clone(),
            gnr_network: self.wallet.network(),
            btc_network: net_name(self.btc_network),
            gnr,
            sats,
            hash: secret.hash(),
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
            hash: secret.hash(),
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
        };
        s.say("Send the request code to the GNR seller and paste their answer here.");
        swaps.push(s.clone());
        save(self.wallet, &swaps)?;
        Ok(s)
    }

    /// GNR seller: accepts a request; the bitcoin will be paid to `payout_address`.
    /// Returns the swap with the accept code for the BTC seller.
    pub fn accept(&mut self, code: &str, payout_address: &str) -> Result<Swap> {
        self.check_active()?;
        let Msg::Request {
            id,
            gnr_network,
            btc_network,
            gnr,
            sats,
            hash,
            claim_fee,
            claim_outputs,
            refund_pub,
        } = Msg::decode(code)?
        else {
            bail!("this is not a swap request");
        };
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
        // Room for the deposit to arrive and confirm before the lock window starts.
        let gnr_refund_height = self.gnr_tip()? + self.timing.gnr_lock_blocks * 3 / 2;
        let msg = Msg::Accept {
            id: id.clone(),
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
        };
        s.say("Send the accept code to the BTC seller and wait for their deposit code.");
        swaps.push(s.clone());
        save(self.wallet, &swaps)?;
        Ok(s)
    }

    /// Takes the other side's next code (accept or funded).
    pub fn receive(&mut self, code: &str) -> Result<Swap> {
        let msg = Msg::decode(code)?;
        if matches!(msg, Msg::Request { .. }) {
            bail!("REQUEST: this is a new swap request; accept it to sell GNR");
        }
        let mut swaps = load(self.wallet);
        let i = swaps
            .iter()
            .position(|s| s.id == msg.id())
            .context("no swap in this wallet matches the code")?;
        let mut s = swaps[i].clone();
        match msg {
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
                });
                s.stage = Stage::Deposited;
                s.code = None;
                s.say("Checking the bitcoin deposit.");
            }
            Msg::Request { .. } => unreachable!("handled above"),
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
                s.say("Expired: no bitcoin deposit within 3 hours. Nothing was locked.");
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
                });
                s.stage = Stage::Deposited;
                s.say(format!(
                    "A deposit arrived after the swap ended; it refunds after block {}.",
                    s.btc_refund_height.unwrap_or(0)
                ));
            }
            if s.stage.finished() {
                changed |= before != (s.stage, s.status.clone());
                continue;
            }
            let r = match s.role {
                Role::BtcSeller => self.step_btc_seller(s, btc_tip, gnr_tip),
                Role::GnrSeller => self.step_gnr_seller(s, btc_tip, gnr_tip),
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

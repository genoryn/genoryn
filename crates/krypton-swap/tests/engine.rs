//! Two wallets swap BTC for GNR through the engine: a real GNR devnet chain with real
//! proofs, and a simulated Bitcoin chain that runs every transaction through Bitcoin
//! Core's script interpreter and enforces nLockTime.

use anyhow::{Context, Result, bail};
use bitcoin::hashes::Hash;
use bitcoin::key::CompressedPublicKey;
use bitcoin::{Address, Amount, FeeRate, Network as BtcNet, OutPoint, Transaction, TxOut, Txid};
use krypton_core::address::Address as GnrAddress;
use krypton_core::types::{BlockHeader, Transaction as GnrTx};
use krypton_core::{Hash32, Network};
use krypton_crypto::field::{Digest, digest_to_bytes};
use krypton_light::CompactBlock;
use krypton_store::Chain;
use krypton_swap::btc::{BtcChain, Unconfirmed, Utxo};
use krypton_swap::engine::{self, Engine, Role, Stage, Swap, load};
use krypton_swap::{Timing, derive_key, derive_secret, public_key};
use krypton_wallet_core::{ChainSource, Wallet, keys};
use std::cell::RefCell;
use std::collections::HashMap;
use std::str::FromStr;
use std::sync::Mutex;

struct Local<'a> {
    chain: &'a Chain,
    pool: RefCell<Vec<GnrTx>>,
}

impl ChainSource for Local<'_> {
    fn tip_height(&self) -> Result<u64> {
        Ok(self.chain.tip().height())
    }
    fn header(&self, height: u64) -> Result<BlockHeader> {
        self.chain.header_at(height)?.context("no header")
    }
    fn compact_blocks(&self, from: u64, count: u32) -> Result<Vec<CompactBlock>> {
        (from..from + count as u64)
            .filter_map(|h| self.chain.compact_block(h).transpose())
            .collect()
    }
    fn frontier(&self, height: u64) -> Result<krypton_core::tree::Frontier> {
        self.chain.frontier_at(height)?.context("no checkpoint")
    }
    fn witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        self.chain.auth_path(pos)
    }
    fn submit(&self, tx: &GnrTx) -> Result<Hash32> {
        self.chain
            .check_mempool_tx(tx)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.pool.borrow_mut().push(tx.clone());
        Ok(tx.txid())
    }
    fn verified_tip(
        &self,
        _: &krypton_core::ChainParams,
        _: &krypton_pow::Verifier,
        _: Option<&BlockHeader>,
    ) -> Result<Option<BlockHeader>> {
        Ok(None)
    }
    fn header_in(&self, h: &BlockHeader, _: &BlockHeader) -> Result<bool> {
        Ok(self
            .chain
            .header_at(h.height)?
            .is_some_and(|x| x.hash() == h.hash()))
    }
}

fn mine(src: &Local, to: &GnrAddress) {
    let c = src.chain;
    let txs: Vec<GnrTx> = src.pool.borrow_mut().drain(..).collect();
    let mut b = c
        .template(
            digest_to_bytes(&to.owner),
            vec![],
            txs,
            c.tip().header.timestamp + 60,
        )
        .unwrap();
    while !c.pow.verify(
        b.header.height,
        &b.header.pow_hash(),
        b.header.nonce,
        b.header.difficulty,
    ) {
        b.header.nonce += 1;
    }
    c.process_block(&b, u64::MAX / 4).expect("block accepted");
}

/// A Bitcoin chain in memory: a UTXO set, a mempool and a height.
#[derive(Default)]
struct MockBtc {
    inner: Mutex<BtcState>,
}

#[derive(Default)]
struct BtcState {
    height: u32,
    utxos: HashMap<OutPoint, (TxOut, Option<u32>)>,
    spent: HashMap<OutPoint, Txid>,
    txs: HashMap<Txid, Transaction>,
    mempool: Vec<Transaction>,
    /// A busy network: blocks leave the mempool's transactions unconfirmed.
    stalled: bool,
    /// What payments from outside pay while they wait, and the fee estimates of the day.
    waiting: HashMap<Txid, Unconfirmed>,
    estimates: Vec<(u32, f64)>,
    /// A server that is behind: it takes payments, and what it tells about addresses and
    /// outputs does not know them yet.
    lagging: bool,
    unlisted: std::collections::HashSet<Txid>,
    /// A server like Bitcoin Core: it lists what pays an address only once that is in a
    /// block, and of one output it says whether it is there, the mempool included.
    blocks_only: bool,
}

impl BtcState {
    fn evict(&mut self, id: Txid) {
        let Some(tx) = self.txs.remove(&id) else {
            return;
        };
        for i in &tx.input {
            self.spent.remove(&i.previous_output);
        }
        for n in 0..tx.output.len() {
            self.utxos.remove(&OutPoint::new(id, n as u32));
        }
        self.mempool.retain(|t| t.compute_txid() != id);
    }
    fn fee(&self, tx: &Transaction) -> u64 {
        let inputs: u64 = tx
            .input
            .iter()
            .filter_map(|i| self.utxos.get(&i.previous_output))
            .map(|(o, _)| o.value.to_sat())
            .sum();
        inputs - tx.output.iter().map(|o| o.value.to_sat()).sum::<u64>()
    }
}

impl MockBtc {
    /// Someone pays `sats` to `addr` (the BTC seller's own wallet, outside the engine).
    fn pay(&self, addr: &Address, sats: u64) -> OutPoint {
        let mut s = self.inner.lock().unwrap();
        let n = s.utxos.len() as u8;
        let op = OutPoint::new(Txid::from_byte_array([n.wrapping_add(1); 32]), 0);
        s.utxos.insert(
            op,
            (
                TxOut {
                    value: Amount::from_sat(sats),
                    script_pubkey: addr.script_pubkey(),
                },
                None,
            ),
        );
        op
    }
    /// Someone pays `sats` to `addr` with a fee of `fee` sat for `vsize` vB; `parents`:
    /// what that payment spends is in a block.
    fn pay_waiting(
        &self,
        addr: &Address,
        sats: u64,
        fee: u64,
        vsize: u64,
        parents: bool,
    ) -> OutPoint {
        let op = self.pay(addr, sats);
        self.inner.lock().unwrap().waiting.insert(
            op.txid,
            Unconfirmed {
                fee,
                vsize,
                parents_confirmed: parents,
                seen: None,
            },
        );
        op
    }
    fn estimates(&self, v: &[(u32, f64)]) {
        self.inner.lock().unwrap().estimates = v.to_vec();
    }
    fn mine(&self) {
        let mut s = self.inner.lock().unwrap();
        s.height += 1;
        let h = s.height;
        let waiting: std::collections::HashSet<Txid> = if s.stalled {
            s.mempool.iter().map(|t| t.compute_txid()).collect()
        } else {
            Default::default()
        };
        for (op, u) in s.utxos.iter_mut() {
            if !waiting.contains(&op.txid) {
                u.1.get_or_insert(h);
            }
        }
        if !s.stalled {
            s.mempool.clear();
        }
    }
    fn stall(&self, on: bool) {
        self.inner.lock().unwrap().stalled = on;
    }
    /// The server does not show the payment `id` (yet).
    fn unlist(&self, id: Txid) {
        self.inner.lock().unwrap().unlisted.insert(id);
    }
    fn blocks_only(&self, on: bool) {
        self.inner.lock().unwrap().blocks_only = on;
    }
    /// The server falls behind (payments sent from now on are not listed), or catches up.
    fn lag(&self, on: bool) {
        let mut s = self.inner.lock().unwrap();
        s.lagging = on;
        if !on {
            s.unlisted.clear();
        }
    }
    /// The fee of a transaction the chain knows.
    fn fee_of(&self, id: &str) -> u64 {
        let s = self.inner.lock().unwrap();
        let tx = s.txs[&Txid::from_str(id).unwrap()].clone();
        let inputs: u64 = tx
            .input
            .iter()
            .map(|i| s.utxos[&i.previous_output].0.value.to_sat())
            .sum();
        inputs - tx.output.iter().map(|o| o.value.to_sat()).sum::<u64>()
    }
    fn received(&self, addr: &Address) -> u64 {
        let s = self.inner.lock().unwrap();
        s.utxos
            .iter()
            .filter(|(op, (o, _))| {
                o.script_pubkey == addr.script_pubkey() && !s.spent.contains_key(op)
            })
            .map(|(_, (o, _))| o.value.to_sat())
            .sum()
    }
    fn broadcasts(&self) -> usize {
        self.inner.lock().unwrap().spent.len()
    }
    /// Takes an unconfirmed transaction out of the mempool (dropped, or replaced).
    fn evict(&self, id: Txid) {
        self.inner.lock().unwrap().evict(id);
    }
    /// A conflicting transaction with a higher fee takes the place of the one in the mempool.
    fn replace(&self, tx: &Transaction) -> Result<Txid> {
        let old: Vec<Txid> = {
            let s = self.inner.lock().unwrap();
            tx.input
                .iter()
                .filter_map(|i| s.spent.get(&i.previous_output).copied())
                .collect()
        };
        for id in old {
            self.evict(id);
        }
        self.broadcast(tx)
    }
}

impl BtcChain for MockBtc {
    fn tip(&self) -> Result<u32> {
        Ok(self.inner.lock().unwrap().height)
    }
    fn outputs(&self, address: &Address) -> Result<Vec<Utxo>> {
        let s = self.inner.lock().unwrap();
        Ok(s.utxos
            .iter()
            .filter(|(op, _)| !s.unlisted.contains(&op.txid))
            .filter(|(_, (_, h))| !s.blocks_only || h.is_some())
            .filter(|(_, (o, _))| o.script_pubkey == address.script_pubkey())
            .map(|(op, (o, h))| Utxo {
                outpoint: *op,
                value: o.value,
                height: *h,
            })
            .collect())
    }
    fn is_spent(&self, op: &OutPoint) -> Result<bool> {
        let s = self.inner.lock().unwrap();
        if s.unlisted.contains(&op.txid) {
            bail!("Transaction not found");
        }
        // (Bitcoin Core says "not there" of an output it does not know, too.)
        if s.blocks_only && !s.utxos.contains_key(op) {
            return Ok(true);
        }
        Ok(s.spent.get(op).is_some_and(|by| !s.unlisted.contains(by)))
    }
    fn lists_unconfirmed(&self) -> bool {
        !self.inner.lock().unwrap().blocks_only
    }
    fn spender(&self, op: &OutPoint, _from: u32) -> Result<Option<Transaction>> {
        let s = self.inner.lock().unwrap();
        Ok(s.spent
            .get(op)
            .filter(|id| !s.unlisted.contains(*id))
            .and_then(|id| s.txs.get(id))
            .cloned())
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        let mut s = self.inner.lock().unwrap();
        // A spend still in the mempool may be replaced by one paying more fee (RBF).
        let mut replaced = Vec::new();
        for i in &tx.input {
            if let Some(old) = s.spent.get(&i.previous_output).copied() {
                if !s.mempool.iter().any(|t| t.compute_txid() == old) {
                    bail!("bad-txns-inputs-missingorspent");
                }
                replaced.push(old);
            }
        }
        for old in replaced {
            let old_tx = s.txs[&old].clone();
            if s.fee(tx) <= s.fee(&old_tx) {
                bail!("insufficient fee");
            }
            s.evict(old);
        }
        // Final for the next block only if nLockTime is below its height.
        let lt = tx.lock_time.to_consensus_u32();
        if lt != 0 && lt > s.height {
            bail!("non-final");
        }
        let utxos = s.utxos.clone();
        tx.verify(|op| utxos.get(op).map(|(o, _)| o.clone()))
            .map_err(|e| anyhow::anyhow!("script verification failed: {e}"))?;
        let id = tx.compute_txid();
        for i in &tx.input {
            s.spent.insert(i.previous_output, id);
        }
        for (n, o) in tx.output.iter().enumerate() {
            s.utxos
                .insert(OutPoint::new(id, n as u32), (o.clone(), None));
        }
        s.mempool.push(tx.clone());
        s.txs.insert(id, tx.clone());
        if s.lagging {
            s.unlisted.insert(id);
        }
        Ok(id)
    }
    fn fee_rate(&self) -> Result<bitcoin::FeeRate> {
        Ok(bitcoin::FeeRate::from_sat_per_vb(3).unwrap())
    }
    fn tx(&self, txid: &Txid) -> Result<Option<Transaction>> {
        let s = self.inner.lock().unwrap();
        if s.unlisted.contains(txid) {
            bail!("Transaction not found");
        }
        Ok(s.txs.get(txid).cloned())
    }
    fn unconfirmed(&self, txid: &Txid) -> Result<Option<Unconfirmed>> {
        let s = self.inner.lock().unwrap();
        let in_block = |id: &Txid| {
            s.utxos
                .iter()
                .any(|(op, (_, h))| op.txid == *id && h.is_some())
        };
        if in_block(txid) {
            return Ok(None);
        }
        if let Some(u) = s.waiting.get(txid) {
            return Ok(Some(*u));
        }
        // A payment this chain saw itself: its fee, its size, and whether what it spends
        // is in a block.
        Ok(s.txs.get(txid).map(|tx| Unconfirmed {
            fee: s.fee(tx),
            vsize: tx.vsize() as u64,
            seen: None,
            parents_confirmed: tx.input.iter().all(|i| {
                s.utxos
                    .get(&i.previous_output)
                    .is_some_and(|u| u.1.is_some())
            }),
        }))
    }
    fn fee_estimates(&self) -> Result<Vec<(u32, f64)>> {
        Ok(self.inner.lock().unwrap().estimates.clone())
    }
}

fn btc_addr(seed: u8) -> Address {
    let sk = derive_key(&[seed; 64], 0);
    Address::p2wpkh(&CompressedPublicKey(public_key(&sk).inner), BtcNet::Regtest)
}

struct Side {
    wallet: Wallet,
}

impl Side {
    fn engine<'a>(
        &'a mut self,
        src: &'a Local,
        btc: &'a MockBtc,
        chain: &'a Chain,
    ) -> Engine<'a, Local<'a>> {
        let blocks: &'a dyn Fn(u64) -> Result<Vec<GnrTx>> = Box::leak(Box::new(move |h: u64| {
            Ok(chain.block_at(h)?.context("block")?.txs)
        }));
        Engine {
            wallet: &mut self.wallet,
            gnr: src,
            btc,
            btc_network: BtcNet::Regtest,
            timing: Timing::TEST,
            block_txs: blocks,
        }
    }
    fn swap(&self) -> Swap {
        load(&self.wallet).pop().unwrap()
    }
}

fn setup(dir: &std::path::Path, chain: &Chain, src: &Local) -> (Side, Side) {
    let mk = |name: &str| {
        Wallet::create(
            &dir.join(name),
            "password-x",
            &keys::new_mnemonic(),
            "",
            Network::Devnet,
            0,
            krypton_crypto::kdf::Argon2Params::TEST,
        )
        .unwrap()
    };
    let mut gnr_seller = Side {
        wallet: mk("gnr-seller"),
    };
    let mut btc_seller = Side {
        wallet: mk("btc-seller"),
    };
    for _ in 0..12 {
        mine(src, &gnr_seller.wallet.address());
    }
    let _ = chain;
    gnr_seller.wallet.sync(src, |_, _| {}).unwrap();
    btc_seller.wallet.sync(src, |_, _| {}).unwrap();
    (gnr_seller, btc_seller)
}

fn sync(src: &Local, sides: &mut [&mut Side]) {
    for s in sides {
        s.wallet.sync(src, |_, _| {}).unwrap();
    }
}

const GNR: u64 = 100_000_000;
const SATS: u64 = 50_000;

fn mine_both(src: &Local, miner: &GnrAddress, a: &mut Side, b: &mut Side) {
    mine(src, miner);
    sync(src, &mut [a, b]);
}

/// Request and accept: the seller has sent its lock, the buyer waits for it.
fn agreed(a: &mut Side, b: &mut Side, src: &Local, btc: &MockBtc, chain: &Chain) {
    let req = b
        .engine(src, btc, chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    assert_eq!(req.stage, Stage::Requested);
    assert!(
        a.engine(src, btc, chain)
            .accept(&req.code.clone().unwrap(), "not an address")
            .is_err()
    );
    let acc = a
        .engine(src, btc, chain)
        .accept(req.code.as_ref().unwrap(), &btc_addr(1).to_string())
        .unwrap();
    assert!(
        a.engine(src, btc, chain)
            .accept(req.code.as_ref().unwrap(), &btc_addr(1).to_string())
            .is_err(),
        "twice"
    );
    let s = b
        .engine(src, btc, chain)
        .receive(acc.code.as_ref().unwrap())
        .unwrap();
    assert_eq!(s.stage, Stage::Accepted);
    assert!(
        s.htlc_address().is_err(),
        "no payment address before the lock"
    );
}

/// Until the buyer may pay: the seller locks, the lock confirms. Returns the address.
fn locked(a: &mut Side, b: &mut Side, src: &Local, btc: &MockBtc, chain: &Chain) -> Address {
    agreed(a, b, src, btc, chain);
    let miner = a.wallet.address();
    // The buyer sees nothing to pay until the seller's lock is on chain and confirmed.
    b.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Accepted);
    a.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Locked, "{}", a.swap().status);
    assert_eq!(src.pool.borrow().len(), 1, "lock transaction sent");
    assert!(
        !a.swap().cancellable(),
        "the seller cannot walk away once locked"
    );
    mine_both(src, &miner, a, b);
    b.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Accepted, "{}", b.swap().status);
    assert!(
        b.swap().status.contains("of 2 confirmations"),
        "{}",
        b.swap().status
    );
    mine_both(src, &miner, a, b);
    b.engine(src, btc, chain).step(&[]).unwrap();
    let s = b.swap();
    assert_eq!(s.stage, Stage::AwaitDeposit, "{}", s.status);
    assert!(s.pay_by.is_some());
    s.htlc_address().unwrap()
}

/// The buyer pays and sends the deposit code; the seller records it.
fn paid(a: &mut Side, b: &mut Side, src: &Local, btc: &MockBtc, chain: &Chain) -> Address {
    let htlc = locked(a, b, src, btc, chain);
    btc.pay(&htlc, SATS);
    let s = b.engine(src, btc, chain).step(&[]).unwrap().pop().unwrap();
    assert_eq!(s.stage, Stage::Deposited, "{}", s.status);
    let s = a
        .engine(src, btc, chain)
        .receive(s.code.as_ref().unwrap())
        .unwrap();
    assert_eq!(s.stage, Stage::Deposited);
    htlc
}

fn new_chain(dir: &std::path::Path) -> Chain {
    Chain::open(&dir.join("chain"), Network::Devnet).unwrap()
}

#[test]
fn swap_completes_gnr_first() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let b_before = b.wallet.balance().total;
    paid(&mut a, &mut b, &src, &btc, &chain);
    assert_eq!(a.swap().role, Role::GnrSeller);
    // The seller waits for a bitcoin confirmation, then takes it with its secret.
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Deposited);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    assert!(btc.received(&btc_addr(1)) > SATS - 2_000);
    // The buyer reads the secret from that bitcoin transaction and claims the GNR.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Claiming, "{}", b.swap().status);
    assert!(b.swap().learned.is_some());
    mine_both(&src, &miner, &mut a, &mut b);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Done, "{}", a.swap().status);
    assert_eq!(b.swap().stage, Stage::Done, "{}", b.swap().status);
    assert_eq!(
        b.wallet.balance().total,
        b_before + GNR - b.swap().claim_fee
    );
    assert_eq!(btc.received(&btc_addr(2)), 0, "no refund to the buyer");
}

/// The seller's wallet stops right after it sent the lock, before the swap was written
/// down. When it runs again the swap carries on with that lock: no second lock is sent,
/// and the first is not left without the swap that would refund it.
#[test]
fn a_lock_sent_before_the_wallet_stopped_is_carried_on_with() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    agreed(&mut a, &mut b, &src, &btc, &chain);
    // The round that locks, cut off after the lock went out: the record still says
    // "accepted".
    let s = a.swap();
    assert_eq!((s.stage, s.lock_cm.clone()), (Stage::Accepted, None));
    let (_, terms) = a
        .wallet
        .lock_funds(
            &src,
            s.index,
            s.hash,
            s.gnr_refund_height.unwrap(),
            s.claim_outputs,
            s.gnr,
            s.refund_fee.unwrap(),
            krypton_wallet_core::DEFAULT_FEE,
            None,
        )
        .unwrap();
    assert_eq!(Some(terms.clone()), s.terms());
    assert_eq!(src.pool.borrow().len(), 1);
    assert_eq!(a.wallet.locked_balance(), GNR);

    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    let s = a.swap();
    assert_eq!(s.stage, Stage::Locked, "{}", s.status);
    assert_eq!(s.lock_cm, Some(hex::encode(terms.commitment(GNR))));
    assert_eq!(src.pool.borrow().len(), 1, "no second lock");
    assert_eq!(a.wallet.locked_balance(), GNR);
    // From here it is a swap like any other: the lock confirms, the buyer may pay, and
    // if nobody pays the seller gets the GNR back.
    mine_both(&src, &miner, &mut a, &mut b);
    mine_both(&src, &miner, &mut a, &mut b);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::AwaitDeposit, "{}", b.swap().status);
    let before = a.wallet.balance().total;
    let other = b.wallet.address();
    while chain.tip().height() + 1 < s.gnr_refund_height.unwrap() {
        mine(&src, &other);
    }
    a.wallet.sync(&src, |_, _| {}).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Refunding, "{}", a.swap().status);
    mine(&src, &other);
    a.wallet.sync(&src, |_, _| {}).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Refunded, "{}", a.swap().status);
    assert_eq!(a.wallet.locked_balance(), 0);
    assert!(a.wallet.balance().total >= before + GNR - s.refund_fee.unwrap());
}

#[test]
fn a_seller_without_the_gnr_gets_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    // The seller accepts but never locks (it has no GNR, or walks away).
    agreed(&mut a, &mut b, &src, &btc, &chain);
    for _ in 0..4 {
        mine_both(&src, &miner, &mut a, &mut b);
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
        assert_eq!(b.swap().stage, Stage::Accepted);
        assert!(b.swap().htlc_address().is_err(), "never told where to pay");
    }
    // After three hours the buyer's swap ends; nothing was ever paid.
    let mut v = load(&b.wallet);
    v[0].created -= krypton_swap::engine::DEPOSIT_TIMEOUT_SECS + 10;
    krypton_swap::engine::save(&mut b.wallet, &v).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Failed, "{}", b.swap().status);
    assert_eq!(btc.broadcasts(), 0);
    assert!(b.swap().deposit.is_none());
}

#[test]
fn a_lock_of_less_gnr_is_not_paid() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    agreed(&mut a, &mut b, &src, &btc, &chain);
    // A cheating seller locks half the amount under the agreed terms.
    let mut v = load(&a.wallet);
    v[0].gnr = GNR / 2;
    krypton_swap::engine::save(&mut a.wallet, &v).unwrap();
    // It cannot even make one: the claim outputs pin the buyer's amount, so a smaller
    // lock does not balance and is refused.
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Accepted, "{}", a.swap().status);
    assert!(src.pool.borrow().is_empty(), "no lock sent");
    for _ in 0..4 {
        mine_both(&src, &miner, &mut a, &mut b);
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
    }
    assert_eq!(b.swap().stage, Stage::Accepted, "{}", b.swap().status);
    assert!(
        b.swap().htlc_address().is_err(),
        "the buyer never pays for a smaller lock"
    );
}

#[test]
fn a_buyer_who_never_pays_costs_the_seller_nothing() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = b.wallet.address();
    locked(&mut a, &mut b, &src, &btc, &chain);
    assert_eq!(a.wallet.locked_balance(), GNR);
    let a_before = a.wallet.balance().total;
    let refund_at = a.swap().gnr_refund_height.unwrap();
    while chain.tip().height() + 1 < refund_at {
        mine(&src, &miner);
    }
    a.wallet.sync(&src, |_, _| {}).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Refunding, "{}", a.swap().status);
    mine(&src, &miner);
    a.wallet.sync(&src, |_, _| {}).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Refunded, "{}", a.swap().status);
    assert_eq!(a.wallet.locked_balance(), 0);
    assert!(a.wallet.balance().total >= a_before + GNR - a.swap().refund_fee.unwrap());
    // The buyer's payment window closed long ago.
    let mut v = load(&b.wallet);
    v[0].pay_by = Some(1);
    krypton_swap::engine::save(&mut b.wallet, &v).unwrap();
    b.wallet.sync(&src, |_, _| {}).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Failed, "{}", b.swap().status);
}

/// The GNR came back through another copy of the seller's wallet (the same wallet on a
/// second computer, or restored from its phrase): this copy's swap ends as refunded instead
/// of waiting for a refund that can no longer happen.
#[test]
fn a_lock_refunded_by_another_copy_ends_the_swap() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = b.wallet.address();
    locked(&mut a, &mut b, &src, &btc, &chain);
    let refund_at = a.swap().gnr_refund_height.unwrap();
    while chain.tip().height() + 1 < refund_at {
        mine(&src, &miner);
    }
    a.wallet.sync(&src, |_, _| {}).unwrap();
    let cm = a.wallet.locks()[0].cm;
    a.wallet.refund_lock(&src, &cm).unwrap();
    mine(&src, &miner);
    a.wallet.sync(&src, |_, _| {}).unwrap();
    assert!(a.wallet.locks()[0].spent_height.is_some());
    assert_eq!(
        a.swap().stage,
        Stage::Locked,
        "the engine did not send that refund"
    );
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Refunded, "{}", a.swap().status);
    assert_eq!(a.wallet.locked_balance(), 0);
}

#[test]
fn a_seller_who_never_takes_the_bitcoin_refunds_the_buyer() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    paid(&mut a, &mut b, &src, &btc, &chain);
    btc.mine();
    let refund_btc = b.swap().btc_refund_height.unwrap();
    while btc.tip().unwrap() < refund_btc {
        btc.mine();
    }
    // The seller went silent; the buyer's bitcoin comes back at the timelock.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunded);
    assert!(btc.received(&btc_addr(2)) > SATS - 2_000);
    // Too late now for the seller: it does not take the (gone) bitcoin.
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_ne!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
}

#[test]
fn pay_late_and_cancel_rules() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let htlc = locked(&mut a, &mut b, &src, &btc, &chain);
    let id = b.swap().id;
    assert!(
        a.engine(&src, &btc, &chain).cancel(&id, "x").is_err(),
        "locked GNR"
    );
    // The buyer may still walk away before paying.
    let s = b
        .engine(&src, &btc, &chain)
        .cancel(&id, "Cancelled by you.")
        .unwrap();
    assert_eq!(s.stage, Stage::Failed);
    // Paid anyway: the wallet watches the contract and refunds at the timelock.
    btc.pay(&htlc, SATS);
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Deposited, "{}", b.swap().status);
    while btc.tip().unwrap() < b.swap().btc_refund_height.unwrap() {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
}

#[test]
fn old_requests_and_unsafe_terms_are_refused() {
    use krypton_swap::engine::Msg;
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let req = b
        .engine(&src, &btc, &chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    let Msg::Request2 {
        id,
        claim_outputs,
        refund_pub,
        ..
    } = Msg::decode(req.code.as_ref().unwrap()).unwrap()
    else {
        panic!("a GNR-first request")
    };
    let old = Msg::Request {
        id: id.clone(),
        gnr_network: Network::Devnet,
        btc_network: "regtest".into(),
        gnr: GNR,
        sats: SATS,
        hash: [7; 32],
        claim_fee: krypton_wallet_core::DEFAULT_FEE,
        claim_outputs,
        refund_pub,
    };
    let e = a
        .engine(&src, &btc, &chain)
        .accept(&old.encode(), &btc_addr(1).to_string())
        .unwrap_err();
    assert!(e.to_string().contains("older than 1.0.23"), "{e}");
    // An accept whose GNR lock would last more than a week is refused by the buyer.
    let tip = chain.tip().height();
    let long = Msg::Accept2 {
        id,
        hash: [9; 32],
        claim_pub: public_key(&derive_key(&[3; 64], 1)).to_string(),
        gnr_refund_height: tip + 20_000,
        refund_outputs: [
            krypton_core::lock::HexCm([1; 32]),
            krypton_core::lock::HexCm([2; 32]),
        ],
        refund_fee: krypton_wallet_core::DEFAULT_FEE,
    };
    let e = b
        .engine(&src, &btc, &chain)
        .receive(&long.encode())
        .unwrap_err();
    assert!(e.to_string().contains("week"), "{e}");
    // Confirmations grow with the size of the swap.
    let t = Timing::MAINNET;
    // By what the buyer is about to pay: 0.0002 BTC, 0.001, 0.01, 0.1 and more.
    assert_eq!(
        [20_000, 100_000, 100_001, 1_000_000, 10_000_000, 10_000_001].map(|s| t.gnr_confs_for(s)),
        [3, 3, 6, 6, 15, 30]
    );
    assert_eq!(
        [500_000, 2_000_000, 10_000_001, 100_000_000, 100_000_001].map(|s| t.btc_confs_for(s)),
        [1, 2, 3, 3, 4]
    );
    assert_eq!(Timing::TEST.btc_confs_for(100_000_001), 1);
    assert_eq!(Timing::TEST.gnr_confs_for(50_000_000), 2);
    // The pay check: a week at most, and the GNR must outlast the bitcoin.
    let ok = krypton_swap::Timelocks {
        btc_refund_height: 1_036,
        gnr_refund_height: 10_000 + 1_868,
    };
    assert!(t.check_pay(&ok, 1_000, 10_000).is_ok());
    let short = krypton_swap::Timelocks {
        gnr_refund_height: 10_000 + 900,
        ..ok
    };
    assert!(t.check_pay(&short, 1_000, 10_000).is_err());
    let week = krypton_swap::Timelocks {
        gnr_refund_height: 10_000 + 11_000,
        ..ok
    };
    assert!(t.check_pay(&week, 1_000, 10_000).is_err());
    // The shorter locks of 1.0.43: a bitcoin lock of 18 blocks needs 1,080 GNR blocks
    // left when the buyer pays; the seller's 1,260 leave 180 for the confirmations.
    let fast = |left: u64| krypton_swap::Timelocks {
        btc_refund_height: 1_000 + t.btc_lock2_blocks,
        gnr_refund_height: 10_000 + left,
    };
    assert_eq!(t.btc_lock2_blocks, 18);
    assert!(t.check_pay(&fast(1_080), 1_000, 10_000).is_ok());
    assert!(t.check_pay(&fast(1_079), 1_000, 10_000).is_err());
    assert!(
        t.check_pay(&fast(t.gnr_lock2_fast_blocks - 40), 1_000, 10_000)
            .is_ok()
    );
    // What a wallet before 1.0.43 asks for (36 blocks, 6 hours to claim): 1,720.
    assert!(t.gnr_lock2_blocks >= 1_720 + 150);
}

/// The length of the seller's lock follows the buyer's wallet: the shorter lock for a
/// wallet that knows it, the old one for a wallet that would refuse anything shorter.
/// And a buyer whose bitcoin is deposited can prove it to the seller.
#[test]
fn the_lock_follows_the_buyers_wallet_and_a_deposit_can_be_proven() {
    use krypton_swap::engine::{Msg, PROTOCOL, prepaid_address};
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let tip = chain.tip().height();
    let t = Timing::TEST;
    assert!(t.gnr_lock2_fast_blocks < t.gnr_lock2_blocks);

    // A wallet of today, paying by hand: the short lock, and no proof of a deposit.
    let req = b
        .engine(&src, &btc, &chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    let code = req.code.clone().unwrap();
    let Msg::Request2 { v, prepaid, .. } = Msg::decode(&code).unwrap() else {
        panic!("a request")
    };
    assert_eq!((v, prepaid), (PROTOCOL, None));
    assert_eq!(prepaid_address(&code, BtcNet::Regtest).unwrap(), None);
    let s = a
        .engine(&src, &btc, &chain)
        .accept(&code, &btc_addr(1).to_string())
        .unwrap();
    assert_eq!(s.gnr_refund_height, Some(tip + t.gnr_lock2_fast_blocks));
    assert_eq!(s.lock_blocks, Some(t.gnr_lock2_fast_blocks));
    assert!(!s.prepaid);
    // The buyer accepts that lock.
    b.engine(&src, &btc, &chain)
        .receive(s.code.as_ref().unwrap())
        .unwrap();

    // The same request as a wallet before 1.0.43 would send it: no version. It would
    // refuse a lock shorter than the old one, so it gets the old one.
    let mut old: serde_json::Value = serde_json::to_value(Msg::decode(&code).unwrap()).unwrap();
    old.as_object_mut().unwrap().remove("v");
    old["id"] = "11".repeat(16).into();
    let old: Msg = serde_json::from_value(old).unwrap();
    let s = a
        .engine(&src, &btc, &chain)
        .accept(&old.encode(), &btc_addr(1).to_string())
        .unwrap();
    assert_eq!(s.gnr_refund_height, Some(tip + t.gnr_lock2_blocks));

    // A buyer whose bitcoin waits at deposit address 3 of its wallet says so, signed by
    // that address's key. The proof names the address and fits this request only.
    let req = b
        .engine(&src, &btc, &chain)
        .request_from(GNR, SATS, &btc_addr(2).to_string(), Some(3))
        .unwrap();
    assert!(req.prepaid);
    let code = req.code.clone().unwrap();
    let dep = b.engine(&src, &btc, &chain).deposit_address(3);
    assert_eq!(prepaid_address(&code, BtcNet::Regtest).unwrap(), Some(dep));
    let mut other: serde_json::Value = serde_json::to_value(Msg::decode(&code).unwrap()).unwrap();
    other["sats"] = (SATS + 1).into();
    let other: Msg = serde_json::from_value(other).unwrap();
    assert!(prepaid_address(&other.encode(), BtcNet::Regtest).is_err());
    let mut moved: serde_json::Value = serde_json::to_value(Msg::decode(&code).unwrap()).unwrap();
    moved["id"] = "22".repeat(16).into();
    let moved: Msg = serde_json::from_value(moved).unwrap();
    assert!(prepaid_address(&moved.encode(), BtcNet::Regtest).is_err());
    // The seller's wallet records what it was told it checked.
    let s = a
        .engine(&src, &btc, &chain)
        .accept_from(&code, &btc_addr(1).to_string(), None, true)
        .unwrap();
    assert!(s.prepaid);
}

#[test]
fn codes_roundtrip_and_reject_garbage() {
    use krypton_swap::engine::Msg;
    let m = Msg::Funded {
        id: "ab".into(),
        btc_refund_height: 7,
        txid: "00".repeat(32),
        vout: 1,
    };
    let code = m.encode();
    assert!(code.starts_with("gks1"));
    assert_eq!(
        Msg::decode(&format!("  {}\n", code.to_uppercase())).unwrap(),
        m
    );
    assert!(Msg::decode("gks1abc").is_err());
    assert!(Msg::decode("hello").is_err());
}

/// A sell offer proves its GNR. The coins are set aside in a note of their own; the proof
/// fits that offer only and no block can ever include it; whoever checks it can see the
/// moment the coins move; and a take locks exactly those coins.
#[test]
fn a_sell_offer_proves_its_gnr() {
    use krypton_core::consensus::StateView;
    use krypton_swap::board::{self, Offer, Side as OfferSide};
    use krypton_wallet_core::{Aside, DEFAULT_FEE};
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = b.wallet.address();
    let offer = |gnr: u64, id: u8| Offer {
        id: hex::encode([id; 16]),
        network: "devnet".into(),
        btc_network: "regtest".into(),
        side: OfferSide::SellGnr,
        gnr,
        sats: SATS,
        key: hex::encode(board::offer_key(&[7; 64], 1).public().to_bytes()),
        expires: krypton_core::now() + 3600,
        cancel: board::cancel_hash(&[id; 32]),
        nonce: 0,
    };
    let o = offer(GNR, 1);
    let (owner, rcm) = board::reserve_output(&o);
    let h32 = |s: &str| <[u8; 32]>::try_from(hex::decode(s).unwrap()).unwrap();
    let prove = |w: &Wallet, cm: &Hash32, fee: u64, expiry: u64| {
        w.prove_reserve(&src, cm, owner, o.gnr, rcm, fee, expiry)
    };

    // The offer's amount and the fee of the lock a take needs go into one note.
    let need = GNR + board::RESERVE_FEE;
    let Aside::Pending { cm, .. } = a.wallet.set_aside(&src, need, DEFAULT_FEE, &o.id).unwrap()
    else {
        panic!("a new note was needed");
    };
    assert!(
        prove(&a.wallet, &cm, board::RESERVE_FEE, board::RESERVE_EXPIRY).is_err(),
        "nothing to prove before the note is in a block"
    );
    mine_both(&src, &miner, &mut a, &mut b);
    assert_eq!(a.wallet.balance().held, need);

    let tx = prove(&a.wallet, &cm, board::RESERVE_FEE, board::RESERVE_EXPIRY).unwrap();
    let backing = board::check_reserve(&o, &tx).expect("a sound proof of funds");
    // What a board checks against its own chain: the proof is for this chain, and the
    // notes behind it are unspent.
    {
        let v = chain.view().unwrap();
        assert!(v.anchor_height(&h32(&backing.anchor)).is_some());
        assert!(
            backing
                .nullifiers
                .iter()
                .all(|n| !v.nullifier_spent(&h32(n)))
        );
    }
    // Nothing moved, and nothing can: no node takes this as a transaction.
    assert!(chain.check_mempool_tx(&tx).is_err());
    assert_eq!(a.wallet.balance().held, need);
    assert_eq!(a.wallet.balance().pending, 0);

    // It fits this offer only: not another amount, another offer, or a buy offer.
    assert!(board::check_reserve(&offer(GNR + 1, 1), &tx).is_err());
    assert!(board::check_reserve(&offer(GNR, 2), &tx).is_err());
    let mut buy = o.clone();
    buy.side = OfferSide::BuyGnr;
    assert!(board::check_reserve(&buy, &tx).is_err());
    // The wallet makes no proof that a block could still include (0 never expires).
    let tip = chain.tip().height();
    for expiry in [0, tip + 1, tip + 100] {
        assert!(prove(&a.wallet, &cm, board::RESERVE_FEE, expiry).is_err());
    }
    // And a board takes none: a transfer that could be mined is not a proof of funds,
    // nor is one that leaves nothing for the fee of the lock, nor one that was changed
    // afterwards.
    for expiry in [0, 2, tip + 1, tip + 100] {
        let mut minable = tx.clone();
        minable.expiry_height = expiry;
        let why = board::check_reserve(&o, &minable).unwrap_err().to_string();
        assert!(why.contains("can be mined"), "{why}");
    }
    assert!(
        board::check_reserve(
            &o,
            &prove(
                &a.wallet,
                &cm,
                board::RESERVE_FEE - 1,
                board::RESERVE_EXPIRY
            )
            .unwrap()
        )
        .is_err()
    );
    let mut forged = tx.clone();
    forged.nullifiers.swap(0, 1);
    assert!(board::check_reserve(&o, &forged).is_err());
    // A wallet that does not have the coins cannot make one at all.
    assert!(prove(&b.wallet, &cm, board::RESERVE_FEE, board::RESERVE_EXPIRY).is_err());

    // A buyer takes the offer: the seller's lock spends the note behind it, so whoever
    // watches the proof's nullifiers sees the offer is no longer open.
    let req = b
        .engine(&src, &btc, &chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    let acc = a
        .engine(&src, &btc, &chain)
        .accept_from(
            req.code.as_ref().unwrap(),
            &btc_addr(1).to_string(),
            Some(cm),
            false,
        )
        .unwrap();
    assert_eq!(acc.from_note, Some(hex::encode(cm)));
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Locked, "{}", a.swap().status);
    mine_both(&src, &miner, &mut a, &mut b);
    {
        let v = chain.view().unwrap();
        assert!(
            backing
                .nullifiers
                .iter()
                .any(|n| v.nullifier_spent(&h32(n)))
        );
    }
    assert_eq!(a.wallet.balance().held, 0);
    assert!(a.wallet.holds().is_empty());
    assert!(a.wallet.note(&cm).is_none(), "spent by the lock");
    // The same coins cannot be proven again.
    assert!(prove(&a.wallet, &cm, board::RESERVE_FEE, board::RESERVE_EXPIRY).is_err());
}

/// A buyer who paid the bitcoin of a buy offer in advance does nothing more: once the
/// seller's GNR is locked and confirmed, the wallet pays the contract from the deposit,
/// exactly the agreed amount, and what was set aside for the fee and not used goes back
/// to the buyer's own address.
#[test]
fn a_deposit_pays_the_contract_by_itself() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let (payout, home) = (btc_addr(1), btc_addr(2));
    let reserve = krypton_swap::deposit::fee_reserve(btc.fee_rate().unwrap());
    let dep = b.engine(&src, &btc, &chain).deposit_address(4);
    assert_ne!(dep, b.engine(&src, &btc, &chain).deposit_address(5));
    // The buyer pays in two parts; the second is not confirmed yet.
    btc.pay(&dep, SATS);
    btc.mine();
    btc.pay(&dep, reserve);
    assert_eq!(
        b.engine(&src, &btc, &chain).deposit_coins(4).unwrap().len(),
        2
    );

    let req = b
        .engine(&src, &btc, &chain)
        .request_from(GNR, SATS, &home.to_string(), Some(4))
        .unwrap();
    let acc = a
        .engine(&src, &btc, &chain)
        .accept(req.code.as_ref().unwrap(), &payout.to_string())
        .unwrap();
    b.engine(&src, &btc, &chain)
        .receive(acc.code.as_ref().unwrap())
        .unwrap();
    // Nothing is paid before the seller's GNR is locked and confirmed.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Locked, "{}", a.swap().status);
    mine_both(&src, &miner, &mut a, &mut b);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Accepted, "{}", b.swap().status);
    assert_eq!(btc.broadcasts(), 0, "one confirmation is not enough");
    mine_both(&src, &miner, &mut a, &mut b);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::AwaitDeposit, "{}", b.swap().status);
    // The next round pays, by itself.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    let s = b.swap();
    assert!(s.paid_tx.is_some(), "{}", s.status);
    assert!(!s.cancellable(), "paid: no walking away");
    assert_eq!(btc.broadcasts(), 2, "both parts of the deposit were spent");
    let contract = s.htlc_address().unwrap();
    assert_eq!(btc.received(&contract), SATS, "exactly the agreed amount");
    let back = btc.received(&home);
    assert!(back > 0 && back < reserve, "unused fee came back: {back}");
    assert!(
        b.engine(&src, &btc, &chain)
            .deposit_coins(4)
            .unwrap()
            .is_empty()
    );
    // A second round pays nothing more, and the swap goes on as any other.
    let s = b
        .engine(&src, &btc, &chain)
        .step(&[])
        .unwrap()
        .pop()
        .unwrap();
    assert_eq!(s.stage, Stage::Deposited, "{}", s.status);
    assert_eq!(btc.broadcasts(), 2);
    assert_eq!(s.deposit.as_ref().unwrap().sats, SATS);
    a.engine(&src, &btc, &chain)
        .receive(s.code.as_ref().unwrap())
        .unwrap();
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Claiming, "{}", b.swap().status);
    mine_both(&src, &miner, &mut a, &mut b);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Done, "{}", a.swap().status);
    assert_eq!(b.swap().stage, Stage::Done, "{}", b.swap().status);
    assert!(btc.received(&payout) > SATS - 2_000);
    assert_eq!(
        btc.received(&home),
        back,
        "nothing else came back: it was a sale"
    );
}

/// The bitcoin of a buy offer that ends unsold goes back whole, and a deposit that does
/// not cover the payment is never spent on a part of it.
#[test]
fn an_unsold_deposit_goes_back_and_a_short_one_is_not_spent() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let home = btc_addr(2);

    // Unsold: two payments, one of them unconfirmed, go back in one transaction.
    let dep = b.engine(&src, &btc, &chain).deposit_address(7);
    btc.pay(&dep, 40_000);
    btc.mine();
    btc.pay(&dep, 11_000);
    let mut e = b.engine(&src, &btc, &chain);
    assert!(e.sweep_deposit(7, "not an address").is_err());
    let id = e.sweep_deposit(7, &home.to_string()).unwrap();
    assert!(id.is_some());
    let got = btc.received(&home);
    assert!(
        got > 51_000 - 1_000 && got < 51_000,
        "all but the fee: {got}"
    );
    assert_eq!(e.sweep_deposit(7, &home.to_string()).unwrap(), None);
    assert_eq!(e.sweep_deposit(8, &home.to_string()).unwrap(), None);

    // Short: the offer's amount is there, but nothing for the fee. Nothing is paid.
    let dep = b.engine(&src, &btc, &chain).deposit_address(9);
    btc.pay(&dep, SATS);
    btc.mine();
    let req = b
        .engine(&src, &btc, &chain)
        .request_from(GNR, SATS, &home.to_string(), Some(9))
        .unwrap();
    let acc = a
        .engine(&src, &btc, &chain)
        .accept(req.code.as_ref().unwrap(), &btc_addr(1).to_string())
        .unwrap();
    b.engine(&src, &btc, &chain)
        .receive(acc.code.as_ref().unwrap())
        .unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    mine_both(&src, &miner, &mut a, &mut b);
    mine_both(&src, &miner, &mut a, &mut b);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::AwaitDeposit, "{}", b.swap().status);
    let spent = btc.broadcasts();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    let s = b.swap();
    assert_eq!(s.stage, Stage::AwaitDeposit);
    assert!(s.paid_tx.is_none() && btc.broadcasts() == spent);
    // The wallet says that it pays by itself once the bitcoin is there. Nobody is asked to
    // pay by hand, and no address to pay is named: a payment made by hand beside the
    // wallet's own would be a second one.
    assert_eq!(s.status, engine::SHORT_NOW);
    assert!(!s.status.contains(&s.htlc_address().unwrap().to_string()));
    assert!(s.cancellable());
    // The window passes with nothing paid: the swap ends, and says what happened.
    {
        let keep = load(&b.wallet);
        let mut v = keep.clone();
        v.last_mut().unwrap().pay_by = Some(krypton_core::now() - 60);
        krypton_swap::engine::save(&mut b.wallet, &v).unwrap();
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
        let s = b.swap();
        assert_eq!(s.stage, Stage::Failed, "{}", s.status);
        assert!(s.status.starts_with("Ended unpaid"), "{}", s.status);
        assert!(!s.status.contains("pay now"), "{}", s.status);
        assert_eq!(btc.broadcasts(), spent, "nothing was paid");
        krypton_swap::engine::save(&mut b.wallet, &keep).unwrap();
    }
    // A payment the wallet sent itself, which the Bitcoin server does not show yet, is
    // waited for past the payment window; not for ever.
    let keep = load(&b.wallet);
    let now = krypton_core::now();
    for (late, stage) in [
        (60, Stage::AwaitDeposit),
        (krypton_swap::engine::PAID_UNSEEN_SECS + 60, Stage::Failed),
    ] {
        let mut v = keep.clone();
        let s = v.last_mut().unwrap();
        s.paid_tx = Some("sent".into());
        s.pay_by = Some(now - late);
        krypton_swap::engine::save(&mut b.wallet, &v).unwrap();
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
        assert_eq!(b.swap().stage, stage, "{}", b.swap().status);
    }
    krypton_swap::engine::save(&mut b.wallet, &keep).unwrap();
    // The buyer adds what is missing: the next round pays.
    btc.pay(&dep, 2_000);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert!(b.swap().paid_tx.is_some(), "{}", b.swap().status);
    assert_eq!(btc.received(&b.swap().htlc_address().unwrap()), SATS);
}

/// Two purchases are paid from the same bitcoin, one after the other in the same round.
/// The second payment spends the change of the first, which the wallet knows from the
/// payment it built: a Bitcoin server that has not listed that change yet does not make
/// the wallet think its bitcoin is gone (1.0.57; it used to say so, and to ask for a
/// payment by hand that would have been a second one).
#[test]
fn a_second_payment_follows_the_first_at_once() {
    use krypton_swap::deposit::BALANCE;
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    btc.mine();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let payout = btc_addr(1).to_string();
    let own = b.engine(&src, &btc, &chain).deposit_address(BALANCE);
    let first = btc.pay(&own, 3 * SATS);
    btc.mine();
    for _ in 0..2 {
        let req = b
            .engine(&src, &btc, &chain)
            .request_from(GNR, SATS, &own.to_string(), Some(BALANCE))
            .unwrap();
        let acc = a
            .engine(&src, &btc, &chain)
            .accept_seen(req.code.as_ref().unwrap(), &payout, None)
            .unwrap();
        b.engine(&src, &btc, &chain)
            .receive(acc.code.as_ref().unwrap())
            .unwrap();
    }
    // Both locks are sent and confirm (the second spends the change of the first).
    for _ in 0..12 {
        a.engine(&src, &btc, &chain).step(&[]).unwrap();
        mine_both(&src, &miner, &mut a, &mut b);
        let l = a.wallet.locks();
        let tip = chain.tip().height();
        if l.len() == 2 && l.iter().all(|x| x.height.is_some_and(|h| tip + 1 - h >= 2)) {
            break;
        }
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    let swaps = load(&b.wallet);
    assert_eq!(swaps.len(), 2);
    for s in &swaps {
        assert_eq!(s.stage, Stage::AwaitDeposit, "{}", s.status);
        assert!(s.paid_tx.is_none());
    }
    // Before anything is paid, none of it is change.
    let coins = b.engine(&src, &btc, &chain).deposit_coins(BALANCE).unwrap();
    assert_eq!(coins.len(), 1);
    assert!(!b.engine(&src, &btc, &chain).is_change(BALANCE, &coins[0]));

    // The Bitcoin server falls behind. One round pays both swaps.
    btc.lag(true);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    let swaps = load(&b.wallet);
    for s in &swaps {
        assert!(s.paid_tx.is_some(), "{}", s.status);
        assert!(!s.status.contains("by hand"), "{}", s.status);
    }
    assert_eq!(btc.broadcasts(), 2, "two payments, each spending one coin");
    let (t1, t2) = (
        Txid::from_str(swaps[0].paid_tx.as_ref().unwrap()).unwrap(),
        Txid::from_str(swaps[1].paid_tx.as_ref().unwrap()).unwrap(),
    );
    btc.lag(false);
    let (tx1, tx2) = (btc.tx(&t1).unwrap().unwrap(), btc.tx(&t2).unwrap().unwrap());
    assert_eq!(tx1.input[0].previous_output, first);
    assert_eq!(
        tx2.input[0].previous_output,
        OutPoint::new(t1, 1),
        "the second payment spends the change of the first"
    );
    for s in &swaps {
        assert_eq!(btc.received(&s.htlc_address().unwrap()), SATS);
    }
    // What is left is change of the wallet's own payments, and the wallet knows it: from
    // its own record, and from the chain alone (as the same wallet on another computer
    // would see it).
    let left = btc.received(&own);
    assert!(left > SATS - 2_000 && left < SATS, "{left}");
    let coins = b.engine(&src, &btc, &chain).deposit_coins(BALANCE).unwrap();
    assert_eq!(coins.len(), 1);
    assert_eq!(coins[0].outpoint, OutPoint::new(t2, 1));
    assert_eq!(coins[0].height, None);
    assert!(b.engine(&src, &btc, &chain).is_change(BALANCE, &coins[0]));
    assert!(
        !a.engine(&src, &btc, &chain).is_change(BALANCE, &coins[0]),
        "another wallet's address: not its change"
    );
    let record = b.wallet.app_data("btc_own").unwrap().clone();
    b.wallet
        .set_app_data("btc_own", serde_json::json!([]))
        .unwrap();
    assert!(
        b.engine(&src, &btc, &chain).is_change(BALANCE, &coins[0]),
        "everything that payment spends was at this address"
    );
    b.wallet.set_app_data("btc_own", record).unwrap();
    // Bitcoin someone else sends is not change.
    let gift = btc.pay(&own, 7_000);
    let coins = b.engine(&src, &btc, &chain).deposit_coins(BALANCE).unwrap();
    let g = coins.iter().find(|c| c.outpoint == gift).unwrap();
    assert!(!b.engine(&src, &btc, &chain).is_change(BALANCE, g));
    // The program stopped after a payment went out and before the swap's record said so,
    // and the Bitcoin server does not show the payment yet: the wallet wrote the payment
    // down with the payment itself, and does not pay that contract a second time.
    btc.unlist(t1);
    let mut v = load(&b.wallet);
    let was = v[0].paid_tx.take();
    v[0].status = "stopped before this was written".into();
    krypton_swap::engine::save(&mut b.wallet, &v).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(btc.broadcasts(), 2, "no second payment into the contract");
    let s = &load(&b.wallet)[0];
    assert_eq!(s.paid_tx, was, "{}", s.status);
    assert_eq!(s.stage, Stage::AwaitDeposit);
    btc.lag(false);
    // Nothing is paid a second time, and both swaps go on.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(btc.broadcasts(), 2);
    for s in load(&b.wallet) {
        assert_eq!(s.stage, Stage::Deposited, "{}", s.status);
    }
}

/// Change the Bitcoin server never lists is not spent for ever on the wallet's own word,
/// and change that another copy of the wallet spent is not spent again.
#[test]
fn change_is_taken_on_the_wallets_word_only_for_a_while() {
    use krypton_swap::deposit::BALANCE;
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    btc.mine();
    let (_a, mut b) = setup(dir.path(), &chain, &src);
    let own = b.engine(&src, &btc, &chain).deposit_address(BALANCE);
    btc.pay(&own, 3 * SATS);
    btc.mine();
    // A payment from the balance, with change back to it; the server does not list it.
    btc.lag(true);
    let id = b
        .engine(&src, &btc, &chain)
        .pay_from(BALANCE, &[(btc_addr(5), SATS)], BALANCE)
        .unwrap();
    let change = OutPoint::new(id, 1);
    let coins = |b: &mut Side| b.engine(&src, &btc, &chain).deposit_coins(BALANCE).unwrap();
    assert_eq!(
        coins(&mut b).iter().map(|c| c.outpoint).collect::<Vec<_>>(),
        [change]
    );
    // Half an hour later the server still does not list it: the payment was dropped, and
    // the wallet no longer counts on its change.
    let mut own_list = b.wallet.app_data("btc_own").unwrap().clone();
    for e in own_list.as_array_mut().unwrap() {
        e[3] = (krypton_core::now() - 31 * 60).into();
    }
    b.wallet.set_app_data("btc_own", own_list.clone()).unwrap();
    assert!(coins(&mut b).is_empty());
    // Listed by the server: counted once, not twice.
    btc.lag(false);
    for e in own_list.as_array_mut().unwrap() {
        e[3] = krypton_core::now().into();
    }
    b.wallet.set_app_data("btc_own", own_list).unwrap();
    assert_eq!(coins(&mut b).len(), 1);
    // Spent by the same wallet on another computer (this copy has no record of that):
    // the server says so, and the change is not counted here.
    let key = krypton_swap::deposit::key(b.wallet.swap_seed(), BALANCE);
    let c = coins(&mut b);
    let away = krypton_swap::deposit::spend(
        &c,
        &key,
        None,
        &btc_addr(6).script_pubkey(),
        btc.fee_rate().unwrap(),
    )
    .unwrap();
    btc.broadcast(&away).unwrap();
    assert!(coins(&mut b).is_empty());

    // A server that lists only what is in a block (Bitcoin Core) never shows change
    // before its block. It is asked about the change itself, and for longer: the answer
    // is the server's, not only the wallet's own word.
    btc.mine();
    btc.blocks_only(true);
    btc.pay(&own, 3 * SATS);
    assert!(
        coins(&mut b).is_empty(),
        "a deposit is listed with its block"
    );
    btc.mine();
    let id = b
        .engine(&src, &btc, &chain)
        .pay_from(BALANCE, &[(btc_addr(5), SATS)], BALANCE)
        .unwrap();
    let change = OutPoint::new(id, 1);
    let ops = |b: &mut Side| -> Vec<OutPoint> { coins(b).iter().map(|c| c.outpoint).collect() };
    assert_eq!(ops(&mut b), [change]);
    let mut own_list = b.wallet.app_data("btc_own").unwrap().clone();
    for e in own_list.as_array_mut().unwrap() {
        e[3] = (krypton_core::now() - 3 * 3600).into();
    }
    b.wallet.set_app_data("btc_own", own_list).unwrap();
    assert_eq!(
        ops(&mut b),
        [change],
        "hours later, while the server says it is there"
    );
    // A payment that was dropped is not counted on: the server does not know its change.
    btc.evict(id);
    assert!(ops(&mut b).iter().all(|op| *op != change));
}

/// The same wallet is open on two computers and one of them sells GNR. The other copy
/// has no record of that swap: it sees the GNR as locked elsewhere, leaves it alone
/// while the swap can still run, and takes it back if it is still locked well after the
/// lock ran out (the computer that ran the swap never came back).
#[test]
fn gnr_locked_by_another_copy_of_the_wallet_is_seen_and_taken_back() {
    use krypton_swap::engine::ELSEWHERE_REFUND_DELAY;
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    // The second computer: the same wallet file, as it was before the swap.
    std::fs::copy(dir.path().join("gnr-seller"), dir.path().join("copy")).unwrap();
    let mut copy = Side {
        wallet: Wallet::open(&dir.path().join("copy"), "password-x").unwrap(),
    };
    locked(&mut a, &mut b, &src, &btc, &chain);
    assert!(a.engine(&src, &btc, &chain).elsewhere().is_empty());
    copy.wallet.sync(&src, |_, _| {}).unwrap();
    assert!(load(&copy.wallet).is_empty(), "the copy has no swap");
    let seen = copy.engine(&src, &btc, &chain).elsewhere();
    assert_eq!(seen.len(), 1);
    let refund_at = a.swap().gnr_refund_height.unwrap();
    assert_eq!((seen[0].value, seen[0].refund_height), (GNR, refund_at));
    assert_eq!(copy.wallet.locked_balance(), GNR);
    // While the swap can run, and for a while after its lock ran out, the copy waits.
    let other = b.wallet.address();
    while chain.tip().height() + 1 < refund_at + ELSEWHERE_REFUND_DELAY - 1 {
        mine(&src, &other);
    }
    copy.wallet.sync(&src, |_, _| {}).unwrap();
    assert_eq!(copy.engine(&src, &btc, &chain).refund_elsewhere(), 0);
    mine(&src, &other);
    copy.wallet.sync(&src, |_, _| {}).unwrap();
    let before = copy.wallet.balance().total;
    assert_eq!(copy.engine(&src, &btc, &chain).refund_elsewhere(), 1);
    assert_eq!(
        copy.engine(&src, &btc, &chain).refund_elsewhere(),
        0,
        "sent once"
    );
    assert!(copy.engine(&src, &btc, &chain).elsewhere()[0].refunding);
    mine(&src, &other);
    copy.wallet.sync(&src, |_, _| {}).unwrap();
    assert!(copy.engine(&src, &btc, &chain).elsewhere().is_empty());
    assert_eq!(copy.wallet.locked_balance(), 0);
    assert!(copy.wallet.balance().total >= before + GNR - a.swap().refund_fee.unwrap());
}

/// A Bitcoin server that does not answer.
struct Down;

impl BtcChain for Down {
    fn tip(&self) -> Result<u32> {
        bail!("connection refused")
    }
    fn outputs(&self, _: &Address) -> Result<Vec<Utxo>> {
        bail!("connection refused")
    }
    fn is_spent(&self, _: &OutPoint) -> Result<bool> {
        bail!("connection refused")
    }
    fn spender(&self, _: &OutPoint, _: u32) -> Result<Option<Transaction>> {
        bail!("connection refused")
    }
    fn broadcast(&self, _: &Transaction) -> Result<Txid> {
        bail!("connection refused")
    }
    fn fee_rate(&self) -> Result<bitcoin::FeeRate> {
        bail!("connection refused")
    }
}

/// While the Bitcoin server cannot be reached a swap says so, and a swap that ran out
/// still ends: nothing stays open for as long as the server is away.
#[test]
fn a_swap_ends_in_time_without_a_bitcoin_server() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    agreed(&mut a, &mut b, &src, &btc, &chain);
    let blocks = |h: u64| -> Result<Vec<GnrTx>> { Ok(chain.block_at(h)?.context("block")?.txs) };
    let down = |w: &mut Wallet| {
        Engine {
            wallet: w,
            gnr: &src,
            btc: &Down,
            btc_network: BtcNet::Regtest,
            timing: Timing::TEST,
            block_txs: &blocks,
        }
        .step(&[])
        .expect("a server that is away is not an error of the round")
    };
    for side in [&mut a, &mut b] {
        let s = down(&mut side.wallet).pop().unwrap();
        assert_eq!(s.stage, Stage::Accepted);
        assert!(
            s.status.contains("Bitcoin server did not answer"),
            "{}",
            s.status
        );
    }
    assert_eq!(src.pool.borrow().len(), 0, "nothing is locked meanwhile");
    // Hours later the server is still away. Both swaps end; nothing was locked or paid.
    for side in [&mut a, &mut b] {
        let mut v = load(&side.wallet);
        v[0].created -= 5 * 3600;
        krypton_swap::engine::save(&mut side.wallet, &v).unwrap();
        let s = down(&mut side.wallet).pop().unwrap();
        assert_eq!(s.stage, Stage::Failed, "{}", s.status);
        assert!(s.ended_unlocked());
    }
}

/// The buyer pays `sats` and sends the deposit code; the seller records it.
fn paid_sats(
    a: &mut Side,
    b: &mut Side,
    src: &Local,
    btc: &MockBtc,
    chain: &Chain,
    sats: u64,
) -> Address {
    let htlc = locked(a, b, src, btc, chain);
    btc.pay(&htlc, sats);
    let s = b.engine(src, btc, chain).step(&[]).unwrap().pop().unwrap();
    assert_eq!(s.stage, Stage::Deposited, "{}", s.status);
    let s = a
        .engine(src, btc, chain)
        .receive(s.code.as_ref().unwrap())
        .unwrap();
    assert_eq!(s.stage, Stage::Deposited);
    htlc
}

fn funding_of(s: &Swap) -> krypton_swap::Funding {
    let d = s.deposit.clone().unwrap();
    krypton_swap::Funding {
        outpoint: OutPoint::new(std::str::FromStr::from_str(&d.txid).unwrap(), d.vout),
        value: Amount::from_sat(d.sats),
    }
}

/// The seller holds its claim back and, when the buyer's wallet refunds at the bitcoin lock,
/// replaces that refund with its claim. The buyer's wallet reads the secret from that claim
/// and claims the GNR, which is still locked for it: the buyer ends with the GNR, never
/// with a swap marked refunded while the bitcoin went to the seller.
#[test]
fn a_refund_replaced_by_the_sellers_claim_still_gets_the_buyer_its_gnr() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    paid_sats(&mut a, &mut b, &src, &btc, &chain, SATS);
    btc.mine();
    let refund_btc = b.swap().btc_refund_height.unwrap();
    while btc.tip().unwrap() < refund_btc {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
    let s = a.swap();
    let claim = s
        .htlc()
        .unwrap()
        .claim(
            funding_of(&s),
            &btc_addr(1).script_pubkey(),
            FeeRate::from_sat_per_vb(20).unwrap(),
            &derive_key(a.wallet.swap_seed(), s.index),
            &derive_secret(a.wallet.swap_seed(), s.index),
        )
        .unwrap();
    btc.replace(&claim).unwrap();
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert!(
        b.swap().learned.is_some(),
        "the buyer learns the secret from the claim"
    );
    assert_ne!(b.swap().stage, Stage::Refunded, "{}", b.swap().status);
    for _ in 0..4 {
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
        mine_both(&src, &miner, &mut a, &mut b);
    }
    assert_eq!(b.swap().stage, Stage::Done, "{}", b.swap().status);
}

/// A claim the seller makes is done only once it is in a block: while it waits, a claim
/// that left the mempool is sent again (the secret is out, so the buyer could otherwise
/// refund after the lock and keep both the bitcoin and the GNR).
#[test]
fn the_sellers_claim_is_done_only_in_a_block_and_is_sent_again_when_dropped() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    paid_sats(&mut a, &mut b, &src, &btc, &chain, SATS);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    let first: Txid = std::str::FromStr::from_str(a.swap().btc_spend.as_deref().unwrap()).unwrap();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(
        a.swap().stage,
        Stage::Claiming,
        "not done while unconfirmed: {}",
        a.swap().status
    );
    btc.evict(first);
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert!(
        btc.is_spent(&funding_of(&a.swap()).outpoint).unwrap(),
        "the claim is sent again"
    );
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Done, "{}", a.swap().status);
    assert!(btc.received(&btc_addr(1)) > 0);
}

/// A buyer who pays more than agreed does not block the seller's claim: it signs for the
/// amount in the contract.
#[test]
fn an_overpaid_contract_can_still_be_claimed() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    paid_sats(&mut a, &mut b, &src, &btc, &chain, SATS + 1);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
}

/// The secret is found in any valid claim, also one whose branch argument is not the
/// minimal 0x01 (relay policy, not consensus, asks for that).
#[test]
fn the_secret_is_found_in_a_claim_with_a_non_minimal_branch_argument() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    paid_sats(&mut a, &mut b, &src, &btc, &chain, SATS);
    let s = a.swap();
    let mut claim = s
        .htlc()
        .unwrap()
        .claim(
            funding_of(&s),
            &btc_addr(1).script_pubkey(),
            FeeRate::from_sat_per_vb(3).unwrap(),
            &derive_key(a.wallet.swap_seed(), s.index),
            &derive_secret(a.wallet.swap_seed(), s.index),
        )
        .unwrap();
    let w: Vec<Vec<u8>> = claim.input[0].witness.iter().map(|x| x.to_vec()).collect();
    assert_eq!(w.len(), 4);
    let odd = vec![w[0].clone(), w[1].clone(), vec![0x02], w[3].clone()];
    claim.input[0].witness = bitcoin::Witness::from_slice(&odd);
    let found = b.swap().htlc().unwrap().find_secret(&claim);
    assert_eq!(
        found.map(|x| *x.as_bytes()),
        Some(*derive_secret(a.wallet.swap_seed(), s.index).as_bytes())
    );
}

/// The seller took the bitcoin, but the buyer's wallet never claimed the GNR: once the
/// lock ends the seller's wallet takes it back, even though its swap is done.
#[test]
fn a_done_sellers_gnr_comes_back_when_the_buyer_never_claims() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    paid(&mut a, &mut b, &src, &btc, &chain);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Done, "{}", a.swap().status);
    // The buyer is gone: nobody claims the GNR. Mine past the lock's refund height.
    let refund_at = a.swap().gnr_refund_height.unwrap();
    while chain.tip().height() < refund_at {
        mine(&src, &miner);
    }
    a.wallet.sync(&src, |_, _| {}).unwrap();
    let pool = src.pool.borrow().len();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(
        src.pool.borrow().len(),
        pool + 1,
        "the refund is sent: {}",
        a.swap().status
    );
    assert!(a.swap().status.contains("refunded"), "{}", a.swap().status);
}

/// A swap record this version cannot read is kept as it was when the others are saved,
/// never dropped: its bitcoin can only be recovered with it.
#[test]
fn unreadable_swap_records_are_kept() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    agreed(&mut a, &mut b, &src, &btc, &chain);
    let mut stored = b.wallet.app_data("swaps").cloned().unwrap();
    let future = serde_json::json!({"id": "from-a-newer-wallet", "stage": "something_new"});
    stored.as_array_mut().unwrap().push(future.clone());
    b.wallet.set_app_data("swaps", stored).unwrap();
    let swaps = load(&b.wallet);
    assert_eq!(swaps.len(), 1, "the readable one");
    krypton_swap::engine::save(&mut b.wallet, &swaps).unwrap();
    let after = b.wallet.app_data("swaps").cloned().unwrap();
    assert_eq!(after.as_array().unwrap().len(), 2);
    assert!(after.as_array().unwrap().contains(&future));
}

/// The buyer's bitcoin refund waits in a busy mempool: it is signed again with more fee
/// as the blocks go by, and the buyer gets its bitcoin back.
#[test]
fn a_refund_waiting_in_a_busy_mempool_gets_more_fee() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    paid(&mut a, &mut b, &src, &btc, &chain);
    btc.mine();
    let refund_btc = b.swap().btc_refund_height.unwrap();
    while btc.tip().unwrap() < refund_btc {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
    let first = b.swap().btc_spend.unwrap();
    let first_fee = btc.fee_of(&first);
    btc.stall(true);
    for _ in 0..4 {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    let second = b.swap().btc_spend.unwrap();
    assert_ne!(second, first, "signed again: {}", b.swap().status);
    assert!(btc.fee_of(&second) > first_fee, "with more fee");
    btc.stall(false);
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunded, "{}", b.swap().status);
    assert!(btc.received(&btc_addr(2)) > SATS - 3_000);
}

/// Bitcoin paid into the contract a second time (and not used by the swap) comes back to
/// the buyer once the bitcoin lock has ended, after the swap itself completed.
#[test]
fn extra_bitcoin_left_in_the_contract_comes_back() {
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let htlc = paid(&mut a, &mut b, &src, &btc, &chain);
    btc.pay(&htlc, 30_000);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    mine_both(&src, &miner, &mut a, &mut b);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Done, "{}", b.swap().status);
    let before = btc.received(&btc_addr(2));
    // Before the lock ends the extra bitcoin stays where it is.
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(btc.received(&btc_addr(2)), before);
    let refund_btc = b.swap().btc_refund_height.unwrap();
    while btc.tip().unwrap() < refund_btc {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    btc.mine();
    assert!(
        btc.received(&btc_addr(2)) > before + 30_000 - 2_000,
        "{}",
        b.swap().status
    );
    assert_eq!(b.swap().stage, Stage::Done, "the swap stays done");
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert!(b.swap().extras_done, "nothing left to look for");
}

/// A deposit whose sender paid too little waits in the mempool. The wallet says why, and
/// can pay the difference itself: it sends the waiting coin on to its own address at a fee
/// that makes the two payments worth a miner's while together. Nothing but the fee leaves
/// the wallet, the fee has a ceiling, and a server cannot talk it into more.
#[test]
fn a_waiting_deposit_is_explained_and_can_be_sped_up() {
    use krypton_swap::deposit::BALANCE;
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let (_, mut buyer) = setup(dir.path(), &chain, &src);
    let btc = MockBtc::default();
    btc.mine();
    let mut e = buyer.engine(&src, &btc, &chain);
    let addr = e.deposit_address(BALANCE);
    // Nothing there: nothing waits, nothing to speed up.
    assert_eq!(e.waiting(BALANCE).unwrap(), None);
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_NONE));

    // 0.005 BTC arrives at 2 sat/vB while blocks take 3.
    btc.estimates(&[(1, 3.0), (3, 2.4), (6, 2.0), (144, 0.5)]);
    let op = btc.pay_waiting(&addr, 500_000, 282, 141, true);
    let w = e.waiting(BALANCE).unwrap().expect("it waits");
    assert_eq!((w.coins, w.sats), (1, 500_000));
    assert_eq!(w.rate, Some(2.0));
    assert_eq!(w.next, Some(3.0));
    assert_eq!(w.blocks, Some(6), "its fee is enough within six blocks");

    let plan = e.bump_plan(BALANCE).unwrap().expect("a plan");
    assert_eq!(plan.coins.len(), 1);
    assert_eq!(plan.coins[0].outpoint, op);
    assert_eq!(plan.value, 500_000);
    assert_eq!(plan.together, 4, "a fifth above the next block, rounded up");
    assert!(plan.fee <= plan.value / engine::BUMP_FEE_SHARE);
    let id = e.bump_deposit(BALANCE, &plan).unwrap();
    {
        let s = btc.inner.lock().unwrap();
        let tx = &s.txs[&id];
        assert_eq!(tx.input.len(), 1);
        assert_eq!(tx.input[0].previous_output, op);
        assert_eq!(tx.output.len(), 1, "everything goes to one place");
        assert_eq!(
            tx.output[0].script_pubkey,
            addr.script_pubkey(),
            "this wallet's own address"
        );
        assert_eq!(tx.output[0].value.to_sat(), 500_000 - plan.fee);
        // Together the two pay what was planned, by their real sizes.
        let together = (282 + plan.fee) as f64 / (141 + tx.vsize() as u64) as f64;
        assert!(together >= 4.0, "{together}");
        assert!(together < 4.5, "and not much more: {together}");
    }
    // What waits now is the wallet's own payment, counted together with the one it spends.
    let w = e.waiting(BALANCE).unwrap().expect("still waiting");
    assert!(w.together);
    assert!(
        w.rate.is_some_and(|r| (4.0..4.5).contains(&r)),
        "{:?}",
        w.rate
    );
    assert_eq!(w.blocks, Some(1), "enough for the next block");
    // The balance is the new coin now, still waiting; the wallet's own payment is not sped
    // up a second time.
    let coins = e.deposit_coins(BALANCE).unwrap();
    assert_eq!(coins.len(), 1);
    assert_eq!(coins[0].outpoint.txid, id);
    assert!(coins[0].height.is_none());
    assert_eq!(coins[0].value.to_sat(), 500_000 - plan.fee);
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_NONE));
    assert!(
        e.bump_deposit(BALANCE, &plan).is_err(),
        "the old coin is spent"
    );
    btc.mine();
    assert_eq!(e.waiting(BALANCE).unwrap(), None);
    assert!(e.deposit_coins(BALANCE).unwrap()[0].height.is_some());

    // When it is not offered, and why.
    let op2 = btc.pay_waiting(&addr, 300_000, 705, 141, true);
    assert_eq!(
        e.bump_plan(BALANCE).unwrap(),
        Err(engine::BUMP_ENOUGH),
        "5 sat/vB is enough"
    );
    btc.inner
        .lock()
        .unwrap()
        .waiting
        .get_mut(&op2.txid)
        .unwrap()
        .fee = 141;
    btc.inner
        .lock()
        .unwrap()
        .waiting
        .get_mut(&op2.txid)
        .unwrap()
        .parents_confirmed = false;
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_PARENTS));
    let w = e.waiting(BALANCE).unwrap().unwrap();
    assert_eq!(
        (w.sats, w.rate, w.blocks),
        (300_000, None, None),
        "its own fee does not say when"
    );
    btc.inner.lock().unwrap().waiting.remove(&op2.txid);
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_UNKNOWN));
    // A server that reports absurd fees cannot make the wallet burn the deposit: the plan
    // is refused, and a plan made by hand is refused too.
    btc.inner.lock().unwrap().waiting.insert(
        op2.txid,
        Unconfirmed {
            fee: 141,
            vsize: 141,
            parents_confirmed: true,
            seen: None,
        },
    );
    btc.estimates(&[(1, 480.0)]);
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_COSTLY));
    btc.estimates(&[(1, 3.0)]);
    let mut plan = e
        .bump_plan(BALANCE)
        .unwrap()
        .expect("a plan at ordinary fees");
    let before = btc.broadcasts();
    plan.rate = 400;
    plan.fee = 40_000;
    assert!(e.bump_deposit(BALANCE, &plan).is_err());
    let mut confirmed = e.bump_plan(BALANCE).unwrap().unwrap();
    confirmed.coins = e
        .deposit_coins(BALANCE)
        .unwrap()
        .into_iter()
        .filter(|c| c.height.is_some())
        .collect();
    assert!(
        e.bump_deposit(BALANCE, &confirmed).is_err(),
        "bitcoin in a block is left alone"
    );
    assert_eq!(btc.broadcasts(), before, "nothing was sent");
    // Dust that someone sends to the address, in a payment that cannot be sped up, does
    // not stand in the way of the deposit: it is left to wait, the deposit is sent on.
    let dust = btc.pay_waiting(&addr, 700, 10, 141, false);
    let plan = e
        .bump_plan(BALANCE)
        .unwrap()
        .expect("the deposit is still sped up");
    assert_eq!(plan.coins.len(), 1);
    assert_eq!((plan.coins[0].outpoint, plan.value), (op2, 300_000));
    assert!(!plan.coins.iter().any(|c| c.outpoint == dust));
    let w = e.waiting(BALANCE).unwrap().unwrap();
    assert_eq!(w.coins, 2);
    assert_eq!(w.rate, None, "one of the two cannot be told");
    // A deposit too small to carry the fee is left to wait.
    btc.mine();
    let _ = btc.pay_waiting(&addr, 9_000, 141, 141, true);
    assert_eq!(e.bump_plan(BALANCE).unwrap(), Err(engine::BUMP_COSTLY));
}

/// A wallet sells only to a buyer whose bitcoin it can see. A request that names no
/// bitcoin, names bitcoin that is not there or not in a block yet, or was changed after it
/// was signed, is refused before anything is accepted: no GNR is locked on a promise.
#[test]
fn a_seller_locks_only_for_bitcoin_it_can_see() {
    use krypton_swap::deposit::BALANCE;
    use krypton_swap::engine::Msg;
    let dir = tempfile::tempdir().unwrap();
    let chain = new_chain(dir.path());
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    btc.mine();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let payout = btc_addr(1).to_string();

    // Paying by hand: the request names no bitcoin.
    let by_hand = b
        .engine(&src, &btc, &chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    let e = a
        .engine(&src, &btc, &chain)
        .accept_seen(by_hand.code.as_ref().unwrap(), &payout, None)
        .unwrap_err();
    assert_eq!(e.to_string(), engine::NOT_SHOWN);

    // From the balance, but the balance is empty.
    let own = b.engine(&src, &btc, &chain).deposit_address(BALANCE);
    let req = b
        .engine(&src, &btc, &chain)
        .request_from(GNR, SATS, &own.to_string(), Some(BALANCE))
        .unwrap();
    let code = req.code.clone().unwrap();
    let e = a
        .engine(&src, &btc, &chain)
        .accept_seen(&code, &payout, None)
        .unwrap_err();
    assert!(e.to_string().contains("is not there"), "{e}");

    // Bitcoin on its way does not count; too little does not count.
    btc.pay(&own, SATS);
    assert!(
        a.engine(&src, &btc, &chain)
            .accept_seen(&code, &payout, None)
            .is_err(),
        "not in a block yet"
    );
    btc.mine();

    // A request changed after it was signed: more GNR for the same bitcoin, another
    // amount of bitcoin, another swap. The proof fits none of them.
    for (field, value) in [
        ("gnr", serde_json::Value::from(GNR * 10)),
        ("sats", (SATS - 1).into()),
        ("id", "33".repeat(16).into()),
    ] {
        let mut forged: serde_json::Value =
            serde_json::to_value(Msg::decode(&code).unwrap()).unwrap();
        forged[field] = value;
        let forged: Msg = serde_json::from_value(forged).unwrap();
        assert!(
            a.engine(&src, &btc, &chain)
                .accept_seen(&forged.encode(), &payout, None)
                .is_err(),
            "{field}"
        );
    }
    // A proof lifted from one request does not carry another.
    let other = b
        .engine(&src, &btc, &chain)
        .request(GNR, SATS, &own.to_string())
        .unwrap();
    let mut lifted: serde_json::Value =
        serde_json::to_value(Msg::decode(other.code.as_ref().unwrap()).unwrap()).unwrap();
    lifted["prepaid"] =
        serde_json::to_value(Msg::decode(&code).unwrap()).unwrap()["prepaid"].clone();
    let lifted: Msg = serde_json::from_value(lifted).unwrap();
    assert!(
        a.engine(&src, &btc, &chain)
            .accept_seen(&lifted.encode(), &payout, None)
            .is_err()
    );
    assert!(load(&a.wallet).is_empty(), "nothing was accepted so far");
    assert!(a.wallet.locks().is_empty(), "and nothing locked");

    // The bitcoin is there, confirmed: accepted, and marked as seen.
    let s = a
        .engine(&src, &btc, &chain)
        .accept_seen(&code, &payout, None)
        .unwrap();
    assert!(s.prepaid);
    assert_eq!(s.stage, Stage::Accepted);

    // Once that bitcoin has moved, the same balance carries no further request.
    let req2 = b
        .engine(&src, &btc, &chain)
        .request_from(GNR, SATS, &own.to_string(), Some(BALANCE))
        .unwrap();
    let away = btc_addr(9).to_string();
    b.engine(&src, &btc, &chain)
        .sweep_deposit(BALANCE, &away)
        .unwrap()
        .expect("swept");
    btc.mine();
    let e = a
        .engine(&src, &btc, &chain)
        .accept_seen(req2.code.as_ref().unwrap(), &payout, None)
        .unwrap_err();
    assert!(e.to_string().contains("is not there"), "{e}");

    // A Bitcoin server that does not answer is no proof either.
    let blocks = |h: u64| -> Result<Vec<GnrTx>> { Ok(chain.block_at(h)?.context("block")?.txs) };
    let e = Engine {
        wallet: &mut a.wallet,
        gnr: &src,
        btc: &Down,
        btc_network: BtcNet::Regtest,
        timing: Timing::TEST,
        block_txs: &blocks,
    }
    .accept_seen(req2.code.as_ref().unwrap(), &payout, None)
    .unwrap_err();
    assert!(format!("{e:#}").contains("did not answer"), "{e:#}");
    assert_eq!(
        load(&a.wallet).len(),
        1,
        "only the one swap whose bitcoin was seen"
    );
}

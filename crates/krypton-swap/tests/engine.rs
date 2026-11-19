//! Two wallets swap BTC for GNR through the engine: a real GNR devnet chain with real
//! proofs, and a simulated Bitcoin chain that runs every transaction through Bitcoin
//! Core's script interpreter and enforces nLockTime.

use anyhow::{Context, Result, bail};
use bitcoin::hashes::Hash;
use bitcoin::key::CompressedPublicKey;
use bitcoin::{Address, Amount, Network as BtcNet, OutPoint, Transaction, TxOut, Txid};
use krypton_core::address::Address as GnrAddress;
use krypton_core::types::{BlockHeader, Transaction as GnrTx};
use krypton_core::{Hash32, Network};
use krypton_crypto::field::{Digest, digest_to_bytes};
use krypton_light::CompactBlock;
use krypton_store::Chain;
use krypton_swap::btc::{BtcChain, Utxo};
use krypton_swap::engine::{Engine, Role, Stage, Swap, load};
use krypton_swap::{Timing, derive_key, public_key};
use krypton_wallet_core::{ChainSource, Wallet, keys};
use std::cell::RefCell;
use std::collections::HashMap;
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
    fn mine(&self) {
        let mut s = self.inner.lock().unwrap();
        s.height += 1;
        let h = s.height;
        for u in s.utxos.values_mut() {
            u.1.get_or_insert(h);
        }
        s.mempool.clear();
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
}

impl BtcChain for MockBtc {
    fn tip(&self) -> Result<u32> {
        Ok(self.inner.lock().unwrap().height)
    }
    fn outputs(&self, address: &Address) -> Result<Vec<Utxo>> {
        let s = self.inner.lock().unwrap();
        Ok(s.utxos
            .iter()
            .filter(|(_, (o, _))| o.script_pubkey == address.script_pubkey())
            .map(|(op, (o, h))| Utxo {
                outpoint: *op,
                value: o.value,
                height: *h,
            })
            .collect())
    }
    fn is_spent(&self, op: &OutPoint) -> Result<bool> {
        Ok(self.inner.lock().unwrap().spent.contains_key(op))
    }
    fn spender(&self, op: &OutPoint, _from: u32) -> Result<Option<Transaction>> {
        let s = self.inner.lock().unwrap();
        Ok(s.spent.get(op).and_then(|id| s.txs.get(id)).cloned())
    }
    fn broadcast(&self, tx: &Transaction) -> Result<Txid> {
        let mut s = self.inner.lock().unwrap();
        for i in &tx.input {
            if s.spent.contains_key(&i.previous_output) {
                bail!("bad-txns-inputs-missingorspent");
            }
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
        Ok(id)
    }
    fn fee_rate(&self) -> Result<bitcoin::FeeRate> {
        Ok(bitcoin::FeeRate::from_sat_per_vb(3).unwrap())
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
        (t.btc_confs_for(500_000), t.btc_confs_for(2_000_000)),
        (1, 2)
    );
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
    let e = b.engine(&src, &btc, &chain);
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
    assert!(s.status.contains("no longer covers"), "{}", s.status);
    assert!(s.cancellable());
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

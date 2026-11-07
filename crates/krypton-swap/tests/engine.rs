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
    assert_eq!(t.gnr_confs_for(10 * GNR), 6);
    assert_eq!(t.gnr_confs_for(100 * GNR), 15);
    assert_eq!(t.gnr_confs_for(5_000 * GNR), 30);
    assert_eq!(
        (t.btc_confs_for(500_000), t.btc_confs_for(2_000_000)),
        (1, 2)
    );
    assert_eq!(Timing::TEST.gnr_confs_for(10 * GNR), 2);
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

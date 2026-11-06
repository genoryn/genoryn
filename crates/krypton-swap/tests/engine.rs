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

/// Request, accept and deposit: returns the HTLC address.
fn open_swap(a: &mut Side, b: &mut Side, src: &Local, btc: &MockBtc, chain: &Chain) -> Address {
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
    assert_eq!(s.stage, Stage::AwaitDeposit);
    let htlc = s.htlc_address().unwrap();
    // Nothing happens before the deposit.
    b.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::AwaitDeposit);
    btc.pay(&htlc, SATS);
    let s = b.engine(src, btc, chain).step(&[]).unwrap().pop().unwrap();
    assert_eq!(s.stage, Stage::Deposited);
    let s = a
        .engine(src, btc, chain)
        .receive(s.code.as_ref().unwrap())
        .unwrap();
    assert_eq!(s.stage, Stage::Deposited);
    // The GNR seller waits for a confirmation before locking anything.
    a.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Deposited);
    assert!(src.pool.borrow().is_empty());
    btc.mine();
    a.engine(src, btc, chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Locked);
    assert_eq!(src.pool.borrow().len(), 1, "lock transaction sent");
    htlc
}

#[test]
fn swap_completes() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    let b_before = b.wallet.balance().total;
    open_swap(&mut a, &mut b, &src, &btc, &chain);
    assert_eq!(a.swap().role, Role::GnrSeller);

    mine(&src, &miner);
    sync(&src, &mut [&mut a, &mut b]);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Locked, "{}", b.swap().status);
    mine(&src, &miner);
    sync(&src, &mut [&mut a, &mut b]);
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Claiming, "{}", b.swap().status);

    // The secret reaches the GNR seller through the mempool; it claims the bitcoin.
    let pool = src.pool.borrow().clone();
    a.engine(&src, &btc, &chain).step(&pool).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    assert!(btc.received(&btc_addr(1)) > SATS - 2_000);

    mine(&src, &miner);
    sync(&src, &mut [&mut a, &mut b]);
    btc.mine();
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Done, "{}", a.swap().status);
    assert_eq!(b.swap().stage, Stage::Done, "{}", b.swap().status);
    assert_eq!(
        b.wallet.balance().total,
        b_before + GNR - b.swap().claim_fee
    );
    assert_eq!(btc.received(&btc_addr(2)), 0, "no refund to the BTC seller");
}

#[test]
fn secret_from_a_block_when_the_mempool_was_missed() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = a.wallet.address();
    open_swap(&mut a, &mut b, &src, &btc, &chain);
    for _ in 0..2 {
        mine(&src, &miner);
        sync(&src, &mut [&mut a, &mut b]);
        b.engine(&src, &btc, &chain).step(&[]).unwrap();
    }
    assert_eq!(b.swap().stage, Stage::Claiming);
    mine(&src, &miner);
    sync(&src, &mut [&mut a, &mut b]);
    a.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(a.swap().stage, Stage::Claiming, "{}", a.swap().status);
    assert!(btc.received(&btc_addr(1)) > 0);
}

#[test]
fn both_sides_refund_when_nobody_claims() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let miner = b.wallet.address();
    open_swap(&mut a, &mut b, &src, &btc, &chain);
    mine(&src, &miner);
    a.wallet.sync(&src, |_, _| {}).unwrap();
    let locked = a.wallet.locked_balance();
    assert_eq!(locked, GNR);
    let a_before = a.wallet.balance().total;
    // The BTC seller goes silent. The GNR seller refunds at the GNR refund height.
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
    // Mining payouts accrued earlier keep arriving too, so look for the refund note itself.
    let refunded = GNR - a.swap().refund_fee.unwrap();
    assert!(a.wallet.balance().total >= a_before + refunded);
    assert!(
        a.wallet
            .notes()
            .iter()
            .any(|n| n.value == refunded && n.memo.ends_with("GNR refunded"))
    );

    // Back online too late, the BTC seller does not claim and refunds the bitcoin.
    b.wallet.sync(&src, |_, _| {}).unwrap();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_ne!(b.swap().stage, Stage::Claiming);
    let refund_btc = b.swap().btc_refund_height.unwrap();
    while btc.tip().unwrap() + 1 < refund_btc {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_ne!(
        b.swap().stage,
        Stage::Refunding,
        "not before nLockTime is final"
    );
    let n = btc.broadcasts();
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
    assert_eq!(btc.broadcasts(), n + 1);
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunded);
    assert!(btc.received(&btc_addr(2)) > SATS - 2_000);
}

/// Request, accept, receive: the buyer now waits for its own deposit.
fn agreed(a: &mut Side, b: &mut Side, src: &Local, btc: &MockBtc, chain: &Chain) -> Address {
    let req = b
        .engine(src, btc, chain)
        .request(GNR, SATS, &btc_addr(2).to_string())
        .unwrap();
    let acc = a
        .engine(src, btc, chain)
        .accept(req.code.as_ref().unwrap(), &btc_addr(1).to_string())
        .unwrap();
    b.engine(src, btc, chain)
        .receive(acc.code.as_ref().unwrap())
        .unwrap()
        .htlc_address()
        .unwrap()
}

#[test]
fn cancel_before_deposit_and_pay_anyway_refunds() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let htlc = agreed(&mut a, &mut b, &src, &btc, &chain);
    // Both sides may walk away while nothing is locked.
    let id = b.swap().id;
    let s = b
        .engine(&src, &btc, &chain)
        .cancel(&id, "Cancelled by you.")
        .unwrap();
    assert_eq!(s.stage, Stage::Failed);
    assert!(a.swap().cancellable());
    let s = a
        .engine(&src, &btc, &chain)
        .cancel(&id, "The other side cancelled.")
        .unwrap();
    assert_eq!(s.stage, Stage::Failed);
    assert!(
        a.engine(&src, &btc, &chain).cancel(&id, "again").is_err(),
        "already over"
    );
    // The buyer pays anyway: its wallet still watches the contract and refunds at the timelock.
    btc.pay(&htlc, SATS);
    btc.mine();
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Deposited, "{}", b.swap().status);
    while btc.tip().unwrap() < b.swap().btc_refund_height.unwrap() {
        btc.mine();
    }
    b.engine(&src, &btc, &chain).step(&[]).unwrap();
    assert_eq!(b.swap().stage, Stage::Refunding, "{}", b.swap().status);
    assert!(btc.received(&btc_addr(2)) > SATS - 2_000);
}

#[test]
fn no_cancel_after_deposit_and_expiry_without_one() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let btc = MockBtc::default();
    let (mut a, mut b) = setup(dir.path(), &chain, &src);
    let htlc = agreed(&mut a, &mut b, &src, &btc, &chain);
    btc.pay(&htlc, SATS);
    let id = b.swap().id;
    assert!(
        b.engine(&src, &btc, &chain).cancel(&id, "x").is_err(),
        "the deposit arrived"
    );
    // A second swap that nobody pays expires after three hours, on both sides.
    let htlc2 = agreed(&mut a, &mut b, &src, &btc, &chain);
    let _ = htlc2;
    for side in [&mut a, &mut b] {
        let mut v = load(&side.wallet);
        for s in v.iter_mut().filter(|s| s.deposit.is_none()) {
            s.created -= krypton_swap::engine::DEPOSIT_TIMEOUT_SECS + 3_700;
        }
        krypton_swap::engine::save(&mut side.wallet, &v).unwrap();
        side.engine(&src, &btc, &chain).step(&[]).unwrap();
        let s = side.swap();
        assert_eq!(s.stage, Stage::Failed, "{}", s.status);
        assert!(s.status.starts_with("Expired"), "{}", s.status);
    }
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

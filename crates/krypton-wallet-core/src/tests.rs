use super::*;
use krypton_crypto::kdf::Argon2Params;
use krypton_store::Chain;
use std::cell::RefCell;

struct Local<'a> {
    chain: &'a Chain,
    pool: RefCell<Vec<Transaction>>,
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
    fn frontier(&self, height: u64) -> Result<Frontier> {
        self.chain.frontier_at(height)?.context("no checkpoint")
    }
    fn witness(&self, pos: u64) -> Result<(Vec<Digest>, Digest)> {
        self.chain.auth_path(pos)
    }
    fn submit(&self, tx: &Transaction) -> Result<Hash32> {
        self.chain
            .check_mempool_tx(tx)
            .map_err(|e| anyhow::anyhow!("{e}"))?;
        self.pool.borrow_mut().push(tx.clone());
        Ok(tx.txid())
    }
    /// Exercises the full light-client path: a FlyClient proof built from the store.
    fn verified_tip(
        &self,
        params: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        _: Option<&BlockHeader>,
    ) -> Result<Option<BlockHeader>> {
        let tip = self.chain.tip();
        if tip.height() == 0 {
            return Ok(Some(tip.header));
        }
        let mut heights = krypton_light::sample_heights(tip.height(), 8, 3);
        heights.insert(0, 0);
        heights.dedup();
        let p = krypton_light::FlyProof {
            tip: tip.header.clone(),
            // As the node serves them: the difficulty window and a margin.
            recent: (tip
                .height()
                .saturating_sub(krypton_core::difficulty::WINDOW as u64 + 10)
                ..tip.height())
                .map(|h| self.chain.header_at(h).unwrap().unwrap())
                .collect(),
            samples: heights
                .iter()
                .map(|&h| krypton_light::FlySample {
                    header: self.chain.header_at(h).unwrap().unwrap(),
                    proof: self.chain.mmr_proof(h, tip.height()).unwrap().unwrap(),
                })
                .collect(),
        };
        let genesis = krypton_core::consensus::genesis_block(params).hash();
        krypton_light::verify_flyclient(params, &genesis, pow, &p, &heights)?;
        Ok(Some(p.tip))
    }
    fn header_in(&self, h: &BlockHeader, tip: &BlockHeader) -> Result<bool> {
        if h.height >= tip.height {
            return Ok(h.hash() == tip.hash());
        }
        let p = self
            .chain
            .mmr_proof(h.height, tip.height)?
            .context("proof")?;
        Ok(krypton_core::mmr::verify(
            &tip.mmr_root,
            &krypton_core::mmr::MmrNode::leaf(&h.hash(), h.difficulty),
            &p,
        ))
    }
    fn tip_work(&self, tip: &BlockHeader) -> Result<Option<u128>> {
        if tip.height == 0 {
            return Ok(Some(tip.difficulty));
        }
        let g = self.chain.header_at(0)?.context("genesis")?;
        let p = self.chain.mmr_proof(0, tip.height)?.context("proof")?;
        Ok(krypton_core::mmr::verify_work(
            &tip.mmr_root,
            &krypton_core::mmr::MmrNode::leaf(&g.hash(), g.difficulty),
            &p,
        )
        .map(|(_, total)| total + tip.difficulty))
    }
}

impl proof::ProofSource for Local<'_> {
    fn tx_height(&self, txid: &Hash32) -> Result<Option<u64>> {
        self.chain.tx_height(txid)
    }
    fn raw_block(&self, height: u64) -> Result<Block> {
        self.chain.block_at(height)?.context("no block")
    }
    fn proven_header(
        &self,
        height: u64,
        _: &krypton_core::ChainParams,
        _: &krypton_pow::Verifier,
    ) -> Result<BlockHeader> {
        self.chain.header_at(height)?.context("no header")
    }
    fn tip_height(&self) -> Result<u64> {
        Ok(self.chain.tip().height())
    }
}

fn mine(src: &Local, to: &Address) -> u64 {
    let c = src.chain;
    let h = c.tip().height() + 1;
    let txs: Vec<Transaction> = src.pool.borrow_mut().drain(..).collect();
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
    h
}

/// Sum of payouts made to `to` up to `max_height`.
fn paid_to(c: &Chain, to: &Address, max_height: u64) -> u64 {
    let o = digest_to_bytes(&to.owner);
    (0..=max_height.min(c.tip().height()))
        .flat_map(|h| c.payouts_at(h).unwrap())
        .filter(|p| p.owner == o)
        .map(|p| p.value)
        .sum()
}

#[test]
fn light_client_rejects_forged_data() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let to = Wallet::create(
        &dir.path().join("w"),
        "password-x",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap()
    .address();
    for _ in 0..12 {
        mine(&src, &to);
    }
    let tip = chain.tip();
    // FlyClient: sampled headers with MMR proofs against the tip.
    let heights = krypton_light::sample_heights(tip.height(), 8, 7);
    let mk = |hs: &[u64]| krypton_light::FlyProof {
        tip: tip.header.clone(),
        recent: (0..tip.height())
            .map(|h| chain.header_at(h).unwrap().unwrap())
            .collect(),
        samples: hs
            .iter()
            .map(|&h| krypton_light::FlySample {
                header: chain.header_at(h).unwrap().unwrap(),
                proof: chain.mmr_proof(h, tip.height()).unwrap().unwrap(),
            })
            .collect(),
    };
    let genesis = chain.genesis.hash();
    let good = mk(&heights);
    let work =
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &good, &heights)
            .expect("honest proof verifies");
    let all: u128 = (0..=tip.height())
        .map(|h| chain.header_at(h).unwrap().unwrap().difficulty)
        .sum();
    assert_eq!(work, all, "the proven total work of every block");
    let mut flood = good.clone();
    flood.recent = (0..200).map(|_| good.recent[0].clone()).collect();
    assert!(
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &flood, &heights)
            .is_err(),
        "an unbounded list of recent headers"
    );
    let mut forged = good.clone();
    forged.samples[0].header.timestamp += 1;
    assert!(
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &forged, &heights)
            .is_err(),
        "altered header"
    );
    let mut omitted = good.clone();
    omitted.samples.pop();
    assert!(
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &omitted, &heights)
            .is_err(),
        "omitted sample"
    );
    let mut short = good.clone();
    short.recent.truncate(2);
    assert!(
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &short, &heights)
            .is_err(),
        "too few recent headers to check the difficulty rule"
    );
    let mut extra = good.clone();
    extra.samples.push(extra.samples[0].clone());
    assert!(
        krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &extra, &heights)
            .is_err(),
        "unrequested samples"
    );
    // Release checkpoint: required in the chain, and headers above it need real work.
    let cp_h = heights[heights.len() / 2];
    let cp_hdr = chain.header_at(cp_h).unwrap().unwrap();
    let with_cp = |hash: String, difficulty: u128| {
        let mut p = chain.params.clone();
        p.checkpoint = Some(krypton_core::params::Checkpoint {
            height: cp_h,
            hash: Box::leak(hash.into_boxed_str()),
            difficulty,
        });
        p
    };
    let ok = with_cp(hex::encode(cp_hdr.hash()), cp_hdr.difficulty);
    krypton_light::verify_flyclient(&ok, &genesis, &chain.pow, &good, &heights)
        .expect("honest chain contains the checkpoint");
    let wrong = with_cp(hex::encode([7u8; 32]), cp_hdr.difficulty);
    assert!(
        krypton_light::verify_flyclient(&wrong, &genesis, &chain.pow, &good, &heights).is_err(),
        "a chain without the checkpoint is refused"
    );
    let hard = with_cp(hex::encode(cp_hdr.hash()), u128::MAX / 2);
    assert!(
        krypton_light::verify_flyclient(&hard, &genesis, &chain.pow, &good, &heights).is_err(),
        "headers far below the checkpoint difficulty are refused"
    );

    // Compact blocks: any change to outputs, nullifiers or tree updates is caught.
    let start = chain.header_at(0).unwrap().unwrap();
    let f0 = chain.frontier_at(0).unwrap().unwrap();
    let blocks = src.compact_blocks(1, 12).unwrap();
    let mut v = CompactVerifier::new(start.clone(), f0.clone()).unwrap();
    for b in &blocks {
        v.verify(b).unwrap();
    }
    for tamper in 0..5 {
        let mut v = CompactVerifier::new(start.clone(), f0.clone()).unwrap();
        let mut bs = blocks.clone();
        let paying = bs.iter().position(|b| !b.payouts.is_empty()).unwrap();
        let maturing = bs.iter().position(|b| !b.matured.is_empty()).unwrap();
        match tamper {
            0 => bs[paying].payouts[0].value += 1,
            1 => bs[maturing].matured[0].value += 1,
            2 => bs[maturing].matured_from += 1,
            3 => {
                bs[maturing].matured[0].owner =
                    digest_to_bytes(&rescue::SpendKeys::from_sk(rescue::scalar(5)).owner)
            }
            _ => bs[maturing].leaf_start += 1,
        }
        assert!(
            bs.iter().try_for_each(|b| v.verify(b)).is_err(),
            "tamper case {tamper} accepted"
        );
    }
}

#[test]
fn mine_sync_send_receive() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let ma = keys::new_mnemonic();
    let mb = keys::new_mnemonic();
    let mut a = Wallet::create(
        &dir.path().join("a.wallet"),
        "password-a",
        &ma,
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    let mut b = Wallet::create(
        &dir.path().join("b.wallet"),
        "password-b",
        &mb,
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    let addr_a = a.address();
    let addr_b = b.address();

    // Devnet: payouts every 4 blocks, spendable 2 blocks later.
    for _ in 0..10 {
        mine(&src, &addr_a);
    }
    a.sync(&src, |_, _| {}).unwrap();
    let bal = a.balance();
    let tip = chain.tip().height();
    assert!(bal.total > 0);
    assert_eq!(bal.total, paid_to(&chain, &addr_a, tip), "{bal:?}");
    assert_eq!(
        bal.spendable,
        paid_to(&chain, &addr_a, tip - chain.params.coinbase_maturity),
        "{bal:?}"
    );
    assert!(a.history().iter().any(|h| h.memo == "mining reward"));

    let amount = 1_000_000;
    let txid = a
        .send(&src, &addr_b, amount, DEFAULT_FEE, "first private payment")
        .unwrap();
    assert_ne!(txid, [0; 32]);
    assert!(a.balance().pending > 0);
    // Replaying the same spend is rejected by the mempool check after it is mined.
    let replay = src.pool.borrow()[0].clone();
    mine(&src, &addr_b);
    assert!(
        chain.check_mempool_tx(&replay).is_err(),
        "double spend must be rejected"
    );

    a.sync(&src, |_, _| {}).unwrap();
    b.sync(&src, |_, _| {}).unwrap();
    let tip = chain.tip().height();
    let ba = a.balance();
    let bb = b.balance();
    assert_eq!(
        bb.total,
        amount + paid_to(&chain, &addr_b, tip),
        "recipient {bb:?}"
    );
    assert_eq!(
        b.notes().iter().find(|n| n.value == amount).unwrap().memo,
        "first private payment"
    );

    // Payment proof: A proves the payment; B confirms it paid B; tampering fails.
    let proof_text = a.payment_proof(&hex::encode(txid)).unwrap();
    assert!(proof_text.starts_with("dgnrpay1"));
    let rep = b.verify_payment(&src, &proof_text).unwrap();
    assert_eq!(rep.value, amount);
    assert_eq!(rep.memo, "first private payment");
    assert_eq!(rep.recipient_confirmed, Some(true));
    assert!(rep.recipient_verified);
    assert_eq!(rep.recipient, addr_b.short());
    let third = a.verify_payment(&src, &proof_text).unwrap();
    assert_eq!(third.recipient_confirmed, None, "A is not the recipient");
    assert!(
        !third.recipient_verified,
        "no directory: the named recipient is only a claim"
    );
    // A proof re-labelled to name another wallet as the recipient is refused by that wallet.
    let c = Wallet::create(
        &dir.path().join("c.wallet"),
        "password-c",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    let mut p = proof::PaymentProof::decode(&proof_text).unwrap();
    p.recipient = c.address().short_id();
    let forged = p.encode();
    assert!(c.verify_payment(&src, &forged).is_err(), "forged recipient");
    let r = b.verify_payment(&src, &forged).unwrap();
    assert!(r.recipient_confirmed.is_none() && !r.recipient_verified);
    let mut p = proof::PaymentProof::decode(&proof_text).unwrap();
    p.key[0] ^= 1;
    assert!(b.verify_payment(&src, &p.encode()).is_err(), "wrong key");
    let mut p = proof::PaymentProof::decode(&proof_text).unwrap();
    p.output = 1;
    assert!(
        b.verify_payment(&src, &p.encode()).is_err(),
        "change output is not provable"
    );
    let mut p = proof::PaymentProof::decode(&proof_text).unwrap();
    p.owner = digest_to_bytes(&addr_a.owner);
    assert!(b.verify_payment(&src, &p.encode()).is_err(), "other owner");
    assert!(proof::PaymentProof::decode(&proof_text[..proof_text.len() - 2]).is_err());
    assert_eq!(
        ba.total,
        paid_to(&chain, &addr_a, tip) - amount - DEFAULT_FEE,
        "{ba:?}"
    );

    // Reopen from disk with the password.
    drop(a);
    let a2 = Wallet::open(&dir.path().join("a.wallet"), "password-a").unwrap();
    assert_eq!(a2.balance().total, ba.total);
    assert!(Wallet::open(&dir.path().join("a.wallet"), "wrong-password").is_err());

    // B pays back once its notes are spendable.
    let mut b = b;
    for _ in 0..3 {
        mine(&src, &addr_b);
    }
    b.sync(&src, |_, _| {}).unwrap();
    let back = b.send(&src, &addr_a, 500_000, DEFAULT_FEE, "back").unwrap();
    assert_ne!(back, [0; 32]);
    mine(&src, &addr_b);
    let mut a2 = a2;
    a2.sync(&src, |_, _| {}).unwrap();
    assert!(
        a2.notes()
            .iter()
            .any(|n| n.value == 500_000 && n.memo == "back")
    );
}

fn new_wallet(dir: &Path, name: &str, mnemonic: &str) -> Wallet {
    Wallet::create(
        &dir.join(name),
        "password-x",
        mnemonic,
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap()
}

fn mine_until(src: &Local, to: &Address, height: u64) {
    while src.chain.tip().height() < height {
        mine(src, to);
    }
}

/// The GNR side of an atomic swap end to end, with real proofs on a devnet chain:
/// lock, claim (the secret becomes public), a refund, and a refund by a wallet
/// restored from its recovery phrase.
#[test]
fn atomic_swap_lock_claim_and_refund() {
    use krypton_core::lock::sha256;
    use swap::LockRole;
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let phrase_a = keys::new_mnemonic();
    let mut a = new_wallet(dir.path(), "gnr-seller.wallet", &phrase_a);
    let mut b = new_wallet(dir.path(), "btc-seller.wallet", &keys::new_mnemonic());
    let (addr_a, addr_b) = (a.address(), b.address());
    for _ in 0..12 {
        mine(&src, &addr_a);
    }
    a.sync(&src, |_, _| {}).unwrap();
    b.sync(&src, |_, _| {}).unwrap();

    // Swap 1: the BTC seller holds the secret and names its claim outputs.
    let secret = [0x5a; 32];
    let value = 2_000_000;
    let (claim_fee, refund_fee) = (DEFAULT_FEE, DEFAULT_FEE);
    let claim_outputs = b.claim_outputs(1, value, claim_fee).unwrap();
    let refund_height = chain.tip().height() + 60;
    let before = a.balance().total;
    let (_, terms) = a
        .lock_funds(
            &src,
            1,
            sha256(&secret),
            refund_height,
            claim_outputs,
            value,
            refund_fee,
            DEFAULT_FEE,
        )
        .unwrap();
    let cm = b.watch_lock(1, terms.clone(), value, claim_fee).unwrap();
    assert_eq!(cm, terms.commitment(value));
    // Terms that do not pay B's own claim outputs are refused.
    let mut other = terms.clone();
    other.claim_outputs.swap(0, 1);
    assert!(b.watch_lock(1, other, value, claim_fee).is_err());
    mine(&src, &addr_a);
    a.sync(&src, |_, _| {}).unwrap();
    b.sync(&src, |_, _| {}).unwrap();
    assert_eq!(a.locked_balance(), value);
    assert!(
        a.balance().total
            <= before - value - DEFAULT_FEE + chain.params.payout_interval * 5 * emission::COIN
    );
    assert!(b.locks()[0].position.is_some() && b.locks()[0].height.is_some());
    assert_eq!(a.locks()[0].position, b.locks()[0].position);

    // Wrong secret, wrong role and an early refund are refused.
    assert!(b.claim_lock(&src, &cm, [1; 32]).is_err());
    assert!(a.claim_lock(&src, &cm, secret).is_err());
    assert!(a.refund_lock(&src, &cm).is_err());

    let b_before = b.balance().total;
    b.claim_lock(&src, &cm, secret).unwrap();
    // The GNR seller learns the secret from the mempool, before the block.
    let pool = src.pool.borrow().clone();
    let live: Vec<Hash32> = pool.iter().map(|t| t.txid()).collect();
    a.scan_mempool(&pool, &live);
    assert_eq!(a.locks()[0].secret, Some(secret));
    let claim = pool[0].clone();
    assert!(claim.lock.is_some());
    mine(&src, &addr_a);
    a.sync(&src, |_, _| {}).unwrap();
    b.sync(&src, |_, _| {}).unwrap();
    assert_eq!(b.balance().total, b_before + value - claim_fee);
    assert!(
        b.notes()
            .iter()
            .any(|n| n.value == value - claim_fee && n.memo == "swap 1: GNR received")
    );
    assert!(a.locks()[0].spent_height.is_some() && b.locks()[0].spent_height.is_some());
    assert_eq!(a.locked_balance(), 0);
    assert!(
        chain.check_mempool_tx(&claim).is_err(),
        "a lock is spent once"
    );
    assert!(a.refund_lock(&src, &cm).is_err());

    // Swap 2: nobody claims; the GNR seller refunds from the refund height on.
    let claim_outputs = b.claim_outputs(2, value, claim_fee).unwrap();
    let refund_height = chain.tip().height() + 25;
    let (_, terms2) = a
        .lock_funds(
            &src,
            2,
            sha256(&[0x77; 32]),
            refund_height,
            claim_outputs,
            value,
            refund_fee,
            DEFAULT_FEE,
        )
        .unwrap();
    let cm2 = b.watch_lock(2, terms2.clone(), value, claim_fee).unwrap();
    mine(&src, &addr_b);

    // A wallet restored from the phrase finds the lock and its terms on its own.
    let mut restored = new_wallet(dir.path(), "restored.wallet", &phrase_a);
    restored.sync(&src, |_, _| {}).unwrap();
    let rec = restored
        .locks()
        .iter()
        .find(|l| l.cm == cm2)
        .expect("lock recovered");
    assert_eq!(rec.role, LockRole::Refunder);
    assert_eq!(rec.terms, terms2);
    assert_eq!(rec.fee, refund_fee);
    assert!(
        restored.refund_lock(&src, &cm2).is_err(),
        "not before the refund height"
    );

    mine_until(&src, &addr_b, refund_height - 1);
    b.sync(&src, |_, _| {}).unwrap();
    assert!(
        b.claim_lock(&src, &cm2, [0x77; 32]).is_err(),
        "too late to claim"
    );
    restored.sync(&src, |_, _| {}).unwrap();
    let r_before = restored.balance().total;
    restored.refund_lock(&src, &cm2).unwrap();
    mine(&src, &addr_b);
    restored.sync(&src, |_, _| {}).unwrap();
    assert_eq!(restored.balance().total, r_before + value - refund_fee);
    assert_eq!(restored.locked_balance(), 0);
    a.sync(&src, |_, _| {}).unwrap();
    assert_eq!(
        a.locked_balance(),
        0,
        "the original wallet sees the refund too"
    );
}

#[test]
fn expired_sends_release_their_notes() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let mut a = Wallet::create(
        &dir.path().join("a.wallet"),
        "password-a",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    let to = Wallet::create(
        &dir.path().join("b.wallet"),
        "password-b",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap()
    .address();
    for _ in 0..10 {
        mine(&src, &a.address());
    }
    a.sync(&src, |_, _| {}).unwrap();
    let spendable = a.balance().spendable;
    assert!(spendable > 0);
    a.send(&src, &to, 1_000, DEFAULT_FEE, "never mined")
        .unwrap();
    assert!(a.balance().pending > 0);
    // The transaction is lost (never relayed): it can never be mined after its expiry.
    let lost = src.pool.borrow_mut().pop().unwrap();
    let expiry = lost.expiry_height;
    while chain.tip().height() <= expiry + 1 {
        mine(&src, &a.address());
    }
    a.sync(&src, |_, _| {}).unwrap();
    assert_eq!(a.balance().pending, 0, "expired spend released");
    assert!(a.balance().spendable >= spendable);
    assert!(
        a.history()
            .iter()
            .all(|h| h.txid != hex::encode(lost.txid())),
        "an expired send is not shown as sent"
    );
    // The released notes can be spent again.
    a.send(&src, &to, 1_000, DEFAULT_FEE, "second try").unwrap();
    mine(&src, &a.address());
    a.sync(&src, |_, _| {}).unwrap();
    assert_eq!(a.balance().pending, 0);
}

#[test]
fn wallet_from_another_genesis_rescans() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let mut w = Wallet::create(
        &dir.path().join("w.wallet"),
        "password-w",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    for _ in 0..8 {
        mine(&src, &w.address());
    }
    let r = w.sync(&src, |_, _| {}).unwrap();
    assert!(!r.reset);
    let before = w.balance().total;
    assert!(before > 0);
    assert!(!w.sync(&src, |_, _| {}).unwrap().reset);

    w.state.genesis = Some(hex::encode([9u8; 32]));
    let r = w.sync(&src, |_, _| {}).unwrap();
    assert!(r.reset);
    assert_eq!(w.balance().total, before);
    assert_eq!(w.synced_height(), Some(8));

    w.state.genesis = None;
    assert!(!w.sync(&src, |_, _| {}).unwrap().reset);
    assert_eq!(w.balance().total, before);
}

/// A source whose blocks are not part of the proven chain.
struct Liar<'a>(Local<'a>);
impl ChainSource for Liar<'_> {
    fn tip_height(&self) -> Result<u64> {
        self.0.tip_height()
    }
    fn header(&self, h: u64) -> Result<BlockHeader> {
        ChainSource::header(&self.0, h)
    }
    fn compact_blocks(&self, f: u64, c: u32) -> Result<Vec<CompactBlock>> {
        self.0.compact_blocks(f, c)
    }
    fn frontier(&self, h: u64) -> Result<Frontier> {
        self.0.frontier(h)
    }
    fn witness(&self, p: u64) -> Result<(Vec<Digest>, Digest)> {
        self.0.witness(p)
    }
    fn submit(&self, tx: &Transaction) -> Result<Hash32> {
        self.0.submit(tx)
    }
    fn verified_tip(
        &self,
        p: &krypton_core::ChainParams,
        pow: &krypton_pow::Verifier,
        k: Option<&BlockHeader>,
    ) -> Result<Option<BlockHeader>> {
        self.0.verified_tip(p, pow, k)
    }
    fn header_in(&self, _: &BlockHeader, _: &BlockHeader) -> Result<bool> {
        Ok(false)
    }
}

#[test]
fn sync_rejects_blocks_outside_the_proven_chain() {
    let dir = tempfile::tempdir().unwrap();
    let chain = Chain::open(&dir.path().join("chain"), Network::Devnet).unwrap();
    let src = Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    };
    let mut w = Wallet::create(
        &dir.path().join("w.wallet"),
        "password-l",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    for _ in 0..6 {
        mine(&src, &w.address());
    }
    let liar = Liar(Local {
        chain: &chain,
        pool: RefCell::new(vec![]),
    });
    assert!(w.sync(&liar, |_, _| {}).is_err());
    assert_eq!(w.balance().total, 0, "nothing applied from unproven blocks");
    w.sync(&src, |_, _| {}).unwrap();
    assert!(w.balance().total > 0);
}

/// A wallet keeps the heaviest chain it has verified: a lighter one from a node is refused
/// (a cheap fork cannot fake payments to it), and a heavier fork replaces what it followed.
#[test]
fn sync_follows_the_most_work_only() {
    let dir = tempfile::tempdir().unwrap();
    let a = Chain::open(&dir.path().join("a"), Network::Devnet).unwrap();
    let b = Chain::open(&dir.path().join("b"), Network::Devnet).unwrap();
    let src_a = Local {
        chain: &a,
        pool: RefCell::new(vec![]),
    };
    let src_b = Local {
        chain: &b,
        pool: RefCell::new(vec![]),
    };
    let mut w = Wallet::create(
        &dir.path().join("w.wallet"),
        "password-f",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap();
    let other = Wallet::create(
        &dir.path().join("o.wallet"),
        "password-o",
        &keys::new_mnemonic(),
        "",
        Network::Devnet,
        0,
        Argon2Params::TEST,
    )
    .unwrap()
    .address();
    for _ in 0..10 {
        mine(&src_a, &w.address());
    }
    for _ in 0..6 {
        mine(&src_b, &other);
    }
    w.sync(&src_a, |_, _| {}).unwrap();
    assert_eq!(w.synced_height(), Some(10));
    let err = w.sync(&src_b, |_, _| {}).unwrap_err().to_string();
    assert!(err.contains("less proof of work"), "{err}");
    assert_eq!(w.synced_height(), Some(10), "nothing changed");
    // The other chain grows heavier: the wallet moves to it.
    for _ in 0..10 {
        mine(&src_b, &other);
    }
    let r = w.sync(&src_b, |_, _| {}).unwrap();
    assert!(r.reorged > 0 || r.reset, "{r:?}");
    assert_eq!(w.synced_height(), Some(16));
    // And the first chain, now lighter, is refused in turn.
    assert!(w.sync(&src_a, |_, _| {}).is_err());
}

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
            recent: (tip.height().saturating_sub(20)..tip.height())
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
        recent: (1..tip.height())
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
    krypton_light::verify_flyclient(&chain.params, &genesis, &chain.pow, &good, &heights)
        .expect("honest proof verifies");
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

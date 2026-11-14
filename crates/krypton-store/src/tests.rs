use super::*;
use krypton_crypto::rescue;

fn owner(seed: u64) -> Hash32 {
    digest_to_bytes(&rescue::SpendKeys::from_sk(rescue::scalar(seed)).owner)
}

fn mine_next(c: &Chain, ts_offset: u64) -> Block {
    mine_with(c, owner(1), vec![], ts_offset)
}

fn mine_with(c: &Chain, miner: Hash32, shares: Vec<BlockHeader>, ts_offset: u64) -> Block {
    let mut b = c
        .template(
            miner,
            shares,
            vec![],
            c.tip().header.timestamp + 60 + ts_offset,
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
    b
}

#[test]
fn extend_reopen_and_reorganize() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let a = Chain::open(d1.path(), Network::Devnet).unwrap();
    let b = Chain::open(d2.path(), Network::Devnet).unwrap();
    assert_eq!(a.genesis.hash(), b.genesis.hash());

    // Shared prefix of 3 blocks.
    for _ in 0..3 {
        let blk = mine_next(&a, 0);
        assert_eq!(
            a.process_block(&blk, now).unwrap(),
            Outcome::Connected { reorg_depth: 0 }
        );
        assert_eq!(
            b.process_block(&blk, now).unwrap(),
            Outcome::Connected { reorg_depth: 0 }
        );
        assert_eq!(a.process_block(&blk, now).unwrap(), Outcome::AlreadyKnown);
    }
    // Fork: A mines 2 blocks, B mines 4 blocks.
    let mut a_branch = vec![];
    for _ in 0..2 {
        let blk = mine_next(&a, 1);
        a.process_block(&blk, now).unwrap();
        a_branch.push(blk);
    }
    let mut b_branch = vec![];
    for _ in 0..4 {
        let blk = mine_next(&b, 7);
        b.process_block(&blk, now).unwrap();
        b_branch.push(blk);
    }
    let a_root_before = a.tip().frontier.root();
    assert_eq!(a.tip().height(), 5);
    let mut outcomes = vec![];
    for blk in &b_branch {
        outcomes.push(a.process_block(blk, now).unwrap());
    }
    assert!(
        outcomes.contains(&Outcome::Connected { reorg_depth: 2 }),
        "{outcomes:?}"
    );
    assert_eq!(a.tip().hash, b.tip().hash);
    assert_eq!(a.tip().frontier, b.tip().frontier);
    assert_ne!(a.tip().frontier.root(), a_root_before);

    // Reorg back is impossible with less work; feeding A's old blocks keeps B's chain.
    for blk in &a_branch {
        assert_eq!(a.process_block(blk, now).unwrap(), Outcome::AlreadyKnown);
    }
    // MMR proofs of any block against the tip's header commitment.
    let tip = a.tip();
    for leaf in 0..tip.height() {
        let p = a.mmr_proof(leaf, tip.height()).unwrap().unwrap();
        let hdr = a.header_at(leaf).unwrap().unwrap();
        assert!(mmr::verify(
            &tip.header.mmr_root,
            &MmrNode::leaf(&hdr.hash(), hdr.difficulty),
            &p
        ));
    }
    // Auth paths for every leaf verify against the root.
    let root = a.note_root_digest();
    for pos in 0..tip.frontier.size {
        let (path, r) = a.auth_path(pos).unwrap();
        assert_eq!(r, root);
        let leaf = a.view().unwrap().get(0, pos).unwrap();
        assert_eq!(krypton_zk::layout::DEPTH, path.len());
        let mut cur = leaf;
        for (k, s) in path.iter().enumerate() {
            cur = if (pos >> k) & 1 == 0 {
                krypton_crypto::rescue::merkle_node(&cur, s)
            } else {
                krypton_crypto::rescue::merkle_node(s, &cur)
            };
        }
        assert_eq!(cur, root);
    }
    drop(a);
    let a2 = Chain::open(d1.path(), Network::Devnet).unwrap();
    assert_eq!(a2.tip().hash, b.tip().hash);
    assert_eq!(a2.tip().frontier, b.tip().frontier);
}

#[test]
fn rejects_invalid_blocks_and_orphans() {
    let d = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    let blk = mine_next(&c, 0);
    let mut orphan = blk.clone();
    orphan.header.prev = [9; 32];
    assert_eq!(
        c.process_block(&orphan, now).unwrap(),
        Outcome::Orphan { missing: [9; 32] }
    );
    let mut bad = blk.clone();
    bad.header.payout_digest[0] ^= 1;
    while !c.pow.verify(
        bad.header.height,
        &bad.header.pow_hash(),
        bad.header.nonce,
        bad.header.difficulty,
    ) {
        bad.header.nonce += 1;
    }
    assert!(c.process_block(&bad, now).is_err());
    assert!(c.process_block(&bad, now).is_err(), "remembered as invalid");
    assert_eq!(
        c.process_block(&blk, now).unwrap(),
        Outcome::Connected { reorg_depth: 0 }
    );
}

#[test]
fn database_from_another_genesis_is_set_aside() {
    let d = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    {
        let c = Chain::open(d.path(), Network::Devnet).unwrap();
        let blk = mine_next(&c, 0);
        c.process_block(&blk, now).unwrap();
        assert_eq!(c.tip().height(), 1);
    }
    retire_foreign_chain(d.path(), &[7; 32]).unwrap();
    assert!(!d.path().join("chain.redb").exists());
    let retired: Vec<_> = std::fs::read_dir(d.path())
        .unwrap()
        .filter_map(|e| e.ok())
        .filter(|e| e.file_name().to_string_lossy().ends_with(".redb.retired"))
        .collect();
    assert_eq!(retired.len(), 1);
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    assert_eq!(c.tip().height(), 0);
    drop(c);
    let genesis = consensus::genesis_block(&Network::Devnet.params()).hash();
    retire_foreign_chain(d.path(), &genesis).unwrap();
    assert!(d.path().join("chain.redb").exists());
}

#[test]
fn relayed_block_with_altered_body_does_not_poison_its_header() {
    let d = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    for _ in 0..2 {
        let b = mine_next(&c, 0);
        c.process_block(&b, now).unwrap();
    }
    let s = mine_share(&c, owner(2), 1);
    let genuine = mine_with(&c, owner(1), vec![s], 0);
    assert_eq!(genuine.shares.len(), 1);
    // Same header (valid proof of work), body stripped by a relaying peer.
    let mut altered = genuine.clone();
    altered.shares.clear();
    assert_eq!(altered.hash(), genuine.hash());
    assert!(matches!(
        c.process_block(&altered, now),
        Err(ChainError::Invalid(_))
    ));
    assert!(
        c.header_record(&genuine.hash())
            .unwrap()
            .is_none_or(|r| r.status != STATUS_INVALID),
        "header must not be remembered as invalid"
    );
    assert_eq!(
        c.process_block(&genuine, now).unwrap(),
        Outcome::Connected { reorg_depth: 0 }
    );
    assert_eq!(c.tip().hash, genuine.hash());
}

#[test]
fn undoing_a_block_without_new_leaves_keeps_the_anchor() {
    let d = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    // Devnet payouts first mature at height 7: blocks 1..=4 keep the empty tree's root.
    for _ in 0..4 {
        let b = mine_next(&c, 0);
        c.process_block(&b, now).unwrap();
    }
    let root = c.tip().header.note_root;
    assert_eq!(c.header_at(3).unwrap().unwrap().note_root, root);
    assert_eq!(c.view().unwrap().anchor_height(&root), Some(4));
    {
        let _g = c.write.lock();
        c.disconnect_tip().unwrap();
    }
    assert_eq!(c.tip().height(), 3);
    assert_eq!(
        c.view().unwrap().anchor_height(&root),
        Some(3),
        "the parent's identical root stays a valid anchor"
    );
}

#[test]
fn startup_repair_restores_anchor_roots_and_clears_wrong_invalid_marks() {
    let d = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let (root, side) = {
        let c = Chain::open(d.path(), Network::Devnet).unwrap();
        for _ in 0..2 {
            let b = mine_next(&c, 0);
            c.process_block(&b, now).unwrap();
        }
        // Two valid blocks at height 3; the first seen stays main. Then the state
        // kryptond 1.0.0 could leave: the tip's root missing and the valid side block
        // marked invalid (body deleted).
        let main3 = mine_next(&c, 0);
        let side = mine_next(&c, 5);
        assert_eq!(
            c.process_block(&main3, now).unwrap(),
            Outcome::Connected { reorg_depth: 0 }
        );
        assert_eq!(c.process_block(&side, now).unwrap(), Outcome::SideChain);
        c.mark_invalid(&side.hash()).unwrap();
        let root = c.tip().header.note_root;
        let w = c.db.begin_write().unwrap();
        w.open_table(ROOTS)
            .unwrap()
            .remove(root.as_slice())
            .unwrap();
        w.open_table(META)
            .unwrap()
            .remove(META_INVALID_RESET)
            .unwrap();
        w.commit().unwrap();
        assert_eq!(c.view().unwrap().anchor_height(&root), None);
        (root, side)
    };
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    assert_eq!(c.view().unwrap().anchor_height(&root), Some(3));
    let rec = c.header_record(&side.hash()).unwrap().unwrap();
    assert_eq!(rec.status, STATUS_UNVERIFIED);
    assert_eq!(c.process_block(&side, now).unwrap(), Outcome::SideChain);
    // Invalid marks set after the one-time reset are kept.
    c.mark_invalid(&side.hash()).unwrap();
    drop(c);
    let c = Chain::open(d.path(), Network::Devnet).unwrap();
    assert_eq!(
        c.header_record(&side.hash()).unwrap().unwrap().status,
        STATUS_INVALID
    );
}

#[test]
fn reorganization_with_a_missing_body_leaves_the_chain_untouched() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let a = Chain::open(d1.path(), Network::Devnet).unwrap();
    let b = Chain::open(d2.path(), Network::Devnet).unwrap();
    for _ in 0..2 {
        let blk = mine_next(&a, 0);
        a.process_block(&blk, now).unwrap();
        b.process_block(&blk, now).unwrap();
    }
    let blk = mine_next(&a, 1);
    a.process_block(&blk, now).unwrap();
    let branch: Vec<Block> = (0..3)
        .map(|_| {
            let blk = mine_next(&b, 9);
            b.process_block(&blk, now).unwrap();
            blk
        })
        .collect();
    let tip = a.tip().hash;
    a.process_block(&branch[0], now).unwrap();
    let w = a.db.begin_write().unwrap();
    w.open_table(BLOCKS)
        .unwrap()
        .remove(branch[0].hash().as_slice())
        .unwrap();
    w.commit().unwrap();
    a.process_block(&branch[1], now).unwrap();
    let out = a.process_block(&branch[2], now).unwrap();
    assert_eq!(
        out,
        Outcome::Orphan {
            missing: branch[0].hash()
        }
    );
    assert_eq!(a.tip().hash, tip, "nothing was disconnected");
    // Once the body arrives again the heavier branch wins.
    a.process_block(&branch[0], now).unwrap();
    let next = mine_next(&b, 9);
    b.process_block(&next, now).unwrap();
    assert!(matches!(
        a.process_block(&next, now).unwrap(),
        Outcome::Connected { .. }
    ));
    assert_eq!(a.tip().hash, b.tip().hash);
}

/// A header from `c`'s next template that meets the share target only.
fn mine_share(c: &Chain, miner: Hash32, salt: u64) -> BlockHeader {
    let mut h = c
        .template(miner, vec![], vec![], c.tip().header.timestamp + 30)
        .unwrap()
        .header;
    h.nonce = salt << 32;
    loop {
        h.nonce += 1;
        let ph = h.pow_hash();
        let d = krypton_core::reward::share_difficulty(h.difficulty);
        if c.pow.verify(h.height, &ph, h.nonce, d)
            && !c.pow.verify(h.height, &ph, h.nonce, h.difficulty)
        {
            return h;
        }
    }
}

fn reward_state(c: &Chain, owners: &[Hash32]) -> (u64, Vec<Option<Account>>, Vec<Vec<Payout>>) {
    let tip = c.tip().height();
    (
        c.pool().unwrap(),
        owners.iter().map(|o| c.account(o).unwrap()).collect(),
        (0..=tip).map(|h| c.payouts_at(h).unwrap()).collect(),
    )
}

#[test]
fn shares_rewards_and_reorgs_are_exact() {
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let d3 = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let a = Chain::open(d1.path(), Network::Devnet).unwrap();
    let b = Chain::open(d2.path(), Network::Devnet).unwrap();
    let (fa, fb, sh) = (owner(11), owner(12), owner(13));
    let owners = [fa, fb, sh];
    let mut common = vec![];
    for i in 0..6 {
        let s = mine_share(&a, sh, i);
        assert!(a.check_share(&s).is_ok());
        let blk = mine_with(&a, fa, vec![s.clone()], 0);
        assert_eq!(blk.shares, vec![s.clone()]);
        a.process_block(&blk, now).unwrap();
        b.process_block(&blk, now).unwrap();
        assert!(a.share_height(&s.hash()).unwrap().is_some());
        assert!(
            a.check_share(&s).is_err(),
            "credited shares are not accepted again"
        );
        common.push(blk);
    }
    // Fork: A extends with shares; B extends further without them and wins.
    for i in 0..3 {
        let s = mine_share(&a, sh, 100 + i);
        let blk = mine_with(&a, fa, vec![s], 1);
        a.process_block(&blk, now).unwrap();
    }
    let mut b_branch = vec![];
    for _ in 0..5 {
        let blk = mine_with(&b, fb, vec![], 7);
        b.process_block(&blk, now).unwrap();
        b_branch.push(blk);
    }
    for blk in &b_branch {
        a.process_block(blk, now).unwrap();
    }
    assert_eq!(a.tip().hash, b.tip().hash);
    assert_eq!(reward_state(&a, &owners), reward_state(&b, &owners));
    // A chain built straight along the winning branch has exactly the same state.
    let c = Chain::open(d3.path(), Network::Devnet).unwrap();
    for blk in common.iter().chain(&b_branch) {
        c.process_block(blk, now).unwrap();
    }
    assert_eq!(reward_state(&a, &owners), reward_state(&c, &owners));
    assert_eq!(a.tip().frontier, c.tip().frontier);
    // Payouts happened and became tree leaves after maturity.
    let paid: u64 = (0..=c.tip().height())
        .flat_map(|h| c.payouts_at(h).unwrap())
        .map(|p| p.value)
        .sum();
    assert!(paid > 0);
    let unpaid = c.pool().unwrap()
        + owners
            .iter()
            .filter_map(|o| c.account(o).unwrap())
            .map(|a| a.amount)
            .sum::<u64>();
    assert_eq!(paid + unpaid, c.supply(), "rewards are conserved");
    let compact = c.compact_block(c.tip().height()).unwrap().unwrap();
    assert_eq!(
        compact.matured_from,
        c.tip().height() - c.params.coinbase_maturity
    );
}

/// A coin deploy mined in a block registers the coin; a reorganization to a chain
/// without it removes the coin, frees its ticker and unspends the fee note.
#[test]
fn coin_registry_follows_reorganizations() {
    use krypton_core::asset::*;
    use krypton_core::types::*;
    use krypton_zk::{InputWitness, OutputWitness, SpendWitness};
    let d1 = tempfile::tempdir().unwrap();
    let d2 = tempfile::tempdir().unwrap();
    let now = u64::MAX / 4;
    let a = Chain::open(d1.path(), Network::Devnet).unwrap();
    let b = Chain::open(d2.path(), Network::Devnet).unwrap();
    for _ in 0..300 {
        let blk = mine_next(&a, 0);
        a.process_block(&blk, now).unwrap();
        b.process_block(&blk, now).unwrap();
    }
    // The two largest matured reward notes of owner(1).
    let size = a.tip().frontier.size;
    let leaves = a.leaves(0, size).unwrap();
    let mut found = Vec::new();
    for h in 0..=a.tip().height() {
        for (i, p) in a.payouts_at(h).unwrap().iter().enumerate() {
            let cm = krypton_core::reward::payout_cm(h, i as u32, p).unwrap();
            if p.owner == owner(1)
                && let Some(pos) = leaves.iter().position(|l| *l == cm)
            {
                found.push((p.value, h, i as u32, pos as u64));
            }
        }
    }
    found.sort();
    let notes = &found[found.len() - 2..];
    let value: u64 = notes.iter().map(|n| n.0).sum();
    assert!(value > DEPLOY_FEE, "{found:?}");
    let input = |n: &(u64, u64, u32, u64)| InputWitness {
        sk: rescue::scalar(1),
        value: n.0,
        rcm: krypton_core::reward::payout_rcm(n.1, n.2, &owner(1)),
        pos: n.3,
        path: a.auth_path(n.3).unwrap().0,
        dummy: false,
        lock: false,
    };
    let key = krypton_crypto::sign::MlDsaKey::from_seed(&[5; 32]);
    let m = Manifest {
        name: "Store Test Krypton".into(),
        ticker: "STORK".into(),
        decimals: 0,
        supply: Supply::Mintable,
        cap: None,
        burn: false,
        pause: false,
        backing: Backing::None,
        origin: None,
        reserves: None,
        website: None,
        terms: None,
        logo: None,
        issuer: key.public_bytes(),
    };
    let root = a.note_root_digest();
    let mut w = SpendWitness {
        inputs: [input(&notes[0]), input(&notes[1])],
        outputs: [
            OutputWitness {
                owner: rescue::SpendKeys::from_sk(rescue::scalar(1)).owner,
                value: value - DEPLOY_FEE,
                rcm: rescue::scalar(77),
            },
            OutputWitness::dummy(),
        ],
        root,
        fee: DEPLOY_FEE,
        mint: 0,
        sighash: rescue::scalar(0),
        asset: None,
    };
    let p = w.public();
    let out = |cm: &Digest| Output {
        cm: digest_to_bytes(cm),
        ciphertext: vec![1; NOTE_CIPHERTEXT_LEN],
    };
    let mut tx = Transaction {
        version: TX_VERSION_ASSET,
        kind: TxKind::Transfer,
        anchor: digest_to_bytes(&root),
        nullifiers: p.nullifiers.map(|n| HexHash(digest_to_bytes(&n))),
        outputs: [out(&p.output_cms[0]), out(&p.output_cms[1])],
        fee: DEPLOY_FEE,
        mint: 0,
        expiry_height: 0,
        lock: None,
        proof: vec![],
        asset: Some(AssetPart {
            op: AssetOp::Deploy,
            coin: None,
            burn: 0,
            manifest: Some(m.canonical()),
            issuer_sig: None,
            fee_action: None,
        }),
    };
    w.sighash = tx.sighash();
    tx.asset.as_mut().unwrap().issuer_sig = Some(key.sign(&tx.sighash_bytes(), SIG_CTX).unwrap());
    tx.proof = krypton_zk::prove(&w).unwrap().1;
    a.check_mempool_tx(&tx).unwrap();

    let mut blk = a
        .template(
            owner(1),
            vec![],
            vec![tx.clone()],
            a.tip().header.timestamp + 60,
        )
        .unwrap();
    assert_eq!(blk.txs.len(), 1);
    while !a.pow.verify(
        blk.header.height,
        &blk.header.pow_hash(),
        blk.header.nonce,
        blk.header.difficulty,
    ) {
        blk.header.nonce += 1;
    }
    a.process_block(&blk, now).unwrap();
    let id = m.id();
    let s = a.asset(&id).unwrap().expect("registered");
    assert_eq!(
        (s.manifest.ticker.as_str(), s.height),
        ("STORK", blk.header.height)
    );
    assert_eq!(a.asset_by_ticker("stork").unwrap(), Some(id));
    assert_eq!(a.assets().unwrap().len(), 1);
    assert!(a.check_mempool_tx(&tx).is_err(), "spent and deployed");
    let compact = a.compact_block(blk.header.height).unwrap().unwrap();
    assert_eq!(compact.nullifiers.len(), 2);

    // The coin survives a restart, then a heavier chain without it removes it.
    drop(a);
    let a = Chain::open(d1.path(), Network::Devnet).unwrap();
    assert!(a.asset(&id).unwrap().is_some());
    for _ in 0..3 {
        let blk = mine_next(&b, 9);
        b.process_block(&blk, now).unwrap();
        a.process_block(&blk, now).unwrap();
    }
    assert_eq!(a.tip().hash, b.tip().hash);
    assert!(
        a.asset(&id).unwrap().is_none(),
        "undone by the reorganization"
    );
    assert_eq!(a.asset_by_ticker("STORK").unwrap(), None, "ticker freed");
    assert!(a.assets().unwrap().is_empty());
    assert!(a.take_disconnected_txs().contains(&tx));
}

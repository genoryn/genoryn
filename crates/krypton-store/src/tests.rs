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

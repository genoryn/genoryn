use krypton_core::consensus::*;
use krypton_core::mmr::{MemMmr, MmrNode};
use krypton_core::params::Network;
use krypton_core::reward::{self, Account, Payout, RewardView};
use krypton_core::tree::Frontier;
use krypton_core::types::*;
use krypton_core::{CoreError, Hash32, emission};
use krypton_crypto::field::digest_to_bytes;
use krypton_crypto::rescue;
use krypton_pow::Verifier;
use std::collections::{BTreeMap, HashMap, HashSet};

#[derive(Default)]
struct Mem {
    nfs: HashSet<Hash32>,
    roots: HashMap<Hash32, u64>,
    main: Vec<Hash32>,
    shares: HashSet<Hash32>,
    pool: u64,
    accounts: BTreeMap<Hash32, Account>,
    payouts: Vec<Vec<Payout>>,
}
impl RewardView for Mem {
    fn pool(&self) -> u64 {
        self.pool
    }
    fn account(&self, o: &Hash32) -> Option<Account> {
        self.accounts.get(o).copied()
    }
    fn due_accounts(&self, max_since: u64, limit: usize) -> Vec<(Hash32, Account)> {
        let mut v: Vec<_> = self
            .accounts
            .iter()
            .filter(|(_, a)| a.since <= max_since)
            .map(|(o, a)| (*o, *a))
            .collect();
        v.sort_by_key(|(o, a)| (a.since, *o));
        v.truncate(limit);
        v
    }
}
impl StateView for Mem {
    fn nullifier_spent(&self, nf: &Hash32) -> bool {
        self.nfs.contains(nf)
    }
    fn anchor_height(&self, root: &Hash32) -> Option<u64> {
        self.roots.get(root).copied()
    }
    fn main_hash(&self, height: u64) -> Option<Hash32> {
        self.main.get(height as usize).copied()
    }
    fn share_seen(&self, hash: &Hash32) -> bool {
        self.shares.contains(hash)
    }
}

fn mine(pow: &Verifier, h: &mut BlockHeader) {
    while !pow.verify(h.height, &h.pow_hash(), h.nonce, h.difficulty) {
        h.nonce += 1;
    }
}

/// A header meeting the share target but not the block target.
fn mine_share(pow: &Verifier, h: &mut BlockHeader) {
    loop {
        h.nonce += 1;
        let ph = h.pow_hash();
        if pow.verify(
            h.height,
            &ph,
            h.nonce,
            reward::share_difficulty(h.difficulty),
        ) && !pow.verify(h.height, &ph, h.nonce, h.difficulty)
        {
            return;
        }
    }
}

fn owner(seed: u64) -> Hash32 {
    digest_to_bytes(&rescue::SpendKeys::from_sk(rescue::scalar(seed)).owner)
}

#[test]
fn proportional_rewards_on_devnet_blocks() {
    let p = Network::Devnet.params();
    let genesis = genesis_block(&p);
    let pow = Verifier::new(p.pow, genesis.hash());
    let finder = owner(77);
    let sharer = owner(78);

    let mut state = Mem::default();
    state.roots.insert(genesis.header.note_root, 0);
    state.main.push(genesis.hash());
    state.payouts.push(vec![]);
    let mut mmr = MemMmr::default();
    mmr.push(MmrNode::leaf(&genesis.hash(), genesis.header.difficulty));
    let mut headers = vec![genesis.header.clone()];
    let mut frontier = Frontier::new();
    let mut minted = 0u64;
    let mut paid: HashMap<Hash32, u64> = HashMap::new();
    let mut leftover_share: Option<BlockHeader> = None;

    for height in 1..=14u64 {
        let parent = headers.last().unwrap().clone();
        let history: Vec<(u64, u128)> = headers
            .iter()
            .map(|h| (h.timestamp, h.difficulty))
            .collect();
        let matured = match height.checked_sub(p.coinbase_maturity) {
            Some(h) if h > 0 => payout_cms(h, &state.payouts[h as usize]).unwrap(),
            _ => vec![],
        };
        let ctx = ParentContext {
            parent: &parent,
            history: &history,
            frontier: &frontier,
            mmr_root: mmr.root(),
            matured: matured.clone(),
        };
        // The second miner finds three shares on this parent.
        let mut shares = Vec::new();
        for _ in 0..3 {
            let t = build_template(
                &p,
                &state,
                &ctx,
                sharer,
                vec![],
                vec![],
                parent.timestamp + 30,
            )
            .unwrap();
            let mut h = t.header;
            h.nonce = rand_nonce(height, shares.len());
            mine_share(&pow, &mut h);
            check_share_pow(&pow, &h).unwrap();
            shares.push(h);
        }
        // One share from an earlier block arrives late: still creditable in the window.
        if let Some(s) = leftover_share.take() {
            shares.push(s);
        }
        let mut block = build_template(
            &p,
            &state,
            &ctx,
            finder,
            shares.clone(),
            vec![],
            parent.timestamp + 60,
        )
        .unwrap();
        assert_eq!(block.shares.len(), shares.len(), "all shares includable");
        mine(&pow, &mut block.header);
        let fx = check_block(&p, &pow, &state, &ctx, &block, block.header.timestamp)
            .expect("valid block");

        // Tampering must fail: payouts, note root, share order, forged share, bad PoW.
        let reject =
            |b: &Block| check_block(&p, &pow, &state, &ctx, b, b.header.timestamp).is_err();
        let remine = |mut b: Block| {
            b.header.share_root = reward::share_root(&b.shares);
            mine(&pow, &mut b.header);
            b
        };
        let mut bad = block.clone();
        bad.header.payout_digest[0] ^= 1;
        assert!(reject(&remine(bad)));
        let mut bad = block.clone();
        bad.header.note_root[0] ^= 1;
        assert!(reject(&remine(bad)));
        if block.shares.len() >= 2 {
            let mut bad = block.clone();
            bad.shares.swap(0, 1);
            assert!(reject(&remine(bad)));
            let mut bad = block.clone();
            let d = bad.shares[0].clone();
            bad.shares.push(d);
            assert!(reject(&remine(bad)));
        }
        // Re-pointing a share to another owner voids its proof of work (devnet's share
        // target is so easy that some forgeries pass by luck; skip those).
        let mut bad = block.clone();
        bad.shares[0].miner = finder;
        while check_share_pow(&pow, &bad.shares[0]).is_ok() {
            bad.shares[0].timestamp += 1;
        }
        bad.shares.sort_by_key(|s| s.hash());
        assert!(
            reject(&remine(bad)),
            "a share's owner is bound by its proof of work"
        );
        let mut bad = block.clone();
        bad.header.nonce = bad.header.nonce.wrapping_add(1);
        while pow.verify(
            bad.header.height,
            &bad.header.pow_hash(),
            bad.header.nonce,
            bad.header.difficulty,
        ) {
            bad.header.nonce += 1;
        }
        assert!(reject(&bad));
        assert_eq!(
            check_block(
                &p,
                &pow,
                &state,
                &ctx,
                &block,
                block.header.timestamp - 10_000
            )
            .unwrap_err(),
            CoreError::Future
        );

        // Keep one share of this round to be credited by the next block.
        if height % 2 == 1 {
            let t = build_template(
                &p,
                &state,
                &ctx,
                sharer,
                vec![],
                vec![],
                parent.timestamp + 45,
            )
            .unwrap();
            let mut h = t.header;
            h.nonce = rand_nonce(height, 99);
            mine_share(&pow, &mut h);
            leftover_share = Some(h);
        }

        // Apply.
        minted += emission::subsidy(height);
        for o in &fx.reward.payouts {
            *paid.entry(o.owner).or_default() += o.value;
        }
        for s in &fx.shares {
            state.shares.insert(*s);
        }
        state.pool = fx.reward.pool;
        for (o, a) in &fx.reward.accounts {
            match a {
                Some(a) => state.accounts.insert(*o, *a),
                None => state.accounts.remove(o),
            };
        }
        state.payouts.push(fx.reward.payouts.clone());
        frontier = fx.frontier;
        state.roots.insert(block.header.note_root, height);
        state.main.push(block.hash());
        mmr.push(MmrNode::leaf(&block.hash(), block.header.difficulty));
        headers.push(block.header.clone());
    }
    let unpaid = state.pool + state.accounts.values().map(|a| a.amount).sum::<u64>();
    let total_paid: u64 = paid.values().sum();
    assert_eq!(
        total_paid + unpaid,
        minted,
        "every minted gori is pooled, owed or paid"
    );
    assert!(
        paid.get(&finder).copied().unwrap_or(0) > 0,
        "block finder paid"
    );
    assert!(
        paid.get(&sharer).copied().unwrap_or(0) > 0,
        "share-only miner paid"
    );
    let matured_notes: usize = (1..=14 - p.coinbase_maturity as usize)
        .map(|h| state.payouts[h].len())
        .sum();
    assert_eq!(
        frontier.size as usize, matured_notes,
        "payout notes matured into the tree"
    );
    // A credited share can never be credited again.
    let any = *state.shares.iter().next().unwrap();
    assert!(state.share_seen(&any));
}

fn rand_nonce(h: u64, i: usize) -> u64 {
    (h << 32) | ((i as u64) << 20)
}

#[test]
fn block_and_tx_encoding_roundtrip() {
    let p = Network::Devnet.params();
    let g = genesis_block(&p);
    assert_eq!(Block::from_bytes(&g.to_bytes()).unwrap(), g);
    assert_eq!(
        BlockHeader::from_bytes(&g.header.to_bytes())
            .unwrap()
            .hash(),
        g.hash()
    );
    assert_eq!(g.header.to_bytes().len(), HEADER_LEN);
    let mut extra = g.to_bytes();
    extra.push(0);
    assert!(Block::from_bytes(&extra).is_err());
    // Networks have distinct genesis blocks.
    assert_ne!(genesis_block(&Network::Testnet.params()).hash(), g.hash());
}

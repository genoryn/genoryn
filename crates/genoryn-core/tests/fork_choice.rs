use genoryn_core::{
    block::Block,
    chain::{Chain, is_policy_rejection, is_unknown_parent, unix_time},
    params::Network,
    pow, privacy,
};

fn mine(mut block: Block) -> Block {
    loop {
        let hash = pow::randomx(block.header.pow_seed, &block.header.bytes().unwrap()).unwrap();
        if pow::meets_difficulty(hash, block.header.difficulty) {
            return block;
        }
        block.header.nonce += 1;
    }
}
fn extend(chain: &mut Chain) -> Block {
    let block = mine(
        chain
            .template(privacy::treasury_address(Network::Devnet))
            .unwrap(),
    );
    assert!(chain.accept(block.clone()).unwrap().became_best);
    block
}

#[test]
#[ignore = "mines real devnet blocks with Halo2 coinbase proofs; explicit full suite"]
fn side_branches_are_header_checked_then_reorganize_by_cumulative_work() {
    let dir_a = tempfile::tempdir().unwrap();
    let dir_b = tempfile::tempdir().unwrap();
    let path_a = dir_a.path().join("chain.sqlite");
    let mut a = Chain::open(&path_a, Network::Devnet).unwrap();
    let mut b = Chain::open(&dir_b.path().join("chain.sqlite"), Network::Devnet).unwrap();

    let one = extend(&mut a);
    assert!(b.accept(one.clone()).unwrap().became_best);
    let a2 = extend(&mut a);
    let a3 = extend(&mut a);
    let b2 = extend(&mut b);
    let b3 = extend(&mut b);
    let b4 = extend(&mut b);

    let side = a.accept(b2.clone()).unwrap();
    assert!(!side.became_best);
    assert_eq!(a.state.best_hash, a3.hash().unwrap());
    let tie = a.accept(b3.clone()).unwrap();
    assert!(!tie.became_best, "equal work keeps the current branch");
    let reorg = a.accept(b4.clone()).unwrap();
    assert!(reorg.became_best);
    assert_eq!(reorg.reorg_depth, 2);
    assert_eq!(a.state.best_hash, b.state.best_hash);
    assert_eq!(a.active.len(), 4);
    assert_eq!(a.state.work, b.state.work);
    assert_eq!(a.state.notes_root(), b.state.notes_root());
    assert_eq!(a.state.nullifiers_root(), b.state.nullifiers_root());
    assert_eq!(a.active_position(one.hash().unwrap()).unwrap(), Some(0));
    assert_eq!(a.active_position(b4.hash().unwrap()).unwrap(), Some(3));
    assert_eq!(a.active_position(a2.hash().unwrap()).unwrap(), None);
    assert!(
        !a.accept(a2.clone()).unwrap().became_best,
        "known blocks are no-ops"
    );

    let mut orphan = b4.clone();
    orphan.header.previous = [9; 32];
    assert!(is_unknown_parent(&a.accept(orphan).unwrap_err()));

    let mut wrong_difficulty = a2.clone();
    wrong_difficulty.header.difficulty += 1;
    let error = a.accept(wrong_difficulty).unwrap_err();
    assert!(error.to_string().contains("difficulty"), "{error}");
    assert!(!is_policy_rejection(&error) && !is_unknown_parent(&error));

    let mut future = a2.clone();
    future.header.timestamp = unix_time() + 10_000;
    assert!(is_policy_rejection(&a.accept(future).unwrap_err()));

    assert_eq!(a.active_position(a3.hash().unwrap()).unwrap(), None);

    let best = a.state.best_hash;
    let work = a.state.work;
    let nullifiers = a.state.nullifiers_root();
    drop(a);
    let reopened = Chain::open(&path_a, Network::Devnet).unwrap();
    assert_eq!(reopened.state.best_hash, best);
    assert_eq!(reopened.state.work, work);
    assert_eq!(reopened.state.nullifiers_root(), nullifiers);
    assert_eq!(reopened.active.len(), 4);
}

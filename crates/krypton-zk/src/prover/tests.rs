use super::*;

fn note_input(sk: u64, value: u64, pos: u64) -> InputWitness {
    let path: Vec<Digest> = (0..DEPTH).map(|_| rand_digest()).collect();
    InputWitness {
        sk: rescue::scalar(sk),
        value,
        rcm: rand_digest(),
        pos,
        path,
        dummy: false,
        lock: false,
    }
}

fn valid_witness() -> SpendWitness {
    witness(false)
}

/// Two real inputs at different positions sharing one tree root; input 0 is a lock
/// note if `lock`.
fn witness(lock: bool) -> SpendWitness {
    witness_in(lock, [0; 3], [0; 3], None)
}

/// Like `witness`, but the tree holds notes of the coin `held` and the spend claims
/// the coin section `asset` (whose id may differ, to model counterfeiting).
fn witness_in(
    lock: bool,
    held: [u64; 3],
    _claimed: [u64; 3],
    asset: Option<AssetWitness>,
) -> SpendWitness {
    let mut a = note_input(11, 700, 5);
    a.lock = lock;
    let mut b = note_input(12, 300, 6);
    // Positions 5 and 6 share all ancestors from level 2 upward: build consistent paths.
    let leaves_left = [rand_digest(), rand_digest()]; // positions 4 and 7
    let cm_a = a.commitment_of(&held);
    let cm_b = b.commitment_of(&held);
    a.path[0] = leaves_left[0]; // sibling of 5 is 4
    b.path[0] = leaves_left[1]; // sibling of 6 is 7
    let n45 = rescue::merkle_node(&leaves_left[0], &cm_a);
    let n67 = rescue::merkle_node(&cm_b, &leaves_left[1]);
    a.path[1] = n67;
    b.path[1] = n45;
    for k in 2..DEPTH {
        b.path[k] = a.path[k];
    }
    let root = merkle_root(&cm_a, 5, &a.path);
    assert_eq!(root, merkle_root(&cm_b, 6, &b.path));
    let recipient = SpendKeys::from_sk(rescue::scalar(99)).owner;
    SpendWitness {
        inputs: [a, b],
        outputs: [
            OutputWitness {
                owner: recipient,
                value: 900,
                rcm: rand_digest(),
            },
            OutputWitness {
                owner: rand_digest(),
                value: 90,
                rcm: rand_digest(),
            },
        ],
        root,
        fee: if asset.is_some() { 0 } else { 10 },
        mint: 0,
        sighash: rand_digest(),
        asset,
    }
    .rebalanced()
}

impl SpendWitness {
    /// Moves the GNR fee of the default witness into output 1 for fee-less coin spends.
    fn rebalanced(mut self) -> Self {
        let burn = self.asset.map_or(0, |a| a.burn);
        let total = self.inputs[0].value + self.inputs[1].value + self.mint;
        self.outputs[1].value = total - self.outputs[0].value - self.fee - burn;
        self
    }
}

const COIN: [u64; 3] = [0x1234_5678_9abc, 0x0fed_cba9_8765, 0];
const PAUSABLE: [u64; 3] = [0x1111_2222_3333, 0x4444_5555_6666, 1];

fn coin(id: [u64; 3], burn: u64, reveal: bool) -> Option<AssetWitness> {
    Some(AssetWitness { id, burn, reveal })
}

fn coin_witness(id: [u64; 3], burn: u64, reveal: bool) -> SpendWitness {
    witness_in(false, id, id, coin(id, burn, reveal))
}

#[test]
fn valid_spend_proves_and_verifies() {
    let w = valid_witness();
    let t = std::time::Instant::now();
    let (public, proof) = prove(&w).expect("prove");
    eprintln!("proof: {} bytes in {:?}", proof.len(), t.elapsed());
    let t = std::time::Instant::now();
    verify(&public, &proof).expect("verify");
    eprintln!("verify: {:?}", t.elapsed());
    assert_eq!(
        public.nullifiers[0],
        rescue::nullifier(&w.inputs[0].keys().nk, 5)
    );
}

#[test]
fn statement_tampering_is_rejected() {
    let (public, proof) = prove(&valid_witness()).unwrap();
    let mut variants = Vec::new();
    let mut p = public.clone();
    p.fee += 1;
    variants.push(p);
    let mut p = public.clone();
    p.mint = 5;
    variants.push(p);
    let mut p = public.clone();
    p.sighash[0] += Felt::ONE;
    variants.push(p);
    let mut p = public.clone();
    p.root[1] += Felt::ONE;
    variants.push(p);
    let mut p = public.clone();
    p.nullifiers.swap(0, 1);
    variants.push(p);
    let mut p = public.clone();
    p.output_cms[0][3] += Felt::ONE;
    variants.push(p);
    for v in variants {
        assert!(
            verify(&v, &proof).is_err(),
            "tampered statement accepted: {v:?}"
        );
    }
}

#[test]
fn proof_tampering_is_rejected() {
    let (public, proof) = prove(&valid_witness()).unwrap();
    for at in [10usize, proof.len() / 3, proof.len() / 2, proof.len() - 9] {
        let mut bad = proof.clone();
        bad[at] ^= 0x40;
        assert!(
            verify(&public, &bad).is_err(),
            "tampered proof byte {at} accepted"
        );
    }
    assert!(verify(&public, &proof[..proof.len() - 1]).is_err());
    assert!(verify(&public, &[]).is_err());
}

#[test]
fn coinbase_with_dummy_inputs() {
    let owner = SpendKeys::from_sk(rescue::scalar(7)).owner;
    let w = SpendWitness {
        inputs: [InputWitness::dummy(), InputWitness::dummy()],
        outputs: [
            OutputWitness {
                owner,
                value: 500_000_000,
                rcm: rand_digest(),
            },
            OutputWitness::dummy(),
        ],
        root: ZERO_DIGEST,
        fee: 0,
        mint: 500_000_000,
        sighash: rand_digest(),
        asset: None,
    };
    let (public, proof) = prove(&w).unwrap();
    verify(&public, &proof).unwrap();
    // Dummy nullifiers live in their own domain.
    assert_ne!(
        public.nullifiers[0],
        rescue::nullifier(&w.inputs[0].keys().nk, w.inputs[0].pos)
    );
}

#[test]
fn invalid_witnesses_are_refused_before_proving() {
    let mut w = valid_witness();
    w.fee += 1;
    assert!(matches!(prove(&w), Err(ZkError::Witness(_))));
    let mut w = valid_witness();
    w.inputs[0].value += 1;
    assert!(prove(&w).is_err());
    let mut w = valid_witness();
    w.outputs[0].value = MAX_VALUE + 1;
    assert!(prove(&w).is_err());
    let mut w = valid_witness();
    w.inputs[1].dummy = true;
    assert!(prove(&w).is_err());
}

/// A dishonest prover that skips the witness checks cannot produce a valid proof:
/// either the debug-mode trace validation panics or the proof fails verification.
#[test]
fn forged_unbalanced_spend_fails() {
    let mut w = valid_witness();
    w.outputs[0].value += 1000; // inflation attempt
    let public = w.public();
    let trace = build_trace(&w);
    let prover = SpendProver {
        options: proof_options(),
        public: public.clone(),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prover.prove(trace)));
    if let Ok(Ok(proof)) = result {
        assert!(verify(&public, &proof.to_bytes()).is_err());
    }
}

fn forged_proof_fails(w: &SpendWitness, public: &SpendPublic) {
    let trace = build_trace(w);
    let prover = SpendProver {
        options: proof_options(),
        public: public.clone(),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prover.prove(trace)));
    if let Ok(Ok(proof)) = result {
        assert!(verify(public, &proof.to_bytes()).is_err());
    }
}

#[test]
fn lock_spend_proves_and_binds_the_lock_owner() {
    let w = witness(true);
    let (public, proof) = prove(&w).expect("prove");
    let k = w.inputs[0].keys();
    assert_eq!(public.lock_owner, Some(rescue::lock_owner_of(&k.ak, &k.nk)));
    assert_ne!(public.lock_owner, Some(k.owner));
    verify(&public, &proof).expect("verify");
    // The same proof is not an ordinary spend, nor a spend of another lock note.
    let mut p = public.clone();
    p.lock_owner = None;
    assert!(verify(&p, &proof).is_err());
    let mut p = public.clone();
    p.lock_owner.as_mut().unwrap()[2] += Felt::ONE;
    assert!(verify(&p, &proof).is_err());
    // An ordinary proof does not pass as a lock spend.
    let (public, proof) = prove(&valid_witness()).unwrap();
    let mut p = public.clone();
    p.lock_owner = Some(SpendKeys::from_sk(rescue::scalar(11)).owner);
    assert!(verify(&p, &proof).is_err());
}

#[test]
fn lock_witness_rules() {
    let mut w = witness(true);
    w.inputs[1].lock = true;
    assert!(matches!(prove(&w), Err(ZkError::Witness(_))));
    let mut w = coinbase_witness();
    w.inputs[0].lock = true;
    assert!(matches!(prove(&w), Err(ZkError::Witness(_))));
}

/// Whoever knows a lock note's key cannot take it with an ordinary spend: the note
/// recomputes under the OWNER domain to a commitment that is not in the tree.
#[test]
fn ordinary_spend_of_a_lock_note_fails() {
    let mut w = witness(true);
    w.inputs[0].lock = false;
    assert!(prove(&w).is_err());
    let public = w.public();
    assert_eq!(public.lock_owner, None);
    forged_proof_fails(&w, &public);
}

/// A lock statement whose trace hashes the owner under OWNER is rejected.
#[test]
fn lock_statement_with_ordinary_owner_hash_fails() {
    let w = valid_witness();
    let mut public = w.public();
    public.lock_owner = Some(w.inputs[0].keys().owner);
    forged_proof_fails(&w, &public);
}

fn coinbase_witness() -> SpendWitness {
    let owner = SpendKeys::from_sk(rescue::scalar(7)).owner;
    SpendWitness {
        inputs: [InputWitness::dummy(), InputWitness::dummy()],
        outputs: [
            OutputWitness {
                owner,
                value: 500_000_000,
                rcm: rand_digest(),
            },
            OutputWitness::dummy(),
        ],
        root: ZERO_DIGEST,
        fee: 0,
        mint: 500_000_000,
        sighash: rand_digest(),
        asset: None,
    }
}

#[test]
fn forged_membership_fails() {
    let mut w = valid_witness();
    w.inputs[0].path[3] = rand_digest(); // note not under the claimed root
    let public = w.public();
    let trace = build_trace(&w);
    let prover = SpendProver {
        options: proof_options(),
        public: public.clone(),
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| prover.prove(trace)));
    if let Ok(Ok(proof)) = result {
        assert!(verify(&public, &proof.to_bytes()).is_err());
    }
}

#[test]
fn hidden_coin_transfer_proves_and_hides_the_coin() {
    let w = coin_witness(COIN, 0, false);
    let (public, proof) = prove(&w).expect("prove");
    let a = public.asset.as_ref().unwrap();
    assert_eq!((a.burn, a.reveal), (0, None));
    verify(&public, &proof).expect("verify");
    // The outputs commit to the coin, not to GNR.
    assert_eq!(public.output_cms[0], w.outputs[0].commitment_of(&COIN));
    assert_ne!(public.output_cms[0], w.outputs[0].commitment());
    // The proof is a coin proof only: not a GNR spend, not another coin, not a burn.
    let mut p = public.clone();
    p.asset = None;
    assert!(verify(&p, &proof).is_err());
    let mut p = public.clone();
    p.asset.as_mut().unwrap().reveal = Some([1, 2, 0]);
    assert!(verify(&p, &proof).is_err());
    let mut p = public.clone();
    p.asset.as_mut().unwrap().burn = 1;
    assert!(verify(&p, &proof).is_err());
    // And a GNR proof is not a coin proof.
    let (public, proof) = prove(&valid_witness()).unwrap();
    let mut p = public.clone();
    p.asset = Some(AssetStatement {
        burn: 0,
        reveal: None,
    });
    assert!(verify(&p, &proof).is_err());
}

#[test]
fn revealed_coin_binds_exactly_that_coin() {
    let w = coin_witness(PAUSABLE, 0, true);
    let (public, proof) = prove(&w).expect("prove");
    assert_eq!(public.asset.as_ref().unwrap().reveal, Some(PAUSABLE));
    verify(&public, &proof).unwrap();
    for other in [
        [PAUSABLE[0], PAUSABLE[1], 0],
        [PAUSABLE[0] + 1, PAUSABLE[1], 1],
        [0; 3],
    ] {
        let mut p = public.clone();
        p.asset.as_mut().unwrap().reveal = Some(other);
        assert!(verify(&p, &proof).is_err(), "{other:?}");
    }
    let mut p = public.clone();
    p.asset.as_mut().unwrap().reveal = None;
    assert!(
        verify(&p, &proof).is_err(),
        "a pausable coin passed as hidden"
    );
}

/// A pausable coin can never move hidden: honest provers refuse and a forged trace
/// fails the `pausable = 0` assertion, so a pause can always be enforced.
#[test]
fn pausable_coin_cannot_hide() {
    let w = coin_witness(PAUSABLE, 0, false);
    assert!(matches!(prove(&w), Err(ZkError::Witness(_))));
    forged_proof_fails(&w, &w.public());
}

/// Notes of one coin (or GNR) can never be spent as another coin: the commitments
/// recomputed with the claimed coin are not in the tree.
#[test]
fn counterfeiting_fails() {
    // GNR notes respent as a coin, a coin respent as GNR, coin A respent as coin B.
    let cases = [
        witness_in(false, [0; 3], COIN, coin(COIN, 0, false)),
        witness_in(false, COIN, [0; 3], None),
        witness_in(false, COIN, [9, 9, 0], coin([9, 9, 0], 0, false)),
    ];
    for w in cases {
        assert!(prove(&w).is_err());
        forged_proof_fails(&w, &w.public());
    }
    // A trace for the real coin presented with outputs of another coin.
    let w = coin_witness(COIN, 0, false);
    let mut public = w.public();
    public.output_cms[0] = w.outputs[0].commitment_of(&[9, 9, 0]);
    forged_proof_fails(&w, &public);
    // Outputs worth more than the inputs.
    let mut w = coin_witness(COIN, 0, false);
    w.outputs[0].value += 1;
    assert!(prove(&w).is_err());
    forged_proof_fails(&w, &w.public());
}

/// Fees are GNR: a coin spend carries no fee, the fee comes from a separate GNR spend.
#[test]
fn fees_are_gnr_only() {
    let mut w = coin_witness(COIN, 0, false);
    w.fee = 10;
    w = w.rebalanced();
    assert!(matches!(prove(&w), Err(ZkError::Witness(_))));
    forged_proof_fails(&w, &w.public());
    // A version-2 GNR spend may pay a fee.
    let mut w = coin_witness([0; 3], 0, false);
    w.fee = 10;
    w = w.rebalanced();
    let (public, proof) = prove(&w).unwrap();
    verify(&public, &proof).unwrap();
}

#[test]
fn burn_and_issue() {
    let w = coin_witness(COIN, 50, true);
    assert_eq!(w.outputs[1].value + w.outputs[0].value + 50, 1000);
    let (public, proof) = prove(&w).unwrap();
    verify(&public, &proof).unwrap();
    let mut p = public.clone();
    p.asset.as_mut().unwrap().burn = 49;
    assert!(verify(&p, &proof).is_err());
    // Burning more than is spent is impossible.
    let mut w = coin_witness(COIN, 0, true);
    w.asset.as_mut().unwrap().burn = 1;
    assert!(prove(&w).is_err());
    forged_proof_fails(&w, &w.public());
    // Issue: dummy inputs, minted units of a public coin.
    let mut w = coinbase_witness();
    w.asset = coin(COIN, 0, true);
    let (public, proof) = prove(&w).unwrap();
    verify(&public, &proof).unwrap();
    assert_eq!(public.mint, 500_000_000);
    let mut p = public.clone();
    p.asset.as_mut().unwrap().reveal = Some([0; 3]);
    assert!(
        verify(&p, &proof).is_err(),
        "a coin issue passed as a GNR mint"
    );
}

#[test]
fn coin_lock_spend_works() {
    let w = witness_in(true, COIN, COIN, coin(COIN, 0, false));
    let (public, proof) = prove(&w).unwrap();
    assert!(public.lock_owner.is_some());
    verify(&public, &proof).unwrap();
}

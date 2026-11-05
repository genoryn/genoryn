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
    let mut a = note_input(11, 700, 5);
    a.lock = lock;
    let mut b = note_input(12, 300, 6);
    // Positions 5 and 6 share all ancestors from level 2 upward: build consistent paths.
    let leaves_left = [rand_digest(), rand_digest()]; // positions 4 and 7
    let cm_a = a.commitment();
    let cm_b = b.commitment();
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
        fee: 10,
        mint: 0,
        sighash: rand_digest(),
    }
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

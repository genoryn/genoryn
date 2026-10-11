//! Payment Proof 2.0 on a real devnet chain: real RandomX blocks, Halo 2
//! coinbase and transfer proofs, Orchard note encryption. Alice pays Bob
//! exactly 10 GNR and answers Bob's fresh challenge; impostors, replays, other
//! payments, other networks and reorganizations are rejected.
use genoryn_core::{
    block::Block,
    chain::{Chain, unix_time},
    params::{MIN_FEE, Network},
    pow,
    privacy::ShieldedTx,
};
use genoryn_wallet_core::{
    keys::{Wallet, encode_address},
    scan::{NoteProof, ScannedWallet, verify_payment_proof},
    sender_auth::{self, Challenge, MemoTrailer, Response},
};
use orchard::keys::Scope;
use std::collections::BTreeSet;

const GNR: u64 = 1_000_000_000_000;
const NETWORK: Network = Network::Devnet;

fn mine(mut block: Block) -> Block {
    loop {
        let hash = pow::randomx(block.header.pow_seed, &block.header.bytes().unwrap()).unwrap();
        if pow::meets_difficulty(hash, block.header.difficulty) {
            return block;
        }
        block.header.nonce += 1;
    }
}
fn extend(chain: &mut Chain, to: &Wallet) -> Block {
    let block = mine(chain.template(to.address(0).unwrap()).unwrap());
    assert!(chain.accept(block.clone()).unwrap().became_best);
    block
}
fn scan(wallet: &Wallet, chain: &Chain) -> ScannedWallet {
    wallet.scan(&chain.active).unwrap()
}
fn own_nullifiers(wallet: &Wallet, scanned: &ScannedWallet) -> BTreeSet<[u8; 32]> {
    let fvk = wallet.viewing_key().unwrap();
    scanned
        .notes
        .iter()
        .map(|owned| owned.note.nullifier(&fvk).to_bytes())
        .collect()
}
fn find(chain: &Chain, txid: [u8; 32]) -> Option<(u64, ShieldedTx)> {
    chain.active.iter().find_map(|block| {
        block
            .body
            .all_transactions()
            .find(|tx| tx.id().unwrap() == txid)
            .map(|tx| (block.header.height, tx.clone()))
    })
}
/// What the paying wallet does with a challenge: find its own output, check
/// what the challenge says about it, derive the committed key and sign.
fn answer_as(
    wallet: &Wallet,
    chain: &Chain,
    challenge: &Challenge,
    now: u64,
) -> anyhow::Result<Response> {
    let (_, tx) = find(chain, challenge.txid).ok_or_else(|| anyhow::anyhow!("unknown"))?;
    let output = wallet
        .sent_output(&tx, challenge.action as usize)?
        .ok_or_else(|| anyhow::anyhow!("this wallet did not make this output"))?;
    anyhow::ensure!(
        output.recipient.to_raw_address_bytes() == challenge.recipient
            && output.note.value().inner() == challenge.amount,
        "the challenge describes a different payment"
    );
    let committed = MemoTrailer::read(&output.memo)
        .ok_or_else(|| anyhow::anyhow!("legacy payment without sender verification"))?;
    let scanned = scan(wallet, chain);
    let anchor = Wallet::anchor_nullifier(&tx, &own_nullifiers(wallet, &scanned))
        .ok_or_else(|| anyhow::anyhow!("no own spend"))?;
    let key = sender_auth::find_payment_key(
        wallet,
        anchor,
        &output.recipient,
        output.note.value().inner(),
        &committed,
    )?
    .ok_or_else(|| anyhow::anyhow!("not this wallet's key"))?;
    sender_auth::answer(challenge, &committed, &key, now)
}

#[test]
#[ignore = "real Halo 2 proofs and RandomX blocks; scripts/test/all.py runs it"]
fn alice_proves_ten_gnr_to_bob_and_answers_a_fresh_challenge() {
    let directory = tempfile::tempdir().unwrap();
    let mut chain = Chain::open(&directory.path().join("chain.sqlite"), NETWORK).unwrap();
    let alice = Wallet::create(NETWORK).unwrap();
    let bob = Wallet::create(NETWORK).unwrap();
    let carol = Wallet::create(NETWORK).unwrap();
    let bob_address = bob.address(0).unwrap();
    for _ in 0..3 {
        extend(&mut chain, &alice);
    }
    let fork_base = chain.active.clone();

    // 1. Alice sends exactly 10 GNR to Bob with a memo and a sender commitment.
    let scanned = scan(&alice, &chain);
    let paid = alice
        .send(&scanned, bob_address, 10 * GNR, MIN_FEE, "invoice 42", true)
        .unwrap();
    let txid = paid.id().unwrap();
    chain.submit(paid.clone()).unwrap();
    let mined_in = extend(&mut chain, &alice).header.height;
    extend(&mut chain, &alice);
    let (height, tx) = find(&chain, txid).expect("payment mined");
    assert_eq!(height, mined_in);

    // 2. Alice's proof; 3. Bob checks amount, recipient and the commitment.
    let proof: NoteProof = alice.payment_proof_auto(&tx).unwrap();
    assert!(proof.sender_key.is_some(), "made by the paying wallet");
    let proven = verify_payment_proof(&proof, &tx).unwrap();
    assert_eq!(proven.amount_atomic, (10 * GNR).to_string());
    assert_eq!(
        proven.recipient,
        encode_address(NETWORK, bob_address).unwrap()
    );
    assert_eq!(proven.memo, "invoice 42", "the trailer never shows as text");
    let committed_hex = proven.sender_auth_key.clone().expect("committed key");
    let committed = MemoTrailer {
        key: hex::decode(&committed_hex).unwrap().try_into().unwrap(),
    };
    // Bob's own wallet reads the same commitment from the note it received.
    let received = bob
        .received_output(&tx, proof.action as usize)
        .unwrap()
        .unwrap();
    assert_eq!(MemoTrailer::read(&received.memo), Some(committed));
    let bob_scan = scan(&bob, &chain);
    assert_eq!(bob_scan.notes.len(), 1);
    assert_eq!(bob_scan.notes[0].note.value().inner(), 10 * GNR);
    assert_eq!(bob_scan.notes[0].memo, "invoice 42");

    // 4. Bob's fresh challenge; 5. Alice signs; 6. Bob verifies.
    let now = unix_time();
    let challenge = Challenge::new(
        NETWORK,
        txid,
        proof.action as u16,
        &bob_address,
        10 * GNR,
        now,
        3_600,
    )
    .unwrap();
    let challenge = Challenge::parse(&challenge.code()).unwrap();
    let response =
        Response::parse(&answer_as(&alice, &chain, &challenge, now).unwrap().code()).unwrap();
    sender_auth::verify(&challenge, &response, &committed, now).unwrap();
    // The same seed restored elsewhere answers too: nothing extra to back up.
    let restored = Wallet::restore(NETWORK, &alice.phrase().unwrap()).unwrap();
    let again = answer_as(&restored, &chain, &challenge, now).unwrap();
    sender_auth::verify(&challenge, &again, &committed, now).unwrap();

    // 7. Another wallet cannot impersonate Alice; 8. neither can Bob with his
    // receiving keys: neither sees the output as its own payment, and neither
    // derives the committed key from its own seed.
    for impostor in [&carol, &bob] {
        assert!(answer_as(impostor, &chain, &challenge, now).is_err());
        let anchor =
            Wallet::anchor_nullifier(&tx, &own_nullifiers(&alice, &scan(&alice, &chain))).unwrap();
        assert!(
            sender_auth::find_payment_key(impostor, anchor, &bob_address, 10 * GNR, &committed)
                .unwrap()
                .is_none()
        );
    }

    // 9. A copied old response does not satisfy a new challenge.
    let fresh = Challenge::new(
        NETWORK,
        txid,
        proof.action as u16,
        &bob_address,
        10 * GNR,
        now,
        3_600,
    )
    .unwrap();
    assert!(sender_auth::verify(&fresh, &response, &committed, now).is_err());

    // 10.-12. A different payment, recipient or amount: Alice's wallet refuses
    // to answer, and a response to the real one does not verify for them.
    let carol_address = carol.address(0).unwrap();
    for wrong in [
        Challenge::new(
            NETWORK,
            [9; 32],
            proof.action as u16,
            &bob_address,
            10 * GNR,
            now,
            3_600,
        ),
        Challenge::new(
            NETWORK,
            txid,
            proof.action as u16,
            &carol_address,
            10 * GNR,
            now,
            3_600,
        ),
        Challenge::new(
            NETWORK,
            txid,
            proof.action as u16,
            &bob_address,
            11 * GNR,
            now,
            3_600,
        ),
    ] {
        let wrong = wrong.unwrap();
        assert!(answer_as(&alice, &chain, &wrong, now).is_err());
        assert!(sender_auth::verify(&wrong, &response, &committed, now).is_err());
    }
    // 13. A challenge for another network is rejected.
    let testnet = Challenge {
        network: Network::Testnet,
        ..challenge.clone()
    };
    assert!(sender_auth::verify(&testnet, &response, &committed, now).is_err());

    // 14. A modified proof is rejected.
    let mut tampered = alice.payment_proof_auto(&tx).unwrap();
    tampered.amount += 1;
    assert!(verify_payment_proof(&tampered, &tx).is_err());
    let mut tampered = alice.payment_proof_auto(&tx).unwrap();
    tampered.sender_key.as_mut().unwrap()[0] ^= 1;
    assert!(verify_payment_proof(&tampered, &tx).is_err());

    // 15. A transaction that is not on the chain is not found.
    assert!(find(&chain, [7; 32]).is_none());

    // 18. A payment without the commitment (sender verification switched off)
    // keeps its ordinary proof, and fresh authentication is not offered.
    let scanned = scan(&alice, &chain);
    let legacy = alice
        .send(&scanned, bob_address, GNR, MIN_FEE, "", false)
        .unwrap();
    let legacy_id = legacy.id().unwrap();
    chain.submit(legacy).unwrap();
    extend(&mut chain, &alice);
    let (_, legacy_tx) = find(&chain, legacy_id).unwrap();
    let legacy_proof = alice.payment_proof_auto(&legacy_tx).unwrap();
    let legacy_proven = verify_payment_proof(&legacy_proof, &legacy_tx).unwrap();
    assert!(legacy_proven.created_by_sender);
    assert_eq!(legacy_proven.sender_auth_key, None);
    let legacy_challenge = Challenge::new(
        NETWORK,
        legacy_id,
        legacy_proof.action as u16,
        &bob_address,
        GNR,
        now,
        3_600,
    )
    .unwrap();
    assert!(
        answer_as(&alice, &chain, &legacy_challenge, now)
            .unwrap_err()
            .to_string()
            .contains("legacy")
    );
    // A memo too long for the commitment is sent whole, without one.
    let scanned = scan(&alice, &chain);
    let long_memo = "m".repeat(sender_auth::MAX_MEMO_WITH_AUTH + 4);
    let long = alice
        .send(&scanned, bob_address, GNR, MIN_FEE, &long_memo, true)
        .unwrap();
    let long_id = long.id().unwrap();
    chain.submit(long).unwrap();
    extend(&mut chain, &alice);
    let (_, long_tx) = find(&chain, long_id).unwrap();
    let long_proven =
        verify_payment_proof(&alice.payment_proof_auto(&long_tx).unwrap(), &long_tx).unwrap();
    assert_eq!(long_proven.memo, long_memo);
    assert_eq!(long_proven.sender_auth_key, None);

    // 20. Bob's view of Alice's transaction shows only his own output: the
    // change and every other output stay closed to him.
    let bundle = tx.bundle.to_bundle().unwrap();
    let bob_ivks = [
        bob.viewing_key().unwrap().to_ivk(Scope::External),
        bob.viewing_key().unwrap().to_ivk(Scope::Internal),
    ];
    assert_eq!(bundle.decrypt_outputs_with_keys(&bob_ivks).len(), 1);
    assert_eq!(
        bundle
            .recover_outputs_with_ovks(&[bob.viewing_key().unwrap().to_ovk(Scope::External)])
            .len(),
        0
    );

    // 16. A heavier competing chain without the payment: it is no longer on
    // the chain the verifier follows, so it must not count as confirmed.
    let other = tempfile::tempdir().unwrap();
    let mut rival = Chain::open(&other.path().join("chain.sqlite"), NETWORK).unwrap();
    for block in &fork_base {
        assert!(rival.accept(block.clone()).unwrap().became_best);
    }
    let lead = chain.active.len() - fork_base.len() + 1;
    for _ in 0..lead {
        let block = extend(&mut rival, &carol);
        chain.accept(block).unwrap();
    }
    assert_eq!(chain.state.best_hash, rival.state.best_hash);
    assert!(find(&chain, txid).is_none(), "the payment left the chain");
}

//! Payment Proof 2.0: outcomes of a proof check on the verified chain, and the
//! fresh sender-verification challenge.
//!
//! - `verify_proof` keeps its 1.0.5 fields and adds `outcome` and
//!   `sender_evidence`. With `structured_outcomes`, a payment that is not on
//!   the verified chain is an outcome (`not_on_chain` or `reorganized`) instead
//!   of an error.
//! - `create_payment_challenge` needs neither a wallet nor the chain: it binds a
//!   fresh nonce to a payment the verifier has already checked.
//! - `answer_payment_challenge` runs in the paying wallet and signs only if
//!   the challenge describes one of its own payments exactly.
//! - `verify_payment_challenge` checks the payment on the verified chain again,
//!   reads the committed key from the payment's own memo and checks the
//!   signature against the verifier's challenge.
use super::*;
use genoryn_wallet_core::{
    scan::OutputView,
    sender_auth::{self, Challenge, MemoTrailer, Response},
};

/// Below this many confirmations a reorganization is a realistic risk.
const SETTLED_CONFIRMATIONS: u64 = 10;

/// A transaction of the verified chain: its block height and bytes.
pub(super) fn find_on_chain<'a>(
    blocks: &'a [Block],
    txid: &str,
) -> Option<(u64, &'a genoryn_core::privacy::ShieldedTx)> {
    blocks.iter().find_map(|block| {
        block
            .body
            .all_transactions()
            .find(|tx| tx.id().map(hex::encode).ok().as_deref() == Some(txid))
            .map(|tx| (block.header.height, tx))
    })
}

fn reorg_risk(confirmations: u64) -> &'static str {
    if confirmations < 2 {
        "high: a competing block can still replace this one"
    } else if confirmations < SETTLED_CONFIRMATIONS {
        "present: fewer than 10 confirmations"
    } else {
        "low: 10 or more confirmations; reorganizations remain possible in principle"
    }
}

/// The three separate levels of assurance a proof can give about its sender.
fn sender_evidence(created_by_sender: bool, committed: bool) -> &'static str {
    match (created_by_sender, committed) {
        (_, true) => "fresh_authentication_available",
        (true, false) => "origin_disclosure_only",
        (false, false) => "none",
    }
}

/// Checks `proof` against the verified `blocks` and adds the 2.0 fields.
pub(super) fn verify_proof(
    request: &Value,
    network: Network,
    wallet: Option<&Wallet>,
    blocks: &[Block],
) -> Result<Value> {
    let tip = blocks.last().map_or(0, |block| block.header.height);
    let proof: NoteProof =
        serde_json::from_value(request["proof"].clone()).context("not a payment proof")?;
    ensure!(
        proof.network == network,
        "this payment proof is for another network"
    );
    let Some((height, tx)) = find_on_chain(blocks, &proof.txid) else {
        ensure!(
            request["structured_outcomes"].as_bool() == Some(true),
            "this payment is not in a block yet (or not on this chain): verify again after it is mined"
        );
        let previously = request["previously_confirmed_at"].as_u64();
        return Ok(json!({
            "outcome": if previously.is_some() { "reorganized" } else { "not_on_chain" },
            "txid": proof.txid,
            "action": proof.action,
            "chain_tip": tip,
            "previously_confirmed_at": previously,
        }));
    };
    let paid = decode_address(network, &proof.recipient)?;
    let proven = verify_payment_proof(&proof, tx)?;
    let mut sender_key = proven.sender_auth_key.clone();
    let mut result = serde_json::to_value(&proven)?;
    // A proof made by the receiving side does not open the memo; the
    // recipient's own wallet can read the commitment from its note.
    if sender_key.is_none()
        && let Some(wallet) = wallet
        && let Some(view) = wallet.received_output(tx, proof.action as usize)?
        && view.recipient.to_raw_address_bytes() == paid.to_raw_address_bytes()
    {
        sender_key = MemoTrailer::read(&view.memo).map(|trailer| hex::encode(trailer.key));
        result["sender_auth_key"] = json!(sender_key);
    }
    let confirmations = tip.saturating_sub(height) + 1;
    result["outcome"] = json!("payment_verified");
    result["sender_evidence"] = json!(sender_evidence(
        proven.created_by_sender,
        sender_key.is_some()
    ));
    result["txid"] = json!(proof.txid);
    result["action"] = json!(proof.action);
    result["block_height"] = json!(height);
    result["chain_tip"] = json!(tip);
    result["confirmations"] = json!(confirmations);
    result["reorg_risk"] = json!(reorg_risk(confirmations));
    if let Some(expected) = request["expected_recipient"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        let expected = decode_address(network, expected)
            .context("the address to check is not a valid GENORYN address")?;
        result["recipient_matches"] =
            json!(expected.to_raw_address_bytes() == paid.to_raw_address_bytes());
    }
    if let Some(expected) = request["expected_amount_atomic"]
        .as_str()
        .map(str::trim)
        .filter(|text| !text.is_empty())
    {
        let expected: u64 = expected
            .parse()
            .context("the amount to check is not a whole number of gori")?;
        result["amount_matches"] = json!(expected == proof.amount);
    }
    if let Some(wallet) = wallet {
        result["belongs_to_wallet"] = json!(wallet.owns_address(&paid)?);
    }
    Ok(result)
}

/// A fresh challenge about a payment the verifier has checked. Pure: no wallet,
/// no chain, nothing sent anywhere.
pub(super) fn create_challenge(request: &Value, network: Network) -> Result<Value> {
    let txid: [u8; 32] = hex::decode(field(request, "txid")?)
        .ok()
        .and_then(|bytes| bytes.try_into().ok())
        .context("txid must be 64 hex characters")?;
    let action = u16::try_from(request["action"].as_u64().context("missing output index")?)
        .context("output index out of range")?;
    let recipient = decode_address(network, field(request, "recipient")?)
        .context("the receiving address is not a valid GENORYN address for this network")?;
    let amount: u64 = field(request, "amount_atomic")?
        .parse()
        .context("amount_atomic must be a whole number of gori")?;
    let seconds = request["valid_seconds"]
        .as_u64()
        .unwrap_or(sender_auth::DEFAULT_CHALLENGE_SECONDS);
    let challenge = Challenge::new(
        network,
        txid,
        action,
        &recipient,
        amount,
        unix_time(),
        seconds,
    )?;
    let mut summary = serde_json::to_value(&challenge)?;
    summary["recipient"] = json!(genoryn_wallet_core::keys::encode_address(
        network, recipient
    )?);
    summary["challenge"] = json!(challenge.code());
    summary["note"] = json!(
        "Send this code to the person who says they made the payment. Their wallet answers it only if it created this exact payment."
    );
    Ok(summary)
}

/// The paying wallet's view of the output a challenge names.
fn own_payment(
    wallet: &Wallet,
    tx: &genoryn_core::privacy::ShieldedTx,
    challenge: &Challenge,
) -> Result<OutputView> {
    let view = wallet
        .sent_output(tx, challenge.action as usize)?
        .context("this wallet did not make this payment, so it cannot answer for its sender")?;
    ensure!(
        !wallet.owns_address(&view.recipient)?,
        "this output is this wallet's own change, not a payment to someone"
    );
    ensure!(
        view.recipient.to_raw_address_bytes() == challenge.recipient,
        "the challenge names a different receiving address than this payment's"
    );
    ensure!(
        view.note.value().inner() == challenge.amount,
        "the challenge names a different amount than this payment's ({} gori)",
        view.note.value().inner()
    );
    Ok(view)
}

/// Signs a challenge about one of this wallet's payments (mined, or still in
/// its outbox). Nothing secret leaves the wallet: only a signature.
pub(super) fn answer_challenge(
    request: &Value,
    network: Network,
    wallet: &Wallet,
    path: &Path,
    blocks: &[Block],
) -> Result<Value> {
    let challenge = Challenge::parse(field(request, "challenge")?)?;
    ensure!(
        challenge.network == network,
        "this challenge is for the GENORYN {}, not the {}",
        challenge.network.name(),
        network.name()
    );
    let now = unix_time();
    challenge.ensure_open(now)?;
    let txid = hex::encode(challenge.txid);
    let outbox = outbox::load(wallet, &outbox::outbox_path(path))?;
    let stored;
    let (mined_at, tx) = match find_on_chain(blocks, &txid) {
        Some((height, tx)) => (Some(height), tx),
        None => {
            stored = outbox
                .entries
                .iter()
                .find(|entry| hex::encode(entry.txid) == txid)
                .context("this payment is neither on the chain nor among this wallet's payments")?
                .transaction()?;
            (None, &stored)
        }
    };
    let view = own_payment(wallet, tx, &challenge)?;
    let committed = MemoTrailer::read(&view.memo).context(
        "this payment was made without a sender-verification key (a legacy payment, or sent with it switched off); share an ordinary payment proof instead",
    )?;
    let cache = sync::load_cache(wallet, &sync::cache_path(path));
    let own: std::collections::BTreeSet<[u8; 32]> =
        cache.notes.iter().map(|note| note.nullifier).collect();
    let anchor = Wallet::anchor_nullifier(tx, &own).context(
        "this wallet's record of the notes it spent is incomplete; sync (or rescan) and try again",
    )?;
    let key = sender_auth::find_payment_key(
        wallet,
        anchor,
        &view.recipient,
        view.note.value().inner(),
        &committed,
    )?
    .context("this wallet did not create this payment's sender-verification key")?;
    let response = sender_auth::answer(&challenge, &committed, &key, now)?;
    Ok(json!({
        "response": response.code(),
        "txid": txid,
        "action": challenge.action,
        "amount_atomic": challenge.amount.to_string(),
        "recipient": genoryn_wallet_core::keys::encode_address(network, view.recipient)?,
        "mined_at": mined_at,
        "expires": challenge.expires,
        "note": "Send this response back to the person who asked. It proves this wallet holds the key of this payment; it reveals no key, balance or other payment.",
    }))
}

/// Checks a response against the verifier's own challenge and the key the
/// payment committed to on the verified chain.
pub(super) fn verify_challenge(
    request: &Value,
    network: Network,
    wallet: Option<&Wallet>,
    blocks: &[Block],
) -> Result<Value> {
    let challenge = Challenge::parse(field(request, "challenge")?)?;
    ensure!(
        challenge.network == network,
        "this challenge is for another network"
    );
    let response = Response::parse(field(request, "response")?)?;
    let txid = hex::encode(challenge.txid);
    let tip = blocks.last().map_or(0, |block| block.header.height);
    let (height, tx) = find_on_chain(blocks, &txid)
        .context("this payment is not on the verified chain (not mined yet, or replaced by a reorganization)")?;
    // The payment itself, from the proof if one is given, else from the note
    // the verifying wallet received.
    let (recipient, amount, memo_key, by_sender) = if !request["proof"].is_null() {
        let proof: NoteProof =
            serde_json::from_value(request["proof"].clone()).context("not a payment proof")?;
        ensure!(
            proof.txid == txid && proof.action == challenge.action as u32,
            "the payment proof is for a different payment than the challenge"
        );
        let proven = verify_payment_proof(&proof, tx)?;
        let key = proven.sender_auth_key.clone();
        (
            decode_address(network, &proof.recipient)?,
            proof.amount,
            key,
            proven.created_by_sender,
        )
    } else {
        let view = wallet
            .and_then(|wallet| {
                wallet
                    .received_output(tx, challenge.action as usize)
                    .ok()
                    .flatten()
            })
            .context("paste the payment proof too, or open the wallet that received the payment")?;
        (
            view.recipient,
            view.note.value().inner(),
            MemoTrailer::read(&view.memo).map(|trailer| hex::encode(trailer.key)),
            false,
        )
    };
    ensure!(
        recipient.to_raw_address_bytes() == challenge.recipient,
        "recipient mismatch: the payment did not go to the address in the challenge"
    );
    ensure!(
        amount == challenge.amount,
        "amount mismatch: the payment is {amount} gori, the challenge says {}",
        challenge.amount
    );
    let confirmations = tip.saturating_sub(height) + 1;
    let base = json!({
        "txid": txid,
        "action": challenge.action,
        "amount_atomic": amount.to_string(),
        "recipient": genoryn_wallet_core::keys::encode_address(network, recipient)?,
        "block_height": height,
        "chain_tip": tip,
        "confirmations": confirmations,
        "reorg_risk": reorg_risk(confirmations),
        "created_by_sender": by_sender,
    });
    let Some(key) = memo_key else {
        let mut result = base;
        result["outcome"] = json!("legacy_sender_evidence_only");
        result["note"] = json!(
            "The payment is real, but it carries no sender-verification key, so fresh sender authentication is not possible for it."
        );
        return Ok(result);
    };
    let committed = MemoTrailer {
        key: hex::decode(&key)?
            .try_into()
            .map_err(|_| anyhow::anyhow!("committed key length"))?,
    };
    sender_auth::verify(&challenge, &response, &committed, unix_time())?;
    let mut result = base;
    result["outcome"] = json!("sender_verified");
    result["sender_auth_key"] = json!(key);
    result["meaning"] = json!(
        "Whoever answered holds the secret key this payment committed to when it was made. It identifies the payment's credential, not a person's legal identity."
    );
    Ok(result)
}

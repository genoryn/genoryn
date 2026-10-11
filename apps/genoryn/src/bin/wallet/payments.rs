//! `send_many`: one private transfer to up to 15 recipients, made at most once
//! per `reference` (a pool's payout batch id, for example).
//!
//! The reference and the transaction id are sealed into `wallet.references`
//! before the transaction enters the outbox, and the outbox before anything is
//! relayed. A retry with the same reference therefore finds either the stored
//! payment (and returns it, never building a second one) or a transaction
//! that was never stored or relayed (which can never confirm), so a crash at
//! any point cannot pay twice. Existing wallet files are not changed.
use super::*;
use genoryn_wallet_core::{scan::PaymentOutput, sealed, sender_auth};

const REFERENCES_DOMAIN: &[u8] = b"GENORYN/payment-references/v1";
const REFERENCES_VERSION: u16 = 1;
const MAX_REFERENCES: usize = 4_096;
const MAX_REFERENCE_BYTES: usize = 128;

type References = Vec<(String, [u8; 32])>;

fn references_path(path: &Path) -> PathBuf {
    path.with_extension("references")
}
fn load_references(wallet: &Wallet, path: &Path) -> Result<References> {
    Ok(sealed::load(
        wallet,
        &references_path(path),
        REFERENCES_DOMAIN,
        REFERENCES_VERSION,
        4 * 1024 * 1024,
    )?
    .unwrap_or_default())
}
fn reference(request: &Value) -> Result<Option<&str>> {
    let Some(reference) = request["reference"].as_str() else {
        return Ok(None);
    };
    ensure!(
        !reference.is_empty()
            && reference.len() <= MAX_REFERENCE_BYTES
            && reference
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_'),
        "a payment reference is 1-128 letters, digits, '-' or '_'"
    );
    Ok(Some(reference))
}

/// The stored payment of a reference already used, so a retry returns it
/// instead of paying again.
pub(super) fn already_sent(wallet: &Wallet, path: &Path, request: &Value) -> Result<Option<Value>> {
    let Some(reference) = reference(request)? else {
        return Ok(None);
    };
    let Some((_, txid)) = load_references(wallet, path)?
        .into_iter()
        .find(|(known, _)| known == reference)
    else {
        return Ok(None);
    };
    let outbox = outbox::load(wallet, &outbox::outbox_path(path))?;
    Ok(outbox
        .entries
        .iter()
        .find(|entry| entry.txid == txid)
        .map(|entry| {
            json!({
                "txid": hex::encode(entry.txid),
                "status": entry.status.label(),
                "acknowledged": entry.status != outbox::OutboxStatus::Pending,
                "confirmed_height": entry.confirmed_height,
                "reference": reference,
                "reused": true,
                "note": "This reference was already paid by this wallet; the stored payment is returned and nothing new was sent.",
            })
        }))
}

pub(super) async fn send_many(
    request: &Value,
    network: Network,
    wallet: &Wallet,
    path: &Path,
    scanned: &genoryn_wallet_core::scan::ScannedWallet,
    outbox: &mut outbox::Outbox,
    relayer: &Relayer<'_>,
) -> Result<Value> {
    let fee = request["fee_atomic"]
        .as_str()
        .unwrap_or("10000000")
        .parse::<u64>()?;
    let list = request["outputs"]
        .as_array()
        .context("send_many needs an outputs list")?;
    ensure!(
        (1..genoryn_core::codec::MAX_ACTIONS).contains(&list.len()),
        "one transfer pays 1-15 recipients"
    );
    let mut outputs = Vec::with_capacity(list.len());
    let mut total = 0u64;
    for item in list {
        let recipient = decode_address(network, field(item, "to")?)
            .with_context(|| not_a_recipient(network))?;
        let amount: u64 = field(item, "amount_atomic")?.parse()?;
        let text = item["memo"].as_str().unwrap_or("");
        ensure!(
            text.len() <= 480 && !text.contains('\0'),
            "a memo is at most 480 bytes without NUL characters"
        );
        let mut memo = [0u8; 512];
        memo[..text.len()].copy_from_slice(text.as_bytes());
        total = total.checked_add(amount).context("amount overflow")?;
        outputs.push(PaymentOutput {
            recipient,
            amount,
            memo,
            sender_auth: request["sender_auth"].as_bool() != Some(false)
                && text.len() <= sender_auth::MAX_MEMO_WITH_AUTH,
        });
    }
    let tx = wallet.send_payments(scanned, &outputs, fee)?;
    if let Some(reference) = reference(request)? {
        let mut references = load_references(wallet, path)?;
        references.retain(|(known, _)| known != reference);
        references.push((reference.to_owned(), tx.id()?));
        if references.len() > MAX_REFERENCES {
            references.drain(..references.len() - MAX_REFERENCES);
        }
        sealed::save(
            wallet,
            &references_path(path),
            REFERENCES_DOMAIN,
            REFERENCES_VERSION,
            &references,
        )
        .context("the payment was not sent: its reference could not be stored safely first")?;
    }
    let payee = Payee {
        recipient: format!("{} recipients", outputs.len()),
        amount: total,
        fee,
    };
    let mut submitted = submit_durably(wallet, path, outbox, &tx, payee, relayer).await?;
    submitted["reference"] = json!(reference(request)?);
    submitted["recipients"] = json!(outputs.len());
    submitted["amount_atomic"] = json!(total.to_string());
    submitted["reused"] = json!(false);
    Ok(submitted)
}

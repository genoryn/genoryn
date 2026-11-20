//! Bitcoin a wallet keeps ready for one of its buy offers.
//!
//! A buyer cannot put bitcoin into the swap contract before a seller exists: the contract
//! names the seller's key and the hash of the seller's secret. What a buyer can do is pay
//! the bitcoin to an address of its own wallet when it publishes the offer. From there the
//! wallet pays the contract by itself the moment a seller's GNR is locked and confirmed,
//! and sends the bitcoin back to the buyer if the offer ends unsold.
//!
//! The address is a plain pay-to-witness-public-key-hash address whose key comes from the
//! wallet's seed and a small number, so the 24 words find it again.

use crate::btc::Utxo;
use anyhow::{Context, Result, bail, ensure};
use bitcoin::absolute::LockTime;
use bitcoin::hashes::Hash;
use bitcoin::secp256k1::{Message, Secp256k1, SecretKey};
use bitcoin::sighash::{EcdsaSighashType, SighashCache};
use bitcoin::transaction::Version;
use bitcoin::{
    Address, Amount, CompressedPublicKey, FeeRate, Network, Script, ScriptBuf, Sequence,
    Transaction, TxIn, TxOut, Witness,
};

/// Virtual size assumed for the payment into the contract when the fee is set aside
/// (two inputs, the contract output and a change output, with room to spare).
pub const PAY_VSIZE: u64 = 230;
/// The least that is set aside for the fee of that payment.
pub const MIN_FEE_RESERVE: u64 = 1_000;
/// The lowest fee rate the network relays.
const MIN_RELAY_SAT_VB: u64 = 1;
/// No transaction of a deposit pays more fee than a tenth of what it moves (or this many
/// sat, for small deposits), whatever fee rate a Bitcoin server reports.
const FEE_CEILING_SAT: u64 = 3_000;
/// The most outputs one transaction spends (the largest first).
pub const MAX_COINS: usize = 50;
/// The number of the wallet's Bitcoin balance (1.0.46): one address of the wallet's own
/// that it keeps bitcoin at for Quick Swaps. Buying pays from it, selling is paid to it,
/// and a Quick Swap buy offer is funded from it. Outside the numbers of offer deposits.
pub const BALANCE: u32 = 0xB000_0000;
/// The most payments one transaction from a deposit makes.
pub const MAX_PAYMENTS: usize = 8;

/// The key of deposit number `n` of a wallet.
pub fn key(seed: &[u8], n: u32) -> SecretKey {
    (0u32..)
        .find_map(|c| {
            SecretKey::from_slice(&crate::tagged("Genoryn/swap/deposit-key", seed, n, c)).ok()
        })
        .expect("a valid key is found within a few tries")
}

fn compressed(sk: &SecretKey) -> CompressedPublicKey {
    CompressedPublicKey(sk.public_key(&Secp256k1::signing_only()))
}

/// The address deposit number `n` is paid to.
pub fn address(seed: &[u8], n: u32, network: Network) -> Address {
    Address::p2wpkh(&compressed(&key(seed, n)), network)
}

/// What to set aside for the fee of the payment into the contract: twice today's fee, so
/// a busier day between publishing and the sale is covered. What is not used comes back.
pub fn fee_reserve(rate: FeeRate) -> u64 {
    rate.fee_vb(2 * PAY_VSIZE)
        .map(|a| a.to_sat())
        .unwrap_or(u64::MAX)
        .max(MIN_FEE_RESERVE)
}

/// Spends `coins` (all of them outputs of the deposit address of `key`).
///
/// With `pay`, exactly that amount goes to that script (the swap contract) and what is
/// left after the fee goes to `rest`; without it, everything goes to `rest` (the bitcoin
/// returns to its owner). The fee follows `rate`; if the coins do not cover it, a lower
/// fee is paid, down to the lowest the network relays.
pub fn spend(
    coins: &[Utxo],
    key: &SecretKey,
    pay: Option<(&Script, Amount)>,
    rest: &Script,
    rate: FeeRate,
) -> Result<Transaction> {
    spend_many(coins, key, pay.as_slice(), rest, rate)
}

/// [`spend`] with up to [`MAX_PAYMENTS`] exact payments, in this order, before the rest.
pub fn spend_many(
    coins: &[Utxo],
    key: &SecretKey,
    pay: &[(&Script, Amount)],
    rest: &Script,
    rate: FeeRate,
) -> Result<Transaction> {
    ensure!(
        !coins.is_empty(),
        "there is no bitcoin at the deposit address"
    );
    ensure!(
        coins.len() <= MAX_COINS,
        "too many separate payments at the deposit address"
    );
    ensure!(pay.len() <= MAX_PAYMENTS, "too many payments at once");
    ensure!(
        pay.iter().all(|(s, a)| *a >= s.minimal_non_dust()),
        "a payment that small cannot be sent"
    );
    let total = coins
        .iter()
        .try_fold(Amount::ZERO, |a, c| a.checked_add(c.value))
        .context("amount overflow")?;
    let paid = pay
        .iter()
        .try_fold(Amount::ZERO, |a, p| a.checked_add(p.1))
        .context("amount overflow")?;
    ensure!(
        total > paid,
        "the deposit holds {total}, the payment is {paid}: nothing is left for the network fee"
    );
    // Sign once to learn the size, then again with the fee for that size.
    let draft = signed(coins, key, pay, rest, Amount::ZERO)?;
    let vsize = draft.vsize() as u64 + 1 + coins.len() as u64 / 4;
    let ceiling = Amount::from_sat((total.to_sat() / 10).max(FEE_CEILING_SAT));
    let want = rate.fee_vb(vsize).context("fee overflow")?.min(ceiling);
    let floor = Amount::from_sat(MIN_RELAY_SAT_VB * vsize);
    let left = total - paid;
    let fee = if !pay.is_empty() {
        // Never less than the payment itself: the fee gives way first.
        want.min(left)
    } else {
        want.min(
            left.checked_sub(rest.minimal_non_dust())
                .unwrap_or(Amount::ZERO),
        )
    };
    if fee < floor {
        bail!(
            "the deposit holds {total}: not enough for {} and a network fee of at least {floor}",
            if !pay.is_empty() {
                format!("the payment of {paid}")
            } else {
                "an output of its own".to_string()
            }
        );
    }
    signed(coins, key, pay, rest, fee)
}

fn signed(
    coins: &[Utxo],
    key: &SecretKey,
    pay: &[(&Script, Amount)],
    rest: &Script,
    fee: Amount,
) -> Result<Transaction> {
    let pk = compressed(key);
    let spk = ScriptBuf::new_p2wpkh(&pk.wpubkey_hash());
    let total = coins
        .iter()
        .try_fold(Amount::ZERO, |a, c| a.checked_add(c.value))
        .context("amount overflow")?;
    let mut output = Vec::with_capacity(pay.len() + 1);
    let mut left = total;
    for (script, amount) in pay {
        left = left
            .checked_sub(*amount)
            .context("the deposit is smaller than the payment")?;
        output.push(TxOut {
            value: *amount,
            script_pubkey: (*script).to_owned(),
        });
    }
    let left = left
        .checked_sub(fee)
        .context("the fee is more than the deposit holds")?;
    // What is too small to be an output of its own goes to the fee.
    if left >= rest.minimal_non_dust() {
        output.push(TxOut {
            value: left,
            script_pubkey: rest.to_owned(),
        });
    }
    ensure!(
        !output.is_empty(),
        "nothing would be left after the network fee"
    );
    let mut tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: coins
            .iter()
            .map(|c| TxIn {
                previous_output: c.outpoint,
                script_sig: ScriptBuf::new(),
                sequence: Sequence::ENABLE_RBF_NO_LOCKTIME,
                witness: Witness::new(),
            })
            .collect(),
        output,
    };
    let secp = Secp256k1::signing_only();
    let mut witnesses = Vec::with_capacity(coins.len());
    {
        let mut cache = SighashCache::new(&tx);
        for (i, c) in coins.iter().enumerate() {
            let h = cache
                .p2wpkh_signature_hash(i, &spk, c.value, EcdsaSighashType::All)
                .map_err(|e| anyhow::anyhow!("signing failed: {e}"))?;
            let sig = bitcoin::ecdsa::Signature {
                signature: secp.sign_ecdsa(&Message::from_digest(h.to_byte_array()), key),
                sighash_type: EcdsaSighashType::All,
            };
            witnesses.push(Witness::from_slice(&[sig.to_vec(), pk.to_bytes().to_vec()]));
        }
    }
    for (i, w) in witnesses.into_iter().enumerate() {
        tx.input[i].witness = w;
    }
    Ok(tx)
}

#[cfg(test)]
mod tests {
    use super::*;
    use bitcoin::{OutPoint, Txid, WPubkeyHash};

    const SEED: &[u8] = b"test seed of a buyer's wallet, 64 bytes in a real wallet........";

    fn coin(n: u8, sats: u64) -> Utxo {
        Utxo {
            outpoint: OutPoint::new(Txid::from_byte_array([n; 32]), n as u32),
            value: Amount::from_sat(sats),
            height: Some(100),
        }
    }

    fn rate(r: u64) -> FeeRate {
        FeeRate::from_sat_per_vb(r).unwrap()
    }

    fn dest(n: u8) -> ScriptBuf {
        ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([n; 20]))
    }

    /// Bitcoin Core's script interpreter on every input.
    fn valid(tx: &Transaction, coins: &[Utxo], n: u32) -> bool {
        let spk = address(SEED, n, Network::Bitcoin).script_pubkey();
        tx.verify(|op| {
            coins.iter().find(|c| c.outpoint == *op).map(|c| TxOut {
                value: c.value,
                script_pubkey: spk.clone(),
            })
        })
        .is_ok()
    }

    #[test]
    fn deposits_have_their_own_keys() {
        assert_ne!(key(SEED, 0), key(SEED, 1));
        assert_ne!(key(SEED, 0), crate::derive_key(SEED, 0));
        assert_eq!(
            address(SEED, 3, Network::Bitcoin),
            address(SEED, 3, Network::Bitcoin)
        );
        assert!(
            address(SEED, 3, Network::Bitcoin)
                .to_string()
                .starts_with("bc1q")
        );
    }

    #[test]
    fn the_payment_is_exact_and_the_rest_comes_back() {
        let k = key(SEED, 5);
        let coins = [coin(1, 150_000), coin(2, 60_000)];
        let (contract, back) = (dest(9), dest(8));
        let tx = spend(
            &coins,
            &k,
            Some((&contract, Amount::from_sat(200_000))),
            &back,
            rate(5),
        )
        .unwrap();
        assert!(valid(&tx, &coins, 5));
        assert_eq!(tx.input.len(), 2);
        assert_eq!(tx.output[0].script_pubkey, contract);
        assert_eq!(tx.output[0].value, Amount::from_sat(200_000));
        assert_eq!(tx.output[1].script_pubkey, back);
        let fee = 210_000 - 200_000 - tx.output[1].value.to_sat();
        let vb = tx.vsize() as u64;
        assert!(
            fee >= 5 * vb && fee <= 5 * (vb + 3),
            "fee {fee} for {vb} vB"
        );
        // Signed by another key, or for other coins, it is not valid.
        let other = spend(
            &coins,
            &key(SEED, 6),
            Some((&contract, Amount::from_sat(200_000))),
            &back,
            rate(5),
        )
        .unwrap();
        assert!(!valid(&other, &coins, 5));
        let mut moved = tx.clone();
        moved.output[0].value = Amount::from_sat(100_000);
        assert!(!valid(&moved, &coins, 5));
    }

    #[test]
    fn a_fee_higher_than_expected_gives_way_but_the_payment_never_does() {
        let k = key(SEED, 1);
        let (contract, back) = (dest(9), dest(8));
        let pay = Amount::from_sat(100_000);
        // 1,000 sat set aside; the network now asks 50 sat/vB (about 7,000 sat).
        let coins = [coin(1, 101_000)];
        let tx = spend(&coins, &k, Some((&contract, pay)), &back, rate(50)).unwrap();
        assert!(valid(&tx, &coins, 1));
        assert_eq!(tx.output.len(), 1, "everything left is the fee");
        assert_eq!(tx.output[0].value, pay);
        // Not even the lowest fee: no payment is made.
        let coins = [coin(1, 100_050)];
        assert!(spend(&coins, &k, Some((&contract, pay)), &back, rate(5)).is_err());
        let coins = [coin(1, 100_000)];
        assert!(spend(&coins, &k, Some((&contract, pay)), &back, rate(5)).is_err());
        let coins = [coin(1, 99_000)];
        assert!(spend(&coins, &k, Some((&contract, pay)), &back, rate(5)).is_err());
        // Change too small to be an output goes to the fee.
        let coins = [coin(1, 100_900)];
        let tx = spend(&coins, &k, Some((&contract, pay)), &back, rate(5)).unwrap();
        assert_eq!(tx.output.len(), 1);
    }

    #[test]
    fn unsold_bitcoin_goes_back_whole() {
        let k = key(SEED, 2);
        let back = dest(8);
        let coins = [coin(1, 30_000), coin(2, 20_000), coin(3, 700)];
        let tx = spend(&coins, &k, None, &back, rate(4)).unwrap();
        assert!(valid(&tx, &coins, 2));
        assert_eq!(tx.output.len(), 1);
        assert_eq!(tx.output[0].script_pubkey, back);
        let fee = 50_700 - tx.output[0].value.to_sat();
        let vb = tx.vsize() as u64;
        assert!(
            fee >= 4 * vb && fee <= 4 * (vb + 3),
            "fee {fee} for {vb} vB"
        );
        // Dust cannot be sent back, and nothing is not sent at all.
        assert!(spend(&[coin(1, 300)], &k, None, &back, rate(1)).is_err());
        assert!(spend(&[], &k, None, &back, rate(1)).is_err());
    }

    #[test]
    fn a_wrong_fee_rate_cannot_burn_a_deposit() {
        let k = key(SEED, 2);
        let back = dest(8);
        // A server reports 2,000 sat/vB. The bitcoin still goes back, less a tenth at most.
        let coins = [coin(1, 200_000)];
        let tx = spend(&coins, &k, None, &back, rate(2_000)).unwrap();
        assert!(valid(&tx, &coins, 2));
        assert_eq!(tx.output[0].value, Amount::from_sat(180_000));
        // A payment: the same ceiling, and exactly the agreed amount.
        let coins = [coin(1, 1_000_000)];
        let tx = spend(
            &coins,
            &k,
            Some((&dest(9), Amount::from_sat(100_000))),
            &back,
            rate(2_000),
        )
        .unwrap();
        assert_eq!(tx.output[0].value, Amount::from_sat(100_000));
        assert_eq!(tx.output[1].value, Amount::from_sat(800_000));
        // Small deposits: at most 3,000 sat.
        let coins = [coin(1, 20_000)];
        let tx = spend(&coins, &k, None, &back, rate(2_000)).unwrap();
        assert_eq!(tx.output[0].value, Amount::from_sat(17_000));
    }

    #[test]
    fn several_exact_payments_and_the_rest_in_one_transaction() {
        let k = key(SEED, BALANCE);
        let coins = [coin(1, 500_000)];
        let (a, b, back) = (dest(1), dest(2), dest(8));
        let tx = spend_many(
            &coins,
            &k,
            &[
                (&a, Amount::from_sat(120_000)),
                (&b, Amount::from_sat(80_000)),
            ],
            &back,
            rate(3),
        )
        .unwrap();
        assert!(valid(&tx, &coins, BALANCE));
        assert_eq!(tx.output[0].script_pubkey, a);
        assert_eq!(tx.output[0].value, Amount::from_sat(120_000));
        assert_eq!(tx.output[1].script_pubkey, b);
        assert_eq!(tx.output[1].value, Amount::from_sat(80_000));
        assert_eq!(tx.output[2].script_pubkey, back);
        let fee = 500_000 - 200_000 - tx.output[2].value.to_sat();
        assert!(fee >= 3 * tx.vsize() as u64 && fee < 3_000);
        // Never more than there is, never dust, never too many at once.
        assert!(
            spend_many(
                &coins,
                &k,
                &[(&a, Amount::from_sat(500_000))],
                &back,
                rate(1)
            )
            .is_err()
        );
        assert!(spend_many(&coins, &k, &[(&a, Amount::from_sat(100))], &back, rate(1)).is_err());
        let many: Vec<(&Script, Amount)> = (0..=MAX_PAYMENTS)
            .map(|_| (a.as_script(), Amount::from_sat(10_000)))
            .collect();
        assert!(spend_many(&coins, &k, &many, &back, rate(1)).is_err());
        // The balance has its own key, apart from every offer's deposit.
        assert!((0..1000).all(|n| key(SEED, n) != k));
    }

    #[test]
    fn the_fee_set_aside_covers_a_busier_day() {
        assert_eq!(fee_reserve(rate(1)), MIN_FEE_RESERVE);
        assert_eq!(fee_reserve(rate(10)), 10 * 2 * PAY_VSIZE);
        // The real payment, with two inputs and change, at twice the rate.
        let coins = [coin(1, 60_000), coin(2, 40_000 + fee_reserve(rate(10)))];
        let tx = spend(
            &coins,
            &key(SEED, 1),
            Some((&dest(9), Amount::from_sat(100_000))),
            &dest(8),
            rate(20),
        )
        .unwrap();
        assert!(tx.vsize() as u64 <= PAY_VSIZE);
        assert_eq!(tx.output[0].value, Amount::from_sat(100_000));
    }
}

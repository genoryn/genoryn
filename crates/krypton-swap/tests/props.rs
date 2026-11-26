//! Properties of the swap's safety rules over arbitrary inputs: the timelock checks (whose
//! numbers come from the other side), the bitcoin contract under Bitcoin Core's script
//! interpreter, the confirmation tiers and the swap-code decoder.

use bitcoin::hashes::Hash;
use bitcoin::{
    Amount, FeeRate, OutPoint, ScriptBuf, Transaction, TxOut, Txid, WPubkeyHash, absolute::LockTime,
};
use krypton_swap::engine::Msg;
use krypton_swap::{Funding, Htlc, Secret, SwapError, Timelocks, Timing, derive_key, public_key};
use proptest::prelude::*;

fn timings() -> impl Strategy<Value = Timing> {
    prop_oneof![Just(Timing::MAINNET), Just(Timing::TEST)]
}

/// Heights as the other side may send them: anything, with the edges well covered.
fn height32() -> impl Strategy<Value = u32> {
    prop_oneof![
        any::<u32>(),
        0u32..2_000_000,
        Just(u32::MAX),
        Just(u32::MAX - 1)
    ]
}

fn height64() -> impl Strategy<Value = u64> {
    prop_oneof![
        any::<u64>(),
        0u64..2_000_000,
        Just(u64::MAX),
        Just(u64::MAX / 45 + 1)
    ]
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(4000))]

    /// When the GNR seller's check passes, the rule it states holds in exact arithmetic.
    #[test]
    fn the_lock_check_holds_exactly(m in timings(), br in height32(), gr in height64(),
                                    bt in 0u32..2_000_000, gt in 0u64..2_000_000_000) {
        let t = Timelocks { btc_refund_height: br, gnr_refund_height: gr };
        if m.check(&t, bt, gt).is_ok() {
            prop_assert!(u128::from(gr) > u128::from(gt) + u128::from(m.gnr_confirmations) + u128::from(m.claim_margin));
            prop_assert!(br > bt);
            let btc = u128::from(br - bt) * u128::from(m.btc_fast_secs);
            let gnr = u128::from(gr - gt) * u128::from(m.gnr_slow_secs);
            prop_assert!(btc >= gnr + u128::from(m.min_window_secs));
        }
    }

    /// When the buyer's check passes, the GNR lock outlasts the bitcoin lock, the payment
    /// window and the claim window, and nothing lasts longer than a week, exactly.
    #[test]
    fn the_pay_check_holds_exactly(m in timings(), br in height32(), gr in height64(),
                                   bt in 0u32..2_000_000, gt in 0u64..2_000_000_000) {
        let t = Timelocks { btc_refund_height: br, gnr_refund_height: gr };
        if m.check_pay(&t, bt, gt).is_ok() {
            prop_assert!(u64::from(br) > u64::from(bt) + u64::from(m.btc_confirmations) + u64::from(m.btc_claim_margin));
            prop_assert!(u128::from(gr) > u128::from(gt) + u128::from(m.claim_margin));
            let gnr = (u128::from(gr) - u128::from(gt) - u128::from(m.claim_margin)) * u128::from(m.gnr_fast_secs);
            let btc = u128::from(br - bt) * u128::from(m.btc_slow_secs);
            prop_assert!(gnr >= btc + u128::from(m.pay_window_secs) + u128::from(m.min_window2_secs));
            prop_assert!((u128::from(gr) - u128::from(gt)) * 60 <= u128::from(krypton_swap::MAX_SWAP_SECS));
            prop_assert!(u128::from(br - bt) * 600 <= u128::from(krypton_swap::MAX_SWAP_SECS));
        }
    }

    /// Our own plan passes our own check at any realistic tip.
    #[test]
    fn a_plan_passes_its_own_check(m in timings(), bt in 0u32..50_000_000, gt in 0u64..5_000_000_000) {
        prop_assert_eq!(m.check(&m.plan(bt, gt), bt, gt), Ok(()));
    }

    /// A claim is allowed only before the refund side could act, and once that moment has
    /// passed it never becomes allowed again.
    #[test]
    fn claims_stop_before_the_refund(m in timings(), br in height32(), gr in height64(),
                                     bt in height32(), gt in height64()) {
        let t = Timelocks { btc_refund_height: br, gnr_refund_height: gr };
        if m.may_claim_btc(&t, bt) {
            prop_assert!(u64::from(bt) + u64::from(m.btc_claim_margin) < u64::from(br));
        } else if let Some(later) = bt.checked_add(1) {
            prop_assert!(!m.may_claim_btc(&t, later));
        }
        if m.may_claim_gnr(&t, gt) {
            prop_assert!(u128::from(gt) + u128::from(m.claim_margin) < u128::from(gr));
        } else if let Some(later) = gt.checked_add(1) {
            prop_assert!(!m.may_claim_gnr(&t, later));
        }
    }

    /// More money never asks for fewer confirmations, and the tiers stay within bounds.
    #[test]
    fn confirmations_grow_with_the_amount(m in timings(), a in any::<u64>(), b in any::<u64>()) {
        let (lo, hi) = if a <= b { (a, b) } else { (b, a) };
        prop_assert!(m.btc_confs_for(lo) <= m.btc_confs_for(hi));
        prop_assert!(m.gnr_confs_for(lo) <= m.gnr_confs_for(hi));
        prop_assert!(m.btc_confs_for(hi) >= 1 && m.btc_confs_for(hi) <= 4);
        prop_assert!(m.gnr_confs_for(hi) >= 1 && m.gnr_confs_for(hi) <= m.gnr_confirmations);
    }

    /// Any text pasted as a swap code is refused or decoded, never a crash.
    #[test]
    fn swap_codes_never_crash(s in ".{0,600}") {
        let _ = Msg::decode(&s);
        let _ = Msg::decode(&format!("gks1{s}"));
    }
}

struct Case {
    htlc: Htlc,
    secret: Secret,
    claim_key: bitcoin::secp256k1::SecretKey,
    refund_key: bitcoin::secp256k1::SecretKey,
    funding: Funding,
    out: TxOut,
}

fn case(seed: [u8; 32], lock: u32, sats: u64) -> Case {
    let secret = Secret::from_bytes(seed);
    let claim_key = derive_key(&seed, 1);
    let refund_key = derive_key(&seed, 2);
    let htlc = Htlc::new(
        secret.hash(),
        public_key(&claim_key),
        public_key(&refund_key),
        lock,
    )
    .unwrap();
    let value = Amount::from_sat(sats);
    let funding = Funding {
        outpoint: OutPoint::new(Txid::from_byte_array(seed), 0),
        value,
    };
    let out = TxOut {
        value,
        script_pubkey: htlc.script_pubkey(),
    };
    Case {
        htlc,
        secret,
        claim_key,
        refund_key,
        funding,
        out,
    }
}

fn valid(c: &Case, tx: &Transaction) -> bool {
    tx.verify(|op| (*op == c.funding.outpoint).then(|| c.out.clone()))
        .is_ok()
}

fn dest() -> ScriptBuf {
    ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([4; 20]))
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(96))]

    /// For any secret, keys, lock height, amount and fee rate: the claim is valid and
    /// reveals the secret, the refund is valid and carries the lock height, the claim pays
    /// the funding minus a fee of the asked rate, and only the right key and secret work.
    #[test]
    fn the_contract_behaves(seed in any::<[u8; 32]>(), lock in 1u32..499_999_999,
                            sats in 20_000u64..2_100_000_000_000_000, rate in 1u64..300) {
        let c = case(seed, lock, sats);
        let fr = FeeRate::from_sat_per_vb(rate).unwrap();
        let claim = c.htlc.claim(c.funding, &dest(), fr, &c.claim_key, &c.secret).unwrap();
        prop_assert!(valid(&c, &claim));
        prop_assert_eq!(c.htlc.find_secret(&claim), Some(c.secret.clone()));
        let fee = c.funding.value - claim.output[0].value;
        prop_assert!(fee >= fr.fee_vb(claim.vsize() as u64).unwrap());
        prop_assert!(fee <= fr.fee_vb(claim.vsize() as u64 + 2).unwrap());
        let refund = c.htlc.refund(c.funding, &dest(), fr, &c.refund_key).unwrap();
        prop_assert!(valid(&c, &refund));
        prop_assert_eq!(refund.lock_time, LockTime::from_height(lock).unwrap());
        prop_assert_eq!(c.htlc.find_secret(&refund), None);
        prop_assert_eq!(c.htlc.claim(c.funding, &dest(), fr, &c.refund_key, &c.secret).unwrap_err(), SwapError::WrongKey);
        let wrong = Secret::from_bytes(seed.map(|b| b ^ 1));
        prop_assert_eq!(c.htlc.claim(c.funding, &dest(), fr, &c.claim_key, &wrong).unwrap_err(), SwapError::WrongSecret);
    }

    /// A contract too small to pay its own fee is refused instead of signed.
    #[test]
    fn dust_is_never_signed(seed in any::<[u8; 32]>(), sats in 1u64..1_000, rate in 1u64..50) {
        let c = case(seed, 900_000, sats);
        let fr = FeeRate::from_sat_per_vb(rate).unwrap();
        match c.htlc.claim(c.funding, &dest(), fr, &c.claim_key, &c.secret) {
            Ok(tx) => prop_assert!(tx.output[0].value.to_sat() >= 294 && tx.output[0].value <= c.funding.value),
            Err(e) => prop_assert!(matches!(e, SwapError::Dust(_)), "{e:?}"),
        }
    }
}

/// A GNR lock chosen so that its length times the seconds per block wraps around to a
/// small number is still seen as the near-endless lock it is.
#[test]
fn a_lock_length_that_would_wrap_is_refused() {
    for m in [Timing::MAINNET, Timing::TEST] {
        let (bt, gt) = (900_000u32, 31_000u64);
        for secs in [60u64, m.gnr_fast_secs, m.gnr_slow_secs] {
            let len = u64::MAX / secs + 1;
            let t = Timelocks {
                btc_refund_height: bt + 18,
                gnr_refund_height: gt + len,
            };
            assert!(m.check_pay(&t, bt, gt).is_err(), "{secs}");
            assert!(m.check(&t, bt, gt).is_err(), "{secs}");
        }
    }
}

//! What reaches the offer board and the market price comes from anyone: board messages
//! and records must be refused or accepted without a crash, sealed messages must open only
//! unchanged and for their key, and the market price must stay within the swaps behind it,
//! ignore their order and move by at most its hourly limit, whatever one more swap claims.

use krypton_crypto::kem;
use krypton_swap::board::{self, Envelope, Offer, Paid, Promise, Trade};
use krypton_swap::price::{self, Swap};
use proptest::prelude::*;
use proptest::strategy::{BoxedStrategy, ValueTree};
use std::sync::atomic::{AtomicUsize, Ordering};

static DECODED: [AtomicUsize; 4] = [const { AtomicUsize::new(0) }; 4];
use serde_json::{Map, Value, json};

const NOW: u64 = 1_800_000_000;

fn leaf() -> impl Strategy<Value = Value> {
    prop_oneof![
        Just(Value::Null),
        any::<bool>().prop_map(Value::Bool),
        any::<u64>().prop_map(|n| json!(n)),
        any::<i64>().prop_map(|n| json!(n)),
        any::<f64>().prop_map(|f| json!(f)),
        "\\PC{0,80}".prop_map(Value::String),
        "[0-9a-f]{0,200}".prop_map(Value::String),
        Just(json!("mainnet")),
        Just(json!("bitcoin")),
        Just(json!("buy")),
        Just(json!("sell")),
    ]
}

fn value() -> impl Strategy<Value = Value> {
    leaf().prop_recursive(3, 24, 6, |inner| {
        prop_oneof![
            prop::collection::vec(inner.clone(), 0..6).prop_map(Value::Array),
            prop::collection::btree_map("[a-z_]{1,12}", inner, 0..6)
                .prop_map(|m| Value::Object(m.into_iter().collect())),
        ]
    })
}

/// A value of the field's own type: what a careless or hostile sender would still get past
/// the decoder, so the checks behind it are reached.
fn typed(key: &'static str) -> BoxedStrategy<Value> {
    match key {
        "network" => prop::sample::select(&["mainnet", "devnet", "testnet", ""][..])
            .prop_map(|s| json!(s))
            .boxed(),
        "btc_network" => prop::sample::select(&["bitcoin", "signet", "regtest", "x"][..])
            .prop_map(|s| json!(s))
            .boxed(),
        "side" => prop::sample::select(&["sell_gnr", "buy_gnr", "SellGnr", "buy"][..])
            .prop_map(|s| json!(s))
            .boxed(),
        "gnr" | "sats" | "nonce" | "expires" => prop_oneof![any::<u64>(), 0u64..10_000_000_000]
            .prop_map(|n| json!(n))
            .boxed(),
        "refund_height" | "funded_height" => any::<u32>().prop_map(|n| json!(n)).boxed(),
        _ => prop_oneof![
            "[0-9a-f]{0,140}",
            "[0-9a-f]{64}",
            "[0-9a-f]{66}",
            "\\PC{0,40}"
        ]
        .prop_map(Value::String)
        .boxed(),
    }
}

/// An object with every field a record has, each of its type or (now and then) anything,
/// plus some fields it does not have.
fn record(keys: &'static [&'static str]) -> impl Strategy<Value = Value> {
    let fields: Vec<_> = keys
        .iter()
        .map(|k| {
            prop_oneof![8 => typed(k).prop_map(Some), 1 => value().prop_map(Some), 1 => Just(None)]
                .prop_map(move |v| (*k, v))
        })
        .collect();
    (
        fields,
        prop::collection::vec(("[a-z_]{1,10}", value()), 0..3),
    )
        .prop_map(|(known, extra)| {
            let mut m = Map::new();
            for (k, v) in extra {
                m.insert(k, v);
            }
            for (k, v) in known {
                if let Some(v) = v {
                    m.insert(k.to_string(), v);
                }
            }
            Value::Object(m)
        })
}

const OFFER: &[&str] = &[
    "id",
    "network",
    "btc_network",
    "side",
    "gnr",
    "sats",
    "key",
    "expires",
    "cancel",
    "nonce",
    "coin",
    "reserve",
    "auto",
    "min_gnr",
    "parts",
];
const TRADE: &[&str] = &["network", "hash", "gnr", "sats", "nonce"];
const PAID: &[&str] = &[
    "btc_network",
    "claim",
    "refund",
    "refund_height",
    "funding",
    "funded_height",
    "key",
    "sig",
];
const PROMISE: &[&str] = &[
    "id",
    "gnr",
    "sats",
    "refund_pub",
    "btc_network",
    "key",
    "sig",
    "release",
];
const ENVELOPE: &[&str] = &["to", "data", "nonce"];

proptest! {
    #![proptest_config(ProptestConfig::with_cases(3000))]

    #[test]
    fn offers_never_crash(v in record(OFFER), net in prop::sample::select(&["mainnet", "devnet"][..])) {
        if let Ok(o) = serde_json::from_value::<Offer>(v) {
            DECODED[0].fetch_add(1, Ordering::Relaxed);
            let _ = o.validate(net, NOW);
            let _ = o.price();
        }
    }

    #[test]
    fn trades_and_payments_never_crash(t in record(TRADE), p in record(PAID)) {
        let t = serde_json::from_value::<Trade>(t);
        let p = serde_json::from_value::<Paid>(p);
        if let Ok(t) = &t {
            DECODED[1].fetch_add(1, Ordering::Relaxed);
            let _ = t.validate("mainnet");
        }
        if let (Ok(t), Ok(p)) = (&t, &p) {
            DECODED[2].fetch_add(1, Ordering::Relaxed);
            let _ = board::check_paid(t, p);
        }
    }

    #[test]
    fn promises_and_envelopes_never_crash(p in record(PROMISE), e in record(ENVELOPE)) {
        if let Ok(p) = serde_json::from_value::<Promise>(p) {
            let _ = krypton_swap::engine::promise_address(&p);
        }
        if let Ok(e) = serde_json::from_value::<Envelope>(e) {
            DECODED[3].fetch_add(1, Ordering::Relaxed);
            let _ = e.validate();
        }
    }

    #[test]
    fn swap_codes_with_a_promise_never_crash(s in "\\PC{0,400}", r in "[0-9a-f]{0,80}") {
        let _ = krypton_swap::engine::promise_of(&s, r.clone());
        let _ = krypton_swap::engine::prepaid_address(&s, bitcoin::Network::Bitcoin);
        let _ = krypton_swap::engine::promise_of(&format!("gks1{s}"), r);
    }

    #[test]
    fn any_bytes_sent_as_a_message_are_refused(b in prop::collection::vec(any::<u8>(), 0..3000)) {
        let me = kem::SecretKey::from_seed(&[7; 96]);
        prop_assert!(board::open(&me, &hex::encode(&b)).is_err());
        let _ = board::open(&me, &String::from_utf8_lossy(&b));
    }
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(48))]

    /// A sealed message opens for its key with the sender and body unchanged; any changed
    /// byte, or another key, and it does not open.
    #[test]
    fn sealed_messages_open_only_intact(body in prop::collection::vec(any::<u8>(), 0..2000),
                                        flip in any::<prop::sample::Index>(), bit in 0u8..8) {
        let a = kem::SecretKey::from_seed(&[1; 96]);
        let b = kem::SecretKey::from_seed(&[2; 96]);
        let e = board::seal(b.public(), &a, &body).unwrap();
        let (from, got) = board::open(&b, &e.data).unwrap();
        prop_assert_eq!(from.to_bytes(), a.public().to_bytes());
        prop_assert_eq!(&got, &body);
        prop_assert!(board::open(&a, &e.data).is_err());
        let mut raw = hex::decode(&e.data).unwrap();
        let i = flip.index(raw.len());
        raw[i] ^= 1 << bit;
        prop_assert!(board::open(&b, &hex::encode(raw)).is_err());
    }
}

fn swaps(max: usize) -> impl Strategy<Value = Vec<Swap>> {
    prop::collection::vec(
        (
            0u64..40 * 86_400,
            prop_oneof![1u64..2_000_000_000_000, Just(0u64)],
            prop_oneof![1u64..1_000_000_000, 100_000u64..5_000_000],
            prop_oneof![Just(0u64), Just(u64::MAX), 0u64..1_000_000_000],
        )
            .prop_map(|(ago, gnr, sats, paid)| Swap {
                at: NOW - ago,
                gnr,
                sats,
                paid: paid.min(sats.saturating_mul(2)),
            }),
        0..max,
    )
}

/// Swaps near one price, as a working market has them (prices in sat per GNR).
fn market(max: usize) -> impl Strategy<Value = Vec<Swap>> {
    (
        100u64..5_000,
        prop::collection::vec(
            (0u64..10 * 86_400, 80u64..120, 100_000u64..3_000_000),
            0..max,
        ),
    )
        .prop_map(|(p, v)| {
            v.into_iter()
                .map(|(ago, pct, sats)| {
                    let gnr = (u128::from(sats) * 100_000_000 * 100
                        / (u128::from(p) * u128::from(pct))) as u64;
                    Swap {
                        at: NOW - ago,
                        gnr: gnr.max(1),
                        sats,
                        paid: sats,
                    }
                })
                .collect()
        })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(400))]

    #[test]
    fn the_price_never_crashes_and_stays_in_bounds(v in swaps(120)) {
        let a = price::assess(&v, NOW);
        prop_assert!(a.confidence <= 100);
        if let Some(p) = a.price {
            let ps: Vec<u128> = v.iter().filter(|s| s.gnr > 0 && s.paid > 0).map(|s| s.price()).collect();
            prop_assert!(!ps.is_empty());
            prop_assert!(p >= *ps.iter().min().unwrap() && p <= *ps.iter().max().unwrap(), "{p} outside the swaps");
        } else {
            prop_assert_eq!(a.confidence, 0);
        }
    }

    #[test]
    fn the_order_of_the_swaps_does_not_matter(v in market(80), seed in any::<u64>()) {
        let mut w = v.clone();
        let n = w.len();
        let mut x = seed | 1;
        for i in (1..n).rev() {
            x ^= x << 13; x ^= x >> 7; x ^= x << 17;
            w.swap(i, (x % (i as u64 + 1)) as usize);
        }
        prop_assert_eq!(price::assess(&v, NOW), price::assess(&w, NOW));
    }

    /// With one more swap, at any price and size, the price still moves at most the hourly
    /// limit (10%) from an hour ago, and swaps recorded after now change nothing.
    #[test]
    fn one_swap_moves_the_price_at_most_the_hourly_limit(v in market(80), gnr in 1u64..10_000_000_000_000,
                                                         sats in 100_000u64..10_000_000_000, later in 1u64..86_400) {
        let a = price::assess(&v, NOW);
        let before = price::published(&v, NOW - 3600);
        let mut w = v.clone();
        w.push(Swap { at: NOW, gnr, sats, paid: sats });
        let b = price::assess(&w, NOW);
        if let (Some(p), Some(q)) = (before, b.price) {
            let limit = p * price::MOVE_PER_HOUR_PERMILLE / 1000 + 1;
            prop_assert!(q.abs_diff(p) <= limit, "{p} an hour ago -> {q}");
        }
        let mut f = v.clone();
        f.push(Swap { at: NOW + later, gnr, sats, paid: sats });
        prop_assert_eq!(price::assess(&f, NOW).price, a.price);
    }
}

/// The generators reach the checks: most records decode, so `validate` runs on them.
#[test]
fn the_records_reach_the_checks() {
    let mut runner = proptest::test_runner::TestRunner::deterministic();
    for _ in 0..400 {
        let o = record(OFFER).new_tree(&mut runner).unwrap().current();
        let t = record(TRADE).new_tree(&mut runner).unwrap().current();
        let p = record(PAID).new_tree(&mut runner).unwrap().current();
        let e = record(ENVELOPE).new_tree(&mut runner).unwrap().current();
        for (i, ok) in [
            serde_json::from_value::<Offer>(o).is_ok(),
            serde_json::from_value::<Trade>(t).is_ok(),
            serde_json::from_value::<Paid>(p).is_ok(),
            serde_json::from_value::<Envelope>(e).is_ok(),
        ]
        .into_iter()
        .enumerate()
        {
            DECODED[i].fetch_add(usize::from(ok), Ordering::Relaxed);
        }
    }
    let got: Vec<usize> = DECODED.iter().map(|d| d.load(Ordering::Relaxed)).collect();
    assert!(got.iter().all(|&n| n >= 20), "decoded per kind: {got:?}");
}

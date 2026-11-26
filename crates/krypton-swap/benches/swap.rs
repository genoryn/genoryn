//! Timings of the swap and market paths that run often: the market price over growing
//! histories, sealing and opening a board message, the proof of work of a board post, and
//! building the bitcoin claim. Prints one JSON object.

use bitcoin::hashes::Hash;
use bitcoin::{Amount, FeeRate, OutPoint, ScriptBuf, Txid, WPubkeyHash};
use krypton_crypto::kem;
use krypton_swap::price::{self, Swap};
use krypton_swap::{Funding, Htlc, Secret, board, derive_key, public_key};
use std::time::Instant;

fn per_call<T>(runs: u32, mut f: impl FnMut() -> T) -> f64 {
    let t = Instant::now();
    for _ in 0..runs {
        std::hint::black_box(f());
    }
    t.elapsed().as_secs_f64() * 1000.0 / f64::from(runs)
}

fn history(n: usize, now: u64) -> Vec<Swap> {
    let mut x: u64 = 0x9e37_79b9_7f4a_7c15;
    (0..n)
        .map(|i| {
            x ^= x << 13;
            x ^= x >> 7;
            x ^= x << 17;
            let sats = 100_000 + x % 5_000_000;
            let pct = 90 + x % 20;
            Swap {
                at: now - (i as u64 * 40 * 86_400 / n as u64),
                gnr: sats * 100_000_000 / (120 * pct / 100),
                sats,
                paid: if x % 10 < 7 { sats } else { 0 },
            }
        })
        .collect()
}

fn main() {
    let now = 1_800_000_000;
    let mut out = Vec::new();
    for n in [1_000usize, 10_000, 50_000] {
        let h = history(n, now);
        let runs = if n > 10_000 { 3 } else { 10 };
        out.push(format!(
            "\"assess_{n}_ms\":{:.1}",
            per_call(runs, || price::assess(&h, now))
        ));
    }
    let a = kem::SecretKey::from_seed(&[1; 96]);
    let b = kem::SecretKey::from_seed(&[2; 96]);
    let body = vec![7u8; 1024];
    out.push(format!(
        "\"seal_1kb_ms\":{:.2}",
        per_call(20, || board::seal(b.public(), &a, &body).unwrap())
    ));
    let e = board::seal(b.public(), &a, &body).unwrap();
    out.push(format!(
        "\"open_1kb_ms\":{:.3}",
        per_call(200, || board::open(&b, &e.data).unwrap())
    ));
    for bits in [16u32, 20] {
        out.push(format!(
            "\"post_pow_{bits}bits_ms\":{:.1}",
            per_call(10, || board::mine("Krypton/bench", &body, bits))
        ));
    }
    let seed = [5u8; 32];
    let secret = Secret::from_bytes(seed);
    let (ck, rk) = (derive_key(&seed, 1), derive_key(&seed, 2));
    let htlc = Htlc::new(secret.hash(), public_key(&ck), public_key(&rk), 900_000).unwrap();
    let funding = Funding {
        outpoint: OutPoint::new(Txid::from_byte_array(seed), 0),
        value: Amount::from_sat(1_000_000),
    };
    let dest = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([4; 20]));
    let fr = FeeRate::from_sat_per_vb(5).unwrap();
    out.push(format!(
        "\"btc_claim_ms\":{:.3}",
        per_call(200, || htlc
            .claim(funding, &dest, fr, &ck, &secret)
            .unwrap())
    ));
    println!("{{{}}}", out.join(","));
}

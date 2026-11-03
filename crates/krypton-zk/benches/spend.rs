use krypton_crypto::field::ZERO_DIGEST;
use krypton_crypto::rescue::{self, SpendKeys};
use krypton_zk::{InputWitness, OutputWitness, SpendWitness, prove, verify};
use std::time::Instant;

fn main() {
    let owner = SpendKeys::from_sk(rescue::scalar(1)).owner;
    let w = SpendWitness {
        inputs: [InputWitness::dummy(), InputWitness::dummy()],
        outputs: [
            OutputWitness {
                owner,
                value: 1,
                rcm: rescue::scalar(2),
            },
            OutputWitness {
                owner,
                value: 2,
                rcm: rescue::scalar(3),
            },
        ],
        root: ZERO_DIGEST,
        fee: 0,
        mint: 3,
        sighash: rescue::scalar(4),
    };
    let runs = 5;
    let mut sizes = 0;
    let t = Instant::now();
    let mut last = None;
    for _ in 0..runs {
        let (p, proof) = prove(&w).unwrap();
        sizes = proof.len();
        last = Some((p, proof));
    }
    let prove_ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    let (p, proof) = last.unwrap();
    let t = Instant::now();
    for _ in 0..runs {
        verify(&p, &proof).unwrap();
    }
    let verify_ms = t.elapsed().as_secs_f64() * 1000.0 / runs as f64;
    println!("{{\"proof_bytes\":{sizes},\"prove_ms\":{prove_ms:.1},\"verify_ms\":{verify_ms:.2}}}");
}

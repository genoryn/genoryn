use super::*;
use bitcoin::{Txid, WPubkeyHash};

const SEED_BTC_SELLER: &[u8] = b"test seed of the bitcoin seller, 64 bytes in a real wallet......";
const SEED_GNR_SELLER: &[u8] = b"test seed of the GNR seller, 64 bytes in a real wallet..........";

struct Case {
    htlc: Htlc,
    secret: Secret,
    claim_key: SecretKey,
    refund_key: SecretKey,
    funding: Funding,
    funding_out: TxOut,
    dest: ScriptBuf,
}

fn case() -> Case {
    let secret = derive_secret(SEED_BTC_SELLER, 7);
    let refund_key = derive_key(SEED_BTC_SELLER, 7);
    let claim_key = derive_key(SEED_GNR_SELLER, 3);
    let htlc = Htlc::new(
        secret.hash(),
        public_key(&claim_key),
        public_key(&refund_key),
        900_288,
    )
    .unwrap();
    let value = Amount::from_sat(250_000);
    let funding = Funding {
        outpoint: OutPoint::new(Txid::from_byte_array([9; 32]), 1),
        value,
    };
    let funding_out = TxOut {
        value,
        script_pubkey: htlc.script_pubkey(),
    };
    let dest = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([4; 20]));
    Case {
        htlc,
        secret,
        claim_key,
        refund_key,
        funding,
        funding_out,
        dest,
    }
}

fn rate() -> FeeRate {
    FeeRate::from_sat_per_vb(5).unwrap()
}

/// Bitcoin Core's script interpreter (libbitcoinconsensus) on input 0.
fn valid(c: &Case, tx: &Transaction) -> bool {
    tx.verify(|op| (*op == c.funding.outpoint).then(|| c.funding_out.clone()))
        .is_ok()
}

#[test]
fn claim_with_secret_is_valid() {
    let c = case();
    let tx = c
        .htlc
        .claim(c.funding, &c.dest, rate(), &c.claim_key, &c.secret)
        .unwrap();
    assert!(valid(&c, &tx));
    assert_eq!(tx.lock_time, LockTime::ZERO);
    let fee = c.funding.value - tx.output[0].value;
    let want = rate().fee_vb(tx.vsize() as u64).unwrap();
    assert!(
        fee >= want && fee <= want + Amount::from_sat(10),
        "fee {fee} for {} vB",
        tx.vsize()
    );
}

#[test]
fn claim_reveals_the_secret() {
    let c = case();
    let tx = c
        .htlc
        .claim(c.funding, &c.dest, rate(), &c.claim_key, &c.secret)
        .unwrap();
    assert_eq!(c.htlc.find_secret(&tx), Some(c.secret.clone()));
    let refund = c
        .htlc
        .refund(c.funding, &c.dest, rate(), &c.refund_key)
        .unwrap();
    assert_eq!(c.htlc.find_secret(&refund), None);
}

#[test]
fn refund_after_timelock_is_valid() {
    let c = case();
    let tx = c
        .htlc
        .refund(c.funding, &c.dest, rate(), &c.refund_key)
        .unwrap();
    assert!(valid(&c, &tx));
    assert_eq!(tx.lock_time, LockTime::from_height(900_288).unwrap());
}

#[test]
fn refund_before_timelock_is_invalid() {
    let c = case();
    let early = LockTime::from_height(900_287).unwrap();
    let tx = c
        .htlc
        .signed(
            c.funding,
            &c.dest,
            Amount::from_sat(500),
            &c.refund_key,
            &Path::Refund,
            early,
            Sequence::ENABLE_RBF_NO_LOCKTIME,
        )
        .unwrap();
    assert!(!valid(&c, &tx));
    // A final sequence switches nLockTime off, so CLTV fails as well.
    let lt = LockTime::from_height(900_288).unwrap();
    let tx = c
        .htlc
        .signed(
            c.funding,
            &c.dest,
            Amount::from_sat(500),
            &c.refund_key,
            &Path::Refund,
            lt,
            Sequence::MAX,
        )
        .unwrap();
    assert!(!valid(&c, &tx));
}

#[test]
fn wrong_secret_or_key_is_refused_and_invalid() {
    let c = case();
    let other = Secret::from_bytes([1; 32]);
    assert_eq!(
        c.htlc
            .claim(c.funding, &c.dest, rate(), &c.claim_key, &other)
            .unwrap_err(),
        SwapError::WrongSecret
    );
    assert_eq!(
        c.htlc
            .claim(c.funding, &c.dest, rate(), &c.refund_key, &c.secret)
            .unwrap_err(),
        SwapError::WrongKey
    );
    assert_eq!(
        c.htlc
            .refund(c.funding, &c.dest, rate(), &c.claim_key)
            .unwrap_err(),
        SwapError::WrongKey
    );
    // Forced past the checks, the script still rejects them.
    let lt = LockTime::ZERO;
    let seq = Sequence::ENABLE_RBF_NO_LOCKTIME;
    let fee = Amount::from_sat(500);
    let tx = c
        .htlc
        .signed(
            c.funding,
            &c.dest,
            fee,
            &c.claim_key,
            &Path::Claim(&other),
            lt,
            seq,
        )
        .unwrap();
    assert!(!valid(&c, &tx));
    let tx = c
        .htlc
        .signed(
            c.funding,
            &c.dest,
            fee,
            &c.refund_key,
            &Path::Claim(&c.secret),
            lt,
            seq,
        )
        .unwrap();
    assert!(!valid(&c, &tx));
    // The claim key cannot take the refund path, even after the timelock.
    let tx = c
        .htlc
        .signed(
            c.funding,
            &c.dest,
            fee,
            &c.claim_key,
            &Path::Refund,
            LockTime::from_height(900_288).unwrap(),
            seq,
        )
        .unwrap();
    assert!(!valid(&c, &tx));
}

#[test]
fn signature_commits_to_outputs() {
    let c = case();
    let mut tx = c
        .htlc
        .claim(c.funding, &c.dest, rate(), &c.claim_key, &c.secret)
        .unwrap();
    tx.output[0].script_pubkey = ScriptBuf::new_p2wpkh(&WPubkeyHash::from_byte_array([5; 20]));
    assert!(!valid(&c, &tx));
}

#[test]
fn dust_is_refused() {
    let mut c = case();
    c.funding.value = Amount::from_sat(600);
    assert!(matches!(
        c.htlc
            .claim(c.funding, &c.dest, rate(), &c.claim_key, &c.secret),
        Err(SwapError::Dust(_))
    ));
}

#[test]
fn funding_is_found_and_addresses_match_network() {
    let c = case();
    let tx = Transaction {
        version: Version::TWO,
        lock_time: LockTime::ZERO,
        input: vec![],
        output: vec![
            TxOut {
                value: Amount::from_sat(1),
                script_pubkey: c.dest.clone(),
            },
            c.funding_out.clone(),
        ],
    };
    let f = c.htlc.find_funding(&tx);
    assert_eq!(f.len(), 1);
    assert_eq!(f[0].outpoint.vout, 1);
    assert_eq!(f[0].value, c.funding.value);
    assert!(
        c.htlc
            .address(Network::Bitcoin)
            .to_string()
            .starts_with("bc1q")
    );
    assert!(
        c.htlc
            .address(Network::Signet)
            .to_string()
            .starts_with("tb1q")
    );
    assert_eq!(
        c.htlc.address(Network::Bitcoin).script_pubkey(),
        c.htlc.script_pubkey()
    );
}

#[test]
fn derivation_is_deterministic_and_separated() {
    assert_eq!(
        derive_key(SEED_BTC_SELLER, 1),
        derive_key(SEED_BTC_SELLER, 1)
    );
    assert_ne!(
        derive_key(SEED_BTC_SELLER, 1),
        derive_key(SEED_BTC_SELLER, 2)
    );
    assert_ne!(
        derive_key(SEED_BTC_SELLER, 1),
        derive_key(SEED_GNR_SELLER, 1)
    );
    assert_eq!(
        derive_secret(SEED_BTC_SELLER, 1),
        derive_secret(SEED_BTC_SELLER, 1)
    );
    assert_ne!(
        derive_secret(SEED_BTC_SELLER, 1).as_bytes(),
        &derive_key(SEED_BTC_SELLER, 1).secret_bytes()
    );
    assert_ne!(Secret::generate(), Secret::generate());
    assert_eq!(
        format!("{:?}", derive_secret(SEED_BTC_SELLER, 1)),
        "Secret(..)"
    );
}

#[test]
fn bad_terms_are_refused() {
    let c = case();
    assert_eq!(
        Htlc::new(c.htlc.hash, c.htlc.claim, c.htlc.refund, 0).unwrap_err(),
        SwapError::BadLockTime
    );
    assert_eq!(
        Htlc::new(c.htlc.hash, c.htlc.claim, c.htlc.refund, 500_000_000).unwrap_err(),
        SwapError::BadLockTime
    );
    let mut uncompressed = c.htlc.claim;
    uncompressed.compressed = false;
    assert_eq!(
        Htlc::new(c.htlc.hash, uncompressed, c.htlc.refund, 10).unwrap_err(),
        SwapError::WrongKey
    );
}

#[test]
fn timelocks() {
    let m = Timing::MAINNET;
    let t = m.plan(900_000, 1_700);
    assert_eq!(
        t,
        Timelocks {
            btc_refund_height: 900_288,
            gnr_refund_height: 2_420
        }
    );
    assert_eq!(m.check(&t, 900_000, 1_700), Ok(()));
    // The GNR seller arrives late: GNR lock still fine, but much of the bitcoin lock is gone.
    assert!(m.check(&t, 900_200, 1_700).is_err());
    // The GNR lock nearly expired.
    assert!(m.check(&t, 900_000, 2_300).is_err());
    // Bitcoin lock shorter than the GNR lock.
    let bad = Timelocks {
        btc_refund_height: 900_072,
        gnr_refund_height: 2_420,
    };
    assert!(m.check(&bad, 900_000, 1_700).is_err());
    let gone = Timelocks {
        btc_refund_height: 899_000,
        gnr_refund_height: 2_420,
    };
    assert!(m.check(&gone, 900_000, 1_700).is_err());
    assert!(m.may_claim_gnr(&t, 1_700));
    assert!(m.may_claim_gnr(&t, 2_299));
    assert!(!m.may_claim_gnr(&t, 2_300));
    let s = Timing::TEST;
    assert_eq!(s.check(&s.plan(100, 50), 100, 50), Ok(()));
    assert_eq!(Timing::for_network(Network::Bitcoin), m);
    assert_eq!(Timing::for_network(Network::Regtest), s);
}

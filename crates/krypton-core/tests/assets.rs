//! Krypton coins end to end at the consensus level: real spend proofs, real issuer
//! signatures, and the registry rules (deploy, issue, transfer, burn, pause).

use krypton_assets::{Backing, Manifest, Supply};
use krypton_core::asset::*;
use krypton_core::consensus::*;
use krypton_core::params::{ChainParams, Network};
use krypton_core::reward::{Account, RewardView};
use krypton_core::tree::MemTree;
use krypton_core::types::*;
use krypton_core::{CoreError, Hash32};
use krypton_crypto::field::{Digest, Felt, digest_to_bytes};
use krypton_crypto::rescue::{self, SpendKeys};
use krypton_crypto::sign::MlDsaKey;
use krypton_zk::{AssetWitness, InputWitness, OutputWitness, SpendWitness};
use rand::RngCore;
use std::collections::{HashMap, HashSet};

const GNR: u64 = 100_000_000;

#[derive(Default)]
struct Mem {
    nfs: HashSet<Hash32>,
    roots: HashMap<Hash32, u64>,
    assets: HashMap<AssetId, AssetState>,
    names: HashMap<String, AssetId>,
}
impl RewardView for Mem {
    fn pool(&self) -> u64 {
        0
    }
    fn account(&self, _: &Hash32) -> Option<Account> {
        None
    }
    fn due_accounts(&self, _: u64, _: usize) -> Vec<(Hash32, Account)> {
        vec![]
    }
}
impl AssetView for Mem {
    fn asset(&self, id: &AssetId) -> Option<AssetState> {
        self.assets.get(id).cloned()
    }
    fn asset_named(&self, key: &str) -> Option<AssetId> {
        self.names.get(key).copied()
    }
}
impl StateView for Mem {
    fn nullifier_spent(&self, nf: &Hash32) -> bool {
        self.nfs.contains(nf)
    }
    fn anchor_height(&self, root: &Hash32) -> Option<u64> {
        self.roots.get(root).copied()
    }
    fn main_hash(&self, _: u64) -> Option<Hash32> {
        None
    }
    fn share_seen(&self, _: &Hash32) -> bool {
        false
    }
}

fn rd() -> Digest {
    let mut r = rand::thread_rng();
    [0; 4].map(|_: u8| Felt::new(r.next_u64() % krypton_crypto::field::MODULUS))
}

/// A note the test wallet owns.
#[derive(Clone)]
struct Note {
    sk: Digest,
    value: u64,
    rcm: Digest,
    pos: u64,
    #[allow(dead_code)]
    coin: [u64; 3],
}

struct World {
    p: ChainParams,
    mem: Mem,
    tree: MemTree,
    height: u64,
}

impl World {
    fn new() -> Self {
        let mut w = Self {
            p: Network::Devnet.params(),
            mem: Mem::default(),
            tree: MemTree::new(),
            height: 1,
        };
        w.mem.roots.insert(digest_to_bytes(&w.tree.root()), 0);
        w
    }
    /// Puts a note straight into the tree (as if paid earlier).
    fn fund(&mut self, sk: u64, value: u64) -> Note {
        let n = Note {
            sk: rescue::scalar(sk),
            value,
            rcm: rd(),
            pos: 0,
            coin: [0; 3],
        };
        let owner = SpendKeys::from_sk(n.sk).owner;
        let pos = self
            .tree
            .append(rescue::note_commitment(&owner, value, &n.rcm));
        self.mem
            .roots
            .insert(digest_to_bytes(&self.tree.root()), self.height);
        Note { pos, ..n }
    }
    fn input(&self, n: &Note) -> InputWitness {
        InputWitness {
            sk: n.sk,
            value: n.value,
            rcm: n.rcm,
            pos: n.pos,
            path: self.tree.path(n.pos),
            dummy: false,
            lock: false,
        }
    }
    fn anchor(&self) -> Hash32 {
        digest_to_bytes(&self.tree.root())
    }
    fn check(&self, tx: &Transaction) -> Result<AssetOverlay, CoreError> {
        let mut ov = AssetOverlay::default();
        self.check_in(&mut ov, tx)?;
        Ok(ov)
    }
    fn check_in(&self, ov: &mut AssetOverlay, tx: &Transaction) -> Result<(), CoreError> {
        let tx = Transaction::from_bytes(&tx.to_bytes()).expect("encoding roundtrip");
        let st = check_tx_structure(&tx)?;
        verify_proof(&tx, &st)?;
        check_tx_state(&self.p, &self.mem, ov, &tx, self.height)
    }
    /// Applies a valid transaction; returns the new notes for `sk` (outputs that
    /// `outs` says belong to it), in the tree.
    fn apply(&mut self, tx: &Transaction, outs: &[(usize, Note)]) -> Vec<Note> {
        let ov = self.check(tx).expect("valid");
        for (id, s) in ov.changes() {
            for k in name_keys(&s.manifest) {
                self.mem.names.insert(k, id);
            }
            self.mem.assets.insert(id, s);
        }
        self.mem.nfs.extend(tx.all_nullifiers());
        let mut placed = Vec::new();
        for (i, o) in tx.all_outputs().iter().enumerate() {
            let pos = self
                .tree
                .append(krypton_crypto::field::digest_from_bytes(&o.cm).unwrap());
            if let Some((_, n)) = outs.iter().find(|(j, _)| *j == i) {
                placed.push(Note { pos, ..n.clone() });
            }
        }
        self.height += 1;
        self.mem
            .roots
            .insert(digest_to_bytes(&self.tree.root()), self.height);
        placed
    }
}

fn output(o: &OutputWitness, coin: &[u64; 3]) -> Output {
    Output {
        cm: digest_to_bytes(&o.commitment_of(coin)),
        ciphertext: vec![7; NOTE_CIPHERTEXT_LEN],
    }
}

fn to_me(sk: u64, value: u64, coin: [u64; 3]) -> (OutputWitness, Note) {
    let o = OutputWitness {
        owner: SpendKeys::from_sk(rescue::scalar(sk)).owner,
        value,
        rcm: rd(),
    };
    let n = Note {
        sk: rescue::scalar(sk),
        value,
        rcm: o.rcm,
        pos: 0,
        coin,
    };
    (o, n)
}

/// A spend of up to two notes (padded with dummies) to two outputs.
fn spend_witness(
    w: &World,
    ins: &[&Note],
    outs: [OutputWitness; 2],
    fee: u64,
    mint: u64,
    asset: Option<AssetWitness>,
) -> SpendWitness {
    let mut inputs = [InputWitness::dummy(), InputWitness::dummy()];
    for (i, n) in ins.iter().enumerate() {
        inputs[i] = w.input(n);
    }
    let root = w.tree.root();
    SpendWitness {
        inputs,
        outputs: outs,
        root,
        fee,
        mint,
        sighash: [Felt::new(0); 4],
        asset,
    }
}

fn fill(tx: &mut Transaction, sw: &SpendWitness, anchor: Hash32) {
    let p = sw.public();
    tx.anchor = anchor;
    tx.nullifiers = p.nullifiers.map(|n| HexHash(digest_to_bytes(&n)));
    let coin = sw.coin();
    tx.outputs = [output(&sw.outputs[0], &coin), output(&sw.outputs[1], &coin)];
}

/// Builds, signs and proves a version-3 transaction.
fn build(
    w: &World,
    main: SpendWitness,
    part: AssetPart,
    fee_w: Option<SpendWitness>,
    issuer: Option<&MlDsaKey>,
) -> Transaction {
    let dummy_out = || Output {
        cm: [0; 32],
        ciphertext: vec![],
    };
    let mut tx = Transaction {
        version: TX_VERSION_ASSET,
        kind: TxKind::Transfer,
        anchor: [0; 32],
        nullifiers: [HexHash([0; 32]); 2],
        outputs: [dummy_out(), dummy_out()],
        fee: main.fee,
        mint: main.mint,
        expiry_height: 0,
        lock: None,
        proof: vec![],
        asset: Some(part),
    };
    let anchor = |_: &SpendWitness| w.anchor();
    fill(&mut tx, &main, anchor(&main));
    if let Some(f) = &fee_w {
        let p = f.public();
        tx.asset.as_mut().unwrap().fee_action = Some(FeeAction {
            anchor: anchor(f),
            nullifiers: p.nullifiers.map(|n| HexHash(digest_to_bytes(&n))),
            outputs: [
                output(&f.outputs[0], &[0; 3]),
                output(&f.outputs[1], &[0; 3]),
            ],
            fee: f.fee,
            proof: vec![],
        });
    }
    let sighash = tx.sighash();
    if let Some(k) = issuer {
        let sig = k.sign(&tx.sighash_bytes(), SIG_CTX).unwrap();
        tx.asset.as_mut().unwrap().issuer_sig = Some(sig);
    }
    assert_eq!(
        tx.sighash(),
        sighash,
        "the signature is outside the sighash"
    );
    let mut main = main;
    main.sighash = sighash;
    tx.proof = krypton_zk::prove(&main).expect("prove").1;
    if let Some(mut f) = fee_w {
        f.sighash = sighash;
        tx.asset
            .as_mut()
            .unwrap()
            .fee_action
            .as_mut()
            .unwrap()
            .proof = krypton_zk::prove(&f).expect("prove fee").1;
    }
    tx
}

fn manifest(
    name: &str,
    ticker: &str,
    supply: Supply,
    cap: Option<u64>,
    pause: bool,
    key: &MlDsaKey,
) -> Manifest {
    Manifest {
        name: name.into(),
        ticker: ticker.into(),
        decimals: 2,
        supply,
        cap,
        burn: true,
        pause,
        backing: Backing::None,
        origin: None,
        reserves: None,
        website: None,
        terms: None,
        logo: None,
        issuer: key.public_bytes(),
    }
}

fn part(op: AssetOp, coin: Option<AssetId>) -> AssetPart {
    AssetPart {
        op,
        coin,
        burn: 0,
        manifest: None,
        issuer_sig: None,
        fee_action: None,
    }
}

/// A GNR deploy or pause/unpause paid from `n` (fee `fee`, change to the payer).
fn gnr_action(
    w: &World,
    n: &Note,
    fee: u64,
    part: AssetPart,
    key: &MlDsaKey,
) -> (Transaction, Note) {
    let (change, cn) = to_me(1, n.value - fee, [0; 3]);
    let sw = spend_witness(w, &[n], [change, OutputWitness::dummy()], fee, 0, None);
    (build(w, sw, part, None, Some(key)), cn)
}

fn fee_from(w: &World, n: &Note) -> (SpendWitness, Note) {
    let fee = GNR / 100;
    let (change, cn) = to_me(1, n.value - fee, [0; 3]);
    (
        spend_witness(w, &[n], [change, OutputWitness::dummy()], fee, 0, None),
        cn,
    )
}

fn deploy(w: &World, m: &Manifest, payer: &Note, key: &MlDsaKey) -> (Transaction, Note) {
    let mut p = part(AssetOp::Deploy, None);
    p.manifest = Some(m.canonical());
    gnr_action(w, payer, DEPLOY_FEE, p, key)
}

fn issue(
    w: &World,
    id: AssetId,
    to: u64,
    amount: u64,
    payer: &Note,
    key: &MlDsaKey,
) -> (Transaction, Note, Note) {
    let c = id.elements();
    let (o, n) = to_me(to, amount, c);
    let sw = spend_witness(
        w,
        &[],
        [o, OutputWitness::dummy()],
        0,
        amount,
        Some(AssetWitness {
            id: c,
            burn: 0,
            reveal: true,
        }),
    );
    let (fw, change) = fee_from(w, payer);
    (
        build(w, sw, part(AssetOp::Issue, Some(id)), Some(fw), Some(key)),
        n,
        change,
    )
}

#[test]
fn coin_lifecycle() {
    let mut w = World::new();
    let key = MlDsaKey::from_seed(&[1; 32]);
    let mallory = MlDsaKey::from_seed(&[2; 32]);
    let mut gnr: Vec<Note> = (0..12).map(|_| w.fund(1, 50 * GNR)).collect();
    let m = manifest(
        "Test Dollar Krypton",
        "TUSDK",
        Supply::Mintable,
        Some(1_000_000),
        false,
        &key,
    );
    let id = m.id();

    // Deploy: the fee, the signature and the manifest are all checked.
    let (tx, change) = deploy(&w, &m, &gnr.pop().unwrap(), &key);
    {
        let mut bad = tx.clone();
        let a = bad.asset.as_mut().unwrap();
        a.issuer_sig = Some(mallory.sign(&tx.sighash_bytes(), SIG_CTX).unwrap());
        assert_eq!(
            w.check(&bad).unwrap_err(),
            CoreError::Invalid("invalid issuer signature")
        );
        let mut late = w.p.clone();
        late.assets_height = 100;
        assert_eq!(
            check_tx_state(&late, &w.mem, &mut AssetOverlay::default(), &tx, w.height).unwrap_err(),
            CoreError::Invalid("Krypton coins are not active yet")
        );
        let mut reserved = w.p.clone();
        reserved.reserved_coins = &[("TUSDK", "00")];
        let mut ov = AssetOverlay::default();
        let tx2 = Transaction::from_bytes(&tx.to_bytes()).unwrap();
        check_tx_structure(&tx2).unwrap();
        assert_eq!(
            check_tx_state(&reserved, &w.mem, &mut ov, &tx2, w.height).unwrap_err(),
            CoreError::Invalid("ticker reserved for another issuer")
        );
    }
    let (cheap, _) = gnr_action(
        &w,
        &gnr[0],
        GNR,
        {
            let mut p = part(AssetOp::Deploy, None);
            p.manifest = Some(m.canonical());
            p
        },
        &key,
    );
    assert!(w.check(&cheap).is_err(), "deploys pay 10 GNR");
    // Two deploys of one ticker in one block: the second is refused.
    let other = manifest(
        "Other Dollar Krypton",
        "TUSDK",
        Supply::Fixed,
        Some(5),
        false,
        &mallory,
    );
    let (tx_other, _) = deploy(&w, &other, &gnr[1], &mallory);
    let mut ov = AssetOverlay::default();
    w.check_in(&mut ov, &tx).unwrap();
    assert_eq!(
        w.check_in(&mut ov, &tx_other).unwrap_err(),
        CoreError::Invalid("ticker or name already taken")
    );
    w.apply(&tx, &[]);
    let _ = change;
    assert!(w.check(&tx_other).is_err(), "ticker taken on chain");
    assert_eq!(w.mem.assets[&id].issued, 0);

    // Issue: only the issuer, only within the cap.
    let (tx, _, _) = issue(&w, id, 9, 1_000, &gnr.pop().unwrap(), &mallory);
    assert_eq!(
        w.check(&tx).unwrap_err(),
        CoreError::Invalid("invalid issuer signature")
    );
    let (tx, _, _) = issue(&w, id, 9, 1_000_001, &gnr.pop().unwrap(), &key);
    assert_eq!(
        w.check(&tx).unwrap_err(),
        CoreError::Invalid("the issue exceeds the declared supply")
    );
    let (tx, coin_note, _) = issue(&w, id, 9, 1_000, &gnr.pop().unwrap(), &key);
    let notes = w.apply(&tx, &[(0, coin_note)]);
    assert_eq!(w.mem.assets[&id].issued, 1_000);
    let held = notes[0].clone();

    // A hidden transfer: no coin on chain, the fee in GNR.
    let c = id.elements();
    let (o1, n1) = to_me(10, 600, c);
    let (o2, n2) = to_me(9, 400, c);
    let sw = spend_witness(
        &w,
        &[&held],
        [o1, o2],
        0,
        0,
        Some(AssetWitness {
            id: c,
            burn: 0,
            reveal: false,
        }),
    );
    let (fw, _) = fee_from(&w, &gnr.pop().unwrap());
    let tx = build(&w, sw, part(AssetOp::Transfer, None), Some(fw), None);
    assert!(tx.asset.as_ref().unwrap().coin.is_none());
    // Claiming the proof is about another (or a public) coin fails.
    let mut bad = tx.clone();
    bad.asset.as_mut().unwrap().coin = Some(other.id());
    assert!(w.check(&bad).is_err());
    // Stealing the fee action into another transaction fails (sighash).
    let mut bad = tx.clone();
    bad.expiry_height = 99;
    assert!(w.check(&bad).is_err());
    // No fee action, no transfer.
    let mut bad = tx.clone();
    bad.asset.as_mut().unwrap().fee_action = None;
    assert!(w.check(&bad).is_err());
    let notes = w.apply(&tx, &[(0, n1), (1, n2)]);
    assert!(w.check(&tx).is_err(), "no double spend");

    // Burn 100 of the 600: public, recorded.
    let (o1, n1) = to_me(10, 500, c);
    let sw = spend_witness(
        &w,
        &[&notes[0]],
        [o1, OutputWitness::dummy()],
        0,
        0,
        Some(AssetWitness {
            id: c,
            burn: 100,
            reveal: true,
        }),
    );
    let (fw, _) = fee_from(&w, &gnr.pop().unwrap());
    let mut bp = part(AssetOp::Burn, Some(id));
    bp.burn = 100;
    let tx = build(&w, sw, bp, Some(fw), None);
    w.apply(&tx, &[(0, n1)]);
    assert_eq!(
        (w.mem.assets[&id].issued, w.mem.assets[&id].burned),
        (1_000, 100)
    );
    assert_eq!(w.mem.assets[&id].outstanding(), 900);

    // A fixed coin: exactly its cap, once.
    let fm = manifest(
        "Fixed Krypton",
        "FIXK",
        Supply::Fixed,
        Some(5_000),
        false,
        &key,
    );
    let fid = fm.id();
    let (tx, _) = deploy(&w, &fm, &gnr.pop().unwrap(), &key);
    w.apply(&tx, &[]);
    let (tx, _, _) = issue(&w, fid, 9, 4_999, &gnr.pop().unwrap(), &key);
    assert!(w.check(&tx).is_err(), "a fixed coin issues exactly its cap");
    let (tx, _, _) = issue(&w, fid, 9, 5_000, &gnr.pop().unwrap(), &key);
    w.apply(&tx, &[]);
    let (tx, _, _) = issue(&w, fid, 9, 5_000, &gnr.pop().unwrap(), &key);
    assert!(w.check(&tx).is_err(), "a fixed coin issues once");
}

#[test]
fn pausable_coins_reveal_and_stop() {
    let mut w = World::new();
    let key = MlDsaKey::from_seed(&[3; 32]);
    let mut gnr: Vec<Note> = (0..8).map(|_| w.fund(1, 50 * GNR)).collect();
    let m = manifest(
        "Pause Krypton",
        "PAUSEK",
        Supply::Mintable,
        None,
        true,
        &key,
    );
    let id = m.id();
    assert!(id.pausable);
    let (tx, _) = deploy(&w, &m, &gnr.pop().unwrap(), &key);
    w.apply(&tx, &[]);
    let (tx, n, _) = issue(&w, id, 9, 77, &gnr.pop().unwrap(), &key);
    let held = w.apply(&tx, &[(0, n)])[0].clone();
    let c = id.elements();
    let transfer = |w: &World, payer: &Note| {
        let (o, _) = to_me(10, 77, c);
        let sw = spend_witness(
            w,
            &[&held],
            [o, OutputWitness::dummy()],
            0,
            0,
            Some(AssetWitness {
                id: c,
                burn: 0,
                reveal: true,
            }),
        );
        let (fw, _) = fee_from(w, payer);
        build(w, sw, part(AssetOp::Transfer, Some(id)), Some(fw), None)
    };
    // Hidden transfers of a pausable coin cannot even be proven.
    let (o, _) = to_me(10, 77, c);
    let sw = spend_witness(
        &w,
        &[&held],
        [o, OutputWitness::dummy()],
        0,
        0,
        Some(AssetWitness {
            id: c,
            burn: 0,
            reveal: false,
        }),
    );
    assert!(krypton_zk::prove(&sw).is_err());

    let (tx, _) = gnr_action(
        &w,
        &gnr.pop().unwrap(),
        GNR / 100,
        part(AssetOp::Pause, Some(id)),
        &key,
    );
    w.apply(&tx, &[]);
    assert!(w.mem.assets[&id].paused);
    assert_eq!(
        w.check(&transfer(&w, &gnr[0])).unwrap_err(),
        CoreError::Invalid("the coin is paused")
    );
    let (tx, _) = gnr_action(
        &w,
        &gnr.pop().unwrap(),
        GNR / 100,
        part(AssetOp::Pause, Some(id)),
        &key,
    );
    assert!(w.check(&tx).is_err(), "already paused");
    let (tx, _) = gnr_action(
        &w,
        &gnr.pop().unwrap(),
        GNR / 100,
        part(AssetOp::Unpause, Some(id)),
        &key,
    );
    w.apply(&tx, &[]);
    let tx = transfer(&w, &gnr.pop().unwrap());
    w.apply(&tx, &[]);
}

#[test]
fn coin_parts_are_strict() {
    let mut w = World::new();
    let key = MlDsaKey::from_seed(&[4; 32]);
    let n = w.fund(1, 50 * GNR);
    let m = manifest(
        "Strict Krypton",
        "STRK",
        Supply::Mintable,
        None,
        false,
        &key,
    );
    let (tx, _) = deploy(&w, &m, &n, &key);
    // Every byte of the encoding matters and nothing may trail it.
    let mut b = tx.to_bytes();
    b.push(0);
    assert!(Transaction::from_bytes(&b).is_err());
    // Version 1 and 2 transactions cannot carry a coin part, nor v3 omit it.
    let mut bad = tx.clone();
    bad.version = TX_VERSION;
    assert!(Transaction::from_bytes(&bad.to_bytes()).is_err());
    assert!(check_tx_structure(&bad).is_err());
    let mut bad = tx.clone();
    bad.asset = None;
    assert!(check_tx_structure(&bad).is_err());
    // A tampered manifest changes the sighash, so the proofs and signature fail.
    let mut bad = tx.clone();
    bad.asset.as_mut().unwrap().manifest = Some(m.canonical().replace("Strict", "Struct"));
    assert!(w.check(&bad).is_err());
    // GNR itself can never be issued as a coin.
    let mut bad = tx.clone();
    let a = bad.asset.as_mut().unwrap();
    a.op = AssetOp::Issue;
    a.coin = Some(krypton_assets::GNR);
    a.manifest = None;
    assert!(check_tx_structure(&bad).is_err());
    w.check(&tx).unwrap();
}

/// The project's stable coins: their published manifests are valid, their tickers are
/// reserved on mainnet for exactly their issuer key, and the signed verified list names
/// exactly these coins.
#[test]
fn project_coins_are_consistent() {
    let p = Network::Mainnet.params();
    let list: serde_json::Value =
        serde_json::from_str(include_str!("../../../docs/verified-assets.json")).unwrap();
    for (text, ticker) in [
        (include_str!("../../../docs/coins/usdk.kam"), "USDK"),
        (include_str!("../../../docs/coins/eurk.kam"), "EURK"),
    ] {
        let m = Manifest::parse(text).expect("canonical manifest");
        m.validate().unwrap();
        assert_eq!(m.ticker, ticker);
        assert!(!m.pause, "the stable coins can never be paused");
        let (_, fp) = p
            .reserved_coins
            .iter()
            .find(|(t, _)| *t == ticker)
            .expect("reserved");
        assert_eq!(hex::encode(issuer_fingerprint(&m.issuer)), *fp);
        let listed = list["coins"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["ticker"] == ticker)
            .expect("verified");
        assert_eq!(listed["id"].as_str().unwrap(), m.id().to_hex());
    }
    assert!(p.assets_height > 15_553 && p.assets_height < u64::MAX);
}

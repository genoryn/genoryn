//! The spend AIR: 2-in / 2-out shielded transfer.
//!
//! Statement (public: anchor root, two nullifiers, two output commitments, fee,
//! mint, sighash). For each input i the prover knows `sk_i, v_i, rcm_i, pos_i`
//! and a Merkle path such that, unless the input is a zero-value dummy,
//!
//! * `cm_i = NoteCommit(Owner(AK(sk_i), NK(sk_i)), v_i, rcm_i)` is the leaf at
//!   position `pos_i` of the depth-40 note tree with root `root`,
//! * `nf_i = H(NF; NK(sk_i), pos_i)`,
//!
//! and for each output j `cm_out_j = NoteCommit(owner_j, w_j, rcm_j)`; all values are
//! below 2^60 and `v_0 + v_1 + mint = w_0 + w_1 + fee`. The sighash is absorbed into
//! the Fiat-Shamir transcript, binding the proof to one exact transaction.
//!
//! Lock spends (atomic swaps, `lock_owner` set): input 0 is a real note whose owner is
//! `H(LOCK_OWNER; AK(sk_0), NK(sk_0))` and equals the public `lock_owner`. Which outputs
//! the note may pay is enforced by consensus, outside the proof.

use crate::layout::*;
use krypton_crypto::field::{Digest, Felt};
use krypton_crypto::rescue::domain;
use winterfell::crypto::hashers::Rp64_256;
use winterfell::math::{FieldElement, ToElements};
use winterfell::{
    Air, AirContext, Assertion, EvaluationFrame, ProofOptions, TraceInfo,
    TransitionConstraintDegree,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SpendPublic {
    pub root: Digest,
    pub nullifiers: [Digest; 2],
    pub output_cms: [Digest; 2],
    pub fee: u64,
    pub mint: u64,
    pub sighash: Digest,
    /// Owner tag of the lock note spent by input 0 (lock spends only).
    pub lock_owner: Option<Digest>,
}

impl ToElements<Felt> for SpendPublic {
    fn to_elements(&self) -> Vec<Felt> {
        let mut v = Vec::with_capacity(31);
        v.extend_from_slice(&self.root);
        for d in self.nullifiers.iter().chain(self.output_cms.iter()) {
            v.extend_from_slice(d);
        }
        v.push(Felt::new(self.fee));
        v.push(Felt::new(self.mint));
        v.extend_from_slice(&self.sighash);
        // Ordinary spends keep exactly the statement they always had.
        if let Some(o) = &self.lock_owner {
            v.push(Felt::new(domain::LOCK_OWNER));
            v.extend_from_slice(o);
        }
        v
    }
}

pub struct SpendAir {
    context: AirContext<Felt>,
    pub_inputs: SpendPublic,
}

/// Collects constraint results or their declared degrees in one shared order.
trait Sink<E> {
    fn push(&mut self, value: impl FnOnce() -> E, base: usize, cycles: &[usize]);
}

struct Degrees(Vec<TransitionConstraintDegree>);
impl<E> Sink<E> for Degrees {
    fn push(&mut self, _value: impl FnOnce() -> E, base: usize, cycles: &[usize]) {
        self.0.push(if cycles.is_empty() {
            TransitionConstraintDegree::new(base)
        } else {
            TransitionConstraintDegree::with_cycles(base, cycles.to_vec())
        });
    }
}

struct Results<'a, E> {
    out: &'a mut [E],
    i: usize,
}
impl<E: Copy> Sink<E> for Results<'_, E> {
    fn push(&mut self, value: impl FnOnce() -> E, _base: usize, _cycles: &[usize]) {
        self.out[self.i] = value();
        self.i += 1;
    }
}

const N: usize = TRACE_LEN;

fn exp7<E: FieldElement>(x: E) -> E {
    let x2 = x.square();
    let x4 = x2.square();
    x4 * x2 * x
}

/// Emits every transition constraint. `cur`, `nxt`, `p` are only read inside the
/// closures, so degree collection never touches trace data.
fn constraints<E, S>(s: &mut S, cur: &[E], nxt: &[E], p: &[E], pubs: &SpendPublic)
where
    E: FieldElement + From<Felt>,
    S: Sink<E>,
{
    let one = E::ONE;
    let c = |v: u64| E::from(Felt::new(v));

    // Rescue rounds for all four hash lanes.
    for &base in K_LANE.iter().chain(M_LANE.iter()) {
        for j in 0..12 {
            s.push(
                || {
                    let mut mid = E::ZERO;
                    for l in 0..12 {
                        mid += E::from(Rp64_256::MDS[j][l]) * exp7(cur[base + l]);
                    }
                    mid += p[P_ARK1 + j];
                    let mut u = E::ZERO;
                    for l in 0..12 {
                        u += E::from(Rp64_256::INV_MDS[j][l]) * (nxt[base + l] - p[P_ARK2 + l]);
                    }
                    p[P_ROUND] * p[P_ACT] * (exp7(u) - mid)
                },
                7,
                &[CYCLE, N],
            );
        }
    }

    // Key/note lanes: fixed program with register links.
    for lane in 0..2 {
        let h = K_LANE[lane];
        let r = REG[lane];
        for (k, spec) in K_PROGRAM.iter().enumerate() {
            let start = p[P_START + k];
            let dom = if lane == 0 && k == OWNER_CYCLE && pubs.lock_owner.is_some() {
                domain::LOCK_OWNER
            } else {
                spec.domain
            };
            s.push(
                || {
                    let mut d = c(dom);
                    if spec.dummy_domain {
                        d += c(domain::DUMMY_OFFSET) * cur[r + R_DUMMY];
                    }
                    start * (cur[h] - d)
                },
                1,
                &[N],
            );
            for j in 1..4 {
                s.push(|| start * cur[h + j], 1, &[N]);
            }
            for (half, src) in [(4usize, spec.left), (8usize, spec.right)] {
                for j in 0..4 {
                    s.push(
                        || {
                            let want = match src {
                                Src::Zero => E::ZERO,
                                Src::Reg(o) => cur[r + o + j],
                                Src::Scalar(o) => {
                                    if j == 0 {
                                        cur[r + o]
                                    } else {
                                        E::ZERO
                                    }
                                }
                            };
                            start * (cur[h + half + j] - want)
                        },
                        1,
                        &[N],
                    );
                }
            }
            if let Out::Reg(o) = spec.out {
                let end = p[P_END + k];
                for j in 0..4 {
                    s.push(|| end * (cur[h + 4 + j] - cur[r + o + j]), 1, &[N]);
                }
            }
        }
    }

    // Merkle lanes.
    for lane in 0..2 {
        let h = M_LANE[lane];
        let b = h + M_BIT;
        let a = h + M_ACC;
        let r = REG[lane];
        s.push(|| p[P_ROUND] * p[P_ACT] * (nxt[b] - cur[b]), 1, &[CYCLE, N]);
        s.push(|| p[P_ROUND] * p[P_ACT] * (nxt[a] - cur[a]), 1, &[CYCLE, N]);
        s.push(|| p[P_ACT] * cur[b] * (one - cur[b]), 2, &[N]);
        // First cycle: the leaf is the input note commitment.
        let first = p[P_START];
        for j in 0..4 {
            s.push(
                || first * (one - cur[b]) * (cur[h + 4 + j] - cur[r + R_CM + j]),
                2,
                &[N],
            );
            s.push(
                || first * cur[b] * (cur[h + 8 + j] - cur[r + R_CM + j]),
                2,
                &[N],
            );
        }
        s.push(|| first * (cur[h] - c(domain::MERKLE)), 1, &[N]);
        for j in 1..4 {
            s.push(|| first * cur[h + j], 1, &[N]);
        }
        s.push(|| first * (cur[a] - cur[b]), 1, &[N]);
        // Cycle boundaries: the digest moves into the left or right half.
        let mb = p[P_MB];
        for j in 0..4 {
            s.push(
                || mb * (one - nxt[b]) * (nxt[h + 4 + j] - cur[h + 4 + j]),
                2,
                &[N],
            );
            s.push(|| mb * nxt[b] * (nxt[h + 8 + j] - cur[h + 4 + j]), 2, &[N]);
        }
        s.push(|| mb * (nxt[h] - c(domain::MERKLE)), 1, &[N]);
        for j in 1..4 {
            s.push(|| mb * nxt[h + j], 1, &[N]);
        }
        s.push(|| mb * (nxt[a] - cur[a]) - nxt[b] * p[P_P2N], 1, &[N]);
        // Final row: root (skipped for dummy inputs) and position.
        let end = p[P_MEND];
        for j in 0..4 {
            s.push(
                || end * (one - cur[r + R_DUMMY]) * (cur[h + 4 + j] - E::from(pubs.root[j])),
                2,
                &[N],
            );
        }
        s.push(|| end * (cur[a] - cur[r + R_POS]), 1, &[N]);
    }

    // Range checks (MSB-first bit accumulation over rows 0..60).
    for (m, &(lane, off)) in RANGE_VALUES.iter().enumerate() {
        let (ra, rb) = (RA + m, RB + m);
        s.push(|| p[P_RC] * (nxt[ra] - cur[ra].double() - cur[rb]), 1, &[N]);
        s.push(|| p[P_RC] * cur[rb] * (one - cur[rb]), 2, &[N]);
        s.push(|| p[P_RCE] * (cur[ra] - cur[REG[lane] + off]), 1, &[N]);
    }

    // Registers are constant over the active region.
    for lane in 0..2 {
        for o in 0..REG_WIDTH {
            let col = REG[lane] + o;
            s.push(|| p[P_ACT] * (nxt[col] - cur[col]), 1, &[N]);
        }
        let d = REG[lane] + R_DUMMY;
        s.push(|| p[P_ACT] * cur[d] * (one - cur[d]), 2, &[N]);
        s.push(|| p[P_ACT] * cur[d] * cur[REG[lane] + R_V], 2, &[N]);
    }

    // Value balance: v0 + v1 + mint = w0 + w1 + fee.
    s.push(
        || {
            let (r0, r1) = (REG[0], REG[1]);
            p[P_ACT]
                * (cur[r0 + R_V] + cur[r1 + R_V] + E::from(Felt::new(pubs.mint))
                    - cur[r0 + R_O_V]
                    - cur[r1 + R_O_V]
                    - E::from(Felt::new(pubs.fee)))
        },
        1,
        &[N],
    );
}

impl Air for SpendAir {
    type BaseField = Felt;
    type PublicInputs = SpendPublic;

    fn new(trace_info: TraceInfo, pub_inputs: SpendPublic, options: ProofOptions) -> Self {
        let mut d = Degrees(Vec::new());
        let row = vec![Felt::ZERO; WIDTH];
        let per = vec![Felt::ZERO; NUM_PERIODIC];
        constraints::<Felt, _>(&mut d, &row, &row, &per, &pub_inputs);
        let n = assertions(&pub_inputs).len();
        let context = AirContext::new(trace_info, d.0, n, options);
        Self {
            context,
            pub_inputs,
        }
    }

    fn context(&self) -> &AirContext<Felt> {
        &self.context
    }

    fn evaluate_transition<E: FieldElement + From<Felt>>(
        &self,
        frame: &EvaluationFrame<E>,
        periodic_values: &[E],
        result: &mut [E],
    ) {
        let mut sink = Results { out: result, i: 0 };
        constraints(
            &mut sink,
            frame.current(),
            frame.next(),
            periodic_values,
            &self.pub_inputs,
        );
        debug_assert_eq!(sink.i, sink.out.len());
    }

    fn get_assertions(&self) -> Vec<Assertion<Felt>> {
        assertions(&self.pub_inputs)
    }

    fn get_periodic_column_values(&self) -> Vec<Vec<Felt>> {
        periodic_columns()
    }
}

fn assertions(pubs: &SpendPublic) -> Vec<Assertion<Felt>> {
    let mut a = Vec::with_capacity(25);
    for m in 0..4 {
        a.push(Assertion::single(RA + m, 0, Felt::ZERO));
    }
    for lane in 0..2 {
        for j in 0..4 {
            a.push(Assertion::single(
                K_LANE[lane] + 4 + j,
                NULLIFIER_ROW,
                pubs.nullifiers[lane][j],
            ));
            a.push(Assertion::single(
                K_LANE[lane] + 4 + j,
                OUTPUT_CM_ROW,
                pubs.output_cms[lane][j],
            ));
        }
    }
    // Registers are constant over the active region, so row 0 pins them for the
    // whole program: input 0 is the lock note itself and not a dummy.
    if let Some(o) = &pubs.lock_owner {
        for j in 0..4 {
            a.push(Assertion::single(REG[0] + R_OWNER + j, 0, o[j]));
        }
        a.push(Assertion::single(REG[0] + R_DUMMY, 0, Felt::ZERO));
    }
    a
}

pub fn periodic_columns() -> Vec<Vec<Felt>> {
    let mut cols = Vec::with_capacity(NUM_PERIODIC);
    let z = Felt::ZERO;
    let o = Felt::ONE;
    cols.push((0..CYCLE).map(|r| if r < ROUNDS { o } else { z }).collect());
    for j in 0..12 {
        cols.push(
            (0..CYCLE)
                .map(|r| if r < ROUNDS { Rp64_256::ARK1[r][j] } else { z })
                .collect(),
        );
    }
    for j in 0..12 {
        cols.push(
            (0..CYCLE)
                .map(|r| if r < ROUNDS { Rp64_256::ARK2[r][j] } else { z })
                .collect(),
        );
    }
    let full = |f: &dyn Fn(usize) -> Felt| (0..N).map(f).collect::<Vec<_>>();
    cols.push(full(&|r| if r + 1 < ACTIVE_ROWS { o } else { z }));
    for k in 0..8 {
        cols.push(full(&|r| if r == k * CYCLE { o } else { z }));
    }
    for k in 0..8 {
        cols.push(full(&|r| if r == k * CYCLE + CYCLE - 1 { o } else { z }));
    }
    cols.push(full(&|r| {
        if r % CYCLE == CYCLE - 1 && r / CYCLE + 1 < DEPTH {
            o
        } else {
            z
        }
    }));
    cols.push(full(&|r| {
        if r % CYCLE == CYCLE - 1 && r / CYCLE + 1 < DEPTH {
            Felt::new(1u64 << (r / CYCLE + 1))
        } else {
            z
        }
    }));
    cols.push(full(&|r| if r == ACTIVE_ROWS - 1 { o } else { z }));
    cols.push(full(&|r| if r < RANGE_BITS { o } else { z }));
    cols.push(full(&|r| if r == RANGE_BITS { o } else { z }));
    debug_assert_eq!(cols.len(), NUM_PERIODIC);
    cols
}

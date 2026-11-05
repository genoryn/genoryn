use crate::ZkError;
use crate::air::{SpendAir, SpendPublic};
use crate::layout::*;
use krypton_crypto::field::{Digest, Felt, ZERO_DIGEST};
use krypton_crypto::rescue::{self, SpendKeys, domain};
use rand_core::{OsRng, RngCore};
use winterfell::crypto::hashers::{Rp64_256, Sha3_256};
use winterfell::crypto::{DefaultRandomCoin, MerkleTree};
use winterfell::math::FieldElement;
use winterfell::matrix::ColMatrix;
use winterfell::{
    AcceptableOptions, AuxRandElements, BatchingMethod, CompositionPoly, CompositionPolyTrace,
    ConstraintCompositionCoefficients, DefaultConstraintCommitment, DefaultConstraintEvaluator,
    DefaultTraceLde, FieldExtension, PartitionOptions, Proof, ProofOptions, Prover, StarkDomain,
    TraceInfo, TracePolyTable, TraceTable,
};

type H = Sha3_256<Felt>;
type VC = MerkleTree<H>;
type Coin = DefaultRandomCoin<H>;

/// 36 queries x log2(16) + 16 grinding bits = 160 bits conjectured security,
/// cubic extension of Goldilocks (~192-bit field).
pub fn proof_options() -> ProofOptions {
    ProofOptions::new(
        36,
        16,
        16,
        FieldExtension::Cubic,
        8,
        31,
        BatchingMethod::Linear,
        BatchingMethod::Linear,
    )
}

#[derive(Clone, Debug)]
pub struct InputWitness {
    pub sk: Digest,
    pub value: u64,
    pub rcm: Digest,
    pub pos: u64,
    /// Sibling hashes from the leaf level upward.
    pub path: Vec<Digest>,
    pub dummy: bool,
    /// A lock note (atomic swap; input 0 only): its owner tag is hashed under
    /// `LOCK_OWNER` instead of `OWNER`.
    pub lock: bool,
}

impl InputWitness {
    /// A zero-value input that proves nothing about the tree (padding / coinbase).
    pub fn dummy() -> Self {
        Self {
            sk: rand_digest(),
            value: 0,
            rcm: rand_digest(),
            pos: OsRng.next_u64() & ((1 << DEPTH) - 1),
            path: (0..DEPTH).map(|_| rand_digest()).collect(),
            dummy: true,
            lock: false,
        }
    }
    pub fn keys(&self) -> SpendKeys {
        SpendKeys::from_sk(self.sk)
    }
    /// The owner tag committed in the note.
    pub fn owner(&self) -> Digest {
        let k = self.keys();
        if self.lock {
            rescue::lock_owner_of(&k.ak, &k.nk)
        } else {
            k.owner
        }
    }
    pub fn commitment(&self) -> Digest {
        rescue::note_commitment(&self.owner(), self.value, &self.rcm)
    }
    pub fn nullifier(&self) -> Digest {
        let nk = self.keys().nk;
        if self.dummy {
            rescue::dummy_nullifier(&nk, self.pos)
        } else {
            rescue::nullifier(&nk, self.pos)
        }
    }
}

#[derive(Clone, Debug)]
pub struct OutputWitness {
    pub owner: Digest,
    pub value: u64,
    pub rcm: Digest,
}

impl OutputWitness {
    pub fn dummy() -> Self {
        Self {
            owner: rand_digest(),
            value: 0,
            rcm: rand_digest(),
        }
    }
    pub fn commitment(&self) -> Digest {
        rescue::note_commitment(&self.owner, self.value, &self.rcm)
    }
}

#[derive(Clone, Debug)]
pub struct SpendWitness {
    pub inputs: [InputWitness; 2],
    pub outputs: [OutputWitness; 2],
    pub root: Digest,
    pub fee: u64,
    pub mint: u64,
    pub sighash: Digest,
}

impl SpendWitness {
    pub fn public(&self) -> SpendPublic {
        SpendPublic {
            root: self.root,
            nullifiers: [self.inputs[0].nullifier(), self.inputs[1].nullifier()],
            output_cms: [self.outputs[0].commitment(), self.outputs[1].commitment()],
            fee: self.fee,
            mint: self.mint,
            sighash: self.sighash,
            lock_owner: self.inputs[0].lock.then(|| self.inputs[0].owner()),
        }
    }

    fn check(&self) -> Result<(), ZkError> {
        if self.inputs[1].lock || (self.inputs[0].lock && self.inputs[0].dummy) {
            return Err(ZkError::Witness(
                "only input 0 may be a lock note, and not a dummy",
            ));
        }
        for i in &self.inputs {
            if i.path.len() != DEPTH {
                return Err(ZkError::Witness("path length"));
            }
            if i.pos >= 1 << DEPTH {
                return Err(ZkError::Witness("position out of range"));
            }
            if i.dummy && i.value != 0 {
                return Err(ZkError::Witness("dummy input with value"));
            }
            if !i.dummy && merkle_root(&i.commitment(), i.pos, &i.path) != self.root {
                return Err(ZkError::Witness("note is not in the tree"));
            }
        }
        let vals = [
            self.inputs[0].value,
            self.inputs[1].value,
            self.outputs[0].value,
            self.outputs[1].value,
        ];
        if vals
            .iter()
            .chain([self.fee, self.mint].iter())
            .any(|&v| v > MAX_VALUE)
        {
            return Err(ZkError::Witness("value out of range"));
        }
        let lhs = vals[0] as u128 + vals[1] as u128 + self.mint as u128;
        let rhs = vals[2] as u128 + vals[3] as u128 + self.fee as u128;
        if lhs != rhs {
            return Err(ZkError::Witness("values do not balance"));
        }
        Ok(())
    }
}

pub fn merkle_root(leaf: &Digest, pos: u64, path: &[Digest]) -> Digest {
    let mut cur = *leaf;
    for (k, sib) in path.iter().enumerate() {
        cur = if (pos >> k) & 1 == 0 {
            rescue::merkle_node(&cur, sib)
        } else {
            rescue::merkle_node(sib, &cur)
        };
    }
    cur
}

fn rand_felt() -> Felt {
    Felt::new(OsRng.next_u64())
}

fn rand_digest() -> Digest {
    [rand_felt(), rand_felt(), rand_felt(), rand_felt()]
}

/// Writes one 8-row Rescue cycle starting at `row` into columns `base..base+12`.
fn write_cycle(cols: &mut [Vec<Felt>], base: usize, row: usize, mut state: [Felt; 12]) -> Digest {
    for r in 0..CYCLE {
        for j in 0..12 {
            cols[base + j][row + r] = state[j];
        }
        if r < ROUNDS {
            Rp64_256::apply_round(&mut state, r);
        }
    }
    [state[4], state[5], state[6], state[7]]
}

fn initial_state(dom: u64, left: &Digest, right: &Digest) -> [Felt; 12] {
    let mut s = [Felt::ZERO; 12];
    s[0] = Felt::new(dom);
    s[4..8].copy_from_slice(left);
    s[8..12].copy_from_slice(right);
    s
}

fn build_trace(w: &SpendWitness) -> TraceTable<Felt> {
    let mut cols: Vec<Vec<Felt>> = (0..WIDTH).map(|_| vec![Felt::ZERO; TRACE_LEN]).collect();
    let outs = [&w.outputs[0], &w.outputs[1]];

    for lane in 0..2 {
        let inp = &w.inputs[lane];
        let keys = inp.keys();
        let out = outs[lane];
        let owner = inp.owner();
        let o_t = rescue::hash2(domain::NOTE_BODY, &out.owner, &rescue::scalar(out.value));
        let t = rescue::hash2(domain::NOTE_BODY, &owner, &rescue::scalar(inp.value));
        let cm = inp.commitment();

        // Registers.
        let mut reg = [Felt::ZERO; REG_WIDTH];
        let put = |reg: &mut [Felt; REG_WIDTH], off: usize, d: &Digest| {
            reg[off..off + 4].copy_from_slice(d)
        };
        put(&mut reg, R_SK, &keys.sk);
        put(&mut reg, R_AK, &keys.ak);
        put(&mut reg, R_NK, &keys.nk);
        put(&mut reg, R_OWNER, &owner);
        put(&mut reg, R_T, &t);
        put(&mut reg, R_CM, &cm);
        put(&mut reg, R_RCM, &inp.rcm);
        reg[R_V] = Felt::new(inp.value);
        reg[R_POS] = Felt::new(inp.pos);
        reg[R_DUMMY] = if inp.dummy { Felt::ONE } else { Felt::ZERO };
        put(&mut reg, R_O_OWNER, &out.owner);
        put(&mut reg, R_O_T, &o_t);
        put(&mut reg, R_O_RCM, &out.rcm);
        reg[R_O_V] = Felt::new(out.value);
        for (o, v) in reg.iter().enumerate() {
            cols[REG[lane] + o][..ACTIVE_ROWS].fill(*v);
        }

        // Key/note lane program.
        let src = |s: Src| -> Digest {
            match s {
                Src::Zero => ZERO_DIGEST,
                Src::Reg(o) => [reg[o], reg[o + 1], reg[o + 2], reg[o + 3]],
                Src::Scalar(o) => [reg[o], Felt::ZERO, Felt::ZERO, Felt::ZERO],
            }
        };
        for (k, spec) in K_PROGRAM.iter().enumerate() {
            let mut dom = spec.domain;
            if k == OWNER_CYCLE && inp.lock {
                dom = domain::LOCK_OWNER;
            }
            if spec.dummy_domain && inp.dummy {
                dom += domain::DUMMY_OFFSET;
            }
            write_cycle(
                &mut cols,
                K_LANE[lane],
                k * CYCLE,
                initial_state(dom, &src(spec.left), &src(spec.right)),
            );
        }
        for k in K_PROGRAM.len()..DEPTH {
            write_cycle(&mut cols, K_LANE[lane], k * CYCLE, [Felt::ZERO; 12]);
        }

        // Merkle lane.
        let h = M_LANE[lane];
        let mut cur = cm;
        let mut acc = 0u64;
        for k in 0..DEPTH {
            let bit = (inp.pos >> k) & 1;
            acc |= bit << k;
            let sib = &inp.path[k];
            let st = if bit == 0 {
                initial_state(domain::MERKLE, &cur, sib)
            } else {
                initial_state(domain::MERKLE, sib, &cur)
            };
            cur = write_cycle(&mut cols, h, k * CYCLE, st);
            for r in 0..CYCLE {
                cols[h + M_BIT][k * CYCLE + r] = Felt::new(bit);
                cols[h + M_ACC][k * CYCLE + r] = Felt::new(acc);
            }
        }
    }

    // Range checks.
    let values = [
        w.inputs[0].value,
        w.inputs[1].value,
        w.outputs[0].value,
        w.outputs[1].value,
    ];
    for (m, &v) in values.iter().enumerate() {
        let mut acc = 0u64;
        for r in 0..ACTIVE_ROWS {
            cols[RA + m][r] = Felt::new(acc);
            if r < RANGE_BITS {
                let bit = (v >> (RANGE_BITS - 1 - r)) & 1;
                cols[RB + m][r] = Felt::new(bit);
                acc = 2 * acc + bit;
            }
        }
    }

    // Zero-knowledge randomization: random tail rows in every column, random mask column.
    for col in cols.iter_mut() {
        for v in col[ACTIVE_ROWS..].iter_mut() {
            *v = rand_felt();
        }
    }
    for v in cols[MASK].iter_mut() {
        *v = rand_felt();
    }
    TraceTable::init(cols)
}

struct SpendProver {
    options: ProofOptions,
    public: SpendPublic,
}

impl Prover for SpendProver {
    type BaseField = Felt;
    type Air = SpendAir;
    type Trace = TraceTable<Felt>;
    type HashFn = H;
    type VC = VC;
    type RandomCoin = Coin;
    type TraceLde<E: FieldElement<BaseField = Felt>> = DefaultTraceLde<E, H, VC>;
    type ConstraintCommitment<E: FieldElement<BaseField = Felt>> =
        DefaultConstraintCommitment<E, H, VC>;
    type ConstraintEvaluator<'a, E: FieldElement<BaseField = Felt>> =
        DefaultConstraintEvaluator<'a, SpendAir, E>;

    fn get_pub_inputs(&self, _trace: &Self::Trace) -> SpendPublic {
        self.public.clone()
    }
    fn options(&self) -> &ProofOptions {
        &self.options
    }
    fn new_trace_lde<E: FieldElement<BaseField = Felt>>(
        &self,
        trace_info: &TraceInfo,
        main_trace: &ColMatrix<Felt>,
        domain: &StarkDomain<Felt>,
        partition_option: PartitionOptions,
    ) -> (Self::TraceLde<E>, TracePolyTable<E>) {
        DefaultTraceLde::new(trace_info, main_trace, domain, partition_option)
    }
    fn build_constraint_commitment<E: FieldElement<BaseField = Felt>>(
        &self,
        composition_poly_trace: CompositionPolyTrace<E>,
        num_constraint_composition_columns: usize,
        domain: &StarkDomain<Felt>,
        partition_options: PartitionOptions,
    ) -> (Self::ConstraintCommitment<E>, CompositionPoly<E>) {
        DefaultConstraintCommitment::new(
            composition_poly_trace,
            num_constraint_composition_columns,
            domain,
            partition_options,
        )
    }
    fn new_evaluator<'a, E: FieldElement<BaseField = Felt>>(
        &self,
        air: &'a SpendAir,
        aux_rand_elements: Option<AuxRandElements<E>>,
        composition_coefficients: ConstraintCompositionCoefficients<E>,
    ) -> Self::ConstraintEvaluator<'a, E> {
        DefaultConstraintEvaluator::new(air, aux_rand_elements, composition_coefficients)
    }
}

/// Produces a serialized spend proof and its public statement.
pub fn prove(w: &SpendWitness) -> Result<(SpendPublic, Vec<u8>), ZkError> {
    w.check()?;
    let public = w.public();
    let trace = build_trace(w);
    let prover = SpendProver {
        options: proof_options(),
        public: public.clone(),
    };
    let proof = prover
        .prove(trace)
        .map_err(|e| ZkError::Prove(e.to_string()))?;
    Ok((public, proof.to_bytes()))
}

/// Verifies a serialized spend proof against its public statement.
pub fn verify(public: &SpendPublic, proof: &[u8]) -> Result<(), ZkError> {
    if public.fee > MAX_VALUE || public.mint > MAX_VALUE {
        return Err(ZkError::Verify("public value out of range".into()));
    }
    let proof = Proof::from_bytes(proof).map_err(|e| ZkError::Verify(e.to_string()))?;
    let ti = proof.trace_info();
    if ti.length() != TRACE_LEN || ti.main_trace_width() != WIDTH || ti.is_multi_segment() {
        return Err(ZkError::Verify("unexpected trace shape".into()));
    }
    let accepted = AcceptableOptions::OptionSet(vec![proof_options()]);
    std::panic::catch_unwind(|| {
        winterfell::verify::<SpendAir, H, Coin, VC>(proof, public.clone(), &accepted)
    })
    .map_err(|_| ZkError::Verify("verifier panicked".into()))?
    .map_err(|e| ZkError::Verify(e.to_string()))
}

#[cfg(test)]
mod tests;

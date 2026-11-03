//! Trace layout of the spend AIR. See `docs/spec/spend-circuit.md`.

use krypton_crypto::rescue::domain;

pub const DEPTH: usize = 40;
pub const CYCLE: usize = 8;
pub const ROUNDS: usize = 7;
pub const ACTIVE_ROWS: usize = DEPTH * CYCLE; // 320
pub const TRACE_LEN: usize = 512;
/// Rows after the active region filled with uniform randomness (zero knowledge).
pub const RANDOM_ROWS: usize = TRACE_LEN - ACTIVE_ROWS; // 192
pub const RANGE_BITS: usize = 60;
pub const MAX_VALUE: u64 = (1 << RANGE_BITS) - 1;

// Hash lanes: 12 state columns each.
pub const K_LANE: [usize; 2] = [0, 12];
pub const M_LANE: [usize; 2] = [24, 38];
pub const M_BIT: usize = 12;
pub const M_ACC: usize = 13;
pub const RA: usize = 52; // 4 range accumulators
pub const RB: usize = 56; // 4 range bits
pub const REG: [usize; 2] = [60, 104];
pub const REG_WIDTH: usize = 44;
pub const MASK: usize = 148;
pub const WIDTH: usize = 149;

// Register offsets inside a lane's register block.
pub const R_SK: usize = 0;
pub const R_AK: usize = 4;
pub const R_NK: usize = 8;
pub const R_OWNER: usize = 12;
pub const R_T: usize = 16;
pub const R_CM: usize = 20;
pub const R_RCM: usize = 24;
pub const R_V: usize = 28;
pub const R_POS: usize = 29;
pub const R_DUMMY: usize = 30;
pub const R_O_OWNER: usize = 31;
pub const R_O_T: usize = 35;
pub const R_O_RCM: usize = 39;
pub const R_O_V: usize = 43;

/// Range-checked values: (lane, register offset).
pub const RANGE_VALUES: [(usize, usize); 4] = [(0, R_V), (1, R_V), (0, R_O_V), (1, R_O_V)];

#[derive(Clone, Copy, Debug)]
pub enum Src {
    Zero,
    Reg(usize),
    Scalar(usize),
}

#[derive(Clone, Copy, Debug)]
pub enum Out {
    Reg(usize),
    Nullifier,
    OutputCm,
}

#[derive(Clone, Copy, Debug)]
pub struct CycleSpec {
    pub domain: u64,
    pub dummy_domain: bool,
    pub left: Src,
    pub right: Src,
    pub out: Out,
}

/// The fixed program executed by each key/note lane, one Rescue permutation per cycle.
pub const K_PROGRAM: [CycleSpec; 8] = [
    CycleSpec {
        domain: domain::AUTH_KEY,
        dummy_domain: false,
        left: Src::Reg(R_SK),
        right: Src::Zero,
        out: Out::Reg(R_AK),
    },
    CycleSpec {
        domain: domain::NULLIFIER_KEY,
        dummy_domain: false,
        left: Src::Reg(R_SK),
        right: Src::Zero,
        out: Out::Reg(R_NK),
    },
    CycleSpec {
        domain: domain::OWNER,
        dummy_domain: false,
        left: Src::Reg(R_AK),
        right: Src::Reg(R_NK),
        out: Out::Reg(R_OWNER),
    },
    CycleSpec {
        domain: domain::NOTE_BODY,
        dummy_domain: false,
        left: Src::Reg(R_OWNER),
        right: Src::Scalar(R_V),
        out: Out::Reg(R_T),
    },
    CycleSpec {
        domain: domain::NOTE_COMMIT,
        dummy_domain: false,
        left: Src::Reg(R_T),
        right: Src::Reg(R_RCM),
        out: Out::Reg(R_CM),
    },
    CycleSpec {
        domain: domain::NULLIFIER,
        dummy_domain: true,
        left: Src::Reg(R_NK),
        right: Src::Scalar(R_POS),
        out: Out::Nullifier,
    },
    CycleSpec {
        domain: domain::NOTE_BODY,
        dummy_domain: false,
        left: Src::Reg(R_O_OWNER),
        right: Src::Scalar(R_O_V),
        out: Out::Reg(R_O_T),
    },
    CycleSpec {
        domain: domain::NOTE_COMMIT,
        dummy_domain: false,
        left: Src::Reg(R_O_T),
        right: Src::Reg(R_O_RCM),
        out: Out::OutputCm,
    },
];

pub const NULLIFIER_ROW: usize = 5 * CYCLE + CYCLE - 1;
pub const OUTPUT_CM_ROW: usize = 7 * CYCLE + CYCLE - 1;

// Periodic column indices.
pub const P_ROUND: usize = 0;
pub const P_ARK1: usize = 1;
pub const P_ARK2: usize = 13;
pub const P_ACT: usize = 25;
pub const P_START: usize = 26;
pub const P_END: usize = 34;
pub const P_MB: usize = 42;
pub const P_P2N: usize = 43;
pub const P_MEND: usize = 44;
pub const P_RC: usize = 45;
pub const P_RCE: usize = 46;
pub const NUM_PERIODIC: usize = 47;

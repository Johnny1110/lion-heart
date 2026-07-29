//! Real passive tone stacks — the coupled RC networks a Fender or Marshall
//! actually has, in place of three independent filter bands.
//!
//! # Why this exists
//!
//! The old shared 3-band ([`super::super::drive`]'s `ToneStack`) summed three
//! *orthogonal* filters: `x + lo·low + mid·band + hi·high`. That is a graphic
//! EQ. A real FMV/TMB stack is one network in which Bass, Mid and Treble all
//! load the same nodes, so:
//!
//! - the three knobs **interact** — moving Mid changes the treble response;
//! - noon is **not flat**: the network has an intrinsic mid scoop (~9.5 dB on
//!   the Bassman, ~7.4 dB on the JCM800) and a real insertion loss.
//!
//! Those two facts are most of what "sounds like an amp" means.
//!
//! # How it works
//!
//! A passive tone stack is a *linear* network (R, C and pots only), so it does
//! not need the WDF machinery in [`crate::blocks::wdf`] — that exists for
//! nonlinear roots. Instead each model is a **netlist** ([`El`]), and the
//! engine turns it into a filter numerically:
//!
//! 1. **Netlist → continuous state space.** States are the capacitor voltages.
//!    Each capacitor is stamped into a modified-nodal-analysis system as a
//!    voltage source of its own state; solving that system once per basis
//!    vector of `[x₁..x_k, u]` reads off every capacitor current and the output
//!    voltage, which *is* `(A, B, C, D)`.
//! 2. **Tustin.** `A_d = P(I + ½TA)`, `B_d = T·P·B`, `C_d = C·P`,
//!    `D_d = D + ½T·C·P·B` with `P = (I − ½TA)⁻¹`.
//! 3. **Run the state space per sample** (≤ 4 states).
//!
//! Two properties fall out of this that a hand-derived direct-form IIR does not
//! get for free, and both matter here:
//!
//! - **Stability is structural.** A passive RC network has real negative poles,
//!   and Tustin maps those strictly inside the unit circle — at every knob
//!   position and every sample rate. No root finding, no cascade pairing.
//! - **Coefficient changes are physically continuous.** The state vector *is*
//!   the capacitor voltages, so rebuilding coefficients under a moving knob
//!   keeps the same physical state instead of reinterpreting an abstract
//!   filter memory. That is why block-rate rebuilds do not click.
//!
//! Everything from step 1 to step 2 runs in `f64` at the block boundary and
//! only when a knob has actually moved (the settled-skip [`super::chain`] uses);
//! the per-sample path is `f32`.
//!
//! # Adding a model
//!
//! Append a [`Kind`] to [`KINDS`]: a netlist, an output node, a per-pot taper
//! and a makeup gain. No engine work — that is the point. Component values are
//! facts read off a schematic; the derivation above is ours.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::blocks::smooth::Smoothed;

// --- netlist -----------------------------------------------------------------

/// Ground. Every netlist reserves node 0 for it.
pub const GND: u8 = 0;
/// The driven input node. Every netlist reserves node 1 for it.
pub const IN: u8 = 1;

/// Faceplate knobs an FMV stack has, in `eq()` order.
pub const BASS: u8 = 0;
pub const MID: u8 = 1;
pub const TREBLE: u8 = 2;

pub const MAX_KNOBS: usize = 3;
const MAX_NODES: usize = 8;
const MAX_CAPS: usize = 4;
/// Voltage-source branch currents: one per capacitor, plus the input source.
const MAX_SRC: usize = MAX_CAPS + 1;
const MNA_DIM: usize = (MAX_NODES - 1) + MAX_SRC;

/// Wiper contact resistance. A pot section never reaches a true short in the
/// real world, and a zero-ohm branch would make the nodal system singular.
const WIPER_OHMS: f64 = 1.0;

/// One netlist element. Nodes are indices; [`GND`] and [`IN`] are reserved.
#[derive(Clone, Copy)]
pub enum El {
    Res {
        a: u8,
        b: u8,
        ohms: f32,
    },
    /// One side of a potentiometer: `ohms · f` where `f` is the knob's tapered
    /// fraction, or `ohms · (1 − f)` when `upper`. A pot wired as a rheostat
    /// (bass and mid in an FMV stack) is a single section; a true pot (treble)
    /// is two sections meeting at the wiper node.
    Pot {
        a: u8,
        b: u8,
        ohms: f32,
        knob: u8,
        upper: bool,
    },
    Cap {
        a: u8,
        b: u8,
        farads: f32,
    },
}

/// How a pot's rotation maps to its resistance fraction.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Taper {
    Linear,
    /// The standard audio law: 10 % of the track at half rotation.
    Audio,
    /// Audio taper measured from the other end.
    ReverseAudio,
}

/// `f(½) = 0.1`, `f(1) = 1` for [`Taper::Audio`].
const AUDIO_BASE: f32 = 81.0;

impl Taper {
    /// Knob position `0..=10` → resistance fraction `0..=1`.
    pub fn fraction(self, pos: f32) -> f32 {
        let x = (pos * 0.1).clamp(0.0, 1.0);
        match self {
            Taper::Linear => x,
            Taper::Audio => (AUDIO_BASE.powf(x) - 1.0) / (AUDIO_BASE - 1.0),
            Taper::ReverseAudio => 1.0 - (AUDIO_BASE.powf(1.0 - x) - 1.0) / (AUDIO_BASE - 1.0),
        }
    }
}

/// One tone stack model: a netlist plus the calibration that makes it usable.
pub struct Kind {
    pub key: &'static str,
    pub name: &'static str,
    /// Node count including ground, i.e. the highest node index + 1.
    pub nodes: usize,
    pub net: &'static [El],
    /// Node the next stage listens to (the treble wiper on an FMV stack).
    pub out: u8,
    pub tapers: [Taper; MAX_KNOBS],
    /// Which faceplate knobs this model actually wires up, as a bit per knob.
    /// The Big Muff has one control; the FMV stacks have three.
    pub knob_mask: u8,
    /// Restores the network's noon *maximum* (its low shelf) to unity: the
    /// largest gain the netlist reaches at noon over 15 Hz–16 kHz, negated.
    /// The noon response is therefore ≤ 0 dB everywhere, like the passive
    /// network itself — what remains below the ceiling is the scoop.
    ///
    /// Never calibrate this to a band average. The first calibration did
    /// (average over 80 Hz–7.2 kHz), and averaging across a 7–9 dB scoop
    /// lifts the shelves *above* unity — every FMV-voiced drive gained a
    /// measured +4–5 dB of absolute low end against its pre-migration self,
    /// which is a bass boost no passive stack can produce (ADR 037).
    pub makeup_db: f32,
}

impl Kind {
    pub fn uses_knob(&self, knob: usize) -> bool {
        self.knob_mask & (1 << knob) != 0
    }
}

// --- the models --------------------------------------------------------------

// FMV node names: A is the treble cap / treble pot top, B the slope resistor's
// bottom, C the treble pot bottom and bass pot top, D the bass pot bottom and
// mid pot top, OUT the treble wiper.
const FMV_A: u8 = 2;
const FMV_B: u8 = 3;
const FMV_C: u8 = 4;
const FMV_D: u8 = 5;
const FMV_OUT: u8 = 6;

/// The Fender/Marshall/Vox stack: one topology, one set of component values
/// per amp. `r1` is the slope resistor, `r2..r4` the treble/bass/mid pots,
/// `c1..c3` the treble/bass/mid caps.
const fn fmv(r1: f32, r2: f32, r3: f32, r4: f32, c1: f32, c2: f32, c3: f32) -> [El; 8] {
    [
        El::Cap {
            a: IN,
            b: FMV_A,
            farads: c1,
        },
        El::Res {
            a: IN,
            b: FMV_B,
            ohms: r1,
        },
        El::Pot {
            a: FMV_A,
            b: FMV_OUT,
            ohms: r2,
            knob: TREBLE,
            upper: true,
        },
        El::Pot {
            a: FMV_OUT,
            b: FMV_C,
            ohms: r2,
            knob: TREBLE,
            upper: false,
        },
        El::Cap {
            a: FMV_B,
            b: FMV_C,
            farads: c2,
        },
        El::Pot {
            a: FMV_C,
            b: FMV_D,
            ohms: r3,
            knob: BASS,
            upper: false,
        },
        El::Cap {
            a: FMV_B,
            b: FMV_D,
            farads: c3,
        },
        El::Pot {
            a: FMV_D,
            b: GND,
            ohms: r4,
            knob: MID,
            upper: false,
        },
    ]
}

/// Fender 5F6-A Bassman — the stack every Marshall descends from.
static BASSMAN_NET: [El; 8] = fmv(56e3, 250e3, 1e6, 25e3, 250e-12, 20e-9, 20e-9);
/// Marshall 2203/2204: a shallower slope resistor and a bigger treble cap put
/// the scoop higher and shallower — brighter, more mid-forward than a Fender.
static JCM800_NET: [El; 8] = fmv(33e3, 220e3, 1e6, 22e3, 470e-12, 22e-9, 22e-9);

// Big Muff tone: a lowpass and a highpass with matched ~400 Hz corners, blended
// by a single pot. The notch sits at the crossover and *moves* with the knob —
// the opposite of a wah, and nothing like a shelf pair.
const MUFF_LP: u8 = 2;
const MUFF_HP: u8 = 3;
const MUFF_OUT: u8 = 4;

static BIG_MUFF_NET: [El; 6] = [
    El::Res {
        a: IN,
        b: MUFF_LP,
        ohms: 39e3,
    },
    El::Cap {
        a: MUFF_LP,
        b: GND,
        farads: 10e-9,
    },
    El::Cap {
        a: IN,
        b: MUFF_HP,
        farads: 3.9e-9,
    },
    El::Res {
        a: MUFF_HP,
        b: GND,
        ohms: 100e3,
    },
    El::Pot {
        a: MUFF_LP,
        b: MUFF_OUT,
        ohms: 100e3,
        knob: TREBLE,
        upper: false,
    },
    El::Pot {
        a: MUFF_OUT,
        b: MUFF_HP,
        ohms: 100e3,
        knob: TREBLE,
        upper: true,
    },
];

pub const KIND_COUNT: usize = 3;

/// Registry indices by name, for the drives that pick a model in code.
/// Pinned to [`KINDS`] by a test.
pub mod kind {
    pub const BASSMAN: usize = 0;
    pub const JCM800: usize = 1;
    pub const BIG_MUFF: usize = 2;
}

/// The model registry, in menu order. **Append-only** — the standalone pedal
/// stores the index in presets.
///
/// Tapers are `Linear` throughout: the published response curves these models
/// are calibrated against (and the noon scoop the acceptance tests pin) are
/// measured at half rotation of a linear track, and real units vary by era and
/// by which end the taper is measured from. [`Taper::Audio`] is implemented and
/// tested, so re-voicing a model is a one-field change once an ear says so.
pub static KINDS: [Kind; KIND_COUNT] = [
    Kind {
        key: "bassman",
        name: "Bassman",
        nodes: 7,
        net: &BASSMAN_NET,
        out: FMV_OUT,
        tapers: [Taper::Linear; MAX_KNOBS],
        knob_mask: 0b111,
        makeup_db: 1.63,
    },
    Kind {
        key: "jcm800",
        name: "JCM800",
        nodes: 7,
        net: &JCM800_NET,
        out: FMV_OUT,
        tapers: [Taper::Linear; MAX_KNOBS],
        knob_mask: 0b111,
        makeup_db: 0.98,
    },
    Kind {
        key: "big-muff",
        name: "Big Muff",
        nodes: 5,
        net: &BIG_MUFF_NET,
        out: MUFF_OUT,
        tapers: [Taper::Linear; MAX_KNOBS],
        knob_mask: 1 << TREBLE,
        makeup_db: 4.05,
    },
];

// --- the engine --------------------------------------------------------------

/// One tone stack: a model, its discrete state space, and two channels of
/// capacitor voltages.
///
/// Knob positions are pedal positions `0..=10`. Feed it already-smoothed
/// values (the drive family's trajectories, or the pedal's own [`Smoothed`]);
/// coefficients rebuild at the block boundary, never per sample.
pub struct ToneStack {
    sample_rate: f32,
    kind: usize,
    knobs: [f32; MAX_KNOBS],
    /// Knob positions the current coefficients were built from.
    applied: [f32; MAX_KNOBS],
    dirty: bool,
    ncaps: usize,
    ad: [[f32; MAX_CAPS]; MAX_CAPS],
    bd: [f32; MAX_CAPS],
    cd: [f32; MAX_CAPS],
    dd: f32,
    x: [[f32; MAX_CAPS]; 2],
}

impl ToneStack {
    pub fn new(kind: usize) -> Self {
        let mut me = Self {
            sample_rate: 48_000.0,
            kind: kind.min(KIND_COUNT - 1),
            knobs: [5.0; MAX_KNOBS],
            applied: [f32::NAN; MAX_KNOBS],
            dirty: true,
            ncaps: 0,
            ad: [[0.0; MAX_CAPS]; MAX_CAPS],
            bd: [0.0; MAX_CAPS],
            cd: [0.0; MAX_CAPS],
            dd: 1.0,
            x: [[0.0; MAX_CAPS]; 2],
        };
        me.rebuild();
        me
    }

    pub fn kind(&self) -> &'static Kind {
        &KINDS[self.kind]
    }

    pub fn prepare(&mut self, sample_rate: f32) {
        self.sample_rate = sample_rate;
        self.dirty = true;
        self.rebuild();
        self.reset();
    }

    pub fn reset(&mut self) {
        self.x = [[0.0; MAX_CAPS]; 2];
    }

    /// Switch models. The states are the *old* network's capacitor voltages, so
    /// they are cleared: a model switch is a structural change, like swapping
    /// pedals, not a knob move.
    pub fn set_kind(&mut self, kind: usize) {
        let kind = kind.min(KIND_COUNT - 1);
        if kind != self.kind {
            self.kind = kind;
            self.dirty = true;
            self.reset();
        }
    }

    pub fn kind_index(&self) -> usize {
        self.kind
    }

    /// Knob positions `0..=10`, in [`BASS`]/[`MID`]/[`TREBLE`] order.
    pub fn set_knobs(&mut self, knobs: [f32; MAX_KNOBS]) {
        self.knobs = knobs;
    }

    /// Rebuild coefficients if a knob has moved since the last one. Settled
    /// knobs cost a three-float comparison and nothing else.
    fn update_coeffs(&mut self) {
        if !self.dirty && self.knobs == self.applied {
            return;
        }
        self.rebuild();
    }

    pub fn process_mono(&mut self, block: &mut [f32]) {
        self.update_coeffs();
        let (n, ad, bd, cd, dd) = (self.ncaps, self.ad, self.bd, self.cd, self.dd);
        let st = &mut self.x[0];
        for s in block.iter_mut() {
            *s = step(st, n, &ad, &bd, &cd, dd, *s);
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        self.update_coeffs();
        let (n, ad, bd, cd, dd) = (self.ncaps, self.ad, self.bd, self.cd, self.dd);
        let [sl, sr] = &mut self.x;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            *l = step(sl, n, &ad, &bd, &cd, dd, *l);
            *r = step(sr, n, &ad, &bd, &cd, dd, *r);
        }
    }
}

/// One sample of the discrete state space. Output first (the `D` term makes it
/// non-strictly-proper — a passive stack passes signal through instantly at
/// high frequency), then the state update, then a denormal flush so a decaying
/// tail cannot stall the FPU.
#[inline]
fn step(
    x: &mut [f32; MAX_CAPS],
    n: usize,
    ad: &[[f32; MAX_CAPS]; MAX_CAPS],
    bd: &[f32; MAX_CAPS],
    cd: &[f32; MAX_CAPS],
    dd: f32,
    u: f32,
) -> f32 {
    let mut y = dd * u;
    for i in 0..n {
        y += cd[i] * x[i];
    }
    let mut next = [0.0f32; MAX_CAPS];
    for i in 0..n {
        let mut acc = bd[i] * u;
        for j in 0..n {
            acc += ad[i][j] * x[j];
        }
        next[i] = if acc.abs() < 1e-20 { 0.0 } else { acc };
    }
    *x = next;
    y
}

// --- netlist → discrete state space -------------------------------------------

impl ToneStack {
    /// Solve the netlist and discretise it. Runs at the block boundary only,
    /// in `f64`: the nodal system mixes conductances five decades apart, and
    /// the Tustin inverse afterwards would lose the low corner in `f32`.
    ///
    /// Allocation-free — every working array is a fixed-size stack array sized
    /// by [`MNA_DIM`], which is why the netlists are bounded.
    fn rebuild(&mut self) {
        self.dirty = false;
        self.applied = self.knobs;

        let kind = &KINDS[self.kind];
        debug_assert!(kind.nodes <= MAX_NODES);

        // Node voltages (ground excluded) come first, then one branch current
        // per voltage source: the input source, then the capacitors in netlist
        // order. Each capacitor is stamped as a source holding its own state,
        // so the solution reads back every capacitor current directly.
        let nv = kind.nodes - 1;
        let ncaps = kind
            .net
            .iter()
            .filter(|e| matches!(e, El::Cap { .. }))
            .count();
        debug_assert!(ncaps <= MAX_CAPS && ncaps > 0);
        let dim = nv + 1 + ncaps;
        debug_assert!(dim <= MNA_DIM);

        let mut m = [[0.0f64; MNA_DIM]; MNA_DIM];
        // One right-hand side per basis vector of `[x₁..x_k, u]`: column `i`
        // charges capacitor `i` to 1 V with the input grounded, column `ncaps`
        // drives the input to 1 V with every capacitor discharged.
        let mut rhs = [[0.0f64; MAX_SRC]; MNA_DIM];

        let stamp = |m: &mut [[f64; MNA_DIM]; MNA_DIM], a: u8, b: u8, y: f64| {
            for (p, q) in [(a, b), (b, a)] {
                if p == GND {
                    continue;
                }
                let pi = p as usize - 1;
                m[pi][pi] += y;
                if q != GND {
                    m[pi][q as usize - 1] -= y;
                }
            }
        };

        for el in kind.net {
            match *el {
                El::Res { a, b, ohms } => stamp(&mut m, a, b, 1.0 / ohms as f64),
                El::Pot {
                    a,
                    b,
                    ohms,
                    knob,
                    upper,
                } => {
                    let f = kind.tapers[knob as usize].fraction(self.knobs[knob as usize]);
                    let f = if upper { 1.0 - f } else { f };
                    let r = (ohms as f64 * f as f64).max(WIPER_OHMS);
                    stamp(&mut m, a, b, 1.0 / r);
                }
                El::Cap { .. } => {}
            }
        }

        // Voltage-source rows: the branch current leaves node `a` and enters
        // node `b`, and the constraint row reads `v(a) − v(b) = value`.
        let source = |m: &mut [[f64; MNA_DIM]; MNA_DIM],
                      rhs: &mut [[f64; MAX_SRC]; MNA_DIM],
                      row: usize,
                      a: u8,
                      b: u8,
                      col: usize| {
            for (node, sign) in [(a, 1.0), (b, -1.0)] {
                if node != GND {
                    let i = node as usize - 1;
                    m[i][row] += sign;
                    m[row][i] += sign;
                }
            }
            rhs[row][col] = 1.0;
        };
        source(&mut m, &mut rhs, nv, IN, GND, ncaps);
        let mut cap = 0;
        for el in kind.net {
            if let El::Cap { a, b, .. } = *el {
                source(&mut m, &mut rhs, nv + 1 + cap, a, b, cap);
                cap += 1;
            }
        }

        if !solve(&mut m, &mut rhs, dim, ncaps + 1) {
            // A netlist that cannot be solved must not reach the audio as NaN.
            // Unreachable for the shipped models (a test sweeps every knob of
            // every kind); this is the belt for a future one.
            debug_assert!(false, "singular tone stack netlist");
            self.ncaps = 0;
            self.dd = 1.0;
            return;
        }

        // (A, B, C, D): capacitor currents divided by capacitance are the state
        // derivatives, and the output node voltage is the readout.
        let out = kind.out as usize;
        let mut a_c = [[0.0f64; MAX_CAPS]; MAX_CAPS];
        let mut b_c = [0.0f64; MAX_CAPS];
        let mut c_c = [0.0f64; MAX_CAPS];
        let mut d_c = 0.0f64;
        let farads: [f64; MAX_CAPS] = {
            let mut f = [1.0; MAX_CAPS];
            let mut i = 0;
            for el in kind.net {
                if let El::Cap { farads, .. } = *el {
                    f[i] = farads as f64;
                    i += 1;
                }
            }
            f
        };
        for col in 0..=ncaps {
            let v_out = if out == GND as usize {
                0.0
            } else {
                rhs[out - 1][col]
            };
            if col == ncaps {
                d_c = v_out;
            } else {
                c_c[col] = v_out;
            }
            for i in 0..ncaps {
                let current = rhs[nv + 1 + i][col] / farads[i];
                if col == ncaps {
                    b_c[i] = current;
                } else {
                    a_c[i][col] = current;
                }
            }
        }

        self.ncaps = ncaps;
        self.tustin(&a_c, &b_c, &c_c, d_c, kind.makeup_db);
    }

    /// Bilinear transform of the continuous state space, with the model's
    /// makeup gain folded into the readout so the per-sample path stays bare.
    // Dense linear algebra: pivoting and row updates are index
    // arithmetic, and the iterator forms of these loops read worse than
    // the textbook they come from.
    #[allow(clippy::needless_range_loop)]
    fn tustin(
        &mut self,
        a: &[[f64; MAX_CAPS]; MAX_CAPS],
        b: &[f64; MAX_CAPS],
        c: &[f64; MAX_CAPS],
        d: f64,
        makeup_db: f32,
    ) {
        let n = self.ncaps;
        let t = 1.0 / self.sample_rate as f64;
        let h = 0.5 * t;

        // P = (I − ½TA)⁻¹ by Gauss-Jordan. Never singular: A's eigenvalues have
        // negative real parts (the network is passive), so those of I − ½TA sit
        // to the right of 1.
        let mut aug = [[0.0f64; 2 * MAX_CAPS]; MAX_CAPS];
        for i in 0..n {
            for j in 0..n {
                aug[i][j] = if i == j { 1.0 } else { 0.0 } - h * a[i][j];
            }
            aug[i][n + i] = 1.0;
        }
        for i in 0..n {
            let mut piv = i;
            for r in i + 1..n {
                if aug[r][i].abs() > aug[piv][i].abs() {
                    piv = r;
                }
            }
            aug.swap(i, piv);
            let d = aug[i][i];
            for v in aug[i][..2 * n].iter_mut() {
                *v /= d;
            }
            for r in 0..n {
                if r != i && aug[r][i] != 0.0 {
                    let f = aug[r][i];
                    for k in 0..2 * n {
                        aug[r][k] -= f * aug[i][k];
                    }
                }
            }
        }
        let mut p = [[0.0f64; MAX_CAPS]; MAX_CAPS];
        for i in 0..n {
            for j in 0..n {
                p[i][j] = aug[i][n + j];
            }
        }

        let makeup = db_to_lin(makeup_db) as f64;
        // P·B, reused by both B_d and the D_d correction.
        let mut pb = [0.0f64; MAX_CAPS];
        for i in 0..n {
            pb[i] = (0..n).map(|k| p[i][k] * b[k]).sum();
        }
        for i in 0..n {
            self.bd[i] = (t * pb[i]) as f32;
            for j in 0..n {
                let ij = (0..n)
                    .map(|k| p[i][k] * (if k == j { 1.0 } else { 0.0 } + h * a[k][j]))
                    .sum::<f64>();
                self.ad[i][j] = ij as f32;
            }
            self.cd[i] = (makeup * (0..n).map(|k| c[k] * p[k][i]).sum::<f64>()) as f32;
        }
        let cpb: f64 = (0..n).map(|k| c[k] * pb[k]).sum();
        self.dd = (makeup * (d + h * cpb)) as f32;
    }
}

/// Gaussian elimination with partial pivoting over `cols` right-hand sides,
/// in place. `false` if the system is singular.
// Dense linear algebra: pivoting and row updates are index arithmetic, and
// the iterator forms of these loops read worse than the textbook they come
// from.
#[allow(clippy::needless_range_loop)]
fn solve(
    m: &mut [[f64; MNA_DIM]; MNA_DIM],
    rhs: &mut [[f64; MAX_SRC]; MNA_DIM],
    n: usize,
    cols: usize,
) -> bool {
    for i in 0..n {
        let mut piv = i;
        for r in i + 1..n {
            if m[r][i].abs() > m[piv][i].abs() {
                piv = r;
            }
        }
        if m[piv][i].abs() < 1e-30 {
            return false;
        }
        m.swap(i, piv);
        rhs.swap(i, piv);
        for r in i + 1..n {
            let f = m[r][i] / m[i][i];
            if f == 0.0 {
                continue;
            }
            for c in i..n {
                m[r][c] -= f * m[i][c];
            }
            for c in 0..cols {
                rhs[r][c] -= f * rhs[i][c];
            }
        }
    }
    for i in (0..n).rev() {
        for c in 0..cols {
            let mut acc = rhs[i][c];
            for j in i + 1..n {
                acc -= m[i][j] * rhs[j][c];
            }
            rhs[i][c] = acc / m[i][i];
        }
    }
    true
}

// --- the pedal ---------------------------------------------------------------

static MODEL_LABELS: [&str; KIND_COUNT] = ["Bassman", "JCM800", "Big Muff"];

static PARAMS: [ParamDesc; 5] = [
    ParamDesc {
        key: "model",
        name: "Model",
        unit: "",
        range: Range::Stepped {
            labels: &MODEL_LABELS,
        },
        default: 0.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "bass",
        name: "Bass",
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 10.0,
        },
        default: 5.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "mid",
        name: "Mid",
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 10.0,
        },
        default: 5.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "treble",
        name: "Treble",
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 10.0,
        },
        default: 5.0,
        smoothing_ms: 30.0,
    },
    ParamDesc {
        key: "level",
        name: "Level",
        unit: "dB",
        range: Range::Linear {
            min: -12.0,
            max: 12.0,
        },
        default: 0.0,
        smoothing_ms: 20.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "tonestack",
    name: "Tone Stack",
    params: &PARAMS,
};

/// The standalone tone stack pedal: an amp's tone network anywhere in the
/// chain, in front of a drive or behind it.
///
/// The Big Muff model has one control on the real pedal — its blend is wired
/// to Treble, and Bass/Mid do nothing there ([`Kind::uses_knob`] says which).
pub struct Stack {
    core: ToneStack,
    knobs: [Smoothed; MAX_KNOBS],
    level: Smoothed,
}

impl Default for Stack {
    fn default() -> Self {
        Self::new()
    }
}

impl Stack {
    pub fn new() -> Self {
        Self {
            core: ToneStack::new(0),
            knobs: [
                Smoothed::new(PARAMS[1].default),
                Smoothed::new(PARAMS[2].default),
                Smoothed::new(PARAMS[3].default),
            ],
            level: Smoothed::new(PARAMS[4].default),
        }
    }

    pub fn prepare(&mut self, sample_rate: u32) {
        for (s, desc) in self.knobs.iter_mut().zip(&PARAMS[1..4]) {
            s.configure(desc.smoothing_ms, sample_rate);
            s.snap_to_target();
        }
        self.level.configure(PARAMS[4].smoothing_ms, sample_rate);
        self.level.snap_to_target();
        self.core.prepare(sample_rate as f32);
    }

    pub fn reset(&mut self) {
        self.core.reset();
    }

    pub fn set_param(&mut self, index: usize, normalized: f32) {
        let Some(param) = PARAMS.get(index) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match index {
            0 => self.core.set_kind(real as usize),
            1..=3 => self.knobs[index - 1].set_target(real),
            4 => self.level.set_target(real),
            _ => {}
        }
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        // Advance the tone smoothers across the block and rebuild once from
        // where they land — the same block-rate contract `eq::chain` uses, and
        // safe here because the network's state is physical (see the module
        // docs).
        for _ in 0..left.len() {
            for s in &mut self.knobs {
                s.tick();
            }
        }
        self.core.set_knobs([
            self.knobs[0].current(),
            self.knobs[1].current(),
            self.knobs[2].current(),
        ]);
        self.core.process(left, right);
        // Level is a plain gain, so it rides its smoother per sample — no
        // reason to make it step at the block boundary.
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let gain = db_to_lin(self.level.tick());
            *l *= gain;
            *r *= gain;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, rms, silence, sine};
    use lh_core::lin_to_db;

    const SR: f32 = 48_000.0;
    /// The band the makeup gain and the scoop measurements are taken over.
    const BAND: [f32; 14] = [
        80.0, 110.0, 160.0, 220.0, 320.0, 450.0, 640.0, 900.0, 1300.0, 1800.0, 2600.0, 3600.0,
        5100.0, 7200.0,
    ];

    // --- an independent oracle -----------------------------------------------
    //
    // ngspice is not a build dependency, so the golden reference is a direct
    // complex nodal analysis of the same netlist, written from the other end:
    // capacitors stamped as admittances `sC` and the input node moved to the
    // right-hand side as a known voltage. It shares no code with the
    // state-space extraction under test — if the MNA stamping, the source
    // signs, the capacitor-current readout or the Tustin algebra were wrong,
    // the two would disagree.

    #[derive(Clone, Copy)]
    struct C64(f64, f64);

    impl C64 {
        fn mul(self, o: C64) -> C64 {
            C64(self.0 * o.0 - self.1 * o.1, self.0 * o.1 + self.1 * o.0)
        }
        fn div(self, o: C64) -> C64 {
            let d = o.0 * o.0 + o.1 * o.1;
            C64(
                (self.0 * o.0 + self.1 * o.1) / d,
                (self.1 * o.0 - self.0 * o.1) / d,
            )
        }
        fn sub(self, o: C64) -> C64 {
            C64(self.0 - o.0, self.1 - o.1)
        }
        fn abs(self) -> f64 {
            self.0.hypot(self.1)
        }
    }

    /// |H(jω)| of `kind` at `freq`, by nodal analysis. Unknowns are the
    /// non-ground, non-input node voltages; the 1 V input contributes to the
    /// right-hand side through whatever it touches.
    // Dense linear algebra: pivoting and row updates are index
    // arithmetic, and the iterator forms of these loops read worse than
    // the textbook they come from.
    #[allow(clippy::needless_range_loop)]
    fn nodal_db(kind: &Kind, knobs: [f32; MAX_KNOBS], freq: f32) -> f64 {
        let w = 2.0 * std::f64::consts::PI * freq as f64;
        let n = kind.nodes - 2; // exclude ground and the input node
        let idx =
            |node: u8| -> Option<usize> { (node != GND && node != IN).then(|| node as usize - 2) };
        let mut y = vec![vec![C64(0.0, 0.0); n]; n];
        let mut b = vec![C64(0.0, 0.0); n];
        for el in kind.net {
            let (a, c, adm) = match *el {
                El::Res { a, b, ohms } => (a, b, C64(1.0 / ohms as f64, 0.0)),
                El::Pot {
                    a,
                    b,
                    ohms,
                    knob,
                    upper,
                } => {
                    let f = kind.tapers[knob as usize].fraction(knobs[knob as usize]);
                    let f = if upper { 1.0 - f } else { f };
                    let r = (ohms as f64 * f as f64).max(WIPER_OHMS);
                    (a, b, C64(1.0 / r, 0.0))
                }
                El::Cap { a, b, farads } => (a, b, C64(0.0, w * farads as f64)),
            };
            for (p, q) in [(a, c), (c, a)] {
                if let Some(pi) = idx(p) {
                    y[pi][pi] = C64(y[pi][pi].0 + adm.0, y[pi][pi].1 + adm.1);
                    match idx(q) {
                        Some(qi) => y[pi][qi] = y[pi][qi].sub(adm),
                        None if q == IN => b[pi] = C64(b[pi].0 + adm.0, b[pi].1 + adm.1),
                        None => {}
                    }
                }
            }
        }
        for i in 0..n {
            let mut piv = i;
            for r in i + 1..n {
                if y[r][i].abs() > y[piv][i].abs() {
                    piv = r;
                }
            }
            y.swap(i, piv);
            b.swap(i, piv);
            for r in i + 1..n {
                let f = y[r][i].div(y[i][i]);
                for c in i..n {
                    y[r][c] = y[r][c].sub(f.mul(y[i][c]));
                }
                b[r] = b[r].sub(f.mul(b[i]));
            }
        }
        let mut x = vec![C64(0.0, 0.0); n];
        for i in (0..n).rev() {
            let mut acc = b[i];
            for j in i + 1..n {
                acc = acc.sub(y[i][j].mul(x[j]));
            }
            x[i] = acc.div(y[i][i]);
        }
        20.0 * x[kind.out as usize - 2].abs().log10()
    }

    // --- helpers -------------------------------------------------------------

    fn prepared(kind: usize, knobs: [f32; MAX_KNOBS]) -> ToneStack {
        let mut ts = ToneStack::new(kind);
        ts.prepare(SR);
        ts.set_knobs(knobs);
        ts
    }

    /// Measured steady-state gain in dB at `freq`, through the audio path.
    fn rendered_db(ts: &mut ToneStack, freq: f32) -> f32 {
        ts.reset();
        let x = sine(SR as u32, freq, SR as usize / 2);
        let mut y = x.clone();
        for blk in y.chunks_mut(64) {
            ts.process_mono(blk);
        }
        assert_finite("tonestack output", &y);
        let n = y.len();
        lin_to_db(rms(&y[n / 2..]) / rms(&x[n / 2..]))
    }

    /// The model's own makeup gain, so measurements can be compared against
    /// the raw network the oracle computes.
    fn makeup(kind: usize) -> f64 {
        KINDS[kind].makeup_db as f64
    }

    // --- coefficient correctness --------------------------------------------

    /// The golden test: the discretised filter must match an independent AC
    /// analysis of the same netlist, at every model and a spread of knob
    /// settings including the extremes. Tolerance grows with frequency because
    /// the bilinear transform genuinely warps — 0.5 dB by 8 kHz at 48 kHz.
    #[test]
    fn discrete_response_matches_the_nodal_oracle() {
        let knob_sets: [[f32; MAX_KNOBS]; 7] = [
            [5.0, 5.0, 5.0],
            [10.0, 5.0, 5.0],
            [0.0, 5.0, 5.0],
            [5.0, 10.0, 5.0],
            [5.0, 0.0, 5.0],
            [5.0, 5.0, 10.0],
            [2.0, 7.0, 8.0],
        ];
        for (kind, spec) in KINDS.iter().enumerate() {
            for knobs in knob_sets {
                let mut ts = prepared(kind, knobs);
                for freq in BAND {
                    let want = nodal_db(&KINDS[kind], knobs, freq) + makeup(kind);
                    let got = rendered_db(&mut ts, freq) as f64;
                    let tol = 0.15 + 0.45 * (freq / 8_000.0) as f64;
                    assert!(
                        (got - want).abs() < tol,
                        "{} at {freq} Hz knobs {knobs:?}: oracle {want:.3} dB, \
                         rendered {got:.3} dB (tol {tol:.2})",
                        spec.key
                    );
                }
            }
        }
    }

    /// Everything above 0/10 on a pot is interpolation; the ends are where a
    /// zero-ohm section would make the nodal system singular. Sweep all of it.
    #[test]
    fn every_knob_extreme_stays_bounded_and_finite() {
        for (kind, spec) in KINDS.iter().enumerate() {
            for b in [0.0f32, 5.0, 10.0] {
                for m in [0.0f32, 5.0, 10.0] {
                    for t in [0.0f32, 5.0, 10.0] {
                        let mut ts = prepared(kind, [b, m, t]);
                        let x = sine(SR as u32, 440.0, 4_096);
                        let mut y = x.clone();
                        for blk in y.chunks_mut(64) {
                            ts.process_mono(blk);
                        }
                        assert_finite(&format!("{} {b}/{m}/{t}", spec.key), &y);
                        assert!(
                            peak(&y) < 4.0,
                            "{} at {b}/{m}/{t} peaked {}",
                            spec.key,
                            peak(&y)
                        );
                    }
                }
            }
        }
    }

    /// A passive RC network has real negative poles, so Tustin must land every
    /// one strictly inside the unit circle. Checked through the characteristic
    /// polynomial of `A_d` with the Jury criterion rather than a root finder.
    #[test]
    fn poles_stay_inside_the_unit_circle_at_every_rate() {
        for rate in [44_100.0f32, 48_000.0, 96_000.0] {
            for (kind, spec) in KINDS.iter().enumerate() {
                for b in [0.0f32, 3.0, 5.0, 7.0, 10.0] {
                    for m in [0.0f32, 5.0, 10.0] {
                        for t in [0.0f32, 5.0, 10.0] {
                            let mut ts = ToneStack::new(kind);
                            ts.prepare(rate);
                            ts.set_knobs([b, m, t]);
                            ts.process_mono(&mut [0.0; 4]);
                            assert!(
                                spectral_radius_below_one(&ts),
                                "{} at {rate} Hz {b}/{m}/{t} has a pole on or \
                                 outside the unit circle",
                                spec.key
                            );
                        }
                    }
                }
            }
        }
    }

    /// Power iteration on `A_d`: after enough steps a matrix with spectral
    /// radius ≥ 1 grows without bound, one below 1 decays to nothing.
    // Dense linear algebra: pivoting and row updates are index
    // arithmetic, and the iterator forms of these loops read worse than
    // the textbook they come from.
    #[allow(clippy::needless_range_loop)]
    fn spectral_radius_below_one(ts: &ToneStack) -> bool {
        let n = ts.ncaps;
        let mut v = [1.0f64; MAX_CAPS];
        for _ in 0..4_000 {
            let mut next = [0.0f64; MAX_CAPS];
            for i in 0..n {
                for j in 0..n {
                    next[i] += ts.ad[i][j] as f64 * v[j];
                }
            }
            let norm = next[..n].iter().fold(0.0f64, |a, b| a.max(b.abs()));
            if norm > 1e6 {
                return false;
            }
            if norm < 1e-30 {
                return true;
            }
            for x in next[..n].iter_mut() {
                *x /= norm;
            }
            v = next;
        }
        // Converged without blowing up: re-measure the growth of one step.
        let mut next = [0.0f64; MAX_CAPS];
        for i in 0..n {
            for j in 0..n {
                next[i] += ts.ad[i][j] as f64 * v[j];
            }
        }
        next[..n].iter().fold(0.0f64, |a, b| a.max(b.abs())) < 1.0
    }

    // --- the two things a real stack does that a 3-band does not -------------

    /// Knob interaction: the three controls share the same nodes, so moving
    /// Mid must move the treble response. The old additive 3-band could not
    /// do this — its bands were orthogonal by construction.
    #[test]
    fn the_mid_knob_moves_the_treble_response() {
        for kind in [kind::BASSMAN, kind::JCM800] {
            let at = |m: f32| {
                let mut ts = prepared(kind, [5.0, m, 5.0]);
                rendered_db(&mut ts, 6_400.0)
            };
            let (lo, hi) = (at(0.0), at(10.0));
            assert!(
                hi - lo > 2.0,
                "{}: sweeping Mid moved 6.4 kHz by only {:.2} dB — the network \
                 is not coupled",
                KINDS[kind].key,
                hi - lo
            );
        }
        // ... and the same coupling the other way: Bass moves the low corner
        // enough that Treble's own band shifts too.
        let at = |l: f32| {
            let mut ts = prepared(0, [l, 5.0, 5.0]);
            rendered_db(&mut ts, 160.0)
        };
        assert!(at(10.0) - at(0.0) > 8.0);
    }

    /// The signature mid scoop: at noon an FMV stack dips several dB around
    /// 400–800 Hz relative to 100 Hz and 3.2 kHz. The Bassman scoops deeper
    /// than the JCM800 — the whole reason a Marshall sounds more mid-forward
    /// than a tweed Fender — and the Big Muff, whose notch *is* the circuit,
    /// deeper still.
    #[test]
    fn noon_has_the_signature_mid_scoop() {
        let depth = |kind: usize| {
            let mut ts = prepared(kind, [5.0, 5.0, 5.0]);
            let shoulders = rendered_db(&mut ts, 100.0).max(rendered_db(&mut ts, 3_200.0));
            let dip = rendered_db(&mut ts, 400.0).min(rendered_db(&mut ts, 800.0));
            shoulders - dip
        };
        let bassman = depth(0);
        let jcm800 = depth(1);
        assert!(
            bassman > 8.0,
            "bassman noon scoop is only {bassman:.2} dB — a real FMV stack is \
             never flat at noon"
        );
        assert!(jcm800 > 6.0, "jcm800 noon scoop is only {jcm800:.2} dB");
        assert!(
            bassman > jcm800 + 1.0,
            "bassman ({bassman:.2} dB) must scoop deeper than the JCM800 \
             ({jcm800:.2} dB)"
        );
    }

    /// The Big Muff's notch *slides* with its knob; an FMV stack's scoop stays
    /// where the components put it. Same engine, different circuit — this is
    /// the topology discriminator.
    ///
    /// The notch sits where the lowpass and highpass branches cross, so
    /// blending in more highpass drags it *down*: 1.3 kHz at position 4 to
    /// ~400 Hz at position 8. Below position 4 the network is a plain lowpass
    /// with no interior minimum at all, which is why the sweep starts there.
    #[test]
    fn the_big_muff_notch_slides_with_its_knob() {
        let probes = [320.0f32, 450.0, 640.0, 900.0, 1300.0];
        let notch = |pos: f32| {
            let mut ts = prepared(2, [5.0, 5.0, pos]);
            let mut best = (probes[0], f32::INFINITY);
            for f in probes {
                let g = rendered_db(&mut ts, f);
                if g < best.1 {
                    best = (f, g);
                }
            }
            best.0
        };
        let (high, mid, low) = (notch(4.0), notch(6.0), notch(8.0));
        assert!(
            high > mid && mid > low,
            "the blend knob must slide the notch downward: \
             pos 4 → {high} Hz, pos 6 → {mid} Hz, pos 8 → {low} Hz"
        );
        assert!(
            mid > probes[0] && mid < probes[probes.len() - 1],
            "at noon the notch must be an interior minimum, not a shelf edge \
             ({mid} Hz)"
        );
        assert!(
            high / low > 2.0,
            "a blend topology slides its notch across most of the midrange; \
             this one only moved {high} Hz → {low} Hz"
        );
    }

    /// The mid control lifts the scoop without becoming a mid *boost* — the
    /// FMV stack can only ever cut relative to its shoulders.
    #[test]
    fn the_mid_knob_lifts_the_scoop() {
        let mut lo = prepared(0, [5.0, 0.0, 5.0]);
        let mut hi = prepared(0, [5.0, 10.0, 5.0]);
        let at_800 = (rendered_db(&mut lo, 800.0), rendered_db(&mut hi, 800.0));
        assert!(
            at_800.1 - at_800.0 > 8.0,
            "mid 0→10 moved 800 Hz by only {:.2} dB",
            at_800.1 - at_800.0
        );
    }

    // --- calibration ---------------------------------------------------------

    /// Every model's makeup gain must land the noon *ceiling* at unity: the
    /// low shelf sits at 0 dB and no frequency exceeds it, so a scooped noon
    /// can never read as an absolute bass boost downstream.
    ///
    /// This replaces the original band-average calibration pin. Averaging
    /// over 80 Hz–7.2 kHz across a 7–9 dB scoop had pushed the shelves
    /// +4–5 dB above unity — measured against a pre-migration build, every
    /// FMV-voiced drive put exactly that much more absolute energy into
    /// 40–160 Hz at identical knobs (ADR 037). The raw maximum comes from
    /// the analog oracle over a fine log grid, so the pin is rate-independent
    /// and fails with the retune value in the message.
    #[test]
    fn noon_ceiling_sits_at_unity_with_the_makeup_applied() {
        for (kind, spec) in KINDS.iter().enumerate() {
            let mut max = f64::NEG_INFINITY;
            for i in 0..=200 {
                let freq = 15.0f32 * (16_000.0f32 / 15.0).powf(i as f32 / 200.0);
                max = max.max(nodal_db(&KINDS[kind], [5.0, 5.0, 5.0], freq));
            }
            let ceiling = max + makeup(kind);
            assert!(
                ceiling.abs() < 0.25,
                "{} noon ceiling sits at {ceiling:+.2} dB — retune makeup_db to {:.2}",
                spec.key,
                -max
            );
        }
    }

    /// The taper law itself: audio pots hit 10 % at half rotation, and reverse
    /// audio is its mirror. Both ends are exact for every taper.
    #[test]
    fn tapers_follow_their_law() {
        for taper in [Taper::Linear, Taper::Audio, Taper::ReverseAudio] {
            assert_eq!(taper.fraction(0.0), 0.0, "{taper:?} at 0");
            assert!((taper.fraction(10.0) - 1.0).abs() < 1e-6, "{taper:?} at 10");
            // Monotonic, and clamped outside 0..=10.
            let mut prev = -1.0;
            for i in 0..=20 {
                let f = taper.fraction(i as f32 * 0.5);
                assert!(f > prev, "{taper:?} must rise");
                prev = f;
            }
            assert_eq!(taper.fraction(-3.0), 0.0);
            assert_eq!(taper.fraction(99.0), taper.fraction(10.0));
        }
        assert!((Taper::Audio.fraction(5.0) - 0.1).abs() < 1e-4);
        assert!((Taper::ReverseAudio.fraction(5.0) - 0.9).abs() < 1e-4);
    }

    /// An audio taper puts noon somewhere else on the same network — the
    /// mechanism the registry can reach for when an ear says the scoop sits
    /// wrong, verified here so it is known to work before it is used.
    #[test]
    fn taper_choice_moves_where_noon_lands() {
        let lin = nodal_db(&KINDS[0], [5.0, 5.0, 5.0], 6_400.0);
        let audio = {
            let mut kind_with_audio = 0.0;
            // Same network, treble read through the audio law: position 5 is
            // 10 % of the track, not 50 %.
            let f = Taper::Audio.fraction(5.0) * 10.0;
            kind_with_audio += nodal_db(&KINDS[0], [5.0, 5.0, f], 6_400.0);
            kind_with_audio
        };
        assert!(
            (lin - audio).abs() > 3.0,
            "taper must matter: linear {lin:.2} dB vs audio {audio:.2} dB"
        );
    }

    // --- real-time behaviour --------------------------------------------------

    #[test]
    fn silence_in_silence_out() {
        for (kind, spec) in KINDS.iter().enumerate() {
            let mut ts = prepared(kind, [8.0, 2.0, 7.0]);
            let mut x = silence(4_096);
            ts.process_mono(&mut x);
            assert!(rms(&x) == 0.0, "{} broke silence", spec.key);
        }
    }

    /// A knob sweep rebuilds coefficients on every block. Because the state is
    /// the capacitor voltages, that must stay continuous — no click.
    #[test]
    fn sweeping_a_knob_is_click_free() {
        for (kind, spec) in KINDS.iter().enumerate() {
            let mut ts = prepared(kind, [5.0, 5.0, 5.0]);
            let mut y = sine(SR as u32, 220.0, SR as usize);
            let blocks = y.len() / 64;
            for (i, blk) in y.chunks_mut(64).enumerate() {
                let pos = 10.0 * i as f32 / blocks as f32;
                ts.set_knobs([pos, 10.0 - pos, pos]);
                ts.process_mono(blk);
            }
            assert_finite("knob sweep", &y);
            let step = y
                .windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0, f32::max);
            assert!(
                step < 0.25,
                "{}: knob sweep clicked, max step {step}",
                spec.key
            );
        }
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for (kind, _) in KINDS.iter().enumerate() {
            for rate in [44_100u32, 48_000, 96_000] {
                let mut ts = ToneStack::new(kind);
                ts.prepare(rate as f32);
                ts.set_knobs([7.0, 3.0, 8.0]);
                for chunk in [32usize, 483, 1_024] {
                    let mut x = sine(rate, 440.0, 4_096);
                    for blk in x.chunks_mut(chunk) {
                        ts.process_mono(blk);
                    }
                    assert_finite("tonestack multirate", &x);
                    assert!(peak(&x) < 4.0);
                }
            }
        }
    }

    /// Settled knobs must not rebuild — the steady-state cost is the state
    /// space and nothing else.
    #[test]
    fn settled_knobs_skip_the_rebuild() {
        let mut ts = prepared(0, [5.0, 5.0, 5.0]);
        ts.process_mono(&mut [0.0; 64]);
        let before = ts.ad;
        // Poison the applied marker's twin: if `update_coeffs` rebuilt, these
        // would be recomputed and match again.
        ts.ad[0][0] = 42.0;
        ts.process_mono(&mut [0.0; 64]);
        assert_eq!(ts.ad[0][0], 42.0, "settled knobs must skip the rebuild");
        ts.set_knobs([5.0, 6.0, 5.0]);
        ts.process_mono(&mut [0.0; 64]);
        assert_ne!(ts.ad[0][0], 42.0, "a moved knob must rebuild");
        assert_ne!(before[0][0], 0.0);
    }

    /// Switching models clears the old network's capacitor voltages instead of
    /// reinterpreting them, and stays bounded across the switch.
    #[test]
    fn model_switch_is_bounded() {
        let mut ts = prepared(0, [5.0, 5.0, 5.0]);
        let mut y = sine(SR as u32, 330.0, 12_000);
        for (i, blk) in y.chunks_mut(64).enumerate() {
            if i == 60 {
                ts.set_kind(1);
            }
            if i == 120 {
                ts.set_kind(2);
            }
            ts.process_mono(blk);
        }
        assert_finite("model switch", &y);
        assert!(peak(&y) < 4.0);
    }

    // --- the registry ---------------------------------------------------------

    /// `knob_mask` is a faceplate hint; it must agree with what the netlist
    /// actually wires up, or the pedal grows a dead knob nobody expects.
    #[test]
    fn knob_masks_match_the_netlists() {
        for kind in KINDS.iter() {
            let mut wired = 0u8;
            let mut caps = 0;
            for el in kind.net {
                match *el {
                    El::Pot { knob, .. } => wired |= 1 << knob,
                    El::Cap { .. } => caps += 1,
                    El::Res { .. } => {}
                }
                let (a, b) = match *el {
                    El::Res { a, b, .. } | El::Pot { a, b, .. } | El::Cap { a, b, .. } => (a, b),
                };
                assert!(
                    (a as usize) < kind.nodes && (b as usize) < kind.nodes,
                    "{}: node index past `nodes`",
                    kind.key
                );
            }
            assert_eq!(wired, kind.knob_mask, "{}: knob_mask drift", kind.key);
            assert!(kind.makeup_db.is_finite());
            assert!(caps <= MAX_CAPS && caps > 0, "{}: cap count", kind.key);
            assert!(kind.nodes <= MAX_NODES, "{}: node count", kind.key);
        }
        assert_eq!(KINDS.len(), MODEL_LABELS.len());
        assert_eq!(PARAMS.len(), DESC.params.len());
        // The named indices the drive family builds against.
        assert_eq!(KINDS[kind::BASSMAN].key, "bassman");
        assert_eq!(KINDS[kind::JCM800].key, "jcm800");
        assert_eq!(KINDS[kind::BIG_MUFF].key, "big-muff");
    }

    // --- the pedal ------------------------------------------------------------

    #[test]
    fn pedal_routes_its_params() {
        let mut stack = Stack::new();
        stack.prepare(SR as u32);
        for (i, p) in PARAMS.iter().enumerate() {
            stack.set_param(i, p.default_norm());
        }
        assert_eq!(stack.core.kind_index(), 0);
        stack.set_param(0, PARAMS[0].range.to_norm(1.0));
        assert_eq!(stack.core.kind_index(), 1);
        // Level is a plain trim on top of the network.
        let mut l = sine(SR as u32, 440.0, 24_000);
        let mut r = l.clone();
        for (a, b) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
            stack.process(a, b);
        }
        let flat = rms(&l[12_000..]);
        stack.reset();
        stack.set_param(4, PARAMS[4].range.to_norm(6.0));
        let mut l2 = sine(SR as u32, 440.0, 24_000);
        let mut r2 = l2.clone();
        for (a, b) in l2.chunks_mut(64).zip(r2.chunks_mut(64)) {
            stack.process(a, b);
        }
        let boosted = rms(&l2[12_000..]);
        assert!((lin_to_db(boosted / flat) - 6.0).abs() < 0.5);
        assert_eq!(l2, r2, "both channels must run the same network");
        // The level smoother rides per sample, so a step change ramps.
        let max_step = l2
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0, f32::max);
        assert!(max_step < 0.25, "level change clicked, step {max_step}");
    }

    #[test]
    fn pedal_every_knob_sweep_stays_finite() {
        for (i, param) in PARAMS.iter().enumerate() {
            let mut stack = Stack::new();
            stack.prepare(SR as u32);
            let mut x = sine(SR as u32, 330.0, 24_000);
            let mut xr = x.clone();
            let third = x.len() / 3;
            let (a, rest) = x.split_at_mut(third);
            let (b, c) = rest.split_at_mut(third);
            let (ar, restr) = xr.split_at_mut(third);
            let (br, cr) = restr.split_at_mut(third);
            stack.process(a, ar);
            stack.set_param(i, 0.0);
            stack.process(b, br);
            stack.set_param(i, 1.0);
            stack.process(c, cr);
            assert_finite(&format!("tonestack sweep {}", param.key), &x);
            assert!(peak(&x) < 8.0, "sweeping {} must stay bounded", param.key);
        }
    }
}

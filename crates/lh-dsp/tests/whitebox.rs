//! **The white-box harness** — proof that a WDF tree solves the circuit its
//! netlist draws, and the reusable kit for telling a modelled circuit from a
//! curve (Tone Revolution phase 08 §2.3; PRD 035).
//!
//! # The two questions a white-box pedal has to answer
//!
//! 1. **Is it the circuit?** Character tests cannot say. "Has a mid hump",
//!    "makes even harmonics", "the knee moves with frequency" are all things a
//!    *wrong* circuit can do. The only answer is a second, independent
//!    solution of the same netlist —
//!    [`netlist`](lh_dsp::testutil::netlist), which solves by modified nodal
//!    analysis and shares no code with `blocks::wdf`.
//! 2. **Is it better than a curve?** That is
//!    [`whitebox`](lh_dsp::testutil::whitebox): the measurements
//!    that separate a solved circuit from a memoryless waveshaper, written as
//!    reusable helpers so a new pedal inherits them instead of re-deriving
//!    them.
//!
//! # What the comparison actually pins
//!
//! Both solvers discretise capacitors trapezoidally, so they are two views of
//! **one** discrete system, not two approximations of a continuous one. Their
//! outputs should therefore agree to arithmetic precision, and the residual is
//! informative rather than arbitrary — it is dominated by `f32` in the tree and
//! by the Wright-omega closed form at the root (PRD 022), both of which the
//! tests below name and bound separately.
//!
//! The circuits here are declared once and built **twice** — as a WDF tree and
//! as a netlist — from the same constants, with the op-amp junction taken
//! straight from the framework's own public layout
//! ([`NON_INVERTING_PORTS`](lh_dsp::blocks::wdf::NON_INVERTING_PORTS)). So a
//! change to the layout breaks the reference, which is the coupling we want.
//! What this file does *not* claim is that any given `drive/*.rs` transcribed
//! its schematic correctly; that is the pedal's own tests' job.

use lh_dsp::blocks::wdf::{
    CapacitiveVoltageSource, Capacitor, DiodePair, Junction, NON_INVERTING_NODES,
    NON_INVERTING_PORTS, Parallel, RType, ResistiveVoltageSource, Resistor,
    ResistorCapacitorParallel, ResistorCapacitorSeries, Wdf, non_inverting_els,
};
use lh_dsp::testutil::netlist::{Circuit, El, GND};
use lh_dsp::testutil::whitebox;

/// The oversampled rate every WDF pedal in this project runs its tree at.
const OS_RATE: f64 = 4.0 * 48_000.0;

// ---------------------------------------------------------------------------
// Circuit 1 — the shunt RC-diode clipper (`drive/diode_clipper.rs`, Shunt mode;
// the same shape as `screamer`'s output stage).
// ---------------------------------------------------------------------------

const R_SERIES: f64 = 2200.0;
const C_SHUNT: f64 = 22e-9;
const IS: f64 = 2.52e-9;
const N: f64 = 1.75;
const VT: f64 = 0.02585;

fn shunt_reference() -> Circuit {
    // node 1 = the driving source, node 2 = the clipping node.
    let els = [
        El::Src { node: 1 },
        El::R {
            a: 1,
            b: 2,
            ohms: R_SERIES,
        },
        El::C {
            a: 2,
            b: GND,
            farads: C_SHUNT,
        },
        El::Pair {
            a: 2,
            b: GND,
            is: IS,
            vt_n: N * VT,
        },
    ];
    let mut c = Circuit::new(&els, 3);
    c.prepare(OS_RATE);
    c
}

/// The WDF side of circuit 1, driven exactly as the pedal drives it.
struct ShuntWdf {
    tree: Parallel<ResistiveVoltageSource, Capacitor>,
    pair: DiodePair,
    /// Use the iterative root instead of the closed form, to separate the
    /// tree's error from the omega approximation's.
    newton: bool,
}

impl ShuntWdf {
    fn new(newton: bool) -> Self {
        let mut tree = Parallel::new(
            ResistiveVoltageSource::new(R_SERIES as f32),
            Capacitor::new(C_SHUNT as f32, OS_RATE as f32),
        );
        tree.prepare(OS_RATE as f32);
        tree.calc_impedance();
        tree.reset();
        Self {
            tree,
            pair: DiodePair::new(IS as f32, N as f32, VT as f32),
            newton,
        }
    }

    fn step(&mut self, e: f32) -> f32 {
        self.tree.port1_mut().set_voltage(e);
        let a = self.tree.reflected();
        let r = self.tree.resistance();
        let (v, b) = if self.newton {
            self.pair.solve_newton(a, r)
        } else {
            self.pair.solve(a, r)
        };
        self.tree.incident(b);
        v
    }
}

// ---------------------------------------------------------------------------
// Circuit 2 — the op-amp overdrive junction (`NON_INVERTING_PORTS`; the shape
// shared by `ts-wdf`, `zendrive`, `king-of-tone` and `diode-clipper`'s Feedback
// mode). This is the R-type adaptor, i.e. the scattering matrix ADR 032 builds
// numerically at run time — the piece with no closed form to check it against.
// ---------------------------------------------------------------------------

const FB_R: f64 = 100e3;
const FB_C: f64 = 100e-12;
const LEG_R: f64 = 4.7e3;
const LEG_C: f64 = 47e-9;
const IN_C: f64 = 1e-6;
const IN_R: f64 = 470e3;
const LOAD_R: f64 = 1e6;
const AG: f64 = 3.0e3;
const RI: f64 = 1e9;
const RO: f64 = 100.0;

/// The netlist of circuit 2, written from the **documented** node numbering of
/// [`NON_INVERTING_PORTS`]: 0 ground, 1 non-inverting input, 2 inverting input,
/// 3 output, 4 the op-amp's internal node — plus 5 and 6, the two nodes that
/// live inside the port elements rather than inside the junction.
fn opamp_reference() -> Circuit {
    let els = [
        El::Src { node: 5 },
        El::C {
            a: 5,
            b: 1,
            farads: IN_C,
        }, // CapacitiveVoltageSource, port 1
        El::R {
            a: 1,
            b: GND,
            ohms: IN_R,
        }, //   ‖ Resistor
        El::R {
            a: 1,
            b: 2,
            ohms: RI,
        }, // the op-amp, as three junction elements
        El::Vcvs {
            p: 4,
            n: GND,
            cp: 1,
            cn: 2,
            gain: AG,
        },
        El::R {
            a: 4,
            b: 3,
            ohms: RO,
        },
        El::R {
            a: 2,
            b: 6,
            ohms: LEG_R,
        }, // ResistorCapacitorSeries, port 2
        El::C {
            a: 6,
            b: GND,
            farads: LEG_C,
        },
        El::R {
            a: 3,
            b: GND,
            ohms: LOAD_R,
        }, // Resistor, port 3
        El::R {
            a: 3,
            b: 2,
            ohms: FB_R,
        }, // ResistorCapacitorParallel, across the up port
        El::C {
            a: 3,
            b: 2,
            farads: FB_C,
        },
        El::Pair {
            a: 3,
            b: 2,
            is: IS,
            vt_n: N * VT,
        }, // the root
    ];
    let mut c = Circuit::new(&els, 7);
    c.prepare(OS_RATE);
    c
}

type InputLeg = Parallel<CapacitiveVoltageSource, Resistor>;
type OpAmpNode = RType<4, 3, (InputLeg, ResistorCapacitorSeries, Resistor)>;
type FeedbackTree = Parallel<ResistorCapacitorParallel, OpAmpNode>;

/// The same junction with **both** reactive legs made resistive, for the static
/// sweep. Coupling capacitors would leave the stage a unity-gain follower at
/// DC, and a follower never reaches the diodes — the sweep has to put the root
/// to work for the comparison to mean anything.
type DcOpAmpNode = RType<4, 3, (ResistiveVoltageSource, Resistor, Resistor)>;
type DcFeedbackTree = Parallel<ResistorCapacitorParallel, DcOpAmpNode>;

static OPAMP: [lh_dsp::blocks::wdf::JEl; 3] = non_inverting_els(AG as f32, RI as f32, RO as f32);
static JUNCTION: Junction = Junction {
    nodes: NON_INVERTING_NODES,
    els: &OPAMP,
    ports: &NON_INVERTING_PORTS,
};

struct OpAmpWdf {
    tree: FeedbackTree,
    pair: DiodePair,
    newton: bool,
}

impl OpAmpWdf {
    fn new(newton: bool) -> Self {
        let mut tree = Parallel::new(
            ResistorCapacitorParallel::new(FB_R as f32, FB_C as f32, OS_RATE as f32),
            RType::new(
                &JUNCTION,
                (
                    Parallel::new(
                        CapacitiveVoltageSource::new(IN_C as f32, OS_RATE as f32),
                        Resistor::new(IN_R as f32),
                    ),
                    ResistorCapacitorSeries::new(LEG_R as f32, LEG_C as f32, OS_RATE as f32),
                    Resistor::new(LOAD_R as f32),
                ),
            ),
        );
        tree.prepare(OS_RATE as f32);
        tree.calc_impedance();
        tree.reset();
        Self {
            tree,
            pair: DiodePair::new(IS as f32, N as f32, VT as f32),
            newton,
        }
    }

    /// Returns the stage output — port 3 of the junction, the load node.
    fn step(&mut self, e: f32) -> f32 {
        self.tree
            .port2_mut()
            .ports_mut()
            .0
            .port1_mut()
            .set_voltage(e);
        let a = self.tree.reflected();
        let r = self.tree.resistance();
        let (_v, b) = if self.newton {
            self.pair.solve_newton(a, r)
        } else {
            self.pair.solve(a, r)
        };
        self.tree.incident(b);
        self.tree.port2().port_voltage(3)
    }
}

// ---------------------------------------------------------------------------
// The comparisons
// ---------------------------------------------------------------------------

/// Largest absolute difference between two runs, and the reference's own peak,
/// so a caller can report both an absolute and a relative figure.
fn compare(a: &[f64], b: &[f64]) -> (f64, f64) {
    let err = a
        .iter()
        .zip(b)
        .fold(0.0f64, |m, (x, y)| m.max((x - y).abs()));
    let peak = b.iter().fold(0.0f64, |m, v| m.max(v.abs()));
    (err, peak)
}

/// Static transfer curve: hold the input still until the capacitors are open,
/// then read the output. No discretisation is involved at equilibrium, so this
/// isolates the *algebra* — the tree's impedance reduction and the root's
/// solution — from everything time-dependent.
#[test]
fn the_shunt_clipper_solves_the_same_static_curve_as_nodal_analysis() {
    let mut refc = shunt_reference();
    let mut wdf = ShuntWdf::new(true);

    let mut worst = 0.0f64;
    for e in [
        -20.0, -5.0, -1.0, -0.6, -0.4, -0.2, -0.05, 0.0, 0.05, 0.2, 0.4, 0.6, 1.0, 5.0, 20.0f64,
    ] {
        refc.reset();
        refc.settle(e, 400);
        // The capacitor is charged through 2.2 kΩ; 4000 samples at 192 kHz is
        // ~430 time constants.
        for _ in 0..4000 {
            wdf.step(e as f32);
        }
        let got = f64::from(wdf.step(e as f32));
        let want = refc.node(2);
        worst = worst.max((got - want).abs());
        assert!(
            (got - want).abs() < 2e-6,
            "e = {e} V: wdf {got:.9} vs nodal {want:.9}"
        );
    }
    println!("shunt static: worst |Δ| = {worst:.3e} V");
}

/// The dynamic case, where the discretisation has to agree too. Driven with a
/// signal that spends time on both sides of the knee at a frequency where the
/// shunt capacitor matters (its corner is 3.3 kHz).
#[test]
fn the_shunt_clipper_tracks_nodal_analysis_through_a_swept_signal() {
    for (label, newton, bound) in [("omega", false, 4e-4), ("newton", true, 5e-6)] {
        let mut refc = shunt_reference();
        let mut wdf = ShuntWdf::new(newton);
        let (mut got, mut want) = (Vec::new(), Vec::new());
        let n = 4096;
        for k in 0..n {
            // Two tones plus a slow envelope: crosses the knee in both
            // directions and at both sides of the capacitor's corner.
            let t = k as f64 / OS_RATE;
            let env = 0.05 + 2.0 * (std::f64::consts::TAU * 30.0 * t).sin().abs();
            let e = env
                * ((std::f64::consts::TAU * 440.0 * t).sin()
                    + 0.5 * (std::f64::consts::TAU * 5000.0 * t).sin());
            refc.step(e);
            want.push(refc.node(2));
            got.push(f64::from(wdf.step(e as f32)));
        }
        let (err, peak) = compare(&got, &want);
        println!("shunt dynamic ({label}): |Δ| ≤ {err:.3e} V on a {peak:.3} V peak");
        assert!(
            err < bound,
            "{label}: worst |Δ| {err:.3e} V exceeds {bound:.0e} (peak {peak:.3} V)"
        );
    }
}

/// **The phase's acceptance criterion**: the R-type adaptor — whose scattering
/// matrix is built numerically from the junction netlist at run time (ADR 032)
/// and which therefore has no published closed form to check against — solves
/// the same circuit as nodal analysis.
///
/// Statically, so the capacitors are open and what is left is exactly the
/// matrix's algebra: four ports, an op-amp folded in as three junction
/// elements, and a diode root. To get a DC path the input leg is a plain
/// resistive source here rather than the pedal's coupling capacitor; the ports
/// and the junction are the framework's own.
#[test]
fn the_op_amp_junction_solves_the_same_static_curve_as_nodal_analysis() {
    // Same netlist as `opamp_reference`, with the input coupling capacitor
    // replaced by a direct drive through `IN_R`.
    let els = [
        El::Src { node: 5 },
        El::R {
            a: 5,
            b: 1,
            ohms: IN_R,
        },
        El::R {
            a: 1,
            b: 2,
            ohms: RI,
        },
        El::Vcvs {
            p: 4,
            n: GND,
            cp: 1,
            cn: 2,
            gain: AG,
        },
        El::R {
            a: 4,
            b: 3,
            ohms: RO,
        },
        El::R {
            a: 2,
            b: GND,
            ohms: LEG_R,
        },
        El::R {
            a: 3,
            b: GND,
            ohms: LOAD_R,
        },
        El::R {
            a: 3,
            b: 2,
            ohms: FB_R,
        },
        El::C {
            a: 3,
            b: 2,
            farads: FB_C,
        },
        El::Pair {
            a: 3,
            b: 2,
            is: IS,
            vt_n: N * VT,
        },
    ];
    let mut refc = Circuit::new(&els, 6);
    refc.prepare(OS_RATE);

    let mut worst = 0.0f64;
    // DC gain is 1 + 100k/4.7k = 22.3×, so the diodes take over above ~20 mV
    // and the sweep spans clean, knee and hard-clipped.
    for e in [
        -1.0, -0.2, -0.05, -0.02, -0.005, 0.0, 0.005, 0.02, 0.05, 0.2, 1.0f64,
    ] {
        refc.reset();
        refc.settle(e, 2_000);

        let mut tree: DcFeedbackTree = Parallel::new(
            ResistorCapacitorParallel::new(FB_R as f32, FB_C as f32, OS_RATE as f32),
            RType::new(
                &JUNCTION,
                (
                    ResistiveVoltageSource::new(IN_R as f32),
                    Resistor::new(LEG_R as f32),
                    Resistor::new(LOAD_R as f32),
                ),
            ),
        );
        tree.prepare(OS_RATE as f32);
        tree.calc_impedance();
        tree.reset();
        let mut pair = DiodePair::new(IS as f32, N as f32, VT as f32);

        // Only the 100 pF across the 100 kΩ feedback resistor is left: 10 µs,
        // so 2 000 samples at 192 kHz is ~1000 time constants.
        let mut got = 0.0f32;
        for _ in 0..2_000 {
            tree.port2_mut().ports_mut().0.set_voltage(e as f32);
            let a = tree.reflected();
            let r = tree.resistance();
            let (_v, b) = pair.solve_newton(a, r);
            tree.incident(b);
            got = tree.port2().port_voltage(3);
        }

        let got = f64::from(got);
        let want = refc.node(3);
        worst = worst.max((got - want).abs());
        // The bound is relative because the error is: `f32` through a junction
        // whose element values span seven decades (`Ri` = 1 GΩ beside
        // `Ro` = 100 Ω) conditions the solve at ~300, and 300·f32 eps is
        // 3.6e-5. Measured: 3.3e-5 of signal, flat across the sweep — which is
        // the signature of conditioning rather than of a wrong matrix, and is
        // −90 dB either way.
        // The bound is relative, because the residual is: `f32` through a
        // junction whose element values span seven decades (`Ri` = 1 GΩ beside
        // `Ro` = 100 Ω) conditions the solve at a few hundred, and a few
        // hundred × `f32` eps is a few times 1e-5. Measured: 3.2e-5 of signal,
        // flat across the sweep — the signature of conditioning, not of a
        // wrong matrix, and −90 dB either way.
        assert!(
            (got - want).abs() < 2e-7 + 5e-5 * want.abs(),
            "e = {e} V: wdf {got:.9} vs nodal {want:.9}"
        );
    }
    println!("op-amp junction static: worst |Δ| = {worst:.3e} V");
}

/// The same junction under signal. This is the one that would catch a
/// scattering matrix that is right at DC and wrong under reactive load.
#[test]
fn the_op_amp_junction_tracks_nodal_analysis_through_a_swept_signal() {
    for (label, newton, bound) in [("omega", false, 2e-3), ("newton", true, 3e-4)] {
        let mut refc = opamp_reference();
        let mut wdf = OpAmpWdf::new(newton);
        let (mut got, mut want) = (Vec::new(), Vec::new());
        let n = 8192;
        for k in 0..n {
            let t = k as f64 / OS_RATE;
            let env = 0.002 + 0.08 * (std::f64::consts::TAU * 25.0 * t).sin().abs();
            let e = env
                * ((std::f64::consts::TAU * 220.0 * t).sin()
                    + 0.4 * (std::f64::consts::TAU * 3300.0 * t).sin());
            refc.step(e);
            got.push(f64::from(wdf.step(e as f32)));
            want.push(refc.node(3));
        }
        let (err, peak) = compare(&got, &want);
        println!("op-amp dynamic ({label}): |Δ| ≤ {err:.3e} V on a {peak:.3} V peak");
        assert!(
            err < bound,
            "{label}: worst |Δ| {err:.3e} V exceeds {bound:.0e} (peak {peak:.3} V)"
        );
    }
}

/// The residual in the two runs above is not noise — it is the Wright-omega
/// closed form's approximation error (PRD 022), and swapping the root for its
/// iterative twin removes most of it. Pinning that ordering keeps the harness
/// honest about *what* it is measuring: if a future change made the omega and
/// Newton runs equally wrong, the error would no longer be attributable to the
/// root, and this test says so.
#[test]
fn the_closed_form_root_is_the_dominant_error_not_the_tree() {
    let run = |newton: bool| {
        let mut refc = shunt_reference();
        let mut wdf = ShuntWdf::new(newton);
        let (mut got, mut want) = (Vec::new(), Vec::new());
        for k in 0..4096 {
            let t = k as f64 / OS_RATE;
            let e = 2.0 * (std::f64::consts::TAU * 440.0 * t).sin();
            refc.step(e);
            want.push(refc.node(2));
            got.push(f64::from(wdf.step(e as f32)));
        }
        compare(&got, &want).0
    };
    let omega = run(false);
    let newton = run(true);
    println!("root error: omega {omega:.3e} V, newton {newton:.3e} V");
    assert!(
        omega > 4.0 * newton,
        "the closed form should dominate the residual (omega {omega:.3e}, newton {newton:.3e})"
    );
}

// ---------------------------------------------------------------------------
// The discrimination kit, applied to the framework's own circuits. These are
// the worked examples the cookbook points at: what a new pedal should measure,
// and what the numbers look like when the answer is "yes, it is a circuit".
// ---------------------------------------------------------------------------

/// A `tanh` at a comparable drive, as the control every figure below is read
/// against. Whatever the kit reports for the circuits has to stand clear of
/// what it reports for a curve.
fn curve_control(amp: f64) -> (f64, f64) {
    let m = whitebox::memory(|x| (3.0 * x).tanh(), OS_RATE, amp, 16_384);
    let k = whitebox::knee_shift(|x| (3.0 * x).tanh(), OS_RATE, 200.0, 4000.0, amp);
    (m, k)
}

#[test]
fn the_shunt_clipper_is_a_circuit_and_a_curve_is_not() {
    let mut wdf = ShuntWdf::new(false);
    let m = whitebox::memory(|x| wdf.step(x), OS_RATE, 2.0, 16_384);
    let (control, _) = curve_control(2.0);
    println!("shunt clipper memory {m:.4} vs tanh {control:.3e}");
    assert!(
        m > 0.02,
        "a shunt RC-diode clipper must carry state, got {m:.3e}"
    );
    assert!(
        m > 100.0 * control,
        "circuit {m:.3e} must stand clear of the curve floor {control:.3e}"
    );
}

/// The shunt capacitor is *why* this circuit is not a curve: above its corner
/// it takes the signal away from the diodes, so the same amplitude distorts
/// less at 4 kHz than at 200 Hz. A `tanh` cannot do that at all.
#[test]
fn the_shunt_clippers_knee_moves_with_frequency() {
    let mut wdf = ShuntWdf::new(false);
    let k = whitebox::knee_shift(|x| wdf.step(x), OS_RATE, 200.0, 4000.0, 1.0);
    let (_, control) = curve_control(1.0);
    println!("shunt clipper knee shift {k:.3} (tanh control {control:.6})");
    assert!(
        (control - 1.0).abs() < 1e-6,
        "the control must be frequency-flat, got {control:.6}"
    );
    assert!(
        k < 0.85,
        "the shunt cap must soften the highs, got a THD ratio of {k:.3}"
    );
}

#[test]
fn the_op_amp_stage_is_a_circuit_and_carries_more_state_than_the_shunt() {
    let mut wdf = OpAmpWdf::new(false);
    let m = whitebox::memory(|x| wdf.step(x), OS_RATE, 0.05, 16_384);
    let (control, _) = curve_control(0.05);
    println!("op-amp stage memory {m:.4} vs tanh {control:.3e}");
    assert!(
        m > 0.05,
        "a feedback clipper with three reactive legs must carry state, got {m:.3e}"
    );
    assert!(m > 100.0 * control);
}

/// The two properties every solved circuit owes the audio thread, measured the
/// same way for both trees: it cannot run away, and it cannot invent a signal.
#[test]
fn the_wdf_trees_are_bounded_and_exactly_silent() {
    const SLAM: f32 = 1e6;
    let mut shunt = ShuntWdf::new(false);
    let mut opamp = OpAmpWdf::new(false);
    // The two bounds differ because the two topologies differ, and the
    // difference is worth stating rather than papering over with one number.
    // A *shunt* clipper clamps to the diode knee no matter what arrives, so its
    // output is bounded absolutely. A *non-inverting* stage passes its input
    // through at unity and only clips the loop, so an absolute bound is the
    // wrong property — what must not run away is the gain.
    for (name, peak, bound) in [
        (
            "shunt",
            whitebox::bounded(|x| shunt.step(x), SLAM, 4_096),
            5.0,
        ),
        (
            "op-amp",
            whitebox::bounded(|x| opamp.step(x), SLAM, 4_096),
            1.1 * f64::from(SLAM),
        ),
    ] {
        let peak = peak.unwrap_or_else(|| panic!("{name}: a root diverged"));
        assert!(peak < bound, "{name}: {peak:.3} V exceeds {bound:.3} V");
    }

    let mut shunt = ShuntWdf::new(false);
    assert!(
        whitebox::silent(|x| shunt.step(x), 1_024),
        "shunt: silence in must give exactly silence out"
    );
    let mut opamp2 = OpAmpWdf::new(false);
    assert!(
        whitebox::silent(|x| opamp2.step(x), 1_024),
        "op-amp: silence in must give exactly silence out"
    );
}

//! An **independent** numerical circuit solver, for use as a golden reference
//! against [`lh_dsp::blocks::wdf`] (Tone Revolution phase 08 §2.3).
//!
//! # Why a second solver
//!
//! Every white-box pedal in this project rests on one claim: *the WDF tree
//! solves the circuit the schematic draws.* Character tests cannot check that —
//! they check that the output has a mid hump, or even harmonics, or a moving
//! knee, all of which a wrong-but-plausible circuit also has. What checks it is
//! a second solution of the same circuit, derived a different way.
//!
//! So this module solves circuits by **modified nodal analysis** — the textbook
//! method WDF exists to avoid — and shares no code, no formula and no constant
//! with the thing it judges. Where WDF reasons in wave variables and pushes a
//! scattering matrix around a tree, this stamps conductances into one big
//! matrix and inverts it. If they agree, the tree is wired the way the netlist
//! says.
//!
//! # The method
//!
//! Unknowns are the non-ground node voltages, followed by one branch current
//! per voltage source and per controlled source. Resistors stamp a
//! conductance. Capacitors stamp their **trapezoidal companion model** —
//! conductance `G = 2C/T` in parallel with a history current `s`, updated
//! `s' = 2·G·v − s` — which is the same discretisation the bilinear WDF
//! capacitor uses, so the two solvers are two views of *one* discrete system
//! rather than two approximations of a continuous one. That is what makes
//! agreement to `1e-6` a meaningful bar rather than a lucky one.
//!
//! Nonlinear elements are linearised at the current guess (conductance
//! `di/dv` plus an equivalent current source) and the whole system is re-solved
//! until it stops moving — Newton–Raphson on the full state, damped the way
//! SPICE damps it: the step direction is Newton's, but its length is cut back
//! so no junction moves more than a few thermal voltages per iteration.
//!
//! # What it is not
//!
//! Not real-time anything. It allocates, it iterates to convergence with no
//! ceiling worth the name, and it runs in `f64` throughout. It is a test
//! oracle; the RT rules do not apply to it and it must never be linked into
//! `lh-dsp` proper. It lives in the library rather than in `tests/` so that a
//! pedal's own unit tests can reach it (see [`super`]).

/// Ground. Every netlist reserves node 0 for it.
pub const GND: usize = 0;

/// Convergence threshold on the largest unknown update, in volts.
const TOL: f64 = 1e-12;
/// Newton ceiling. Generous — this is offline code, and a circuit that needs
/// more than this many iterations is a circuit worth failing on.
const MAX_ITERS: usize = 200;
/// Largest junction-voltage move Newton is allowed in one iteration, in
/// thermal-voltage units.
const DV_MAX_VT: f64 = 5.0;
/// Past this many thermal voltages the exponential is clamped; the junction is
/// either hard on or hard off and the derivative no longer carries information.
const EXP_CLAMP: f64 = 80.0;

/// One element of a netlist.
///
/// Node numbers are indices into the circuit's node set, with [`GND`] = 0.
/// Two terminals on the same node number *are* a wire.
#[derive(Clone, Copy, Debug)]
pub enum El {
    /// A resistor of `ohms` between `a` and `b`.
    R { a: usize, b: usize, ohms: f64 },
    /// A capacitor of `farads` between `a` and `b`, trapezoidal companion.
    C { a: usize, b: usize, farads: f64 },
    /// The driven source: an ideal voltage source from `node` to ground whose
    /// value is the `e` passed to [`Circuit::step`]. Exactly one per netlist.
    Src { node: usize },
    /// Voltage-controlled voltage source: `v(p) − v(n) = gain·(v(cp) − v(cn))`.
    Vcvs {
        p: usize,
        n: usize,
        cp: usize,
        cn: usize,
        gain: f64,
    },
    /// Antiparallel diode pair from `a` to `b`: `i = 2·Is·sinh(v/vt_n)`.
    ///
    /// `vt_n` is the *pair's* thermal scale — `n·Vt` times however many devices
    /// sit in series per branch.
    Pair {
        a: usize,
        b: usize,
        is: f64,
        vt_n: f64,
    },
    /// Asymmetric stack from `a` to `b`: `i = Is·(exp(v/vt_f) − exp(−v/vt_r))`,
    /// i.e. `m_fwd` devices one way and `m_rev` the other.
    Asym {
        a: usize,
        b: usize,
        is: f64,
        vt_f: f64,
        vt_r: f64,
    },
}

impl El {
    fn is_nonlinear(&self) -> bool {
        matches!(self, El::Pair { .. } | El::Asym { .. })
    }

    /// Current from `a` to `b` at branch voltage `v`, and its derivative.
    fn nonlinear(&self, v: f64) -> (f64, f64) {
        match *self {
            El::Pair { is, vt_n, .. } => {
                let u = (v / vt_n).clamp(-EXP_CLAMP, EXP_CLAMP);
                let e = u.exp();
                let ei = 1.0 / e;
                (is * (e - ei), is * (e + ei) / vt_n)
            }
            El::Asym { is, vt_f, vt_r, .. } => {
                let uf = (v / vt_f).clamp(-EXP_CLAMP, EXP_CLAMP);
                let ur = (-v / vt_r).clamp(-EXP_CLAMP, EXP_CLAMP);
                let ef = uf.exp();
                let er = ur.exp();
                (is * (ef - er), is * (ef / vt_f + er / vt_r))
            }
            _ => (0.0, 0.0),
        }
    }

    /// The junction's thermal scale, for damping.
    fn vt_scale(&self) -> f64 {
        match *self {
            El::Pair { vt_n, .. } => vt_n,
            El::Asym { vt_f, vt_r, .. } => vt_f.min(vt_r),
            _ => f64::INFINITY,
        }
    }

    /// Every node this element names, for the netlist check in
    /// [`Circuit::new`].
    fn nodes(&self) -> Vec<usize> {
        match *self {
            El::R { a, b, .. } | El::C { a, b, .. } => vec![a, b],
            El::Pair { a, b, .. } | El::Asym { a, b, .. } => vec![a, b],
            El::Src { node } => vec![node],
            El::Vcvs { p, n, cp, cn, .. } => vec![p, n, cp, cn],
        }
    }

    fn terminals(&self) -> (usize, usize) {
        match *self {
            El::R { a, b, .. } | El::C { a, b, .. } => (a, b),
            El::Pair { a, b, .. } | El::Asym { a, b, .. } => (a, b),
            El::Src { node } => (node, GND),
            El::Vcvs { p, n, .. } => (p, n),
        }
    }
}

/// A netlist plus the state needed to march it through time.
pub struct Circuit {
    els: Vec<El>,
    /// Node count *including* ground.
    nodes: usize,
    /// Unknown count: `(nodes − 1)` node voltages plus one branch current per
    /// voltage source and controlled source.
    dim: usize,
    /// Companion conductance per element (`2C/T` for capacitors, `1/R` for
    /// resistors, unused otherwise). Rebuilt by [`Circuit::prepare`].
    g: Vec<f64>,
    /// Capacitor history current, indexed like `els`.
    hist: Vec<f64>,
    /// Branch-current unknown index per source element, indexed like `els`.
    branch: Vec<usize>,
    /// Last solution — Newton's warm start and the caller's output.
    x: Vec<f64>,
    m: Vec<f64>,
    rhs: Vec<f64>,
    /// Scratch for the linear solve. Reused rather than reallocated per Newton
    /// iteration: this is offline code, but a golden run is millions of steps
    /// and the allocator is the whole cost otherwise.
    lhs: Vec<f64>,
    next: Vec<f64>,
    /// Iterations the last [`step`](Circuit::step) needed, for diagnostics.
    pub iters: usize,
}

impl Circuit {
    /// `nodes` counts ground.
    ///
    /// Panics if an element names a node outside the set, or if the netlist has
    /// no driven source — both of which are the typo a hand-written netlist
    /// actually makes, and both of which would otherwise show up much later as
    /// a wrong number rather than as an error.
    pub fn new(els: &[El], nodes: usize) -> Self {
        assert!(nodes >= 2, "a netlist needs ground and at least one node");
        for (i, el) in els.iter().enumerate() {
            for n in el.nodes() {
                assert!(
                    n < nodes,
                    "element {i} ({el:?}) names node {n}, but the netlist has {nodes} \
                     (0..{})",
                    nodes - 1
                );
            }
        }
        assert_eq!(
            els.iter().filter(|e| matches!(e, El::Src { .. })).count(),
            1,
            "a netlist needs exactly one driven source"
        );

        let mut dim = nodes - 1;
        let mut branch = vec![usize::MAX; els.len()];
        for (i, el) in els.iter().enumerate() {
            if matches!(el, El::Src { .. } | El::Vcvs { .. }) {
                branch[i] = dim;
                dim += 1;
            }
        }
        Self {
            els: els.to_vec(),
            nodes,
            dim,
            g: vec![0.0; els.len()],
            hist: vec![0.0; els.len()],
            branch,
            x: vec![0.0; dim],
            m: vec![0.0; dim * dim],
            rhs: vec![0.0; dim],
            lhs: vec![0.0; dim * dim],
            next: vec![0.0; dim],
            iters: 0,
        }
    }

    /// Fix the time step. Capacitor companion conductances are `2C/T`.
    pub fn prepare(&mut self, sample_rate: f64) {
        for (i, el) in self.els.iter().enumerate() {
            self.g[i] = match *el {
                El::R { ohms, .. } => 1.0 / ohms,
                El::C { farads, .. } => 2.0 * farads * sample_rate,
                _ => 0.0,
            };
        }
        self.reset();
    }

    /// Zero every state — capacitor histories and the Newton warm start.
    pub fn reset(&mut self) {
        self.hist.iter_mut().for_each(|s| *s = 0.0);
        self.x.iter_mut().for_each(|v| *v = 0.0);
    }

    /// Node count, counting ground.
    pub fn node_count(&self) -> usize {
        self.nodes
    }

    /// Voltage at a node (ground reads 0).
    pub fn node(&self, i: usize) -> f64 {
        if i == GND { 0.0 } else { self.x[i - 1] }
    }

    /// Voltage across two nodes.
    pub fn across(&self, a: usize, b: usize) -> f64 {
        self.node(a) - self.node(b)
    }

    /// Advance one time step with the driven source at `e`. Returns the node
    /// voltages by way of [`node`](Circuit::node).
    pub fn step(&mut self, e: f64) {
        self.iters = 0;
        for it in 0..MAX_ITERS {
            self.assemble(e);
            self.lhs.copy_from_slice(&self.m);
            self.next.copy_from_slice(&self.rhs);
            solve(&mut self.lhs, &mut self.next, self.dim).expect("reference circuit is singular");

            // Damp: Newton's direction, but no junction moves more than
            // `DV_MAX_VT` thermal voltages in one iteration.
            let mut lambda = 1.0f64;
            for el in self.els.iter().filter(|e| e.is_nonlinear()) {
                let (a, b) = el.terminals();
                let old = node_of(&self.x, a) - node_of(&self.x, b);
                let new = node_of(&self.next, a) - node_of(&self.next, b);
                let d = (new - old).abs();
                let cap = DV_MAX_VT * el.vt_scale();
                if d > cap {
                    lambda = lambda.min(cap / d);
                }
            }

            let mut moved = 0.0f64;
            for (x, n) in self.x.iter_mut().zip(self.next.iter()) {
                let step = lambda * (*n - *x);
                moved = moved.max(step.abs());
                *x += step;
            }
            self.iters = it + 1;
            if moved < TOL {
                break;
            }
        }
        self.advance();
    }

    /// Stamp the whole netlist, linearised at the current guess.
    fn assemble(&mut self, e: f64) {
        self.m.iter_mut().for_each(|v| *v = 0.0);
        self.rhs.iter_mut().for_each(|v| *v = 0.0);

        for i in 0..self.els.len() {
            match self.els[i] {
                El::R { a, b, .. } => self.conductance(a, b, self.g[i]),
                El::C { a, b, .. } => {
                    self.conductance(a, b, self.g[i]);
                    self.current(a, b, -self.hist[i]);
                }
                El::Src { node } => {
                    let k = self.branch[i];
                    self.source_row(k, node, GND);
                    self.rhs[k] += e;
                }
                El::Vcvs { p, n, cp, cn, gain } => {
                    let k = self.branch[i];
                    self.source_row(k, p, n);
                    if cp != GND {
                        self.m[k * self.dim + (cp - 1)] -= gain;
                    }
                    if cn != GND {
                        self.m[k * self.dim + (cn - 1)] += gain;
                    }
                }
                el @ (El::Pair { a, b, .. } | El::Asym { a, b, .. }) => {
                    let v = self.across(a, b);
                    let (i_d, g_d) = el.nonlinear(v);
                    self.conductance(a, b, g_d);
                    // Equivalent source: `i ≈ i(v₀) + g·(v − v₀)`, so the
                    // constant part `i(v₀) − g·v₀` is a current from a to b.
                    self.current(a, b, i_d - g_d * v);
                }
            }
        }
    }

    /// Stamp a conductance between two nodes.
    fn conductance(&mut self, a: usize, b: usize, g: f64) {
        let d = self.dim;
        if a != GND {
            self.m[(a - 1) * d + (a - 1)] += g;
        }
        if b != GND {
            self.m[(b - 1) * d + (b - 1)] += g;
        }
        if a != GND && b != GND {
            self.m[(a - 1) * d + (b - 1)] -= g;
            self.m[(b - 1) * d + (a - 1)] -= g;
        }
    }

    /// Stamp a current source of `i` amps flowing from `a` to `b` *through the
    /// element*, i.e. leaving node `a`.
    fn current(&mut self, a: usize, b: usize, i: f64) {
        if a != GND {
            self.rhs[a - 1] -= i;
        }
        if b != GND {
            self.rhs[b - 1] += i;
        }
    }

    /// The two halves of a voltage-source stamp: the branch current in the KCL
    /// rows of `p`/`n`, and the constraint row `v(p) − v(n) = …`.
    fn source_row(&mut self, k: usize, p: usize, n: usize) {
        let d = self.dim;
        if p != GND {
            self.m[(p - 1) * d + k] += 1.0;
            self.m[k * d + (p - 1)] += 1.0;
        }
        if n != GND {
            self.m[(n - 1) * d + k] -= 1.0;
            self.m[k * d + (n - 1)] -= 1.0;
        }
    }

    /// Roll every capacitor's history forward: `s' = 2·G·v − s`.
    fn advance(&mut self) {
        for i in 0..self.els.len() {
            if let El::C { a, b, .. } = self.els[i] {
                let v = self.across(a, b);
                self.hist[i] = 2.0 * self.g[i] * v - self.hist[i];
            }
        }
    }

    /// Settle the circuit at a fixed input: run until the state stops moving.
    /// Used for static transfer curves, where the capacitors must end up open.
    pub fn settle(&mut self, e: f64, steps: usize) {
        for _ in 0..steps {
            self.step(e);
        }
    }
}

fn node_of(x: &[f64], i: usize) -> f64 {
    if i == GND { 0.0 } else { x[i - 1] }
}

/// Dense Gaussian elimination with partial pivoting, row-major, in place.
///
/// Written out rather than pulled from a crate so the reference shares nothing
/// with the code it judges — and small enough that "written out" costs nothing.
fn solve(m: &mut [f64], b: &mut [f64], n: usize) -> Option<()> {
    for col in 0..n {
        let (mut pivot, mut best) = (col, m[col * n + col].abs());
        for row in col + 1..n {
            let v = m[row * n + col].abs();
            if v > best {
                best = v;
                pivot = row;
            }
        }
        if best < 1e-30 {
            return None;
        }
        if pivot != col {
            for k in 0..n {
                m.swap(col * n + k, pivot * n + k);
            }
            b.swap(col, pivot);
        }
        let d = m[col * n + col];
        for row in col + 1..n {
            let f = m[row * n + col] / d;
            if f == 0.0 {
                continue;
            }
            for k in col..n {
                m[row * n + k] -= f * m[col * n + k];
            }
            b[row] -= f * b[col];
        }
    }
    for col in (0..n).rev() {
        let mut acc = b[col];
        for k in col + 1..n {
            acc -= m[col * n + k] * b[k];
        }
        b[col] = acc / m[col * n + col];
    }
    Some(())
}

/// Self-checks: the reference is only worth anything if it is right, and every
/// one of these has a closed form to be right against.
#[cfg(test)]
mod tests {
    use super::*;

    /// A divider is the smallest thing that can be wrong, so it is the first
    /// thing checked: if the stamps are right, `v = e·R2/(R1+R2)` exactly.
    #[test]
    fn the_reference_solves_a_resistive_divider() {
        let els = [
            El::Src { node: 1 },
            El::R {
                a: 1,
                b: 2,
                ohms: 1000.0,
            },
            El::R {
                a: 2,
                b: GND,
                ohms: 3000.0,
            },
        ];
        let mut c = Circuit::new(&els, 3);
        c.prepare(48_000.0);
        c.step(4.0);
        assert!((c.node(2) - 3.0).abs() < 1e-12, "got {}", c.node(2));
    }

    /// An RC settles to the source and its corner lands where `1/2πRC` says.
    #[test]
    fn the_reference_lowpass_has_the_right_corner() {
        const R: f64 = 1000.0;
        const C: f64 = 100e-9;
        let els = [
            El::Src { node: 1 },
            El::R {
                a: 1,
                b: 2,
                ohms: R,
            },
            El::C {
                a: 2,
                b: GND,
                farads: C,
            },
        ];
        let sr = 192_000.0;
        let mut c = Circuit::new(&els, 3);
        c.prepare(sr);
        c.settle(1.0, 5_000);
        assert!((c.node(2) - 1.0).abs() < 1e-9, "DC: {}", c.node(2));

        // At the corner the magnitude is −3 dB. Measure it with a sine.
        let f = 1.0 / (std::f64::consts::TAU * R * C);
        c.reset();
        let n = 20_000;
        let mut peak = 0.0f64;
        for k in 0..n {
            let t = k as f64 / sr;
            c.step((std::f64::consts::TAU * f * t).sin());
            if k > n / 2 {
                peak = peak.max(c.node(2).abs());
            }
        }
        let db = 20.0 * peak.log10();
        assert!((db + 3.01).abs() < 0.1, "corner magnitude {db:.2} dB");
    }

    /// A diode pair across an ideal source has to sit on its own curve:
    /// `i = 2·Is·sinh(v/nVt)` with the series resistor carrying `(e − v)/R`.
    #[test]
    fn the_reference_puts_a_diode_on_its_own_curve() {
        const R: f64 = 2200.0;
        const IS: f64 = 2.52e-9;
        const VT_N: f64 = 1.75 * 25.85e-3;
        let els = [
            El::Src { node: 1 },
            El::R {
                a: 1,
                b: 2,
                ohms: R,
            },
            El::Pair {
                a: 2,
                b: GND,
                is: IS,
                vt_n: VT_N,
            },
        ];
        let mut c = Circuit::new(&els, 3);
        c.prepare(192_000.0);
        for e in [0.001, 0.05, 0.4, 1.0, 5.0, -5.0, 50.0] {
            c.reset();
            c.settle(e, 200);
            let v = c.node(2);
            let i_diode = 2.0 * IS * (v / VT_N).sinh();
            let i_res = (e - v) / R;
            assert!(
                (i_diode - i_res).abs() < 1e-12 + 1e-9 * i_res.abs(),
                "e={e}: diode {i_diode:e} vs resistor {i_res:e}"
            );
        }
    }

    /// An op-amp built from the same three elements the WDF junction uses must
    /// come out as the textbook non-inverting amplifier.
    #[test]
    fn the_reference_solves_a_non_inverting_op_amp() {
        // nodes: 1 = +in (driven), 2 = −in, 3 = out, 4 = internal
        const AG: f64 = 1e6;
        let els = [
            El::Src { node: 1 },
            El::R {
                a: 1,
                b: 2,
                ohms: 1e12,
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
                ohms: 100.0,
            },
            El::R {
                a: 3,
                b: 2,
                ohms: 90e3,
            },
            El::R {
                a: 2,
                b: GND,
                ohms: 10e3,
            },
        ];
        let mut c = Circuit::new(&els, 5);
        c.prepare(48_000.0);
        c.step(0.1);
        // 1 + 90k/10k = 10×
        assert!((c.node(3) - 1.0).abs() < 1e-3, "got {}", c.node(3));
    }
}

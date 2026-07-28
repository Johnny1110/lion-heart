//! The **R-type** adaptor: an N-port junction that series/parallel reduction
//! cannot express — op-amp feedback, bridged networks, the interacting legs of
//! a tone stack.
//!
//! Where [`Series`](super::Series) and [`Parallel`](super::Parallel) have
//! closed-form scattering relations, a general junction needs a full N×N
//! **scattering matrix** `b = S·a`, whose entries are functions of the port
//! reference resistances. Everything else about the adaptor is bookkeeping:
//! gather each child's reflected wave into `a`, multiply, push `b` back down.
//!
//! # Where `S` comes from
//!
//! From the junction's own netlist, solved numerically — not from a symbolic
//! algebra pass, and never transcribed from anyone else's published matrix.
//!
//! The construction falls out of what a port *is*. `a_k = v_k + R_k·i_k` says
//! the outside world presents port `k` as a Thévenin source of EMF `a_k` behind
//! resistance `R_k`; `b_k = v_k − R_k·i_k = 2·v_k − a_k` reads the answer back.
//! So: stamp every port as `a_k` in series with `R_k`, stamp the junction's own
//! elements, solve the resulting nodal system for the port voltages, and the
//! reflected waves follow. Doing that once per basis vector `a = e_k` fills in
//! column `k` of `S` — the same "solve once per basis vector" trick
//! `eq::tonestack` uses to extract a state space from a netlist.
//!
//! There is nothing to mis-transcribe, which is the point: the plan named
//! scattering-matrix correctness as this phase's likeliest failure, and a
//! matrix that is *derived from the topology at run time* cannot drift from the
//! topology it claims to model. See ADR 032 for the trade against offline
//! symbolic code generation.
//!
//! # Cost, and when it is paid
//!
//! Building `S` is `N + 1` solves of a system no bigger than
//! [`MNA_DIM`]`×`[`MNA_DIM`] — hundreds of nanoseconds, on fixed-size stack
//! arrays, with no allocation. It happens in [`Wdf::calc_impedance`], i.e. at
//! the **block boundary and only when a knob actually moved**. The per-sample
//! path is one N×N matrix–vector product and nothing else. (`BYOD` rebuilds its
//! matrices per sample while a parameter smooths; that is not a cost this
//! project needs to pay for a knob a human is turning.)
//!
//! # The up port
//!
//! Port **0** is the up port — the one facing the rest of the tree or the
//! nonlinear root. It is *adapted*: its reference resistance is set to the
//! Thévenin resistance the junction presents there, which makes `S[0][0] = 0`
//! and breaks the delay-free loop. Children are ports `1..N`.
//!
//! That choice is not free, and it constrains where the up port may sit: the
//! adaptation *is* `R_up = R_thévenin`, so a node whose impedance a feedback
//! loop drives to nearly zero — an op-amp's own output pin, say — makes a
//! degenerate up port. Put the up port where the circuit has a real impedance
//! (past the series output resistor, or facing the diode network, which is what
//! the classic op-amp overdrive topologies do), and hang the low-impedance node
//! inside the junction.
//!
//! Structure follows `chowdsp_wdf`'s `rtype/*.h` (BSD-3).

use super::Wdf;

/// Largest junction the fixed-size solver accepts (including the up port).
pub const MAX_PORTS: usize = 8;
/// Largest node count in a junction netlist, counting ground as node 0.
pub const MAX_J_NODES: usize = 12;
/// Largest number of controlled sources (one per op-amp) in a junction.
pub const MAX_J_VCVS: usize = 4;
/// Dimension of the nodal system: one unknown per non-ground node, plus one
/// branch current per controlled source.
pub const MNA_DIM: usize = (MAX_J_NODES - 1) + MAX_J_VCVS;

/// An element *inside* an R-type junction — something that is not a port.
///
/// Ports carry the circuit's resistors and capacitors; what remains here is the
/// wiring and whatever active devices are folded into the junction. Wires need
/// no entry at all: two terminals on the same node number *are* a wire.
#[derive(Clone, Copy)]
pub enum JEl {
    /// A resistor internal to the junction. Used for an op-amp's input and
    /// output resistances, which belong to the junction rather than to a port.
    Res { a: u8, b: u8, ohms: f32 },
    /// Voltage-controlled voltage source: `v(p) − v(n) = gain · (v(cp) − v(cn))`.
    /// This is what makes an op-amp expressible — the controlled source is part
    /// of the junction, so it is folded into `S` once and costs nothing per
    /// sample.
    Vcvs {
        p: u8,
        n: u8,
        cp: u8,
        cn: u8,
        gain: f32,
    },
}

/// A junction's topology: the elements that are not ports, and where each port
/// attaches.
///
/// Node `0` is ground. `ports[0]` is the **up port**; `ports[1..]` are the
/// children, in the same order as the [`RType`]'s port tuple. A port entry
/// `(p, n)` means the port's `+` terminal is node `p` and its `−` terminal is
/// node `n`.
pub struct Junction {
    /// Node count *including* ground.
    pub nodes: usize,
    pub els: &'static [JEl],
    pub ports: &'static [(u8, u8)],
}

/// An op-amp as three junction elements: differential input resistance, the
/// controlled source, and the output resistance.
///
/// `ag` is the open-loop gain, `ri` the input resistance, `ro` the output
/// resistance; `internal` must be a node number used by nothing else (it sits
/// between the controlled source and `ro`). As `ag → ∞`, `ri → ∞`, `ro → 0`
/// this converges on the ideal virtual short — which
/// `op_amp_converges_on_the_ideal_virtual_short` pins, and which is why
/// `drive/sd1.rs`'s hand-reduced ideal op-amp and this model are two views of
/// one device rather than two different circuits.
pub const fn op_amp(
    in_p: u8,
    in_n: u8,
    out: u8,
    internal: u8,
    ag: f32,
    ri: f32,
    ro: f32,
) -> [JEl; 3] {
    [
        JEl::Res {
            a: in_p,
            b: in_n,
            ohms: ri,
        },
        JEl::Vcvs {
            p: internal,
            n: 0,
            cp: in_p,
            cn: in_n,
            gain: ag,
        },
        JEl::Res {
            a: internal,
            b: out,
            ohms: ro,
        },
    ]
}

/// Node count of the [non-inverting op-amp junction](NON_INVERTING_PORTS).
pub const NON_INVERTING_NODES: usize = 5;

/// Port layout of the **classic op-amp overdrive junction** — a non-inverting
/// amplifier whose feedback path is the adapted up port, so the clipping diodes
/// hang there and see the loop rather than the signal.
///
/// Nodes: `0` ground, `1` non-inverting input, `2` inverting input, `3` output,
/// `4` the op-amp's internal node.
///
/// The Tube Screamer, the ZenDrive and the MXR Distortion+ are *this junction*
/// with different parts hung off it — same nodes, same ports, same scattering
/// structure. Sharing the layout here is how that fact gets expressed without a
/// matrix being written down twice: each pedal builds its own `S` from these
/// ports and its own op-amp constants, at run time, from the topology (ADR 032).
///
/// The up port is on the feedback path for a reason beyond faithfulness — it is
/// a high-impedance node (roughly `(Ag+1)·Z_gain-leg`), which keeps
/// `R_up = R_thévenin` well conditioned. The op-amp's own output pin would not
/// be; see the module docs.
pub static NON_INVERTING_PORTS: [(u8, u8); 4] = [
    (3, 2), // up: the feedback path — where the clipping diodes hang
    (1, 0), // the input leg, loading the non-inverting pin
    (2, 0), // the gain leg, inverting pin to ground
    (3, 0), // the load, and the stage's output tap
];

/// The elements of the [`NON_INVERTING_PORTS`] junction: one op-amp, wired to
/// the node numbering that layout assumes.
pub const fn non_inverting_els(ag: f32, ri: f32, ro: f32) -> [JEl; 3] {
    op_amp(1, 2, 3, 4, ag, ri, ro)
}

/// Scratch for one junction solve. Lives on the caller's stack; nothing here
/// allocates.
struct Mna {
    m: [[f64; MNA_DIM]; MNA_DIM],
    rhs: [[f64; MAX_PORTS]; MNA_DIM],
    /// Unknown count: `(nodes − 1) + vcvs`.
    n: usize,
    /// First unknown index belonging to a controlled-source branch current.
    vbase: usize,
}

impl Mna {
    fn new(j: &Junction) -> Self {
        let vcvs = j
            .els
            .iter()
            .filter(|e| matches!(e, JEl::Vcvs { .. }))
            .count();
        debug_assert!(j.nodes <= MAX_J_NODES, "junction has too many nodes");
        debug_assert!(vcvs <= MAX_J_VCVS, "junction has too many op-amps");
        debug_assert!(j.ports.len() <= MAX_PORTS, "junction has too many ports");
        Self {
            m: [[0.0; MNA_DIM]; MNA_DIM],
            rhs: [[0.0; MAX_PORTS]; MNA_DIM],
            n: (j.nodes - 1) + vcvs,
            vbase: j.nodes - 1,
        }
    }

    /// Stamp a conductance between two nodes (node 0 is ground and has no row).
    fn conductance(&mut self, a: u8, b: u8, g: f64) {
        let (a, b) = (a as usize, b as usize);
        if a != 0 {
            self.m[a - 1][a - 1] += g;
        }
        if b != 0 {
            self.m[b - 1][b - 1] += g;
        }
        if a != 0 && b != 0 {
            self.m[a - 1][b - 1] -= g;
            self.m[b - 1][a - 1] -= g;
        }
    }

    /// Stamp a current `i` injected into node `p` and drawn from node `n`, in
    /// right-hand-side column `col`.
    fn current(&mut self, p: u8, n: u8, i: f64, col: usize) {
        if p != 0 {
            self.rhs[p as usize - 1][col] += i;
        }
        if n != 0 {
            self.rhs[n as usize - 1][col] -= i;
        }
    }

    /// Stamp the junction's own elements. Controlled sources are numbered in
    /// the order they appear.
    fn stamp_elements(&mut self, j: &Junction) {
        let mut vi = 0usize;
        for el in j.els {
            match *el {
                JEl::Res { a, b, ohms } => self.conductance(a, b, 1.0 / f64::from(ohms)),
                JEl::Vcvs { p, n, cp, cn, gain } => {
                    let row = self.vbase + vi;
                    vi += 1;
                    // The branch current leaves node p and enters node n...
                    if p != 0 {
                        self.m[p as usize - 1][row] += 1.0;
                        self.m[row][p as usize - 1] += 1.0;
                    }
                    if n != 0 {
                        self.m[n as usize - 1][row] -= 1.0;
                        self.m[row][n as usize - 1] -= 1.0;
                    }
                    // ...under the constraint v(p) − v(n) − gain·(v(cp) − v(cn)) = 0.
                    let g = f64::from(gain);
                    if cp != 0 {
                        self.m[row][cp as usize - 1] -= g;
                    }
                    if cn != 0 {
                        self.m[row][cn as usize - 1] += g;
                    }
                }
            }
        }
    }

    /// Voltage across port `k`'s terminals, read out of solved column `col`.
    fn port_voltage(&self, j: &Junction, k: usize, col: usize) -> f64 {
        let (p, n) = j.ports[k];
        let vp = if p == 0 {
            0.0
        } else {
            self.rhs[p as usize - 1][col]
        };
        let vn = if n == 0 {
            0.0
        } else {
            self.rhs[n as usize - 1][col]
        };
        vp - vn
    }

    /// Gaussian elimination with partial pivoting over `cols` right-hand sides;
    /// solutions replace `rhs` in place.
    #[allow(clippy::needless_range_loop)]
    fn solve(&mut self, cols: usize) -> bool {
        let n = self.n;
        for i in 0..n {
            let mut piv = i;
            for r in i + 1..n {
                if self.m[r][i].abs() > self.m[piv][i].abs() {
                    piv = r;
                }
            }
            if self.m[piv][i].abs() < 1e-30 {
                return false;
            }
            self.m.swap(i, piv);
            self.rhs.swap(i, piv);
            for r in i + 1..n {
                let f = self.m[r][i] / self.m[i][i];
                if f == 0.0 {
                    continue;
                }
                for c in i..n {
                    self.m[r][c] -= f * self.m[i][c];
                }
                for c in 0..cols {
                    self.rhs[r][c] -= f * self.rhs[i][c];
                }
            }
        }
        for i in (0..n).rev() {
            for c in 0..cols {
                let mut acc = self.rhs[i][c];
                for k in i + 1..n {
                    acc -= self.m[i][k] * self.rhs[k][c];
                }
                self.rhs[i][c] = acc / self.m[i][i];
            }
        }
        true
    }
}

/// The Thévenin resistance the junction presents at its up port, with every
/// child port terminated in its own reference resistance.
///
/// Measured the way an engineer would on the bench: inject 1 A and read the
/// voltage. Setting the up port's reference resistance to this value is exactly
/// what makes `S[0][0]` vanish.
fn up_resistance(j: &Junction, child_r: &[f32]) -> f32 {
    let mut mna = Mna::new(j);
    mna.stamp_elements(j);
    for (k, &r) in child_r.iter().enumerate() {
        let (p, n) = j.ports[k + 1];
        mna.conductance(p, n, 1.0 / f64::from(r));
    }
    let (p, n) = j.ports[0];
    mna.current(p, n, 1.0, 0);
    if !mna.solve(1) {
        return 1.0;
    }
    mna.port_voltage(j, 0, 0) as f32
}

/// Build the adapted scattering matrix for `j`, given the children's port
/// resistances. Returns the up port's reference resistance.
///
/// `S[row][col]` is the standard orientation: `b = S·a`.
// Dense linear algebra: the port index is the meaning of the loop, and it
// indexes three different arrays at once. Iterator adaptors obscure that.
#[allow(clippy::needless_range_loop)]
pub fn adapted_scattering<const N: usize>(
    j: &Junction,
    child_r: &[f32],
    s: &mut [[f32; N]; N],
) -> f32 {
    debug_assert_eq!(j.ports.len(), N);
    debug_assert_eq!(child_r.len(), N - 1);

    // A degenerate junction (all ports shorted out, say) would give a
    // non-positive up resistance and poison every downstream reciprocal. Clamp
    // rather than propagate a NaN into the audio path.
    let r_up = up_resistance(j, child_r);
    let r_up = if r_up.is_finite() && r_up > 1e-9 {
        r_up
    } else {
        1e-9
    };

    let mut r = [0.0f32; MAX_PORTS];
    r[0] = r_up;
    r[1..N].copy_from_slice(&child_r[..N - 1]);

    // One system, N right-hand sides: column k drives port k with a = 1 (as a
    // Norton current e/R into its terminals) and every other port with a = 0.
    let mut mna = Mna::new(j);
    mna.stamp_elements(j);
    for k in 0..N {
        let (p, n) = j.ports[k];
        mna.conductance(p, n, 1.0 / f64::from(r[k]));
        mna.current(p, n, 1.0 / f64::from(r[k]), k);
    }
    if !mna.solve(N) {
        // Singular: fall back to a transparent junction rather than NaNs.
        *s = [[0.0; N]; N];
        return r_up;
    }

    for col in 0..N {
        for row in 0..N {
            let v = mna.port_voltage(j, row, col);
            let delta = if row == col { 1.0 } else { 0.0 };
            s[row][col] = (2.0 * v - delta) as f32;
        }
    }
    r_up
}

/// The child ports of an [`RType`], as a tuple. Implemented for tuples of 1..=7
/// [`Wdf`] nodes — the Rust equivalent of `chowdsp_wdf`'s variadic port pack.
pub trait PortSet<const M: usize> {
    fn calc_impedance(&mut self);
    fn impedances(&self, out: &mut [f32; M]);
    fn gather(&mut self, out: &mut [f32; M]);
    fn scatter(&mut self, b: &[f32; M]);
    fn prepare(&mut self, sample_rate: f32);
    fn reset(&mut self);
}

macro_rules! impl_port_set {
    ($m:expr; $($idx:tt : $t:ident),+) => {
        impl<$($t: Wdf),+> PortSet<$m> for ($($t,)+) {
            fn calc_impedance(&mut self) { $(self.$idx.calc_impedance();)+ }
            fn impedances(&self, out: &mut [f32; $m]) { $(out[$idx] = self.$idx.resistance();)+ }
            #[inline]
            fn gather(&mut self, out: &mut [f32; $m]) { $(out[$idx] = self.$idx.reflected();)+ }
            #[inline]
            fn scatter(&mut self, b: &[f32; $m]) { $(self.$idx.incident(b[$idx]);)+ }
            fn prepare(&mut self, sample_rate: f32) { $(self.$idx.prepare(sample_rate);)+ }
            fn reset(&mut self) { $(self.$idx.reset();)+ }
        }
    };
}

impl_port_set!(1; 0: A);
impl_port_set!(2; 0: A, 1: B);
impl_port_set!(3; 0: A, 1: B, 2: C);
impl_port_set!(4; 0: A, 1: B, 2: C, 3: D);
impl_port_set!(5; 0: A, 1: B, 2: C, 3: D, 4: E);
impl_port_set!(6; 0: A, 1: B, 2: C, 3: D, 4: E, 5: F);
impl_port_set!(7; 0: A, 1: B, 2: C, 3: D, 4: E, 5: F, 6: G);

/// An adapted N-port R-type adaptor owning its `M = N − 1` child ports.
///
/// Both counts are const parameters because Rust cannot yet compute `N − 1` in
/// a type position; [`RType::new`] asserts the invariant.
pub struct RType<const N: usize, const M: usize, P: PortSet<M>> {
    junction: &'static Junction,
    ports: P,
    s: [[f32; N]; N],
    a: [f32; N],
    b: [f32; N],
    r: f32,
    g: f32,
}

impl<const N: usize, const M: usize, P: PortSet<M>> RType<N, M, P> {
    /// Build an adaptor for `junction`, taking ownership of its child ports.
    /// `junction.ports[0]` is the up port; `ports.0`, `ports.1`, … attach to
    /// `junction.ports[1..]` in order.
    pub fn new(junction: &'static Junction, ports: P) -> Self {
        assert_eq!(N, M + 1, "an R-type's port count is its children plus one");
        assert_eq!(junction.ports.len(), N, "junction port count mismatch");
        let mut s = Self {
            junction,
            ports,
            s: [[0.0; N]; N],
            a: [0.0; N],
            b: [0.0; N],
            r: 1.0,
            g: 1.0,
        };
        s.calc_impedance();
        s
    }

    pub fn ports(&self) -> &P {
        &self.ports
    }
    pub fn ports_mut(&mut self) -> &mut P {
        &mut self.ports
    }

    /// The scattering matrix as last built — for tests and diagnostics.
    pub fn s_matrix(&self) -> &[[f32; N]; N] {
        &self.s
    }

    /// The voltage across port `k` after a complete wave exchange —
    /// `v = (a + b)/2`, the standard WDF read. Port 0 is the up port.
    ///
    /// This is how a pedal taps its **output node**: an op-amp's output is a
    /// junction node, not a leaf, so there is no one-port to ask. Hanging the
    /// load resistor on a port and reading its voltage here costs nothing per
    /// sample — both waves are already sitting in the adaptor's arrays.
    #[inline]
    pub fn port_voltage(&self, k: usize) -> f32 {
        0.5 * (self.a[k] + self.b[k])
    }
}

impl<const N: usize, const M: usize, P: PortSet<M>> Wdf for RType<N, M, P> {
    fn calc_impedance(&mut self) {
        self.ports.calc_impedance();
        let mut child_r = [0.0f32; M];
        self.ports.impedances(&mut child_r);
        self.r = adapted_scattering::<N>(self.junction, &child_r, &mut self.s);
        self.g = 1.0 / self.r;
    }

    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }

    #[inline]
    fn reflected(&mut self) -> f32 {
        let mut child = [0.0f32; M];
        self.ports.gather(&mut child);
        self.a[1..N].copy_from_slice(&child[..M]);
        // The up port is adapted (`S[0][0] = 0`), so its own incident wave —
        // not yet known this sample — contributes nothing to what it reflects.
        let mut acc = 0.0;
        for k in 1..N {
            acc += self.s[0][k] * self.a[k];
        }
        acc
    }

    #[inline]
    fn incident(&mut self, a: f32) {
        self.a[0] = a;
        for row in 0..N {
            let mut acc = 0.0;
            for k in 0..N {
                acc += self.s[row][k] * self.a[k];
            }
            self.b[row] = acc;
        }
        let mut child = [0.0f32; M];
        child[..M].copy_from_slice(&self.b[1..N]);
        self.ports.scatter(&child);
    }

    fn prepare(&mut self, sample_rate: f32) {
        self.ports.prepare(sample_rate);
    }

    fn reset(&mut self) {
        self.ports.reset();
        self.a = [0.0; N];
        self.b = [0.0; N];
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::wdf::adaptor::{Parallel, PolarityInverter, Series};
    use crate::blocks::wdf::one_port::{Capacitor, ResistiveVoltageSource, Resistor};

    const SR: f32 = 96_000.0;

    /// Three ports all bridging one node to ground: a parallel junction.
    static PARALLEL_J: Junction = Junction {
        nodes: 2,
        els: &[],
        ports: &[(1, 0), (1, 0), (1, 0)],
    };

    /// Four ports in a directed cycle: a series junction. Node 0 is ground, so
    /// the last port closes the loop back to it.
    static SERIES_J: Junction = Junction {
        nodes: 3,
        els: &[],
        ports: &[(1, 2), (2, 0), (0, 1)],
    };

    /// Four ports on one node: a wider parallel junction, for the tree
    /// cross-check.
    static PARALLEL4_J: Junction = Junction {
        nodes: 2,
        els: &[],
        ports: &[(1, 0), (1, 0), (1, 0), (1, 0)],
    };

    /// **The independent check on `S` itself.** A junction of nothing but wires
    /// stores no energy and dissipates none, so the power carried in by the
    /// incident waves must leave in the reflected ones: `Σ aₖ²/Rₖ = Σ bₖ²/Rₖ`
    /// for *every* `a`, i.e. `Sᵀ R⁻¹ S = R⁻¹`.
    ///
    /// This is pure algebra on the finished matrix — it shares no code and no
    /// reasoning with the nodal construction that produced it, so a sign slip,
    /// a transposed stamp or a mis-numbered node cannot satisfy it by accident.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn wire_junctions_conserve_power() {
        for (j, name) in [(&PARALLEL_J, "parallel"), (&SERIES_J, "series")] {
            for child in [[1_000.0f32, 1_000.0], [470.0, 22_000.0], [1e5, 33.0]] {
                let mut s = [[0.0f32; 3]; 3];
                let r_up = adapted_scattering::<3>(j, &child, &mut s);
                let r = [r_up, child[0], child[1]];

                for row in 0..3 {
                    for col in 0..3 {
                        // (Sᵀ R⁻¹ S)[row][col] must equal (R⁻¹)[row][col].
                        let mut acc = 0.0f64;
                        for k in 0..3 {
                            acc += f64::from(s[k][row]) * f64::from(s[k][col]) / f64::from(r[k]);
                        }
                        let want = if row == col {
                            1.0 / f64::from(r[row])
                        } else {
                            0.0
                        };
                        assert!(
                            (acc - want).abs() <= 1e-6 * (1.0 / f64::from(r[row])).max(1e-9),
                            "{name} r={child:?}: (SᵀR⁻¹S)[{row}][{col}] = {acc:e}, want {want:e}"
                        );
                    }
                }
            }
        }
    }

    /// The adapted up port must reflect none of its own incident wave — the
    /// property that breaks the delay-free loop. It is what choosing `R_up` as
    /// the junction's Thévenin resistance buys, so this also checks
    /// [`up_resistance`].
    #[test]
    fn the_up_port_is_reflection_free() {
        for j in [&PARALLEL_J, &SERIES_J] {
            for child in [[1_000.0f32, 1_000.0], [1.0, 1e6]] {
                let mut s = [[0.0f32; 3]; 3];
                adapted_scattering::<3>(j, &child, &mut s);
                assert!(s[0][0].abs() < 1e-5, "S[0][0] = {}", s[0][0]);
            }
        }
    }

    /// A parallel junction built numerically must reproduce the closed-form
    /// parallel adaptor — both the resistance it presents and every entry of
    /// its scattering. The closed form was derived independently (and pinned
    /// against `chowdsp_wdf`), so this is a genuine cross-check of the netlist
    /// machinery against known physics.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn rtype_reproduces_the_parallel_adaptor() {
        for child in [[1_000.0f32, 1_000.0], [470.0, 22_000.0]] {
            let mut s = [[0.0f32; 3]; 3];
            let r_up = adapted_scattering::<3>(&PARALLEL_J, &child, &mut s);
            let (g1, g2) = (1.0 / child[0], 1.0 / child[1]);
            assert!(
                (r_up - 1.0 / (g1 + g2)).abs() / r_up < 1e-4,
                "R_up {r_up} vs {}",
                1.0 / (g1 + g2)
            );

            // Closed form: v = Σ Gₖaₖ / Σ Gₖ and bₖ = 2v − aₖ, with G_up = G1+G2.
            let g = [g1 + g2, g1, g2];
            let gsum: f32 = g.iter().sum();
            for col in 0..3 {
                for row in 0..3 {
                    let v = g[col] / gsum;
                    let want = 2.0 * v - if row == col { 1.0 } else { 0.0 };
                    assert!(
                        (s[row][col] - want).abs() < 1e-4,
                        "child={child:?} S[{row}][{col}] = {} want {want}",
                        s[row][col]
                    );
                }
            }
        }
    }

    /// The same for a series junction: one shared current, voltages summing to
    /// zero, `b_up = −(a₁ + a₂)`.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn rtype_reproduces_the_series_adaptor() {
        for child in [[1_000.0f32, 1_000.0], [470.0, 22_000.0]] {
            let mut s = [[0.0f32; 3]; 3];
            let r_up = adapted_scattering::<3>(&SERIES_J, &child, &mut s);
            assert!(
                (r_up - (child[0] + child[1])).abs() / r_up < 1e-4,
                "R_up {r_up} vs {}",
                child[0] + child[1]
            );

            let r = [r_up, child[0], child[1]];
            let rsum: f32 = r.iter().sum();
            for col in 0..3 {
                for row in 0..3 {
                    // bₖ = aₖ − 2Rₖ·i with i = Σa/ΣR; here a = e_col.
                    let i = 1.0 / rsum;
                    let want = if row == col { 1.0 } else { 0.0 } - 2.0 * r[row] * i;
                    assert!(
                        (s[row][col] - want).abs() < 1e-4,
                        "child={child:?} S[{row}][{col}] = {} want {want}",
                        s[row][col]
                    );
                }
            }
        }
    }

    /// **End to end.** A four-port parallel junction driven as an [`RType`]
    /// must track the equivalent nested [`Parallel`] tree sample for sample,
    /// reactive state and all. The two share no arithmetic: one multiplies a
    /// matrix built by nodal analysis, the other evaluates closed-form
    /// three-port relations. Agreement over a long run means the gather /
    /// scatter bookkeeping, the port ordering and the matrix are all right.
    #[test]
    fn rtype_tracks_the_equivalent_adaptor_tree() {
        let mut rt: RType<4, 3, _> = RType::new(
            &PARALLEL4_J,
            (
                ResistiveVoltageSource::new(2_200.0),
                Capacitor::new(22e-9, SR),
                Resistor::new(47_000.0),
            ),
        );
        let mut tree = Parallel::new(
            Parallel::new(
                ResistiveVoltageSource::new(2_200.0),
                Capacitor::new(22e-9, SR),
            ),
            Resistor::new(47_000.0),
        );
        rt.calc_impedance();
        tree.calc_impedance();
        assert!(
            (rt.resistance() - tree.resistance()).abs() / tree.resistance() < 1e-4,
            "R: {} vs {}",
            rt.resistance(),
            tree.resistance()
        );

        for k in 0..2_000 {
            let e = 3.0 * (k as f32 * 0.021).sin() + (k as f32 * 0.31).cos();
            rt.ports_mut().0.set_voltage(e);
            tree.port1_mut().port1_mut().set_voltage(e);

            let (br, bt) = (rt.reflected(), tree.reflected());
            assert!(
                (br - bt).abs() < 2e-4 * br.abs().max(1.0),
                "k={k}: {br} vs {bt}"
            );
            // Open root (b = a) — nothing damps a divergence away.
            rt.incident(br);
            tree.incident(bt);
        }
    }

    // ---- op-amp ------------------------------------------------------------

    /// Series output resistor, between the op-amp's output and the up port.
    /// Every real pedal has one; here it also keeps the adapted up port off a
    /// node whose impedance a feedback loop drives to nearly zero (see the
    /// module docs — an up port wants a sane Thévenin resistance).
    const R_OUT: f32 = 1_000.0;

    /// A non-inverting amplifier, as an R-type junction. Nodes: 0 ground,
    /// 1 the `+` input, 2 the `−` input, 3 the op-amp output, 4 its internal
    /// pre-`Ro` node, 5 the pedal output past [`R_OUT`]. Ports: up = the output
    /// (open-circuited by the test root), then the input source, the gain leg
    /// `Rg` to ground, and the feedback resistor `Rf` from output to `−`.
    macro_rules! non_inverting {
        ($name:ident, $ag:expr, $ri:expr, $ro:expr) => {
            static $name: Junction = Junction {
                nodes: 6,
                els: &{
                    let oa = op_amp(1, 2, 3, 4, $ag, $ri, $ro);
                    [
                        oa[0],
                        oa[1],
                        oa[2],
                        JEl::Res {
                            a: 3,
                            b: 5,
                            ohms: R_OUT,
                        },
                    ]
                },
                ports: &[(5, 0), (1, 0), (2, 0), (3, 2)],
            };
        };
    }

    non_inverting!(OPAMP_REAL, 100.0, 1e9, 0.1);
    non_inverting!(OPAMP_BETTER, 1e4, 1e11, 1e-3);
    non_inverting!(OPAMP_IDEAL, 1e7, 1e13, 1e-6);

    /// Open-circuit output voltage of the non-inverting amp for a 1 V source.
    fn non_inverting_gain(j: &'static Junction, rg: f32, rf: f32) -> f32 {
        let mut rt: RType<4, 3, _> = RType::new(
            j,
            (
                ResistiveVoltageSource::new(1.0), // near-ideal source
                Resistor::new(rg),
                Resistor::new(rf),
            ),
        );
        rt.ports_mut().0.set_voltage(1.0);
        rt.calc_impedance();
        // Open root: a = b, so the port voltage is the reflected wave itself.
        let b = rt.reflected();
        rt.incident(b);
        b
    }

    /// **The op-amp's acceptance test** (Phase 03 §4.1). Pushing `Ag → ∞`,
    /// `Ri → ∞`, `Ro → 0` must converge on the ideal virtual short's textbook
    /// gain `1 + Rf/Rg` — which is exactly the reduction `drive/sd1.rs` applies
    /// by hand. The two models are therefore one device seen two ways, and
    /// Phase 04's "faithful" variants can be built on this with confidence.
    #[test]
    fn op_amp_converges_on_the_ideal_virtual_short() {
        for (rg, rf) in [
            (4_700.0f32, 47_000.0f32),
            (1_000.0, 1_000.0),
            (10_000.0, 470_000.0),
        ] {
            let ideal = 1.0 + rf / rg;
            let got = non_inverting_gain(&OPAMP_IDEAL, rg, rf);
            assert!(
                (got - ideal).abs() / ideal < 1e-3,
                "Rg={rg} Rf={rf}: {got} vs ideal {ideal}"
            );

            // And the approach is monotone in open-loop gain: a better op-amp
            // sits strictly closer to the ideal than a worse one.
            let real = non_inverting_gain(&OPAMP_REAL, rg, rf);
            let better = non_inverting_gain(&OPAMP_BETTER, rg, rf);
            assert!(
                (real - ideal).abs() > (better - ideal).abs(),
                "Rg={rg} Rf={rf}: Ag=100 {real}, Ag=1e4 {better}, ideal {ideal}"
            );
        }
    }

    /// The point of modelling finite gain at all: a real op-amp cannot deliver
    /// the ideal closed-loop gain, and the shortfall grows as the demanded gain
    /// approaches the open-loop gain. With `Ag = 100`, asking for 48× gets
    /// noticeably less — that is the boundary behaviour an ideal virtual short
    /// simply does not have.
    #[test]
    fn finite_open_loop_gain_falls_short() {
        let (rg, rf) = (4_700.0f32, 220_000.0f32);
        let ideal = 1.0 + rf / rg;
        let real = non_inverting_gain(&OPAMP_REAL, rg, rf);
        assert!(real < ideal, "{real} should undershoot {ideal}");
        assert!(real > 0.5 * ideal, "but not collapse: {real} vs {ideal}");
    }

    // ---- RT behaviour ------------------------------------------------------

    /// Silence in, silence out — exactly, with no state anywhere leaking.
    #[test]
    fn silence_stays_silent() {
        let mut rt: RType<4, 3, _> = RType::new(
            &PARALLEL4_J,
            (
                ResistiveVoltageSource::new(2_200.0),
                Capacitor::new(22e-9, SR),
                Resistor::new(47_000.0),
            ),
        );
        rt.calc_impedance();
        for _ in 0..1_000 {
            let b = rt.reflected();
            assert_eq!(b, 0.0);
            rt.incident(b);
        }
    }

    /// RT rule 7: a slammed, alternating drive must stay finite and bounded.
    #[test]
    fn bounded_when_slammed() {
        let mut rt: RType<4, 3, _> = RType::new(
            &PARALLEL4_J,
            (
                ResistiveVoltageSource::new(2_200.0),
                Capacitor::new(22e-9, SR),
                Resistor::new(47_000.0),
            ),
        );
        rt.calc_impedance();
        for k in 0..5_000 {
            rt.ports_mut()
                .0
                .set_voltage(if k % 2 == 0 { 1e6 } else { -1e6 });
            let b = rt.reflected();
            assert!(b.is_finite(), "k={k}");
            assert!(b.abs() < 1e7, "k={k}: {b}");
            rt.incident(b);
        }
    }

    /// The matrix follows a moving pot, and only when asked to: reading a
    /// child's resistance is not enough, `calc_impedance` is. That is the
    /// settled-skip contract the block-rate rebuild depends on.
    #[test]
    fn the_matrix_follows_a_moving_pot_on_demand() {
        let mut rt: RType<3, 2, _> = RType::new(
            &PARALLEL_J,
            (Resistor::new(10_000.0), Resistor::new(10_000.0)),
        );
        rt.calc_impedance();
        let before = *rt.s_matrix();
        let r_before = rt.resistance();

        rt.ports_mut().1.set_ohms(1_000.0);
        assert_eq!(*rt.s_matrix(), before, "stale until recomputed");
        assert_eq!(rt.resistance(), r_before);

        rt.calc_impedance();
        assert_ne!(*rt.s_matrix(), before, "recomputed");
        // 10k ‖ 1k ≈ 909 Ω.
        assert!(
            (rt.resistance() - 909.09).abs() < 1.0,
            "{}",
            rt.resistance()
        );
    }

    /// Rate changes must reach the capacitors inside the ports, and the matrix
    /// must be rebuilt from the new impedances.
    #[test]
    fn prepare_reaches_the_ports() {
        let mut rt: RType<3, 2, _> = RType::new(
            &PARALLEL_J,
            (Resistor::new(10_000.0), Capacitor::new(100e-9, 48_000.0)),
        );
        rt.calc_impedance();
        let at_48k = rt.resistance();
        rt.prepare(96_000.0);
        rt.calc_impedance();
        // Doubling the rate halves the capacitor's port resistance, so the
        // parallel combination drops.
        assert!(rt.resistance() < at_48k, "{} vs {at_48k}", rt.resistance());
        let g = 1.0 / 10_000.0 + 2.0 * 100e-9 * 96_000.0;
        assert!((rt.resistance() - 1.0 / g).abs() / rt.resistance() < 1e-4);
    }

    /// A junction with an internal resistor is *not* lossless, and must not
    /// pretend to be: the reflected power is strictly less than the incident
    /// power. This is the counterpart to `wire_junctions_conserve_power` — it
    /// pins that internal elements really do enter the matrix.
    #[test]
    #[allow(clippy::needless_range_loop)]
    fn a_lossy_junction_absorbs_power() {
        static LOSSY: Junction = Junction {
            nodes: 3,
            els: &[JEl::Res {
                a: 1,
                b: 2,
                ohms: 1_000.0,
            }],
            ports: &[(1, 0), (2, 0), (2, 0)],
        };
        let child = [1_000.0f32, 1_000.0];
        let mut s = [[0.0f32; 3]; 3];
        let r_up = adapted_scattering::<3>(&LOSSY, &child, &mut s);
        let r = [r_up, child[0], child[1]];

        let a = [1.0f32, -0.4, 0.7];
        let mut b = [0.0f32; 3];
        for row in 0..3 {
            b[row] = (0..3).map(|k| s[row][k] * a[k]).sum();
        }
        let pin: f32 = (0..3).map(|k| a[k] * a[k] / r[k]).sum();
        let pout: f32 = (0..3).map(|k| b[k] * b[k] / r[k]).sum();
        assert!(pout < pin, "lossy junction: in {pin}, out {pout}");
        assert!(pout > 0.0);
    }

    /// Series and parallel junctions of the *same* ports must differ — a guard
    /// against a construction that ignores the netlist and returns something
    /// generic.
    #[test]
    fn topology_actually_matters() {
        let child = [1_000.0f32, 4_700.0];
        let mut sp = [[0.0f32; 3]; 3];
        let mut ss = [[0.0f32; 3]; 3];
        let rp = adapted_scattering::<3>(&PARALLEL_J, &child, &mut sp);
        let rs = adapted_scattering::<3>(&SERIES_J, &child, &mut ss);
        assert!(rp < rs, "parallel {rp} must be below series {rs}");
        assert!(
            (0..3).any(|r| (0..3).any(|c| (sp[r][c] - ss[r][c]).abs() > 0.1)),
            "the two matrices are indistinguishable"
        );
    }

    /// A three-element series chain expressed as an R-type must track the
    /// nested [`Series`] tree, reactive state included — the series counterpart
    /// to `rtype_tracks_the_equivalent_adaptor_tree`.
    ///
    /// Note the [`PolarityInverter`]: a series adaptor reflects `−(a₁ + a₂)`,
    /// so nesting one inside another flips the inner subtree's polarity, and
    /// `Series<A, Series<B, C>>` is *not* the flat three-element chain. This is
    /// exactly what the inverter is for, and getting it wrong is the easiest
    /// mistake to make when porting a topology — so the flat R-type junction
    /// is the reference that catches it.
    #[test]
    fn rtype_tracks_a_series_tree() {
        static SERIES4_J: Junction = Junction {
            nodes: 4,
            els: &[],
            ports: &[(1, 2), (2, 3), (3, 0), (0, 1)],
        };
        let mut rt: RType<4, 3, _> = RType::new(
            &SERIES4_J,
            (
                ResistiveVoltageSource::new(1_000.0),
                Capacitor::new(47e-9, SR),
                Resistor::new(4_700.0),
            ),
        );
        let mut tree = Series::new(
            ResistiveVoltageSource::new(1_000.0),
            PolarityInverter::new(Series::new(
                Capacitor::new(47e-9, SR),
                Resistor::new(4_700.0),
            )),
        );
        rt.calc_impedance();
        tree.calc_impedance();
        assert!((rt.resistance() - tree.resistance()).abs() / tree.resistance() < 1e-4);

        for k in 0..1_000 {
            let e = 2.0 * (k as f32 * 0.037).sin();
            rt.ports_mut().0.set_voltage(e);
            tree.port1_mut().set_voltage(e);
            let (br, bt) = (rt.reflected(), tree.reflected());
            assert!(
                (br - bt).abs() < 2e-4 * br.abs().max(1.0),
                "k={k}: {br} vs {bt}"
            );
            rt.incident(br);
            tree.incident(bt);
        }
    }
}

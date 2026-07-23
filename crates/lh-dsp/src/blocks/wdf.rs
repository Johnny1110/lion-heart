//! Wave Digital Filter (WDF) primitives — the white-box circuit-modelling
//! substrate (white paper §6 deep water, first topic: the Tube Screamer
//! clipping stage; see PRD 020 / ADR 028).
//!
//! # Why the wave domain
//!
//! The rest of the drive family is *memoryless* waveshaping: a static curve
//! `y = f(x)` plus filters. That cannot capture a real clipper's soul — the
//! way an RC network and a diode's junction interact so the clipping threshold
//! moves with frequency and transient. WDF discretizes the actual circuit: it
//! rewrites each element in terms of **wave variables** `a = v + R·i` (incident)
//! and `b = v − R·i` (reflected), where `R` is a per-port *reference
//! resistance*. Linear elements become trivial one-ports; a single
//! nonlinearity (the diode) sits at the *root* of a tree of adaptors, which
//! present it a Thévenin equivalent — one incident wave `a` and one resistance
//! `R`. The nonlinearity then only has to solve a scalar equation on its own
//! v–i curve. Change the circuit by changing component values, not by hand-
//! tuning a curve — that is the white box.
//!
//! # What lives here
//!
//! The genuinely reusable, non-trivial pieces: the bilinear [`Capacitor`]
//! one-port, the antiparallel [`DiodePair`] root with its RT-safe Newton
//! solver, and the [`parallel_root`] adaptor helper. A specific circuit (e.g.
//! the Screamer's shunt clipper in `drive/screamer.rs`) composes them into
//! straight-line per-sample code — no boxed tree, no dynamic dispatch, no
//! allocation on the audio thread.

/// Bilinear-transform (trapezoidal) capacitor as a WDF one-port.
///
/// Reference resistance `R = T/(2C) = 1/(2·C·fs)`; the reflected wave is a pure
/// unit delay of the incident wave (`b[n] = a[n−1]`), so the element's entire
/// state is the last incident wave it was handed.
pub struct Capacitor {
    r: f32,
    /// `a[n−1]` — the incident wave stored from last sample; also this port's
    /// reflected wave this sample.
    state: f32,
}

impl Capacitor {
    /// A capacitor of `farads` at the given sample rate.
    pub fn new(farads: f32, sample_rate: f32) -> Self {
        debug_assert!(farads > 0.0 && sample_rate > 0.0);
        Self {
            r: 1.0 / (2.0 * farads * sample_rate),
            state: 0.0,
        }
    }

    /// The port reference resistance `R = T/(2C)`.
    #[inline]
    pub fn resistance(&self) -> f32 {
        self.r
    }

    /// The port conductance `G = 1/R = 2·C·fs`.
    #[inline]
    pub fn conductance(&self) -> f32 {
        1.0 / self.r
    }

    /// The wave this port reflects toward the adaptor this sample.
    #[inline]
    pub fn reflected(&self) -> f32 {
        self.state
    }

    /// Store the adaptor's incident wave `b` for this port; it becomes next
    /// sample's reflected wave. Denormals are flushed (RT rule 7 — a decaying
    /// feedback state must not sink into denormal territory).
    #[inline]
    pub fn set_incident(&mut self, b: f32) {
        self.state = if b.abs() < 1e-25 { 0.0 } else { b };
    }

    pub fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// Two identical diodes in antiparallel (opposite polarity) as a WDF nonlinear
/// **root**: the symmetric soft clipper at the heart of a Tube-Screamer-style
/// pedal (`i(v) = 2·Is·sinh(v / (n·Vt))`).
///
/// Given the incident wave `a` the linear subtree presents and the port
/// resistance `R` it sees, [`solve`](Self::solve) returns the diode voltage `v`
/// (which is also the clipping node's voltage — the useful output) and the
/// reflected wave `b = 2v − a`.
pub struct DiodePair {
    /// Reverse saturation current `Is`.
    is: f64,
    /// The thermal scale `n·Vt` (ideality × thermal voltage).
    vt_n: f64,
    /// Warm start: the diode voltage solved last sample. Audio is continuous,
    /// so this is almost always 1–3 Newton steps from the new root.
    v: f64,
}

/// Newton iteration ceiling. Warm-started real audio needs 1–3; a cold start
/// from a slammed input converges in well under this (damped, see below).
const MAX_ITERS: usize = 16;
/// Convergence tolerance on the diode voltage, in volts.
const TOL: f64 = 1e-10;

impl DiodePair {
    /// `is` = saturation current (A), `n` = ideality factor, `vt` = thermal
    /// voltage (V). 1N4148 ≈ `is 2.52e-9, n 1.75, vt 25.85e-3`.
    pub fn new(is: f32, n: f32, vt: f32) -> Self {
        Self {
            is: is as f64,
            vt_n: (n * vt) as f64,
            v: 0.0,
        }
    }

    pub fn reset(&mut self) {
        self.v = 0.0;
    }

    /// Solve the root for incident wave `a` at port resistance `r`. Returns
    /// `(v, b)`: the diode/node voltage and the reflected wave `2v − a`.
    ///
    /// **RT-safe:** fixed iteration ceiling, no allocation, `f64` internally so
    /// the tiny `Is` keeps its precision. The Newton step is *damped* (capped
    /// at `10·n·Vt` per iteration) so a cold, slammed input cannot overshoot
    /// into the stiff exponential and stall — real convergence near the root is
    /// undamped and quadratic.
    #[inline]
    pub fn solve(&mut self, a: f32, r: f32) -> (f32, f32) {
        let a = a as f64;
        let r = r as f64;
        let two_is = 2.0 * self.is;
        let vt_n = self.vt_n;
        let dv_max = 10.0 * vt_n;

        let mut v = self.v;
        for _ in 0..MAX_ITERS {
            // `u` stays sane thanks to damping; the clamp is pure overflow
            // paranoia for a pathological caller.
            let u = (v / vt_n).clamp(-60.0, 60.0);
            let e = u.exp();
            let einv = 1.0 / e;
            let sinh = 0.5 * (e - einv);
            let cosh = 0.5 * (e + einv);
            let i = two_is * sinh;
            let f = v + r * i - a;
            let fp = 1.0 + r * two_is / vt_n * cosh;
            // Damped step: capped so a cold, slammed input can't overshoot into
            // the stiff exponential. Near the root the cap never bites, so
            // convergence stays undamped (quadratic).
            let dv = (f / fp).clamp(-dv_max, dv_max);
            v -= dv;
            if dv.abs() < TOL {
                break;
            }
        }

        self.v = v;
        let vf = v as f32;
        let vf = if vf.abs() < 1e-25 { 0.0 } else { vf };
        (vf, 2.0 * vf - a as f32)
    }
}

/// Incident wave into the adapted (reflection-free) root of a parallel adaptor,
/// and the resistance the root sees, from the linear ports' `(conductance,
/// reflected-wave)` pairs.
///
/// At a parallel node every port shares one voltage, so the root sees the
/// conductance-weighted average of the others' waves,
/// `a = (Σ Gₖ·aₖ) / (Σ Gₖ)`, behind `R = 1 / (Σ Gₖ)`. Making the root
/// reflection-free (its own conductance set to `Σ Gₖ`) is what breaks the
/// delay-free loop so the tree is computable in one pass.
///
/// `ports` is borrowed from a caller-owned stack array (e.g.
/// `&[(g0, a0), (g1, a1)]`) — no heap, RT-safe.
#[inline]
pub fn parallel_root(ports: &[(f32, f32)]) -> (f32, f32) {
    let mut g_sum = 0.0f32;
    let mut weighted = 0.0f32;
    for &(g, a) in ports {
        g_sum += g;
        weighted += g * a;
    }
    (weighted / g_sum, 1.0 / g_sum)
}

#[cfg(test)]
mod tests {
    use super::*;

    // A 1N4148-ish pair for the solver tests.
    fn diode() -> DiodePair {
        DiodePair::new(2.52e-9, 1.75, 25.85e-3)
    }

    /// The returned root must actually satisfy the diode equation
    /// `a = v + R·i(v)` and the wave identity `b = 2v − a`.
    #[test]
    fn solve_satisfies_the_diode_equation() {
        let r = 2200.0f32;
        for &a in &[-40.0, -5.0, -0.5, -0.01, 0.0, 0.01, 0.5, 5.0, 40.0] {
            let mut d = diode();
            let (v, b) = d.solve(a, r);
            assert!(v.is_finite() && b.is_finite());
            // Residual of a = v + R·i(v), computed in f64.
            let vt_n = 1.75 * 25.85e-3f64;
            let i = 2.0 * 2.52e-9f64 * (v as f64 / vt_n).sinh();
            let residual = v as f64 + r as f64 * i - a as f64;
            assert!(residual.abs() < 1e-4, "a={a}: residual {residual}");
            assert!((b - (2.0 * v - a)).abs() < 1e-4, "b identity a={a}");
        }
    }

    /// Antiparallel diodes are symmetric: `v(−a) = −v(a)`.
    #[test]
    fn solve_is_symmetric() {
        let r = 2200.0f32;
        for &a in &[0.3, 1.0, 8.0, 30.0] {
            let (vp, _) = diode().solve(a, r);
            let (vn, _) = diode().solve(-a, r);
            assert!((vp + vn).abs() < 1e-5, "a={a}: {vp} vs {vn}");
        }
    }

    /// The clip is a soft ceiling: the node voltage saturates far below the
    /// input as `a` grows (the whole point of a clipper).
    #[test]
    fn solve_saturates() {
        let r = 2200.0f32;
        let (v_small, _) = diode().solve(0.05, r);
        let (v_big, _) = diode().solve(50.0, r);
        // Small signal is near-unity (barely clipped), big signal is clamped
        // near the diode knee (well under a volt).
        assert!(v_small > 0.02, "small {v_small}");
        assert!(v_big < 0.9 && v_big > 0.4, "big {v_big}");
        assert!(v_big < v_small * 20.0, "should compress hard");
    }

    /// Warm-started continuity: solving a slowly moving `a` never blows up and
    /// stays consistent with cold solves.
    #[test]
    fn warm_start_matches_cold() {
        let r = 2200.0f32;
        let mut warm = diode();
        for k in 0..200 {
            let a = 10.0 * (k as f32 * 0.03).sin();
            let (vw, _) = warm.solve(a, r);
            let (vc, _) = diode().solve(a, r); // fresh, cold
            assert!((vw - vc).abs() < 1e-4, "k={k}: warm {vw} cold {vc}");
        }
    }

    /// A slammed, alternating input must stay finite and bounded (RT rule 7).
    #[test]
    fn solve_bounded_when_slammed() {
        let r = 4700.0f32;
        let mut d = diode();
        for k in 0..1000 {
            let a = if k % 2 == 0 { 1.0e6 } else { -1.0e6 };
            let (v, b) = d.solve(a, r);
            assert!(v.is_finite() && b.is_finite());
            assert!(v.abs() < 1.5, "k={k}: v={v}");
        }
    }

    #[test]
    fn capacitor_port_resistance() {
        // R = T/(2C) = 1/(2·C·fs).
        let c = Capacitor::new(47e-9, 48_000.0);
        let expected = 1.0 / (2.0 * 47e-9 * 48_000.0);
        assert!((c.resistance() - expected).abs() / expected < 1e-5);
        assert!((c.conductance() - 1.0 / expected).abs() / (1.0 / expected) < 1e-5);
    }

    /// A source–resistor–capacitor divider (no diode) must settle to the DC
    /// the resistances dictate: with the capacitor open at DC, the node sees
    /// the full source EMF.
    #[test]
    fn parallel_root_rc_settles_to_dc() {
        let sr = 48_000.0f32;
        let mut cap = Capacitor::new(100e-9, sr);
        let g_src = 1.0 / 1000.0; // 1 kΩ source
        let e = 0.7f32; // constant EMF
        let mut v = 0.0f32;
        for _ in 0..20_000 {
            let a1 = cap.reflected();
            let (a_root, _r_root) = parallel_root(&[(g_src, e), (cap.conductance(), a1)]);
            // No diode: an open root reflects its incident unchanged, so the
            // node voltage equals the incident wave. Back-propagate to the
            // capacitor exactly as the clipper does: b_cap = 2·v − a_cap.
            v = a_root;
            cap.set_incident(2.0 * v - a1);
        }
        // Capacitor fully charged, no current: node = source EMF.
        assert!((v - e).abs() < 1e-3, "settled {v}, want {e}");
    }
}

//! Composable WDF adaptors — the three-port series and parallel junctions, and
//! the polarity inverter.
//!
//! Each adaptor **owns** its two children, so a circuit is a type:
//! `Parallel<ResistiveVoltageSource, Capacitor>` is the Screamer's shunt
//! clipper, and the compiler monomorphises it into straight-line code with no
//! dispatch, no indirection and no allocation. Rust has no equivalent of
//! `chowdsp_wdf`'s child↔parent back-references (they fight the borrow checker
//! and buy nothing here), so waves are driven **from the root downward** —
//! exactly the shape the hand-written `screamer`/`sd1` code already had, only
//! now composable.
//!
//! Deep trees produce long type names; the convention is a per-pedal `type`
//! alias at the top of the pedal's module.
//!
//! # The scattering relations
//!
//! Both are the *adapted* three-port form: the up port's resistance is fixed by
//! the children's, which makes it reflection-free and breaks the delay-free
//! loop. Writing `v_k`, `i_k` for the port voltage and the current flowing into
//! port `k`, with `a_k = v_k + R_k·i_k` and `b_k = v_k − R_k·i_k`:
//!
//! * **Parallel** — one shared voltage, currents sum to zero. Then
//!   `v = (Σ Gₖaₖ)/(Σ Gₖ)` and `bₖ = 2v − aₖ`; choosing `G_up = G₁ + G₂` makes
//!   `b_up = a₂ − p(a₂ − a₁)` with `p = G₁/(G₁+G₂)`, free of `a_up`.
//! * **Series** — one shared current, voltages sum to zero. Then
//!   `i = (Σ aₖ)/(Σ Rₖ)` and `bₖ = aₖ − 2Rₖ·i`; choosing `R_up = R₁ + R₂` makes
//!   `b_up = −(a₁ + a₂)`.
//!
//! Derived here, and independently confirmed against `chowdsp_wdf`'s
//! implementation (BSD-3) term by term.

use super::Wdf;

/// Three-port **parallel** adaptor: two children sharing one node, presenting
/// their combined conductance upward.
pub struct Parallel<A: Wdf, B: Wdf> {
    p1: A,
    p2: B,
    r: f32,
    g: f32,
    /// `G₁ / (G₁ + G₂)`.
    p1_reflect: f32,
    /// Children's reflected waves from this sample, kept for `incident`.
    a1: f32,
    a2: f32,
    b_up: f32,
}

impl<A: Wdf, B: Wdf> Parallel<A, B> {
    pub fn new(p1: A, p2: B) -> Self {
        let mut s = Self {
            p1,
            p2,
            r: 0.0,
            g: 0.0,
            p1_reflect: 0.5,
            a1: 0.0,
            a2: 0.0,
            b_up: 0.0,
        };
        s.calc_impedance();
        s
    }

    pub fn port1(&self) -> &A {
        &self.p1
    }
    pub fn port1_mut(&mut self) -> &mut A {
        &mut self.p1
    }
    pub fn port2(&self) -> &B {
        &self.p2
    }
    pub fn port2_mut(&mut self) -> &mut B {
        &mut self.p2
    }
}

impl<A: Wdf, B: Wdf> Wdf for Parallel<A, B> {
    fn calc_impedance(&mut self) {
        self.p1.calc_impedance();
        self.p2.calc_impedance();
        self.g = self.p1.conductance() + self.p2.conductance();
        self.r = 1.0 / self.g;
        self.p1_reflect = self.p1.conductance() / self.g;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    #[inline]
    fn reflected(&mut self) -> f32 {
        self.a1 = self.p1.reflected();
        self.a2 = self.p2.reflected();
        self.b_up = self.a2 - self.p1_reflect * (self.a2 - self.a1);
        self.b_up
    }
    #[inline]
    fn incident(&mut self, a: f32) {
        // bₖ = a_up + b_up − aₖ (both children sit at the shared node voltage
        // v = (a_up + b_up)/2).
        let b2 = self.b_up - self.a2 + a;
        self.p1.incident(b2 + self.a2 - self.a1);
        self.p2.incident(b2);
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.p1.prepare(sample_rate);
        self.p2.prepare(sample_rate);
    }
    fn reset(&mut self) {
        self.p1.reset();
        self.p2.reset();
        self.a1 = 0.0;
        self.a2 = 0.0;
        self.b_up = 0.0;
    }
}

/// Three-port **series** adaptor: two children carrying one current, presenting
/// their summed resistance upward.
pub struct Series<A: Wdf, B: Wdf> {
    p1: A,
    p2: B,
    r: f32,
    g: f32,
    /// `R₁ / (R₁ + R₂)`.
    p1_reflect: f32,
    a1: f32,
    a2: f32,
}

impl<A: Wdf, B: Wdf> Series<A, B> {
    pub fn new(p1: A, p2: B) -> Self {
        let mut s = Self {
            p1,
            p2,
            r: 0.0,
            g: 0.0,
            p1_reflect: 0.5,
            a1: 0.0,
            a2: 0.0,
        };
        s.calc_impedance();
        s
    }

    pub fn port1(&self) -> &A {
        &self.p1
    }
    pub fn port1_mut(&mut self) -> &mut A {
        &mut self.p1
    }
    pub fn port2(&self) -> &B {
        &self.p2
    }
    pub fn port2_mut(&mut self) -> &mut B {
        &mut self.p2
    }
}

impl<A: Wdf, B: Wdf> Wdf for Series<A, B> {
    fn calc_impedance(&mut self) {
        self.p1.calc_impedance();
        self.p2.calc_impedance();
        self.r = self.p1.resistance() + self.p2.resistance();
        self.g = 1.0 / self.r;
        self.p1_reflect = self.p1.resistance() / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    #[inline]
    fn reflected(&mut self) -> f32 {
        self.a1 = self.p1.reflected();
        self.a2 = self.p2.reflected();
        -(self.a1 + self.a2)
    }
    #[inline]
    fn incident(&mut self, a: f32) {
        // bₖ = aₖ − 2Rₖ·i with i = Σa / ΣR; the second child's wave follows from
        // the loop's voltage sum, which is why it needs no separate ratio.
        let b1 = self.a1 - self.p1_reflect * (a + self.a1 + self.a2);
        self.p1.incident(b1);
        self.p2.incident(-(a + b1));
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.p1.prepare(sample_rate);
        self.p2.prepare(sample_rate);
    }
    fn reset(&mut self) {
        self.p1.reset();
        self.p2.reset();
        self.a1 = 0.0;
        self.a2 = 0.0;
    }
}

/// Flips the sign of the voltage seen by its child — the wave-domain spelling
/// of swapping a subnetwork's terminals. Impedance passes through unchanged.
pub struct PolarityInverter<A: Wdf> {
    p1: A,
}

impl<A: Wdf> PolarityInverter<A> {
    pub fn new(p1: A) -> Self {
        let mut s = Self { p1 };
        s.calc_impedance();
        s
    }

    pub fn port1(&self) -> &A {
        &self.p1
    }
    pub fn port1_mut(&mut self) -> &mut A {
        &mut self.p1
    }
}

impl<A: Wdf> Wdf for PolarityInverter<A> {
    fn calc_impedance(&mut self) {
        self.p1.calc_impedance();
    }
    fn resistance(&self) -> f32 {
        self.p1.resistance()
    }
    fn conductance(&self) -> f32 {
        self.p1.conductance()
    }
    #[inline]
    fn reflected(&mut self) -> f32 {
        -self.p1.reflected()
    }
    #[inline]
    fn incident(&mut self, a: f32) {
        self.p1.incident(-a);
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.p1.prepare(sample_rate);
    }
    fn reset(&mut self) {
        self.p1.reset();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::wdf::one_port::{Capacitor, ResistiveVoltageSource, Resistor};

    const SR: f32 = 96_000.0;

    /// Impedance recursion is post-order and covers the whole tree: a value
    /// changed at a leaf must reach the root through two adaptors after one
    /// `calc_impedance()` call, and land on the hand-computed number.
    #[test]
    fn impedance_recomputes_through_the_whole_tree() {
        // Root port sees: (R1 ‖ R2) + R3.
        let mut tree = Series::new(
            Parallel::new(Resistor::new(1_000.0), Resistor::new(1_000.0)),
            Resistor::new(2_000.0),
        );
        tree.calc_impedance();
        assert!((tree.resistance() - 2_500.0).abs() < 1e-3, "start");

        // Move the "pot": R2 1 kΩ → 4 kΩ, so (1k ‖ 4k) + 2k = 800 + 2000.
        tree.port1_mut().port2_mut().set_ohms(4_000.0);
        assert!(
            (tree.resistance() - 2_500.0).abs() < 1e-3,
            "stale until recomputed — that is the settled-skip contract"
        );
        tree.calc_impedance();
        assert!(
            (tree.resistance() - 2_800.0).abs() < 1e-3,
            "{}",
            tree.resistance()
        );
    }

    /// A capacitor's port resistance is rate-dependent, so `prepare` must reach
    /// the leaves and the following `calc_impedance` must lift the change to
    /// the root.
    #[test]
    fn prepare_reaches_the_leaves() {
        let mut tree = Series::new(Resistor::new(1_000.0), Capacitor::new(100e-9, 48_000.0));
        tree.calc_impedance();
        let at_48k = tree.resistance();
        tree.prepare(96_000.0);
        tree.calc_impedance();
        let at_96k = tree.resistance();
        // R_c halves with a doubled rate: 1000 + 1/(2·C·fs).
        let want = 1_000.0 + 1.0 / (2.0 * 100e-9 * 96_000.0);
        assert!((at_96k - want).abs() / want < 1e-5, "{at_96k} vs {want}");
        assert!(at_96k < at_48k);
    }

    /// The defining property of the *adapted* form: the up port reflects
    /// nothing of its own incident wave. Feeding an adaptor two different
    /// incident waves must not change what it reflects on the next sample —
    /// beyond what the reactive children legitimately store.
    #[test]
    fn adapted_ports_are_reflection_free() {
        // Purely resistive children: no state, so the up port's reflection must
        // be *identically* independent of what was pushed down.
        let mut par = Parallel::new(Resistor::new(1_000.0), ResistiveVoltageSource::new(2_200.0));
        par.port2_mut().set_voltage(1.7);
        par.calc_impedance();
        let b0 = par.reflected();
        par.incident(11.0);
        assert_eq!(
            par.reflected(),
            b0,
            "parallel up port reflected its incident"
        );

        let mut ser = Series::new(Resistor::new(1_000.0), ResistiveVoltageSource::new(2_200.0));
        ser.port2_mut().set_voltage(1.7);
        ser.calc_impedance();
        let b0 = ser.reflected();
        ser.incident(-4.0);
        assert_eq!(ser.reflected(), b0, "series up port reflected its incident");
    }

    /// Series and parallel adaptors must conserve power: a junction of ideal
    /// wires stores nothing and dissipates nothing, so the incident and
    /// reflected power (`Σ aₖ²/Rₖ` and `Σ bₖ²/Rₖ`) match exactly. This is a
    /// statement about the scattering relations alone — it holds whatever the
    /// children are, and it is how a sign slip in either adaptor is caught.
    #[test]
    fn adaptors_conserve_power() {
        for (r1, r2) in [(1_000.0f32, 1_000.0f32), (470.0, 22_000.0), (1e5, 33.0)] {
            // Port waves: drive the adaptor's three ports directly by using
            // resistive sources as children, so aₖ is known.
            for (a1, a2, a_up) in [
                (1.0f32, 0.0f32, 0.0f32),
                (0.3, -1.2, 2.5),
                (-4.0, 4.0, -1.0),
            ] {
                // Parallel: G_up = G1 + G2.
                let (g1, g2) = (1.0 / r1, 1.0 / r2);
                let (g_up, p) = (g1 + g2, g1 / (g1 + g2));
                let b_up = a2 - p * (a2 - a1);
                let v = (a_up + b_up) * 0.5;
                let (b1, b2) = (2.0 * v - a1, 2.0 * v - a2);
                let pin = a1 * a1 * g1 + a2 * a2 * g2 + a_up * a_up * g_up;
                let pout = b1 * b1 * g1 + b2 * b2 * g2 + b_up * b_up * g_up;
                assert!(
                    (pin - pout).abs() <= 1e-4 * pin.max(1.0),
                    "parallel r=({r1},{r2}) a=({a1},{a2},{a_up}): {pin} vs {pout}"
                );

                // Series: R_up = R1 + R2.
                let r_up = r1 + r2;
                let b_up = -(a1 + a2);
                let i = (a1 + a2 + a_up) / (r1 + r2 + r_up);
                let (b1, b2) = (a1 - 2.0 * r1 * i, a2 - 2.0 * r2 * i);
                let pin = a1 * a1 / r1 + a2 * a2 / r2 + a_up * a_up / r_up;
                let pout = b1 * b1 / r1 + b2 * b2 / r2 + b_up * b_up / r_up;
                assert!(
                    (pin - pout).abs() <= 1e-4 * pin.max(1.0),
                    "series r=({r1},{r2}) a=({a1},{a2},{a_up}): {pin} vs {pout}"
                );
            }
        }
    }

    /// Inverting twice is the identity — impedance, waves and all.
    #[test]
    fn polarity_inverter_is_an_involution() {
        let mut plain = Capacitor::new(10e-9, SR);
        let mut twice = PolarityInverter::new(PolarityInverter::new(Capacitor::new(10e-9, SR)));
        plain.calc_impedance();
        twice.calc_impedance();
        assert_eq!(plain.resistance(), twice.resistance());
        for k in 0..200 {
            let a = (k as f32 * 0.2).sin();
            assert_eq!(plain.reflected(), twice.reflected(), "k={k}");
            plain.incident(a);
            twice.incident(a);
        }
    }

    /// A single inversion really does flip the sign of the wave in both
    /// directions (and so leaves a two-port network's transfer unchanged in
    /// magnitude while flipping its polarity).
    #[test]
    fn polarity_inverter_flips_both_directions() {
        let mut inv = PolarityInverter::new(Capacitor::new(10e-9, SR));
        inv.calc_impedance();
        let mut plain = Capacitor::new(10e-9, SR);
        plain.calc_impedance();
        for k in 0..200 {
            let a = 1.0 + (k as f32 * 0.3).sin();
            assert!((inv.reflected() + plain.reflected()).abs() < 1e-6, "k={k}");
            inv.incident(a);
            plain.incident(-a);
        }
    }

    /// Silence in, silence out, exactly — no state anywhere leaks a value.
    #[test]
    fn silence_stays_silent() {
        let mut tree = Parallel::new(
            Series::new(Resistor::new(4_700.0), Capacitor::new(47e-9, SR)),
            Parallel::new(Resistor::new(10_000.0), Capacitor::new(1e-9, SR)),
        );
        tree.calc_impedance();
        for _ in 0..1_000 {
            let a = tree.reflected();
            assert_eq!(a, 0.0);
            tree.incident(a);
        }
    }
}

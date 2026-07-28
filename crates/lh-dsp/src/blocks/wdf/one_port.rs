//! WDF one-ports — the leaves of the tree.
//!
//! Each is a single element (or a hand-reduced composite of two) presenting one
//! port to its parent. Rewritten in Rust from `chowdsp_wdf`'s
//! `wdft_one_ports.h` / `wdft_sources.h` (BSD-3); the scattering relations are
//! re-derived here rather than transcribed, and the composites are pinned
//! against the equivalent [`super::adaptor`] tree by test.
//!
//! # Composites are fast paths, not new physics
//!
//! [`ResistorCapacitorSeries`] is exactly `Series<Resistor, Capacitor>` and
//! [`ResistorCapacitorParallel`] exactly `Parallel<Resistor, Capacitor>` — they
//! exist only because those two shapes are everywhere in pedal schematics and
//! the reduced form skips an adaptor's worth of arithmetic per sample.
//! `rc_composites_match_the_generic_trees` holds them to that claim. Anything
//! not listed here is still expressible as a tree: an R–C–source leg, for
//! instance, is `Series<Resistor, CapacitiveVoltageSource>`.

use super::{Wdf, flush};

/// A resistor: reflection-free (`b = 0`), because a resistor terminated in its
/// own resistance absorbs everything.
pub struct Resistor {
    r: f32,
    g: f32,
}

impl Resistor {
    pub fn new(ohms: f32) -> Self {
        debug_assert!(ohms > 0.0);
        Self {
            r: ohms,
            g: 1.0 / ohms,
        }
    }

    /// Set the resistance (a pot wiper moving). Takes effect on the next
    /// [`Wdf::calc_impedance`] pass — block boundary, never per sample.
    pub fn set_ohms(&mut self, ohms: f32) {
        debug_assert!(ohms > 0.0);
        self.r = ohms;
    }

    pub fn ohms(&self) -> f32 {
        self.r
    }
}

impl Wdf for Resistor {
    fn calc_impedance(&mut self) {
        self.g = 1.0 / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        0.0
    }
    fn incident(&mut self, _a: f32) {}
    fn reset(&mut self) {}
}

/// Bilinear-transform (trapezoidal) capacitor as a WDF one-port.
///
/// Reference resistance `R = T/(2C) = 1/(2·C·fs)`; the reflected wave is a pure
/// unit delay of the incident wave (`b[n] = a[n−1]`), so the element's entire
/// state is the last incident wave it was handed.
pub struct Capacitor {
    farads: f32,
    fs: f32,
    r: f32,
    g: f32,
    /// `a[n−1]` — the incident wave stored from last sample; also this port's
    /// reflected wave this sample.
    state: f32,
}

impl Capacitor {
    /// A capacitor of `farads` at the given sample rate.
    pub fn new(farads: f32, sample_rate: f32) -> Self {
        debug_assert!(farads > 0.0 && sample_rate > 0.0);
        let r = 1.0 / (2.0 * farads * sample_rate);
        Self {
            farads,
            fs: sample_rate,
            r,
            g: 1.0 / r,
            state: 0.0,
        }
    }

    pub fn set_farads(&mut self, farads: f32) {
        debug_assert!(farads > 0.0);
        self.farads = farads;
    }
}

impl Wdf for Capacitor {
    fn calc_impedance(&mut self) {
        self.r = 1.0 / (2.0 * self.farads * self.fs);
        self.g = 1.0 / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        self.state
    }
    /// Store the adaptor's incident wave `b` for this port; it becomes next
    /// sample's reflected wave. Denormals are flushed (RT rule 7 — a decaying
    /// feedback state must not sink into denormal territory).
    fn incident(&mut self, a: f32) {
        self.state = flush(a);
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.fs = sample_rate;
        self.calc_impedance();
        self.reset();
    }
    fn reset(&mut self) {
        self.state = 0.0;
    }
}

/// A resistor in series with a capacitor, reduced to one port.
///
/// `R_port = R + T/(2C)`; the state is the internal capacitor's, so the
/// reflected wave is `−z` (the sign is the series adaptor's, inherited).
pub struct ResistorCapacitorSeries {
    ohms: f32,
    farads: f32,
    fs: f32,
    r: f32,
    g: f32,
    /// `T / (2RC + T)` — the fraction of `(a + z)` the capacitor absorbs.
    k: f32,
    z: f32,
}

impl ResistorCapacitorSeries {
    pub fn new(ohms: f32, farads: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            ohms,
            farads,
            fs: sample_rate,
            r: 0.0,
            g: 0.0,
            k: 0.0,
            z: 0.0,
        };
        s.calc_impedance();
        s
    }

    pub fn set_ohms(&mut self, ohms: f32) {
        self.ohms = ohms;
    }
    pub fn set_farads(&mut self, farads: f32) {
        self.farads = farads;
    }
}

impl Wdf for ResistorCapacitorSeries {
    fn calc_impedance(&mut self) {
        let t = 1.0 / self.fs;
        self.r = t / (2.0 * self.farads) + self.ohms;
        self.g = 1.0 / self.r;
        self.k = t / (2.0 * self.ohms * self.farads + t);
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        -self.z
    }
    fn incident(&mut self, a: f32) {
        self.z = flush(self.z - self.k * (a + self.z));
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.fs = sample_rate;
        self.calc_impedance();
        self.reset();
    }
    fn reset(&mut self) {
        self.z = 0.0;
    }
}

/// A resistor in parallel with a capacitor, reduced to one port.
///
/// `R_port = R ‖ T/(2C)`.
pub struct ResistorCapacitorParallel {
    ohms: f32,
    farads: f32,
    fs: f32,
    r: f32,
    g: f32,
    /// `2RC / (2RC + T)` — the fraction of the state that reflects.
    k: f32,
    z: f32,
    b: f32,
}

impl ResistorCapacitorParallel {
    pub fn new(ohms: f32, farads: f32, sample_rate: f32) -> Self {
        let mut s = Self {
            ohms,
            farads,
            fs: sample_rate,
            r: 0.0,
            g: 0.0,
            k: 0.0,
            z: 0.0,
            b: 0.0,
        };
        s.calc_impedance();
        s
    }

    pub fn set_ohms(&mut self, ohms: f32) {
        self.ohms = ohms;
    }
    pub fn set_farads(&mut self, farads: f32) {
        self.farads = farads;
    }
}

impl Wdf for ResistorCapacitorParallel {
    fn calc_impedance(&mut self) {
        let t = 1.0 / self.fs;
        let two_rc = 2.0 * self.ohms * self.farads;
        self.r = self.ohms * t / (two_rc + t);
        self.g = 1.0 / self.r;
        self.k = two_rc / (two_rc + t);
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        self.b = self.k * self.z;
        self.b
    }
    fn incident(&mut self, a: f32) {
        self.z = flush(self.b + a - self.z);
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.fs = sample_rate;
        self.calc_impedance();
        self.reset();
    }
    fn reset(&mut self) {
        self.z = 0.0;
        self.b = 0.0;
    }
}

/// An ideal voltage source `e` behind its own internal resistance `R`
/// (a Thévenin source). Its port reflects the EMF unchanged: `b = e`.
///
/// A half-rail bias supply is exactly this with `e = 4.5 V` — no separate
/// mechanism needed (the pedal's output DC blocker removes the offset, as the
/// family already does).
pub struct ResistiveVoltageSource {
    r: f32,
    g: f32,
    e: f32,
}

impl ResistiveVoltageSource {
    pub fn new(ohms: f32) -> Self {
        debug_assert!(ohms > 0.0);
        Self {
            r: ohms,
            g: 1.0 / ohms,
            e: 0.0,
        }
    }

    /// Set the EMF. Free to change every sample — it is not an impedance.
    #[inline]
    pub fn set_voltage(&mut self, e: f32) {
        self.e = e;
    }

    pub fn set_ohms(&mut self, ohms: f32) {
        debug_assert!(ohms > 0.0);
        self.r = ohms;
    }
}

impl Wdf for ResistiveVoltageSource {
    fn calc_impedance(&mut self) {
        self.g = 1.0 / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        self.e
    }
    fn incident(&mut self, _a: f32) {}
    fn reset(&mut self) {}
}

/// A current source `I` across its own internal resistance `R` (a Norton
/// source). Thévenin–Norton duality makes its reflected wave `b = R·I` — the
/// same shape as [`ResistiveVoltageSource`], which is what `e = R·I` means.
///
/// An *ideal* current source (`R → ∞`) has no one-port form; injected at the
/// root it is a shift of the incident wave instead — see
/// [`parallel_root_with_source`](super::parallel_root_with_source).
pub struct ResistiveCurrentSource {
    r: f32,
    g: f32,
    i: f32,
}

impl ResistiveCurrentSource {
    pub fn new(ohms: f32) -> Self {
        debug_assert!(ohms > 0.0);
        Self {
            r: ohms,
            g: 1.0 / ohms,
            i: 0.0,
        }
    }

    #[inline]
    pub fn set_current(&mut self, i: f32) {
        self.i = i;
    }

    pub fn set_ohms(&mut self, ohms: f32) {
        debug_assert!(ohms > 0.0);
        self.r = ohms;
    }
}

impl Wdf for ResistiveCurrentSource {
    fn calc_impedance(&mut self) {
        self.g = 1.0 / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        self.r * self.i
    }
    fn incident(&mut self, _a: f32) {}
    fn reset(&mut self) {}
}

/// A capacitor in series with a voltage source — the standard way to inject a
/// bias rail that the circuit's own DC path must charge through.
///
/// The source only enters as its *change*: `b[n] = a[n−1] + e[n] − e[n−1]`.
/// That falls straight out of `v = e + v_C` — a constant EMF in series with a
/// capacitor is invisible once the capacitor has charged, which is exactly what
/// a real coupling cap does.
pub struct CapacitiveVoltageSource {
    farads: f32,
    fs: f32,
    r: f32,
    g: f32,
    z: f32,
    e: f32,
    e_prev: f32,
}

impl CapacitiveVoltageSource {
    pub fn new(farads: f32, sample_rate: f32) -> Self {
        debug_assert!(farads > 0.0 && sample_rate > 0.0);
        let r = 1.0 / (2.0 * farads * sample_rate);
        Self {
            farads,
            fs: sample_rate,
            r,
            g: 1.0 / r,
            z: 0.0,
            e: 0.0,
            e_prev: 0.0,
        }
    }

    #[inline]
    pub fn set_voltage(&mut self, e: f32) {
        self.e = e;
    }

    pub fn set_farads(&mut self, farads: f32) {
        self.farads = farads;
    }
}

impl Wdf for CapacitiveVoltageSource {
    fn calc_impedance(&mut self) {
        self.r = 1.0 / (2.0 * self.farads * self.fs);
        self.g = 1.0 / self.r;
    }
    fn resistance(&self) -> f32 {
        self.r
    }
    fn conductance(&self) -> f32 {
        self.g
    }
    fn reflected(&mut self) -> f32 {
        let b = self.z + self.e - self.e_prev;
        self.e_prev = self.e;
        b
    }
    fn incident(&mut self, a: f32) {
        self.z = flush(a);
    }
    fn prepare(&mut self, sample_rate: f32) {
        self.fs = sample_rate;
        self.calc_impedance();
        self.reset();
    }
    fn reset(&mut self) {
        self.z = 0.0;
        self.e_prev = 0.0;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::wdf::adaptor::{Parallel, Series};

    const SR: f32 = 192_000.0;

    #[test]
    fn capacitor_port_resistance() {
        // R = T/(2C) = 1/(2·C·fs).
        let c = Capacitor::new(47e-9, 48_000.0);
        let expected = 1.0 / (2.0 * 47e-9 * 48_000.0);
        assert!((c.resistance() - expected).abs() / expected < 1e-5);
        assert!((c.conductance() - 1.0 / expected).abs() / (1.0 / expected) < 1e-5);
    }

    #[test]
    fn resistor_is_reflection_free() {
        let mut r = Resistor::new(4700.0);
        r.calc_impedance();
        assert_eq!(r.resistance(), 4700.0);
        for &a in &[0.0f32, 1.0, -30.0] {
            r.incident(a);
            assert_eq!(r.reflected(), 0.0);
        }
    }

    /// The composites must *be* their trees, not merely resemble them: same
    /// port resistance, same reflected wave, sample after sample, driven by the
    /// same root. This is what lets the reduced forms stay as fast paths
    /// without becoming a second implementation that can drift.
    #[test]
    fn rc_composites_match_the_generic_trees() {
        let (r, c) = (10_000.0f32, 47e-9f32);

        let mut fast_s = ResistorCapacitorSeries::new(r, c, SR);
        let mut tree_s = Series::new(Resistor::new(r), Capacitor::new(c, SR));
        let mut fast_p = ResistorCapacitorParallel::new(r, c, SR);
        let mut tree_p = Parallel::new(Resistor::new(r), Capacitor::new(c, SR));
        for n in [
            &mut fast_s as &mut dyn Wdf,
            &mut tree_s,
            &mut fast_p,
            &mut tree_p,
        ] {
            n.calc_impedance();
        }

        assert!(
            (fast_s.resistance() - tree_s.resistance()).abs() / tree_s.resistance() < 1e-6,
            "series R: {} vs {}",
            fast_s.resistance(),
            tree_s.resistance()
        );
        assert!(
            (fast_p.resistance() - tree_p.resistance()).abs() / tree_p.resistance() < 1e-6,
            "parallel R: {} vs {}",
            fast_p.resistance(),
            tree_p.resistance()
        );

        // Drive both pairs from an open root (b = a), which is the harshest
        // case for state divergence: nothing damps an error away.
        for k in 0..2_000 {
            let drive = 2.0 * (k as f32 * 0.031).sin() + 0.4 * (k as f32 * 0.37).cos();
            for (fast, tree, what) in [
                (
                    &mut fast_s as &mut dyn Wdf,
                    &mut tree_s as &mut dyn Wdf,
                    "series",
                ),
                (&mut fast_p, &mut tree_p, "parallel"),
            ] {
                let (bf, bt) = (fast.reflected(), tree.reflected());
                assert!((bf - bt).abs() < 1e-4, "k={k} {what}: {bf} vs {bt}");
                fast.incident(drive);
                tree.incident(drive);
            }
        }
    }

    /// A Norton source is a Thévenin source: `R·I` in, EMF out.
    #[test]
    fn current_source_is_its_thevenin_twin() {
        let (r, i) = (2200.0f32, 1.5e-3f32);
        let mut cs = ResistiveCurrentSource::new(r);
        cs.set_current(i);
        cs.calc_impedance();
        let mut vs = ResistiveVoltageSource::new(r);
        vs.set_voltage(r * i);
        vs.calc_impedance();
        assert_eq!(cs.resistance(), vs.resistance());
        assert!((cs.reflected() - vs.reflected()).abs() < 1e-9);
    }

    /// A constant EMF behind a capacitor vanishes once the capacitor has
    /// charged — the coupling-cap fact, which is why the source enters the
    /// reflection only as its *difference*.
    #[test]
    fn capacitive_source_passes_only_changes() {
        let mut s = CapacitiveVoltageSource::new(100e-9, SR);
        s.calc_impedance();
        s.set_voltage(4.5);
        // First sample carries the step...
        assert!((s.reflected() - 4.5).abs() < 1e-6);
        // ...after which a held EMF contributes nothing and the element is a
        // plain capacitor.
        let mut plain = Capacitor::new(100e-9, SR);
        plain.calc_impedance();
        for k in 0..500 {
            let a = (k as f32 * 0.11).sin();
            s.incident(a);
            plain.incident(a);
            assert!((s.reflected() - plain.reflected()).abs() < 1e-6, "k={k}");
        }
    }
}

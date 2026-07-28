//! Wave Digital Filter (WDF) primitives — the white-box circuit-modelling
//! substrate (white paper §6 deep water; PRD 020 / ADR 028, PRD 021 / ADR 029,
//! PRD 025 / ADR 032).
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
//! # The tree
//!
//! [`Wdf`] is the common port interface; [`one_port`] holds the leaves
//! (resistors, capacitors, sources), [`adaptor`] the composable
//! [`Series`](adaptor::Series) / [`Parallel`](adaptor::Parallel) junctions, and
//! [`rtype`] the N-port [`RType`](rtype::RType) adaptor for the topologies that
//! series/parallel reduction cannot express (op-amp feedback, bridged networks).
//! [`diode`] holds the nonlinear roots.
//!
//! Trees are built by **ownership**: `Parallel<ResistiveVoltageSource,
//! Capacitor>` owns both children, monomorphises to straight-line code, and
//! allocates nothing. Waves are driven **from the root downward** — the root
//! asks the tree for its reflected wave and resistance, solves its scalar
//! equation, and hands an incident wave back down:
//!
//! ```ignore
//! let a = tree.reflected();
//! let (v, b) = diode.solve(a, tree.resistance());
//! tree.incident(b);
//! ```
//!
//! # Impedance propagation
//!
//! `chowdsp_wdf` propagates impedance changes *upward* through parent pointers,
//! with a scoped guard to coalesce simultaneous changes. A child holding a
//! pointer to its parent is an anti-pattern under Rust's borrow checker, and
//! with owned subtrees it is also unnecessary: [`Wdf::calc_impedance`] recurses
//! the whole tree **post-order from the root** (children first), so several
//! knobs moving at once cost exactly one pass and no coalescing machinery is
//! needed. Call it at the block boundary, and only when a knob actually moved
//! (the settled-skip convention shared with `eq::chain` and `eq::tonestack`);
//! the per-sample path then contains no impedance arithmetic at all.
//!
//! # Provenance
//!
//! Structure and scattering conventions follow `chowdsp_wdf` (BSD-3), rewritten
//! in Rust. Circuit topologies, component values and diode SPICE parameters are
//! facts taken from schematics. No GPL sources were copied.

pub mod adaptor;
pub mod diode;
pub mod omega;
pub mod one_port;
pub mod rtype;

pub use adaptor::{Parallel, PolarityInverter, Series};
pub use diode::{AsymDiode, DiodePair};
pub use one_port::{
    CapacitiveVoltageSource, Capacitor, ResistiveCurrentSource, ResistiveVoltageSource, Resistor,
    ResistorCapacitorParallel, ResistorCapacitorSeries,
};
pub use rtype::{
    JEl, Junction, NON_INVERTING_NODES, NON_INVERTING_PORTS, RType, non_inverting_els, op_amp,
};

/// Flush a denormal to zero (RT rule 7 — a decaying reactive state must not
/// sink into denormal territory and stall the FPU).
#[inline]
pub(crate) fn flush(v: f32) -> f32 {
    if v.abs() < 1e-25 { 0.0 } else { v }
}

/// One port of a wave digital filter: the interface every element and adaptor
/// presents to whatever sits above it.
///
/// The contract, in the order a host drives it:
///
/// 1. [`prepare`](Wdf::prepare) — set the sample rate, recursively. Rate-
///    dependent port resistances (capacitors) update here.
/// 2. [`calc_impedance`](Wdf::calc_impedance) — recompute port resistances
///    **post-order** (children first, then self). Call on the root after
///    `prepare` and after any component value changes; never per sample.
/// 3. Per sample: [`reflected`](Wdf::reflected) to pull the wave up the tree,
///    then [`incident`](Wdf::incident) to push the root's answer back down.
///
/// [`resistance`](Wdf::resistance) and [`conductance`](Wdf::conductance) are
/// cached reads — valid only after `calc_impedance`.
pub trait Wdf {
    /// Recompute this port's reference resistance, recursing into children
    /// first. Not RT-hot: block boundary only.
    fn calc_impedance(&mut self);

    /// The cached port reference resistance `R`.
    fn resistance(&self) -> f32;

    /// The cached port conductance `G = 1/R`.
    fn conductance(&self) -> f32;

    /// The wave this port reflects toward its parent, computed for this sample.
    /// Adaptors recurse into their children.
    fn reflected(&mut self) -> f32;

    /// Accept the parent's incident wave and push the resulting child waves
    /// down the subtree.
    fn incident(&mut self, a: f32);

    /// Set the sample rate, recursively. Default: nothing is rate-dependent.
    fn prepare(&mut self, _sample_rate: f32) {}

    /// Clear all reactive state, recursively.
    fn reset(&mut self);
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
/// This is the **hand-reduced fast path**, kept for circuits whose whole
/// topology is one parallel node — building a [`Parallel`] tree for those buys
/// nothing. `parallel_adaptor_matches_the_hand_reduced_helper` pins that the
/// two agree, so the shortcut can never silently drift from the framework.
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

/// Like [`parallel_root`] but with an external current source `i_src` injected
/// into the node: the root's Thévenin voltage rises by `i_src · R`, i.e.
/// `a = (Σ Gₖ·aₖ + i_src) / Σ Gₖ` behind the same `R = 1 / Σ Gₖ`. This is how an
/// ideal op-amp's forced feedback current drives a diode clipper — the
/// virtual-short reduction of a feedback-topology overdrive (`drive/sd1.rs`).
/// `parallel_root(ports)` is exactly the `i_src = 0` case.
///
/// Note the identity this encodes, which is why the framework needs no
/// "ideal current source" adaptor: injecting `I` into the root node is exactly
/// a shift of the incident wave, `a' = a + R·I`, leaving `R` alone. An *ideal*
/// current source has infinite port resistance and therefore no one-port
/// representation ([`ResistiveCurrentSource`] is the finite-`R` Norton form);
/// at the root it needs none.
#[inline]
pub fn parallel_root_with_source(ports: &[(f32, f32)], i_src: f32) -> (f32, f32) {
    let mut g_sum = 0.0f32;
    let mut weighted = 0.0f32;
    for &(g, a) in ports {
        g_sum += g;
        weighted += g * a;
    }
    ((weighted + i_src) / g_sum, 1.0 / g_sum)
}

#[cfg(test)]
mod tests {
    use super::*;

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
            cap.incident(2.0 * v - a1);
        }
        // Capacitor fully charged, no current: node = source EMF.
        assert!((v - e).abs() < 1e-3, "settled {v}, want {e}");
    }

    /// The current-injected parallel root: zero source reproduces
    /// [`parallel_root`] bit-for-bit, and a positive current lifts the node
    /// voltage by exactly `i·R` while leaving the resistance untouched.
    #[test]
    fn parallel_root_source_injects_current() {
        let ports = [(1.0 / 1000.0, 0.3f32), (1.0 / 2200.0, -0.1f32)];
        let (a0, r0) = parallel_root(&ports);
        assert_eq!((a0, r0), parallel_root_with_source(&ports, 0.0));
        let (a1, r1) = parallel_root_with_source(&ports, 1e-3);
        assert!((r1 - r0).abs() < 1e-9, "resistance unchanged by the source");
        assert!((a1 - (a0 + 1e-3 * r0)).abs() < 1e-6, "node rises by i·R");
    }

    /// The bridge between the hand-reduced helper (option (c)) and the
    /// composable framework (option (a)): a two-port parallel node must scatter
    /// identically whichever way it is spelled. This is what lets
    /// [`parallel_root`] survive as a shortcut without becoming a second,
    /// drifting implementation of the same physics.
    #[test]
    fn parallel_adaptor_matches_the_hand_reduced_helper() {
        let sr = 192_000.0f32;
        let mut tree = Parallel::new(
            ResistiveVoltageSource::new(2200.0),
            Capacitor::new(22e-9, sr),
        );
        tree.calc_impedance();

        let mut cap = Capacitor::new(22e-9, sr);
        let g_src = 1.0 / 2200.0;

        for k in 0..500 {
            let e = 3.0 * (k as f32 * 0.07).sin();

            tree.port1_mut().set_voltage(e);
            let a_tree = tree.reflected();
            let r_tree = tree.resistance();

            let a_cap = cap.reflected();
            let (a_ref, r_ref) = parallel_root(&[(g_src, e), (cap.conductance(), a_cap)]);

            assert!((a_tree - a_ref).abs() < 1e-6, "k={k}: {a_tree} vs {a_ref}");
            assert!((r_tree - r_ref).abs() / r_ref < 1e-6, "k={k}: R");

            // Drive both with the same root answer (an open root: b = a).
            let v = a_ref;
            tree.incident(2.0 * v - a_tree);
            cap.incident(2.0 * v - a_cap);
        }
    }
}

//! Equalizers: the in-chain `eq` pedal family and the 8-band parametric EQ
//! that lives on the engine's fixed output stage ([`global`], PRD 003).
//!
//! The chain family has two pedals (PRD 011):
//!
//! - [`chain`] — the 3-band tone pedal (low/mid/high, sweepable mid): fast
//!   tone shaping, four knobs.
//! - [`parametric`] — the global EQ's 8-band engine as a pedal: the same
//!   visual editor, anywhere in the chain, as many instances as slots allow.
//!
//! Both cores are preallocated side by side in [`Eq`]; `select_pedal` is an
//! index move plus a state reset of the incoming pedal (PRD 001 — values are
//! re-sent by the control side from its per-pedal shadow).

pub mod chain;
pub mod global;
pub mod parametric;

use lh_core::{EffectDesc, FamilyDesc};

use crate::Effect;

pub use global::GlobalEq;

/// The eq family, in menu order. Append-only (PRD 001).
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "eq",
    name: "EQ",
    pedals: &[&chain::DESC, &parametric::DESC],
};

pub const PEDAL_COUNT: usize = 2;

/// The `eq` chain slot: both pedals preallocated, dispatch by index.
pub struct Eq {
    pedal: usize,
    tone: chain::Tone,
    para: parametric::Parametric,
}

impl Default for Eq {
    fn default() -> Self {
        Self::new()
    }
}

impl Eq {
    pub fn new() -> Self {
        Self {
            pedal: 0,
            tone: chain::Tone::new(),
            para: parametric::Parametric::new(),
        }
    }
}

impl Effect for Eq {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.pedal
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal == self.pedal || pedal >= FAMILY.pedals.len() {
            return;
        }
        self.pedal = pedal;
        // Fresh filter memories for the incoming pedal; the control side
        // re-sends its knob values from the shadow (PRD 001).
        match pedal {
            0 => self.tone.reset(),
            _ => self.para.reset(),
        }
    }

    fn descriptor(&self) -> &'static EffectDesc {
        FAMILY.pedals[self.pedal]
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.tone.prepare(sample_rate);
        self.para.prepare(sample_rate);
    }

    fn reset(&mut self) {
        self.tone.reset();
        self.para.reset();
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        match self.pedal {
            0 => self.tone.set_param(index, normalized),
            _ => self.para.set_param(index, normalized),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        match self.pedal {
            0 => self.tone.process(left, right),
            _ => self.para.process(left, right),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_stereo_in_blocks, rms, silence, sine};
    use lh_core::lin_to_db;

    const SR: u32 = 48_000;

    fn prepared(pedal: usize) -> Eq {
        let mut eq = Eq::new();
        eq.prepare(SR);
        eq.select_pedal(pedal);
        let desc = FAMILY.pedals[pedal];
        for (i, p) in desc.params.iter().enumerate() {
            eq.set_param(i, p.default_norm());
        }
        eq
    }

    fn set_by(eq: &mut Eq, key: &str, real: f32) {
        let desc = FAMILY.pedals[eq.pedal_index()];
        let i = desc.param_index(key).unwrap();
        eq.set_param(i, desc.params[i].range.to_norm(real));
    }

    /// Steady-state gain (dB) at `freq` through the family effect.
    fn gain_at(eq: &mut Eq, freq: f32) -> f32 {
        let x = sine(SR, freq, SR as usize / 2);
        let (l, _) = process_stereo_in_blocks(eq, &x, 64);
        assert_finite("eq output", &l);
        let n = l.len();
        lin_to_db(rms(&l[n / 2..]) / rms(&x[n / 2..]))
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.key, "eq");
        assert_eq!(FAMILY.pedals.len(), PEDAL_COUNT);
        // Append-only: the 3-band keeps the family key (pre-PRD 011 presets
        // and plugin ids), the parametric appends after it.
        assert_eq!(FAMILY.pedals[0].key, "eq");
        assert_eq!(FAMILY.pedals[1].key, "parametric");
        assert_eq!(FAMILY.pedals[0].params.len(), 4);
        assert_eq!(
            FAMILY.pedals[1].params.len(),
            lh_core::global_eq::BAND_COUNT * parametric::BAND_PARAMS
        );
    }

    /// Same drive as the global EQ's own suite, but through the pedal's
    /// flat param space: a +9 dB bell at 800 Hz boosts its center only.
    #[test]
    fn parametric_bell_boosts_at_center_only() {
        let mut eq = prepared(1);
        set_by(&mut eq, "b3_on", 1.0);
        set_by(&mut eq, "b3_freq", 800.0);
        set_by(&mut eq, "b3_gain", 9.0);
        set_by(&mut eq, "b3_q", 0.9);
        assert!((gain_at(&mut eq, 800.0) - 9.0).abs() < 0.5);
        eq.reset();
        assert!(gain_at(&mut eq, 60.0).abs() < 1.0);
        eq.reset();
        assert!(gain_at(&mut eq, 10_000.0).abs() < 1.0);
    }

    /// Every band type reaches the audio through the param mapping.
    #[test]
    fn parametric_every_type_shapes_its_range() {
        let case = |kind: f32, freq: f32, gain: f32, probe: f32, expect_db: f32, tol: f32| {
            let mut eq = prepared(1);
            set_by(&mut eq, "b4_on", 1.0);
            set_by(&mut eq, "b4_type", kind);
            set_by(&mut eq, "b4_freq", freq);
            set_by(&mut eq, "b4_gain", gain);
            set_by(&mut eq, "b4_q", 0.707);
            let got = gain_at(&mut eq, probe);
            assert!(
                (got - expect_db).abs() < tol,
                "type {kind} at {probe} Hz: expected {expect_db} dB, got {got:.2}"
            );
        };
        case(0.0, 200.0, 0.0, 50.0, -24.0, 4.0); // low cut
        case(1.0, 150.0, 8.0, 40.0, 8.0, 1.0); // low shelf
        case(3.0, 3_000.0, -9.0, 12_000.0, -9.0, 1.5); // high shelf
        case(4.0, 2_000.0, 0.0, 8_000.0, -26.0, 6.0); // high cut
    }

    /// All bands off must be bit-transparent — the inherited fast path.
    #[test]
    fn parametric_flat_is_bit_transparent() {
        let mut eq = prepared(1);
        let x = sine(SR, 220.0, 8_192);
        let (l, r) = process_stereo_in_blocks(&mut eq, &x, 512);
        assert_eq!(x, l, "all-off parametric must not touch the signal");
        assert_eq!(x, r);
    }

    /// The GUI curve and the audio must agree through the param mapping too.
    #[test]
    fn parametric_response_matches_rendered_audio() {
        let mut eq = prepared(1);
        set_by(&mut eq, "b5_on", 1.0);
        set_by(&mut eq, "b5_freq", 1_200.0);
        set_by(&mut eq, "b5_gain", -7.0);
        set_by(&mut eq, "b5_q", 1.4);
        let state = eq.para.state();
        for freq in [200.0, 1_200.0, 6_000.0] {
            let analytic = global::response_db(&state, SR as f32, freq);
            let rendered = gain_at(&mut eq, freq);
            eq.reset();
            assert!(
                (analytic - rendered).abs() < 0.5,
                "{freq} Hz: curve {analytic:.2} vs audio {rendered:.2}"
            );
        }
    }

    /// Param index 0 is `low` (dB) on the tone pedal but `b1_on` on the
    /// parametric — switching pedals must re-route, stay bounded, and leave
    /// each pedal's own state consistent.
    #[test]
    fn pedal_switch_is_bounded_and_routes_params() {
        let mut eq = prepared(0);
        let x = sine(SR, 330.0, SR as usize);
        let mut l = x.clone();
        let mut r = x.clone();
        let third = x.len() / 3;
        let (a, rest) = l.split_at_mut(third);
        let (b, c) = rest.split_at_mut(third);
        let (ar, restr) = r.split_at_mut(third);
        let (br, cr) = restr.split_at_mut(third);
        eq.process(a, ar);
        eq.select_pedal(1);
        assert_eq!(eq.descriptor().key, "parametric");
        eq.set_param(0, 1.0); // b1_on — not the tone pedal's low shelf
        eq.process(b, br);
        eq.select_pedal(0);
        assert_eq!(eq.descriptor().key, "eq");
        eq.process(c, cr);
        assert_finite("eq pedal switch L", &l);
        assert_finite("eq pedal switch R", &r);
        assert!(peak(&l) < 4.0);
    }

    /// Enabling a band mid-stream engages through the wet ramp — no click.
    #[test]
    fn parametric_engage_is_click_free() {
        let mut eq = prepared(1);
        let x = sine(SR, 220.0, SR as usize);
        let mut l = x.clone();
        let mut r = x.clone();
        for (i, (cl, cr)) in l.chunks_mut(64).zip(r.chunks_mut(64)).enumerate() {
            if i == 100 {
                set_by(&mut eq, "b3_freq", 500.0);
                set_by(&mut eq, "b3_gain", 15.0);
                set_by(&mut eq, "b3_q", 2.0);
                set_by(&mut eq, "b3_on", 1.0);
            }
            if i == 400 {
                set_by(&mut eq, "b3_on", 0.0);
            }
            eq.process(cl, cr);
        }
        assert_finite("engage sweep", &l);
        let max_step = l
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.25, "click detected, step {max_step}");
    }

    /// Sweeping every one of the 40 params end to end stays finite and
    /// bounded (the family-wide fuzz the other families run).
    #[test]
    fn every_knob_sweep_stays_finite() {
        for (i, param) in parametric::DESC.params.iter().enumerate() {
            let mut eq = prepared(1);
            let mut x = sine(SR, 330.0, 24_000);
            let mut xr = x.clone();
            let third = x.len() / 3;
            let (a, rest) = x.split_at_mut(third);
            let (b, c) = rest.split_at_mut(third);
            let (ar, restr) = xr.split_at_mut(third);
            let (br, cr) = restr.split_at_mut(third);
            // Engage the band under test so its knobs actually reach audio.
            eq.set_param((i / parametric::BAND_PARAMS) * parametric::BAND_PARAMS, 1.0);
            eq.process(a, ar);
            eq.set_param(i, 0.0);
            eq.process(b, br);
            eq.set_param(i, 1.0);
            eq.process(c, cr);
            assert_finite(&format!("parametric sweep {}", param.key), &x);
            assert!(peak(&x) < 16.0, "sweeping {} must stay bounded", param.key);
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for pedal in 0..PEDAL_COUNT {
            let mut eq = prepared(pedal);
            if pedal == 1 {
                set_by(&mut eq, "b3_on", 1.0);
                set_by(&mut eq, "b3_gain", 12.0);
            }
            let x = silence(8_192);
            let (l, r) = process_stereo_in_blocks(&mut eq, &x, 512);
            assert!(
                rms(&l) == 0.0 && rms(&r) == 0.0,
                "pedal {pedal} must stay silent"
            );
        }
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for pedal in 0..PEDAL_COUNT {
            for sr in [44_100u32, 48_000, 96_000] {
                let mut eq = Eq::new();
                eq.prepare(sr);
                eq.select_pedal(pedal);
                if pedal == 1 {
                    let desc = FAMILY.pedals[1];
                    let i = desc.param_index("b5_on").unwrap();
                    eq.set_param(i, 1.0);
                    let g = desc.param_index("b5_gain").unwrap();
                    eq.set_param(g, desc.params[g].range.to_norm(6.0));
                }
                for chunk in [32usize, 483, 1_024] {
                    let x = sine(sr, 440.0, 4_096);
                    let (l, r) = process_stereo_in_blocks(&mut eq, &x, chunk);
                    assert_finite("eq multirate L", &l);
                    assert_finite("eq multirate R", &r);
                }
            }
        }
    }
}

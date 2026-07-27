//! **waveshaper** — not a pedal anyone built, a palette: twelve shaping curves
//! behind one Shape knob, every one of them anti-aliased.
//!
//! The drive family models circuits; this one models *functions*. Saturation
//! curves you already have elsewhere sit next to things no analogue box does
//! cheaply — a triangle wavefolder, a quantiser staircase, Chebyshev
//! polynomials that synthesise one chosen harmonic and almost nothing else.
//!
//! Every curve runs through [`crate::blocks::waveshaper`]'s ADAA on top of the
//! family's 4× oversampling, which is what makes the harsh ones usable: a
//! quantiser or a folder point-sampled at 4× is mostly foldover.
//!
//! Curves that have an elementary second antiderivative get second-order ADAA
//! (the hard corners, where it counts); the rest get first-order. Switching
//! Shape resets that state — it holds `F₁` of the *outgoing* curve.

use lh_core::{EffectDesc, ParamDesc, Range, db_to_lin};

use crate::blocks::waveshaper::{Adaa1, Adaa2, Curve};

use super::{Circuit, OnePole, Ramp, knob, lp_coeff};

static SHAPE_RANGE: Range = Range::Stepped {
    labels: &Curve::LABELS,
};

static PARAMS: [ParamDesc; 4] = [
    knob("drive", "Drive", 5.0, 20.0),
    ParamDesc {
        key: "shape",
        name: "Shape",
        unit: "",
        range: SHAPE_RANGE,
        default: 0.0,
        smoothing_ms: 0.0,
    },
    knob("tone", "Tone", 6.0, 30.0),
    knob("level", "Level", 6.0, 20.0),
];

pub(super) static DESC: EffectDesc = EffectDesc {
    key: "waveshaper",
    name: "Waveshaper",
    params: &PARAMS,
};

/// Unity up to +33 dB. The bottom of the range matters more here than on a
/// circuit model: the Chebyshev curves are only themselves while the signal
/// stays inside ±1, so "drive at 2" is a real setting, not a spare one.
const DRIVE_MIN_DB: f32 = 0.0;
const DRIVE_SPAN_DB: f32 = 33.0;

/// Post lowpass, swept by the Tone knob. Nearly every curve here puts energy
/// far above where a guitar amp would.
const TONE_MIN_HZ: f32 = 700.0;
const TONE_MAX_HZ: f32 = 14_000.0;

/// DC blocker: `Asym`, `Fuzz`, `Cheby2` and `Cheby4` are not odd functions, so
/// they all park the signal off centre.
const DC_HZ: f32 = 12.0;

/// Calibrated with `modelled_pedals_sit_near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.62;

pub(super) struct Waveshaper {
    curve: Curve,
    adaa1: Adaa1,
    adaa2: Adaa2,
    dc_os: OnePole,
    tone_lp: OnePole,
    base_rate: f32,
    c_dc: f32,
}

impl Waveshaper {
    pub(super) fn new() -> Self {
        Self {
            curve: Curve::ALL[0],
            adaa1: Adaa1::new(),
            adaa2: Adaa2::new(),
            dc_os: OnePole::default(),
            tone_lp: OnePole::default(),
            base_rate: 48_000.0,
            c_dc: 0.0,
        }
    }
}

impl Circuit for Waveshaper {
    fn prepare(&mut self, base_rate: f32, os_rate: f32) {
        self.base_rate = base_rate;
        self.c_dc = lp_coeff(DC_HZ, os_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.adaa1.reset();
        self.adaa2.reset();
        self.dc_os.reset();
        self.tone_lp.reset();
    }

    fn set_shape(&mut self, index: usize) {
        let curve = Curve::from_index(index);
        if curve != self.curve {
            self.curve = curve;
            // The ADAA state holds F₁ of the curve being left behind.
            self.adaa1.reset();
            self.adaa2.reset();
        }
    }

    fn shape(&mut self, block: &mut [f32], drive: &[f32]) {
        let mut gain = Ramp::over(drive, |d| {
            db_to_lin(DRIVE_MIN_DB + DRIVE_SPAN_DB * (d * 0.1).powf(1.5))
        });
        let curve = self.curve;
        // Hoisted out of the loop: the match on `curve` inside the closures
        // would otherwise run per sample, and the order test is loop-invariant.
        if curve.order() == 2 {
            for s in block.iter_mut() {
                let v = gain.tick() * *s;
                *s = self
                    .adaa2
                    .process(v, |x| curve.f(x), |x| curve.f1(x), |x| curve.f2(x));
            }
        } else {
            for s in block.iter_mut() {
                let v = gain.tick() * *s;
                *s = self.adaa1.process(v, |x| curve.f(x), |x| curve.f1(x));
            }
        }
        for s in block.iter_mut() {
            *s -= self.dc_os.lp(*s, self.c_dc);
        }
    }

    fn post(&mut self, block: &mut [f32], tone: &[f32]) {
        // One `lp_coeff` per chunk end, eased across the block like the rest
        // of the family.
        let mut c = Ramp::over(tone, |t| {
            let hz = TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(t * 0.1);
            lp_coeff(hz, self.base_rate)
        });
        for s in block.iter_mut() {
            *s = self.tone_lp.lp(*s, c.tick()) * MAKEUP;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blocks::waveshaper::CURVE_COUNT;

    #[test]
    fn faceplate_matches_the_curve_registry() {
        assert_eq!(PARAMS.len(), 4);
        let Range::Stepped { labels } = PARAMS[1].range else {
            panic!("Shape must be a stepped selector");
        };
        assert_eq!(labels.len(), CURVE_COUNT);
        // Stepped params round to an index; the whole range must be reachable.
        for i in 0..CURVE_COUNT {
            let norm = PARAMS[1].range.to_norm(i as f32);
            assert_eq!(PARAMS[1].range.to_real(norm) as usize, i);
        }
    }

    /// Switching curves must not carry the old curve's antiderivative into the
    /// new one — that would put a step on the output.
    #[test]
    fn a_shape_switch_clears_the_adaa_state() {
        let mut w = Waveshaper::new();
        w.prepare(48_000.0, 192_000.0);
        let drive = [5.0f32; 64];
        let mut block = [0.5f32; 64];
        w.shape(&mut block, &drive);
        w.set_shape(6);
        assert_eq!(w.curve, Curve::Digital);
        let mut fresh = Waveshaper::new();
        fresh.prepare(48_000.0, 192_000.0);
        fresh.set_shape(6);
        let mut a = [0.5f32; 64];
        let mut b = [0.5f32; 64];
        w.shape(&mut a, &drive);
        fresh.shape(&mut b, &drive);
        // The DC blocker still carries history, so compare the ADAA state
        // itself rather than the output.
        assert_eq!(
            w.adaa1
                .process(0.3, |x| Curve::Digital.f(x), |x| Curve::Digital.f1(x)),
            fresh
                .adaa1
                .process(0.3, |x| Curve::Digital.f(x), |x| Curve::Digital.f1(x)),
            "a shape switch must leave the ADAA state as if freshly reset"
        );
    }
}

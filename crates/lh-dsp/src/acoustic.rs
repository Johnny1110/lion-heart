//! The acoustic family: an **acoustic simulator** behind one chain slot —
//! reshape a magnetic-pickup electric into a steel-string acoustic. The family
//! key is deliberately broad ("acoustic"): jumbo, nylon, and piezo voicings
//! all have an obvious home here later, and slots address families (PRD 001).
//!
//! One pedal today ([`acoustic`], the steel-string voicing), a pure parametric
//! filter network — the way the analog Boss AC-2/AC-3 does it, not a body
//! impulse response (that would be the cab's convolution path). The transform
//! is three ideas, cascaded RBJ biquads per channel (feed-forward, no tail —
//! [`Effect::tail_seconds`] stays 0):
//!
//! 1. **De-electrify (fixed).** A subsonic high-pass keeps the body boost from
//!    turning to mud, and a broad ~900 Hz peaking *cut* pulls out the
//!    magnetic-pickup midrange honk that no acoustic has.
//! 2. **Body (the `Body` knob).** Three graduated resonant peaks stand in for a
//!    guitar body's low modes — the ~110 Hz main air (Helmholtz) resonance, the
//!    ~200 Hz top-plate (T1), and a ~380 Hz back/side mode — so the box sounds
//!    hollow and woody instead of merely bass-boosted. Body scales all three.
//! 3. **Top (the `Top` knob).** A ~3 kHz presence peak (pick attack) and a
//!    ~5.5 kHz high shelf (string zing / air) restore the sparkle a pickup's
//!    inductance rolls off. Top scales both.
//!
//! There is no transparent knob position — an acoustic simulator colors the
//! signal wherever it sits — so, like the filter family, this is an **opt-in**
//! family: registered (reachable from the ＋ menu / REPL `add acoustic`) but
//! absent from `lh_core::DEFAULT_CHAIN`. Unlike the filter it ships *active*
//! when added: you add it because you want the sound now.

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::biquad::Biquad;
use crate::blocks::smooth::Smoothed;

/// A 0..1 "amount" knob (Body / Top) or the output Level.
const fn amount(key: &'static str, name: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 30.0,
    }
}

static PARAMS: [ParamDesc; 3] = [
    amount("body", "Body", 0.5),
    amount("top", "Top", 0.5),
    amount("level", "Level", 0.7),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "acoustic",
    name: "Acoustic",
    params: &PARAMS,
};

/// The acoustic family, in menu order. Append-only (PRD 001).
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "acoustic",
    name: "Acoustic",
    pedals: &[&DESC],
};

// --- fixed "de-electrify" voicing ---
const HP_HZ: f32 = 45.0;
const HP_Q: f32 = 0.707;
const SCOOP_HZ: f32 = 900.0;
const SCOOP_DB: f32 = -4.0;
const SCOOP_Q: f32 = 0.7;

// Body resonances, `(center Hz, Q, dB at Body = 1.0)`. Graduated like a real
// body's mode series — the low air resonance is the strongest, higher modes
// taper off — so it reads as a hollow box, not one bass bump.
const BODY_RES: [(f32, f32, f32); 3] = [(110.0, 1.5, 5.0), (200.0, 1.2, 4.0), (380.0, 1.0, 2.5)];

// Top: presence peak (pick attack) then an air high shelf (string zing).
const PRESENCE_HZ: f32 = 3_000.0;
const PRESENCE_Q: f32 = 0.8;
const PRESENCE_DB: f32 = 4.0;
const AIR_HZ: f32 = 5_500.0;
const AIR_DB: f32 = 8.0;

/// Broadband makeup so the default board sits near unity — the low-body and
/// air boosts add more than the 900 Hz scoop removes on a guitar spectrum.
/// Calibrated by `near_unity_at_default_knobs`.
const MAKEUP: f32 = 0.82;

/// Output level: audio-taper on the 0..1 knob — unity near 0.7, +6 dB at 1.0.
#[inline]
fn level_gain(pos: f32) -> f32 {
    let p = pos.clamp(0.0, 1.0);
    p * p * 2.0
}

/// One channel's cascade of biquad sections. The fixed sections are built once
/// in [`Voicing::build_fixed`]; the Body/Top sections are rebuilt at block rate
/// only while their knob is moving (see [`Acoustic::process`]).
#[derive(Default)]
struct Voicing {
    hp: Biquad,
    scoop: Biquad,
    body: [Biquad; 3],
    presence: Biquad,
    air: Biquad,
}

impl Voicing {
    fn build_fixed(&mut self, sr: f32) {
        self.hp.set_highpass(sr, HP_HZ, HP_Q);
        self.scoop.set_peaking(sr, SCOOP_HZ, SCOOP_DB, SCOOP_Q);
    }

    fn build_body(&mut self, sr: f32, body: f32) {
        for (bq, (hz, q, max_db)) in self.body.iter_mut().zip(BODY_RES) {
            bq.set_peaking(sr, hz, body * max_db, q);
        }
    }

    fn build_top(&mut self, sr: f32, top: f32) {
        self.presence
            .set_peaking(sr, PRESENCE_HZ, top * PRESENCE_DB, PRESENCE_Q);
        self.air.set_high_shelf(sr, AIR_HZ, top * AIR_DB);
    }

    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let mut y = self.hp.process_sample(x);
        y = self.scoop.process_sample(y);
        for bq in &mut self.body {
            y = bq.process_sample(y);
        }
        y = self.presence.process_sample(y);
        self.air.process_sample(y)
    }

    fn reset(&mut self) {
        self.hp.reset();
        self.scoop.reset();
        for bq in &mut self.body {
            bq.reset();
        }
        self.presence.reset();
        self.air.reset();
    }
}

pub struct Acoustic {
    sample_rate: f32,
    ch: [Voicing; 2],
    body: Smoothed,
    top: Smoothed,
    level: Smoothed,
    /// The knob values the Body/Top coefficients were last built for, so a
    /// settled control costs nothing (the global-EQ settled-skip pattern).
    last_body: f32,
    last_top: f32,
}

impl Default for Acoustic {
    fn default() -> Self {
        Self::new()
    }
}

impl Acoustic {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            ch: [Voicing::default(), Voicing::default()],
            body: Smoothed::new(PARAMS[0].default),
            top: Smoothed::new(PARAMS[1].default),
            level: Smoothed::new(PARAMS[2].default),
            last_body: f32::NAN,
            last_top: f32::NAN,
        }
    }
}

impl Effect for Acoustic {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for (s, ms) in [
            (&mut self.body, 30.0),
            (&mut self.top, 30.0),
            (&mut self.level, 20.0),
        ] {
            s.configure(ms, sample_rate);
            s.snap_to_target();
        }
        for v in &mut self.ch {
            v.build_fixed(self.sample_rate);
            v.build_body(self.sample_rate, self.body.current());
            v.build_top(self.sample_rate, self.top.current());
        }
        self.last_body = self.body.current();
        self.last_top = self.top.current();
        self.reset();
    }

    fn reset(&mut self) {
        for v in &mut self.ch {
            v.reset();
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let Some(param) = PARAMS.get(index) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match index {
            0 => self.body.set_target(real),
            1 => self.top.set_target(real),
            2 => self.level.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let sr = self.sample_rate;
        // Rebuild the knob-dependent sections at block rate, and only while the
        // knob is actually moving — a settled Body/Top costs one comparison.
        let body = self.body.current();
        if (body - self.last_body).abs() > 1e-6 {
            for v in &mut self.ch {
                v.build_body(sr, body);
            }
            self.last_body = body;
        }
        let top = self.top.current();
        if (top - self.last_top).abs() > 1e-6 {
            for v in &mut self.ch {
                v.build_top(sr, top);
            }
            self.last_top = top;
        }

        let [ch_l, ch_r] = &mut self.ch;
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let g = level_gain(self.level.tick()) * MAKEUP;
            // Advance the coefficient smoothers so the next block's rebuild
            // reflects the moved knob (the value used above is the block start).
            self.body.tick();
            self.top.tick();
            *l = ch_l.process(*l) * g;
            *r = ch_r.process(*r) * g;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{peak, process_stereo_in_blocks, rms, sine};

    const SR: u32 = 48_000;

    fn prepared(body: f32, top: f32, level: f32) -> Acoustic {
        let mut a = Acoustic::new();
        a.prepare(SR);
        a.set_param(0, body);
        a.set_param(1, top);
        a.set_param(2, level);
        a
    }

    /// Steady-state gain (out/in RMS) at `freq` for a given knob setting.
    fn gain_at(freq: f32, body: f32, top: f32, level: f32) -> f64 {
        let mut a = prepared(body, top, level);
        let x = sine(SR, freq, SR as usize / 2);
        let (l, _) = process_stereo_in_blocks(&mut a, &x, 128);
        let out = f64::from(rms(&l[l.len() / 2..]));
        let inp = f64::from(rms(&x[x.len() / 2..]));
        out / inp
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.key, "acoustic");
        assert_eq!(FAMILY.pedals.len(), 1);
        assert!(std::ptr::eq(FAMILY.pedals[0], &DESC));
        let captions: Vec<&str> = DESC.params.iter().map(|p| p.name).collect();
        assert_eq!(captions, ["Body", "Top", "Level"]);
    }

    #[test]
    fn base_voicing_scoops_the_electric_mids() {
        // Even with Body and Top off, the fixed voicing pulls the ~900 Hz
        // magnetic-pickup honk down relative to the low body and the air — the
        // "de-electrify" move that separates this from a flat wire.
        let mid = gain_at(900.0, 0.0, 0.0, 0.7);
        let low = gain_at(180.0, 0.0, 0.0, 0.7);
        assert!(
            mid < 0.75 * low,
            "mids must be scooped below the body: 900 Hz {mid:.3} vs 180 Hz {low:.3}"
        );
    }

    #[test]
    fn body_knob_lifts_the_low_body() {
        // Turning Body up must lift the ~110 Hz air resonance.
        let off = gain_at(110.0, 0.0, 0.5, 0.7);
        let on = gain_at(110.0, 1.0, 0.5, 0.7);
        assert!(
            on > 1.3 * off,
            "Body must resonate the lows: {on:.3} vs {off:.3}"
        );
    }

    #[test]
    fn top_knob_lifts_the_air() {
        // Turning Top up must lift the high shelf (string zing).
        let off = gain_at(6_000.0, 0.5, 0.0, 0.7);
        let on = gain_at(6_000.0, 0.5, 1.0, 0.7);
        assert!(on > 1.5 * off, "Top must add air: {on:.3} vs {off:.3}");
    }

    #[test]
    fn level_controls_output() {
        let quiet = gain_at(220.0, 0.5, 0.5, 0.3);
        let loud = gain_at(220.0, 0.5, 0.5, 1.0);
        assert!(
            loud > 3.0 * quiet,
            "Level must scale output: {loud:.3} vs {quiet:.3}"
        );
    }

    #[test]
    fn near_unity_at_default_knobs() {
        // A model switch onto the acoustic sim must not jump the monitors: the
        // default board sits within a few dB of unity on a broadband guitar
        // spectrum. Averaged over a spread of tones (the voicing is not flat).
        let bands = [110.0, 220.0, 440.0, 900.0, 2_000.0, 5_000.0];
        let mean: f64 = bands
            .iter()
            .map(|f| gain_at(*f, 0.5, 0.5, 0.7))
            .sum::<f64>()
            / bands.len() as f64;
        let db = 20.0 * mean.log10();
        assert!(
            db.abs() < 4.0,
            "default acoustic should sit near unity, got {db:.1} dB"
        );
    }

    #[test]
    fn stays_finite_and_bounded_across_the_knob_ranges() {
        for &(b, t, lv) in &[
            (0.0, 0.0, 0.7),
            (1.0, 1.0, 1.0),
            (1.0, 0.0, 0.5),
            (0.0, 1.0, 1.0),
        ] {
            let mut a = prepared(b, t, lv);
            let x = sine(SR, 196.0, SR as usize / 2);
            let (l, r) = process_stereo_in_blocks(&mut a, &x, 64);
            assert!(l.iter().chain(&r).all(|s| s.is_finite()), "no NaN/inf");
            assert!(peak(&l).max(peak(&r)) < 4.0, "bounded output");
        }
    }

    #[test]
    fn silence_in_silence_out_after_reset() {
        let mut a = prepared(1.0, 1.0, 1.0);
        a.reset();
        let x = vec![0.0f32; 4096];
        let (l, r) = process_stereo_in_blocks(&mut a, &x, 64);
        assert_eq!(peak(&l).max(peak(&r)), 0.0, "silence stays silent");
    }

    #[test]
    fn runs_at_studio_rates() {
        for sr in [44_100u32, 96_000] {
            let mut a = Acoustic::new();
            a.prepare(sr);
            a.set_param(0, 0.7);
            a.set_param(1, 0.7);
            a.set_param(2, 0.7);
            let x = sine(sr, 220.0, sr as usize / 2);
            let (l, _) = process_stereo_in_blocks(&mut a, &x, 96);
            assert!(l.iter().all(|s| s.is_finite()));
            assert!(rms(&l[l.len() / 2..]) > 1e-3);
        }
    }
}

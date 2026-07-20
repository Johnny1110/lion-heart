//! The pitch family: octave and interval shifters behind one chain slot.
//! The family key is deliberately broad ("pitch", not "octaver"): a
//! harmonizer, a whammy, and a detuner all have an obvious home here later,
//! and slots address families (PRD 001).
//!
//! One pedal today, one shared engine (the filter/delay-family pattern — a
//! `Ctl` routing table per faceplate, no per-sample vtable):
//!
//! - [`octaver`] — a clean Dry path mixed with a granular sub-octave and an
//!   up-octave voice (POG-flavored, chord-friendly).
//!
//! **Engine.** The two shifted voices come off the *mono sum* of the stereo
//! input, run through the shared granular shifter
//! ([`crate::blocks::grain::GrainShift`]) at the pedal's fixed ratios, get a
//! shared Tone lowpass to tame the granular fizz, and fold back onto the
//! (stereo) dry — so the octaves sit centered and fat while the dry keeps its
//! width. The shifter is feed-forward (no regeneration): it cannot run away
//! and it has no tail worth spilling ([`Effect::tail_seconds`] stays 0).
//!
//! This is a granular (time-domain) shifter, not an analog OC-2 frequency
//! divider: it tracks chords and any register at the cost of a characteristic
//! warble — that texture *is* the voice (ADR 016).

mod octaver;

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::blocks::grain::GrainShift;
use crate::blocks::onepole_hz;
use crate::blocks::smooth::Smoothed;

/// Tone-knob endpoints: the shifted-voice lowpass corner at knob 0 (dark) and
/// knob 1 (bright). Dark tucks the up-octave's granular fizz under the dry.
const TONE_MIN_HZ: f32 = 600.0;
const TONE_MAX_HZ: f32 = 9_000.0;

/// A 0..1 mix level for one voice (Dry / Sub / Oct).
const fn level_param(key: &'static str, name: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 20.0,
    }
}

/// Shifted-voice brightness 0..1 (dark ⇄ bright), mapped geometrically to the
/// lowpass corner over `[TONE_MIN_HZ, TONE_MAX_HZ]`.
const fn tone_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "tone",
        name: "Tone",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 30.0,
    }
}

/// The pitch family, in menu order. Append-only (PRD 001).
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "pitch",
    name: "Pitch",
    pedals: &[&octaver::DESC],
};

pub const PEDAL_COUNT: usize = 1;

/// Which engine control a pedal's param position drives.
#[derive(Clone, Copy)]
enum Ctl {
    /// Clean-path level.
    Dry,
    /// Down-voice (sub-octave) level.
    Sub,
    /// Up-voice (octave-up) level.
    Oct,
    /// Shifted-voice lowpass brightness.
    Tone,
}

/// One pedal's faceplate, param→control routing (same length as the
/// faceplate), and its fixed shift ratios.
pub struct PedalDef {
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    /// Pitch ratio of the down voice (`0.5` = one octave down).
    down_ratio: f32,
    /// Pitch ratio of the up voice (`2.0` = one octave up).
    up_ratio: f32,
}

/// The pedal registry, aligned with [`FAMILY`]`.pedals`.
pub static PEDALS: [PedalDef; PEDAL_COUNT] = [octaver::PEDAL];

pub struct Pitch {
    sample_rate: f32,
    pedal: usize,
    // Shared smoothers; each pedal routes its faceplate onto them through its
    // `Ctl` table, so a control means the same thing on every face.
    dry: Smoothed,
    sub: Smoothed,
    oct: Smoothed,
    tone: Smoothed,
    /// The two shifted voices come off the mono sum (centered octaves).
    down: GrainShift,
    up: GrainShift,
    /// Shared Tone lowpass state and its block-rate coefficient.
    tone_lp: f32,
    tone_coeff: f32,
}

impl Default for Pitch {
    fn default() -> Self {
        Self::new()
    }
}

impl Pitch {
    pub fn new() -> Self {
        let p = octaver::DESC.params;
        Self {
            sample_rate: 48_000.0,
            pedal: 0,
            dry: Smoothed::new(p[0].default),
            sub: Smoothed::new(p[1].default),
            oct: Smoothed::new(p[2].default),
            tone: Smoothed::new(p[3].default),
            down: GrainShift::new(),
            up: GrainShift::new(),
            tone_lp: 0.0,
            tone_coeff: 0.0,
        }
    }
}

impl Effect for Pitch {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.pedal
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal == self.pedal || pedal >= PEDALS.len() {
            return;
        }
        self.pedal = pedal;
        // Fresh state for the incoming pedal; the control side re-sends its
        // knob values from the shadow (PRD 001).
        self.reset();
    }

    fn descriptor(&self) -> &'static EffectDesc {
        PEDALS[self.pedal].desc
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for (smoothed, ms) in [
            (&mut self.dry, 20.0),
            (&mut self.sub, 20.0),
            (&mut self.oct, 20.0),
            (&mut self.tone, 30.0),
        ] {
            smoothed.configure(ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.down.prepare(self.sample_rate);
        self.up.prepare(self.sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.down.clear();
        self.up.clear();
        // Offset the up-voice grain so the two shifters' window seams don't
        // line up at t=0.
        self.up.set_phase(0.25);
        self.tone_lp = 0.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &PEDALS[self.pedal];
        // Out-of-range indices are ignored (Effect contract).
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Dry => self.dry.set_target(real),
            Ctl::Sub => self.sub.set_target(real),
            Ctl::Oct => self.oct.set_target(real),
            Ctl::Tone => self.tone.set_target(real),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let sr = self.sample_rate;
        let def = &PEDALS[self.pedal];
        // Block-rate Tone corner (one exp per block is noise, per the filter
        // family's precedent); the per-sample cost is the two shifters.
        let tone_hz =
            TONE_MIN_HZ * (TONE_MAX_HZ / TONE_MIN_HZ).powf(self.tone.current().clamp(0.0, 1.0));
        self.tone_coeff = onepole_hz(tone_hz, sr);

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let dry = self.dry.tick();
            let sub = self.sub.tick();
            let oct = self.oct.tick();
            self.tone.tick();

            let (dry_l, dry_r) = (*l, *r);
            let mono = 0.5 * (dry_l + dry_r);
            let d = self.down.process(mono, def.down_ratio);
            let u = self.up.process(mono, def.up_ratio);

            // Sum the shifted voices, darken with the shared Tone lowpass.
            let target = sub * d + oct * u;
            self.tone_lp += self.tone_coeff * (target - self.tone_lp);
            if self.tone_lp.abs() < 1e-20 {
                self.tone_lp = 0.0;
            }

            // Centered octaves fold onto the stereo dry.
            *l = dry * dry_l + self.tone_lp;
            *r = dry * dry_r + self.tone_lp;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{peak, process_stereo_in_blocks, rms, sine};

    const SR: u32 = 48_000;

    fn prepared() -> Pitch {
        let mut p = Pitch::new();
        p.prepare(SR);
        for (i, param) in octaver::DESC.params.iter().enumerate() {
            p.set_param(i, param.default_norm());
        }
        p
    }

    fn set_by(p: &mut Pitch, key: &str, real: f32) {
        let desc = PEDALS[p.pedal].desc;
        let i = desc.param_index(key).unwrap();
        p.set_param(i, desc.params[i].range.to_norm(real));
    }

    /// Projection magnitude onto `freq` over the settled tail (a Goertzel bin).
    fn tone_at(y: &[f32], freq: f32) -> f64 {
        let tail = &y[y.len() / 2..];
        let n = tail.len() as f64;
        let (mut cs, mut cc) = (0.0f64, 0.0f64);
        for (i, s) in tail.iter().enumerate() {
            let ph = 2.0 * std::f64::consts::PI * f64::from(freq) * i as f64 / f64::from(SR);
            cs += f64::from(*s) * ph.sin();
            cc += f64::from(*s) * ph.cos();
        }
        ((cs * 2.0 / n).powi(2) + (cc * 2.0 / n).powi(2)).sqrt()
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.key, "pitch");
        assert_eq!(FAMILY.pedals.len(), PEDAL_COUNT);
        assert_eq!(FAMILY.pedals[0].key, "octaver");
        for (pedal, def) in FAMILY.pedals.iter().zip(PEDALS.iter()) {
            assert!(std::ptr::eq(*pedal, def.desc), "registry order matches");
            assert_eq!(
                def.desc.params.len(),
                def.controls.len(),
                "{}: one control per param",
                def.desc.key
            );
        }
        let captions: Vec<&str> = octaver::DESC.params.iter().map(|p| p.name).collect();
        assert_eq!(captions, ["Dry", "Sub", "Oct", "Tone"]);
    }

    #[test]
    fn dry_path_is_transparent() {
        // Dry at unity, both shifted voices silent: the output is the input.
        let mut p = prepared();
        set_by(&mut p, "dry", 1.0);
        set_by(&mut p, "sub", 0.0);
        set_by(&mut p, "oct", 0.0);
        // Settle the level smoothers fully to zero (they are asymptotic) and
        // empty the grain buffers, so the shifted voices contribute nothing.
        let quiet = vec![0.0f32; SR as usize / 2];
        let _ = process_stereo_in_blocks(&mut p, &quiet, 64);
        let x = sine(SR, 330.0, 4096);
        let (l, r) = process_stereo_in_blocks(&mut p, &x, 64);
        for ((o_l, o_r), i) in l.iter().zip(&r).zip(&x) {
            assert!((o_l - i).abs() < 1e-4, "left transparent: {o_l} vs {i}");
            assert!((o_r - i).abs() < 1e-4, "right transparent: {o_r} vs {i}");
        }
    }

    #[test]
    fn sub_voice_lands_an_octave_below() {
        let mut p = prepared();
        set_by(&mut p, "dry", 0.0);
        set_by(&mut p, "sub", 1.0);
        set_by(&mut p, "oct", 0.0);
        set_by(&mut p, "tone", 1.0); // bright — don't filter the read
        let x = sine(SR, 220.0, SR as usize);
        let (l, _) = process_stereo_in_blocks(&mut p, &x, 64);
        let sub = tone_at(&l, 110.0);
        let fund = tone_at(&l, 220.0);
        assert!(sub > 0.1, "sub-octave present: {sub:.3}");
        assert!(
            sub > fund,
            "sub dominates the original pitch: {sub:.3} vs {fund:.3}"
        );
    }

    #[test]
    fn oct_voice_lands_an_octave_above() {
        let mut p = prepared();
        set_by(&mut p, "dry", 0.0);
        set_by(&mut p, "sub", 0.0);
        set_by(&mut p, "oct", 1.0);
        set_by(&mut p, "tone", 1.0);
        let x = sine(SR, 220.0, SR as usize);
        let (l, _) = process_stereo_in_blocks(&mut p, &x, 64);
        let up = tone_at(&l, 440.0);
        let fund = tone_at(&l, 220.0);
        assert!(up > 0.1, "up-octave present: {up:.3}");
        assert!(
            up > fund,
            "up dominates the original pitch: {up:.3} vs {fund:.3}"
        );
    }

    #[test]
    fn tone_darkens_the_shifted_voice() {
        // A 2 kHz input up-shifts to 4 kHz; a dark Tone must attenuate it far
        // more than a bright Tone. Dry off so we read the shifted voice alone.
        let bright = {
            let mut p = prepared();
            set_by(&mut p, "dry", 0.0);
            set_by(&mut p, "sub", 0.0);
            set_by(&mut p, "oct", 1.0);
            set_by(&mut p, "tone", 1.0);
            let x = sine(SR, 2_000.0, SR as usize / 2);
            let (l, _) = process_stereo_in_blocks(&mut p, &x, 64);
            rms(&l[l.len() / 2..])
        };
        let dark = {
            let mut p = prepared();
            set_by(&mut p, "dry", 0.0);
            set_by(&mut p, "sub", 0.0);
            set_by(&mut p, "oct", 1.0);
            set_by(&mut p, "tone", 0.0);
            let x = sine(SR, 2_000.0, SR as usize / 2);
            let (l, _) = process_stereo_in_blocks(&mut p, &x, 64);
            rms(&l[l.len() / 2..])
        };
        assert!(
            dark < bright * 0.5,
            "dark tone tucks the fizz: {dark:.4} vs {bright:.4}"
        );
    }

    #[test]
    fn stays_finite_and_bounded_across_the_knob_ranges() {
        for &(dry, sub, oct, tone) in &[
            (1.0, 1.0, 1.0, 1.0),
            (0.0, 1.0, 1.0, 0.0),
            (1.0, 0.0, 1.0, 0.5),
            (0.5, 1.0, 0.0, 1.0),
        ] {
            let mut p = prepared();
            set_by(&mut p, "dry", dry);
            set_by(&mut p, "sub", sub);
            set_by(&mut p, "oct", oct);
            set_by(&mut p, "tone", tone);
            let x = sine(SR, 196.0, SR as usize / 2);
            let (l, r) = process_stereo_in_blocks(&mut p, &x, 64);
            assert!(l.iter().chain(&r).all(|s| s.is_finite()), "no NaN/inf");
            assert!(peak(&l).max(peak(&r)) < 4.0, "bounded output");
        }
    }

    #[test]
    fn silence_in_silence_out_after_reset() {
        let mut p = prepared();
        set_by(&mut p, "sub", 1.0);
        set_by(&mut p, "oct", 1.0);
        p.reset();
        let x = vec![0.0f32; 4096];
        let (l, r) = process_stereo_in_blocks(&mut p, &x, 64);
        assert_eq!(peak(&l).max(peak(&r)), 0.0, "silence stays silent");
    }
}

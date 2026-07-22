//! Compressor: a family of three classic compression *topologies* behind one
//! chain slot (PRD 001/015), all built on one detector → gain-computer engine.
//! Each pedal owns its faceplate — its own knob set, time-constant laws, and
//! character:
//!
//! - **vca** (dbx-style VCA): the original transparent digital compressor —
//!   full Threshold/Ratio/Attack/Release control, a hard knee. The clean
//!   leveler. Old single-`comp` presets migrate onto it (schema v7→v8), and
//!   at its defaults it is bit-for-bit the pre-family compressor (bar a
//!   denormal flush on the detector envelope, inaudible below −400 dB).
//! - **opto** (Teletronix LA-2A-style): a slow, round optical leveler — a
//!   fixed slow attack, a **program-dependent** two-stage release (the deeper
//!   it is compressing, the slower it recovers), a "ratio" that *rises* toward
//!   limiting as you push it, and a soft knee. The faceplate is just
//!   Peak Reduction / Gain — the optical cell owns attack and ratio.
//! - **fet** (UREI 1176-style): a fast FET peak limiter — microsecond attack,
//!   a hard knee, and a **stepped** ratio (4/8/12/20/All) whose "all-buttons-in"
//!   position drops the threshold and slams into the famous aggressive pump.
//!   Fast enough to shape transients.
//!
//! One engine, three voicings: the per-sample loop reads the active voice's
//! [`VoiceDef`] constants (like [`crate::time::delay`] and
//! [`crate::modulation`]) — no per-sample vtable. Switching pedals keeps the
//! detector envelope (a smooth handover) and the control side re-sends the
//! incoming pedal's knob values (PRD 001).
//!
//! Two knobs are **shared** across all three faceplates (PRD 015):
//! - **blend**: parallel / New-York compression — the output is
//!   `dry·(1−blend) + compressed·blend`, so `blend = 0` is bit-transparent dry
//!   and `blend = 1` is fully compressed (the vca default, = the old behavior).
//! - **sc_hpf**: a high-pass on the *sidechain* only (20–300 Hz), so bass stops
//!   ducking the whole mix. At its 20 Hz minimum the detector reads the raw
//!   signal (bypassed) — which is what keeps the vca migration exact.

mod fet;
mod opto;
mod vca;

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range, db_to_lin, lin_to_db};

use crate::Effect;
use crate::blocks::smooth::Smoothed;
use crate::blocks::{onepole_hz, onepole_ms};

/// Sidechain high-pass floor: at this corner (its minimum) the detector reads
/// the raw signal, so a default faceplate leaves detection full-band — the
/// vca migration stays exact.
const SC_HPF_MIN_HZ: f32 = 20.0;

/// Opto Peak-Reduction depth: the knob (0..1) maps to a threshold of
/// `0 dB .. −OPTO_PR_DEPTH_DB`, so more reduction = a lower threshold =
/// heavier leveling.
const OPTO_PR_DEPTH_DB: f32 = 48.0;

/// Over-threshold span (dB) across which the opto's program-dependent release
/// slides from its fast to its slow time constant, and its rising ratio climbs
/// from `base` to `top`.
const OPTO_PROGRAM_RANGE_DB: f32 = 24.0;

/// FET "all-buttons-in": on top of the 20:1 ratio the threshold drops by this
/// much, so the whole signal slams into gain reduction (the aggressive pump).
const FET_ALL_OFFSET_DB: f32 = -8.0;

/// The fet voice's stepped ratio labels (4:1 … all-buttons-in).
pub const FET_RATIOS: &[&str] = &["4:1", "8:1", "12:1", "20:1", "All"];

// --- shared faceplate parameters ---------------------------------------------
// Every voice reuses these keys/ranges so a shared knob means the same thing
// across pedals (and the old flat `comp` params migrate cleanly onto vca);
// each voice picks its own defaults and, where it applies, its own time ranges.

const fn threshold_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "threshold",
        name: "Threshold",
        unit: "dB",
        range: Range::Linear {
            min: -60.0,
            max: 0.0,
        },
        default,
        smoothing_ms: 0.0,
    }
}

const fn ratio_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "ratio",
        name: "Ratio",
        unit: ":1",
        range: Range::Linear {
            min: 1.0,
            max: 20.0,
        },
        default,
        smoothing_ms: 0.0,
    }
}

/// The fet voice's stepped ratio: 4:1 / 8:1 / 12:1 / 20:1 / all-buttons-in.
const fn ratio_step_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "ratio",
        name: "Ratio",
        unit: "",
        range: Range::Stepped { labels: FET_RATIOS },
        default,
        smoothing_ms: 0.0,
    }
}

const fn attack_param(min: f32, max: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "attack",
        name: "Attack",
        unit: "ms",
        range: Range::Log { min, max },
        default,
        smoothing_ms: 0.0,
    }
}

const fn release_param(min: f32, max: f32, default: f32) -> ParamDesc {
    ParamDesc {
        key: "release",
        name: "Release",
        unit: "ms",
        range: Range::Log { min, max },
        default,
        smoothing_ms: 0.0,
    }
}

/// Makeup / output gain (dB). `key`/`name` vary — vca and fet call it Makeup,
/// opto calls it Gain — but they all land on the same engine control.
const fn makeup_param(key: &'static str, name: &'static str, default: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "dB",
        range: Range::Linear {
            min: 0.0,
            max: 24.0,
        },
        default,
        smoothing_ms: 30.0,
    }
}

/// Opto Peak Reduction (0..1) — the LA-2A's one compression knob, mapped to a
/// threshold. Not smoothed: like a threshold knob it only shifts the gain
/// computer, which the envelope follower then smooths.
const fn peak_reduction_param(default: f32) -> ParamDesc {
    ParamDesc {
        key: "peak_reduction",
        name: "Peak Reduct",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default,
        smoothing_ms: 0.0,
    }
}

/// Parallel-compression blend (0 = dry/bit-transparent, 1 = fully compressed).
const fn blend_param() -> ParamDesc {
    ParamDesc {
        key: "blend",
        name: "Blend",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 1.0,
        smoothing_ms: 20.0,
    }
}

/// Sidechain high-pass corner (Hz). At the 20 Hz minimum the detector is
/// full-band (bypassed); higher settings stop bass from driving compression.
const fn sc_hpf_param() -> ParamDesc {
    ParamDesc {
        key: "sc_hpf",
        name: "SC HPF",
        unit: "Hz",
        range: Range::Log {
            min: SC_HPF_MIN_HZ,
            max: 300.0,
        },
        default: SC_HPF_MIN_HZ,
        smoothing_ms: 0.0,
    }
}

/// The compressor family, in menu order. Pinned to
/// `lh_core::preset::COMP_PEDALS` (the v7→v8 migration) by a test below.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "comp",
    name: "Compressor",
    pedals: &[&vca::DESC, &opto::DESC, &fet::DESC],
};

pub const VOICE_COUNT: usize = 3;

/// Which engine control a voice's param position drives. A voice's `controls`
/// slice is the same length as its faceplate; `set_param` reads it to route.
#[derive(Clone, Copy)]
enum Ctl {
    /// Threshold in dBFS (vca, fet).
    Threshold,
    /// Opto Peak Reduction (0..1) → threshold.
    PeakReduction,
    /// Continuous ratio 1..20 (vca).
    Ratio,
    /// Stepped ratio incl. all-buttons-in (fet).
    RatioStep,
    Attack,
    Release,
    /// Makeup (vca/fet) or Gain (opto) — same control.
    Makeup,
    Blend,
    ScHpf,
}

/// How a voice derives its compression ratio.
#[derive(Clone, Copy)]
enum RatioMode {
    /// Read straight from the continuous Ratio knob (vca).
    Knob,
    /// Program-dependent: the effective ratio climbs from `base` toward `top`
    /// as the signal pushes further over threshold (opto — leveling → limiting).
    Rising { base: f32, top: f32 },
    /// Read from the stepped Ratio selector (fet); "All" also drops threshold.
    Stepped,
}

/// One voice's faceplate, param→control routing (same length as the
/// faceplate), and topology constants. The engine reads these in the hot loop
/// instead of dispatching through a trait.
pub struct VoiceDef {
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    /// Soft-knee width in dB (0 = hard knee). vca/fet hard, opto soft.
    knee_db: f32,
    ratio_mode: RatioMode,
    /// Opto's two-stage release. When false the Release knob rules (vca/fet).
    program_release: bool,
    /// Attack used when the voice exposes no Attack knob (opto). Knob voices
    /// set this to their default; the control side then overrides it.
    fixed_attack_ms: f32,
    /// Program-dependent release endpoints (opto), fast → slow; ignored when
    /// `program_release` is false.
    release_fast_ms: f32,
    release_slow_ms: f32,
}

/// The compressor voice registry, aligned with [`FAMILY`]`.pedals`.
pub static VOICES: [VoiceDef; VOICE_COUNT] = [vca::VOICE, opto::VOICE, fet::VOICE];

pub struct Compressor {
    sample_rate: u32,
    voice: usize,
    // Effective controls — some come from knobs, some are fixed by the voice.
    threshold_db: f32,
    ratio: f32,
    /// fet all-buttons-in (drops the threshold); only read in `Stepped` mode.
    all_buttons: bool,
    attack_ms: f32,
    release_ms: f32,
    attack_coeff: f32,
    release_coeff: f32,
    /// Slow-release coefficient for the opto's program-dependent recovery.
    release_slow_coeff: f32,
    makeup: Smoothed,
    blend: Smoothed,
    // Sidechain high-pass (detector only): a per-channel one-pole low-pass we
    // subtract to get the high-passed signal, then rectify. `sc_hpf_hz` is
    // stored so `prepare` can rebuild the coefficient at a new sample rate.
    sc_hpf_hz: f32,
    sc_active: bool,
    sc_coeff: f32,
    sc_lp: [f32; 2],
    /// Linked detector envelope (one gain for both channels — image stays put).
    env: f32,
}

impl Default for Compressor {
    fn default() -> Self {
        Self::new()
    }
}

impl Compressor {
    pub fn new() -> Self {
        let mut comp = Self {
            sample_rate: 48_000,
            voice: 0,
            threshold_db: vca::DESC.params[0].default,
            ratio: vca::DESC.params[1].default,
            all_buttons: false,
            attack_ms: vca::DESC.params[2].default,
            release_ms: vca::DESC.params[3].default,
            attack_coeff: 0.0,
            release_coeff: 0.0,
            release_slow_coeff: 0.0,
            makeup: Smoothed::new(db_to_lin(vca::DESC.params[4].default)),
            blend: Smoothed::new(blend_param().default),
            sc_hpf_hz: SC_HPF_MIN_HZ,
            sc_active: false,
            sc_coeff: 0.0,
            sc_lp: [0.0; 2],
            env: 0.0,
        };
        comp.apply_voice_baseline();
        comp.sc_coeff = onepole_hz(SC_HPF_MIN_HZ, comp.sample_rate as f32);
        comp
    }

    /// Set the time constants the active voice does not expose as knobs (opto's
    /// fixed attack, and its program-release endpoints), then rebuild the
    /// envelope coefficients. Knob voices set attack to their default here and
    /// the control side re-sends the real value.
    fn apply_voice_baseline(&mut self) {
        self.attack_ms = VOICES[self.voice].fixed_attack_ms;
        self.recompute_time();
    }

    fn recompute_time(&mut self) {
        let def = &VOICES[self.voice];
        self.attack_coeff = onepole_ms(self.attack_ms, self.sample_rate);
        if def.program_release {
            self.release_coeff = onepole_ms(def.release_fast_ms, self.sample_rate);
            self.release_slow_coeff = onepole_ms(def.release_slow_ms, self.sample_rate);
        } else {
            self.release_coeff = onepole_ms(self.release_ms, self.sample_rate);
        }
    }
}

impl Effect for Compressor {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn pedal_index(&self) -> usize {
        self.voice
    }

    fn select_pedal(&mut self, pedal: usize) {
        if pedal != self.voice && pedal < VOICE_COUNT {
            self.voice = pedal;
            // Install the incoming voice's fixed time constants; keep the
            // detector envelope so gain reduction hands over smoothly. The
            // control side re-sends the exposed knob values right after.
            self.apply_voice_baseline();
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        // Rebuild coefficients from the stored values (a rate change must not
        // clobber knob-set attack/release the way a pedal switch does).
        self.recompute_time();
        self.makeup.configure(30.0, sample_rate);
        self.makeup.snap_to_target();
        self.blend.configure(20.0, sample_rate);
        self.blend.snap_to_target();
        self.sc_active = self.sc_hpf_hz > SC_HPF_MIN_HZ + 0.5;
        self.sc_coeff = onepole_hz(self.sc_hpf_hz, sample_rate as f32);
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        self.sc_lp = [0.0; 2];
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &VOICES[self.voice];
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Threshold => self.threshold_db = real,
            Ctl::PeakReduction => self.threshold_db = -OPTO_PR_DEPTH_DB * real.clamp(0.0, 1.0),
            Ctl::Ratio => self.ratio = real,
            Ctl::RatioStep => {
                let (ratio, all) = match real.round().max(0.0) as usize {
                    0 => (4.0, false),
                    1 => (8.0, false),
                    2 => (12.0, false),
                    3 => (20.0, false),
                    _ => (20.0, true),
                };
                self.ratio = ratio;
                self.all_buttons = all;
            }
            Ctl::Attack => {
                self.attack_ms = real;
                self.attack_coeff = onepole_ms(real, self.sample_rate);
            }
            Ctl::Release => {
                self.release_ms = real;
                self.release_coeff = onepole_ms(real, self.sample_rate);
            }
            Ctl::Makeup => self.makeup.set_target(db_to_lin(real)),
            Ctl::Blend => self.blend.set_target(real),
            Ctl::ScHpf => {
                self.sc_hpf_hz = real;
                self.sc_active = real > SC_HPF_MIN_HZ + 0.5;
                self.sc_coeff = onepole_hz(real, self.sample_rate as f32);
            }
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let def = &VOICES[self.voice];
        let knee = def.knee_db;
        let is_stepped = matches!(def.ratio_mode, RatioMode::Stepped);
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            // --- detector: linked peak, with an optional sidechain high-pass ---
            let det = if self.sc_active {
                let hl = *l - self.sc_lp[0];
                self.sc_lp[0] += self.sc_coeff * (*l - self.sc_lp[0]);
                let hr = *r - self.sc_lp[1];
                self.sc_lp[1] += self.sc_coeff * (*r - self.sc_lp[1]);
                hl.abs().max(hr.abs())
            } else {
                l.abs().max(r.abs())
            };

            // --- envelope follower: attack when rising, release when falling.
            // The opto's release is program-dependent — the deeper the current
            // gain reduction, the slower the recovery.
            let coeff = if det > self.env {
                self.attack_coeff
            } else if def.program_release {
                let over = lin_to_db(self.env.max(1e-9)) - self.threshold_db;
                let depth = (over / OPTO_PROGRAM_RANGE_DB).clamp(0.0, 1.0);
                self.release_coeff + (self.release_slow_coeff - self.release_coeff) * depth
            } else {
                self.release_coeff
            };
            self.env += coeff * (det - self.env);
            if self.env < 1e-20 {
                self.env = 0.0; // flush detector denormals (RT rule 7)
            }

            // --- gain computer (dB domain, per-voice knee & ratio law) ---
            let eff_threshold = if is_stepped && self.all_buttons {
                self.threshold_db + FET_ALL_OFFSET_DB
            } else {
                self.threshold_db
            };
            let over = lin_to_db(self.env.max(1e-9)) - eff_threshold;
            let ratio = match def.ratio_mode {
                RatioMode::Knob | RatioMode::Stepped => self.ratio,
                RatioMode::Rising { base, top } => {
                    base + (top - base) * (over / OPTO_PROGRAM_RANGE_DB).clamp(0.0, 1.0)
                }
            };
            let slope = 1.0 / ratio - 1.0; // dB of gain per dB over threshold
            let gr_db = if knee <= 0.0 {
                if over > 0.0 { over * slope } else { 0.0 }
            } else {
                // Quadratic soft knee centered on the threshold.
                let half = knee * 0.5;
                if over <= -half {
                    0.0
                } else if over >= half {
                    over * slope
                } else {
                    let x = over + half;
                    slope * x * x / (2.0 * knee)
                }
            };

            let gain = db_to_lin(gr_db) * self.makeup.tick();
            let b = self.blend.tick();
            // Parallel compression: blend the compressed signal against the dry
            // input. blend 1 → fully compressed; blend 0 → bit-transparent dry.
            *l = *l * (1.0 - b) + (*l * gain) * b;
            *r = *r * (1.0 - b) + (*r * gain) * b;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_in_blocks, rms, silence, sine};

    const SR: u32 = 48_000;

    const VCA: usize = 0;
    const OPTO: usize = 1;
    const FET: usize = 2;

    fn prepared(voice: usize) -> Compressor {
        let mut c = Compressor::new();
        c.prepare(SR);
        c.select_pedal(voice);
        c
    }

    /// The param index of `key` on the active voice.
    fn idx(c: &Compressor, key: &str) -> usize {
        VOICES[c.voice].desc.param_index(key).unwrap()
    }

    /// Set a param by real value at the active voice.
    fn set_by(c: &mut Compressor, key: &str, real: f32) {
        let i = idx(c, key);
        let param = &VOICES[c.voice].desc.params[i];
        c.set_param(i, param.range.to_norm(real));
    }

    /// Steady-state peak of the processed tail of a sine at `amp`.
    fn settled_peak(c: &mut Compressor, amp: f32) -> f32 {
        let x: Vec<f32> = sine(SR, 220.0, SR as usize)
            .iter()
            .map(|s| s * amp)
            .collect();
        let y = process_in_blocks(c, &x, 256);
        assert_finite("comp output", &y);
        peak(&y[SR as usize / 2..])
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.pedals.len(), VOICES.len());
        for (def, desc) in VOICES.iter().zip(FAMILY.pedals) {
            assert!(std::ptr::eq(def.desc, *desc), "VOICES aligned with FAMILY");
            assert_eq!(def.controls.len(), def.desc.params.len());
        }
        // Keys are unique (REPL/preset-facing identifiers).
        for (i, a) in FAMILY.pedals.iter().enumerate() {
            for b in &FAMILY.pedals[i + 1..] {
                assert_ne!(a.key, b.key);
            }
        }
        // The v7→v8 migration references voices by key and index; pin them.
        let keys: Vec<&str> = FAMILY.pedals.iter().map(|p| p.key).collect();
        assert_eq!(keys, lh_core::preset::COMP_PEDALS);
        // Every voice carries the two shared knobs (PRD 015).
        for pedal in FAMILY.pedals {
            for key in ["blend", "sc_hpf"] {
                assert!(
                    pedal.param_index(key).is_some(),
                    "{} lacks {key}",
                    pedal.key
                );
            }
            // blend ships fully-compressed so a fresh pedal behaves classically.
            let blend = &pedal.params[pedal.param_index("blend").unwrap()];
            assert_eq!(blend.default, 1.0, "{}: blend ships at 1.0", pedal.key);
        }
        // Each voice wears its own faceplate.
        let captions =
            |i: usize| -> Vec<&str> { FAMILY.pedals[i].params.iter().map(|p| p.name).collect() };
        assert_eq!(
            captions(VCA),
            [
                "Threshold",
                "Ratio",
                "Attack",
                "Release",
                "Makeup",
                "Blend",
                "SC HPF"
            ]
        );
        assert_eq!(captions(OPTO), ["Peak Reduct", "Gain", "Blend", "SC HPF"]);
        assert_eq!(
            captions(FET),
            [
                "Threshold",
                "Attack",
                "Release",
                "Ratio",
                "Makeup",
                "Blend",
                "SC HPF"
            ]
        );
        // fet's ratio is stepped, vca's is continuous.
        assert!(matches!(
            FAMILY.pedals[FET].params[idx(&prepared(FET), "ratio")].range,
            Range::Stepped { .. }
        ));
        assert!(matches!(
            FAMILY.pedals[VCA].params[idx(&prepared(VCA), "ratio")].range,
            Range::Linear { .. }
        ));
    }

    // --- vca: the pre-family compressor, preserved (migration parity) --------

    #[test]
    fn vca_below_threshold_is_unity() {
        let mut c = prepared(VCA);
        let peak = settled_peak(&mut c, db_to_lin(-30.0)); // −30 dBFS under −24
        let err_db = lin_to_db(peak) - -30.0;
        assert!(err_db.abs() < 0.2, "unity below threshold, off {err_db} dB");
    }

    #[test]
    fn vca_static_curve_matches_ratio() {
        let mut c = prepared(VCA);
        // −6 dBFS into threshold −24, ratio 4: 18 dB over → 13.5 dB reduction.
        let out_db = lin_to_db(settled_peak(&mut c, db_to_lin(-6.0)));
        let expected = -6.0 - 18.0 * (1.0 - 1.0 / 4.0);
        assert!(
            (out_db - expected).abs() < 1.0,
            "expected ≈ {expected} dBFS, got {out_db}"
        );
    }

    #[test]
    fn vca_higher_ratio_compresses_more() {
        let mut soft = prepared(VCA);
        set_by(&mut soft, "ratio", 2.0);
        let mut hard = prepared(VCA);
        set_by(&mut hard, "ratio", 12.0);
        let loud = db_to_lin(-6.0);
        assert!(
            settled_peak(&mut hard, loud) < settled_peak(&mut soft, loud) * 0.8,
            "ratio 12 must reduce more than ratio 2"
        );
    }

    #[test]
    fn vca_makeup_gain_applies() {
        let mut c = prepared(VCA);
        set_by(&mut c, "makeup", 12.0);
        let peak = settled_peak(&mut c, db_to_lin(-30.0));
        let err_db = lin_to_db(peak) - (-30.0 + 12.0);
        assert!(err_db.abs() < 0.5, "makeup +12 dB, off {err_db} dB");
    }

    // --- shared knobs: blend + sidechain HPF (PRD 015) -----------------------

    #[test]
    fn blend_zero_is_bit_transparent() {
        // Parallel blend at 0 must return the dry input verbatim, even with a
        // slammed compressor and makeup behind it.
        let mut c = prepared(VCA);
        set_by(&mut c, "threshold", -40.0);
        set_by(&mut c, "ratio", 20.0);
        set_by(&mut c, "makeup", 18.0);
        set_by(&mut c, "blend", 0.0);
        // Settle the blend smoother fully to 0 (one second ≫ the 20 ms law).
        let warm: Vec<f32> = sine(SR, 220.0, SR as usize)
            .iter()
            .map(|s| s * 0.5)
            .collect();
        let _ = process_in_blocks(&mut c, &warm, 256);
        let x: Vec<f32> = sine(SR, 330.0, 4_096).iter().map(|s| s * 0.5).collect();
        let y = process_in_blocks(&mut c, &x, 256);
        for (o, i) in y.iter().zip(&x) {
            assert!((o - i).abs() < 1e-6, "blend 0 off by {}", (o - i).abs());
        }
    }

    #[test]
    fn sc_hpf_spares_the_low_end() {
        // With the sidechain high-passed, a loud *low* tone drives far less
        // gain reduction than an equally-loud *mid* tone.
        let gr_at = |freq: f32, sc_hz: f32| -> f32 {
            let mut c = prepared(VCA);
            set_by(&mut c, "threshold", -30.0);
            set_by(&mut c, "ratio", 8.0);
            set_by(&mut c, "sc_hpf", sc_hz);
            let amp = db_to_lin(-6.0);
            let x: Vec<f32> = sine(SR, freq, SR as usize)
                .iter()
                .map(|s| s * amp)
                .collect();
            let y = process_in_blocks(&mut c, &x, 256);
            // Gain reduction = how far the output falls below the input peak.
            lin_to_db(amp) - lin_to_db(peak(&y[SR as usize / 2..]))
        };
        let low_hp = gr_at(60.0, 250.0);
        let mid_hp = gr_at(500.0, 250.0);
        assert!(
            mid_hp > low_hp + 4.0,
            "sidechain HPF must spare lows: 60 Hz GR {low_hp:.1} dB, 500 Hz GR {mid_hp:.1} dB"
        );
        // At the 20 Hz floor the detector is full-band: a low tone compresses.
        let low_flat = gr_at(60.0, 20.0);
        assert!(
            low_flat > low_hp + 4.0,
            "bypassed sidechain must compress lows again: {low_flat:.1} vs {low_hp:.1} dB"
        );
    }

    // --- topology signatures -------------------------------------------------

    /// Output RMS over the first 5 ms of a loud onset — a proxy for attack
    /// speed: a fast compressor has already clamped (low energy), a slow one
    /// is still passing the transient (high energy).
    fn early_energy(voice: usize) -> f32 {
        let mut c = prepared(voice);
        if voice == OPTO {
            set_by(&mut c, "peak_reduction", 0.8);
        } else {
            set_by(&mut c, "threshold", -30.0);
        }
        let amp = db_to_lin(-6.0);
        let x: Vec<f32> = sine(SR, 1_000.0, SR as usize / 2)
            .iter()
            .map(|s| s * amp)
            .collect();
        let y = process_in_blocks(&mut c, &x, 64);
        let five_ms = (SR as usize) * 5 / 1_000;
        rms(&y[..five_ms])
    }

    #[test]
    fn fet_attacks_faster_than_opto() {
        // The microsecond FET has clamped within 5 ms; the 10 ms opto attack
        // is still letting the onset through.
        let fet = early_energy(FET);
        let opto = early_energy(OPTO);
        assert!(
            fet < opto * 0.8,
            "fet must clamp sooner than opto: fet {fet:.4}, opto {opto:.4}"
        );
    }

    #[test]
    fn opto_release_is_program_dependent() {
        // Deep gain reduction recovers slower than shallow — the LA-2A trait.
        // Charge with a loud burst, then measure how fast the output recovers
        // toward a quiet steady tone after the burst ends.
        let recovery = |burst_db: f32| -> f32 {
            let mut c = prepared(OPTO);
            set_by(&mut c, "peak_reduction", 0.7);
            let quiet = db_to_lin(-24.0);
            let loud = db_to_lin(burst_db);
            let mut x: Vec<f32> = sine(SR, 220.0, SR as usize / 4)
                .iter()
                .map(|s| s * loud)
                .collect();
            x.extend(sine(SR, 220.0, SR as usize).iter().map(|s| s * quiet));
            let y = process_in_blocks(&mut c, &x, 64);
            // Output level 50 ms after the burst ends — lower = still ducked.
            let at = SR as usize / 4 + (SR as usize * 50 / 1_000);
            peak(&y[at..at + SR as usize / 20])
        };
        let deep = recovery(0.0); // 0 dBFS burst → heavy reduction
        let shallow = recovery(-18.0); // gentle burst → light reduction
        assert!(
            shallow > deep * 1.15,
            "deep compression must release slower: deep {deep:.4}, shallow {shallow:.4}"
        );
    }

    #[test]
    fn fet_all_buttons_slams_hardest_and_stays_bounded() {
        let gr_at_step = |step: f32| -> f32 {
            let mut c = prepared(FET);
            set_by(&mut c, "threshold", -24.0);
            let i = idx(&c, "ratio");
            c.set_param(i, VOICES[FET].desc.params[i].range.to_norm(step));
            let amp = db_to_lin(-3.0);
            let x: Vec<f32> = sine(SR, 220.0, SR as usize)
                .iter()
                .map(|s| s * amp)
                .collect();
            let y = process_in_blocks(&mut c, &x, 128);
            assert_finite("fet", &y);
            lin_to_db(amp) - lin_to_db(peak(&y[SR as usize / 2..]))
        };
        let four = gr_at_step(0.0); // 4:1
        let all = gr_at_step(4.0); // all-buttons-in
        assert!(
            all > four + 3.0,
            "all-buttons must slam harder than 4:1: {four:.1} → {all:.1} dB"
        );
        assert!(
            all.is_finite() && all < 60.0,
            "all-buttons GR bounded: {all}"
        );
    }

    #[test]
    fn topologies_are_audibly_distinct() {
        // The same slammed input through each voice must leave measurably
        // different steady-state levels — three real topologies, not one.
        let level = |voice: usize| -> f32 {
            let mut c = prepared(voice);
            let amp = db_to_lin(-3.0);
            settled_peak(&mut c, amp)
        };
        let (v, o, f) = (level(VCA), level(OPTO), level(FET));
        assert!(
            (lin_to_db(v) - lin_to_db(o)).abs() > 1.0
                || (lin_to_db(v) - lin_to_db(f)).abs() > 1.0
                || (lin_to_db(o) - lin_to_db(f)).abs() > 1.0,
            "voices must differ: vca {v:.4}, opto {o:.4}, fet {f:.4}"
        );
    }

    // --- family-wide invariants ----------------------------------------------

    #[test]
    fn every_voice_is_finite_bounded_and_silent_in_silent_out() {
        for (voice, def) in VOICES.iter().enumerate() {
            let mut c = prepared(voice);
            // Slam every exposed knob to an extreme and hold a loud tone.
            for p in def.desc.params {
                c.set_param(idx(&c, p.key), 1.0);
            }
            let x: Vec<f32> = sine(SR, 220.0, SR as usize)
                .iter()
                .map(|s| s * 4.0)
                .collect();
            let y = process_in_blocks(&mut c, &x, 200);
            assert_finite(def.desc.key, &y);
            assert!(peak(&y) < 64.0, "{}: bounded", def.desc.key);

            c.reset();
            let s = silence(SR as usize / 2);
            let y = process_in_blocks(&mut c, &s, 128);
            assert!(rms(&y) == 0.0, "{}: silence in → out", def.desc.key);
        }
    }

    #[test]
    fn pedal_switch_mid_note_stays_finite() {
        let mut c = prepared(VCA);
        set_by(&mut c, "threshold", -30.0);
        let x: Vec<f32> = sine(SR, 220.0, SR as usize / 2)
            .iter()
            .map(|s| s * 0.8)
            .collect();
        let mut left = x.clone();
        let mut right = x.clone();
        for (i, (bl, br)) in left.chunks_mut(64).zip(right.chunks_mut(64)).enumerate() {
            c.select_pedal(i % VOICE_COUNT);
            c.process(bl, br);
        }
        assert_finite("comp pedal switch", &left);
        assert!(peak(&left) < 8.0);
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            for voice in 0..VOICE_COUNT {
                let mut c = Compressor::new();
                c.prepare(sr);
                c.select_pedal(voice);
                for chunk in [32usize, 483, 1_024] {
                    let x: Vec<f32> = sine(sr, 440.0, 4_096).iter().map(|s| s * 0.7).collect();
                    let y = process_in_blocks(&mut c, &x, chunk);
                    assert_finite("comp multirate", &y);
                }
            }
        }
    }
}

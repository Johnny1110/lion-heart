//! Power amp: a hand-written valve power-stage that sits between the amp
//! (NAM preamp capture) and the cab. The NAM ecosystem is overwhelmingly
//! *preamp-only* captures — they have no push-pull power section, so they
//! miss the sag, the compression and the transformer thump that make a real
//! amp feel alive. This pedal supplies that behavioral layer (PRD 017,
//! ADR 024).
//!
//! It ships **bypassed** on the default board ([`lh_core::default_active`]):
//! a full-amp capture already contains a power stage, so double-stacking
//! would colour twice. Preamp-only players light the LED to bring the amp to
//! life.
//!
//! Signal path per channel:
//! ```text
//!   presence pre-emphasis ─▶ 4× push-pull waveshaper (sag-modulated)
//!                            ─▶ output transformer (low-cut + core sat)
//!                            ─▶ depth low-shelf ─▶ master
//! ```
//! The nonlinear stage runs inside the shared 4× oversampler
//! ([`crate::blocks::oversample`]); the linear shaping runs at the base rate.
//! One **linked** sag detector drives both channels — a push-pull amp has one
//! shared power supply, so a loud chord sags the whole stage together.
//!
//! Presence is modelled *before* the clipper (a negative-feedback high-end
//! lift makes the top break up — the authentic "presence" behaviour), while
//! depth/resonance is modelled *after* the transformer (it is a
//! power-tube/speaker low-frequency resonance, not a preamp EQ) — see ADR 024
//! for why this deviates from the PRD's "both pre-shaping" sketch.

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range, db_to_lin, drive_law::level_lin};

use crate::Effect;
use crate::blocks::oversample::{CHUNK, Oversampler4x};
use crate::blocks::smooth::Smoothed;
use crate::blocks::{onepole_hz, onepole_ms};

/// A pedal-style position knob `0..=10`.
const fn knob(key: &'static str, name: &'static str, default: f32, smoothing_ms: f32) -> ParamDesc {
    ParamDesc {
        key,
        name,
        unit: "",
        range: Range::Linear {
            min: 0.0,
            max: 10.0,
        },
        default,
        smoothing_ms,
    }
}

static PARAMS: [ParamDesc; 5] = [
    knob("drive", "Drive", 4.0, 20.0),
    knob("sag", "Sag", 4.0, 30.0),
    knob("presence", "Presence", 3.0, 30.0),
    knob("depth", "Depth", 3.0, 30.0),
    knob("master", "Master", 6.0, 20.0),
];

pub static DESC: EffectDesc = EffectDesc {
    key: "power",
    name: "Power Amp",
    params: &PARAMS,
};

/// Single-pedal family: the pedal key doubles as the family key.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "power",
    name: "Power Amp",
    pedals: &[&DESC],
};

// --- voicing constants -------------------------------------------------------

/// Push-pull operating-point offset. A class-AB stage runs its two tubes
/// slightly unbalanced; the fixed bias skews the transfer curve so the
/// clipped waveform is asymmetric — the even harmonics that fatten a power
/// amp. A post-clip DC offset would just be a level the transformer's HP
/// erases (symmetric square, no evens); the bias must sit *inside* the
/// nonlinearity.
const BIAS: f32 = 0.25;
/// Post-shaper makeup so drive 5 / master 6 lands near unity loudness.
const MAKEUP: f32 = 0.42;

/// Presence: high-shelf pre-emphasis corner and its ceiling (dB at knob 10).
const PRESENCE_HZ: f32 = 3_000.0;
const PRESENCE_MAX_DB: f32 = 8.0;
/// Depth: post-transformer low-shelf resonance corner and ceiling.
const DEPTH_HZ: f32 = 100.0;
const DEPTH_MAX_DB: f32 = 8.0;
/// Output transformer low-frequency limit (also the DC blocker — a one-pole
/// high-pass fully rejects the shaper's asymmetric DC).
const OT_HP_HZ: f32 = 35.0;
/// Transformer core: a gentle `tanh(k·x)/k` that is near-transparent at
/// nominal level and only rounds large peaks (the "iron" softness).
const OT_DRIVE: f32 = 0.9;

/// Sag: maximum supply droop (fraction) at knob 10, the floor the rail never
/// falls below, and the input level (`SAG_REF`, ≈ −12 dBFS) that reaches full
/// droop. `INV_SAG_REF` avoids a per-sample divide.
const SAG_MAX: f32 = 0.55;
const SUPPLY_MIN: f32 = 0.40;
const SAG_REF: f32 = 0.25;
const INV_SAG_REF: f32 = 1.0 / SAG_REF;
const SAG_ATTACK_MS: f32 = 8.0;
const SAG_RELEASE_MS: f32 = 180.0;

/// Drive knob → pre-clip gain (linear). 0 → −6 dB (barely pushed), 10 →
/// +24 dB (a cranked power amp saturating hard).
#[inline]
fn drive_gain(pos: f32) -> f32 {
    db_to_lin(-6.0 + 3.0 * pos)
}

/// A boost-only shelf's added-band multiplier for a knob position `0..10`:
/// 0 at knob 0 (flat), `db_to_lin(max_db) − 1` at knob 10.
#[inline]
fn shelf_add(max_db: f32, pos: f32) -> f32 {
    db_to_lin(max_db * pos * 0.1) - 1.0
}

// --- shared building blocks (local copies of the drive family's idioms) ------

/// One-pole lowpass state; highpass is the input minus this. The flush keeps
/// decaying feedback out of denormal territory (RT rule 7).
#[derive(Default)]
struct OnePole {
    y: f32,
}

impl OnePole {
    #[inline]
    fn lp(&mut self, x: f32, c: f32) -> f32 {
        self.y += c * (x - self.y);
        if self.y.abs() < 1e-20 {
            self.y = 0.0;
        }
        self.y
    }

    fn reset(&mut self) {
        self.y = 0.0;
    }
}

/// Per-sample linear ramp between a chunk's first and last mapped knob values
/// — the mapping (`powf`) runs twice per chunk instead of per sample.
struct Ramp {
    v: f32,
    step: f32,
}

impl Ramp {
    fn over(traj: &[f32], f: impl Fn(f32) -> f32) -> Self {
        let a = f(traj[0]);
        let b = f(traj[traj.len() - 1]);
        Self {
            v: a,
            step: (b - a) / traj.len() as f32,
        }
    }

    #[inline]
    fn tick(&mut self) -> f32 {
        let v = self.v;
        self.v += self.step;
        v
    }
}

// --- the effect --------------------------------------------------------------

pub struct PowerAmp {
    os: [Oversampler4x; 2],
    /// Presence pre-emphasis, transformer low-cut, and depth resonance filter
    /// state — one per channel.
    presence_lp: [OnePole; 2],
    ot_hp: [OnePole; 2],
    depth_lp: [OnePole; 2],
    /// Linked sag envelope (one shared supply — see the module docs).
    env: f32,
    // Smoothed knob values. `drive` is ticked at the oversampled rate (its
    // trajectory feeds the shaper); the rest at the base rate.
    drive_s: Smoothed,
    sag_s: Smoothed,
    presence_s: Smoothed,
    depth_s: Smoothed,
    master_s: Smoothed,
    // Per-chunk trajectory scratch, shared by both channels.
    drive_traj: Vec<f32>,
    presence_traj: Vec<f32>,
    depth_traj: Vec<f32>,
    master_traj: Vec<f32>,
    supply: Vec<f32>,
    supply4: Vec<f32>,
    // Base-rate filter coefficients.
    c_presence: f32,
    c_ot: f32,
    c_depth: f32,
    sag_atk: f32,
    sag_rel: f32,
}

impl Default for PowerAmp {
    fn default() -> Self {
        Self::new()
    }
}

impl PowerAmp {
    pub fn new() -> Self {
        Self {
            os: [Oversampler4x::new(), Oversampler4x::new()],
            presence_lp: [OnePole::default(), OnePole::default()],
            ot_hp: [OnePole::default(), OnePole::default()],
            depth_lp: [OnePole::default(), OnePole::default()],
            env: 0.0,
            drive_s: Smoothed::new(4.0),
            sag_s: Smoothed::new(4.0),
            presence_s: Smoothed::new(3.0),
            depth_s: Smoothed::new(3.0),
            master_s: Smoothed::new(6.0),
            drive_traj: vec![0.0; 4 * CHUNK],
            presence_traj: vec![0.0; CHUNK],
            depth_traj: vec![0.0; CHUNK],
            master_traj: vec![0.0; CHUNK],
            supply: vec![0.0; CHUNK],
            supply4: vec![0.0; 4 * CHUNK],
            c_presence: 0.0,
            c_ot: 0.0,
            c_depth: 0.0,
            sag_atk: 0.0,
            sag_rel: 0.0,
        }
    }
}

impl Effect for PowerAmp {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        let base = sample_rate as f32;
        self.drive_s.configure(20.0, sample_rate * 4);
        self.sag_s.configure(30.0, sample_rate);
        self.presence_s.configure(30.0, sample_rate);
        self.depth_s.configure(30.0, sample_rate);
        self.master_s.configure(20.0, sample_rate);
        self.drive_s.snap_to_target();
        self.sag_s.snap_to_target();
        self.presence_s.snap_to_target();
        self.depth_s.snap_to_target();
        self.master_s.snap_to_target();
        self.c_presence = onepole_hz(PRESENCE_HZ, base);
        self.c_ot = onepole_hz(OT_HP_HZ, base);
        self.c_depth = onepole_hz(DEPTH_HZ, base);
        self.sag_atk = onepole_ms(SAG_ATTACK_MS, sample_rate);
        self.sag_rel = onepole_ms(SAG_RELEASE_MS, sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        for os in &mut self.os {
            os.reset();
        }
        for f in &mut self.presence_lp {
            f.reset();
        }
        for f in &mut self.ot_hp {
            f.reset();
        }
        for f in &mut self.depth_lp {
            f.reset();
        }
        self.env = 0.0;
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let Some(param) = PARAMS.get(index) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match index {
            0 => self.drive_s.set_target(real),
            1 => self.sag_s.set_target(real),
            2 => self.presence_s.set_target(real),
            3 => self.depth_s.set_target(real),
            4 => self.master_s.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let [os_l, os_r] = &mut self.os;
        let [pres_l, pres_r] = &mut self.presence_lp;
        let [ot_l, ot_r] = &mut self.ot_hp;
        let [dep_l, dep_r] = &mut self.depth_lp;
        let tanh_bias = BIAS.tanh();
        let (c_presence, c_ot, c_depth) = (self.c_presence, self.c_ot, self.c_depth);

        for (bl, br) in left.chunks_mut(CHUNK).zip(right.chunks_mut(CHUNK)) {
            let n = bl.len();
            // Pre-pass over the pristine input: the linked sag supply and the
            // base-rate knob trajectories, both ticked exactly once so the two
            // channels stay in lock-step.
            for i in 0..n {
                let level = bl[i].abs().max(br[i].abs());
                let coeff = if level > self.env {
                    self.sag_atk
                } else {
                    self.sag_rel
                };
                self.env += coeff * (level - self.env);
                if self.env < 1e-20 {
                    self.env = 0.0;
                }
                let sag_depth = SAG_MAX * self.sag_s.tick() * 0.1;
                let droop = sag_depth * (self.env * INV_SAG_REF).min(1.0);
                self.supply[i] = (1.0 - droop).clamp(SUPPLY_MIN, 1.0);
                self.presence_traj[i] = self.presence_s.tick();
                self.depth_traj[i] = self.depth_s.tick();
                self.master_traj[i] = self.master_s.tick();
            }
            for v in &mut self.drive_traj[..4 * n] {
                *v = self.drive_s.tick();
            }
            for i in 0..n {
                let s = self.supply[i];
                self.supply4[4 * i] = s;
                self.supply4[4 * i + 1] = s;
                self.supply4[4 * i + 2] = s;
                self.supply4[4 * i + 3] = s;
            }

            let drive_traj = &self.drive_traj[..4 * n];
            let supply4 = &self.supply4[..4 * n];
            let presence_traj = &self.presence_traj[..n];
            let depth_traj = &self.depth_traj[..n];
            let master_traj = &self.master_traj[..n];

            for (block, presence_lp, ot_hp, depth_lp, os) in [
                (&mut *bl, &mut *pres_l, &mut *ot_l, &mut *dep_l, &mut *os_l),
                (&mut *br, &mut *pres_r, &mut *ot_r, &mut *dep_r, &mut *os_r),
            ] {
                // Presence: high-shelf pre-emphasis into the clipper.
                let mut gp = Ramp::over(presence_traj, |p| shelf_add(PRESENCE_MAX_DB, p));
                for s in block.iter_mut() {
                    let x = *s;
                    let hp = x - presence_lp.lp(x, c_presence);
                    *s = x + gp.tick() * hp;
                }
                // Push-pull waveshaper at 4× with the sagging supply. Dropping
                // the supply raises the effective gain (clips earlier) and
                // lowers the ceiling (compresses) — the dynamic "give".
                os.process(block, |buf| {
                    let mut g = Ramp::over(drive_traj, drive_gain);
                    for (s, &sup) in buf.iter_mut().zip(supply4) {
                        let u = g.tick() * *s / sup + BIAS;
                        *s = MAKEUP * sup * (u.tanh() - tanh_bias);
                    }
                });
                // Output transformer (low-cut + core saturation), depth
                // resonance, master.
                let mut gd = Ramp::over(depth_traj, |p| shelf_add(DEPTH_MAX_DB, p));
                for (s, &m) in block.iter_mut().zip(master_traj) {
                    let x = *s;
                    let hp = x - ot_hp.lp(x, c_ot);
                    let ot = (hp * OT_DRIVE).tanh() / OT_DRIVE;
                    let lo = depth_lp.lp(ot, c_depth);
                    *s = (ot + gd.tick() * lo) * level_lin(m);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_in_blocks, rms, sine};

    const SR: u32 = 48_000;

    fn prepared() -> PowerAmp {
        let mut p = PowerAmp::new();
        p.prepare(SR);
        p
    }

    /// Set a knob by position `0..=10` at param `index`.
    fn set_pos(p: &mut PowerAmp, index: usize, pos: f32) {
        p.set_param(index, pos / 10.0);
    }

    /// A sine at nominal guitar level (−18 dBFS).
    fn guitar(freq: f32, len: usize) -> Vec<f32> {
        let mut x = sine(SR, freq, len);
        for s in &mut x {
            *s *= 0.126;
        }
        x
    }

    /// A quiet multi-tone probe: −30 dBFS sines at `freqs`.
    fn tones(freqs: &[f32], len: usize) -> Vec<f32> {
        let mut x = vec![0.0f32; len];
        for (i, s) in x.iter_mut().enumerate() {
            let t = i as f32 / SR as f32;
            for f in freqs {
                *s += 0.03 * (std::f32::consts::TAU * f * t).sin();
            }
        }
        x
    }

    /// Magnitude of the projection onto `freq` over the settled tail.
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

    /// RMS fraction of the output that is *not* the fundamental.
    fn harmonic_residual(y: &[f32], f0: f32) -> f64 {
        let tail = &y[y.len() / 2..];
        let fund_rms = tone_at(y, f0) / 2f64.sqrt();
        let total_rms = f64::from(rms(tail));
        (total_rms.powi(2) - fund_rms.powi(2)).max(0.0).sqrt() / total_rms
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.pedals.len(), 1);
        assert!(std::ptr::eq(FAMILY.pedals[0], &DESC));
        let captions: Vec<&str> = DESC.params.iter().map(|p| p.name).collect();
        assert_eq!(captions, ["Drive", "Sag", "Presence", "Depth", "Master"]);
        // Ships bypassed on the default board — a full-amp capture already has
        // a power stage (ADR 024).
        assert!(!lh_core::default_active("power"));
    }

    #[test]
    fn drive_adds_harmonics() {
        let x = guitar(220.0, SR as usize);
        let mut low = prepared();
        set_pos(&mut low, 0, 1.0);
        let low_res = harmonic_residual(&process_in_blocks(&mut low, &x, 256), 220.0);
        let mut high = prepared();
        set_pos(&mut high, 0, 8.0);
        let high_res = harmonic_residual(&process_in_blocks(&mut high, &x, 256), 220.0);
        assert!(
            high_res > 2.0 * low_res.max(0.01),
            "cranked power amp must saturate: high {high_res:.3} vs low {low_res:.3}"
        );
        assert!(high_res > 0.05, "expected audible drive, got {high_res:.3}");
    }

    #[test]
    fn asymmetric_bias_makes_even_harmonics() {
        // The class-AB bias skews the transfer curve — a strong 2nd harmonic.
        let x = guitar(220.0, SR as usize);
        let mut p = prepared();
        set_pos(&mut p, 0, 7.0);
        let y = process_in_blocks(&mut p, &x, 256);
        let h2 = tone_at(&y, 440.0) / tone_at(&y, 220.0);
        assert!(h2 > 0.03, "push-pull even harmonics: h2/f0 = {h2:.4}");
    }

    #[test]
    fn sag_compresses_loud_more_than_quiet() {
        // Sag's fingerprint: a loud note is compressed *more* than a quiet one
        // because the supply droops under it. Isolate sag by comparing the
        // loud/quiet gain ratio with sag full vs sag off — clipping alone
        // compresses both, sag adds to the loud one.
        let gain = |sag: f32, amp: f32| -> f64 {
            let mut p = prepared();
            set_pos(&mut p, 0, 6.0);
            set_pos(&mut p, 1, sag);
            set_pos(&mut p, 2, 0.0);
            set_pos(&mut p, 3, 0.0);
            let x: Vec<f32> = sine(SR, 220.0, SR as usize)
                .iter()
                .map(|s| s * amp)
                .collect();
            let y = process_in_blocks(&mut p, &x, 256);
            f64::from(rms(&y[y.len() / 2..])) / f64::from(rms(&x[x.len() / 2..]))
        };
        let with_sag = gain(10.0, 0.5) / gain(10.0, 0.02);
        let without_sag = gain(0.0, 0.5) / gain(0.0, 0.02);
        assert!(
            with_sag < 0.9 * without_sag,
            "sag must compress the loud note further: with {with_sag:.3} vs without {without_sag:.3}"
        );
    }

    #[test]
    fn presence_and_depth_shelves_shape_the_spectrum() {
        // At low drive the stage is ~linear, so the shelves read cleanly.
        let x = tones(&[60.0, 90.0, 500.0, 3_000.0, 6_100.0], SR as usize);
        let measure = |presence: f32, depth: f32, freq: f32| -> f64 {
            let mut p = prepared();
            set_pos(&mut p, 0, 1.0);
            set_pos(&mut p, 2, presence);
            set_pos(&mut p, 3, depth);
            tone_at(&process_in_blocks(&mut p, &x, 256), freq)
        };
        // Presence lifts the top (6.1 kHz) relative to the mids (500 Hz).
        let flat_hi = measure(0.0, 0.0, 6_100.0) / measure(0.0, 0.0, 500.0);
        let up_hi = measure(10.0, 0.0, 6_100.0) / measure(10.0, 0.0, 500.0);
        assert!(
            up_hi > 1.3 * flat_hi,
            "presence: {up_hi:.3} vs flat {flat_hi:.3}"
        );
        // Depth lifts the lows (60 Hz) relative to the mids.
        let flat_lo = measure(0.0, 0.0, 60.0) / measure(0.0, 0.0, 500.0);
        let up_lo = measure(0.0, 10.0, 60.0) / measure(0.0, 10.0, 500.0);
        assert!(
            up_lo > 1.3 * flat_lo,
            "depth: {up_lo:.3} vs flat {flat_lo:.3}"
        );
    }

    #[test]
    fn oversampling_suppresses_aliasing() {
        // Drive an 8 kHz tone hard: 1 kHz and 3 kHz are neither its harmonics
        // nor foldover targets, so aliasing (if any) shows up there. With 4×
        // oversampling they stay near the floor.
        let x = guitar(8_000.0, SR as usize);
        let mut p = prepared();
        set_pos(&mut p, 0, 8.0);
        set_pos(&mut p, 2, 0.0);
        set_pos(&mut p, 3, 0.0);
        let y = process_in_blocks(&mut p, &x, 256);
        let fund = tone_at(&y, 8_000.0);
        let alias = tone_at(&y, 1_000.0).max(tone_at(&y, 3_000.0));
        assert!(
            alias < 0.03 * fund,
            "aliasing floor too high: alias {alias:.5} vs fundamental {fund:.5}"
        );
    }

    #[test]
    fn near_unity_at_defaults() {
        // Switching the stage in at its defaults must not jump the monitors.
        let x = guitar(220.0, SR as usize);
        let mut p = prepared();
        let y = process_in_blocks(&mut p, &x, 256);
        let db =
            20.0 * (f64::from(rms(&y[y.len() / 2..])) / f64::from(rms(&x[x.len() / 2..]))).log10();
        assert!(
            db.abs() < 6.0,
            "defaults should sit near unity, got {db:.1} dB"
        );
    }

    #[test]
    fn blocks_dc_and_dies_to_silence() {
        let mut p = prepared();
        set_pos(&mut p, 0, 9.0);
        let x = guitar(220.0, SR as usize);
        let y = process_in_blocks(&mut p, &x, 256);
        let tail = &y[SR as usize / 2..];
        let mean = tail.iter().map(|s| f64::from(*s)).sum::<f64>() / tail.len() as f64;
        assert!(mean.abs() < 2e-3, "DC must be blocked, mean {mean}");

        p.reset();
        let silence = vec![0.0f32; SR as usize / 4];
        let y = process_in_blocks(&mut p, &silence, 128);
        assert!(rms(&y[y.len() / 2..]) < 1e-4, "silence in → silence out");
    }

    #[test]
    fn bounded_and_finite_when_slammed() {
        let mut p = prepared();
        for i in 0..5 {
            set_pos(&mut p, i, 10.0);
        }
        let x = sine(SR, 220.0, SR as usize / 2); // full-scale input
        let y = process_in_blocks(&mut p, &x, 64);
        assert_finite("power slammed", &y);
        let pk = peak(&y);
        assert!(pk < 6.0, "bounded output, got peak {pk}");
        assert!(pk > 0.2, "signal present, got peak {pk}");
    }

    #[test]
    fn runs_at_studio_rates() {
        for sr in [44_100u32, 96_000] {
            for block in [32usize, 1024] {
                let mut p = PowerAmp::new();
                p.prepare(sr);
                p.set_param(0, 0.7);
                let x = sine(sr, 220.0, sr as usize / 2);
                let y = process_in_blocks(&mut p, &x, block);
                assert_finite("power studio rate", &y);
                assert!(rms(&y[y.len() / 2..]) > 1e-3);
            }
        }
    }

    #[test]
    fn param_changes_are_smooth() {
        let mut p = prepared();
        set_pos(&mut p, 0, 0.0);
        let x = sine(SR, 220.0, SR as usize / 2);
        let mut y = x.clone();
        let mut yr = x.clone();
        let (a, b) = y.split_at_mut(SR as usize / 4);
        let (ar, br) = yr.split_at_mut(SR as usize / 4);
        p.process(a, ar);
        p.set_param(0, 1.0); // slam drive
        p.set_param(4, 0.0); // and master
        p.process(b, br);
        assert_finite("power sweep", &y);
        let max_step = y
            .windows(2)
            .map(|w| (w[1] - w[0]).abs())
            .fold(0.0f32, f32::max);
        assert!(max_step < 0.5, "click detected, step {max_step}");
    }
}

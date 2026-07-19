//! The filter family: swept filters behind one chain slot (PRD 007/008).
//! The family key is deliberately broader than any one pedal ("filter",
//! not "wah"): LFO wah, sample & hold, and formant filters have an obvious
//! home here later, and slots address families.
//!
//! Two pedals, one engine (the delay-family pattern — a `Ctl` routing
//! table per faceplate, no per-sample vtable):
//!
//! - [`autowah`] — envelope-driven: how hard you pick is where the filter
//!   sits.
//! - [`wah`] — position-driven: a smoothed `pos` param is the treadle,
//!   built to be the landing zone of an expression pedal (PRD 008).
//!
//! Both sweep geometrically over their own corner range (the ear hears
//! pitch in octaves, so a linear map would rush the lows and crawl in the
//! highs) into the same filter: a Chamberlin state-variable filter — one
//! structure yields lowpass/bandpass/highpass simultaneously (the `mode`
//! switch is free) and retuning per sample costs one `sin`, not a full
//! biquad cookbook pass. The wah ranges top out far below the SVF's
//! stability region (fc ≪ sr/6 at every supported rate), and the band
//! state is soft-clipped every sample — at `q` 12 the resonance saturates
//! like an overdriven analog filter instead of running away (RT rule 7).
//!
//! Stereo: the sweep is one event shared by both channels (the vibrato
//! principle — the envelope reads the mono sum, the treadle is one
//! treadle); per-channel SVF state keeps the audio path stereo-clean.

mod autowah;
mod wah;

use lh_core::{EffectDesc, FamilyDesc};

use crate::Effect;
use crate::blocks::onepole_ms;
use crate::blocks::smooth::Smoothed;

pub const MODES: &[&str] = &["lowpass", "bandpass", "highpass"];
pub const DIRECTIONS: &[&str] = &["up", "down"];

const ATTACK_MS: f32 = 2.0;
/// `sens` at 1.0 adds +30 dB into the follower — weak picking can still
/// reach the top of the sweep.
const SENS_MAX_DB: f32 = 30.0;
/// Band-state soft clip drive: unity small-signal, bounded resonance.
const BAND_DRIVE: f32 = 0.7;

/// The filter family, in menu order. Append-only (PRD 001).
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "filter",
    name: "Filter",
    pedals: &[&autowah::DESC, &wah::DESC],
};

pub const PEDAL_COUNT: usize = 2;

/// Which engine control a pedal's param position drives.
#[derive(Clone, Copy)]
enum Ctl {
    Sens,
    Q,
    Decay,
    Mode,
    Direction,
    Mix,
    /// The wah treadle — the expression pedal's landing zone (PRD 008).
    Pos,
}

/// One pedal's faceplate, param→control routing (same length as the
/// faceplate), and voicing constants.
pub struct PedalDef {
    pub desc: &'static EffectDesc,
    controls: &'static [Ctl],
    /// Sweep endpoints of the geometric fc map.
    fc_min_hz: f32,
    fc_max_hz: f32,
    /// Envelope-driven (autowah) vs. position-driven (wah).
    follower: bool,
}

/// The pedal registry, aligned with [`FAMILY`]`.pedals`.
pub static PEDALS: [PedalDef; PEDAL_COUNT] = [autowah::PEDAL, wah::PEDAL];

/// One channel's Chamberlin SVF state.
#[derive(Default, Clone, Copy)]
struct Svf {
    low: f32,
    band: f32,
}

impl Svf {
    /// One sample: retune to `f`, damp by `1/q`, return (low, band, high).
    #[inline]
    fn step(&mut self, x: f32, f: f32, damp: f32) -> (f32, f32, f32) {
        self.low += f * self.band;
        let high = x - self.low - damp * self.band;
        self.band += f * high;
        // Analog filters self-limit; ours does too (bounded at any Q).
        self.band = (self.band * BAND_DRIVE).tanh() / BAND_DRIVE;
        if self.low.abs() < 1e-20 {
            self.low = 0.0;
        }
        if self.band.abs() < 1e-20 {
            self.band = 0.0;
        }
        (self.low, self.band, high)
    }

    fn clear(&mut self) {
        *self = Self::default();
    }
}

pub struct Filter {
    sample_rate: f32,
    pedal: usize,
    // Shared smoothers; each pedal routes its faceplate onto them through
    // its `Ctl` table, so a control means the same thing on every face.
    sens: Smoothed,
    q: Smoothed,
    decay_ms: Smoothed,
    mix: Smoothed,
    pos: Smoothed,
    mode: usize,
    direction: usize,
    env: f32,
    attack_coeff: f32,
    release_coeff: f32,
    sens_gain: f32,
    damp: f32,
    svf: [Svf; 2],
}

impl Default for Filter {
    fn default() -> Self {
        Self::new()
    }
}

impl Filter {
    pub fn new() -> Self {
        let aw = autowah::DESC.params;
        Self {
            sample_rate: 48_000.0,
            pedal: 0,
            sens: Smoothed::new(aw[0].default),
            q: Smoothed::new(aw[1].default),
            decay_ms: Smoothed::new(aw[2].default),
            mix: Smoothed::new(aw[5].default),
            pos: Smoothed::new(wah::DESC.params[0].default),
            mode: 0,
            direction: 0,
            env: 0.0,
            attack_coeff: 1.0,
            release_coeff: 0.1,
            sens_gain: 1.0,
            damp: 0.25,
            svf: [Svf::default(); 2],
        }
    }
}

impl Effect for Filter {
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
        // Fresh filter state for the incoming pedal; the control side
        // re-sends its knob values from the shadow (PRD 001).
        self.reset();
    }

    fn descriptor(&self) -> &'static EffectDesc {
        PEDALS[self.pedal].desc
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        for (smoothed, ms) in [
            (&mut self.sens, 50.0),
            (&mut self.q, 60.0),
            (&mut self.decay_ms, 60.0),
            (&mut self.mix, 30.0),
            (&mut self.pos, 25.0),
        ] {
            smoothed.configure(ms, sample_rate);
            smoothed.snap_to_target();
        }
        self.attack_coeff = onepole_ms(ATTACK_MS, sample_rate);
        self.reset();
    }

    fn reset(&mut self) {
        self.env = 0.0;
        for svf in &mut self.svf {
            svf.clear();
        }
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let def = &PEDALS[self.pedal];
        // Out-of-range indices are ignored (Effect contract).
        let (Some(ctl), Some(param)) = (def.controls.get(index), def.desc.params.get(index)) else {
            return;
        };
        let real = param.range.to_real(normalized);
        match ctl {
            Ctl::Sens => self.sens.set_target(real),
            Ctl::Q => self.q.set_target(real),
            Ctl::Decay => self.decay_ms.set_target(real),
            Ctl::Mode => self.mode = real as usize,
            Ctl::Direction => self.direction = real as usize,
            Ctl::Mix => self.mix.set_target(real),
            Ctl::Pos => self.pos.set_target(real),
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        let sr = self.sample_rate;
        let def = &PEDALS[self.pedal];
        // Block-rate coefficients off the smoothed knob values (three exps
        // per block is noise; the per-sample costs are the sweep's own
        // exp + sin).
        self.sens_gain = 10f32.powf(SENS_MAX_DB / 20.0 * self.sens.current());
        self.damp = 1.0 / self.q.current().max(0.5);
        self.release_coeff = onepole_ms(self.decay_ms.current(), sr as u32);
        let span_ln = (def.fc_max_hz / def.fc_min_hz).ln();

        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.sens.tick();
            self.q.tick();
            self.decay_ms.tick();
            let mix = self.mix.tick();
            let pos = self.pos.tick();

            let sweep = if def.follower {
                // Envelope: fast attack, knob-set release, mono-summed
                // source; `direction` down flips the sweep.
                let mag = 0.5 * (*l + *r).abs() * self.sens_gain;
                let coeff = if mag > self.env {
                    self.attack_coeff
                } else {
                    self.release_coeff
                };
                self.env += coeff * (mag - self.env);
                if self.env < 1e-20 {
                    self.env = 0.0;
                }
                let e = self.env.min(1.0);
                if self.direction == 1 { 1.0 - e } else { e }
            } else {
                // The treadle: its smoother is the anti-staircase layer
                // between a 7-bit CC and the ear.
                pos.clamp(0.0, 1.0)
            };
            let fc = def.fc_min_hz * (span_ln * sweep).exp();
            let f = 2.0 * (std::f32::consts::PI * fc / sr).sin();

            let (dry_l, dry_r) = (*l, *r);
            let mode = self.mode.min(2);
            let pick = |outs: (f32, f32, f32)| match mode {
                0 => outs.0,
                1 => outs.1,
                _ => outs.2,
            };
            let wet_l = pick(self.svf[0].step(dry_l, f, self.damp));
            let wet_r = pick(self.svf[1].step(dry_r, f, self.damp));
            *l = dry_l + mix * (wet_l - dry_l);
            *r = dry_r + mix * (wet_r - dry_r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, peak, process_stereo_in_blocks, rms, silence, sine};

    const SR: u32 = 48_000;

    fn prepared(pedal: usize) -> Filter {
        let mut w = Filter::new();
        w.prepare(SR);
        w.select_pedal(pedal);
        let desc = PEDALS[pedal].desc;
        for (i, p) in desc.params.iter().enumerate() {
            w.set_param(i, p.default_norm());
        }
        w
    }

    fn set_by(w: &mut Filter, key: &str, real: f32) {
        let desc = PEDALS[w.pedal].desc;
        let i = desc.param_index(key).unwrap();
        w.set_param(i, desc.params[i].range.to_norm(real));
    }

    /// A 900 Hz tone: loud (0.7) for 1 s, then −26 dB (0.035) for 1.5 s —
    /// long enough for the release to actually land before we measure.
    fn two_level_probe() -> Vec<f32> {
        let mut x = sine(SR, 900.0, SR as usize * 5 / 2);
        for s in &mut x[SR as usize..] {
            *s *= 0.05;
        }
        for s in &mut x[..SR as usize] {
            *s *= 0.7;
        }
        x
    }

    /// Wet gain at 900 Hz (output rms over input rms) for each level step.
    fn tracked_gains(direction: f32) -> (f32, f32) {
        let mut w = prepared(0);
        set_by(&mut w, "direction", direction);
        set_by(&mut w, "sens", 0.5);
        set_by(&mut w, "q", 2.0); // soften the resonant shelf for the gain read
        set_by(&mut w, "decay", 100.0); // settle well inside the quiet window
        let x = two_level_probe();
        let (l, _) = process_stereo_in_blocks(&mut w, &x, 64);
        // Compare steady sections well inside each level (skip transitions;
        // the quiet read takes the final half second, after the release).
        let loud_in = rms(&x[SR as usize / 4..SR as usize * 3 / 4]);
        let loud_out = rms(&l[SR as usize / 4..SR as usize * 3 / 4]);
        let quiet_in = rms(&x[SR as usize * 2..]);
        let quiet_out = rms(&l[SR as usize * 2..]);
        (loud_out / loud_in, quiet_out / quiet_in)
    }

    #[test]
    fn registry_is_consistent() {
        assert_eq!(FAMILY.key, "filter");
        assert_eq!(FAMILY.pedals.len(), PEDAL_COUNT);
        assert_eq!(FAMILY.pedals[0].key, "autowah");
        assert_eq!(FAMILY.pedals[1].key, "wah");
        for (pedal, def) in FAMILY.pedals.iter().zip(PEDALS.iter()) {
            assert!(std::ptr::eq(*pedal, def.desc), "registry order matches");
            assert_eq!(
                def.desc.params.len(),
                def.controls.len(),
                "{}: one control per param",
                def.desc.key
            );
        }
        let captions: Vec<&str> = autowah::DESC.params.iter().map(|p| p.name).collect();
        assert_eq!(captions, ["Sens", "Q", "Decay", "Mode", "Direction", "Mix"]);
        let captions: Vec<&str> = wah::DESC.params.iter().map(|p| p.name).collect();
        assert_eq!(captions, ["Pos", "Q", "Mode", "Mix"]);
        // The family ships bypassed on the default board: a filter has no
        // transparent knob position (lh-core owns that flag).
        assert!(!lh_core::default_active(FAMILY.key));
        assert!(lh_core::default_active("gate"));
    }

    #[test]
    fn envelope_opens_the_filter_with_picking_strength() {
        // Lowpass, up: loud playing sweeps fc above 900 Hz (tone passes),
        // quiet playing parks fc near the floor (tone is filtered out).
        let (loud, quiet) = tracked_gains(0.0);
        assert!(
            loud > 2.0 * quiet,
            "loud must open the filter: {loud:.3} vs quiet {quiet:.3}"
        );
    }

    #[test]
    fn direction_down_reverses_the_sweep() {
        let (loud, quiet) = tracked_gains(1.0);
        assert!(
            quiet > 2.0 * loud,
            "down: quiet must sit open, loud must duck: {loud:.3} vs {quiet:.3}"
        );
    }

    #[test]
    fn modes_are_three_different_filters() {
        for pedal in 0..PEDAL_COUNT {
            let render = |mode: f32| {
                let mut w = prepared(pedal);
                set_by(&mut w, "mode", mode);
                let x = sine(SR, 500.0, SR as usize / 2);
                process_stereo_in_blocks(&mut w, &x, 64).0
            };
            let lp = render(0.0);
            let bp = render(1.0);
            let hp = render(2.0);
            for (a, b, label) in [
                (&lp, &bp, "lp/bp"),
                (&bp, &hp, "bp/hp"),
                (&lp, &hp, "lp/hp"),
            ] {
                let diff = a
                    .iter()
                    .zip(b.iter())
                    .map(|(x, y)| (x - y).abs())
                    .fold(0.0f32, f32::max);
                assert!(diff > 1e-3, "pedal {pedal} {label} must differ ({diff})");
            }
        }
    }

    #[test]
    fn high_q_boosts_the_resonance() {
        // The Chamberlin bandpass is the constant-skirt kind: Q shows up at
        // the peak (gain ≈ Q), not on the skirts. Pin the sweep at the top
        // (max sens: even this quiet probe clamps the envelope, parking fc
        // at 2.4 kHz — an LTI measurement) and probe AT resonance, quietly
        // enough that the band soft-clip stays essentially linear.
        let resonance_gain = |q: f32| {
            let mut w = prepared(0);
            set_by(&mut w, "q", q);
            set_by(&mut w, "mode", 1.0); // bandpass
            set_by(&mut w, "sens", 1.0);
            let x: Vec<f32> = sine(SR, 2_400.0, SR as usize)
                .iter()
                .map(|s| s * 0.05)
                .collect();
            let (l, _) = process_stereo_in_blocks(&mut w, &x, 64);
            rms(&l[SR as usize / 2..]) / rms(&x[SR as usize / 2..])
        };
        let soft = resonance_gain(1.5);
        let sharp = resonance_gain(12.0);
        assert!(
            sharp > 2.5 * soft,
            "Q 12 must resonate far harder at fc: {sharp:.3} vs {soft:.3}"
        );
    }

    /// The treadle is the sweep: parking `pos` so fc sits on the probe tone
    /// must pass far more of it than parking the peak an octave-plus away
    /// (constant-skirt BP: the peak is where the gain is).
    #[test]
    fn wah_position_moves_the_resonant_peak() {
        let pos_for = |fc: f32| (fc / 350.0).ln() / (2_200.0f32 / 350.0).ln();
        let gain_at = |pos: f32, probe_hz: f32| {
            let mut w = prepared(1);
            set_by(&mut w, "mode", 1.0); // bandpass — read the peak itself
            set_by(&mut w, "pos", pos);
            let x: Vec<f32> = sine(SR, probe_hz, SR as usize)
                .iter()
                .map(|s| s * 0.05)
                .collect();
            let (l, _) = process_stereo_in_blocks(&mut w, &x, 64);
            rms(&l[SR as usize / 2..]) / rms(&x[SR as usize / 2..])
        };
        let heel = gain_at(pos_for(500.0), 500.0);
        let toe = gain_at(pos_for(2_000.0), 500.0);
        assert!(
            heel > 2.5 * toe,
            "peak on the tone must beat peak two octaves up: {heel:.3} vs {toe:.3}"
        );
        let toe_hi = gain_at(pos_for(2_000.0), 2_000.0);
        let heel_hi = gain_at(pos_for(500.0), 2_000.0);
        assert!(
            toe_hi > 2.5 * heel_hi,
            "and the reverse at the toe: {toe_hi:.3} vs {heel_hi:.3}"
        );
    }

    /// A hard `pos` jump (a 7-bit CC can step the whole range in one
    /// message) must glide through the 25 ms smoother, not click. Compare
    /// the worst sample-to-sample delta after the jump against the steady
    /// tone's own — a click would be an order of magnitude out.
    #[test]
    fn wah_sweep_is_declicked() {
        let mut w = prepared(1);
        set_by(&mut w, "q", 2.0);
        set_by(&mut w, "pos", 0.0);
        let x = sine(SR, 500.0, SR as usize);
        let half = x.len() / 2;
        let mut l = x.clone();
        let mut r = x.clone();
        let (a, b) = l.split_at_mut(half);
        let (ar, br) = r.split_at_mut(half);
        w.process(a, ar);
        set_by(&mut w, "pos", 1.0); // the jump
        w.process(b, br);
        let max_delta = |s: &[f32]| {
            s.windows(2)
                .map(|w| (w[1] - w[0]).abs())
                .fold(0.0f32, f32::max)
        };
        let steady = max_delta(&l[half / 2..half]);
        let jump = max_delta(&l[half..]);
        assert!(
            jump < 3.0 * steady.max(0.05),
            "pos jump must glide: delta {jump:.3} vs steady {steady:.3}"
        );
    }

    /// Switching pedals mid-stream is clean: state resets, values re-sent
    /// by the control side land on the incoming face, output stays sane.
    #[test]
    fn pedal_switch_is_bounded_and_routes_params() {
        let mut w = prepared(0);
        let x = sine(SR, 330.0, SR as usize);
        let mut l = x.clone();
        let mut r = x.clone();
        let third = x.len() / 3;
        let (a, rest) = l.split_at_mut(third);
        let (b, c) = rest.split_at_mut(third);
        let (ar, restr) = r.split_at_mut(third);
        let (br, cr) = restr.split_at_mut(third);
        w.process(a, ar);
        w.select_pedal(1);
        assert_eq!(w.descriptor().key, "wah");
        // Param index 0 is `sens` on the autowah but `pos` on the wah —
        // the Ctl table must route by the active face.
        w.set_param(0, 1.0);
        w.process(b, br);
        w.select_pedal(0);
        assert_eq!(w.descriptor().key, "autowah");
        w.process(c, cr);
        assert_finite("filter pedal switch L", &l);
        assert_finite("filter pedal switch R", &r);
        assert!(peak(&l) < 8.0);
    }

    #[test]
    fn bounded_at_max_everything() {
        for pedal in 0..PEDAL_COUNT {
            let mut w = prepared(pedal);
            set_by(&mut w, "q", 12.0);
            match pedal {
                0 => set_by(&mut w, "sens", 1.0),
                _ => set_by(&mut w, "pos", 1.0),
            }
            let x: Vec<f32> = sine(SR, 220.0, SR as usize * 2)
                .iter()
                .map(|s| s * 0.95)
                .collect();
            let (l, r) = process_stereo_in_blocks(&mut w, &x, 64);
            assert_finite("filter max L", &l);
            assert_finite("filter max R", &r);
            assert!(
                peak(&l) < 8.0,
                "pedal {pedal} resonance must stay bounded, peak {}",
                peak(&l)
            );
            assert!(peak(&r) < 8.0);
        }
    }

    #[test]
    fn mix_zero_is_bit_exact_dry() {
        for pedal in 0..PEDAL_COUNT {
            let mut w = prepared(pedal);
            set_by(&mut w, "mix", 0.0);
            let warm = sine(SR, 220.0, SR as usize);
            let _ = process_stereo_in_blocks(&mut w, &warm, 512);
            let x = sine(SR, 220.0, 8_192);
            let (l, r) = process_stereo_in_blocks(&mut w, &x, 512);
            assert_eq!(x, l, "pedal {pedal}: mix 0 must pass dry (L)");
            assert_eq!(x, r, "pedal {pedal}: mix 0 must pass dry (R)");
        }
    }

    #[test]
    fn every_knob_sweep_stays_finite() {
        for (pedal, def) in PEDALS.iter().enumerate() {
            for (i, param) in def.desc.params.iter().enumerate() {
                let mut w = prepared(pedal);
                let mut x = sine(SR, 330.0, SR as usize);
                let mut xr = x.clone();
                let third = x.len() / 3;
                let (a, rest) = x.split_at_mut(third);
                let (b, c) = rest.split_at_mut(third);
                let (ar, restr) = xr.split_at_mut(third);
                let (br, cr) = restr.split_at_mut(third);
                w.process(a, ar);
                w.set_param(i, 0.0);
                w.process(b, br);
                w.set_param(i, 1.0);
                w.process(c, cr);
                assert_finite(&format!("filter {pedal} sweep {}", param.key), &x);
                assert!(
                    peak(&x) < 16.0,
                    "pedal {pedal}: sweeping {} must stay bounded",
                    param.key
                );
            }
        }
    }

    #[test]
    fn silence_in_silence_out() {
        for pedal in 0..PEDAL_COUNT {
            let mut w = prepared(pedal);
            let x = silence(8_192);
            let (l, r) = process_stereo_in_blocks(&mut w, &x, 512);
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
                let mut w = Filter::new();
                w.prepare(sr);
                w.select_pedal(pedal);
                for chunk in [32usize, 483, 1_024] {
                    let x = sine(sr, 440.0, 4_096);
                    let (l, r) = process_stereo_in_blocks(&mut w, &x, chunk);
                    assert_finite("filter multirate L", &l);
                    assert_finite("filter multirate R", &r);
                }
            }
        }
    }
}

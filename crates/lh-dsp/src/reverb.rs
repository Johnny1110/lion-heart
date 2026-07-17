//! Reverb: an 8-line feedback delay network (FDN).
//!
//! Topology: predelay → two series diffusion allpasses → 8 delay lines with
//! mutually incommensurate lengths, fed back through a Householder matrix
//! (`H = I − (2/N)·J`, orthogonal, O(N) to apply). Each feedback path has a
//! one-pole damping lowpass (the `tone` control) and a per-line gain derived
//! from the `decay` (T60) parameter: `g = 10^(−3·delay/t60)`, which makes
//! every path lose 60 dB in exactly `t60` seconds — the tail's decay rate is
//! uniform and unconditionally stable (|g| < 1, H orthogonal).
//!
//! Stereo (M7): the input is mono-summed into one shared FDN core; the two
//! output channels take differently-signed tap mixes of the same eight lines
//! (orthogonal ±1 Hadamard rows), which yields a decorrelated L/R pair —
//! real width — without touching the feedback structure.

use lh_core::{EffectDesc, FamilyDesc, ParamDesc, Range};

use crate::Effect;
use crate::smooth::Smoothed;

const N: usize = 8;
/// Line lengths in ms — spread, mutually incommensurate, longest < 80 ms.
const LINE_MS: [f32; N] = [29.7, 37.1, 41.9, 47.3, 53.9, 61.3, 71.9, 79.7];
const DIFFUSION_MS: [f32; 2] = [5.1, 7.9];
const DIFFUSION_G: f32 = 0.7;
/// Wet tail level: Σ of 8 unit-scale lines needs pulling down.
const WET_SCALE: f32 = 0.35;
/// Output tap signs per channel: two orthogonal Hadamard rows, so L and R
/// hear the same tail energy but decorrelated.
const OUT_L: [f32; N] = [1.0, -1.0, 1.0, -1.0, 1.0, -1.0, 1.0, -1.0];
const OUT_R: [f32; N] = [1.0, 1.0, -1.0, -1.0, 1.0, 1.0, -1.0, -1.0];

static PARAMS: [ParamDesc; 4] = [
    ParamDesc {
        key: "decay",
        name: "Decay",
        unit: "s",
        range: Range::Log { min: 0.2, max: 8.0 },
        default: 1.8,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "tone",
        name: "Tone",
        unit: "Hz",
        range: Range::Log {
            min: 1_000.0,
            max: 12_000.0,
        },
        default: 5_000.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "predelay",
        name: "Predelay",
        unit: "ms",
        range: Range::Linear {
            min: 0.0,
            max: 120.0,
        },
        default: 20.0,
        smoothing_ms: 0.0,
    },
    ParamDesc {
        key: "mix",
        name: "Mix",
        unit: "",
        range: Range::Linear { min: 0.0, max: 1.0 },
        default: 0.3,
        smoothing_ms: 30.0,
    },
];

pub static DESC: EffectDesc = EffectDesc {
    key: "reverb",
    name: "Reverb",
    params: &PARAMS,
};

/// Single-pedal family: the pedal key doubles as the family key.
pub static FAMILY: FamilyDesc = FamilyDesc {
    key: "reverb",
    name: "Reverb",
    pedals: &[&DESC],
};

/// Fixed-length circular delay.
struct Line {
    buf: Vec<f32>,
    write: usize,
}

impl Line {
    fn new(samples: usize) -> Self {
        Self {
            buf: vec![0.0; samples.max(1)],
            write: 0,
        }
    }

    #[inline]
    fn read(&self) -> f32 {
        self.buf[self.write] // one full lap behind the write head
    }

    #[inline]
    fn write(&mut self, value: f32) {
        self.buf[self.write] = value;
        self.write = (self.write + 1) % self.buf.len();
    }

    fn clear(&mut self) {
        self.buf.iter_mut().for_each(|s| *s = 0.0);
        self.write = 0;
    }
}

/// Schroeder allpass for input diffusion.
struct Allpass {
    line: Line,
    g: f32,
}

impl Allpass {
    #[inline]
    fn process(&mut self, x: f32) -> f32 {
        let delayed = self.line.read();
        let input = x + self.g * delayed;
        self.line.write(input);
        delayed - self.g * input
    }
}

pub struct Reverb {
    sample_rate: f32,
    t60: f32,
    tone_hz: f32,
    predelay_ms: f32,
    mix: Smoothed,
    predelay: Line,
    predelay_len: usize,
    diffusion: Vec<Allpass>,
    lines: Vec<Line>,
    line_gains: [f32; N],
    damp_state: [f32; N],
    damp_coeff: f32,
}

impl Default for Reverb {
    fn default() -> Self {
        Self::new()
    }
}

impl Reverb {
    pub fn new() -> Self {
        Self {
            sample_rate: 48_000.0,
            t60: PARAMS[0].default,
            tone_hz: PARAMS[1].default,
            predelay_ms: PARAMS[2].default,
            mix: Smoothed::new(PARAMS[3].default),
            predelay: Line::new(1),
            predelay_len: 1,
            diffusion: Vec::new(),
            lines: Vec::new(),
            line_gains: [0.0; N],
            damp_state: [0.0; N],
            damp_coeff: 1.0,
        }
    }

    fn recompute(&mut self) {
        for (gain, ms) in self.line_gains.iter_mut().zip(LINE_MS) {
            *gain = 10f32.powf(-3.0 * (ms * 1e-3) / self.t60);
        }
        self.damp_coeff = 1.0 - (-std::f32::consts::TAU * self.tone_hz / self.sample_rate).exp();
        let max = self.predelay.buf.len();
        self.predelay_len = ((self.predelay_ms * 1e-3 * self.sample_rate) as usize).clamp(1, max);
    }
}

impl Effect for Reverb {
    fn family(&self) -> &'static FamilyDesc {
        &FAMILY
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate as f32;
        let smp = |ms: f32| (ms * 1e-3 * self.sample_rate) as usize;
        self.predelay = Line::new(smp(PARAMS[2].range.max()) + 1);
        self.diffusion = DIFFUSION_MS
            .iter()
            .map(|&ms| Allpass {
                line: Line::new(smp(ms)),
                g: DIFFUSION_G,
            })
            .collect();
        self.lines = LINE_MS.iter().map(|&ms| Line::new(smp(ms))).collect();
        self.mix.configure(PARAMS[3].smoothing_ms, sample_rate);
        self.mix.snap_to_target();
        self.recompute();
        self.reset();
    }

    fn reset(&mut self) {
        self.predelay.clear();
        for ap in &mut self.diffusion {
            ap.line.clear();
        }
        for line in &mut self.lines {
            line.clear();
        }
        self.damp_state = [0.0; N];
    }

    fn set_param(&mut self, index: usize, normalized: f32) {
        let real = PARAMS[index].range.to_real(normalized);
        match index {
            0 => {
                self.t60 = real;
                self.recompute();
            }
            1 => {
                self.tone_hz = real;
                self.recompute();
            }
            2 => {
                self.predelay_ms = real;
                self.recompute();
            }
            3 => self.mix.set_target(real),
            _ => {}
        }
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.lines.is_empty() {
            return; // prepare() not called yet
        }
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            let (dry_l, dry_r) = (*l, *r);
            let dry = 0.5 * (dry_l + dry_r); // the FDN core is mono-fed

            // Predelay: read `predelay_len` behind the write head.
            let len = self.predelay.buf.len();
            let rp = (self.predelay.write + len - self.predelay_len) % len;
            let delayed = self.predelay.buf[rp];
            self.predelay.write(dry);

            let mut input = delayed;
            for ap in &mut self.diffusion {
                input = ap.process(input);
            }

            // Read tails, damp, apply decay gain; Householder feedback.
            let mut v = [0.0f32; N];
            let mut sum = 0.0;
            let mut wet_l = 0.0;
            let mut wet_r = 0.0;
            for (i, (((line, damp), gain), fed_back)) in self
                .lines
                .iter()
                .zip(&mut self.damp_state)
                .zip(&self.line_gains)
                .zip(&mut v)
                .enumerate()
            {
                let tail = line.read();
                wet_l += tail * OUT_L[i];
                wet_r += tail * OUT_R[i];
                *damp += self.damp_coeff * (tail - *damp);
                *fed_back = gain * *damp;
                sum += *fed_back;
            }
            let house = 2.0 / N as f32 * sum;
            for (line, fed_back) in self.lines.iter_mut().zip(&v) {
                line.write(input + fed_back - house);
            }

            let mix = self.mix.tick();
            *l = dry_l + mix * (wet_l * WET_SCALE - dry_l);
            *r = dry_r + mix * (wet_r * WET_SCALE - dry_r);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{assert_finite, rms, silence};

    const SR: u32 = 48_000;

    fn prepared(t60: f32) -> Reverb {
        let mut r = Reverb::new();
        r.prepare(SR);
        r.set_param(0, PARAMS[0].range.to_norm(t60));
        r.set_param(3, PARAMS[3].range.to_norm(1.0)); // full wet for tail tests
        let _ = r.mix.tick(); // ensure smoothing target registered
        r.mix.snap_to_target();
        r
    }

    /// Render `secs` of stereo impulse response.
    fn impulse_response_stereo(r: &mut Reverb, secs: f32) -> (Vec<f32>, Vec<f32>) {
        let n = (SR as f32 * secs) as usize;
        let mut l = vec![0.0f32; n];
        l[0] = 1.0;
        let mut right = l.clone();
        for (a, b) in l.chunks_mut(64).zip(right.chunks_mut(64)) {
            r.process(a, b);
        }
        (l, right)
    }

    /// Render `secs` of impulse response, left channel.
    fn impulse_response(r: &mut Reverb, secs: f32) -> Vec<f32> {
        impulse_response_stereo(r, secs).0
    }

    fn window_rms(x: &[f32], from_s: f32, to_s: f32) -> f32 {
        let a = (SR as f32 * from_s) as usize;
        let b = ((SR as f32 * to_s) as usize).min(x.len());
        rms(&x[a..b])
    }

    #[test]
    fn tail_decays_monotonically() {
        let mut r = prepared(1.0);
        let ir = impulse_response(&mut r, 3.0);
        assert_finite("reverb ir", &ir);
        let early = window_rms(&ir, 0.05, 0.30);
        let mid = window_rms(&ir, 0.8, 1.2);
        let late = window_rms(&ir, 2.0, 2.8);
        assert!(
            early > mid && mid > late,
            "tail must decay: {early} {mid} {late}"
        );
        // ~1 s T60: by 2 s the tail sits far below the early field.
        assert!(late < early * 0.01, "late tail too loud: {late} vs {early}");
    }

    #[test]
    fn decay_parameter_stretches_the_tail() {
        let mut short = prepared(0.3);
        let mut long = prepared(6.0);
        let ir_short = impulse_response(&mut short, 2.0);
        let ir_long = impulse_response(&mut long, 2.0);
        let at_1s = |ir: &[f32]| window_rms(ir, 0.9, 1.3);
        assert!(
            at_1s(&ir_long) > at_1s(&ir_short) * 10.0,
            "6 s decay must ring much longer than 0.3 s: {} vs {}",
            at_1s(&ir_long),
            at_1s(&ir_short)
        );
    }

    #[test]
    fn stays_bounded_over_a_long_render() {
        let mut r = prepared(8.0); // max decay
        let ir = impulse_response(&mut r, 10.0);
        assert_finite("long render", &ir);
        let peak = ir.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!(peak < 4.0, "FDN must not blow up: peak {peak}");
    }

    #[test]
    fn predelay_shifts_the_onset() {
        let mut r = prepared(1.0);
        r.set_param(2, PARAMS[2].range.to_norm(100.0));
        let ir = impulse_response(&mut r, 0.5);
        // Nothing (but the dry impulse) before the 100 ms predelay +
        // shortest line (~30 ms) has elapsed.
        let before = rms(&ir[SR as usize * 5 / 1000..SR as usize * 120 / 1000]);
        let after = window_rms(&ir, 0.14, 0.4);
        assert!(before < 1e-6, "silent before predelay, rms {before}");
        assert!(after > 1e-4, "tail must arrive after predelay");
    }

    #[test]
    fn mix_zero_is_bit_exact_dry() {
        let mut r = prepared(2.0);
        r.set_param(3, 0.0);
        let mut warm = vec![0.1f32; SR as usize]; // let mix settle to 0
        let mut warm_r = warm.clone();
        r.process(&mut warm, &mut warm_r);
        let x: Vec<f32> = (0..8_192).map(|i| (i as f32 * 0.01).sin() * 0.5).collect();
        let mut y = x.clone();
        let mut yr = x.clone();
        r.process(&mut y, &mut yr);
        assert_eq!(x, y, "mix 0 must pass dry (L)");
        assert_eq!(x, yr, "mix 0 must pass dry (R)");
    }

    #[test]
    fn stereo_tails_share_energy_but_decorrelate() {
        let mut r = prepared(2.0);
        let (l, right) = impulse_response_stereo(&mut r, 1.0);
        assert_finite("stereo ir L", &l);
        assert_finite("stereo ir R", &right);
        let tail = SR as usize / 10..; // skip the direct field
        let (tl, tr) = (&l[tail.clone()], &right[tail]);
        let rl = rms(tl);
        let rr = rms(tr);
        assert!(
            (rl / rr).max(rr / rl) < 1.6,
            "channel tail energy must roughly match: {rl} vs {rr}"
        );
        // Normalized cross-correlation at lag 0 should be well below 1.
        let dot: f64 = tl
            .iter()
            .zip(tr)
            .map(|(a, b)| f64::from(*a) * f64::from(*b))
            .sum();
        let corr = dot / (f64::from(rl) * f64::from(rr) * tl.len() as f64);
        assert!(
            corr.abs() < 0.5,
            "tails must decorrelate, correlation {corr:.3}"
        );
    }

    #[test]
    fn silence_in_silence_out_after_reset() {
        let mut r = prepared(2.0);
        let ir = impulse_response(&mut r, 0.5); // excite
        assert!(rms(&ir) > 0.0);
        r.reset();
        let mut x = silence(8_192);
        let mut xr = silence(8_192);
        r.process(&mut x, &mut xr);
        assert!(
            rms(&x) == 0.0 && rms(&xr) == 0.0,
            "reset must clear the tail"
        );
    }

    #[test]
    fn survives_all_rates_and_block_sizes() {
        for sr in [44_100u32, 48_000, 96_000] {
            let mut r = Reverb::new();
            r.prepare(sr);
            for chunk in [32usize, 483, 1_024] {
                let mut x: Vec<f32> = (0..4_096).map(|i| (i as f32 * 0.05).sin() * 0.5).collect();
                let mut xr = x.clone();
                for (a, b) in x.chunks_mut(chunk).zip(xr.chunks_mut(chunk)) {
                    r.process(a, b);
                }
                assert_finite("reverb multirate", &x);
                assert_finite("reverb multirate R", &xr);
            }
        }
    }
}

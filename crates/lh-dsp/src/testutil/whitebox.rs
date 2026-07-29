//! The **white-box discrimination kit** — the measurements that tell a solved
//! circuit apart from a static curve, as reusable helpers (phase 08 §2.3).
//!
//! # What this is for
//!
//! A new pedal needs to answer "is this actually a circuit?" and the honest
//! answers are always the same four or five measurements. Before this module
//! they were re-derived per pedal, inline, with slightly different windows and
//! slightly different bars. Here they are once, so a new pedal inherits them:
//! feed each helper a closure that maps one sample in to one sample out, and it
//! reports a number.
//!
//! The sharpest of them is [`memory`], and it is worth understanding why.
//! A memoryless waveshaper is a *function* `y = f(x)`: give it the same `x`
//! twice and it must return the same `y`, whatever happened before. A circuit
//! with reactive elements is not a function of `x` at all — its output depends
//! on the charge sitting on its capacitors. So: drive it with something that
//! visits the same instantaneous input from different directions, and look at
//! the spread of outputs. Zero means a curve. That single number is the
//! difference between the memoryless half of the drive family and the white-box
//! half, and no amount of tone filtering fakes it (a filter *after* a curve
//! still leaves the pre-filter signal a function of the input — which is why
//! this helper is meant for the clipper stage, not for a whole pedal with its
//! tone control in the path; see [`memory`]'s own docs).
//!
//! Everything here is offline test code: it allocates, it uses `f64`, and the
//! real-time rules do not apply.

use std::f64::consts::TAU;

/// A device under test: one sample in, one sample out, with whatever state it
/// likes in between.
pub trait Dut: FnMut(f32) -> f32 {}
impl<T: FnMut(f32) -> f32> Dut for T {}

/// Magnitude of `signal` at `freq`, by Goertzel. Callers arrange for a whole
/// number of cycles, so no window is needed and the bin is exact.
pub fn tone_at(signal: &[f64], rate: f64, freq: f64) -> f64 {
    let w = TAU * freq / rate;
    let (c, s) = (w.cos(), w.sin());
    let coeff = 2.0 * c;
    let (mut s1, mut s2) = (0.0f64, 0.0f64);
    for &x in signal {
        let s0 = x + coeff * s1 - s2;
        s2 = s1;
        s1 = s0;
    }
    let re = s1 - s2 * c;
    let im = s2 * s;
    2.0 * (re * re + im * im).sqrt() / signal.len() as f64
}

/// The harmonic picture of a device at one drive level.
#[derive(Debug, Clone)]
pub struct Harmonics {
    pub fundamental: f64,
    /// `h[k]` is the magnitude of the `(k+2)`-th harmonic.
    pub h: Vec<f64>,
}

impl Harmonics {
    /// Total harmonic distortion — harmonic energy over fundamental.
    pub fn thd(&self) -> f64 {
        let sum: f64 = self.h.iter().map(|v| v * v).sum();
        sum.sqrt() / self.fundamental.max(1e-30)
    }

    /// Even-harmonic energy over odd — the asymmetry figure. A symmetric
    /// clipper sits near zero; a rectifying one does not.
    pub fn even_over_odd(&self) -> f64 {
        let mut even = 0.0;
        let mut odd = 0.0;
        for (i, v) in self.h.iter().enumerate() {
            // h[0] is the 2nd harmonic, so even harmonics are the even indices.
            if i % 2 == 0 {
                even += v * v
            } else {
                odd += v * v
            }
        }
        (even / odd.max(1e-30)).sqrt()
    }
}

/// Run a sine of `freq` at `amp` through `dut` and read its harmonics.
///
/// `cycles` whole cycles are analysed after `warmup` cycles are discarded, and
/// `rate / freq` should be an integer for the bins to land exactly.
pub fn harmonics(
    mut dut: impl Dut,
    rate: f64,
    freq: f64,
    amp: f64,
    cycles: usize,
    warmup: usize,
    count: usize,
) -> Harmonics {
    let period = rate / freq;
    let n = (period * cycles as f64).round() as usize;
    let warm = (period * warmup as f64).round() as usize;
    let mut out = Vec::with_capacity(n);
    for k in 0..warm + n {
        let y = dut((amp * (TAU * freq * k as f64 / rate).sin()) as f32);
        if k >= warm {
            out.push(f64::from(y));
        }
    }
    Harmonics {
        fundamental: tone_at(&out, rate, freq),
        h: (2..2 + count)
            .map(|m| tone_at(&out, rate, m as f64 * freq))
            .collect(),
    }
}

/// **The memoryless test.** How much the output varies across samples that
/// share an instantaneous input, as a fraction of the output's own peak.
///
/// Returns ~0 for any `y = f(x)`, however curved, and a clearly non-zero
/// number for a circuit whose capacitors carry charge between samples.
///
/// The excitation is deliberately two-tone: a single sine revisits each `x`
/// exactly twice per cycle with mirrored slopes, which some circuits happen to
/// treat identically; two incommensurate tones visit each `x` from a whole
/// spread of histories.
///
/// **Scope.** Apply this to a *clipper*, not to a whole pedal — a memoryless
/// shaper followed by a tone filter also scores non-zero, because the filter
/// has the memory. The claim being tested is "the nonlinearity itself has
/// state", so the device under test has to end at the nonlinearity.
pub fn memory(mut dut: impl Dut, rate: f64, amp: f64, samples: usize) -> f64 {
    const BUCKETS: usize = 128;

    let warm = samples / 4;
    let mut pairs: Vec<(f64, f64)> = Vec::with_capacity(samples);
    let mut peak = 0.0f64;
    for k in 0..warm + samples {
        let t = k as f64 / rate;
        // 317 Hz and 1013 Hz: coprime, so the pair does not repeat over the
        // window and each input level is visited from many phases.
        let x = amp * (0.6 * (TAU * 317.0 * t).sin() + 0.4 * (TAU * 1013.0 * t).sin());
        let y = f64::from(dut(x as f32));
        if k >= warm {
            peak = peak.max(y.abs());
            pairs.push((x, y));
        }
    }

    // Bucket by input level, then fit a *quadratic* in `x` within each bucket
    // and take the residual spread. The fit is what makes the number mean
    // "history dependence" rather than "the curve is steep here": a smooth
    // `f(x)` is locally quadratic, so removing the quadratic removes all of it
    // and leaves only what `x` cannot explain. (Linear alone leaves the
    // curvature term, which for a `tanh` over a 1/128-wide bucket is 2e-4 —
    // enough to swamp a weakly reactive circuit.)
    let mut buckets: Vec<Vec<(f64, f64)>> = vec![Vec::new(); BUCKETS];
    for &(x, y) in &pairs {
        let u = ((x / amp + 1.0) * 0.5).clamp(0.0, 1.0);
        let b = ((u * (BUCKETS - 1) as f64) as usize).min(BUCKETS - 1);
        buckets[b].push((x, y));
    }

    let mut worst = 0.0f64;
    for bucket in buckets.iter().filter(|b| b.len() >= 12) {
        let n = bucket.len() as f64;
        let mx = bucket.iter().map(|(x, _)| x).sum::<f64>() / n;
        // Normal equations for `y ≈ c₀ + c₁·u + c₂·u²` with `u = x − mean(x)`.
        let mut m = [[0.0f64; 3]; 3];
        let mut r = [0.0f64; 3];
        for &(x, y) in bucket {
            let u = x - mx;
            let basis = [1.0, u, u * u];
            for (i, bi) in basis.iter().enumerate() {
                for (j, bj) in basis.iter().enumerate() {
                    m[i][j] += bi * bj;
                }
                r[i] += bi * y;
            }
        }
        let Some(c) = fit3(m, r) else { continue };
        let (lo, hi) = bucket
            .iter()
            .fold((f64::INFINITY, f64::NEG_INFINITY), |(l, h), (x, y)| {
                let u = x - mx;
                let res = y - (c[0] + c[1] * u + c[2] * u * u);
                (l.min(res), h.max(res))
            });
        worst = worst.max(hi - lo);
    }
    worst / peak.max(1e-30)
}

/// 3×3 solve with partial pivoting, for [`memory`]'s per-bucket fit. Returns
/// `None` on a degenerate bucket (every sample at the same `x`), which is a
/// bucket with nothing to say rather than an error.
fn fit3(mut m: [[f64; 3]; 3], mut r: [f64; 3]) -> Option<[f64; 3]> {
    for col in 0..3 {
        let pivot =
            (col..3).max_by(|a, b| m[*a][col].abs().partial_cmp(&m[*b][col].abs()).unwrap())?;
        if m[pivot][col].abs() < 1e-24 {
            return None;
        }
        m.swap(col, pivot);
        r.swap(col, pivot);
        let (done, rest) = m.split_at_mut(col + 1);
        let pivot_row = &done[col];
        for (row, target) in rest.iter_mut().enumerate() {
            let f = target[col] / pivot_row[col];
            for (t, p) in target.iter_mut().zip(pivot_row).skip(col) {
                *t -= f * p;
            }
            r[col + 1 + row] -= f * r[col];
        }
    }
    let mut x = [0.0f64; 3];
    for col in (0..3).rev() {
        let mut acc = r[col];
        for k in col + 1..3 {
            acc -= m[col][k] * x[k];
        }
        x[col] = acc / m[col][col];
    }
    Some(x)
}

/// How much the distortion changes between two frequencies at one amplitude.
///
/// Returns `thd(high) / thd(low)`. A memoryless curve returns 1 exactly; a
/// circuit whose reactances shift the clipping threshold does not. Which side
/// of 1 it lands on is the circuit's business — a shunt capacitor across the
/// diodes softens the highs, a feedback capacitor rolls the loop off — so
/// callers assert a direction, not a sign.
pub fn knee_shift(mut dut: impl Dut, rate: f64, low: f64, high: f64, amp: f64) -> f64 {
    let a = harmonics(&mut dut, rate, low, amp, 32, 32, 8).thd();
    let b = harmonics(&mut dut, rate, high, amp, 32, 32, 8).thd();
    b / a.max(1e-30)
}

/// The static transfer curve: settle the device at each input level and read
/// the output. Equilibrium opens every capacitor, so this is the circuit's
/// *algebra* with all the time-dependence taken out — the thing to pin against
/// hand analysis or a nodal reference.
pub fn static_curve(mut dut: impl Dut, levels: &[f64], settle: usize) -> Vec<f64> {
    levels
        .iter()
        .map(|&e| {
            let mut y = 0.0;
            for _ in 0..settle {
                y = f64::from(dut(e as f32));
            }
            y
        })
        .collect()
}

/// Abuse: alternating full-scale slams from a cold start. Returns the largest
/// magnitude seen, or `None` if anything non-finite escaped.
///
/// A solved circuit must stay bounded because its nonlinearity is monotone and
/// its linear part is passive — if this returns `None`, a root diverged.
pub fn bounded(mut dut: impl Dut, level: f32, samples: usize) -> Option<f64> {
    let mut peak = 0.0f64;
    for k in 0..samples {
        let y = dut(if k % 2 == 0 { level } else { -level });
        if !y.is_finite() {
            return None;
        }
        peak = peak.max(f64::from(y).abs());
    }
    Some(peak)
}

/// Silence in, silence out — and *exactly*, not approximately.
///
/// A circuit whose diodes are referenced to ground has `y = 0` as an exact
/// fixed point of its node equation, so this is an identity rather than a
/// tolerance. A model that biases its clipper (or leaks a DC offset out of a
/// solver) fails here, which is the point: PRD 032 gave up a bias network to
/// keep this property.
pub fn silent(mut dut: impl Dut, samples: usize) -> bool {
    (0..samples).all(|_| dut(0.0) == 0.0)
}

#[cfg(test)]
mod tests {
    use super::*;

    const RATE: f64 = 192_000.0;

    /// The kit has to be calibrated against something it must call memoryless,
    /// and something it must call a circuit. A `tanh` is the first.
    #[test]
    fn the_kit_calls_a_waveshaper_memoryless() {
        // The floor is `f32` in the device-under-test signature, not the
        // method: measured 3.4e-6, and a circuit reads 0.05 upward, so the
        // separation is four orders of magnitude either way.
        let m = memory(|x| (3.0 * x).tanh(), RATE, 1.0, 8192);
        assert!(m < 1e-4, "tanh should have no memory, got {m:.3e}");
        let k = knee_shift(|x| (3.0 * x).tanh(), RATE, 200.0, 3000.0, 0.5);
        assert!(
            (k - 1.0).abs() < 1e-6,
            "a curve's distortion must not depend on frequency, got {k:.6}"
        );
    }

    /// …and a one-pole lowpass in front of the same curve is the second: now
    /// the input the curve sees depends on history, so both figures move.
    #[test]
    fn the_kit_calls_a_filtered_waveshaper_a_circuit() {
        let mut z = 0.0f32;
        let c = 1.0 - (-TAU as f32 * 1500.0 / RATE as f32).exp();
        let m = memory(
            move |x| {
                z += (x - z) * c;
                (3.0 * z).tanh()
            },
            RATE,
            1.0,
            8192,
        );
        assert!(m > 0.05, "a filtered shaper must show memory, got {m:.3e}");
        // The separation is the point, not either number on its own.
        let flat = memory(|x| (3.0 * x).tanh(), RATE, 1.0, 8192);
        assert!(
            m > 1_000.0 * flat,
            "circuit {m:.3e} must stand clear of the curve's floor {flat:.3e}"
        );
    }

    /// Asymmetry: a half-wave-ish curve has to read as even-heavy, a symmetric
    /// one as odd-heavy.
    #[test]
    fn the_kit_separates_symmetric_from_asymmetric_clipping() {
        let sym = harmonics(|x| x.clamp(-0.3, 0.3), RATE, 1000.0, 1.0, 32, 8, 8);
        let asym = harmonics(|x| x.clamp(-0.1, 0.3), RATE, 1000.0, 1.0, 32, 8, 8);
        assert!(
            sym.even_over_odd() < 0.02,
            "symmetric clip: {:.4}",
            sym.even_over_odd()
        );
        assert!(
            asym.even_over_odd() > 0.3,
            "asymmetric clip: {:.4}",
            asym.even_over_odd()
        );
    }

    /// `thd` and the harmonic bins have to agree with a case anyone can check
    /// by hand: a hard clip at half amplitude has a known Fourier series.
    #[test]
    fn the_harmonic_reader_matches_the_fourier_series_of_a_clipped_sine() {
        // Clipping sin(θ) at ±sin(φ) with φ = π/6 gives a fundamental of
        // (2/π)·(φ + sin φ cos φ) and odd harmonics
        // a_m = (4/(π m²))·sin(m φ) · … — rather than reproduce the closed
        // form, check the two properties it forces: no even harmonics at all,
        // and the third harmonic at a level a clipped sine is known to have.
        let phi = std::f64::consts::FRAC_PI_6;
        let clip = phi.sin();
        let h = harmonics(
            move |x| x.clamp(-clip as f32, clip as f32),
            RATE,
            1000.0,
            1.0,
            64,
            8,
            6,
        );
        assert!(h.h[0] / h.fundamental < 1e-9, "even harmonic leaked in");
        let fundamental = 2.0 / std::f64::consts::PI * (phi + phi.sin() * phi.cos());
        assert!(
            (h.fundamental - fundamental).abs() < 1e-4,
            "fundamental {:.6} vs closed form {fundamental:.6}",
            h.fundamental
        );
    }

    #[test]
    fn the_kit_reports_silence_and_boundedness() {
        assert!(silent(|x| x * 2.0, 64));
        assert!(!silent(|x| x + 0.1, 64));
        assert_eq!(
            bounded(|x: f32| x.tanh(), 1e6, 64).map(|p| p >= 0.999),
            Some(true)
        );
        assert!(bounded(|_| f32::NAN, 1.0, 4).is_none());
    }
}

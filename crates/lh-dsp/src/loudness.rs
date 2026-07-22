//! Integrated loudness (LUFS) to ITU-R BS.1770-4 — an **offline** analysis pass,
//! not an RT effect (PRD 016). Used by the app's `level` command to measure how
//! loud a preset renders a reference DI, so a per-preset master trim can even
//! out the volume jumps between presets.
//!
//! The chain is the standard's:
//! 1. **K-weighting** per channel — a two-stage IIR: a +4 dB high-shelf
//!    ("head" pre-filter) then a ~38 Hz high-pass (the RLB curve). Both are RBJ
//!    biquads designed from the physical parameters the spec fixes, so the
//!    weighting is correct at any sample rate (not just the tabulated 48 kHz).
//! 2. **Mean-square** over 400 ms blocks with 75 % overlap, summed across
//!    channels (`G = 1.0` for L and R).
//! 3. **Two-stage gating** — an absolute −70 LUFS gate, then a relative gate
//!    10 LU below the mean of the surviving blocks — so silence and quiet tails
//!    do not drag the integrated figure down.
//!
//! Pure and deterministic: buffer in, LUFS out. `f32::NEG_INFINITY` means
//! "nothing above the gate" (silence).

use crate::blocks::biquad::Biquad;

/// The BS.1770 loudness offset (calibrates the mean-square sum to LUFS).
const OFFSET_DB: f32 = -0.691;
/// Absolute gate — blocks quieter than this never count.
const ABSOLUTE_GATE_LUFS: f32 = -70.0;
/// Relative gate — surviving blocks must be within this many LU of the mean.
const RELATIVE_GATE_LU: f32 = 10.0;
/// Gating block length and hop (75 % overlap) in seconds.
const BLOCK_SEC: f32 = 0.4;
const HOP_SEC: f32 = 0.1;

// K-weighting physical parameters (BS.1770-4). Designed through the RBJ
// cookbook they reproduce the tabulated 48 kHz coefficients and generalize to
// any rate.
const SHELF_FC: f32 = 1_681.974_4;
const SHELF_GAIN_DB: f32 = 3.999_844;
const HIGHPASS_FC: f32 = 38.135_47;
const HIGHPASS_Q: f32 = 0.500_327_04;

/// Build the two K-weighting sections (shelf → high-pass) for `sample_rate`.
fn k_weighting(sample_rate: f32) -> (Biquad, Biquad) {
    let mut shelf = Biquad::default();
    shelf.set_high_shelf(sample_rate, SHELF_FC, SHELF_GAIN_DB);
    let mut hp = Biquad::default();
    hp.set_highpass(sample_rate, HIGHPASS_FC, HIGHPASS_Q);
    (shelf, hp)
}

/// K-weight one channel in place (offline: filters the whole buffer once, so
/// the biquad state is continuous across the overlapping analysis blocks).
fn k_weight(channel: &mut [f32], sample_rate: f32) {
    let (mut shelf, mut hp) = k_weighting(sample_rate);
    for s in channel.iter_mut() {
        *s = hp.process_sample(shelf.process_sample(*s));
    }
}

/// Mean square of `x`, or 0 for an empty slice.
fn mean_square(x: &[f32]) -> f64 {
    if x.is_empty() {
        return 0.0;
    }
    x.iter().map(|s| f64::from(*s) * f64::from(*s)).sum::<f64>() / x.len() as f64
}

/// Integrated loudness (LUFS) of a stereo signal to BS.1770-4. Pass the same
/// slice for both channels to measure a mono signal. Returns
/// [`f32::NEG_INFINITY`] when nothing rises above the absolute gate.
pub fn integrated_lufs(left: &[f32], right: &[f32], sample_rate: u32) -> f32 {
    let n = left.len().min(right.len());
    if n == 0 || sample_rate == 0 {
        return f32::NEG_INFINITY;
    }
    let sr = sample_rate as f32;
    let mut l = left[..n].to_vec();
    let mut r = right[..n].to_vec();
    k_weight(&mut l, sr);
    k_weight(&mut r, sr);

    let block = (BLOCK_SEC * sr) as usize;
    let hop = (HOP_SEC * sr).max(1.0) as usize;
    if block == 0 || n < block {
        return f32::NEG_INFINITY; // shorter than one gating block
    }

    // Per-block channel-summed mean square (z_j) and its block loudness (l_j).
    let mut z: Vec<f64> = Vec::new();
    let mut start = 0;
    while start + block <= n {
        let zl = mean_square(&l[start..start + block]);
        let zr = mean_square(&r[start..start + block]);
        z.push(zl + zr); // G_L = G_R = 1.0
        start += hop;
    }

    let loudness_of = |ms: f64| -> f32 { OFFSET_DB + 10.0 * (ms.max(1e-30)).log10() as f32 };

    // Absolute gate at −70 LUFS.
    let abs_kept: Vec<f64> = z
        .iter()
        .copied()
        .filter(|&zj| loudness_of(zj) >= ABSOLUTE_GATE_LUFS)
        .collect();
    if abs_kept.is_empty() {
        return f32::NEG_INFINITY;
    }

    // Relative gate: 10 LU below the mean loudness of the absolute-gated blocks.
    let abs_mean = abs_kept.iter().sum::<f64>() / abs_kept.len() as f64;
    let rel_gate = loudness_of(abs_mean) - RELATIVE_GATE_LU;
    let rel_kept: Vec<f64> = abs_kept
        .iter()
        .copied()
        .filter(|&zj| loudness_of(zj) >= rel_gate)
        .collect();
    if rel_kept.is_empty() {
        return f32::NEG_INFINITY;
    }

    let mean = rel_kept.iter().sum::<f64>() / rel_kept.len() as f64;
    loudness_of(mean)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testutil::{silence, sine};

    const SR: u32 = 48_000;

    /// A stereo sine at `dbfs` (peak), both channels identical.
    fn stereo_sine(dbfs: f32, freq: f32, secs: f32) -> (Vec<f32>, Vec<f32>) {
        let amp = 10f32.powf(dbfs / 20.0);
        let x: Vec<f32> = sine(SR, freq, (SR as f32 * secs) as usize)
            .iter()
            .map(|s| s * amp)
            .collect();
        (x.clone(), x)
    }

    #[test]
    fn calibrated_against_a_reference_sine() {
        // BS.1770 is calibrated so identical in-phase 1 kHz sines read their
        // dBFS-peak value in LUFS. Our K-weighting should land a −18 dBFS sine
        // within a fraction of a LU of −18.
        let (l, r) = stereo_sine(-18.0, 1_000.0, 3.0);
        let lufs = integrated_lufs(&l, &r, SR);
        assert!(
            (lufs - -18.0).abs() < 0.7,
            "−18 dBFS sine measured {lufs} LUFS"
        );
    }

    #[test]
    fn loudness_tracks_level_one_for_one() {
        // A 6 dB louder signal is 6 LU louder — the property leveling relies on.
        let (l0, r0) = stereo_sine(-24.0, 1_000.0, 3.0);
        let (l1, r1) = stereo_sine(-18.0, 1_000.0, 3.0);
        let a = integrated_lufs(&l0, &r0, SR);
        let b = integrated_lufs(&l1, &r1, SR);
        assert!((b - a - 6.0).abs() < 0.1, "expected +6 LU, got {}", b - a);
    }

    #[test]
    fn k_weighting_favors_highs() {
        // The high-shelf makes a 6 kHz tone measure louder than a 100 Hz tone
        // at the same dBFS (the whole point of K-weighting).
        let (ll, lr) = stereo_sine(-20.0, 100.0, 3.0);
        let (hl, hr) = stereo_sine(-20.0, 6_000.0, 3.0);
        let low = integrated_lufs(&ll, &lr, SR);
        let high = integrated_lufs(&hl, &hr, SR);
        assert!(high > low + 2.0, "highs must weight up: {low} vs {high}");
    }

    #[test]
    fn silence_gates_to_negative_infinity() {
        let s = silence(SR as usize * 2);
        assert_eq!(integrated_lufs(&s, &s, SR), f32::NEG_INFINITY);
    }

    #[test]
    fn quiet_tail_is_gated_out() {
        // A loud passage then a long quiet tail: the integrated figure reflects
        // the loud part, barely pulled down by the gated tail.
        let (mut l, mut r) = stereo_sine(-14.0, 1_000.0, 3.0);
        let loud = integrated_lufs(&l, &r, SR);
        let (ql, qr) = stereo_sine(-50.0, 1_000.0, 4.0);
        l.extend(ql);
        r.extend(qr);
        let gated = integrated_lufs(&l, &r, SR);
        assert!(
            (gated - loud).abs() < 1.0,
            "quiet tail must be gated: loud {loud}, with tail {gated}"
        );
    }

    #[test]
    fn survives_all_rates() {
        for sr in [44_100u32, 48_000, 96_000] {
            let amp = 10f32.powf(-18.0 / 20.0);
            let x: Vec<f32> = sine(sr, 1_000.0, sr as usize * 3)
                .iter()
                .map(|s| s * amp)
                .collect();
            let lufs = integrated_lufs(&x, &x, sr);
            assert!(
                (lufs - -18.0).abs() < 0.8,
                "sr {sr}: −18 dBFS measured {lufs} LUFS"
            );
        }
    }
}

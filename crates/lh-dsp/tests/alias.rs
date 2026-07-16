//! Proof that oversampling earns its cost: drive a 6.3 kHz sine hard and
//! compare aliasing against a naive full-rate shaper.
//!
//! At 48 kHz, tanh's 7th harmonic of 6.3 kHz sits at 44.1 kHz and folds down
//! to |44100 − 48000| = 3.9 kHz — below the fundamental, impossible to miss.
//! The 4× oversampled path must suppress that image by a clear margin.

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_dsp::testutil::sine;
use realfft::RealFftPlanner;

const SR: u32 = 48_000;
const F0: f32 = 6_300.0;
const N: usize = 9_600; // 5 Hz bins; 6300 → bin 1260, 3900 → bin 780
const DRIVE_DB: f32 = 30.0;

fn spectrum_db(signal: &[f32]) -> Vec<f32> {
    let mut planner = RealFftPlanner::<f32>::new();
    let fft = planner.plan_fft_forward(N);
    let mut input: Vec<f32> = signal[signal.len() - N..]
        .iter()
        .enumerate()
        .map(|(i, s)| {
            // Hann window against leakage.
            let w = 0.5 - 0.5 * (2.0 * std::f32::consts::PI * i as f32 / (N - 1) as f32).cos();
            s * w
        })
        .collect();
    let mut out = fft.make_output_vec();
    fft.process(&mut input, &mut out).unwrap();
    out.iter()
        .map(|c| 20.0 * c.norm().max(1e-12).log10())
        .collect()
}

/// Peak magnitude in a small window around a bin (leakage tolerance).
fn peak_around(spec: &[f32], bin: usize) -> f32 {
    spec[bin - 3..=bin + 3]
        .iter()
        .fold(f32::NEG_INFINITY, |m, v| m.max(*v))
}

#[test]
fn oversampled_drive_suppresses_aliasing_by_at_least_20_db() {
    let x = sine(SR, F0, 4 * N);

    // Reference: the same shaper with no oversampling.
    let gain = lh_core::db_to_lin(DRIVE_DB);
    let naive: Vec<f32> = x
        .iter()
        .map(|s| (s * gain + 0.2).tanh() - 0.2f32.tanh())
        .collect();

    // Device under test: the classic model (the same biased tanh as the
    // naive reference), tone wide open so the lowpass doesn't do the work.
    let mut drive = Drive::new();
    drive.prepare(SR);
    drive.set_param(0, 1.0); // model = classic (last registry index)
    drive.set_param(1, lh_core::drive_law::classic_drive_pos(DRIVE_DB) / 10.0);
    drive.set_param(2, 1.0); // tone pos 10 = 8 kHz
    drive.set_param(3, lh_core::drive_law::level_pos(1.0) / 10.0); // unity
    let mut processed = x.clone();
    let mut processed_r = x.clone();
    for (chunk, chunk_r) in processed.chunks_mut(256).zip(processed_r.chunks_mut(256)) {
        drive.process(chunk, chunk_r);
    }

    let fund_bin = (F0 / 5.0) as usize; // 1260
    let alias_bin = ((SR as f32 - 7.0 * F0) / 5.0) as usize; // |44100 − 48000| → 780

    let naive_spec = spectrum_db(&naive);
    let os_spec = spectrum_db(&processed);

    // Compare alias level *relative to each signal's own fundamental* so gain
    // staging and the residual tone filter cancel out of the comparison.
    let naive_alias_rel = peak_around(&naive_spec, alias_bin) - peak_around(&naive_spec, fund_bin);
    let os_alias_rel = peak_around(&os_spec, alias_bin) - peak_around(&os_spec, fund_bin);
    let improvement = naive_alias_rel - os_alias_rel;

    assert!(
        naive_alias_rel > -60.0,
        "test premise: naive shaper must alias audibly (got {naive_alias_rel:.1} dB)"
    );
    assert!(
        improvement >= 20.0,
        "oversampling must suppress the folded 7th harmonic by ≥20 dB, got {improvement:.1} dB \
         (naive {naive_alias_rel:.1} dB, oversampled {os_alias_rel:.1} dB)"
    );
}

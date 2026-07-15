//! Load/process tests against a real (tiny) WaveNet capture — see
//! `fixtures/README.md` for provenance.

use std::path::PathBuf;

use lh_dsp::Effect;
use lh_dsp::testutil::{assert_finite, rms, sine};
use lh_nam::{NamAmp, NamError, load_nam_file};

fn fixture() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reference.nam")
}

#[test]
fn loads_and_reports_metadata() {
    let (asset, info) = load_nam_file(&fixture(), 48_000).unwrap();
    assert_eq!(info.architecture, "WaveNet");
    assert_eq!(info.sample_rate, 48_000);
    let loudness = info.loudness_db.expect("fixture carries loudness");
    assert!((loudness - -20.02).abs() < 0.1);
    assert!(info.normalized);
    // Normalization: -18 target vs -20.02 capture ⇒ ~+2 dB.
    assert!((asset.base_gain - lh_core::db_to_lin(2.02)).abs() < 0.02);
}

#[test]
fn rejects_rate_mismatch_with_actionable_error() {
    let err = match load_nam_file(&fixture(), 44_100) {
        Err(e) => e,
        Ok(_) => panic!("expected a rate-mismatch error"),
    };
    match err {
        NamError::RateMismatch { model, engine } => {
            assert_eq!(model, 48_000);
            assert_eq!(engine, 44_100);
        }
        other => panic!("expected RateMismatch, got {other}"),
    }
}

#[test]
fn amp_processes_audio_through_the_swap_seam() {
    let (mut amp, mut handle) = NamAmp::new();
    amp.prepare(48_000);

    // Before install: exact passthrough.
    let x = sine(48_000, 220.0, 4_096);
    let mut y = x.clone();
    for block in y.chunks_mut(64) {
        amp.process(block);
    }
    assert_eq!(x, y, "unloaded amp must be a passthrough");

    // Install the capture and process again.
    let (asset, _info) = load_nam_file(&fixture(), 48_000).unwrap();
    handle.install(asset).unwrap();
    let mut z = x.clone();
    for block in z.chunks_mut(64) {
        amp.process(block);
    }
    assert_finite("amp output", &z);
    assert!(rms(&z[2_048..]) > 1e-4, "model must produce signal");
    let diff = x
        .iter()
        .zip(&z)
        .map(|(a, b)| (a - b).abs())
        .fold(0.0f32, f32::max);
    assert!(diff > 1e-3, "model must actually change the signal");

    // Unload: passthrough again, old model comes back to die here.
    assert!(handle.clear());
    let mut w = x.clone();
    for block in w.chunks_mut(64) {
        amp.process(block);
    }
    assert_eq!(x, w);
    assert_eq!(handle.collect_garbage(), 1);
}

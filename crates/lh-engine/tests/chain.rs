//! Chain integration: the full M1 pedalboard rendered offline, driven through
//! the same message queue the CLI uses.

use lh_dsp::Effect;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::gate::NoiseGate;
use lh_dsp::testutil::{assert_finite, rms, sine};
use lh_engine::build_chain;

const SR: u32 = 48_000;

fn pedalboard() -> Vec<Box<dyn Effect>> {
    vec![
        Box::new(NoiseGate::new()),
        Box::new(Drive::new()),
        Box::new(Delay::new()),
    ]
}

#[test]
fn full_chain_renders_finite_audio() {
    let (mut chain, _handle) = build_chain(pedalboard());
    chain.prepare(SR);

    let mut x = sine(SR, 220.0, SR as usize);
    for block in x.chunks_mut(256) {
        chain.process(block);
    }
    assert_finite("chain output", &x);
    assert!(rms(&x[SR as usize / 2..]) > 0.05, "signal must pass");
}

#[test]
fn handles_blocks_larger_than_internal_chunk() {
    let (mut chain, _handle) = build_chain(pedalboard());
    chain.prepare(SR);
    let mut x = sine(SR, 220.0, 4_096); // > MAX_BLOCK, forces chunking
    chain.process(&mut x);
    assert_finite("big block", &x);
}

#[test]
fn params_travel_through_the_queue() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    // Crank the drive via the handle; output harmonics must increase.
    let render = |chain: &mut lh_engine::Chain| {
        let mut x = sine(SR, 220.0, SR as usize / 2);
        for block in x.chunks_mut(256) {
            chain.process(block);
        }
        chain.reset();
        rms(&x[SR as usize / 4..])
    };

    let applied = handle.set_param("drive", "drive", 0.0).unwrap();
    assert_eq!(applied.unit, "dB");
    let quiet = render(&mut chain);

    handle.set_param("drive", "drive", 40.0).unwrap();
    handle.set_param("drive", "level", 6.0).unwrap();
    let loud = render(&mut chain);
    assert!(
        loud > quiet * 1.2,
        "cranked drive must be audibly hotter: {loud} vs {quiet}"
    );

    // Values are clamped and echoed back.
    let clamped = handle.set_param("delay", "feedback", 5.0).unwrap();
    assert!((clamped.real - 0.9).abs() < 1e-6);
}

#[test]
fn bypassing_everything_becomes_a_passthrough() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    handle.set_active("gate", false).unwrap();
    handle.set_active("drive", false).unwrap();
    handle.set_active("delay", false).unwrap();

    // First blocks apply the messages and ride the 10 ms crossfade out.
    let mut warm = sine(SR, 220.0, SR as usize / 10);
    for block in warm.chunks_mut(64) {
        chain.process(block);
    }

    // Once settled, bypassed slots are skipped entirely: exact passthrough.
    let x = sine(SR, 220.0, 8_192);
    let mut y = x.clone();
    for block in y.chunks_mut(64) {
        chain.process(block);
    }
    assert_eq!(x, y, "settled bypass must be bit-exact passthrough");
}

#[test]
fn unknown_keys_are_rejected() {
    let (_chain, mut handle) = build_chain(pedalboard());
    assert!(handle.set_param("wah", "position", 0.5).is_err());
    assert!(handle.set_param("drive", "sparkle", 0.5).is_err());
    assert!(handle.set_active("chorus", true).is_err());
}

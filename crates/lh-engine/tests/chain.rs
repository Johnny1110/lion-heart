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

// --- reorder & preset machinery, probed with two distinguishable effects ---

use lh_core::{EffectDesc, ParamDesc};

static NO_PARAMS: [ParamDesc; 0] = [];
static ADD_DESC: EffectDesc = EffectDesc {
    key: "add",
    name: "Add One",
    params: &NO_PARAMS,
};
static MUL_DESC: EffectDesc = EffectDesc {
    key: "mul",
    name: "Times Two",
    params: &NO_PARAMS,
};

struct AddOne;
impl Effect for AddOne {
    fn descriptor(&self) -> &'static EffectDesc {
        &ADD_DESC
    }
    fn prepare(&mut self, _: u32) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: usize, _: f32) {}
    fn process(&mut self, block: &mut [f32]) {
        for x in block.iter_mut() {
            *x += 1.0;
        }
    }
}

struct TimesTwo;
impl Effect for TimesTwo {
    fn descriptor(&self) -> &'static EffectDesc {
        &MUL_DESC
    }
    fn prepare(&mut self, _: u32) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: usize, _: f32) {}
    fn process(&mut self, block: &mut [f32]) {
        for x in block.iter_mut() {
            *x *= 2.0;
        }
    }
}

fn probe_chain() -> (lh_engine::Chain, lh_engine::ChainHandle) {
    let (mut chain, handle) = build_chain(vec![Box::new(AddOne), Box::new(TimesTwo)]);
    chain.prepare(SR);
    (chain, handle)
}

/// Feed DC 1.0 and return the settled output value.
fn settled_value(chain: &mut lh_engine::Chain) -> f32 {
    let mut last = 0.0;
    for _ in 0..200 {
        // 200 × 64 ≈ 267 ms — enough for any fade to finish
        let mut block = [1.0f32; 64];
        chain.process(&mut block);
        last = block[63];
    }
    last
}

#[test]
fn reorder_changes_processing_order() {
    let (mut chain, mut handle) = probe_chain();
    assert_eq!(handle.order_keys(), ["add", "mul"]);
    assert!((settled_value(&mut chain) - 4.0).abs() < 1e-3, "(1+1)*2");

    handle.set_order(&["mul", "add"]).unwrap();
    assert_eq!(handle.order_keys(), ["mul", "add"]);
    assert!((settled_value(&mut chain) - 3.0).abs() < 1e-3, "1*2+1");
}

#[test]
fn reorder_fades_through_silence_without_clicks() {
    let (mut chain, mut handle) = probe_chain();
    settled_value(&mut chain); // steady 4.0

    handle.set_order(&["mul", "add"]).unwrap();
    let mut out = Vec::new();
    for _ in 0..200 {
        let mut block = [1.0f32; 64];
        chain.process(&mut block);
        out.extend_from_slice(&block);
    }
    let dip = out.iter().fold(f32::INFINITY, |m, v| m.min(v.abs()));
    assert!(dip < 0.05, "must pass near silence, dip {dip}");
    let max_step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.2, "no hard switch, step {max_step}");
    assert!(
        (out.last().unwrap() - 3.0).abs() < 1e-3,
        "lands on new order"
    );
}

#[test]
fn bad_orders_are_rejected() {
    let (_chain, mut handle) = probe_chain();
    assert!(handle.set_order(&["add"]).is_err(), "missing slots");
    assert!(handle.set_order(&["add", "add"]).is_err(), "duplicate");
    assert!(handle.set_order(&["add", "wah"]).is_err(), "unknown");
}

#[test]
fn preset_snapshot_applies_back_identically() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.set_param("drive", "drive", 24.0).unwrap();
    handle.set_param("delay", "time", 500.0).unwrap();
    handle.set_active("gate", false).unwrap();
    handle.set_order(&["drive", "gate", "delay"]).unwrap();

    let saved = handle.snapshot_chain();
    assert_eq!(saved[0].key, "drive");
    assert_eq!(saved[0].params["drive"], 24.0);
    assert!(!saved[1].active);

    // A fresh chain of the same pedals, restored from the snapshot.
    let (mut chain2, mut handle2) = build_chain(pedalboard());
    chain2.prepare(SR);
    let warnings = handle2.apply_preset_chain(&saved).unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(handle2.snapshot_chain(), saved);
}

#[test]
fn input_tap_sees_raw_input_and_never_blocks() {
    let (mut chain, _handle) = build_chain(pedalboard());
    chain.prepare(SR);

    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(4_096);
    chain.set_input_tap(producer);

    // The tap must carry the *input* even though the chain mutates the block.
    let x = sine(SR, 220.0, 1_024);
    let mut y = x.clone();
    for block in y.chunks_mut(64) {
        chain.process(block);
    }
    let tapped: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
    assert_eq!(tapped, x, "tap is the pre-processing input");

    // Unread tap fills up: processing must go on, new samples are dropped.
    let mut z = sine(SR, 220.0, 3 * 4_096);
    chain.process(&mut z);
    assert_finite("output with full tap", &z);
    assert_eq!(consumer.slots(), 4_096, "ring capped, nothing blocked");

    // A vanished consumer (tuner closed) must not disturb the audio path.
    drop(consumer);
    let mut w = sine(SR, 220.0, 512);
    chain.process(&mut w);
    assert_finite("output after consumer dropped", &w);
}

#[test]
fn preset_apply_is_forward_compatible() {
    use lh_core::preset::SlotState;
    use std::collections::BTreeMap;

    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    // A preset from "the future": unknown slot, unknown param, and it
    // doesn't mention the delay at all.
    let chain_states = vec![
        SlotState {
            key: "drive".into(),
            active: true,
            params: BTreeMap::from([("drive".into(), 30.0), ("sparkle".into(), 1.0)]),
        },
        SlotState {
            key: "wah".into(),
            active: true,
            params: BTreeMap::new(),
        },
        SlotState {
            key: "gate".into(),
            active: false,
            params: BTreeMap::new(),
        },
    ];
    let warnings = handle.apply_preset_chain(&chain_states).unwrap();
    assert_eq!(warnings.len(), 3, "{warnings:?}"); // sparkle, wah, delay

    let now = handle.snapshot_chain();
    assert_eq!(now[0].key, "drive");
    assert_eq!(now[0].params["drive"], 30.0);
    assert_eq!(now[1].key, "gate");
    assert_eq!(now[2].key, "delay", "unmentioned slot kept at the end");
}

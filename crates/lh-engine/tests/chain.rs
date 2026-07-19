//! Chain integration: the full M1 pedalboard rendered offline, driven through
//! the same message queue the CLI uses.

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::testutil::{assert_finite, rms, sine};
use lh_dsp::time::Delay;
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
    let mut xr = x.clone();
    for (l, r) in x.chunks_mut(256).zip(xr.chunks_mut(256)) {
        chain.process(l, r);
    }
    assert_finite("chain output L", &x);
    assert_finite("chain output R", &xr);
    assert!(rms(&x[SR as usize / 2..]) > 0.05, "signal must pass");
}

#[test]
fn handles_blocks_larger_than_internal_chunk() {
    let (mut chain, _handle) = build_chain(pedalboard());
    chain.prepare(SR);
    let mut x = sine(SR, 220.0, 4_096); // > MAX_BLOCK, forces chunking
    let mut xr = x.clone();
    chain.process(&mut x, &mut xr);
    assert_finite("big block", &x);
}

#[test]
fn params_travel_through_the_queue() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    // Crank the drive via the handle; output harmonics must increase.
    let render = |chain: &mut lh_engine::Chain| {
        let mut x = sine(SR, 220.0, SR as usize / 2);
        let mut xr = x.clone();
        for (l, r) in x.chunks_mut(256).zip(xr.chunks_mut(256)) {
            chain.process(l, r);
        }
        chain.reset();
        rms(&x[SR as usize / 4..])
    };

    let applied = handle.set_param("drive", "drive", 0.0).unwrap();
    assert_eq!(applied.unit, "");
    let quiet = render(&mut chain);

    handle.set_param("drive", "drive", 10.0).unwrap();
    handle.set_param("drive", "level", 10.0).unwrap();
    let loud = render(&mut chain);
    assert!(
        loud > quiet * 1.2,
        "cranked drive must be audibly hotter: {loud} vs {quiet}"
    );

    // Values are clamped and echoed back.
    let clamped = handle.set_param("delay", "feedback", 5.0).unwrap();
    assert!((clamped.real - 0.9).abs() < 1e-6);
}

/// Snapshots (PRD 009): `capture_scene` reflects the *selected* pedal's
/// live values and the bypass flag, so a scene stored now re-applies later.
#[test]
fn capture_scene_reflects_selected_pedal_and_bypass() {
    let (_chain, mut handle) = build_chain(pedalboard());

    handle.set_param("drive", "drive", 7.0).unwrap();
    handle.set_active("delay", false).unwrap();
    let delay_time = handle.set_param("delay", "time", 350.0).unwrap().real;

    let scene = handle.capture_scene();
    // One entry per handle, keyed by handle.
    assert_eq!(scene.slots.len(), 3);
    assert!(scene.slots.contains_key("gate"));

    let drive = &scene.slots["drive"];
    assert!(drive.active, "drive left active");
    assert!(
        (drive.values["drive"] - 7.0).abs() < 1e-4,
        "{:?}",
        drive.values
    );
    // Only the selected pedal's params are captured (not the whole family).
    assert!(
        drive.values.contains_key("drive") && !drive.values.contains_key("gain"),
        "captured the selected pedal's faceplate only: {:?}",
        drive.values.keys().collect::<Vec<_>>()
    );

    let delay = &scene.slots["delay"];
    assert!(!delay.active, "delay bypass captured");
    assert!((delay.values["time"] - delay_time).abs() < 1e-3);
}

#[test]
fn bypassing_everything_becomes_a_passthrough() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    handle.set_active("gate", false).unwrap();
    handle.set_active("drive", false).unwrap();
    handle.set_active("delay", false).unwrap();

    // First blocks apply the messages and ride the 10 ms crossfade out.
    // Half scale throughout keeps the output safety ceiling (-0.3 dBFS)
    // disengaged — an engaged limiter recovers, but not bit-exactly fast.
    let mut warm: Vec<f32> = sine(SR, 220.0, SR as usize / 10)
        .iter()
        .map(|s| s * 0.5)
        .collect();
    let mut warm_r = warm.clone();
    for (l, r) in warm.chunks_mut(64).zip(warm_r.chunks_mut(64)) {
        chain.process(l, r);
    }

    // Once settled, bypassed slots are skipped entirely: exact passthrough.
    let x: Vec<f32> = sine(SR, 220.0, 8_192).iter().map(|s| s * 0.5).collect();
    let mut y = x.clone();
    let mut yr = x.clone();
    for (l, r) in y.chunks_mut(64).zip(yr.chunks_mut(64)) {
        chain.process(l, r);
    }
    assert_eq!(x, y, "settled bypass must be bit-exact passthrough");
    assert_eq!(x, yr, "right channel too");
}

#[test]
fn unknown_keys_are_rejected() {
    let (_chain, mut handle) = build_chain(pedalboard());
    assert!(handle.set_param("wah", "position", 0.5).is_err());
    assert!(handle.set_param("drive", "sparkle", 0.5).is_err());
    assert!(handle.set_active("chorus", true).is_err());
}

// --- reorder & preset machinery, probed with two distinguishable effects ---

use lh_core::{EffectDesc, FamilyDesc, ParamDesc};

static NO_PARAMS: [ParamDesc; 0] = [];
static ADD_DESC: EffectDesc = EffectDesc {
    key: "add",
    name: "Add One",
    params: &NO_PARAMS,
};
static ADD_FAMILY: FamilyDesc = FamilyDesc {
    key: "add",
    name: "Add One",
    pedals: &[&ADD_DESC],
};
static MUL_DESC: EffectDesc = EffectDesc {
    key: "mul",
    name: "Times Two",
    params: &NO_PARAMS,
};
static MUL_FAMILY: FamilyDesc = FamilyDesc {
    key: "mul",
    name: "Times Two",
    pedals: &[&MUL_DESC],
};

/// Adds a 0.1 step — probe values stay under the output stage's
/// always-on safety ceiling (-0.3 dBFS).
struct AddOne;
impl Effect for AddOne {
    fn family(&self) -> &'static FamilyDesc {
        &ADD_FAMILY
    }
    fn prepare(&mut self, _: u32) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: usize, _: f32) {}
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x += 0.1;
        }
    }
}

struct TimesTwo;
impl Effect for TimesTwo {
    fn family(&self) -> &'static FamilyDesc {
        &MUL_FAMILY
    }
    fn prepare(&mut self, _: u32) {}
    fn reset(&mut self) {}
    fn set_param(&mut self, _: usize, _: f32) {}
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for x in left.iter_mut().chain(right.iter_mut()) {
            *x *= 2.0;
        }
    }
}

fn probe_chain() -> (lh_engine::Chain, lh_engine::ChainHandle) {
    let (mut chain, handle) = build_chain(vec![Box::new(AddOne), Box::new(TimesTwo)]);
    chain.prepare(SR);
    (chain, handle)
}

/// Feed DC 0.1 and return the settled output value.
fn settled_value(chain: &mut lh_engine::Chain) -> f32 {
    let mut last = 0.0;
    for _ in 0..200 {
        // 200 × 64 ≈ 267 ms — enough for any fade to finish
        let mut block = [0.1f32; 64];
        let mut block_r = [0.1f32; 64];
        chain.process(&mut block, &mut block_r);
        assert_eq!(block, block_r, "identical inputs stay identical");
        last = block[63];
    }
    last
}

#[test]
fn reorder_changes_processing_order() {
    let (mut chain, mut handle) = probe_chain();
    assert_eq!(handle.order_handles(), ["add", "mul"]);
    assert!((settled_value(&mut chain) - 0.4).abs() < 1e-4, "(.1+.1)*2");

    handle.set_order(&["mul", "add"]).unwrap();
    assert_eq!(handle.order_handles(), ["mul", "add"]);
    assert!((settled_value(&mut chain) - 0.3).abs() < 1e-4, ".1*2+.1");
}

#[test]
fn install_and_remove_slots_mid_stream() {
    // PRD 002: structure edits while the stream runs — one fade, correct
    // math on the other side, retired effects die on the control thread.
    let (mut chain, mut handle) = probe_chain();
    assert!((settled_value(&mut chain) - 0.4).abs() < 1e-4, "(.1+.1)*2");

    // Append a second AddOne: (.1+.1)*2 + .1 = .5. Duplicate family → "add2".
    let installed = handle.install_slot(Box::new(AddOne), usize::MAX).unwrap();
    assert_eq!(installed, "add2");
    assert_eq!(handle.order_handles(), ["add", "mul", "add2"]);
    assert!(
        (settled_value(&mut chain) - 0.5).abs() < 1e-4,
        "(.1+.1)*2+.1"
    );

    // Insert a third AddOne up front: ((.1+.1)+.1)*2 + .1 = .7. The new first
    // instance takes the plain handle; the others shift ranks.
    let installed = handle.install_slot(Box::new(AddOne), 0).unwrap();
    assert_eq!(installed, "add");
    assert_eq!(handle.order_handles(), ["add", "add2", "mul", "add3"]);
    assert!((settled_value(&mut chain) - 0.7).abs() < 1e-4);

    // Remove the multiplier: .1+.1+.1+.1 = .4.
    handle.remove_slot("mul").unwrap();
    assert_eq!(handle.order_handles(), ["add", "add2", "add3"]);
    assert!((settled_value(&mut chain) - 0.4).abs() < 1e-4);
    assert!(
        handle.set_param("mul", "x", 0.0).is_err(),
        "removed slots are unaddressable"
    );

    // The removed effect came back down the chute to die here.
    assert_eq!(handle.collect_garbage(), 1);
}

#[test]
fn structure_edits_fade_through_silence() {
    let (mut chain, mut handle) = probe_chain();
    settled_value(&mut chain); // steady 4.0

    handle.install_slot(Box::new(AddOne), 0).unwrap();
    let mut out = Vec::new();
    for _ in 0..200 {
        let mut block = [0.1f32; 64];
        let mut block_r = [0.1f32; 64];
        chain.process(&mut block, &mut block_r);
        out.extend_from_slice(&block);
    }
    let dip = out.iter().fold(f32::INFINITY, |m, v| m.min(v.abs()));
    assert!(dip < 0.005, "install must pass near silence, dip {dip}");
    let max_step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.025, "no hard switch, step {max_step}");
    assert!(
        (out.last().unwrap() - 0.6).abs() < 1e-4,
        "lands on ((.1+.1)+.1)*2"
    );
}

#[test]
fn chain_capacity_is_enforced() {
    let (_chain, mut handle) = probe_chain();
    for _ in 0..(lh_engine::MAX_SLOTS - 2) {
        handle.install_slot(Box::new(AddOne), usize::MAX).unwrap();
    }
    assert!(handle.is_full());
    match handle.install_slot(Box::new(AddOne), 0) {
        Err(lh_engine::EngineError::ChainFull) => {}
        other => panic!("expected ChainFull, got {other:?}"),
    }
}

#[test]
fn reorder_fades_through_silence_without_clicks() {
    let (mut chain, mut handle) = probe_chain();
    settled_value(&mut chain); // steady 4.0

    handle.set_order(&["mul", "add"]).unwrap();
    let mut out = Vec::new();
    for _ in 0..200 {
        let mut block = [0.1f32; 64];
        let mut block_r = [0.1f32; 64];
        chain.process(&mut block, &mut block_r);
        out.extend_from_slice(&block);
    }
    let dip = out.iter().fold(f32::INFINITY, |m, v| m.min(v.abs()));
    assert!(dip < 0.005, "must pass near silence, dip {dip}");
    let max_step = out
        .windows(2)
        .map(|w| (w[1] - w[0]).abs())
        .fold(0.0f32, f32::max);
    assert!(max_step < 0.02, "no hard switch, step {max_step}");
    assert!(
        (out.last().unwrap() - 0.3).abs() < 1e-4,
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
    handle.set_param("drive", "drive", 7.5).unwrap(); // ts9 knob
    handle.select_pedal("drive", "bd2").unwrap();
    handle.set_param("drive", "gain", 6.0).unwrap(); // bd2 knob
    handle.set_param("delay", "time", 500.0).unwrap();
    handle.set_active("gate", false).unwrap();
    handle.set_order(&["drive", "gate", "delay"]).unwrap();

    let saved = handle.snapshot_chain();
    assert_eq!(saved[0].key, "drive");
    assert_eq!(saved[0].pedal.as_deref(), Some("bd2"));
    assert_eq!(saved[0].pedals["bd2"]["gain"], 6.0);
    assert_eq!(saved[0].pedals["ts9"]["drive"], 7.5, "knob memory saved");
    assert!(!saved[1].active);

    // A fresh chain of the same pedals, restored from the snapshot.
    let (mut chain2, mut handle2) = build_chain(pedalboard());
    chain2.prepare(SR);
    let warnings = handle2
        .apply_preset_chain(&saved, false, &mut |_| None)
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(handle2.snapshot_chain(), saved);
}

#[test]
fn preset_apply_rebuilds_structure() {
    use lh_core::preset::SlotState;

    // PRD 002: the preset defines the structure — duplicates are built via
    // the factory, leftovers are removed, survivors keep their state.
    let (mut chain, mut handle) = probe_chain(); // add, mul
    settled_value(&mut chain);

    let states: Vec<SlotState> = vec![
        SlotState {
            key: "mul".into(),
            ..Default::default()
        },
        SlotState {
            key: "add".into(),
            ..Default::default()
        },
        SlotState {
            key: "add".into(),
            ..Default::default()
        },
    ];
    let mut built = 0;
    let warnings = handle
        .apply_preset_chain(&states, false, &mut |key| {
            (key == "add").then(|| {
                built += 1;
                Box::new(AddOne) as Box<dyn Effect>
            })
        })
        .unwrap();
    assert!(warnings.is_empty(), "{warnings:?}");
    assert_eq!(built, 1, "one new instance built, two claimed");
    assert_eq!(handle.order_handles(), ["mul", "add", "add2"]);
    // DC .1: .1*2 = .2, +.1, +.1 = .4.
    assert!((settled_value(&mut chain) - 0.4).abs() < 1e-4);
}

#[test]
fn pedal_switch_restores_each_pedals_knobs() {
    // PRD 001 acceptance: tweak ts9, switch to evva, tweak, switch back —
    // every pedal keeps its own values.
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.set_param("drive", "drive", 7.0).unwrap();
    handle.select_pedal("drive", "evva").unwrap();
    assert_eq!(handle.active_pedal("drive").unwrap(), "evva");
    handle.set_param("drive", "gain", 3.0).unwrap();
    assert!(
        handle.set_param("drive", "drive", 1.0).is_err(),
        "ts9's knob is not addressable while evva is selected"
    );
    handle.select_pedal("drive", "ts9").unwrap();

    let snap = handle.snapshot_chain();
    let drive = snap.iter().find(|s| s.key == "drive").unwrap();
    assert_eq!(drive.pedal.as_deref(), Some("ts9"));
    assert_eq!(drive.pedals["ts9"]["drive"], 7.0, "ts9 kept its knob");
    assert_eq!(drive.pedals["evva"]["gain"], 3.0, "evva kept its knob");

    // The engine drains the whole switch+restore burst without trouble.
    let mut l = [0.0f32; 64];
    let mut r = [0.0f32; 64];
    chain.process(&mut l, &mut r);
    assert_finite("post-switch block", &l);
}

#[test]
fn pedal_selector_aliases_and_cc_norms() {
    let (_chain, mut handle) = build_chain(pedalboard());
    // `model` is the pre-v3 alias for `pedal` on any slot.
    let applied = handle.set_param("drive", "model", 1.0).unwrap();
    assert_eq!(applied.real, 1.0);
    assert_eq!(handle.active_pedal("drive").unwrap(), "bd2");
    // A CC at full deflection lands on the last pedal — whatever the
    // (append-only) registry's newest entry is.
    let last = lh_dsp::drive::FAMILY.pedals.last().unwrap().key;
    handle.select_pedal_norm("drive", 1.0).unwrap();
    assert_eq!(handle.active_pedal("drive").unwrap(), last);
    assert!(handle.select_pedal("drive", "wah").is_err());
    // Display names resolve too.
    handle.select_pedal("drive", "Blues Driver").unwrap();
    assert_eq!(handle.active_pedal("drive").unwrap(), "bd2");
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
    let mut yr = x.clone();
    for (l, r) in y.chunks_mut(64).zip(yr.chunks_mut(64)) {
        chain.process(l, r);
    }
    let tapped: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
    assert_eq!(tapped, x, "tap is the pre-processing input");

    // Unread tap fills up: processing must go on, new samples are dropped.
    let mut z = sine(SR, 220.0, 3 * 4_096);
    let mut zr = z.clone();
    chain.process(&mut z, &mut zr);
    assert_finite("output with full tap", &z);
    assert_eq!(consumer.slots(), 4_096, "ring capped, nothing blocked");

    // A vanished consumer (tuner closed) must not disturb the audio path.
    drop(consumer);
    let mut w = sine(SR, 220.0, 512);
    let mut wr = w.clone();
    chain.process(&mut w, &mut wr);
    assert_finite("output after consumer dropped", &w);
}

#[test]
fn output_stage_eq_and_safety_limiter() {
    use lh_core::global_eq::{Band, BandKind};

    // An empty chain is a passthrough into the output stage.
    let (mut chain, mut handle) = build_chain(vec![]);
    chain.prepare(SR);

    let render = |chain: &mut lh_engine::Chain, amp: f32| {
        let mut l: Vec<f32> = sine(SR, 220.0, SR as usize / 2)
            .iter()
            .map(|s| s * amp)
            .collect();
        let mut r = l.clone();
        for (cl, cr) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
            chain.process(cl, cr);
        }
        l
    };

    // Transparent by default (safety only touches overs).
    let y = render(&mut chain, 0.5);
    assert_finite("output stage", &y);
    let base = rms(&y[y.len() / 2..]);
    assert!(
        (base - 0.5 / 2f32.sqrt()).abs() < 1e-3,
        "transparent: {base}"
    );

    // A +12 dB bell at the test frequency is audible after the messages land.
    handle
        .set_eq_band(
            2,
            Band {
                enabled: true,
                kind: BandKind::Bell,
                freq: 220.0,
                gain_db: 12.0,
                q: 0.7,
            },
        )
        .unwrap();
    let boosted = rms(&render(&mut chain, 0.1)[SR as usize / 4..]);
    let flat = 0.1 / 2f32.sqrt();
    assert!(
        boosted > 2.5 * flat,
        "EQ boost must land: {boosted} vs flat {flat}"
    );

    // The safety limiter caps everything near -0.3 dBFS — even with the
    // EQ still boosting +12 dB into it.
    let hot = render(&mut chain, 2.0);
    let peak_out = hot[hot.len() / 2..]
        .iter()
        .fold(0.0f32, |m, s| m.max(s.abs()));
    assert!(peak_out <= 1.0, "safety ceiling must hold, peak {peak_out}");

    // Master off: bit-transparent again after the crossfade settles (the
    // reset clears the safety limiter's recovery from the hot render).
    handle.set_eq_active(false).unwrap();
    let _settle = render(&mut chain, 0.1);
    chain.reset();
    let x: Vec<f32> = sine(SR, 220.0, 4_096).iter().map(|s| s * 0.5).collect();
    let mut l = x.clone();
    let mut r = x.clone();
    for (cl, cr) in l.chunks_mut(64).zip(r.chunks_mut(64)) {
        chain.process(cl, cr);
    }
    assert_eq!(x, l, "disabled global EQ must be bit-exact");
}

#[test]
fn output_tap_carries_the_post_stage_mono_sum() {
    let (mut chain, _handle) = build_chain(vec![]);
    chain.prepare(SR);
    let (producer, mut consumer) = rtrb::RingBuffer::<f32>::new(8_192);
    chain.set_output_tap(producer);

    let x = sine(SR, 220.0, 1_024);
    let mut l = x.clone();
    let mut r = x.clone();
    chain.process(&mut l, &mut r);
    let tapped: Vec<f32> = std::iter::from_fn(|| consumer.pop().ok()).collect();
    assert_eq!(tapped.len(), 1_024);
    // Identical channels ⇒ the mono sum is the processed signal itself.
    assert_eq!(tapped, l, "tap sees what leaves the output stage");

    // A full tap never blocks processing.
    let mut big = sine(SR, 220.0, 3 * 8_192);
    let mut big_r = big.clone();
    chain.process(&mut big, &mut big_r);
    assert_finite("output with full tap", &big);
}

#[test]
fn preset_apply_is_forward_compatible() {
    use lh_core::preset::SlotState;
    use std::collections::BTreeMap;

    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);

    // A preset from "the future": unknown slot, unknown param, and it
    // doesn't mention the delay at all. Flat (pre-v3) params apply to the
    // selected pedal.
    let chain_states = vec![
        SlotState {
            key: "drive".into(),
            params: BTreeMap::from([("drive".into(), 8.0), ("sparkle".into(), 1.0)]),
            ..Default::default()
        },
        SlotState {
            key: "wah".into(),
            ..Default::default()
        },
        SlotState {
            key: "gate".into(),
            active: false,
            ..Default::default()
        },
    ];
    let warnings = handle
        .apply_preset_chain(&chain_states, false, &mut |_| None)
        .unwrap();
    assert_eq!(warnings.len(), 3, "{warnings:?}"); // sparkle, wah, delay

    let now = handle.snapshot_chain();
    assert_eq!(now.len(), 2, "unmentioned delay is removed (PRD 002)");
    assert_eq!(now[0].key, "drive");
    assert_eq!(now[0].pedals["ts9"]["drive"], 8.0);
    assert_eq!(now[1].key, "gate");
}

// --- spillover (PRD 010) --------------------------------------------------

static TAIL_DESC: EffectDesc = EffectDesc {
    key: "tail",
    name: "Tail",
    params: &NO_PARAMS,
};
static TAIL_FAMILY: FamilyDesc = FamilyDesc {
    key: "tail",
    name: "Tail",
    pedals: &[&TAIL_DESC],
};

/// A deterministic tail generator for spill tests: a pure feedback resonator
/// (`y = x + fb·y⁻¹`). Charged with an impulse, on silence it rings out at
/// `fb` per sample. `fb = 1.0` sustains forever (a stand-in for a self-
/// oscillating feedback delay) — the spill lane's forced decay must cap it.
struct Tail {
    l: f32,
    r: f32,
    fb: f32,
}

impl Tail {
    fn new(fb: f32) -> Self {
        Self { l: 0.0, r: 0.0, fb }
    }
}

impl Effect for Tail {
    fn family(&self) -> &'static FamilyDesc {
        &TAIL_FAMILY
    }
    fn prepare(&mut self, _: u32) {}
    fn reset(&mut self) {
        self.l = 0.0;
        self.r = 0.0;
    }
    fn set_param(&mut self, _: usize, _: f32) {}
    fn tail_seconds(&self) -> f32 {
        5.0
    }
    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (l, r) in left.iter_mut().zip(right.iter_mut()) {
            self.l = *l + self.l * self.fb;
            self.r = *r + self.r * self.fb;
            *l = self.l;
            *r = self.r;
        }
    }
}

/// Run `blocks` × 64 frames of silence; assert every sample stays finite and
/// return the peak of the **final** block (the tail's current level).
fn drain_silence(chain: &mut lh_engine::Chain, blocks: usize) -> f32 {
    let mut last = 0.0f32;
    for _ in 0..blocks {
        let mut l = [0.0f32; 64];
        let mut r = [0.0f32; 64];
        chain.process(&mut l, &mut r);
        last = 0.0;
        for s in l.iter().chain(r.iter()) {
            assert!(s.is_finite(), "spill output must stay finite");
            last = last.max(s.abs());
        }
    }
    last
}

/// Charge the chain's Tail with a single impulse so it has something to ring.
fn charge(chain: &mut lh_engine::Chain) {
    let mut l = [0.0f32; 64];
    let mut r = [0.0f32; 64];
    l[0] = 0.5;
    r[0] = 0.5;
    chain.process(&mut l, &mut r);
}

#[test]
fn spilled_tail_rings_on_then_evicts() {
    let (mut chain, mut handle) = build_chain(vec![Box::new(Tail::new(0.999))]);
    chain.prepare(SR);
    assert!(handle.slot_has_tail("tail"), "tail hint routes the spill");

    charge(&mut chain);
    handle.spill_slot("tail").unwrap();
    assert!(handle.order_handles().is_empty(), "slot left the chain");

    // Right after the spill the lane is still ringing on silence.
    let early = drain_silence(&mut chain, 4);
    assert!(
        early > 1e-3,
        "tail must keep sounding after the slot left: {early}"
    );

    // Given enough silence (fb 0.999 ≈ 21 ms τ → ~0.17 s to the floor, plus
    // the 0.25 s hold) the tail decays out and the lane retires the effect.
    let late = drain_silence(&mut chain, 500); // ~0.67 s
    assert!(late < early, "tail must decay: {late} vs {early}");
    assert_eq!(handle.collect_garbage(), 1, "the spent tail was retired");
}

#[test]
fn spillover_off_path_cuts_the_tail() {
    // The hard-remove path (what the session uses with spillover off) drops
    // the slot at the fade bottom — no lingering tail in a lane.
    let (mut chain, mut handle) = build_chain(vec![Box::new(Tail::new(0.999))]);
    chain.prepare(SR);
    charge(&mut chain);
    handle.remove_slot("tail").unwrap();
    // Once the master fade and the deferred removal settle, output is silent
    // — the tail was cut, not left ringing in a lane.
    let peak = drain_silence(&mut chain, 50);
    assert!(
        peak < 1e-3,
        "hard remove must not leave a ringing tail: {peak}"
    );
    assert_eq!(handle.collect_garbage(), 1, "removed effect retired");
}

#[test]
fn forced_decay_caps_a_self_oscillating_tail() {
    // fb = 1.0 never decays on its own (a bounded self-oscillation). The
    // lane must let it ring through the 8 s grace, then force it down.
    let (mut chain, mut handle) = build_chain(vec![Box::new(Tail::new(1.0))]);
    chain.prepare(SR);
    charge(&mut chain);
    handle.spill_slot("tail").unwrap();

    let bps = SR as usize / 64;
    // Through the grace (~7 s): still sustaining, undecayed.
    let before = drain_silence(&mut chain, bps * 7);
    assert!(
        before > 0.1,
        "must ring undecayed through the grace: {before}"
    );

    // Past the 8 s grace (out to ~12 s): forced −12 dB/s has pulled it down.
    let after = drain_silence(&mut chain, bps * 5);
    assert!(
        after < before * 0.3,
        "forced decay must pull a non-decaying tail down: {after} vs {before}"
    );
}

#[test]
fn lane_exhaustion_retires_the_oldest() {
    // Five tailed slots, four lanes: spilling the fifth evicts one to fit.
    let effects: Vec<Box<dyn Effect>> = (0..5).map(|_| Box::new(Tail::new(0.999)) as _).collect();
    let (mut chain, mut handle) = build_chain(effects);
    chain.prepare(SR);
    charge(&mut chain);

    assert_eq!(handle.order_handles().len(), 5);
    // Each spill removes the front slot, so the next becomes "tail" — spill
    // the front five times to drain the chain into the lanes.
    for _ in 0..5 {
        handle.spill_slot("tail").unwrap();
    }
    assert!(handle.order_handles().is_empty());
    // One process call applies all five SpillSlot messages; the fifth found
    // every lane full and retired one to make room.
    drain_silence(&mut chain, 1);
    assert_eq!(
        handle.collect_garbage(),
        1,
        "one of five spills was evicted to make room for the rest"
    );
}

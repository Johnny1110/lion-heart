//! The profiler has to answer a human question — *which pedal is slow?* — so
//! the numbers must come back attached to the handle a user actually types.
//!
//! `profile_alloc.rs` proves the measurement is real-time safe. This proves the
//! reporting join: raw slot-table indices → chain order → pedal handles, ranked
//! worst first, with removed and bypassed slots handled sanely.

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::testutil::sine;
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

/// Run enough audio through the chain for every slot to register a cost.
fn run_blocks(chain: &mut lh_engine::Chain, blocks: usize) {
    let mut left = sine(SR, 220.0, 256);
    let mut right = left.clone();
    for _ in 0..blocks {
        chain.process(&mut left, &mut right);
    }
}

fn handles(report: &[(String, lh_engine::profile::SlotTiming)]) -> Vec<&str> {
    report.iter().map(|(h, _)| h.as_str()).collect()
}

#[test]
fn report_names_every_pedal_in_the_chain() {
    let (mut chain, handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);
    run_blocks(&mut chain, 8);

    let report = handle.profile_report();
    assert_eq!(report.len(), 3, "one row per slot in the chain");

    let mut names = handles(&report);
    names.sort_unstable();
    assert_eq!(names, vec!["delay", "drive", "gate"]);
}

#[test]
fn report_ranks_the_most_expensive_pedal_first() {
    let (mut chain, handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);
    run_blocks(&mut chain, 32);

    let report = handle.profile_report();
    let costs: Vec<u64> = report.iter().map(|(_, t)| t.last_nanos).collect();
    assert!(
        costs.windows(2).all(|w| w[0] >= w[1]),
        "rows must descend by cost, got {costs:?}"
    );
    assert!(
        costs.iter().any(|&c| c > 0),
        "a real chain must cost measurable time"
    );
}

#[test]
fn repeated_families_get_distinct_handles() {
    // Two drives in one chain: the second must report as `drive2`, matching
    // the handle the REPL accepts — otherwise the table is ambiguous.
    let board: Vec<Box<dyn Effect>> = vec![
        Box::new(Drive::new()),
        Box::new(Delay::new()),
        Box::new(Drive::new()),
    ];
    let (mut chain, handle) = build_chain(board);
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);
    run_blocks(&mut chain, 8);

    let report = handle.profile_report();
    let mut names = handles(&report);
    names.sort_unstable();
    assert_eq!(names, vec!["delay", "drive", "drive2"]);
}

#[test]
fn a_bypassed_pedal_reports_no_cost_but_keeps_its_row() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);

    let drive = handle.order_handles()[1].clone();
    handle.set_active(&drive, false).expect("bypass drive");
    // Long enough for the bypass crossfade to settle into the skip fast path.
    run_blocks(&mut chain, 64);

    let report = handle.profile_report();
    let (_, timing) = report
        .iter()
        .find(|(h, _)| *h == drive)
        .expect("a bypassed pedal is still part of the chain");
    assert_eq!(
        timing.last_nanos, 0,
        "a fully bypassed slot is skipped, so it costs nothing"
    );
    assert_eq!(report.len(), 3, "bypass must not drop the row");
}

#[test]
fn a_removed_pedal_leaves_the_report() {
    let (mut chain, mut handle) = build_chain(pedalboard());
    chain.prepare(SR);
    handle.telemetry().profile().set_enabled(true);
    run_blocks(&mut chain, 8);

    let drive = handle.order_handles()[1].clone();
    handle.remove_slot(&drive).expect("remove drive");
    run_blocks(&mut chain, 64);

    let report = handle.profile_report();
    assert_eq!(report.len(), 2, "the removed slot is gone from the chain");
    assert!(
        !handles(&report).contains(&drive.as_str()),
        "a removed pedal must not linger in the table"
    );
}

#[test]
fn report_is_empty_before_any_audio_and_does_not_panic() {
    let (_chain, handle) = build_chain(pedalboard());
    // Never processed, never enabled — the table should still render.
    let report: Vec<(String, lh_engine::profile::SlotTiming)> = handle.profile_report();
    assert_eq!(report.len(), 3);
    assert!(report.iter().all(|(_, t)| t.last_nanos == 0));
}

/// Guard the contract the REPL relies on: it reads the profiler through the
/// handle's shared telemetry, so toggling there must reach the audio side.
#[test]
fn enabling_through_the_handle_reaches_the_audio_thread() {
    let (mut chain, handle) = build_chain(pedalboard());
    chain.prepare(SR);

    run_blocks(&mut chain, 4);
    assert_eq!(
        handle.telemetry().profile().snapshot().blocks,
        0,
        "off by default: the audio thread must not record"
    );

    handle.telemetry().profile().set_enabled(true);
    run_blocks(&mut chain, 4);
    assert_eq!(
        handle.telemetry().profile().snapshot().blocks,
        4,
        "the audio thread must observe the handle-side toggle"
    );

    handle.telemetry().profile().set_enabled(false);
    run_blocks(&mut chain, 4);
    assert_eq!(
        handle.telemetry().profile().snapshot().blocks,
        4,
        "switching off must stop recording again"
    );
}

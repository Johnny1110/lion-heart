//! Recording taps (PRD 014): the DI tap copies the raw input at chain entry,
//! the wet tap copies the processed output after the output stage. Both are
//! stereo interleaved, drop-on-full, and dormant until armed.

use std::sync::Arc;

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_engine::{RecordTapState, build_chain};

const SR: u32 = 48_000;

/// Drain a consumer into a flat Vec (interleaved L,R,L,R…).
fn drain(cons: &mut rtrb::Consumer<f32>) -> Vec<f32> {
    let mut out = Vec::new();
    while let Ok(v) = cons.pop() {
        out.push(v);
    }
    out
}

#[test]
fn di_captures_raw_input_wet_captures_processed_output() {
    // A drive pedal changes the signal, so DI (pre-chain) and wet (post-chain)
    // must differ — proving the two taps sit at the right points.
    let (mut chain, mut handle) = build_chain(vec![Box::new(Drive::new()) as Box<dyn Effect>]);
    chain.prepare(SR);
    handle.set_param("drive", "drive", 9.0).unwrap();
    handle.set_param("drive", "level", 8.0).unwrap();

    let (di_p, mut di_c) = rtrb::RingBuffer::new(8_192);
    let (wet_p, mut wet_c) = rtrb::RingBuffer::new(8_192);
    let di_s = Arc::new(RecordTapState::default());
    let wet_s = Arc::new(RecordTapState::default());
    chain.set_record_taps(di_p, Arc::clone(&di_s), wet_p, Arc::clone(&wet_s));

    // Settle the param changes with the taps still dormant (disarmed).
    for _ in 0..64 {
        let mut l = [0.0f32; 64];
        let mut r = [0.0f32; 64];
        chain.process(&mut l, &mut r);
    }
    assert!(di_c.is_empty(), "disarmed DI tap must not write");
    assert!(wet_c.is_empty(), "disarmed wet tap must not write");

    di_s.armed.store(true, std::sync::atomic::Ordering::Relaxed);
    wet_s
        .armed
        .store(true, std::sync::atomic::Ordering::Relaxed);

    // Distinct L/R so interleaving and stereo routing are both checked.
    const N: usize = 256;
    let mut left: Vec<f32> = (0..N).map(|i| (i as f32 * 0.05).sin() * 0.4).collect();
    let mut right: Vec<f32> = (0..N).map(|i| (i as f32 * 0.05).cos() * 0.3).collect();
    let in_l = left.clone();
    let in_r = right.clone();
    chain.process(&mut left, &mut right);
    // After process, left/right hold the wet output.
    let out_l = left.clone();
    let out_r = right.clone();

    let di = drain(&mut di_c);
    let wet = drain(&mut wet_c);
    assert_eq!(
        di.len(),
        2 * N,
        "DI captured one stereo frame per input frame"
    );
    assert_eq!(wet.len(), 2 * N);

    for i in 0..N {
        assert_eq!(di[2 * i], in_l[i], "DI L must be the untouched input");
        assert_eq!(di[2 * i + 1], in_r[i], "DI R must be the untouched input");
        assert_eq!(wet[2 * i], out_l[i], "wet L must be the processed output");
        assert_eq!(wet[2 * i + 1], out_r[i]);
    }

    // The drive must actually have changed the signal.
    let changed: f32 = (0..N).map(|i| (di[2 * i] - wet[2 * i]).abs()).sum();
    assert!(changed > 0.01, "processed wet must differ from the dry DI");
}

#[test]
fn ring_backpressure_counts_dropped_frames() {
    // A tiny DI ring cannot hold a whole block: the shortfall is counted, and
    // the callback still returns (never blocks).
    let (mut chain, _handle) = build_chain(vec![]);
    chain.prepare(SR);

    let (di_p, mut di_c) = rtrb::RingBuffer::new(4); // 2 stereo frames
    let (wet_p, mut wet_c) = rtrb::RingBuffer::new(8_192);
    let di_s = Arc::new(RecordTapState::default());
    let wet_s = Arc::new(RecordTapState::default());
    chain.set_record_taps(di_p, Arc::clone(&di_s), wet_p, Arc::clone(&wet_s));
    di_s.armed.store(true, std::sync::atomic::Ordering::Relaxed);
    wet_s
        .armed
        .store(true, std::sync::atomic::Ordering::Relaxed);

    const N: usize = 100;
    let mut l = vec![0.2f32; N];
    let mut r = vec![0.2f32; N];
    chain.process(&mut l, &mut r);

    let captured = drain(&mut di_c).len() / 2; // whole frames the ring took
    let dropped = di_s.dropped.load(std::sync::atomic::Ordering::Relaxed) as usize;
    assert!(captured > 0, "some frames fit");
    assert!(dropped > 0, "the rest were counted as dropped");
    assert_eq!(
        captured + dropped,
        N,
        "every input frame is captured or counted"
    );
    // The wet ring was roomy, so it took the whole block with no drops.
    assert_eq!(drain(&mut wet_c).len(), 2 * N);
    assert_eq!(wet_s.dropped.load(std::sync::atomic::Ordering::Relaxed), 0);
}

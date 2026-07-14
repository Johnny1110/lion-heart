//! Per-block cost of each effect at the target live format: 48 kHz, 64-frame
//! blocks (1.33 ms budget per block — white paper §3.2).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use lh_dsp::Effect;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::gate::NoiseGate;

const SR: u32 = 48_000;
const BLOCK: usize = 64;

fn signal() -> Vec<f32> {
    lh_dsp::testutil::sine(SR, 220.0, BLOCK)
}

fn bench_effects(c: &mut Criterion) {
    let mut group = c.benchmark_group("block64_48k");

    let mut gate = NoiseGate::new();
    gate.prepare(SR);
    let mut buf = signal();
    group.bench_function("gate", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            gate.process(black_box(&mut buf));
        })
    });

    let mut drive = Drive::new();
    drive.prepare(SR);
    group.bench_function("drive_4x_oversampled", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            drive.process(black_box(&mut buf));
        })
    });

    let mut delay = Delay::new();
    delay.prepare(SR);
    group.bench_function("delay", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            delay.process(black_box(&mut buf));
        })
    });

    let mut g2 = NoiseGate::new();
    let mut d2 = Drive::new();
    let mut dl2 = Delay::new();
    g2.prepare(SR);
    d2.prepare(SR);
    dl2.prepare(SR);
    group.bench_function("gate_drive_delay", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            g2.process(black_box(&mut buf));
            d2.process(black_box(&mut buf));
            dl2.process(black_box(&mut buf));
        })
    });

    group.finish();
}

criterion_group!(benches, bench_effects);
criterion_main!(benches);

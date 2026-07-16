//! Per-block cost of each effect at the target live format: 48 kHz, 64-frame
//! blocks (1.33 ms budget per block — white paper §3.2).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use lh_dsp::Effect;
use lh_dsp::comp::Compressor;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::eq::Eq;
use lh_dsp::gate::NoiseGate;
use lh_dsp::modulation::{Modulation, TYPES};
use lh_dsp::reverb::Reverb;

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

    // Cab with a realistic 100 ms IR (4800 taps at 48 kHz, 128-sample partitions).
    let (mut cab, mut cab_handle) = lh_dsp::cab::CabIr::new();
    cab.prepare(SR);
    let ir: Vec<f32> = (0..4_800)
        .map(|n| {
            let env = (-(n as f32) / (SR as f32 * 0.02)).exp();
            ((n as f32 * 12.9898).sin() * 43_758.547).fract() * env
        })
        .collect();
    let mut convolver = fft_convolver::FFTConvolver::<f32>::default();
    convolver.init(128, &ir).unwrap();
    cab_handle
        .install(Box::new(lh_dsp::cab::IrAsset { convolver }))
        .unwrap();
    group.bench_function("cab_ir_100ms", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            cab.process(black_box(&mut buf));
        })
    });

    let mut comp = Compressor::new();
    comp.prepare(SR);
    group.bench_function("comp", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            comp.process(black_box(&mut buf));
        })
    });

    let mut eq = Eq::new();
    eq.prepare(SR);
    group.bench_function("eq_3band", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            eq.process(black_box(&mut buf));
        })
    });

    for (index, name) in TYPES.iter().enumerate() {
        let mut modulation = Modulation::new();
        modulation.prepare(SR);
        modulation.set_param(0, index as f32 / (TYPES.len() - 1) as f32);
        group.bench_function(format!("mod_{name}"), |b| {
            b.iter(|| {
                buf.copy_from_slice(&signal());
                modulation.process(black_box(&mut buf));
            })
        });
    }

    let mut reverb = Reverb::new();
    reverb.prepare(SR);
    group.bench_function("reverb_fdn8", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            reverb.process(black_box(&mut buf));
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

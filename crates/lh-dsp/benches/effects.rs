//! Per-block cost of each effect at the target live format: 48 kHz, 64-frame
//! blocks (1.33 ms budget per block — white paper §3.2).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use lh_dsp::Effect;
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::Compressor;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::eq::Eq;
use lh_dsp::modulation::Modulation;
use lh_dsp::time::Delay;
use lh_dsp::time::Reverb;

const SR: u32 = 48_000;
const BLOCK: usize = 64;

fn signal() -> Vec<f32> {
    lh_dsp::testutil::sine(SR, 220.0, BLOCK)
}

/// Refill both channels and run one stereo process call.
macro_rules! bench_stereo {
    ($group:expr, $name:expr, $effect:expr, $buf_l:expr, $buf_r:expr) => {
        $group.bench_function($name, |b| {
            b.iter(|| {
                $buf_l.copy_from_slice(&signal());
                $buf_r.copy_from_slice(&signal());
                $effect.process(black_box(&mut $buf_l), black_box(&mut $buf_r));
            })
        });
    };
}

fn bench_effects(c: &mut Criterion) {
    let mut group = c.benchmark_group("block64_48k");

    let mut buf = signal();
    let mut buf_r = signal();

    let mut gate = NoiseGate::new();
    gate.prepare(SR);
    bench_stereo!(group, "gate", gate, buf, buf_r);

    for (index, pedal) in lh_dsp::drive::FAMILY.pedals.iter().enumerate() {
        let mut drive = Drive::new();
        drive.prepare(SR);
        drive.select_pedal(index);
        bench_stereo!(
            group,
            format!("drive_{}_4x_oversampled", pedal.key),
            drive,
            buf,
            buf_r
        );
    }

    for (index, pedal) in lh_dsp::time::delay::FAMILY.pedals.iter().enumerate() {
        let mut delay = Delay::new();
        delay.prepare(SR);
        delay.select_pedal(index);
        bench_stereo!(group, format!("delay_{}", pedal.key), delay, buf, buf_r);
    }

    // Cab with a realistic 100 ms IR (4800 taps at 48 kHz, 128-sample partitions).
    let (mut cab, mut cab_handle) = lh_dsp::cab::CabIr::new();
    cab.prepare(SR);
    let ir: Vec<f32> = (0..4_800)
        .map(|n| {
            let env = (-(n as f32) / (SR as f32 * 0.02)).exp();
            ((n as f32 * 12.9898).sin() * 43_758.547).fract() * env
        })
        .collect();
    let build = || {
        let mut convolver = fft_convolver::FFTConvolver::<f32>::default();
        convolver.init(128, &ir).unwrap();
        convolver
    };
    cab_handle
        .install(Box::new(lh_dsp::cab::IrAsset {
            a: lh_dsp::cab::IrPair {
                left: build(),
                right: build(),
            },
            b: None,
        }))
        .unwrap();
    bench_stereo!(group, "cab_ir_100ms", cab, buf, buf_r);

    let mut comp = Compressor::new();
    comp.prepare(SR);
    bench_stereo!(group, "comp", comp, buf, buf_r);

    for (index, pedal) in lh_dsp::filter::FAMILY.pedals.iter().enumerate() {
        let mut filter = lh_dsp::filter::Filter::new();
        filter.prepare(SR);
        filter.select_pedal(index);
        bench_stereo!(group, format!("filter_{}", pedal.key), filter, buf, buf_r);
    }

    let mut eq = Eq::new();
    eq.prepare(SR);
    bench_stereo!(group, "eq_3band", eq, buf, buf_r);

    for (index, pedal) in lh_dsp::modulation::FAMILY.pedals.iter().enumerate() {
        let mut modulation = Modulation::new();
        modulation.prepare(SR);
        modulation.select_pedal(index);
        bench_stereo!(group, format!("mod_{}", pedal.key), modulation, buf, buf_r);
    }

    for (index, pedal) in lh_dsp::time::reverb::FAMILY.pedals.iter().enumerate() {
        let mut reverb = Reverb::new();
        reverb.prepare(SR);
        reverb.select_pedal(index);
        for (i, p) in pedal.params.iter().enumerate() {
            reverb.set_param(i, p.default_norm());
        }
        bench_stereo!(group, format!("reverb_{}", pedal.key), reverb, buf, buf_r);
    }

    // The always-on output stage EQ with a representative four bands live.
    let mut global_eq = lh_dsp::eq::global::GlobalEq::new();
    global_eq.prepare(SR);
    let mut state = lh_core::global_eq::GlobalEqState::default();
    for (i, freq) in [(0usize, 40.0), (2, 250.0), (5, 3_000.0), (7, 11_000.0)] {
        state.bands[i].enabled = true;
        state.bands[i].freq = freq;
        state.bands[i].gain_db = 4.0;
        global_eq.set_band(i, state.bands[i]);
    }
    bench_stereo!(group, "global_eq_4band", global_eq, buf, buf_r);

    group.finish();
}

/// The full hand-written M5 pedalboard (everything but NAM) at the live
/// 64-frame format and the M6 stage target of 32 frames, where per-block
/// overhead weighs double.
fn bench_full_chain(c: &mut Criterion) {
    let mut group = c.benchmark_group("full_chain_no_nam");
    for block in [64usize, 32] {
        let mut gate = NoiseGate::new();
        let mut comp = Compressor::new();
        let mut drive = Drive::new();
        let mut eq = Eq::new();
        let mut modulation = Modulation::new();
        let mut delay = Delay::new();
        let mut reverb = Reverb::new();
        let mut limiter = lh_dsp::dynamics::Limiter::new();
        let effects: [&mut dyn Effect; 8] = [
            &mut gate,
            &mut comp,
            &mut drive,
            &mut eq,
            &mut modulation,
            &mut delay,
            &mut reverb,
            &mut limiter,
        ];
        let mut effects = effects;
        for effect in effects.iter_mut() {
            effect.prepare(SR);
        }
        let signal = lh_dsp::testutil::sine(SR, 220.0, block);
        let mut buf = signal.clone();
        let mut buf_r = signal.clone();
        group.bench_function(format!("block{block}"), |b| {
            b.iter(|| {
                buf.copy_from_slice(&signal);
                buf_r.copy_from_slice(&signal);
                for effect in effects.iter_mut() {
                    effect.process(black_box(&mut buf), black_box(&mut buf_r));
                }
            })
        });
    }
    group.finish();
}

criterion_group!(benches, bench_effects, bench_full_chain);
criterion_main!(benches);

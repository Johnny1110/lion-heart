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
use lh_dsp::power::PowerAmp;
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

    // Power amp: 4× oversampled push-pull + sag, driven so the shaper is in
    // its nonlinear region (its worst case).
    let mut power = PowerAmp::new();
    power.prepare(SR);
    power.set_param(0, 0.8); // drive
    bench_stereo!(group, "power_4x_oversampled", power, buf, buf_r);

    for (index, pedal) in lh_dsp::time::delay::FAMILY.pedals.iter().enumerate() {
        let mut delay = Delay::new();
        delay.prepare(SR);
        delay.select_pedal(index);
        bench_stereo!(group, format!("delay_{}", pedal.key), delay, buf, buf_r);
    }

    // Looper (PRD 013): the three steady states — record (write), play (read
    // + seam fade), overdub (read + soft-clipped in-place write).
    {
        let rec_i = lh_dsp::looper::DESC.param_index("rec").unwrap();
        let press = |lp: &mut lh_dsp::looper::Looper| {
            lp.set_param(rec_i, 1.0);
            lp.set_param(rec_i, 0.0);
        };
        let warm = |lp: &mut lh_dsp::looper::Looper, n: usize| {
            let mut l = vec![0.2f32; n];
            let mut r = vec![0.2f32; n];
            lp.process(&mut l, &mut r);
        };

        let mut rec = lh_dsp::looper::Looper::new();
        rec.prepare(SR);
        press(&mut rec); // Empty -> Recording
        bench_stereo!(group, "looper_record", rec, buf, buf_r);

        let mut play = lh_dsp::looper::Looper::new();
        play.prepare(SR);
        press(&mut play);
        warm(&mut play, SR as usize / 4); // record 250 ms
        press(&mut play); // -> Playing
        bench_stereo!(group, "looper_play", play, buf, buf_r);

        let mut dub = lh_dsp::looper::Looper::new();
        dub.prepare(SR);
        press(&mut dub);
        warm(&mut dub, SR as usize / 4);
        press(&mut dub); // -> Playing
        press(&mut dub); // -> Overdubbing
        bench_stereo!(group, "looper_overdub", dub, buf, buf_r);
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

    for (index, pedal) in lh_dsp::dynamics::comp::FAMILY.pedals.iter().enumerate() {
        let mut comp = Compressor::new();
        comp.prepare(SR);
        comp.select_pedal(index);
        bench_stereo!(group, format!("comp_{}", pedal.key), comp, buf, buf_r);
    }

    for (index, pedal) in lh_dsp::filter::FAMILY.pedals.iter().enumerate() {
        let mut filter = lh_dsp::filter::Filter::new();
        filter.prepare(SR);
        filter.select_pedal(index);
        bench_stereo!(group, format!("filter_{}", pedal.key), filter, buf, buf_r);
    }

    let mut eq = Eq::new();
    eq.prepare(SR);
    bench_stereo!(group, "eq_3band", eq, buf, buf_r);

    // The parametric pedal with the same four representative bands live as
    // the output-stage bench below — settled, its cost must match.
    let mut para = Eq::new();
    para.prepare(SR);
    para.select_pedal(1);
    let desc = lh_dsp::eq::FAMILY.pedals[1];
    for (band, freq) in [(0usize, 40.0), (2, 250.0), (5, 3_000.0), (7, 11_000.0)] {
        let set = |eff: &mut Eq, key: &str, real: f32| {
            let i = desc.param_index(key).unwrap();
            eff.set_param(i, desc.params[i].range.to_norm(real));
        };
        set(&mut para, &format!("b{}_freq", band + 1), freq);
        set(&mut para, &format!("b{}_gain", band + 1), 4.0);
        set(&mut para, &format!("b{}_on", band + 1), 1.0);
    }
    bench_stereo!(group, "eq_parametric_4band", para, buf, buf_r);

    // The tone stack pedal (PRD 023) in its two states: settled knobs run only
    // the state space, while a moving knob re-solves the netlist and
    // re-discretises it once per 64-sample sub-block — the worst case.
    let mut stack = Eq::new();
    stack.prepare(SR);
    stack.select_pedal(2);
    bench_stereo!(group, "eq_tonestack_settled", stack, buf, buf_r);

    let mut sweeping = Eq::new();
    sweeping.prepare(SR);
    sweeping.select_pedal(2);
    let bass = lh_dsp::eq::FAMILY.pedals[2].param_index("bass").unwrap();
    let mut pos = 0.0f32;
    group.bench_function("eq_tonestack_knob_moving", |b| {
        b.iter(|| {
            pos = if pos > 0.9 { 0.0 } else { pos + 0.05 };
            sweeping.set_param(bass, pos);
            buf.copy_from_slice(&signal());
            buf_r.copy_from_slice(&signal());
            sweeping.process(black_box(&mut buf), black_box(&mut buf_r));
        })
    });

    for (index, pedal) in lh_dsp::modulation::FAMILY.pedals.iter().enumerate() {
        let mut modulation = Modulation::new();
        modulation.prepare(SR);
        modulation.select_pedal(index);
        bench_stereo!(group, format!("mod_{}", pedal.key), modulation, buf, buf_r);
    }

    // Pitch: both granular shifters run every sample regardless of knob levels.
    for (index, pedal) in lh_dsp::pitch::FAMILY.pedals.iter().enumerate() {
        let mut pitch = lh_dsp::pitch::Pitch::new();
        pitch.prepare(SR);
        pitch.select_pedal(index);
        for (i, p) in pedal.params.iter().enumerate() {
            pitch.set_param(i, p.default_norm());
        }
        bench_stereo!(group, format!("pitch_{}", pedal.key), pitch, buf, buf_r);
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

    // Practice metronome (PRD 019): worst-case block cost — a click sounding
    // through the whole block (restart before each iter fires the downbeat).
    // Runs on the player thread, not the audio callback; the engine's aux sum
    // is only a per-sample stereo add on top of this.
    {
        let mut metro = lh_dsp::practice::Metronome::new();
        metro.prepare(SR);
        metro.set_bpm(120.0);
        let mut mono = vec![0.0f32; BLOCK];
        group.bench_function("metronome_click", |b| {
            b.iter(|| {
                metro.restart();
                metro.render(black_box(&mut mono));
            })
        });
    }

    // Procedural drum groove (PRD 019 Phase 2): the busiest built-in pattern
    // (funk, 16th hats) rendered steadily — also a player-thread source.
    {
        let mut drums = lh_dsp::practice::DrumMachine::new();
        drums.prepare(SR);
        drums.set_pattern(lh_dsp::practice::pattern_index("funk").unwrap());
        drums.set_bpm(140.0);
        let mut mono = vec![0.0f32; BLOCK];
        group.bench_function("drum_groove_funk", |b| {
            b.iter(|| drums.render(black_box(&mut mono)));
        });
    }

    // Song player (PRD 019 Phase 3): WSOLA varispeed + a semitone transpose,
    // the worst-case pipeline. Player thread, off the RT budget; the WSOLA
    // correlation search is the cost.
    {
        let src: Vec<f32> = signal().iter().cycle().take(SR as usize).copied().collect();
        let song = std::sync::Arc::new(lh_dsp::practice::SongBuffer {
            r: src.clone(),
            l: src,
            sample_rate: SR,
        });
        let mut player = lh_dsp::practice::SongPlayer::new();
        player.prepare(SR);
        player.set_song(song);
        player.set_speed(0.75); // varispeed engages WSOLA
        player.set_semitones(2.0); // transpose engages the grain shifter
        player.set_loop(0, SR as usize / 2); // loop so it never stops mid-bench
        player.play();
        let mut sl = vec![0.0f32; BLOCK];
        let mut sr = vec![0.0f32; BLOCK];
        group.bench_function("song_player_stretch_shift", |b| {
            b.iter(|| player.render(black_box(&mut sl), black_box(&mut sr)));
        });
    }

    group.finish();
}

/// The full hand-written M5 pedalboard (everything but NAM) at the live
/// 64-frame format and the M6 stage target of 32 frames, where per-block
/// overhead weighs double.
/// ADAA's own cost, isolated from everything else (PRD 024): the same curve
/// through the same 4× oversampler, point-sampled versus first- and
/// second-order anti-aliased. Everything else in the drive rows below is held
/// constant, so the difference here is what the retrofit actually charges.
fn bench_adaa(c: &mut Criterion) {
    use lh_dsp::blocks::oversample::Oversampler4x;
    use lh_dsp::blocks::waveshaper::{Adaa1, Adaa2, Curve};

    let mut group = c.benchmark_group("block64_48k");
    let mut buf = signal();
    let curve = Curve::Hard;

    let mut os = Oversampler4x::new();
    group.bench_function("waveshaper_hard_plain", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            os.process(black_box(&mut buf), |blk| {
                for s in blk.iter_mut() {
                    *s = curve.f(f64::from(4.0 * *s)) as f32;
                }
            });
        })
    });

    let mut os1 = Oversampler4x::new();
    let mut a1 = Adaa1::new();
    group.bench_function("waveshaper_hard_adaa1", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            os1.process(black_box(&mut buf), |blk| {
                for s in blk.iter_mut() {
                    *s = a1.process(4.0 * *s, |u| curve.f(u), |u| curve.f1(u));
                }
            });
        })
    });

    let mut os2 = Oversampler4x::new();
    let mut a2 = Adaa2::new();
    group.bench_function("waveshaper_hard_adaa2", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal());
            os2.process(black_box(&mut buf), |blk| {
                for s in blk.iter_mut() {
                    *s = a2.process(4.0 * *s, |u| curve.f(u), |u| curve.f1(u), |u| curve.f2(u));
                }
            });
        })
    });
    group.finish();
}

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

/// What a *moving* knob costs a pedal whose drive pot sits inside an R-type
/// tree (`ts-wdf`, PRD 026). Turning it invalidates the scattering matrix, so
/// the block pays a rebuild per sub-block on top of its steady-state cost.
///
/// Read against `block64_48k/drive_ts-wdf_4x_oversampled`, which is the same
/// pedal with the knob parked: the difference is the whole price of building
/// scattering matrices at run time instead of evaluating a symbolic formula
/// (ADR 032), measured on a real circuit rather than a synthetic junction.
fn bench_wdf_knob_sweep(c: &mut Criterion) {
    let mut group = c.benchmark_group("block64_48k");

    let mut buf = signal();
    let mut buf_r = signal();
    let index = lh_dsp::drive::FAMILY
        .pedal_index("ts-wdf")
        .expect("ts-wdf is registered");
    let mut drive = Drive::new();
    drive.prepare(SR);
    drive.select_pedal(index);

    let mut pos = 0.0f32;
    group.bench_function("drive_ts-wdf_knob_sweeping", |b| {
        b.iter(|| {
            // A knob the user is actually turning: never twice the same value,
            // so the settled-skip never fires and every sub-block rebuilds.
            pos = if pos > 0.98 { 0.0 } else { pos + 0.02 };
            drive.set_param(0, pos);
            buf.copy_from_slice(&signal());
            buf_r.copy_from_slice(&signal());
            drive.process(black_box(&mut buf), black_box(&mut buf_r));
        })
    });
    group.finish();
}

/// The WDF diode root on its own, both solvers, over **256 solves** — exactly
/// one 64-frame block at 4× oversampling, mono. Reading the two numbers against
/// each other gives the closed form's speedup; reading either against the whole
/// `drive_screamer_4x_oversampled` row gives the root's share of the pedal.
fn bench_wdf_root(c: &mut Criterion) {
    use lh_dsp::blocks::wdf::DiodePair;

    // The port resistance the screamer's clipper actually presents: 2.2 kΩ
    // series in parallel with the 22 nF shunt discretized at 192 kHz.
    const R: f32 = 112.3;
    // 1N4148, as used by every diode-clipper pedal in the family.
    const DIODE: (f32, f32, f32) = (2.52e-9, 1.75, 25.85e-3);

    let mut group = c.benchmark_group("wdf_root_256_solves");

    // A continuous waveform swinging well past the ~0.7 V knee, so both the
    // near-linear and the hard-clipped regions are exercised — and so the
    // Newton path gets the warm start it is entitled to.
    let waves: Vec<f32> = (0..256)
        .map(|k| 2.0 * (std::f32::consts::TAU * 220.0 * k as f32 / (4.0 * SR as f32)).sin())
        .collect();

    group.bench_function("omega", |b| {
        let mut d = DiodePair::new(DIODE.0, DIODE.1, DIODE.2);
        b.iter(|| {
            for &a in black_box(&waves) {
                black_box(d.solve(a, R));
            }
        })
    });
    group.bench_function("newton", |b| {
        let mut d = DiodePair::new(DIODE.0, DIODE.1, DIODE.2);
        b.iter(|| {
            for &a in black_box(&waves) {
                black_box(d.solve_newton(a, R));
            }
        })
    });

    group.finish();
}

/// The composable WDF framework's own cost (PRD 025 / ADR 032), isolated from
/// any pedal: the per-sample scattering of an adaptor tree versus an R-type of
/// the same circuit, and the block-rate price of rebuilding a scattering matrix
/// when a knob moves.
///
/// All three run one 64-sample block's worth of samples at the 4× oversampled
/// rate the roots really see (256 iterations), so the numbers are directly
/// comparable with the `drive_*_4x_oversampled` rows.
fn bench_wdf_framework(c: &mut Criterion) {
    use lh_dsp::blocks::wdf::rtype::{Junction, RType};
    use lh_dsp::blocks::wdf::{Capacitor, Parallel, ResistiveVoltageSource, Resistor, Wdf};

    const OS: f32 = 4.0 * 48_000.0;
    const N: usize = 256; // one 64-frame block, 4× oversampled

    let mut group = c.benchmark_group("block64_48k");

    // The Screamer's shunt clipper, minus the diode: source ‖ capacitor.
    let mut tree = Parallel::new(
        ResistiveVoltageSource::new(2_200.0),
        Capacitor::new(22e-9, OS),
    );
    tree.calc_impedance();
    group.bench_function("wdf_parallel_tree", |b| {
        b.iter(|| {
            for k in 0..N {
                tree.port1_mut().set_voltage(black_box(k as f32 * 1e-3));
                let a = tree.reflected();
                tree.incident(black_box(a));
            }
        })
    });

    // The pre-framework spelling of that same node, in the same run: the
    // hand-reduced `parallel_root` helper the pedals used before PRD 025. This
    // is the only honest way to price the framework — whole-pedal numbers are
    // dominated by the diode root and the oversampler, and container speed
    // drifts between benchmarking sessions.
    let mut cap = Capacitor::new(22e-9, OS);
    cap.calc_impedance();
    let g_src = 1.0 / 2_200.0;
    group.bench_function("wdf_parallel_helper", |b| {
        b.iter(|| {
            for k in 0..N {
                let e = black_box(k as f32 * 1e-3);
                let a1 = cap.reflected();
                let (a, _r) =
                    lh_dsp::blocks::wdf::parallel_root(&[(g_src, e), (cap.conductance(), a1)]);
                cap.incident(black_box(2.0 * a - a1));
            }
        })
    });

    // The same node expressed as a 4-port R-type: source, capacitor and a load
    // resistor around one junction.
    static J4: Junction = Junction {
        nodes: 2,
        els: &[],
        ports: &[(1, 0), (1, 0), (1, 0), (1, 0)],
    };
    let mut rt: RType<4, 3, _> = RType::new(
        &J4,
        (
            ResistiveVoltageSource::new(2_200.0),
            Capacitor::new(22e-9, OS),
            Resistor::new(47_000.0),
        ),
    );
    rt.calc_impedance();
    group.bench_function("wdf_rtype4_scatter", |b| {
        b.iter(|| {
            for k in 0..N {
                rt.ports_mut().0.set_voltage(black_box(k as f32 * 1e-3));
                let a = rt.reflected();
                rt.incident(black_box(a));
            }
        })
    });

    // What a moving knob costs: one full nodal rebuild of the 4×4 matrix. This
    // happens at most once per block, and only when a knob actually moved.
    group.bench_function("wdf_rtype4_rebuild", |b| {
        b.iter(|| {
            rt.ports_mut().2.set_ohms(black_box(47_000.0));
            rt.calc_impedance();
        })
    });

    // An op-amp junction is the Phase 04 shape: three internal elements
    // including a controlled source, so the rebuild solves a 6×6 system.
    static OPAMP_J: Junction = Junction {
        nodes: 6,
        els: &{
            let oa = lh_dsp::blocks::wdf::op_amp(1, 2, 3, 4, 1e5, 1e9, 0.1);
            [
                oa[0],
                oa[1],
                oa[2],
                lh_dsp::blocks::wdf::JEl::Res {
                    a: 3,
                    b: 5,
                    ohms: 1_000.0,
                },
            ]
        },
        ports: &[(5, 0), (1, 0), (2, 0), (3, 2)],
    };
    let mut oa: RType<4, 3, _> = RType::new(
        &OPAMP_J,
        (
            ResistiveVoltageSource::new(10_000.0),
            Resistor::new(4_700.0),
            Resistor::new(47_000.0),
        ),
    );
    group.bench_function("wdf_rtype4_opamp_rebuild", |b| {
        b.iter(|| {
            oa.ports_mut().2.set_ohms(black_box(47_000.0));
            oa.calc_impedance();
        })
    });

    group.finish();
}

criterion_group!(
    benches,
    bench_effects,
    bench_full_chain,
    bench_wdf_knob_sweep,
    bench_wdf_root,
    bench_adaa,
    bench_wdf_framework
);
criterion_main!(benches);

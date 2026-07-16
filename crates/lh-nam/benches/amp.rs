//! NAM inference cost per 64-frame block at 48 kHz, using the small real
//! WaveNet fixture. Real captures ("standard" WaveNet) are ~100× heavier —
//! treat this as the plumbing cost floor, not the amp budget (see
//! docs/benchmarks.md for the budget math).

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;
use std::path::PathBuf;

use lh_dsp::Effect;
use lh_nam::{NamAmp, load_nam_file};

const SR: u32 = 48_000;
const BLOCK: usize = 64;

fn bench_amp(c: &mut Criterion) {
    let fixture = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/reference.nam");
    let (asset, _info) = load_nam_file(&fixture, SR).unwrap();

    let (mut amp, mut handle) = NamAmp::new();
    amp.prepare(SR);
    handle.install(asset).unwrap();

    let signal = lh_dsp::testutil::sine(SR, 220.0, BLOCK);
    let mut buf = signal.clone();
    let mut buf_r = signal.clone();
    c.bench_function("block64_48k/nam_reference_wavenet", |b| {
        b.iter(|| {
            buf.copy_from_slice(&signal);
            buf_r.copy_from_slice(&signal);
            amp.process(black_box(&mut buf), black_box(&mut buf_r));
        })
    });
}

criterion_group!(benches, bench_amp);
criterion_main!(benches);

//! Cost of the spill lanes under load (PRD 010): every lane ringing out the
//! priciest tail we have (reverb) into the output bus. Reported at the live
//! target format: 48 kHz, 64-frame blocks (1.33 ms budget per block).
//!
//! This mirrors `Chain::process_spill`'s inner loop — render each lane's
//! effect on silence, then sum into the bus — rather than driving the full
//! `Chain` lifecycle. A reverb's `process` cost is constant regardless of
//! its tail level (the FDN always runs), so a steady loop measures the true
//! worst-case per-block cost; the actual lane also does an age check and a
//! per-sample gain multiply, which are negligible next to the reverb.

use criterion::{Criterion, criterion_group, criterion_main};
use std::hint::black_box;

use lh_dsp::Effect;
use lh_dsp::time::Reverb;
use lh_engine::SPILL_LANES;

const SR: u32 = 48_000;
const BLOCK: usize = 64;

fn bench_spillover(c: &mut Criterion) {
    let mut group = c.benchmark_group("block64_48k");

    // One reverb per spill lane, excited so their tails are ringing.
    let mut lanes: Vec<Reverb> = (0..SPILL_LANES)
        .map(|_| {
            let mut rv = Reverb::new();
            rv.prepare(SR);
            rv
        })
        .collect();
    let warm_l = vec![0.5f32; BLOCK];
    let warm_r = vec![0.5f32; BLOCK];
    for rv in &mut lanes {
        rv.process(&mut warm_l.clone(), &mut warm_r.clone());
    }

    let mut bus_l = vec![0.0f32; BLOCK];
    let mut bus_r = vec![0.0f32; BLOCK];
    let mut scratch_l = vec![0.0f32; BLOCK];
    let mut scratch_r = vec![0.0f32; BLOCK];

    group.bench_function("spillover_worst", |b| {
        b.iter(|| {
            bus_l.fill(0.0);
            bus_r.fill(0.0);
            for rv in &mut lanes {
                scratch_l.fill(0.0);
                scratch_r.fill(0.0);
                rv.process(&mut scratch_l, &mut scratch_r);
                for i in 0..BLOCK {
                    bus_l[i] += scratch_l[i];
                    bus_r[i] += scratch_r[i];
                }
            }
            black_box((&bus_l, &bus_r));
        })
    });

    group.finish();
}

criterion_group!(benches, bench_spillover);
criterion_main!(benches);

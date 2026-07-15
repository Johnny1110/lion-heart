# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-14 — Linux container (aarch64, Docker on Apple Silicon) — indicative only

| Bench                            | Median      | % of 64-frame deadline |
| -------------------------------- | ----------- | ---------------------- |
| gate                             | ~409 ns     | 0.03 %                 |
| drive (4× oversampled)           | ~4.99 µs    | 0.37 %                 |
| delay                            | ~388 ns     | 0.03 %                 |
| cab IR (100 ms, 128-partitions)  | ~2.59 µs    | 0.19 %                 |
| NAM (tiny 131-weight fixture)    | ~6.25 µs    | 0.47 %                 |
| chain: gate → drive → delay      | ~5.43 µs    | **0.41 %**             |

Drive dominates the hand-written pedals — four half-band FIR passes plus tanh at 4×
rate. The NAM row uses the tiny test fixture and is a plumbing-cost floor: a realistic
"standard" WaveNet capture runs ~1.9 µs/sample (nam-rs, x86 reference) ⇒ ~122 µs/block
≈ 9 % of the deadline. Full M2 chain estimate with a real capture: **~10 %** — on
budget (white paper §3.2 targets < 40 % average).

_Add rows measured on real hardware (Apple Silicon, `cargo bench` on macOS) as they come._

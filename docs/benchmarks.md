# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-16 (M5) — Linux container (aarch64, Docker on Apple Silicon) — indicative only

| Bench                            | Median      | % of 64-frame deadline |
| -------------------------------- | ----------- | ---------------------- |
| gate                             | ~455 ns     | 0.03 %                 |
| comp                             | ~837 ns     | 0.06 %                 |
| drive (4× oversampled)           | ~5.54 µs    | 0.42 %                 |
| eq (3 biquads, block-rate coeffs)| ~603 ns     | 0.05 %                 |
| mod — chorus                     | ~743 ns     | 0.06 %                 |
| mod — flanger                    | ~779 ns     | 0.06 %                 |
| mod — phaser (4-stage swept)     | ~1.87 µs    | 0.14 %                 |
| mod — tremolo                    | ~542 ns     | 0.04 %                 |
| reverb (8-line FDN, Householder) | ~868 ns     | 0.07 %                 |
| delay                            | ~394 ns     | 0.03 %                 |
| cab IR (100 ms, 128-partitions)  | ~2.51 µs    | 0.19 %                 |
| NAM (tiny 131-weight fixture)    | ~6.25 µs    | 0.47 %                 |
| chain: gate → drive → delay      | ~6.18 µs    | 0.46 %                 |
| full 8-pedal chain (no NAM), 64  | ~13.9 µs    | 1.05 % (stereo bus)    |
| full 8-pedal chain (no NAM), 32  | ~6.73 µs    | 1.01 % of 667 µs       |

Drive still dominates the hand-written pedals (four half-band FIR passes plus tanh
at 4× rate); the phaser is next (per-sample `tan` for the swept allpass corner).
Since M7 the chain is **stereo end to end**; the full-chain rows above are stereo
and cost ~1.7× their old mono numbers (linked dynamics and the shared reverb core
keep it under 2×) — still ≈ 1 % of the deadline, scaling linearly down to
32-frame blocks. Per-effect rows predate the stereo bus where noted in git
history; refresh on the next hardware run. The NAM row uses the tiny test fixture and is a plumbing-cost floor: a
realistic "standard" WaveNet capture runs ~1.9 µs/sample (nam-rs, x86 reference)
⇒ ~122 µs/block ≈ 9 % of the deadline. Full chain estimate with a real capture:
**~10 %** — on budget (white paper §3.2 targets < 40 % average).

_Add rows measured on real hardware (Apple Silicon, `cargo bench` on macOS) as they come._

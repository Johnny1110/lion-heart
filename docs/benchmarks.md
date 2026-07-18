# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-18 (post-M8 health pass) — macOS, Apple Silicon (native)

First native-hardware run. Includes the health-pass optimizations: both EQs
skip their trig coefficient rebuilds while controls are settled (the numbers
below are the settled steady state — while a knob is actually moving the
global EQ costs ~2× this), and the 3-band drive pedals map their EQ gains
per chunk instead of per sample.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| gate                               | ~597 ns     | 0.04 %                 |
| comp                               | ~468 ns     | 0.04 %                 |
| drive — ts9 (4× oversampled)       | ~6.67 µs    | 0.50 %                 |
| drive — bd2                        | ~7.40 µs    | 0.55 %                 |
| drive — classic                    | ~5.66 µs    | 0.42 %                 |
| drive — centaur                    | ~6.60 µs    | 0.50 %                 |
| drive — evva                       | ~7.29 µs    | 0.55 %                 |
| drive — red-charlie                | ~9.61 µs    | 0.72 %                 |
| drive — monster5150                | ~12.9 µs    | 0.97 %                 |
| eq (3 biquads, settled)            | ~375 ns     | 0.03 %                 |
| mod — chorus                       | ~713 ns     | 0.05 %                 |
| mod — flanger                      | ~734 ns     | 0.06 %                 |
| mod — phaser (4-stage swept)       | ~1.40 µs    | 0.11 %                 |
| mod — tremolo                      | ~555 ns     | 0.04 %                 |
| reverb (8-line FDN, Householder)   | ~735 ns     | 0.06 %                 |
| delay                              | ~572 ns     | 0.04 %                 |
| cab IR (100 ms, 128-partitions)    | ~3.50 µs    | 0.26 %                 |
| global EQ (4 bands live, settled)  | ~804 ns     | 0.06 %                 |
| full 8-pedal chain (no NAM), 64    | ~8.72 µs    | 0.65 % (stereo bus)    |
| full 8-pedal chain (no NAM), 32    | ~4.40 µs    | 0.66 % of 667 µs       |

Micro-optimizations benched **and rejected** on this hardware (kept the
original code): a branchless conditional wrap replacing `%` in the
delay/modulation/reverb ring buffers made the delay ~10 % *slower* (the
integer divide pipelines under the surrounding float math; the extra
branches do not), and a below-threshold fast path in the compressor cost
~8 % in the above-threshold worst case — and the worst case is the
real-time budget.

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

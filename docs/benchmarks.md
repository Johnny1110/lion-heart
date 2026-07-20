# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-20 (M16 looper) — Linux dev container (relative)

The looper (PRD 013 / ADR 016) is a chain slot with a preallocated 60-second
double buffer. Its three steady states cost, in order: recording (a write per
sample), playing (one interpolated read + a smoothstep seam gain), and
overdubbing (read + soft-clipped in-place write, plus the undo-snapshot copy
during the first pass). All are a small fraction of the 0.15 % target set in
the PRD. Numbers from the Linux dev sandbox (read **relative**; re-measure
native on the Mac for the absolute table):

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| looper_record                      | ~0.81 µs    | 0.06 %                 |
| looper_play                        | ~0.65 µs    | 0.05 %                 |
| looper_overdub                     | ~1.03 µs    | 0.08 %                 |

## 2026-07-20 (M14 parametric EQ pedal) — Linux dev container (relative)

The eq family's second pedal (PRD 011 / ADR 014) is the output-stage
`GlobalEq` reused whole behind a 40-param façade, so its settled cost must
match the global stage — and it does. Numbers below are from the Linux dev
sandbox (same box, same run — read them **relative to each other**;
re-measure native on the Mac for the absolute table):

| Bench                              | Median      | Note                    |
| ---------------------------------- | ----------- | ----------------------- |
| eq_3band (tone pedal)              | ~684 ns     | unchanged path          |
| eq_parametric_4band                | ~1.45 µs    | 4 bands live, settled   |
| global_eq_4band (same box)         | ~1.46 µs    | parity: same engine     |

## 2026-07-19 (M13 expression: manual wah) — macOS, Apple Silicon (native)

The filter family's second pedal (PRD 008 / ADR 011): the manual wah drops
the envelope follower and reads a smoothed `pos` instead — same per-sample
sweep (exp + sin) and SVF, so the two pedals price alike. The family
restructure (one engine, per-pedal `Ctl` tables) left the autowah's cost
unchanged.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| filter — autowah (env + SVF)       | ~1.20 µs    | 0.09 %                 |
| filter — wah (pos + SVF)           | ~1.15 µs    | 0.09 %                 |

## 2026-07-19 (M13 spillover) — macOS, Apple Silicon (native)

The spill lanes (PRD 010 / ADR 013): tails ringing out after their slot
leaves the chain, summed into the output bus. Cost is one `Effect::process`
per occupied lane per block — a reverb's FDN runs the same whatever its tail
level, so this is a true per-block worst case, not a transient. Run with
`cargo bench -p lh-engine --bench spillover`.

`spillover_worst` fills all four lanes with reverb (the priciest tail) and
sums them; measured with the default `hall` voice. The absolute worst case
is four of the costliest voice (~4.4 µs each, see below) ≈ 18 µs — still
1.4 % of the deadline, and only while four tails ring at once.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| spillover_worst (4 × hall)         | ~7.6 µs     | 0.57 %                 |

## 2026-07-19 (M12 filter family) — macOS, Apple Silicon (native)

The new `filter` slot's first pedal (PRD 007 / ADR 010). Per-sample cost is
the sweep itself (one exp for the geometric fc map, one sin for the SVF
retune) plus the band soft clip.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| filter — autowah (env + SVF)       | ~1.23 µs    | 0.09 %                 |

## 2026-07-18 (M11 mod family expansion) — macOS, Apple Silicon (native)

Tremolo rebuilt (dB-linear depth, wave/spread) and four pedals added
(PRD 006 / ADR 009). Univibe pays four per-sample `tan`s for its staggered
stage corners — 0.21 % of the deadline, cache rejected as premature.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| mod — chorus                       | ~871 ns     | 0.07 %                 |
| mod — flanger                      | ~908 ns     | 0.07 %                 |
| mod — phaser (4-stage swept)       | ~1.56 µs    | 0.12 %                 |
| mod — tremolo (dB-depth, slewed)   | ~804 ns     | 0.06 %                 |
| mod — vibrato                      | ~852 ns     | 0.06 %                 |
| mod — harmonic                     | ~766 ns     | 0.06 %                 |
| mod — rotary (two rotors)          | ~972 ns     | 0.07 %                 |
| mod — univibe (staggered stages)   | ~2.85 µs    | 0.21 %                 |

## 2026-07-18 (M10 reverb family) — macOS, Apple Silicon (native)

The reverb slot became a twelve-machine family (PRD 005 / ADR 008); the old
`reverb_fdn8` bench is superseded by one bench per voice, each at its own
faceplate defaults. The tank now does interpolated reads (size scaling +
mod), per-line length ramps, and per-sample knob smoothing, so even the
plain hall costs more than the old fixed-read FDN (~735 ns) — the worst
voice is still ~0.33 % of the 1.33 ms deadline.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| reverb — hall                      | ~2.73 µs    | 0.21 %                 |
| reverb — room                      | ~3.76 µs    | 0.28 %                 |
| reverb — plate                     | ~3.46 µs    | 0.26 %                 |
| reverb — spring                    | ~4.06 µs    | 0.31 %                 |
| reverb — swell                     | ~3.57 µs    | 0.27 %                 |
| reverb — bloom                     | ~3.85 µs    | 0.29 %                 |
| reverb — cloud                     | ~3.71 µs    | 0.28 %                 |
| reverb — chorale                   | ~3.87 µs    | 0.29 %                 |
| reverb — shimmer                   | ~4.36 µs    | 0.33 %                 |
| reverb — magneto                   | ~4.43 µs    | 0.33 %                 |
| reverb — nonlinear                 | ~3.13 µs    | 0.24 %                 |
| reverb — reflections               | ~1.97 µs    | 0.15 %                 |

Hall at defaults (mod 0) skips the LFO trig; voices with mod on by default
(room/plate upward) pay one `sin_cos` per sample distributed to all eight
lines by phase rotation. If the reverb ever needs to shrink again, the
candidate is a fixed-read fast path when size/mod are settled at neutral —
rejected for now as premature (0.3 % of budget).

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

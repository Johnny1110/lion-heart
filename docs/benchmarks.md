# DSP benchmarks

Per-block processing cost at the live target format: **48 kHz, 64-frame blocks**,
deadline **1,333 µs** per block (white paper §3.2). Run with:

```sh
cargo bench -p lh-dsp --bench effects
```

## 2026-07-28 (Tone Revolution phase 04: `rat`, `diode-clipper`, `king-of-tone`) — Linux dev container (relative)

The last three pedals of the family, which completes phase 04's roster.

**Read the calibration row first, and discount accordingly.** `drive_screamer` is
untouched by any of this work and reads **~42 µs** here against ~31.9 µs in the
`ts-wdf` session below — this container was ~1.3× slower and noisier for this
run (criterion's intervals span ±10 % rather than the usual ±1 %). The ratios
hold; the absolute numbers do not compare across sessions without dividing them
through.

| Bench                                  | Median    | ÷ screamer | Reading                     |
| -------------------------------------- | --------- | ---------- | --------------------------- |
| `drive_screamer_4x_oversampled`        | ~42.1 µs  | 1.00       | calibration                 |
| `drive_diode-clipper_4x_oversampled`   | ~49.1 µs  | 1.17       | default Shunt wiring — the cheapest tree here |
| `drive_rat_4x_oversampled`             | ~50.7 µs  | 1.21       | deep tree, one root         |
| `drive_ts-wdf_4x_oversampled`          | ~55.2 µs  | 1.31       | (reference, unchanged)      |
| `drive_king-of-tone_4x_oversampled`    | ~76.4 µs  | 1.82       | **two stages, two roots**   |

`king-of-tone` is the family's most expensive because it is genuinely two
circuits: two trees solved and two diode roots per oversampled sample. At 1.82×
the Screamer it is still under 6 % of the deadline once the machine is
discounted, and it is the only pedal here that pays twice.

`diode-clipper` is the cheapest of the three at its default because the Shunt
wiring is a two-element tree; selecting its Feedback wiring puts it on the same
op-amp junction as the rest and costs about what they do.

## 2026-07-28 (Tone Revolution phase 04: `mxr-dist`) — Linux dev container (relative)

Third pedal (PRD 028). Same op-amp junction as the two before it, but adapted at
the *output* port instead of the feedback path, because this circuit clips shunt
to ground rather than inside the loop.

| Bench                               | Median    | Reading                     |
| ----------------------------------- | --------- | --------------------------- |
| `drive_screamer_4x_oversampled`     | ~32.5 µs  | machine calibration         |
| `drive_zendrive_4x_oversampled`     | ~41.1 µs  | feedback clipper            |
| `drive_ts-wdf_4x_oversampled`       | ~42.7 µs  | feedback clipper            |
| **`drive_mxr-dist_4x_oversampled`** | **~48.8 µs** | shunt clipper, deeper tree |

48.8 µs is **3.7 %** of the deadline, ~15 % over the feedback-clipping pair. The
difference is tree depth, not the junction: the input leg gains a `Series`, and
the output network is three adaptor levels (`Series` → `Parallel` → `Parallel`)
where the others had one.

Calibration: `drive_screamer` reads 32.5 µs here against 31.4 µs in the
`zendrive` session below — the top of this container's session spread, so read
cross-session differences under ~4 % as noise.

## 2026-07-28 (Tone Revolution phase 04: `zendrive`) — Linux dev container (relative)

Second pedal of the family (PRD 027 / ADR 034), and the one that prices the
framework's reuse claim: it is the *same junction* as `ts-wdf` with different
parts, so it should cost the same. It does.

| Bench                              | Median    | Reading                        |
| ---------------------------------- | --------- | ------------------------------ |
| `drive_screamer_4x_oversampled`    | ~31.4 µs  | machine calibration            |
| `drive_ts-wdf_4x_oversampled`      | ~40.3 µs  | shared junction, TS parts      |
| **`drive_zendrive_4x_oversampled`**| **~41.2 µs** | shared junction, Zendrive parts |
| `drive_sd1_4x_oversampled`         | ~69.2 µs  | ideal-op-amp WDF               |

41.2 µs is **3.1 %** of the deadline, and 2 % over `ts-wdf` — the same 4-port
matrix–vector product per sample, the same tree depth, one extra pot to compare
at sub-block boundaries.

Calibration: `drive_screamer` reads 31.4 µs here against 31.9 µs in the `ts-wdf`
session below, so the two tables are comparable within this container's ~2 %
session spread.

## 2026-07-28 (Tone Revolution phase 04: `ts-wdf`) — Linux dev container (relative)

The framework's first *new* pedal (PRD 026 / ADR 033): the whole Tube Screamer
clipping amplifier — op-amp, feedback network, diodes inside it, gain leg, input
coupling, load — as one WDF tree solved per oversampled sample.

Same run, so the four Screamer models can be read against each other directly:

| Bench                             | Median    | Reading                          |
| --------------------------------- | --------- | -------------------------------- |
| `drive_ts9_4x_oversampled`        | ~17.9 µs  | memoryless curve, the baseline   |
| `drive_screamer_4x_oversampled`   | ~31.9 µs  | WDF shunt clipper                |
| **`drive_ts-wdf_4x_oversampled`** | **~42.1 µs** | this pedal, knobs parked      |
| **`drive_ts-wdf_knob_sweeping`**  | **~48.7 µs** | this pedal, drive knob turning |
| `drive_sd1_4x_oversampled`        | ~71.7 µs  | WDF loop clipper, ideal op-amp   |

42.1 µs is **3.2 %** of the 1,333 µs deadline. It sits between `screamer` and
`sd1`: the extra over `screamer` is the R-type's 4×4 matrix–vector product plus
one more adaptor layer per sample.

The **sweeping** row is the one worth keeping. Turning the drive pot invalidates
the scattering matrix, so the block pays a full-tree rebuild per sub-block — 8
per stereo block here, ~840 ns each. That is **+16 % while a knob is moving and
zero when it is still**, and it is the first time ADR 032's "build `S`
numerically at run time" has been priced on a real circuit rather than on a
synthetic junction (the phase-03 rows below measured 114 ns / 458 ns for bare
junctions; the difference is the post-order recursion over the rest of the tree).

Machine calibration: `drive_screamer` reads 31.9 µs here against 31.8 µs in the
phase-03 session below, so that session's table and this one are directly
comparable.

## 2026-07-28 (Tone Revolution phase 03: WDF composable framework) — Linux dev container (relative)

`blocks::wdf` graduates from hand-reduced straight-line code to a composable
adaptor tree plus an N-port R-type (PRD 025 / ADR 032). The question this table
has to answer is the one the plan asked: **did the framework make anything
slower?** It did not — the framework path is *faster* than the code it replaced.

The whole-pedal rows are not the right place to look for that: they are
dominated by the diode root and the four half-band FIR passes, and container
speed drifts between sessions. So the framework is priced **in isolation and in
the same run**: 256 samples = one 64-frame block at 4× oversampling, mono, the
Screamer's shunt node with the diode removed, spelled both ways.

| Bench                       | Median      | Reading                              |
| --------------------------- | ----------- | ------------------------------------ |
| `wdf_parallel_tree`         | ~2.28 µs    | framework `Parallel<Rvs, Capacitor>` |
| `wdf_parallel_helper`       | ~2.92 µs    | the pre-PRD-025 hand-reduced helper — **28 % slower** |
| `wdf_rtype4_scatter`        | ~3.42 µs    | 4-port R-type, per-sample `b = S·a`  |
| `wdf_rtype4_rebuild`        | ~114 ns     | one knob move: rebuild the 4×4       |
| `wdf_rtype4_opamp_rebuild`  | ~458 ns     | ditto with an op-amp inside (6×6)    |

The adaptor beats the helper because it caches `p = G₁/(G₁+G₂)` and the port
resistance at `calc_impedance` time, while the helper re-derived `ΣGa/ΣG` and
`1/ΣG` — two divides — every sample. Generic monomorphisation costs nothing
here: the tree compiles to the same straight-line arithmetic the hand-written
version had.

The **rebuild** rows are what replaced offline symbolic code generation (ADR 032
§3). They are paid at the block boundary and only when a knob actually moved, so
114 ns lands at 0.009 % of the deadline and 458 ns at 0.034 % — and zero while
the knobs are still.

Whole-pedal rows, with the machine calibrated against the phase-01 session:

| Bench                          | This run  | 2026-07-27 (phase 01) | Δ       |
| ------------------------------ | --------- | --------------------- | ------- |
| `wdf_root_256_solves/omega`    | ~2.33 µs  | ~2.29 µs              | +1.7 %  |
| `wdf_root_256_solves/newton`   | ~27.1 µs  | ~29.1 µs              | −6.9 %  |
| `drive_screamer_4x_oversampled`| ~31.8 µs  | ~30.5 µs              | +4.3 %  |
| `drive_sd1_4x_oversampled`     | ~68.8 µs  | ~68.0 µs              | +1.2 %  |

The two untouched root rows disagree by +1.7 % and −6.9 %, which is this
container's honest session-to-session spread; both rewritten pedals sit inside
it. **No same-run old-vs-new whole-pedal measurement was taken**, so the pedal
rows are calibrated comparison, not a controlled one — the isolated rows above
are the controlled measurement, and the equivalence itself is proven
numerically instead (~1e-8 relative against the pre-rewrite commit, PRD 025 §3.1).

## 2026-07-27 (Tone Revolution phase 06: ADAA) — Linux dev container (relative)

**Read the machine speed first.** `drive_screamer` is untouched by this phase and
came in at **54.7 µs** here against the **30.5 µs** recorded in the phase-01
section above — the same binary shape on a container running roughly 1.8× slower
today. Absolute microsecond figures do not compare across sessions in this
environment; only same-run comparisons do.

So the authoritative cost of ADAA is the isolated group, where the *only*
difference between rows is the anti-aliasing (same curve, same 4× oversampler,
same buffer):

| Bench                        | Median   | vs plain |
| ---------------------------- | -------- | -------- |
| waveshaper_hard_plain        | ~4.2 µs  | —        |
| waveshaper_hard_adaa1        | ~4.5 µs  | **+7 %** |
| waveshaper_hard_adaa2        | ~5.4 µs  | **+28 %**|

Medians over four runs; the run-to-run spread on this container is comparable to
the ADAA1 difference, so treat +7 % as "small" rather than as three digits.

A whole pedal costs more than its shaper stage suggests, for two reasons worth
recording: `tanh`'s antiderivative `ln cosh` is a pair of **`f64`** transcendentals
per oversampled sample (ADR 031 explains why `f64` is not negotiable here), and
the cascade pedals run two or three ADAA stages. Same-run figures after the
retrofit:

| Bench                             | Median    | % deadline |
| --------------------------------- | --------- | ---------- |
| drive_red-charlie (2× ADAA1)      | ~32.5 µs  | 2.4 %      |
| drive_monster5150 (3× ADAA2)      | ~28.2 µs  | 2.1 %      |
| drive_angry-charlie-v2 (2× ADAA2) | ~22.7 µs  | 1.7 %      |
| drive_ts9 (ADAA2 + dry sum)       | ~18.1 µs  | 1.4 %      |
| drive_waveshaper (new pedal)      | ~17.2 µs  | 1.3 %      |
| drive_overdrive (ADAA1)           | ~10.6 µs  | 0.8 %      |
| drive_screamer (WDF, unchanged)   | ~54.7 µs  | 4.1 %      |
| drive_sd1 (WDF, unchanged)        | ~69.8 µs  | 5.2 %      |

One optimisation **kept**: `tanh_f1` returns `|x| − ln 2` directly past
`|x| = 20`. The correction term is smaller than one `f64` ulp of `|x|` there, so
this is exact rather than an approximation, and at these gains most samples take
it — `drive_jan-ray` went 23.0 → 17.8 µs on it.

One optimisation **benched and rejected**: rewriting ADAA2's two halves through a
reciprocal (trading two `f64` divisions for multiplies) made no measurable
difference, so the code keeps the form that reads like the derivation.

## 2026-07-27 (Tone Revolution phase 02: real tone stack) — Linux dev container (relative)

The shared 3-band becomes a real passive amp network (PRD 023 / ADR 030): each
model is a **netlist**, solved numerically into a continuous state space and
Tustin-discretised at the block boundary, then run as a ≤4-state filter.

The two `eq_tonestack` rows are the ones to read — they measure the engine
directly and were stable across runs:

- **Settled** (knobs still) is the steady-state cost: only the state space runs,
  and it lands on top of `eq_parametric_4band`.
- **Knob moving** re-solves the netlist *and* re-discretises it every 64 samples
  — the worst case the drive family can ask for. It costs **+20 %**, i.e. one
  full rebuild is ~300 ns. That number is what justified choosing a numeric
  netlist solve over hand-derived closed-form coefficients (ADR 030): the thing
  the closed form would have bought is 0.3 µs per knob-moving block.

| Bench                              | Median      | % deadline             |
| ---------------------------------- | ----------- | ---------------------- |
| eq_tonestack_settled               | ~1.52 µs    | 0.11 %                 |
| eq_tonestack_knob_moving           | ~1.91 µs    | 0.14 % (worst case)    |
| eq_parametric_4band (ref)          | ~1.49 µs    | 0.11 %                 |
| eq_3band (ref, the old additive)   | ~0.75 µs    | 0.06 %                 |

The five migrated drives, before (worktree at `5fa0f9e`) and after, both on this
container. **The container is noisy** — `drive_evva` alone read 12.9 µs and
15.4 µs on two runs of the *same* binary, and the unchanged `drive_ts9`
reference drifted 11.4→11.8 µs — so read the deltas as "about a microsecond",
which is what the engine rows above predict (one mono stack per channel).

| Bench                              | Before   | After    | Δ        |
| ---------------------------------- | -------- | -------- | -------- |
| drive_ts9 (ref, **unchanged**)     | ~11.36 µs| ~11.77 µs| drift    |
| drive_evva                         | ~12.44 µs| ~12.86 µs| +0.4 µs  |
| drive_red-charlie                  | ~17.93 µs| ~18.65 µs| +0.7 µs  |
| drive_monster5150                  | ~11.99 µs| ~13.93 µs| +1.9 µs  |
| drive_angry-charlie                | ~11.39 µs| ~11.91 µs| +0.5 µs  |
| drive_angry-charlie-v2             | ~12.52 µs| ~13.14 µs| +0.6 µs  |

All five stay near the 4× oversampler floor that `drive_ts9` marks; the tone
stack is not what any of them costs.

## 2026-07-27 (Tone Revolution phase 01: Wright Omega root) — Linux dev container (relative)

The WDF diode root stops iterating (PRD 022). `DiodePair::solve` is now a
closed-form evaluation — Werner eqn (39) rearranged into two Wright-omega
lookups, each a fitted quartic guess plus one Newton correction over
polynomial/bit-trick `exp`/`log` (`blocks::wdf::omega`). The `f64` damped Newton
survives as `solve_newton`, the accuracy oracle and the row below.

**All four rows come from this run**, so the Newton screamer number is a true
same-machine baseline and not the 2026-07-23 figure (which was ~68 µs on a
different day). Two things are worth reading off the table:

- The root **microbench** is 256 back-to-back solves = one 64-frame block at 4×
  oversampling, mono. Closed form vs Newton there is **12.7×**.
- Inside the pedal the same swap is only **2.4×**, and that is not a
  contradiction: `solve` is *stateless*, so the microbench pipelines 256
  independent solves, while in the circuit every solve feeds the capacitor state
  and the chain is strictly serial. The pedal measures **latency**, the
  microbench **throughput**. Screamer minus its root costs 12.4 µs (measured by
  stubbing the root out), which is the floor any 4× oversampled pedal pays.

`sd1` keeps the Newton root — its asymmetric curve has no eqn (39) form, and
generalising one is deferred (see `docs/tone_revolution/phase/01`). Its row is
unchanged and confirms the baseline machine speed.

| Bench                              | Median      | % deadline             |
| ---------------------------------- | ----------- | ---------------------- |
| wdf_root_256_solves/omega          | ~2.29 µs    | —      (root only)     |
| wdf_root_256_solves/newton         | ~29.1 µs    | —      (root only)     |
| drive_screamer_4x_oversampled      | ~30.5 µs    | 2.3 %  (was ~72.3)     |
| drive_screamer, Newton root (ref)  | ~72.3 µs    | 5.4 %  (same run)      |
| drive_sd1_4x_oversampled           | ~68.0 µs    | 5.1 %  (still Newton)  |
| drive_ts9_4x_oversampled (ref)     | ~11.3 µs    | 0.85 % (memoryless)    |

## 2026-07-24 (deep water #2: WDF feedback overdrive) — Linux dev container (relative)

The second white-box circuit (PRD 021 / ADR 029): the Boss SD-1 op-amp
overdrive, this time the **feedback topology** (diodes in the op-amp feedback
loop, reduced by the ideal-op-amp virtual short) with an **asymmetric** diode
root (2 diodes one way, 1 the other). Same shape as the screamer's cost — a
warm-started damped Newton solve in `f64` per **oversampled** sample per channel
— so it benches essentially identical (the asymmetric root's two `exp` per
iteration ≈ the symmetric root's `sinh`; the extra linear gain-leg bilinear is
cheap). Deep-water price, only paid when `sd1` is the selected drive pedal; the
optimisation paths noted for the screamer apply here too.

| Bench                              | Median      | % deadline             |
| ---------------------------------- | ----------- | ---------------------- |
| drive_sd1_4x_oversampled           | ~70.9 µs    | 5.3 %                  |
| drive_screamer_4x_oversampled (ref)| ~70.1 µs    | 5.3 %  (WDF, matched)  |
| drive_ts9_4x_oversampled (ref)     | ~11.4 µs    | 0.86 %  (memoryless)   |

## 2026-07-23 (deep water: WDF Tube Screamer) — Linux dev container (relative)

The first white-box circuit model (PRD 020 / ADR 028): the Screamer clipping
stage solved as a Wave Digital Filter — an antiparallel-diode root behind a
shunt RC, a warm-started damped Newton solve in `f64` per **oversampled** sample
per channel. This is the deep-water research line's deliberately-expensive path,
and it shows: ~6× the memoryless `ts9`, the `exp` in the Newton loop dominating.
Still well inside budget, and only paid when `screamer` is the selected drive
pedal. The `ts9` row is the memoryless reference from the same run. Optimisation
paths (an `f32` solve, a fast `exp`, below-knee oversampling bypass) are left as
future work — correctness first for a circuit model.

| Bench                              | Median      | % deadline             |
| ---------------------------------- | ----------- | ---------------------- |
| drive_screamer_4x_oversampled      | ~68 µs      | 5.1 %                  |
| drive_ts9_4x_oversampled (ref)     | ~11.4 µs    | 0.86 %  (memoryless)   |
| drive_overdrive_4x_oversampled     | ~9.2 µs     | 0.69 %  (memoryless)   |

## 2026-07-22 (M20 power amp) — Linux dev container (relative)

The hand-written valve power stage (PRD 017 / ADR 024): a 4× oversampled
push-pull waveshaper with sag, bracketed by presence/depth shelves and a
transformer stage. Its cost sits with the drive family's 4× pedals — the
oversampler round trip dominates, the sag/shelves are cheap. Benched driven
into saturation (its worst case); **zero cost while bypassed**, which is how
it ships (`default_active("power") == false`). The reference drive rows are
from the same run for calibration (this box runs hotter than the Apple-Silicon
sections below).

| Bench                              | Median      | % deadline             |
| ---------------------------------- | ----------- | ---------------------- |
| power_4x_oversampled               | ~12.5 µs    | 0.94 %                 |
| drive_ts9_4x_oversampled (ref)     | ~11.5 µs    | 0.86 %                 |
| drive_monster5150_4x (ref)         | ~12.5 µs    | 0.94 %                 |

## 2026-07-21 (practice tools: song player) — Linux dev container (relative)

The song player (PRD 019 Phase 3 / ADR 022) is a WSOLA varispeed stage plus a
GrainShift transpose. Like the metronome and drums it renders on the player
thread, **off the RT budget**. WSOLA's per-grain cross-correlation search
dominates: the worst case below is 75 % speed **and** a +2-semitone transpose
(both granular stages active). The player thread fills ~2048 frames per wake
(~1.2 ms of this compute) every ~3 ms, so there is ample slack; the null-device
run showed no underruns. The correlation search is the obvious optimization
target (decimate / SIMD) if it ever needs trimming.

| Bench                              | Median      | Note                        |
| ---------------------------------- | ----------- | --------------------------- |
| song_player_stretch_shift          | ~38 µs      | player thread, off RT budget |

## 2026-07-21 (practice tools: drum groove) — Linux dev container (relative)

The procedural drum groove (PRD 019 Phase 2 / ADR 021) is a five-voice synth
kit clocked at the exact global BPM. Like the metronome it renders on the
player thread, **off the RT budget**; the audio-thread aux cost is unchanged (a
ring read + a stereo add). The number is the busiest pattern (funk, 16th hats)
rendered steadily.

| Bench                              | Median      | Note                        |
| ---------------------------------- | ----------- | --------------------------- |
| drum_groove_funk (5 voices)        | ~0.82 µs    | player thread, off RT budget |

## 2026-07-21 (practice tools: metronome) — Linux dev container (relative)

The metronome (PRD 019, Phase 1 / ADR 020) is an aux **monitor** source: it
renders on the app's player thread — not the audio callback — and the engine
only sums its ring into the output after the safety limiter. So its render cost
is off the RT budget entirely; the number below is the worst case (a click
sounding through the whole 64-frame block). The audio-thread cost the aux lane
*adds* is a per-sample stereo add plus one lock-free ring read — below the noise
floor of a criterion bench, and `assert_no_alloc`-clean (validated by a
null-device jam with the click on).

| Bench                              | Median      | Note                        |
| ---------------------------------- | ----------- | --------------------------- |
| metronome_click (worst-case block) | ~218 ns     | player thread, off RT budget |

## 2026-07-20 (M16 looper) — Linux dev container (relative)

The looper (PRD 013 / ADR 019) is a chain slot with a preallocated 60-second
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

The eq family's second pedal (PRD 011 / ADR 017) is the output-stage
`GlobalEq` reused whole behind a 40-param façade, so its settled cost must
match the global stage — and it does. Numbers below are from the Linux dev
sandbox (same box, same run — read them **relative to each other**;
re-measure native on the Mac for the absolute table):

| Bench                              | Median      | Note                    |
| ---------------------------------- | ----------- | ----------------------- |
| eq_3band (tone pedal)              | ~684 ns     | unchanged path          |
| eq_parametric_4band                | ~1.45 µs    | 4 bands live, settled   |
| global_eq_4band (same box)         | ~1.46 µs    | parity: same engine     |

## 2026-07-19 (pitch family: octaver) — macOS, Apple Silicon (native)

The new `pitch` slot's first pedal (ADR 016): a granular octaver. Per-sample
cost is two `blocks::grain::GrainShift` reads (each a phasor advance + two
interpolated taps + two sine windows) plus one block-rate Tone coefficient.
Both shifters run every sample regardless of knob levels, so this is the true
per-block cost. Opt-in family (off the default board), so it only costs when
the player adds it.

| Bench                              | Median      | % of 64-frame deadline |
| ---------------------------------- | ----------- | ---------------------- |
| pitch — octaver (2 grain shifters) | ~1.05 µs    | 0.08 %                 |

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

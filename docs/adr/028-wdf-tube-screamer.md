# ADR 028: WDF Tube Screamer clipping stage — the first white-box circuit model

Status: **accepted — code landed (uncommitted); pending user verification by ear**
Date: 2026-07-23
Relates to: white paper §6 (deep water: "WDF 白箱電路模擬，第一個題目：Tube
Screamer 削波級"), PRD 020, ADR 003 (drive registry + memoryless waveshaping —
the approach this departs from), the drive family's `Circuit` trait +
`Oversampler4x`, and `ts9` (the memoryless model kept as the A/B reference)

## Context

The 2026-07-20 nine-feature roadmap is complete and the next direction, chosen
by the user 2026-07-23, is the white paper's **deep-water research line** —
whose explicitly-named first topic is a **Wave Digital Filter model of the Tube
Screamer clipping stage**.

Every one of the 11 drive pedals so far is *memoryless* waveshaping: a static
curve (`tanh`, `x/√(1+x²)`, polynomial knees) plus one-pole filters. That is
good, cheap and controllable, but it is structurally blind to what a real
clipper's tone comes from — the interaction of an RC network with the diode
junction, so the clipping threshold moves with frequency and transient. No
static curve `y = f(x)` can have that; it needs the actual circuit, discretized.

## Decision

Build the **reusable WDF substrate** and its **first application**, the Screamer
clipping stage, as a **new drive pedal** — not a replacement for `ts9`.

### Reusable framework: `lh_dsp::blocks::wdf`

A minimal, RT-safe set of wave-domain primitives (no boxed tree, no dynamic
dispatch, no allocation on the audio thread — a circuit composes them into
straight-line per-sample code):

- **`Capacitor`** — bilinear one-port, `R = T/(2C)`, reflected wave `b[n] =
  a[n−1]` (its entire state is the last incident wave). Denormal-flushed.
- **`DiodePair`** — antiparallel matched diodes (`i = 2·Is·sinh(v/nVt)`) as a
  nonlinear **root**. Solves `a = v + R·i(v)` for the reflected wave by
  **warm-started, damped Newton–Raphson** in `f64` (the tiny `Is` needs the
  precision): warm start from last sample's `v` (continuous audio → 1–3 steps);
  the step is capped at `10·nVt` so a cold, slammed input cannot overshoot into
  the stiff exponential and stall; a fixed 16-iteration ceiling and an
  exp-overflow clamp make it bounded and finite by construction (RT rule 7).
- **`parallel_root`** — the parallel-adaptor reduction: from the linear ports'
  `(conductance, reflected-wave)` pairs it returns the incident wave the root
  sees and its port resistance, `a = (ΣGₖaₖ)/(ΣGₖ)`, `R = 1/(ΣGₖ)`. Making the
  root reflection-free is what breaks the delay-free loop.

### The pedal: `drive::screamer` (key `screamer`, name "Screamer")

The clipping stage as a **shunt RC-diode clipper** driven at the TS op-amp gain:
`e = g·hp` (720 Hz input high-pass → op-amp gain law shared with `ts9`) into a
parallel node of {resistive source through `R_SERIES` 2.2 kΩ, shunt `C` 22 nF,
antiparallel 1N4148 root}. `shape()` solves one WDF sample per oversampled frame
(4×); the clipped node voltage is summed with the unity dry path for the classic
mid-hump; `post()` keeps the `ts9` tone tilt + makeup + DC block. 1N4148
parameters `Is 2.52 nA, n 1.75, Vt 25.85 mV`.

**New pedal, `ts9` untouched.** `ts9` is calibrated, tested and shipped; keeping
it makes the whole point auditable — the user can A/B the circuit model against
the curve and decide whether the CPU is worth it. Append-only: `DRIVE_PEDALS`
grows to 12, `MODEL_COUNT` 11→12, **no preset schema bump** (every drive
addition is append-only), plugin auto-expands `drive_screamer_*` via
`from_families` (no plugin code change).

### Deltas from PRD 020

1. **Modelling scope is the shunt clipper, not the feedback topology.** The real
   TS clips in the op-amp *feedback* loop; v1 models it as a shunt clipper driven
   at the op-amp gain — a faithful reduction of the *audible* diode dynamics
   (symmetric soft clip, reactive frequency dependence). Modelling the ideal-
   op-amp feedback loop as a WDF (an R-type adaptor / ideal-op-amp constraint)
   is deferred to v2 along with asymmetric diode counts.
2. **Bench cost is higher than the PRD's estimate.** PRD 020 guessed ~20–40 µs;
   measured (x86 sandbox, criterion) **≈68 µs / 64-frame stereo block** — about
   6× the memoryless `ts9` (11 µs), ~5.1 % of the 1333 µs deadline. The
   `f64 exp` inside Newton, per oversampled sample per channel, is the cost.
   Accepted as the deep-water white-box price (still well inside budget, and
   only paid when `screamer` is the selected drive pedal); optimisation paths
   (an `f32` solve, a fast `exp` approximation, or below-knee oversampling
   bypass) are noted for later rather than compromising accuracy now.
3. **The frequency-dependence discriminator moved to the WDF core.** Through the
   full pedal the 720 Hz high-pass confounds a low-vs-high harmonic comparison;
   the clean, unconfounded proof drives `Screamer::clip()` directly at two
   frequencies (`shunt_cap_makes_clipping_frequency_dependent`). The full-pedal
   `screamer` vs `ts9` test asserts they voice highs *measurably differently*
   (the honest claim — the circuit model is not a reskin) rather than a specific
   direction (the WDF's harder diode knee actually keeps *more* high-end edge
   than the `ts9`'s soft curve behind its 51 pF lowpass).

## Consequences

- **The white box exists.** Tone now follows component values (`Is`, `R`, `C`)
  through an actual discretized circuit, not a hand-drawn curve — the substrate
  the deep-water line was reserved for. `blocks::wdf` is ready for the next
  circuit (a feedback-topology TS, a diode-ladder tone stack, a triode stage).
- **RT-safe and correct.** 68 µs is real but bounded and allocation-free; the
  Newton solver is proven convergent/finite on `±1e6` slams. `assert_no_alloc`
  is unviolated (fixed iterations, `f64` locals, stack-array adaptor call).
- **Additive everywhere.** No schema bump, no engine/session change, no plugin
  code change; `drive_screamer_*` param ids appear (pre-v0.1 additive break —
  re-run clap-validator). Theme gained a jade Screamer livery under the
  distinct-livery pin.
- **`ts9` stays** as the deliberate A/B reference; the two are pinned to voice
  highs differently.

# ADR 029: WDF feedback-topology overdrive with asymmetric clipping — deep water #2

Status: **accepted — code landed (uncommitted); pending user verification by ear**
Date: 2026-07-24
Relates to: PRD 021, white paper §6 (deep water: WDF white-box circuit
modelling), PRD 020 / ADR 028 (deep water #1, the WDF Tube Screamer shunt
clipper — this is the **v2** it explicitly deferred), the `blocks::wdf`
substrate, and `ts9` / `screamer` (the memoryless and shunt-WDF references kept
for A/B)

## Context

Deep water #1 (ADR 028) shipped the reusable `blocks::wdf` substrate and its
first application, the `screamer` — the Tube Screamer clipping stage as a
**shunt** RC-diode network with **matched** antiparallel diodes. Its "non-goals"
wrote down two things for a v2: the real **feedback topology** (the diodes clip
in the op-amp *feedback* loop, not to ground) and **asymmetric** diode counts
(even harmonics). This ADR delivers both.

The vehicle is the **Boss SD-1 "Super OverDrive"** — the same op-amp overdrive
family as the TS, but whose defining feature *is* asymmetric feedback clipping
(2 silicon diodes one way, 1 the other). Modelling the SD-1 therefore exercises
exactly the two substrate gaps a v2 needs, and yields a real, famous pedal that
sounds audibly different from the matched screamer rather than "a more accurate
screamer". `ts9` and `screamer` are untouched, so the user can A/B three points
on the same circuit: memoryless curve → shunt-WDF → feedback-WDF.

## Decision

Grow the substrate by two RT-safe primitives and add one new drive pedal.

### Substrate: `lh_dsp::blocks::wdf`

- **`AsymDiode`** — an antiparallel clipper with asymmetric branch counts,
  `i(v) = Is·(exp(v/(m·nVt)) − exp(−v/(k·nVt)))`. Monotonic (Newton converges),
  and it reduces to `DiodePair`'s `2·Is·sinh` exactly at `m = k = 1` (pinned by a
  test). Same RT-safe solver as `DiodePair`: warm-started **damped Newton** in
  `f64`, step capped at `10·min(m,k)·nVt`, 16-iteration ceiling, exp-clamped →
  bounded/finite/alloc-free by construction.
- **`parallel_root_with_source`** — the parallel-adaptor reduction with an
  external **current injection**: `a = (Σ Gₖaₖ + I)/Σ Gₖ`, `R = 1/Σ Gₖ`. This is
  how an ideal op-amp's forced feedback current drives the diode clipper.
  `parallel_root` is untouched (the `I = 0` case; `screamer` stays bit-identical).

### The feedback topology, reduced by the ideal op-amp

Rather than a general Werner R-type nullor scattering adaptor, the single op-amp
loop is reduced **analytically** by the ideal-op-amp virtual short — correct for
this topology and far cheaper:

1. The op-amp holds `V(−) = V(+) = Vin`; the gain-setting leg (`R_gain` series
   `C_g`, to ground) then draws a current `I_g` that depends only on `Vin` and
   the leg's state — a linear sub-problem (direct **bilinear** RC, kept in the
   pedal file like `screamer`'s series RC).
2. KCL forces that same `I_g` into the feedback network `[R_f ‖ C_f ‖ AsymDiode]`;
   `parallel_root_with_source` + `AsymDiode::solve` give the node voltage `V_fb`
   (the one nonlinear step).
3. `Vout = Vin + V_fb`.

Two TS behaviours the screamer had to fake now fall out of the topology: the
**dry signal always passes** (`+Vin`, gain ≥ 1 → an op-amp overdrive never
becomes a fuzz, no hand-summed dry path), and the **mid-hump is intrinsic**
(`C_g` rolls loop gain toward 1 below its drive-dependent corner → lows stay
clean, no separate 720 Hz input high-pass). `C_f` sits across the diodes and
rounds the hardest highs.

### The pedal: `drive::sd1` (key `sd1`, name "Super Drive")

Faceplate Drive / Tone / Level (the SD-1's own three knobs); `shape()` solves one
WDF sample per 4× oversampled frame; `post()` keeps the shared `ts9`/`screamer`
tone tilt + makeup + DC block (the DC blocker also removes the offset the
asymmetric clip creates). Append-only: `DRIVE_PEDALS` / `MODEL_COUNT` 12→13, **no
preset schema bump**, plugin auto-expands `drive_sd1_*` via `from_families` (no
plugin code change). Theme gained an SD-1 canary-yellow livery under the
distinct-livery pin.

## Deltas from PRD 021

1. **Component values are calibration, not datasheet.** `R_f` landed at **120 kΩ**
   (a hotter op-amp gain than a stock SD-1's ~47 kΩ) because our signal is in
   sample units (nominal guitar ≈ −18 dBFS) while the diode drops are ~0.6/1.2 V:
   120 kΩ is the value that pushes a nominal level past the knees across the drive
   range **and** keeps a low note below the knee at drive 6 so the mid-hump still
   reads (`sd1_makes_the_mid_hump`), while dropping `C_f`'s corner to a musical
   ≈14 kHz. `R_gain` 4.7 kΩ + 100 kΩ drive pot, `C_g` 0.047 µF, 1N4148 2/1.
   `MAKEUP` 0.26 for unity at defaults. These are pinned by the character tests,
   the PRD's stated intent (calibrate by ear, pin the values).
2. **Bench cost matches the screamer, not "same order, TBD".** Measured **≈70.9 µs
   / block** (x86 sandbox) vs the screamer's ≈70.1 µs — the asymmetric root's two
   `exp` per Newton iteration ≈ the symmetric root's `sinh`, and the linear gain
   leg is cheap. ~5.3 % of the 1333 µs deadline; deep-water price, only paid when
   `sd1` is selected.
3. **The white-box discriminator is the gain leg, not a shunt cap.** Where the
   screamer's core test shows its shunt cap softening *highs*, the sd1 core test
   (`gain_leg_makes_clipping_frequency_dependent`) shows `C_g` lifting *mids* —
   the dual effect, and the mid-hump grown from the circuit. The asymmetry proof
   is `core_clip_is_asymmetric` (non-zero DC from a symmetric input) at the core
   and `sd1_makes_even_harmonics_where_the_screamer_does_not` through the pedal.

## Consequences

- **The feedback topology and asymmetric root exist**, closing PRD 020's v2
  debt. `blocks::wdf` now covers matched *and* asymmetric roots and current-driven
  parallel nodes — the substrate for the next feedback circuits (a diode-ladder
  tone stack, a triode stage — deep water #3).
- **RT-safe and correct.** ~70 µs is real but bounded and allocation-free (fixed
  Newton iterations, `f64` locals, a stack-array adaptor call, a bilinear RC).
  The solver is proven convergent/finite on `±1e6` slams. `assert_no_alloc`
  validation on the Mac is pending (the sandbox has no audio device), same as
  every prior feature; it is unviolated by construction (screamer's exact
  pattern).
- **Additive everywhere.** No schema bump, no engine/session change, no plugin
  code change; `drive_sd1_*` param ids appear (pre-v0.1 additive break — re-run
  clap-validator).
- **`ts9` and `screamer` stay** as the deliberate A/B references; the three are
  pinned to distinct behaviours (memoryless symmetric curve / WDF shunt matched /
  WDF feedback asymmetric).
- **13 new tests** (5 `blocks::wdf`: asym=matched at 1/1, asym solves its
  equation, asymmetric, ±1e6-bounded, current-injection; 3 sd1 core:
  frequency-dependent clip, asymmetric-DC, silence→silence; 2 sd1 character:
  even-harmonics-vs-screamer, mid-hump; plus the family suites and the
  registry/unity/theme pins auto-cover index 12). Full suite 435 green; fmt /
  clippy clean.

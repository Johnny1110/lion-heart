# ADR 008: Reverb family — twelve machines, one engine

Status: **accepted — implemented**
Date: 2026-07-18
Relates to: PRD 005 (`docs/PRD/005-reverb-family.md`), ADR 007 (delay
family), PRD 001 (per-pedal faceplates), white paper §4.2/§4.3/§7

## Context

The reverb slot was the M5 single-pedal FDN (8 lines, Householder feedback,
`decay/tone/predelay/mix`). PRD 005 wants the full BigSky-style machine
list — hall, room, plate, spring, swell, bloom, cloud, chorale, shimmer,
magneto, nonlinear, reflections — which spans genuinely different
*structures*, not just voicings: pitch-shifted regeneration, envelope-gated
bursts, a multi-head echo hybrid. The delay family (ADR 007) already proved
the multi-pedal pattern; the question was how far one engine can stretch
before per-voice structs are cleaner.

## Decision

- **One `Reverb` engine, twelve `VoiceDef`s** (family key unchanged,
  `hall` first = the M5 voicing and the v4→v5 migration target). Like ADR
  007, the hot loop reads voice constants instead of dispatching through a
  trait — but the reverb voices differ structurally, so `VoiceDef` carries a
  structural `Kind` on top of the numeric constants:
  - `Tank` — the FDN, now with **interpolated line reads**: lengths scale
    with `size` (geometric sweep; hall's noon is exactly 1.0 so migrated
    files hit the M5 lengths) and wobble under `mod` (one LFO distributed
    across the 8 lines by phase rotation: one `sin_cos` per sample, eight
    constant rotations — not eight `sin` calls).
  - `Magneto` — a 1–4 head echo line (feedback tap soft-clipped and
    darkened in-loop, tape-style) whose head bus feeds the tank.
  - `Shaped`/`Early` — **feedback-free multitap bursts** for nonlinear and
    reflections. A gated/reverse envelope is not a decay loop; making it a
    finite window of taps (24 jittered taps × shape law, or 12-tap ER
    tables × 3 room shapes) means the "physics-defying" voices are
    trivially bounded and end exactly when the window ends.
  - In-tank inserts, one per voice where needed: shimmer's granular pitch
    shifter in the regeneration path (dual-tap, sine-crossfaded, ~64 ms
    grain), chorale's two vowel bandpasses **outside** the loop (resonant
    boosts never re-enter the feedback), spring's chirp bank (6 cascaded
    2nd-order allpasses × up to 3 detuned springs — unity magnitude, so
    in-path dispersion costs no stability), swell's onset-retriggered ramp,
    bloom's regenerative diffusion loop.
- **Control vocabulary**: explicit `Ctl` enum per param position (Decay,
  Predelay, Mix, Tone, Mod, Size, LowEnd, Dwell, Rise, …) instead of delay's
  generic ModA/ModB — twelve voices share too little for positional
  conventions to stay readable. Stepped knobs (springs, heads, interval,
  shape, mode) snap; everything else lands on a `Smoothed`.
- **`tone` stays a damping corner in Hz** (the v4 key, unit, and range on
  hall) — one-pole in the tank feedback (per line), in the echo loop
  (magneto), or on the burst output (nonlinear/reflections). `lowend` keeps
  stability by construction: below neutral it drives an **in-loop highpass**
  (loss only), above neutral an **input low shelf** (gain outside the loop).
- **Boundedness invariants** (white paper §7): shimmer re-entry is
  soft-clipped (`tanh(1.3·x)/1.3`) and its effective amount capped at 0.72,
  so max-everything drones instead of diverging; bloom's loop gain is the
  knob, capped at 0.85 by the param range; the FDN's per-line
  `g = 10^(−3·len/t60)` stays < 1 for every reachable size/decay pair. A
  test slams every param of every voice min→max mid-note and asserts finite
  bounded output.
- **Coefficient cadence**: line lengths/gains, damp, lowcut/shelf, and
  formant coefficients rebuild at block rate only while a feeding knob is
  unsettled (the health-pass settled-skip pattern); line lengths ramp
  per-sample within the block so size sweeps glide (doppler, like a real
  size knob) instead of zipping.
- **Preset schema v5**: `migrate_v4_reverb_pedal` renames pedal `reverb` →
  `hall` (values verbatim — the faceplate is a superset with neutral new
  defaults: mod 0, size noon, lowend neutral). `REVERB_PEDALS` in lh-core
  pins the registry order; a test in lh-dsp pins the two lists together.
- **Zero engine/session/plugin code changes** — the M8/M9 multi-pedal
  machinery covered everything. Plugin param ids change
  (`reverb_decay` → `reverb_hall_decay`, …, plus `reverb_pedal`):
  **pre-v0.1 param-id break**, re-run clap-validator.

## Consequences

- Twelve voices cost 1.97–4.43 µs per 64-frame stereo block on Apple
  Silicon (worst: magneto, 0.33 % of deadline; see `docs/benchmarks.md`).
  The interpolated tank makes hall ~3.7× the old fixed-read FDN (~735 ns →
  ~2.73 µs) — accepted; a settled-neutral fast path was considered and
  rejected as premature at 0.2 % of budget.
- The engine allocates every voice's state up front (~1 MB per slot:
  predelay, 8 scaled lines, echo, burst, shifters) so pedal switches touch
  no memory on the audio thread (RT rule 1), and tank tails ring through a
  switch like the delay family's.
- BigSky is the *inspiration*, not a parity target: parameters map onto
  Lion-Heart's knob paradigm (e.g. reflections has no listener position;
  swell's rise replaces an expression input). Documented per voice in the
  module headers.
- The one-`match`-engine approach is at its comfortable limit: a
  thirteenth structurally novel voice (granular, convolution) should be a
  new `Kind` only if it reuses the tank; otherwise revisit per-voice
  structs behind the same family.

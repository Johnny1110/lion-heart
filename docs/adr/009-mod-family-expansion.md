# ADR 009: Mod family expansion — perceptual tremolo + four new pedals

Status: **accepted — implemented**
Date: 2026-07-18
Relates to: PRD 006 (`docs/PRD/006-mod-family-expansion.md`), ADR 008
(reverb family Ctl routing), PRD 001, white paper §4.2/§4.3

## Context

The tremolo was reported as barely audible. Diagnosis found three design
causes, not bugs: a hardwired half-cycle stereo offset (an auto-panner whose
L+R largely cancels in a room — the old test even asserted the sum stays
steady), a linear-amplitude depth law (−6 dB at noon; −2.5 dB after the v2
`depth × mix` fold), and a sine-only LFO. Separately, the family covered
only half of the classic modulation canon (no pitch vibrato, no harmonic
tremolo, no rotary, no vibe).

## Decision

- **Tremolo, rebuilt around perception.** Gain = `exp(−60 dB · depth · w)`:
  dB-linear in the LFO, peaks pinned at unity. Half depth now digs −30 dB
  instead of −6. `wave` picks sine / triangle / chop; the applied gain runs
  through a ~1.2 ms one-pole slew (declicks the chop edges, snaps when
  settled so depth 0 stays bit-exact). `spread` turns the old hardwired
  auto-pan into a knob, **default 0 (in phase)** — the fix's core: both
  speakers throb together. At full spread the dB law's convexity means L+R
  is *not* conserved (both sides are quiet at the crossing) — accepted and
  documented as hard ping-pong; the test asserts envelope anti-correlation,
  not sum constancy. Old presets get audibly stronger tremolo — that is the
  request, not a regression (noted against the usual migration-transparency
  rule).
- **Four new pedals**, appended after the v2 migration's four (the
  `MOD_PEDALS` index map covers exactly the first four; a test pins the
  prefix):
  - **vibrato** — wet-only swept delay read (5.5 ± 4.5 ms), both channels
    driven by the same LFO phase: a pitch bend is one event, and the test
    asserts L == R exactly. Character test: strong FM collapses the 440 Hz
    carrier (> 2×) while RMS holds within ±25 %.
  - **harmonic** — complementary one-pole split at 700 Hz, band gains in
    counter-phase (`w` vs `1 − w`), so level holds while timbre seesaws.
    Sub-threshold depth (< 1e-7, −140 dB) returns the input verbatim —
    the split re-sum `low + (x − low)` is not bit-exact in floats, so the
    off position is an explicit branch, and the smoother's near-zero tail
    can't defeat it.
  - **rotary** — mono into the "cabinet", stereo out: complementary split
    at 800 Hz; horn and drum rotors each with doppler read, AM, and pan off
    their own phase; per-rotor inertia via `Smoothed` rate targets (horn
    900 ms, drum 3 200 ms) — flipping `speed` yields the Leslie spin-up,
    and `select_pedal` starts rotors at slow so arriving at a fast preset
    spins up audibly. `balance` is an equal-power drum⇄horn crossfade.
  - **univibe** — the phaser machinery with *staggered* stage corners
    (78/210/620/1 750 Hz), swept together by a lamp-skewed LFO
    (`(0.5+0.5·sin)^1.6`), fixed 50/50 dry blend, no feedback knob. A test
    pins it apart from the phaser.
- **Per-pedal `Ctl` routing table** (ADR 008's pattern, scaled down):
  param positions no longer imply rate/depth/feedback/mix — rotary leads
  with a stepped `speed`, tremolo carries `wave`/`spread`.
- No schema bump: appended pedals and appended tremolo knobs are both
  covered by sparse-default handling; the v2 tremolo fold writes only
  `rate`/`depth`, which keep their keys and leading positions.

## Consequences

- Costs on Apple Silicon (64-frame stereo block): chorus 0.87 µs, flanger
  0.91 µs, phaser 1.56 µs, tremolo 0.80 µs, vibrato 0.85 µs, harmonic
  0.77 µs, rotary 0.97 µs, univibe 2.85 µs. Univibe pays four `tan`s per
  sample (per-stage corners); at 0.21 % of the deadline a block-rate
  coefficient cache was rejected as premature.
- The mod family now has eight liveries in the theme (Phase-90 orange,
  blonde tremolo, Leslie walnut…), pinned distinct by the livery test
  alongside drive/delay/reverb.
- Plugin param ids: tremolo gains `mod_tremolo_wave`/`_spread`, and the
  four new pedals' params appear — another **pre-v0.1 param-id break**;
  re-run clap-validator.
- The old "auto-pan conserves the sum" behavior is gone by default. Anyone
  wanting it back sets spread to taste; the linear-law softness is not
  recoverable, by design.

# ADR 003: Drive model registry — pedal-style knobs, preset schema v2

Status: **accepted — implemented**
Date: 2026-07-16
Relates to: white paper §4.3 (params & presets), CLAUDE.md DSP conventions

## Context

The drive slot was a single hand-written waveshaper (biased tanh at 4×
oversampling) with `drive` in dB, `tone` as a lowpass corner in Hz, and
`level` in dB. The user wants classic pedals — a TS9 and a Boss Blues
Driver first — selectable from a dropdown, with more models (including
self-designed ones) registrable later, and with each pedal's knob layout
matching the real unit so no relearning is needed.

Two constraints shape the design:

1. **The plugin needs a stable param set.** CLAP/VST3 hosts cache parameter
   lists; per-model parameter sets would break automation. Every commercial
   multi-model plugin solves this with one fixed param set plus a stepped
   model selector — so do we (the modulation family set the precedent with
   its stepped `type`).
2. **Real pedal knobs are positions, not units.** A TS9 tone knob is not a
   cutoff in Hz — showing "3200 Hz" is both wrong (its tone stack is a
   tilt, not a sweepable lowpass) and alien to a guitarist. Faithful knobs
   must be `0..=10` positions with per-model tapers.

## Decision

- **One `drive` slot, stepped `model` param + three shared position knobs**
  (`drive`/`tone`/`level`, `Range::Linear 0..10`). Param keys never change;
  presets, MIDI CC mappings, and plugin params stay stable across models.
- **Registry in `lh-dsp::drive`**: `MODELS: [ModelDef; N]` where each entry
  carries the menu label, the knob captions (so the GUI shows "Gain" for
  the Blues Driver), and a builder for the model's `Circuit` (nonlinear
  `shape` pass at the 4× oversampled rate, linear `post` tone stack at the
  base rate). The stepped labels are derived from the registry at compile
  time; GUI dropdown, REPL labels, MIDI, and plugin all read the registry.
  **Append-only**: presets store the model index.
- All models are **preallocated per channel** at construction; switching on
  the audio thread is an index change plus a state reset of the incoming
  model (brief discontinuity, never an allocation — same contract as the
  modulation family).
- Registered models: `ts9` (720 Hz high-passed gain path, matched-diode
  soft clip, dry sum → the mid-hump), `blues driver` (full-range gain,
  asymmetric knees → even harmonics), `classic` (the original waveshaper,
  bit-for-bit gain structure).
- **Preset schema v1 → v2 migration** (`PRESET_SCHEMA_VERSION = 2`): v1
  drive values (dB/Hz/dB) are mapped through inverse knob laws onto
  positions and pinned to `model = classic`, so old presets sound
  identical. The laws and their inverses live together in
  `lh_core::drive_law` (lh-core cannot see the DSP crate; a test in lh-dsp
  pins the classic registry index to `preset::CLASSIC_DRIVE_MODEL`).
- Knob captions are per-model **in the GUI only**; the plugin's param names
  stay fixed ("Drive"), since hosts cache them.

## Consequences

- Adding a model = implement `Circuit`, append one `ModelDef`. Nothing else
  to touch; everything downstream is registry-driven.
- The plugin's drive section gains one host-visible param (`model`) and its
  knob ranges change — a break for any pre-v2 host automation, accepted
  pre-v0.1.
- TS9/Blues Driver are calibrated to ≈unity loudness at default knobs and
  nominal guitar level (tested); `classic` keeps its v1 gain structure and
  can be hotter — switching to it mid-song behaves like swapping to a
  louder pedal.
- The character claims are pinned by offline tests (mid-hump vs full-range
  clipping, matched vs asymmetric diodes via 2nd-harmonic measurement, DC
  removal, click-free knob slams, level matching) at nominal guitar level.

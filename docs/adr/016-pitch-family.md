# ADR 016: Pitch family — a granular octaver (opt-in, off the default board)

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: white paper §6 (DSP module plan), ADR 004 (per-pedal params),
ADR 005 (dynamic chain), ADR 010 (filter family — the new-family precedent),
ADR 008 (delay/reverb VoiceDef engine pattern)

## Context

The effect lineup covers dynamics, drive (10), EQ, modulation (8), delay (3),
reverb (12), filter (2), and cab — but there is **no pitch effect** at all.
Octave/pitch (POG, Whammy, OC-2) is a staple guitar category and the one
obvious hole. The shimmer reverb already contains a proven, RT-safe granular
pitch shifter (`time::reverb`), so the DSP risk is low: promote that primitive
and build a standalone family on it.

## Decision

- **New family `pitch`** (key deliberately broad, like `filter`: a harmonizer,
  whammy, or detuner all belong here later). First and only pedal today:
  **`octaver`**.
- **Opt-in, not on the default board.** Unlike `filter` (ADR 010), `pitch` is
  **registered but absent from `lh_core::DEFAULT_CHAIN`**: it is added from the
  ＋ menu / REPL `add pitch`, so it does not consume one of the 12 default-board
  slots (the board stays 11/12, keeping a free slot for a second drive etc.).
  This decouples `FAMILY_REGISTRY` from `DEFAULT_CHAIN` for the first time: the
  default board is now an **in-order subsequence** of the registry, not an exact
  match. The pinning test was relaxed to the subsequence relation; the invariants
  (unique keys, no trailing digit, only amp/cab mount) still hold over the full
  registry. The plugin's fixed chain stays pinned to `DEFAULT_CHAIN` directly, so
  **the plugin does not include `pitch` in v1** (a fixed-chain host can't add
  slots; host pitch is a follow-up).
- **Shared granular engine** (`blocks::grain::GrainShift`): an interpolated
  delay line read by two taps a half-grain apart, sine-windowed so each wrap
  hides under the other tap's peak — the classic no-FFT "doppler" shifter, the
  same math the shimmer reverb uses. `ratio` is the pitch multiplier (2.0 = up
  an octave, 0.5 = down). Feed-forward: no regeneration, so it cannot run away
  and has **no tail** (`tail_seconds()` stays 0 — removal is instant, no spill).
  It stands in `blocks/` as the reusable primitive; the reverb keeps its private
  copy for now (migrating a 1965-line M10 module for pure dedup is deferred to a
  health pass, not done pre-v0.1).
- **Octaver voicing.** The two shifted voices come off the **mono sum** (fat,
  centered octaves) while the **dry stays stereo**; a shared **Tone** lowpass
  (600 Hz–9 kHz) tames the granular fizz on the up-octave. Faceplate: `Dry`
  (default 1.0), `Sub` (−1 oct, default 0.5), `Oct` (+1 oct, default 0.0),
  `Tone`. Independent level knobs = the POG paradigm; no separate wet/dry mix.
  The pedal-level `Ctl` table + `PedalDef` shift ratios follow the filter/delay
  engine pattern, so a second pitch pedal (a fifth-harmonizer, a whammy) is a
  new file plus one registry entry.
- **No preset schema bump.** A new family is new vocabulary (like `filter`):
  old presets simply don't reference `pitch` and reconcile the (absent) slot
  away. Livery: orchid magenta, joined the distinct-livery test.

## Consequences

- **~1.05 µs / 64-frame block** (0.08 % of the 1333 µs deadline): two granular
  shifters plus one block-rate Tone coefficient. `pitch_octaver` bench added.
- **RT-safe**: `GrainShift` allocates its ring only in `prepare`; `process`
  allocates nothing (`assert_no_alloc` stays green). Feed-forward → bounded,
  never NaN.
- **Granular, not analog-divider.** It is polyphonic (tracks chords, any
  register) at the cost of a characteristic warble — that texture *is* the
  voice. It is **not** an OC-2-style monophonic frequency divider; a tracking
  sub-octave with a squarer, tighter low end is a possible future sibling pedal.
- **Tested offline**: octave-up/down land where expected (Goertzel probe), the
  Dry path is transparent with the shifted voices gated, Tone darkens the
  up-voice, output stays finite/bounded across the knob ranges, silence→silence.
  `GrainShift` has its own primitive-level tests. The warble character is an ear
  check on hardware.
- **Plugin unchanged** — no new param ids, clap-validator unaffected (the
  registry grew, but the plugin builds from `DEFAULT_CHAIN`, which did not).
- The `FAMILY_REGISTRY == DEFAULT_CHAIN` invariant is gone by design; adding a
  future opt-in family is now a registry entry with no board slot cost.

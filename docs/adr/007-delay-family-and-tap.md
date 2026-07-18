# ADR 007: Delay family (digital/tape/vintage) + tap tempo

Status: **accepted — implemented**
Date: 2026-07-18
Relates to: PRD 004 (`docs/PRD/004-delay-family.md`), PRD 001 (per-pedal
faceplates), ADR 003 (drive model registry), white paper §4.2/§4.3/§7

## Context

The delay slot was a single-pedal family: one interpolated-read delay line
with a fixed 4 kHz feedback lowpass and `time/feedback/mix`. PRD 004 wants
three real delay voices (digital / tape / vintage) with their own signature
knobs, plus four modern controls — tone (repeat brightness), mod depth, mod
rate, and tap tempo. The multi-pedal family machinery already exists (drive,
mod): the per-pedal engine shadow, GUI pedal picker, plugin param expansion,
and preset v3 per-pedal maps all carry over for free. Tap tempo is the one
mechanism that does not fit — it is a momentary action, not a smoothed
continuous parameter.

## Decision

- **`delay` becomes a three-pedal family** (family key unchanged), mirroring
  drive: a `FAMILY` desc + a `VOICES` registry of `VoiceDef` (faceplate +
  param→control routing + voicing constants). Unlike drive's structurally
  distinct oversampled circuits, the delay voices are one engine with three
  voicings, so the hot loop `match`es the voice's constants (like
  `modulation`'s `match mode`) rather than dispatching through a trait — no
  per-sample vtable in a recursive feedback loop. One file per pedal
  (`digital.rs` / `tape.rs` / `vintage.rs`) still holds each face + voicing,
  the engine + registry live in `time/delay/mod.rs`.
- **Voicings.** digital: linear feedback (≤ 0.9), widest/brightest tone
  sweep, longest time (2 s), no modulation. tape: soft-clipped feedback
  (`tanh(drive·x)/drive`, unity small-signal, `1/drive` ceiling), warmer
  tone, two LFOs (slow Wow + fast Flutter, both slightly on by default),
  time ≤ 1.2 s. vintage: darker/narrower tone range, harder soft clip, one
  Mod LFO, time ≤ 600 ms (BBD-like). Feedback reaches 1.0 (tape) / 1.05
  (vintage); the soft clip bounds the loop, so a cranked setting self-
  oscillates into a *bounded* drone — authentic analog behavior that can
  never NaN or run away (white paper §7). digital stays under unity, always
  decaying.
- **`tone` knob** (0..1) sweeps a one-pole lowpass in the feedback path
  across each voice's own `[min,max]` Hz; sitting in the loop it also darkens
  each successive repeat. Its coefficient rebuilds only when the knob moves
  (the settled-skip perf pattern), keeping the exp/powf off the hot path.
- **Modulation** is a phase-accumulator LFO offsetting the read distance
  (chorus-style); the mod-depth knobs (Wow/Flutter/Mod) scale a voice-fixed
  deviation, and the LFO rates are voice constants — so the two "mod depth /
  mod rate" controls from the PRD surface as each voice's signature knobs,
  and a voice's unused mod is gated to zero by its `mod_*_ms = 0` constant
  (stale depths from a previous pedal can't leak).
- **Tap tempo is control-side, GUI-only.** A `subdivision` stepped param
  lives on each faceplate (stored in presets, shown as a dropdown, expanded
  by the plugin) but is a **no-op in the audio path** — there is no
  host-tempo sync in v1; it is a modifier for the tap→time math. The GUI
  owns a per-slot `TapState` (last tap `Instant` + a running average of
  recent intervals); a `TapTempo` message is timed on arrival, and once two+
  taps land it sets `time = period × subdivision_ratio` through the normal
  `ChainHandle::set_param` path (RT-safe ring push — the DSP never learns
  about tap). Flipping the subdivision re-derives the time from the last
  tempo. Not wired into the REPL or MIDI (deliberately deferred).
- **Preset schema v3 → v4.** `DELAY_PEDALS = ["digital","tape","vintage"]`
  (pinned to the family by an lh-dsp test, like `DRIVE_PEDALS`/`MOD_PEDALS`).
  `migrate_v3_delay_pedal` renames the old `delay` pedal (selector + `pedals`
  map key) to `digital`; the shared `time/feedback/mix` keys carry over, and
  `tone/subdivision` take defaults — old files sound the same bar a slightly
  brighter default tone.

## Consequences

- **Plugin param ids change** (`delay_time` → `delay_digital_time`, …, plus a
  `delay_pedal` selector), a pre-v0.1 break like the drive/mod expansion.
  The plugin's fixed chain is still pinned to `DEFAULT_CHAIN` (family key
  unchanged), so `fixed_chain_matches_the_shared_default` still holds;
  re-run clap-validator.
- Per-block cost: digital is roughly the old delay; tape/vintage add the
  soft clip and one/two LFOs. `delay_{digital,tape,vintage}` join the
  criterion bench.
- The engine, session registry, and GUI param plumbing needed **no code
  changes** — the multi-pedal path already covered them; the GUI gained only
  the tap button/BPM readout and per-voice card colors.
- lh-dsp tests pin the registry/faceplates, echo timing, bounded self-
  oscillation at max feedback, tone brightness, per-voice modulation,
  vintage darkness, click-free sweeps, pedal switching, and multi-rate;
  lh-core tests cover the v3→v4 migration.

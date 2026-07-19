# ADR 014: Global tempo & note-division sync

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: PRD 004 (delay family & tap tempo), ADR 007 (delay family),
ADR 009 (mod family), white paper §7 (M6 "on stage"), §4.3 (normalized
params behind a `Range`)

## Context

The rig had no concept of a tempo. Delay offered a per-slot **tap** button
(GUI-side, `subdivision` × the tapped beat → `time`; `delay/mod.rs` even
noted "there is no host-tempo sync in v1"), and every modulation LFO ran at
a free-running `rate` in Hz. So a dotted-eighth delay could not lock to the
song, and a tremolo could not pulse in time — the single most-expected
"serious multi-fx" capability was missing. We want one rig tempo that any
time-based effect can lock to.

## Decision

- **One app-global BPM** lives in `AppConfig.tempo_bpm` (default 120,
  clamped 30–300), persisted in `config.json` beside `morph_ms`/`spillover`
  — environment, not tone (so it is *not* in presets; a per-preset tempo is
  a possible follow-up). Set by a preset-bar **tap** button, an editable BPM
  field, or REPL `tempo <bpm>`.
- **Sync is a per-pedal stepped `sync` param**, appended (append-only) to
  every lockable pedal: labels `Free · 1/1 · 1/2 · 1/4. · 1/4 · 1/8. · 1/8T
  · 1/8 · 1/16` (`lh_core::tempo::SYNC_DIVISIONS`), default **Free**. This
  rides the existing machinery for free: it persists in presets (v3+ per-pedal
  values — **no schema bump**, an old file simply lacks the key and defaults
  to Free → identical sound), it is MIDI-mappable, and the plugin auto-expands
  it. In the DSP it is a **control-side no-op**, exactly like delay's existing
  `subdivision` (`Ctl::Sync => {}`).
- **The math is pure and lives in `lh_core::tempo`.** Divisions are quarter-
  note-beat multiples; `synced_time_ms(bpm, div)` = the note length,
  `synced_rate_hz(bpm, div)` = its reciprocal period (a `1/4` LFO pulses once
  per beat). Unit-tested there.
- **Derivation is control-side, on `ChainHandle`.**
  `ChainHandle::apply_tempo_sync(bpm) -> bool` walks the chain: a slot whose
  active pedal exposes a non-Free `sync` locks its `time` (if it has one — a
  delay) or its `rate` (an LFO effect — tremolo) to the tempo, via the normal
  `set_param` path, so the effect's own smoother declicks it. Idempotent: a
  locked control is re-sent only when its target actually moves (tempo,
  division, or a pedal switch), so a settled rig makes no queue traffic. It
  returns whether anything moved so a UI refreshes only the changed faceplate.
  `Session::tick_tempo` delegates and is called every control tick (GUI frame
  / REPL poll) after `tick_morph`.
- **v1 lockable set: the 3 delay voices + tremolo.** The mechanism is
  family-agnostic (it keys off the presence of a `sync` param plus a
  `time`/`rate` param), so extending it to any other rate-based mod later is a
  one-line param append — no session, GUI, or engine change.
- **The per-slot tap + `subdivision` stay as-is** (untouched, additive): a
  distinct "tap this delay's own tempo" workflow, orthogonal to the global
  lock. No migration risk.

## Consequences

- **Zero DSP/RT cost.** No audio-thread state or code changed — `sync` is a
  no-op in the effects and the derivation is control-side. No new benchmark.
- **The engine is the sync authority for the app.** `apply_tempo_sync` lives
  in `lh-engine` (control-side `ChainHandle`) and is covered by an offline
  engine test (`tempo_sync_locks_delay_time_and_tremolo_rate`).
- **Plugin: `sync` appears but is inert in v1** — consistent with
  `subdivision`, which is already an inert control-side param there. Host
  tempo (`ProcessContext::transport().tempo`) is the natural wiring and a
  clean follow-up; for now DAW users use host automation / the DAW's own
  synced delay. Plugin param ids gain `delay_*_sync` and `mod_tremolo_sync`
  (**pre-v0.1 additive break — re-run clap-validator**).
- **A locked control overrides its knob** while synced; the Time/Rate knob is
  no longer the source of truth for that slot until `sync` returns to Free.
- **Clamped, never wild.** A whole note at a slow tempo can exceed a voice's
  `time` ceiling; `set_param` clamps it to the voice's max (documented
  behavior, not an error).

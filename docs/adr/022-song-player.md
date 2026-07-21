# ADR 022: Song player (practice tools, Phase 3)

Status: **accepted — implemented**
Date: 2026-07-21
Relates to: PRD 019 (`docs/PRD/019-practice-tools.md`, Phase 3), ADR 020 (the aux
monitor lane + player thread this builds on), ADR 021 (Phase 2 drums, same aux
mixer), ADR 016 (the grain pitch shifter reused for transpose), white paper
§3.1 (RT rules), §5.3 (engine canonical 48 kHz)

## Context

Phase 3 completes the practice tools: load a backing track and practice against
it — slow a solo down without dropping its pitch, transpose it to your key, loop
a hard bar. Three independent hard pieces: decode arbitrary audio, time-stretch
it, and pitch-shift it, plus a GUI to drive them. It reuses everything Phases
1–2 built (the aux monitor lane, the player thread, the aux mixer).

## Decision

- **WSOLA for varispeed, the grain shifter for transpose — two stages, not one.**
  Varispeed-without-pitch is a hand-written **WSOLA** time-stretch
  (`lh_dsp::practice::Wsola`): overlap-add of windowed grains, each placed by a
  short normalised cross-correlation search so the pitch period stays intact
  (plain OLA smears it). Stereo-linked — the alignment offset is chosen from L
  and applied to both channels, so the image holds. Transpose-without-tempo
  reuses **`blocks::grain::GrainShift`** (the octaver's shifter, ADR 016), one
  per channel. The pipeline is `source → WSOLA(tempo) → GrainShift(pitch) →
  mix`. Splitting the two features across the two granular tools (rather than
  one combined pitch-and-time stretch) keeps each **independently correct and
  testable**, and reuses a shipped, proven shifter. Pure DSP in
  `lh_dsp::practice::{Wsola, SongPlayer}` — no I/O.

- **`symphonia` decode on a loader thread, in the app — not `lh-assets`.** WAV
  and MP3 decode via `symphonia` (pure Rust, permissive). The dep lives in the
  **app crate only**, so it never bloats the plugin (which has no song player).
  The multi-MB decode + sinc-resample-to-engine-rate (reusing
  `lh_assets::resample_sinc`) runs on a **background loader thread** — never the
  audio thread — and hands a finished `Arc<SongBuffer>` to the player thread via
  a channel. The RT path only ever *reads* the immutable buffer.

- **A third aux source on the existing player thread.** The player renders the
  song (stereo) alongside the metronome and drums (mono) and sums all three into
  the aux ring; the audio thread is unchanged (reads + adds, ADR 020). A
  `SongShared` atomic block carries the transport (play / speed / semitones /
  mix / A-B loop / seek) and publishes the play position back for the GUI. The
  song runs on its **own transport**, not the global tempo — a backing track has
  its own tempo.

- **A-B loop by cursor reset.** The player watches the WSOLA analysis cursor at
  sub-block granularity; crossing B resets it to A (a seam, acceptable for a
  practice loop — a crossfade is future polish). No loop → stop at the end.

- **GUI: a dedicated `song` view tab.** A waveform strip (peak envelope +
  playhead + shaded loop region, draw-only Canvas), a seek slider, A/B/clear
  loop buttons, and speed / transpose / level sliders; the file browser gained
  an `AssetKind::Song` (`.wav`/`.mp3`). Seeking is the slider, not canvas
  click-handling — simpler and fully functional.

## Consequences

- **Not carried across a device restart.** The decoded buffer is large and lives
  on the player thread; a device/buffer change drops it and the user reloads.
  The metronome/drums *are* carried (small state); the song is the one practice
  source that isn't. Deliberate for v1.

- **Granular artifacts at the extremes.** WSOLA at very low speeds and
  GrainShift at large transposes both warble — inherent to time-domain granular
  methods (the octaver has the same character, ADR 016). Fine for the practice
  range (slow a solo to 60–80 %, nudge a couple of semitones); a phase-vocoder
  would be the higher-fidelity route if it ever matters.

- **The loop seam clicks slightly.** The A-B reset drops the overlap state; a
  ~few-ms discontinuity at the wrap. Standard for a simple practice looper; a
  crossfade at the loop point is the noted improvement.

- **Standalone-only; the plugin is untouched.** No new plugin params, no
  preset-schema change, `symphonia` never enters the plugin build. Hosts own
  their transport and file playback (ADR 013's precedent, once more).

- **Practice tools (PRD 019) are complete.** Metronome (ADR 020) + drums
  (ADR 021) + song player (this) — all three phases, all on the one aux lane and
  player thread that Phase 1 established.

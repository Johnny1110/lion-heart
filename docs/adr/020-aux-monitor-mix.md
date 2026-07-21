# ADR 020: Aux monitor mix + metronome (practice tools, Phase 1)

Status: **accepted — implemented**
Date: 2026-07-21
Relates to: PRD 019 (`docs/PRD/019-practice-tools.md`), PRD 003 / ADR 006 (the
output stage the aux sums onto), ADR 014 / ADR 018 / PRD 012 (global tempo — the
click's clock), ADR 013 (spillover — the "standalone-only, plugin unchanged"
precedent), white paper §3.1 (RT rules), §3.3 (the safety-limiter guarantee)

## Context

The practice-tools roadmap item (PRD 019) is three phases — metronome, drum
groove, song player — that share one need: audio that plays **alongside** the
processed guitar without being coloured by the amp or squeezed by the guitar's
dynamics. A click routed through the amp chain would be distorted; a backing
track summed *before* the safety limiter would duck (pump) under every loud
guitar transient. This is the first monitor/aux path in the engine, and the
foundation the later phases build on. Phase 1 ships the metronome; the design
had to be right for drums/song too (both decode/synthesise off the audio thread
and feed the same lane).

## Decision

- **A stereo aux lane on the output, summed *after* the safety limiter.** The
  chain grows an optional aux input ring (`Chain::set_aux_input`, interleaved
  L/R). `Chain::process` drains it and adds it onto the bus **after**
  `OutputStage::process` (global EQ → safety limiter → spectrum tap) and before
  the `peak_out` telemetry. Consequences of the ordering, all deliberate:
  - the click/backing bypasses the amp tone and is **not** limited by the
    guitar's safety limiter — a monitor mix, not part of the tone;
  - the **spectrum tap stays guitar-only** (it is read inside the output stage,
    before the aux sum) — the analyzer shows the tone, not the click;
  - `peak_out` reflects the true device-bound sum (including aux).

- **Synthesis on a player thread; the audio thread only sums.** The app spawns a
  dedicated `lh-aux-player` thread that renders the metronome into the aux ring,
  keeping ~50 ms buffered ahead. The audio callback does a lock-free ring read
  plus a per-sample stereo add — no allocation (`assert_no_alloc`-clean,
  validated by a null-device jam with the click on). Drop-on-empty: an underrun
  is a brief gap in the click, never a stall. When the metronome is off the
  player stops writing, the ring drains, and the aux sum early-returns — the
  output is bit-transparent (an engine test pins this).

- **The metronome is a generator in `lh-dsp`, not an `Effect`.** It lives in a
  new `lh_dsp::practice` module (the home for the Phase 2/3 drum player and
  WSOLA too). It renders a mono enveloped-sine click — beat-1 accent, time
  signature, restart/count-in — driven by an internal sample clock, so it is
  pure and offline-testable. It is *not* in the chain, so it implements none of
  the `Effect` trait; the player duplicates the mono click to both aux channels.

- **The click follows the global tempo (ADR 014).** BPM is not the metronome's
  own knob: the session pushes `config.tempo_bpm` into the shared control state
  whenever the tempo moves (tap / typed / MIDI clock), so tapping the footer BPM
  chip re-times the click for free.

- **Cross-thread control via atomics; runtime state rides a device restart.**
  The session writes an `Arc<MetronomeShared>` (enabled / bpm / volume /
  beats-per-bar / accent / a restart generation / a run flag); the player reads
  it each fill. `Relaxed` suffices — the scalars are independent and the audio
  itself travels the ring's own acquire/release. The thread is joined on
  `Session` drop; the metronome's runtime state is carried across a
  device/buffer restart (`CarryOver`), while BPM re-reads from the persisted
  config.

- **Standalone-only; the plugin is unchanged.** Hosts own their metronome and
  transport (same reasoning as spillover, ADR 013). No preset schema change —
  the metronome is app-global *environment*, not tone, and not stored in
  presets (its transient runtime state persists only across a device restart).

## Consequences

- **A momentary clipping risk, accepted.** Because the aux sums after the safety
  limiter, a full-scale guitar transient (limited to −0.3 dBFS ≈ 0.97) plus a
  click can exceed 0 dBFS and clip at the device. The click is kept moderate — a
  default-level accent peaks ≈ −9 dBFS (0.35), full volume ≈ −4.6 dBFS (0.59) —
  so it sits under the guitar, but a *coincident* full-scale guitar peak and
  click can still clip. This is accepted for a practice monitor: peaks rarely
  align, and a guitar seldom rides the ceiling continuously. Limiting the aux
  was rejected — it reintroduces the pumping the after-limiter placement exists
  to avoid. A gentle brickwall on the *summed* output would remove the risk
  without pumping, and is the noted mitigation if it proves audible in practice.

- **Foundation for Phases 2–3.** The drum groove (bundled WAV + WSOLA to the
  current BPM) and song player (`symphonia` decode, A-B loop, WSOLA varispeed,
  ±semitone via the PRD 018 grain shifter) render into the *same* aux ring from
  the *same* player thread. Only new sources and the `symphonia` dependency are
  added; the RT lane and its RT-safety are already in place.

- **A new always-on helper thread per session.** Cheap (it sleeps ~3 ms between
  fills and skips rendering while the click is off), and it simplifies the
  lifecycle versus lazily spawning on first enable (the aux ring must exist at
  stream start regardless, so the chain can hold the consumer).

- **The metronome's phase is samples-produced, not wall-clock.** It cannot drift
  relative to the audio it is mixed into (every produced sample plays exactly
  once, in order); an underrun shifts the click by the gap length but never
  desynchronises it. There is no external phase reference in standalone, so the
  click starts on beat 1 at enable/count-in, which is the expected feel.

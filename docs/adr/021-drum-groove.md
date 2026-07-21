# ADR 021: Procedural drum groove (practice tools, Phase 2)

Status: **accepted — implemented**
Date: 2026-07-21
Relates to: PRD 019 (`docs/PRD/019-practice-tools.md`, Phase 2), ADR 020 (the aux
monitor lane + player thread this builds on), ADR 014/018 (global tempo — the
groove's clock), white paper §3.1 (RT rules)

## Context

Phase 2 of the practice tools (PRD 019) is a drum groove that plays along at the
rig tempo. The PRD sketched it as **sample playback + WSOLA time-stretch** to the
current BPM (a fixed-tempo loop stretched to match), with a shared WSOLA
primitive spanning Phase 2 and 3. Two problems with that for us:

1. **No sample assets.** Lion-Heart ships no drum recordings, and good ones are
   heavyweight binary blobs to vendor into the repo. A stretched loop also
   introduces WSOLA artifacts at extreme ratios and can only *approximate* the
   target tempo (grain quantisation).
2. **Nothing else in Phase 2 needs WSOLA.** Its real consumer is the Phase 3
   song player (arbitrary-audio varispeed). Implementing it now, unused, is dead
   code the lint gate rejects.

## Decision

- **Synthesise the groove procedurally, at the exact global BPM.** A small
  analog-style drum machine (`lh_dsp::practice::DrumMachine`) builds the beat
  from a synth kit — kick (pitch-swept sine + beater click), snare (tone +
  high-passed noise), closed/open hi-hats (high-passed noise), tom (pitch-swept
  sine) — clocked off an internal 16th-note counter, exactly like the metronome
  (ADR 020). Building at the target tempo gives a **tighter lock than any
  stretched loop** (no grain quantisation, no stretch artifacts) and ships **no
  binary assets**. It is a deviation from the PRD's sample approach, recorded
  here; the tradeoff is timbre — a synth kit, not sampled drums (acceptable for
  a practice tool, and it keeps the beat perfectly on the click).

- **Deterministic synthesis.** The noise voices run a seeded xorshift PRNG, so a
  rendered bar is reproducible — the offline tests assert timing, hit density,
  tempo scaling, and bounds on the actual output.

- **Four built-in patterns + a fill.** `rock` / `funk` / `metal` / `ballad` as
  one-bar velocity tables on the 16-step grid (append-only — indices are the
  API); a one-bar tom-roll `fill` arms on the next downbeat. Patterns/velocity
  are baked in; per-step humanisation and swing are future polish.

- **It shares Phase 1's aux lane and player thread (ADR 020).** The player now
  renders the metronome *and* the drums and sums both into the aux ring; the
  audio thread is unchanged (still just reads + adds). Both track the one global
  tempo (BPM read from the metronome's shared state). A second `GrooveShared`
  atomic block carries the groove control (enabled / pattern / volume /
  fill-gen / restart-gen); it rides a device restart via `CarryOver` like the
  metronome. Standalone-only, app-global, no preset/plugin change (ADR 020's
  reasoning).

- **WSOLA is deferred to Phase 3.** With procedural drums needing no stretch,
  WSOLA lands with its actual consumer (the song player), where it is exercised
  and tested. The PRD's "shared Phase 2/3 primitive" framing assumed sample
  stretch in Phase 2; that assumption no longer holds.

## Consequences

- **Timbre is synthetic.** The kit is a clean 80s-drum-machine voice, not
  sampled acoustic drums. For practice (play *in time* to a solid beat) this is
  the right trade for exact tempo-lock and zero assets. A future sampled-kit
  path could load user WAVs from `~/.lion-heart/grooves/` and use the Phase 3
  WSOLA to lock them — the aux lane already supports it.

- **The groove sits under the guitar, post-limiter.** Like the metronome, drums
  sum after the safety limiter (ADR 020), so their level is kept conservative
  (a full-velocity downbeat peaks well under scale). The same coincident-peak
  clipping caveat applies and is accepted.

- **Cheap and off the RT budget.** ~0.82 µs to render a 64-frame block of the
  busiest pattern (`drum_groove_funk` bench) — and on the player thread, not the
  audio callback. The audio-thread cost is unchanged from Phase 1 (a ring read +
  a stereo add). `assert_no_alloc`-clean, validated by a null-device jam with
  the groove and click both on.

- **Patterns are code, not data.** Adding a groove is a `PATTERNS` entry; there
  is no pattern file format yet. Deliberate for v1 — a groove editor / import is
  out of scope (and the PRD lists "programmable arranger" as a non-goal).

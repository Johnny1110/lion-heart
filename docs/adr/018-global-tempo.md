# ADR 018: Global tempo — session-owned, control-side only

Status: **accepted — implemented**
Date: 2026-07-20
Relates to: PRD 012 (`docs/PRD/012-global-tempo.md`), PRD 004 / ADR 007
(delay family: the `subdivision` control-side-modifier precedent), PRD 008
(virtual MIDI targets: `snapshot.select` is the pattern `tempo.tap` follows),
ADR 014 (tempo sync: the note-division target this feeds)

> **Merge reconciliation (2026-07-21).** This ADR and ADR 014 were authored in
> parallel on two machines for the same feature area, and reconciled at merge
> time into one combined design. This ADR's lasting contribution is the **BPM
> source**: the session-owned `TempoState` (tap history + MIDI-clock median),
> the persisted `AppConfig.tempo_bpm`, and the plugin reading the host
> transport. The **sync mechanism** below — a boolean `sync` that re-derived a
> delay's `time` from `tempo × subdivision` — was **superseded** by ADR 014's
> stepped note-division `sync` selector and its engine-side
> `apply_tempo_sync` (which also locks a tremolo's `rate`). So where this ADR
> says "boolean sync" / "`retime_delay` on every synced delay", read: the
> source here feeds `ChainHandle::apply_tempo_sync` (ADR 014). The per-slot tap
> still sets a *Free* delay's time via its `subdivision`, as described.

## Context

Tap tempo lived per-slot inside the GUI (`TapState` in a `BTreeMap<String,
_>`, keyed by delay instance handle). Nothing understood MIDI clock, and
the plugin's delay `time` never followed the host's transport BPM — a
DAW-hosted delay drifting off the grid reads as a bug, not a missing
feature. Three tempo sources (tap, MIDI clock, host transport) needed one
place to land.

## Decision

- **The tempo lives in the standalone `Session`** (`TempoState`: current
  BPM, tap/clock accumulation), not the GUI. The GUI's `TapState` and its
  three tap-math methods are deleted outright — moving them into session
  is a straight port, not a parallel implementation. Session-transient,
  never persisted: a synced delay's `time` param is the durable result,
  so presets and `midi.json` stay exactly as portable as before.
- **`sync` joins `subdivision` as a second control-side-only delay param**
  (ADR 007's pattern: exists in `ParamDesc`/preset/plugin, the audio path
  treats it as a no-op, `Ctl::Sync => {}`). Both are resolved one layer up:
  the session for standalone, the plugin's `apply_tempo_sync` for hosted.
  **Zero engine or DSP-audio-path changes** — same shape as
  `subdivision`'s own landing.
- **MIDI clock (0xF8/0xFA/0xFC) parses in `lh-midi`** as `MidiEvent::Clock
  { stamp_us }` / `Start` / `Stop`, carrying the *driver's* arrival
  timestamp — tick **intervals** carry the tempo, and the control thread
  drains its channel in frame-sized batches, so a wall-clock read at drain
  time would quantize away exactly the sub-frame timing the tempo lives
  in. `midir`'s default filter is turned off (`Ignore::None`) so realtime
  bytes actually arrive. The session takes the **median** of the last 48
  tick intervals (not the mean) — one USB-scheduling hiccup cannot bend a
  clock-derived tempo, which a mean would let through.
- **Plugin sync overrides, it doesn't feed back.** `apply_tempo_sync` runs
  once per block after the existing float-forwarding loop: while the
  active delay pedal's `sync` is on, it derives `time` from
  `context.transport().tempo` and pushes it directly, **ignoring** that
  pedal's own `time` automation lane (industry norm — a synced knob going
  dead is expected). The host's `time` param itself is never touched, so
  flipping `sync` back off needs no restore logic beyond re-reading it.
  The pure math (`synced_time_ms`) is split out of the trait-bound method
  specifically so it is unit-testable without a `ProcessContext` mock.
- **No arbitration between tap/clock/host — last writer wins**, matching
  how a real pedalboard behaves: a running clock overwrites on every tick
  (by construction — nothing stops feeding it), a tap between ticks lands
  and holds until the next tick corrects it. Simpler than a priority
  system, and the failure mode (a stray tap while clocked) self-heals
  within one beat.
- **`tempo.tap` is a virtual MIDI target**, same shape as `snapshot.select`
  (PRD 009): a bare-slot `SetActive` action would misroute (a momentary
  footswitch's release half is a "value < 64" the target isn't built to
  ignore), so it's threaded through `SetParam` instead and the session
  gates on `norm >= 0.5` — the press, not the release, taps.

## Consequences

- The GUI's delay faceplate TAP button keeps its pre-ADR one-shot
  courtesy (tapping a slot's own button sets *that* slot's time even with
  `sync` off) via `Session::tap_tempo(one_shot: Option<&str>)`; the
  footer's new global BPM chip calls the same method with `None`.
- `sync` renders as a chip (not the generic stepped-param dropdown) next
  to the TAP button — a boolean reads better as a toggle than a two-item
  picklist, and it's the only delay param that needed the exception.
- Engine/DSP diff for this feature is two `ParamDesc` additions and one
  `Ctl` arm; everything else is control-side. `assert_no_alloc` is
  unaffected — no new allocation reaches the audio thread in either binary
  (the plugin's `apply_tempo_sync` runs on the audio callback but only
  does param math and an existing lock-free `set_param` call, the same
  RT-safety envelope the float-forwarding loop above it already lives in).
- Mod-rate sync (tremolo, etc.) is a natural follow-up now that a tempo
  source exists — deliberately out of scope here (PRD 012 §3).

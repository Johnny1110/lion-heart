# ADR 023: Monitor recorder & offline re-amp

Status: **accepted — implemented**
Date: 2026-07-21
Relates to: PRD 014 (`docs/PRD/014-recorder-reamp.md`), PRD 003 (the output-stage
tap this mirrors), ADR 020 (the aux monitor lane — the wet tap sits *before* it),
ADR 013 / ADR 020 (the "hosts own this; standalone-only" precedent for the
plugin), white paper §3.1 (RT rules), §4.1 (heavy work off the audio thread),
"success looks like" #1 (record a guitar track with Lion-Heart)

## Context

The white paper's first success metric — "finish a guitar track with Lion-Heart"
— had zero implementation. Musicians need two things: one-button recording, and
the ability to change the tone *after* the take (re-amp). Both fall out of one
fact the architecture already guarantees: every `Effect` is pure buffer-in /
buffer-out (RT-safe, device-free), so processing a file is the same code as
processing a stream.

## Decision

- **Two engine recording taps, no new `EngineMsg`.** `Chain` grew a `di_tap`
  (raw input at chain entry, before any slot) and a `wet_tap` (after the output
  stage — global EQ → safety limiter — but **before** the aux mix), each a
  stereo-interleaved `StereoTap` over an `rtrb::Producer<f32>`. They mirror the
  PRD 003 spectrum tap: lock-free, drop-on-full, installed with
  `Chain::set_record_taps` **before** the chain moves to the audio thread (like
  `set_output_tap`/`set_aux_input`), so recording start/stop is purely a
  control-thread + disk-thread concern — the audio thread is never re-plumbed.
  The wet tap sits before the aux mix on purpose: a recorded guitar take must
  carry **no metronome/drums/backing bleed** (the same guitar-only philosophy as
  the spectrum tap).

- **Armed flag + drop counter, so idle cost is one atomic load.** Both taps
  share an `Arc<RecordTapState>` { `armed: AtomicBool`, `dropped: AtomicU64` }.
  Disarmed, `StereoTap::write` returns after a single relaxed load — the ring is
  never touched, so the path is bit-transparent when not recording. Armed, it
  interleaves L/R into the ring and **counts the shortfall** when the ring is
  full (disk falling behind = a recording defect, surfaced to the UI, not a
  silent corruption). The rings are drained-and-reset at take start, so
  `dropped` measures only the current take.

- **Disk writer on its own thread; the audio thread only writes the ring.** A
  `Recorder` (app crate) owns the two tap consumers between takes. `start()`
  discards stale ring contents, opens two `hound` WAVs (timestamped
  `~/.lion-heart/recordings/<UTC>-di.wav` / `-wet.wav`), spawns a `lh-recorder`
  disk thread that drains both rings through a streaming writer, then arms the
  taps. `stop()` disarms, joins the thread (final drain + finalize), and reclaims
  the rings. `Recorder::drop` finalizes any in-progress take, so a teardown never
  leaves a truncated WAV.

- **WAV I/O in `lh-assets::wav`, shared and CI-testable.** `read` (interleaved
  `f32`, all channels, no resample), `write` (write-all, for render output),
  `WavStream` (streaming, for recording), and `WavBits` {16/24/float32, default
  24}. `lh-assets` already owned WAV/hound + the disk layout, so this is its
  natural home — and the sample round-trip is unit-tested without a device.

- **Offline `render` reuses the live chain, driven by hand.** A new
  `lion-heart render <di.wav> --preset <name> [-o out] [--tail secs]` subcommand.
  `crate::render::render` builds an **empty** `Chain` via `build_chain(vec![])`,
  reconciles it to the preset with the **exact same** `ChainHandle::apply_preset_chain`
  the live load uses (same reconciliation, same param application), mounts the
  preset's NAM/IR with the same `lh-assets`/`lh-nam` loaders, then pumps silent
  warm-up blocks to drain the control messages and settle fades before feeding
  the DI and a `--tail` of silence (so delay/reverb tails finish). Reusing the
  engine verbatim is the point — a render sounds like the live rig.

- **A render is reproducible: preset + DI only.** The app-global output EQ
  (`global_eq.json`) is **not** applied offline — it is environment, not tone, so
  a shared DI renders identically anywhere. The always-on safety limiter is
  intrinsic to `Chain` and still applies. The DI must already be at the engine
  rate (48 kHz); a mismatch is a hard error (`RateMismatch`), matching NAM's
  rate-lock policy rather than silently resampling.

- **App-global environment, not preset/plugin.** `recordings_dir` and
  `record_bits` live in `config.json` (like the metronome, ADR 020) — no preset
  schema bump. A take does **not** survive a device restart: the fresh session
  builds a fresh recorder, and the old one finalizes its WAV on drop (recording
  is a monitor feature, not carried like the click). The **plugin is unchanged**
  — a DAW is the recorder and re-amp host there (the ADR 013/020 precedent).

## Consequences

- White-paper success #1 is now reachable: record DI+wet, and re-amp the DI
  offline through any preset. PRD 016 (LUFS leveling) can reuse the `render`
  pipeline to measure a reference DI through each preset.
- The offline render inherits the engine's 256-message control-queue ceiling: a
  pathological 12-slot board of all-parametric-EQ pedals could overflow it —
  exactly as a **live** preset load of the same board would. Not a new limit.
- Very slow modulation (a rotary rotor's multi-second inertia) keeps spinning up
  into the first seconds of a render, just as it would on a live preset load.
  Accepted as faithful; a "settle fully first" mode is a possible v2.
- The wet take is post-output-stage, so it includes the user's live global EQ
  (what they heard) — while `render` excludes it (reproducible). The asymmetry is
  intentional and documented: a recording captures the moment; a render applies a
  preset.
- Zero audio-thread cost when not recording (one atomic load per block per tap);
  ~a ring write per block while recording. The disk thread does all I/O.
- Standalone-only: no plugin param-id change, so clap-validator is unaffected.

## Alternatives considered

- **A new `EngineMsg` to install taps at runtime** — rejected. The always-wired
  + armed-flag approach needs no audio-thread re-plumbing and matches the
  existing tap pattern (PRD 014 explicitly asked for "same as `set_output_tap`,
  no new EngineMsg").
- **Holding the whole take in RAM, writing on stop** — rejected. Streaming to
  disk keeps recording length unbounded; a few-second ring is all the RAM cost.
- **Re-amp as a GUI panel first** — deferred to v2 (PRD 014 non-goal). The
  offline CLI is the reproducible, CI-testable core; a live re-amp panel can wrap
  it later.

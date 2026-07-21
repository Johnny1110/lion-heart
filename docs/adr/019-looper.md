# ADR 019: Looper — an add-only chain-slot pedal with control-side transport

Status: **accepted — implemented**
Date: 2026-07-20
Relates to: PRD 013 (`docs/PRD/013-looper.md`), ADR 004 (per-pedal params),
ADR 005 (dynamic chain: install/remove), ADR 018 / PRD 012 (the `tempo.tap`
momentary idiom), white paper §3.1 (RT rules), §4.2 (click-freeness)

## Context

Every competitor bundles a looper (QC, Helix, AmpliTube); Lion-Heart lacked
one. The interesting constraints are all real-time: a hardware-grade loop
buffer is tens of MB, transport is a set of *momentary* actions, and the loop
seam must not click. The first roadmap item (M16) of the 2026-07-20 nine-
feature plan. The design goal was to add the feature **without touching the
engine or the session's message set** — everything a looper needs already
exists (dynamic slots give position-as-semantics for free; the param path
carries transport).

## Decision

- **The looper is a single-pedal family that is add-only** — buildable and
  offered in the "＋" menu, but *not* a member of `lh_core::DEFAULT_CHAIN`
  and *not* in the (host-driven) plugin chain. It is one of the add-only
  families that make the app's `FAMILY_REGISTRY` a proper superset of
  `DEFAULT_CHAIN`. The registry↔default-chain test was relaxed from "equal"
  to "the default chain is an in-order **subsequence** of the registry; the
  remainder are add-only families" — the pitch family (ADR 016) ships off the
  board too, ahead of the default slots rather than after them, so a strict
  prefix relation does not hold. The plugin builds `DEFAULT_CHAIN` only, so it
  never sees the looper (same standalone-only reasoning as spillover, ADR 013).

- **Transport is momentary params, edge-detected in the effect.** `rec`,
  `undo`, and `clear` are linear 0..1 params; a press is a value crossing 0.5
  upward, detected in `set_param` (the `tempo.tap` idiom, ADR 018). The state
  transition is O(1) bookkeeping, so it is RT-safe to run inline. The GUI /
  REPL / MIDI fire a press as a **1.0→0.0 pulse**: the SPSC ring is FIFO and
  never coalesces, so both edges survive in order, and the control shadow
  settles at 0 — a preset can never store a held button, so a load never
  re-triggers. `reverse`/`half` are stepped toggles; `level`/`mix` are the
  smoothed continuous knobs. **No engine or session message was added.**

- **Two 60-second stereo banks, allocated once in `prepare`** (~46 MB at
  48 kHz, ~92 MB at 96 kHz). Dynamic adds prepare the effect on the control
  thread (`ChainHandle::install_slot`) before the pointer-move `InstallSlot`,
  so the allocation never touches the audio thread — same path delay/reverb
  already use.

- **`clear`/`reset` are logical, not a `memset`.** Zeroing tens of MB in a
  process block would blow the budget. Instead they only reset `loop_len`;
  playback reads strictly inside `[0, loop_len)`, and recording overwrites
  from index 0, so stale samples past the (new) loop end are never audible.

- **One-level undo/redo is a bank-index swap**, not an audio-thread copy of
  the loop. The undo snapshot (the other bank) is filled lazily during an
  overdub's **first pass**, copying each pre-overdub sample *before* summing
  the new layer in place; it becomes valid only once that pass completes.
  `undo`/redo swap `play` between the two banks. Undo is gated to the
  `Playing` state (undoing mid-overdub is ill-defined for one level).

- **Playback is a single interpolated tap plus a smoothstep boundary fade.**
  The loop dips to zero over ~6 ms at each side of the wrap, so a recording
  whose start and end don't line up cannot click, while a lone tap keeps the
  loop faithful. A two-grain overlap was rejected: for a single stored loop it
  would blend two loop positions (a smear), not smooth one seam.

- **`reverse`/`half` are `Playing`-only read modifiers**; recording and
  overdubbing always run forward at an integer head, so overdub writes stay
  contiguous and sample-aligned. Overdub sums with a `tanh` soft clip (unity
  small-signal, `1/drive` ceiling), so infinite stacking stays bounded.

- **GUI state LED is a control-side mirror.** The effect owns the
  authoritative state on the audio thread; rather than tap it out of the
  engine, the session mirrors the one-button state machine from the same
  rec/clear presses it forwards (GUI, REPL, and the rising edge of a MIDI
  press). Best-effort: a pathological instant double-tap can drift the mirror
  by one step, which only mistints an LED — never the audio.

## Consequences

- **Preset schema is unchanged.** A looper is new *vocabulary*, like any new
  family: old presets simply don't reference it, and a looper-bearing preset
  stores its transport params as 0 (no re-trigger on load). No version bump.

- **Plugin ids unchanged** — the looper is standalone-only, so no id break and
  no clap-validator re-run for this milestone.

- The buffer is the memory cost of the feature (~46–92 MB per looper
  instance). Multiple loopers multiply it; the 12-slot cap bounds the worst
  case. Accepted as the price of a hardware-grade loop length.

- **Deferred to v2** (PRD 013 non-goals): tempo-quantized loop length (the
  global tempo from PRD 012 is already in place to hang it on), multi-level
  undo history, and exporting a loop to WAV (the recorder, PRD 014, is the
  natural home).

- Cost is trivial — ~0.65–1.03 µs per 64-frame block across record/play/
  overdub (`docs/benchmarks.md`), well under the PRD's 0.15 % target.

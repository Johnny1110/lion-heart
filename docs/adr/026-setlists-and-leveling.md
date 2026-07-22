# ADR 026: Setlists and per-preset loudness leveling

Status: **accepted — implemented**
Date: 2026-07-22
Relates to: PRD 016 (`docs/PRD/016-setlists-leveling.md`), ADR 023 (offline
render — the LUFS measurement reuses it), ADR 006 (output stage — where the
master trim lands), ADR 021 (procedural, no shipped audio binaries), M6 (MIDI
PC contract)

## Context

Two live-stage pain points: (a) presets have no song order — only a sorted
directory with prev/next; (b) presets jump in volume when you switch, the
perennial QC/Helix-forum complaint. PRD 016 bundles a **setlist** and an
offline **LUFS leveling** pass, sharing the ADR 023 render pipeline.

## Decision

Two independent features, both **control-side / offline** — the only real-time
change is one gain in the output stage.

### Setlists (pure control-side)

- `~/.lion-heart/setlists.json` = `{ active, lists: {name → [preset…]} }`
  (`app::setlist::Setlists`). App-global environment, never in a preset,
  absent from the plugin (the host's song/scene mechanism is the answer there).
- **Session-side override of the walk**: `preset_for_pc` and `adjacent_preset`
  consult the active setlist; with none active they fall back to the sorted
  directory verbatim. So the **MIDI PC cross-binary contract is preserved** —
  PC *n* → *n*-th sorted preset when no setlist is active (the plugin, which
  has no setlist concept, is unaffected); an active setlist just reroutes the
  standalone session's PC/prev/next.
- Nav math is pure and unit-tested (`setlist::{preset_at_pc, step, position}`):
  PC clamps to the list, prev/next clamp at the ends.
- Surfaces: a GUI setlist manager page (create / activate / delete / reorder /
  add-current), a preset-bar "▶ name · 3/12" indicator, prev/next that honor
  the active list; REPL `setlist [<name>|off|list|add <list> <preset>|delete]`.

### Loudness leveling (offline + one RT gain)

- **`lh_dsp::loudness::integrated_lufs`** — a from-scratch ITU-R BS.1770-4
  meter: K-weighting (a +4 dB shelf then a ~38 Hz high-pass, designed from the
  spec's physical parameters through the existing RBJ biquads, so it is
  rate-correct at 44.1/48/96 k), 400 ms/75 %-overlap mean-square blocks, and
  the two-stage −70 LUFS absolute / −10 LU relative gating. Pure, offline,
  CI-tested (a −18 dBFS sine reads within 0.7 LU of −18; +6 dB in → +6 LU out).
- **Output-stage master trim** — `EngineMsg::SetMasterTrim(gain)` drives a
  smoothed gain in `OutputStage`, **after the global EQ and before the safety
  limiter** (ADR 006 order: EQ → *trim* → limiter → tap). 0 dB is
  bit-transparent; an over-generous trim is still caught at −0.3 dBFS. This is
  the first output-stage signal change since PRD 003. `ChainHandle::
  set_master_trim_db` mirrors it; the session applies each preset's trim on
  load.
- `~/.lion-heart/levels.json` = `{ target_lufs, trims: {preset → dB} }`
  (`app::leveling::Levels`), app-global like `global_eq.json` — environment,
  not tone, so **no preset schema bump** and nothing in the plugin.
- **`lion-heart level [--preset <n>|--all] [--target -18] [--reference
  <di.wav>] [--dry-run]`** renders a reference DI through each preset (reusing
  ADR 023's `render`), measures LUFS, computes `trim_for = target − measured`
  (clamped ±12 dB), and writes `levels.json`.

## Deltas from PRD 016

- **The built-in reference DI is synthesized, not a shipped recording.** Six
  decaying open-string plucks (E2–E4) at a −8 dBFS DI level, deterministic.
  Shipping a recorded-guitar binary was rejected on the ADR 021 precedent; for
  *relative* leveling the same synthetic yardstick through every preset
  measures the spread just as well. `--reference` overrides it with a real DI.
- **The manual trim control lives on the setlists/leveling page, not the
  audio-settings page** the PRD sketched. Loudness is conceptually part of the
  live/leveling surface, and it kept the change off the device-settings draft.
- **Setlist reorder is ▲▼ buttons, not drag-and-drop** (a v1; the chain-strip
  drag gesture could be adopted later).
- **MIDI footswitch prev/next**: PC already walks the active setlist, which
  covers absolute-PC footswitches. A dedicated increment `preset.next/prev`
  virtual-CC target (the `tempo.tap`/`snapshot.select` pattern) is deferred.

## Consequences

- **Engine**: one new `EngineMsg` + a smoothed gain; RT cost is one multiply
  per sample in the output stage (nil when settled at unity — `x * 1.0` is
  exact). **Plugin unchanged** (no setlists/levels there; the master trim is
  not exposed — hosts own loudness), so no param-id change, clap-validator
  unaffected.
- **The LUFS meter is reusable** — a future real-time loudness readout or a
  normalized export can call the same `integrated_lufs`.
- App-global, disk-backed state (`setlists.json`, `levels.json`) is reloaded
  on every `Session::start` and saved on every mutation, so a device restart
  preserves it without threading through `CarryOver`.
- Missing NAM/IR assets make a preset render quieter — `measure` surfaces the
  render warnings so `level` can flag a skewed measurement rather than writing
  a wrong trim silently.

## Alternatives considered

- **Per-preset loudness stored in the preset.** Rejected: loudness matching is
  environment (the room/PA), not tone — the same reasoning as the global EQ
  (ADR 006). Keeping it in `levels.json` also avoids a schema bump and keeps it
  out of the plugin.
- **A real-time LUFS meter driving automatic normalization.** Out of scope
  (PRD §3): this is a static, measured, per-preset offset the user can tweak,
  not playback-time gain-riding.

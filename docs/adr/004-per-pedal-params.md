# ADR 004: Per-pedal parameters — families, knob memory, preset schema v3

Status: **accepted — implemented**
Date: 2026-07-17
Relates to: PRD 001 (`docs/PRD/001-per-pedal-knobs.md`), ADR 003 (partially
superseded), white paper §4.3

## Context

ADR 003 gave the drive slot one shared param table plus a stepped `model`
param, chosen for plugin param-list stability. The evva (five knobs, 3-band
EQ) broke the shape: its Low/Mid/High entered the shared table, so every
3-knob pedal inherited them (hidden in the GUI by a per-model caption
special-case; visible everywhere else), evva carried a dead `tone` knob, and
knob *values* travelled across model switches because the smoothers were
slot-level. The user requirement (PRD 001) is the opposite: every pedal owns
its faceplate, and every pedal remembers its own settings.

## Decision

- **`FamilyDesc` in lh-core**: a chain slot is a *family* of 1..N pedals;
  `EffectDesc` becomes the per-pedal descriptor (own params, captions,
  defaults, ranges). Single-pedal effects are one-pedal families. amp/cab
  stay asset-driven single-pedal families (PRD non-goal).
- **`Effect` grows `family()` / `pedal_index()` / `select_pedal()`**;
  `descriptor()` returns the active pedal's desc, and `set_param` indexes
  into it. Switching stays RT-safe exactly as in ADR 003: circuits/voices
  preallocated, switch = index change + incoming-pedal state reset.
- **Knob memory lives in the control-side shadow** (`ChainHandle`:
  slot → pedal → norms). A pedal switch sends `SelectPedal` followed by the
  incoming pedal's values from the shadow; the engine never carries values
  across pedals and effects hold no per-pedal value state. SPSC ordering
  makes the switch+restore burst race-free.
- **Preset schema v3**: a slot stores `pedal` (selection) and `pedals`
  (per-pedal value maps — the whole memory survives save/load). v2 → v3
  migration maps `model`/`type` indices to pedal keys via
  `preset::DRIVE_PEDALS` / `preset::MOD_PEDALS` (pinned to the registries by
  lh-dsp tests), renames shared keys onto each faceplate (bd2 `gain ←
  drive`, centaur `gain/treble/output`, evva drops the dead `tone`), and
  folds tremolo's redundant `mix` into `depth` (`depth' = depth × mix`,
  audibly exact). v1 files chain through the existing v1→v2 migration —
  the "old presets sound identical" guarantee is preserved end to end.
- **Modulation becomes four pedals** (chorus/flanger/phaser/tremolo);
  tremolo's faceplate is rate/depth and its output is wet-only.
- **Virtual `pedal` selector**: `slot.pedal` addresses selection from
  REPL/MIDI (`set drive.pedal ts9`, CC → nearest index); `model`/`type`
  accepted as aliases for existing `midi.json` files.
- **Plugin**: multi-pedal slots statically expand *every* pedal's params
  (`drive_ts9_drive`, `drive_evva_low`, …) plus a stepped `{slot}_pedal`
  selector. Host state per pedal is the knob memory; a selector change
  re-sends the incoming pedal's host values. Single-pedal param ids are
  unchanged; drive/mod ids change once (pre-v0.1 break, as ADR 003 allowed).

## Consequences

- Adding a drive pedal = implement `Circuit`, declare its faceplate, append
  one desc + one `ModelDef`. GUI knobs/dropdown, REPL, MIDI, preset I/O and
  plugin params all pick it up from the registry.
- TS9 shows exactly three knobs, evva exactly five; switching pedals
  restores each pedal's own values (engine-tested; preset round-trip keeps
  the whole memory).
- `ChainHandle` grew the per-pedal shadow and selection API; `descriptors()`
  became `families()` (all callers migrated).
- Param index spaces are per-pedal, so host automation of an unselected
  pedal's knob intentionally does nothing until that pedal is selected.
- The registry keeps ADR 003's append-only rule, now doubly load-bearing:
  migration tables and plugin ids reference pedals by position and key.

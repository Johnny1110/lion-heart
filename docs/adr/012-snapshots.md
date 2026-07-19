# ADR 012: Snapshots — per-preset scenes with morph, engine untouched

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: PRD 009 (`docs/PRD/009-snapshots.md`), PRD 001 (per-pedal
values), PRD 002 (dynamic chain), white paper §7 (M8+ "snapshot morphing"),
§4.1

## Context

Live playing wants verse/chorus/solo variations of *one* board — nudge a
few knobs, toggle a boost — without the weight of a preset switch
(reconcile, asset remount, full chain rebuild, and a severed tail). The
white paper parked "snapshot morphing" in the M8+ deep-water list; this
lands it, and it lands cheaply because the value-change plumbing
(`SetParam`/`SetActive` through the smoothing layer) already exists.

## Decision

- **A snapshot is a value+bypass overlay on the preset's fixed board**, not
  a new structure. Per chain slot (addressed by handle — `drive`,
  `drive2`), it stores `active` and the **selected pedal's** param values
  (real, as presets always store). It deliberately cannot change pedal
  selection, slot structure, or mounted assets — the "two drive tones"
  case is served by two drive slots (ADR 005), and storing only the
  selected pedal's values enforces that structurally.
- **Switching is control-side only** — the engine gets nothing new. A
  switch diffs current-vs-target and emits `SetParam` for changed values
  and `SetActive` for changed bypass. No `SetOrder`/`InstallSlot`/
  `RemoveSlot`, so delay/reverb tails ring straight through a scene change
  and every value move is declicked by the effect's own smoother. This is
  the M9/M10 lesson again: the multi-value path already covered us.
- **Morph** (`morph_ms`, app-global in config.json, 0–2000, default 0): at
  0 the switch is one batch (effects declick it); above 0 the session
  interpolates each changed param's **normalized** value from current to
  target over the window, ticked on the GUI/REPL loop — a scene change
  becomes an audible sweep (a filter opening, a mix rising). Norm-space
  interpolation keeps log-ranged params (times, frequencies) musical.
  `active` flips at morph start (the engine's `SetActive` crossfades it).
  The morph math (diff → steps, lerp, convergence) is a pure, unit-tested
  unit; the session wires it to the live `ChainHandle`.
- **Schema v6**: `Preset` gains `snapshots: BTreeMap<"A".."D", Snapshot>`
  (sparse) and `active_snapshot: Option<String>`, both `#[serde(default)]`.
  A v5 file has neither → empty → the baseline chain is the only state, so
  old presets load and sound identical. The version bumps to 6 (rather
  than relying on unknown-field tolerance) so an older build rejects a
  snapshot-bearing file with the standard "update Lion-Heart" message
  instead of silently dropping scenes — the schema gate exists precisely
  to prevent that silent divergence.
- **A switch desyncs soft-takeover** (`midi_desync_all`, PRD 008): scene
  values move out from under the pedals, so pickup controllers must
  re-engage.
- **GUI**: four A–D chips in the preset bar — click switches, ⌥-click
  captures, the active chip glows amber, populated chips read solid /
  empty dashed, and a dirty dot marks current-values-drifted-from-scene.
- **MIDI/REPL**: a virtual `"snapshot"` CC target (value quartered → A–D);
  REPL `snapshot <A-D>` / `snapshot save <A-D>` / `morph <ms>`.

## Consequences

- **The plugin does not get snapshots in v1.** Host automation lanes are
  the DAW's own scene mechanism and collide semantically with an internal
  scene selector; standalone-first keeps the model clean. Revisit if a
  host workflow demands it.
- `Snapshot`/`SnapshotSlot` live in lh-core (plain data, shared by preset
  I/O and the session). `ChainHandle::capture_scene()` reads the active
  pedal's shadow into a `Snapshot`; applying is the session's existing
  `set_param`/`set_active` calls, so the engine's public surface is
  unchanged.
- Snapshots are keyed by handle, so a chain reorder within a session keeps
  scenes aligned to the right slots; a snapshot referencing a handle the
  board no longer has is skipped (a slot removed after the scene was
  stored), matching the preset applier's forward-compat stance.
- `morph_ms` is app-global (an environment/feel preference like the global
  EQ), not per-preset — a player picks one glide feel for the rig. A
  per-preset override and a GUI slider are possible follow-ups; v1 sets it
  via config.json and the REPL.

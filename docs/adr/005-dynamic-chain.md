# ADR 005: Dynamic chain — slot instances, hot install/remove, structural presets

Status: **accepted — implemented**
Date: 2026-07-17
Relates to: PRD 002 (`docs/PRD/002-chain-builder.md`), white paper §4.2, ADR 004

## Context

The chain was a fixed ten-slot set: reorderable and bypassable, but the slot
*set* was decided at build time. PRD 002 wants a user-built board — add,
remove, drag, same family several times (three drives in a row) — persisted
in presets. White paper §4.2 already modelled the chain as a reorderable
`Vec<EffectSlot>` and §4.3 reserved instance semantics; this ADR implements
that, replacing the fixed-set simplification.

## Decision

- **Slot table with holes**: `Chain.slots: Vec<Option<Slot>>` at fixed
  capacity `MAX_SLOTS = 12`; `order` references occupied indices. An index
  is an instance's stable identity while it lives.
- **Two new messages, one chute**:
  - `InstallSlot { index, effect }` — the effect is **built and prepared on
    the control thread**; the engine pointer-swaps. Installs apply
    immediately and are silent by protocol: the control side only targets
    indices outside the audible order (the fresh index enters the order via
    the accompanying faded reorder). An install into an index with a still
    pending removal cancels that removal (the re-install supersedes it) and
    retires the occupant.
  - `RemoveSlot { index }` — deferred to the bottom of the master fade,
    applied *after* the pending order lands, so an occupant never vanishes
    from an audible chain. Removed/replaced effects travel back on a
    **retire ring** (garbage chute, white paper §4.1) and die on the control
    thread; a bounded preallocated parking list absorbs a full chute.
  - Everything rides the existing ORDER fade: one edit = one dip through
    silence; untouched slots keep their state, so delay/reverb tails survive.
- **Instance handles**: control-side addressing is `family` /
  `family<N>` by 1-based rank in chain order (`drive`, `drive2`). Handles
  follow chain order (moving the third drive first makes it `drive`) —
  MIDI maps address the *board position*, like a hardware switcher.
  `ChainHandle` grew `install_slot` / `remove_slot` / `move_position` /
  `order_handles` / `collect_garbage`, and needs the stream rate
  (`set_sample_rate`) to prepare later installs.
- **Presets define structure**: `apply_preset_chain(states, build)` claims
  surviving same-family instances first (their state and tails carry over),
  removes leftovers, and installs what's missing through a **factory
  closure** — the session provides it, since only the app links every
  effect crate and owns the amp/cab asset seams. Slots a preset doesn't
  mention are now removed (previously kept at the end); real presets always
  listed every slot, so only hand-edited files can notice.
- **Constraints**: 12 slots total (message-format bound, one constant);
  `amp`/`cab` at most one instance each — they mount the session's single
  NAM/IR asset handles. Adding one rebuilds the handle pair and re-applies
  the loaded asset. Empty chains are legal (passthrough).
- **Limiter is no longer forced last** and may be removed; white paper
  §3.3's always-on output protection moves to the output stage safety
  limiter (PRD 003 / ADR 006).
- **Plugin unaffected**: it keeps the fixed default chain (host param lists
  cannot change shape at runtime); a structural plugin story is a future
  PRD.

## Consequences

- The GUI chain strip became the board editor (drag to move, ＋ menu to
  add, remove in the params panel); the REPL gained `add`/`remove`, and
  `order` takes instance handles.
- One benign race is documented and engine-neutralized: a rapid
  remove-then-reuse of the same index is resolved by install cancelling the
  pending removal; the control side also allocates free indices round-robin
  to keep the window tiny.
- Engine tests cover install/remove mid-stream, fade-through-silence on
  edits, handle re-ranking, capacity, structural preset apply with a
  counting factory, and chute collection.
- `ChainHandle::families()` returns per-instance entries now; the plugin
  passes `&handle.families()` unchanged in shape because the fixed chain
  has unique families.

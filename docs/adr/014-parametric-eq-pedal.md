# ADR 014: The eq slot becomes a two-pedal family (parametric)

Status: **accepted — implemented**
Date: 2026-07-20
Relates to: PRD 011 (`docs/PRD/011-parametric-eq-pedal.md`), ADR 004
(per-pedal params), ADR 005 (dynamic chain), ADR 006 (output-stage global
EQ), ADR 007 (delay: the single-pedal→family precedent)

## Context

The chain `eq` slot is a deliberately minimal 3-band tone pedal; the user
wants the global EQ's **visual 8-band parametric editor at any chain
position** (and possibly several of them). Candidate shapes: grow the
3-band pedal, open a new family, or append a second pedal to the existing
`eq` family.

## Decision

- **Append `parametric` to the `eq` family.** The 3-band keeps its pedal
  key `eq` — no rename, no preset migration, **no schema bump** (append-only
  vocabulary, the M11/M12 rule). "Anywhere in the chain" and "more than
  one" already exist: slots are movable family instances (ADR 005).
- **The DSP is `GlobalEq`, reused whole.** `eq/parametric.rs` is a 40-param
  façade (8 bands × `on`/`type`/`freq`/`gain`/`q`; kinds pinned to
  `BandKind::ALL`, defaults = the global default layout) over one
  `GlobalEq` core — per-band wet crossfades, log-domain freq smoothing,
  settled-skip rebuilds, and the all-off bit-transparent fast path are
  inherited, not rewritten. Slot bypass stays the engine's crossfade; the
  core's master is pinned at 1.0 and not exposed.
- The 3-band demotes to `eq::chain::Tone` (inherent methods); the family
  dispatcher takes the old name **`eq::Eq`**, so session/plugin imports
  and the bench harness did not change.
- **GUI: one canvas, two targets.** `EqPanel` gained `EqTarget::{Global,
  Slot(handle)}`. A slot panel builds its band model from the slot's param
  mirror (`bands_from_reals`) and publishes slot-param edits
  (diff-and-send — a drag costs knob-drag traffic); the global panel keeps
  its persistence semantics. The spectrum overlay is still the
  output-stage tap — slot panels tag it "OUT" rather than pretending it is
  a per-slot probe (per-slot taps stay out of scope).
- **Plugin: pre-v0.1 param-id break** (M9 precedent). The id scheme
  qualifies multi-pedal slots with the pedal key, so `eq_low` →
  `eq_eq_low`, and `eq_parametric_*` + the stepped `eq_pedal` selector
  appear. Re-run clap-validator.

## Consequences

- Engine, session, preset code: **zero changes** — the multi-pedal path
  covered everything; the registry entry just points at the new family
  static.
- 40 params is the largest faceplate in the app; the board renders the EQ
  canvas instead of a knob row for it. REPL/MIDI address bands as
  `eq.b3_freq`; MIDI learn works through the REPL (`learn eq.b3_freq`) —
  canvas handles have no right-click learn affordance yet (possible
  follow-up on the detail strip).
- Settled cost matches the output stage by construction:
  `eq_parametric_4band` ≈ `global_eq_4band` (~1.45 µs vs ~1.46 µs on the
  same box, 4 bands live). Twelve slots of it would still be <1.5 % of the
  deadline.
- Old presets load the eq slot as the 3-band with their values untouched;
  the parametric's stored defaults are flat (bit-transparent), so
  selecting it before touching anything changes nothing audibly.

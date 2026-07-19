# ADR 011: Expression pipeline — manual wah, CC shaping, MIDI learn

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: PRD 008 (`docs/PRD/008-expression-pedal.md`), ADR 010 (filter
family), white paper §7 M6 ("CC 映射、expression（wah/volume）")

## Context

M6 shipped Program Change → preset and a raw CC table (`midi.json`:
controller → `slot.param`, 0..127 mapped linearly onto the full 0..1
range). The expression half of the M6 promise stayed open: there was no
manual wah to point a pedal at (PRD 007 explicitly deferred it to "the
expression architecture"), no per-mapping range/taper/inversion, no way to
bind without hand-editing JSON, and a hardware pedal whose physical
position disagrees with the parameter after a preset switch caused value
jumps on first movement.

## Decision

Four pieces, all control-side or offline-testable DSP — the engine is
untouched (`SetParam` through the smoothing layer was built for exactly
this traffic):

- **Manual `wah` joins the filter family** (append-only: no preset schema
  bump, plugin params auto-expand as `filter_wah_*` — another pre-v0.1
  param-id addition). `filter.rs` becomes `filter/` (mod.rs shared engine +
  one file per pedal, the delay-family `Ctl`-table pattern). The wah drops
  the envelope follower and `direction`; a smoothed `pos` param (25 ms —
  absorbs 7-bit CC staircases) sweeps 350 Hz–2.2 kHz geometrically into
  the same per-channel soft-clipped Chamberlin SVF. Faceplate:
  `pos`/`q`/`mode`/`mix`, defaults voiced vocal (q 6, lowpass, full wet).
- **CC mappings grow shaping**, backward compatible via serde-untagged:
  a `cc` entry is either the legacy string or
  `{ target, min, max, curve, pickup }`. `min`/`max` (defaults 0/1) bound
  the normalized landing zone, min > max inverts, `curve` is `linear` |
  `audio` (x², the log-taper feel for volume), `pickup` opts into
  soft-takeover. Shaping applies to continuous targets only; bare-slot
  bypass keeps the value ≥ 64 rule.
- **Soft-takeover lives in the session** (control thread): a mapping
  desyncs on preset load, on that slot's pedal switch, and when the GUI
  moves the same param; it re-engages when the shaped CC crosses the
  current value (or lands within ±0.02), and is silent until then. The
  audio thread never sees any of this — only post-engage `SetParam`s.
- **MIDI learn in the GUI**: right-click a knob arms it; the next CC (on
  the configured channel) binds and is written back to
  `~/.lion-heart/midi.json` (input/channel/pc_presets preserved; learned
  entries use the simple string form). Bound knobs wear a badge with the
  CC number; the params panel shows an armed banner with cancel/clear.
  The jam REPL gets `learn`/`unlearn <slot.param>`. Learn overwrites an
  existing binding for that controller and reports what it displaced.

## Consequences

- The white-paper M6 expression item is closed; the hardware pedal the
  user bought for M6 becomes fully usable (wah, volume, any knob).
- lh-midi's `MidiMap::cc` value type changes (string → string-or-object).
  Old files parse unchanged; files written after a learn stay readable by
  older builds only if no shaped entries exist — acceptable pre-v0.1.
- `Action::SetParam` carries a `pickup` flag; the session owns pickup
  state keyed by controller number. A GUI knob drag notifies the session
  so pickup desyncs — a one-line hook in the knob message handler.
- The filter family is now 2 pedals: the registry test, the theme's
  distinct-livery test (filter added to its family list), and the plugin
  chain build (`Filter::new()` replacing `AutoWah::new()`) all move with
  it. Bench: `filter_wah` lands alongside `filter_autowah`
  (~1.2 µs / 64-frame block — no follower, but the same per-sample
  exp+sin sweep and SVF).
- Not done, deliberately: 14-bit CC (7-bit + smoothing is already
  staircase-free), plugin-side learn/pickup (host automation is the
  plugin answer), and a GUI shaping editor (min/max/curve stay
  hand-edited JSON for now).

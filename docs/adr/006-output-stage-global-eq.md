# ADR 006: Output stage — global EQ, safety limiter, spectrum tap

Status: **accepted — implemented**
Date: 2026-07-17
Relates to: PRD 003 (`docs/PRD/003-global-eq.md`), white paper §3.3, ADR 005

## Context

PRD 003 wants a DAW-style parametric EQ on the final output — environment
correction (room / monitors / recording chain), not tone — with a draggable
band editor overlaid on a live output spectrum. Independently, ADR 005 made
the chain limiter optional, which orphaned white paper §3.3's "no patch,
setting, or bug may slam the monitors" guarantee.

## Decision

- **A fixed output stage inside `Chain`**, after the slots and the master
  fade: `global EQ → safety limiter → spectrum tap`. Not a chain slot; the
  `eq` pedal stays tone shaping.
- **Global EQ** (`lh_dsp::param_eq::GlobalEq`): 8 bands driven by
  `lh_core::global_eq` state (low-cut / low-shelf / bell / high-shelf /
  high-cut; 20 Hz–20 kHz log, ±18 dB, Q 0.3–18). Freq (log-domain), gain,
  and Q ride smoothers with block-rate coefficient rebuilds (the chain EQ's
  proven pattern); every band and the master toggle have wet crossfades, so
  enables/disables engage smoothly even for cut filters. Fully settled off,
  the stage is **bit-transparent** (engine-tested).
- **Safety limiter**: the existing `Limiter` DSP at a fixed −0.3 dBFS
  ceiling, always on, not user-visible. §3.3's guarantee now lives at the
  output; EQ boosts land under the ceiling by construction. The limiter's
  release gained a snap at the last ~−120 dB so post-over transparency is
  exact instead of asymptotic.
- **State & addressing**: `EngineMsg::SetEqBand` carries a whole `Band`
  (Copy) — one message per band edit, no field-index protocol.
  `ChainHandle` keeps a `GlobalEqState` shadow. State persists in
  `~/.lion-heart/global_eq.json`, loaded at session start and written at
  commit points (drag release, toggles, resets). **Deliberately not in
  presets**: presets are tone, the output EQ is the environment — switching
  presets never touches it.
- **Spectrum**: the output stage writes a post-stage mono sum into an rtrb
  ring (drop-on-full, the tuner-tap pattern). The GUI drains it every
  frame and FFTs on the GUI thread (`realfft`, 4096-point Hann, ~30 Hz
  updates, fast-attack/slow-release bins). The response curve is computed
  from the same RBJ math as the audio path (`param_eq::response_db` /
  `Biquad::magnitude_db`), so the drawn curve is the truth.
- **GUI**: an "eq" chip opens the editor — log-frequency canvas, spectrum
  fill + response curve overlay, draggable handles (freq/gain), wheel for
  Q, double-click to toggle, a detail strip for type/readouts/flat/master.
- **Not in the plugin** (hosts bring their own EQ and analyzers).

## Consequences

- The output stage costs ~a limiter plus 0–8 biquads per channel per block;
  disabled bands are skipped entirely, and the whole stage early-outs to
  bit-exact passthrough when off (`global_eq_4band` criterion bench tracks
  it).
- Existing behavior changes only at the margin: output now hard-tops at
  −0.3 dBFS where previously a cranked chain with the limiter removed (or
  its ceiling raised) could exceed it.
- lh-dsp tests pin each band type's response, curve-vs-audio agreement,
  click-free engagement, transparency, and multi-rate behavior; engine
  tests cover the stage end-to-end plus the tap; the analyzer has dBFS
  calibration and decay tests.

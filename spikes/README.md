# M4 GUI spike — iced vs vizia

The timeboxed comparison required by white paper §5.5 before committing to a
GUI framework. **Outcome: iced — see [ADR 001](../docs/adr/001-gui-framework.md).**

The same screen, twice: a custom-drawn rotary knob bound to `drive.drive` on
the real engine chain, and realtime IN/OUT peak meters targeting 60 fps.

- `common/` — shared driver: the real `lh-engine` chain (gate → drive → delay)
  paced at 48 kHz on a worker thread over a synthetic guitar pluck. Both UIs
  talk to it exclusively through `ChainHandle` (param messages) and `Telemetry`
  (atomic peaks) — the exact plumbing the product UI uses. No audio device
  needed.
- `iced/` — iced 0.14: `canvas::Program` widgets, `window::frames()` meter loop.
- `vizia/` — vizia 0.4: custom `View`s with skia drawing, signal bindings,
  16 ms timer meter loop.

This is a **separate cargo workspace**, excluded from the root workspace on
purpose: the GUI dependency trees (wgpu, skia) must never leak into engine
builds or CI.

## Run (macOS)

```sh
cd spikes
cargo run -p spike-iced --release
cargo run -p spike-vizia --release
```

What to check: the fps readout holds ~60 (display refresh) while a pluck loops
through the chain; dragging the knob feels smooth and the value text tracks;
meters move with instant attack and steady fall.

Linux needs system libs for the *vizia* build only:
`libwayland-dev libgles-dev libegl-dev libfontconfig1-dev libfreetype-dev`.

Once the Mac check confirms the ADR, this directory can be deleted (or kept as
a widget reference for the product UI).

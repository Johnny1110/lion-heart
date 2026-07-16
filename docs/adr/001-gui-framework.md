# ADR 001: GUI framework — iced over vizia

Status: **accepted** (pending final feel/fps confirmation on macOS hardware)
Date: 2026-07-16
Relates to: white paper §5.5 (GUI technology, spike plan), milestone M4

## Context

The white paper names iced as the primary GUI candidate and requires a timeboxed
spike before committing: the same screen — a custom-drawn rotary knob bound to a
live engine parameter, plus realtime peak meters at 60 fps — implemented in both
**iced** and **vizia**, with the loser documented in an ADR. iced was the default
answer; vizia had to prove itself.

The spike lives in `spikes/` (its own cargo workspace, excluded from the root
workspace so GUI dependency trees never leak into engine builds or CI). Both UIs
bind to the *real* engine plumbing through `spike-common`: the actual
`lh-engine` chain (gate → drive → delay) paced at 48 kHz on a worker thread,
controlled via `ChainHandle` messages, metered via the `Telemetry` atomics —
exactly the topology the product UI will use.

Versions spiked: `iced 0.14.0` and `vizia 0.4.0` (both the current crates.io
releases), on Linux/aarch64 (compile + API evaluation) — visual behavior and
frame pacing are verified on the target Mac.

## What the spike showed

### Rendering & dependency stack

- **iced**: pure Rust end to end (wgpu → Metal on macOS, tiny-skia fallback).
  Compiled in the container with **zero additional system libraries**.
- **vizia**: renders through **skia-safe** — a large C++ library consumed as a
  build-time prebuilt binary download, plus OpenGL via glutin. On Linux it
  needed `libwayland-dev`, `libgles-dev`, `libegl-dev`, `libfontconfig1-dev`,
  `libfreetype-dev` installed before it would link. On macOS, glutin means
  **OpenGL (CGL), deprecated by Apple since 10.14** — the wrong horse for a
  macOS-first product, and a C++ blob complicates future codesign/notarization.
  Dependency counts: 334 (iced) vs 374 (vizia) crates.

### Framework stability

- vizia 0.4 **replaced its entire state system** (the Lens/Model architecture
  its docs, examples, and downstream users like Meadowlark are written against)
  with global reactive signals (`Signal`/`Memo`/`Effect`). The new model is
  pleasant — fine-grained redraw on signal change suits meters well — but the
  built-in views still mix old and new idioms, and almost all community
  knowledge targets the old API. That is high churn risk for a side project
  that will live with the choice for years.
- iced 0.14's API also moved (0.13 → 0.14 changed `application()` and canvas
  `Program::update`), but incrementally, with the ecosystem tracking it.

### Custom-widget cost (the core of an audio UI)

- **iced**: implement `canvas::Program` — an associated `State` type holds drag
  state, `update()` handles events, `draw()` produces geometry with per-widget
  `Cache` (knob redraws only on value change; meters clear every frame).
  Text inside a widget is one `frame.fill_text(..)` call — the knob's value
  readout and the meters' dB ruler were trivial. One real wart: a
  higher-ranked-lifetime inference trap made `.theme(|_| ..)` closures fail to
  compile (fixed by using a named method instead of a closure).
- **vizia**: implement `View` with `event()`/`draw()` — also reasonable, and
  its CSS/styling/accesskit integration is genuinely richer. But: drawing
  **text** in a custom `draw()` requires skia font plumbing (the idiom is to
  compose `Label` views instead — our meter ruler lost its numbers); handle
  modifiers from app code hit the **orphan rule** (vizia's own views use
  inherent impls on `Handle`, app crates must define extension traits); and
  several context internals are crate-private in ways the built-in views don't
  have to respect. vizia ships a built-in audio `Knob`, which is attractive,
  but the product needs custom-drawn widgets either way.

### Frame loop

- iced: `window::frames()` delivers a per-frame tick aligned with actual
  redraws — the meter poll rides the compositor.
- vizia: a 16 ms `add_timer` callback emits into the reactive graph; only
  bound views redraw. Equivalent effort; different but equally workable.

## Decision

**iced** is Lion-Heart's GUI framework, confirming the white paper's default.
vizia did not prove itself: the skia-safe C++ dependency and OpenGL-on-macOS
backend contradict the project's pure-Rust, macOS-first stance, and the 0.4
state-system rewrite makes its API a moving target. Its real advantages
(audio-native widgets, CSS styling, fine-grained reactivity) don't outweigh
that for us.

`egui` remains sanctioned for internal dev tools only (white paper §5.5).

## Consequences

- M4 product UI is built with iced 0.14 in `app/lion-heart`: chain view,
  custom knobs, NAM/IR browsers, preset browser, meters, tuner.
- The engine↔UI pattern validated by the spike carries over: the UI owns
  `ChainHandle`/asset handles, mutates them in `update()`, polls `Telemetry`
  via `window::frames()`, and never blocks the audio thread.
- Custom widgets standardize on `canvas::Program` with per-widget `Cache`;
  known wart: avoid closures for `application()` builder callbacks that take
  `&State` (use named methods) to dodge the HRTB inference trap.
- The spike stays in `spikes/` (excluded from CI) until the Mac-side check —
  run both, confirm iced holds 60 fps with no added xruns — then `spikes/` can
  be deleted or kept as reference. If iced's render cost on the Mac proves
  unacceptable (< 60 fps on this trivial screen), this decision reopens.
- Revisit triggers: iced stalls as a project; wgpu becomes a problem on macOS;
  or vizia 0.4+ stabilizes, moves off deprecated GL on macOS, and the plugin
  milestone (M7) demands vizia's plugin-friendly windowing.

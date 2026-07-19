# ADR 015: Dual-IR cabinet — a blend of two mics

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: white paper §2 (tone core = NAM + cab IR), §5.4 (fft-convolver),
ADR 003 (asset-swap seam), the preset schema (`docs/PRD` — assets)

## Context

The cab convolved with a **single** IR. Real cabinet recordings blend two
mics (e.g. an SM57 on the cone + a ribbon off-axis); the mic mix is a primary
tone-shaping control on pro IR loaders. Since the tone core is explicitly
"NAM + IR", deepening the IR side with a two-mic blend is high-value and lands
squarely on the project's stated heart.

## Decision

- **`IrAsset` holds two IRs**: a primary `a: IrPair` and an optional blend
  `b: Option<IrPair>` (each `IrPair` is the stereo convolver pair). A new
  **`blend` param** (Linear 0..1, default 0 = all `a`) crossfades between them.
- **Linear crossfade** `out = a·(1−blend) + b·blend`, not equal-power: the two
  IRs are the *same source* through different mics — highly correlated — so a
  linear mix keeps the level roughly constant across the knob (identical mics
  sum to unity at any blend) while the mic *difference* (top end + the comb
  between them) sweeps. The blend and level trajectories are snapshotted once
  per block and shared by both stereo channels so L/R stay in lockstep.
- **One asset handle, one hot-swap.** `CabIr::new()` is unchanged (its handle
  is wired by the shared family-builder — changing its arity would ripple the
  signature across all 11 families). The control side owns both IR files and
  **composes the combined `IrAsset`**: `lh-assets::load_ir_pair` decodes one
  IR into an `IrPair`; the session's `rebuild_cab` reloads whichever refs are
  set (`ir_ref` + `ir_b_ref`) and installs the pair together. Changing one IR
  re-decodes both — cheap, control-thread, and rare.
- **Preset schema v7**: `assets.ir_b: Option<AssetRef>`, `#[serde(default)]`.
  A v6 file is a v7 single-mic cab (loads and sounds identical). The version
  bumps so an older build rejects a dual-IR preset instead of silently
  dropping the second mic (same discipline as v6 scenes). `ir_b` rides
  `CarryOver` across a device restart.
- **Surfaces**: REPL `load ir_b <wav>` / `unload ir_b`; the GUI cab faceplate
  shows **MIC A** and **MIC B** rows (a new `AssetKind::IrB` routes the
  browser/unload) with the auto-rendered `blend` knob between them. A blend IR
  requires a primary (`load_ir_b` errors without one); unloading the primary
  clears both. The plugin loads both from a preset and exposes `blend` as an
  automatable param.

## Consequences

- **~2× cab CPU when a blend IR is loaded** (two convolver pairs instead of
  one): the single-mic cab was ~3.5 µs/block, so dual is ~7 µs (~0.5 % of the
  1333 µs deadline). Single-mic cabs (b = None) pay nothing extra — the blend
  branch is skipped. No new benchmark row (the cost is just 2× an existing
  measured node).
- **RT-safe**: all scratch (dry + two wets + two control buffers) is allocated
  in `prepare`; `process` allocates nothing (`assert_no_alloc` stays green).
- **Tested offline**: `blend_crossfades_between_the_two_irs` (two delta IRs at
  different taps — blend 0/0.5/1 lands the energy where expected) and
  `blend_is_inert_without_a_second_ir`; the preset round-trip covers `ir_b`.
  The mic mix itself is an ear check on hardware.
- **Plugin `blend` id** appears (`cab_blend`) — pre-v0.1 additive break, re-run
  clap-validator. The plugin can't *load* a blend IR interactively (assets come
  from presets there, by design); save a dual-IR preset in the app, pick it in
  the host, automate `blend`.
- The blend IR is a mix control, **not** a second cab slot: one cab, two mics,
  one convolution stage summed. A second independent cabinet would be a second
  chain slot instead.

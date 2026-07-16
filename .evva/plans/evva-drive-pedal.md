# Plan: evva Drive Pedal

## Summary

Add a new "evva" overdrive pedal model to the drive slot with 5 knobs:
**Gain**, **Level** (音量), **Low**, **Mid**, **High**.

The current drive architecture has exactly 3 knob params (drive/tone/level)
shared by all models. To support the evva model's 5-knob layout, we add 3
new EQ params (`low`, `mid`, `high`) to the drive slot's PARAMS array and
extend the `Circuit` trait so a model can apply a 3-band EQ in an additional
pass.

## How it fits into the existing architecture

- **Registry**: Append a `ModelDef` to `MODEL_DEFS` (index 4). The GUI
  dropdown, REPL, MIDI, and plugin auto-discover it.
- **Params**: The drive slot gains 3 additional params (`low`, `mid`,
  `high`). Existing models ignore these — their default values have no
  audible effect.
- **Circuit trait**: Add an `eq` default method (no-op) so only models
  that need it override.
- **model_knob_name**: Extended to return EQ knob names for the evva model.
  For other models, returns `None` → falls back to the generic param name.

## Circuit design (evva)

Inspired by the Blues Breaker / Morning Glory lineage: soft asymmetric
clipping, full-range (lows are kept), and a flexible 3-band EQ for tone
shaping — clean boost at low gain, warm breakup when pushed.

### Gain stage (`shape`)
- Asymmetric tanh soft-clip (one knee at 0.8, the other at 0.5 — even
  harmonics)
- Full-range: gentle high-pass at 30 Hz to block subsonics without
  thinning the guitar
- Gain range: +3 dB (clean boost) to +36 dB (medium breakup), audio taper
- DC blocked inside the oversampled stage (12 Hz one-pole)

### EQ stage (`eq`)
- **Low**: ±12 dB shelving at 120 Hz
- **Mid**: ±10 dB peaking at 750 Hz, Q ≈ 0.8
- **High**: ±12 dB shelving at 4 kHz
- All at base sample rate, applied before level

### Level
- Reuses the existing `level_lin` audio-taper law (unity near 6, +9 dB at 10)

### Default knob positions
- Gain: 4.0 (edge of breakup)
- Low: 5.0 (flat)
- Mid: 5.0 (flat)
- High: 5.0 (flat)
- Level: 6.0 (near unity)

## Files to change

### 1. `crates/lh-dsp/src/drive.rs` (main changes)

| Area | Change |
|------|--------|
| `Circuit` trait | Add `fn eq(&mut self, block, low, mid, high)` with default no-op |
| `PARAMS` | Add 3 `ParamDesc` entries for low/mid/high (indices 4,5,6) |
| `Drive` struct | Add `low_s`, `mid_s`, `high_s` smoothed + `low_traj`, `mid_traj`, `high_traj` buffers |
| `Drive::new()` | Init the new smoothed params and trajectory vecs |
| `Drive::prepare()` | Configure + snap the new smoothed params |
| `Drive::set_param()` | Handle indices 4,5,6 |
| `Drive::process()` | Fill EQ trajectories, call `circuit.eq()` after `post`, before level |
| `model_knob_name()` | Return "Low"/"Mid"/"High" for evva model, `None` otherwise |
| `MODEL_DEFS` | Append evva entry: label `"evva"`, knobs `["Gain", "Tone", "Level"]` |
| `Evva` struct | New circuit implementation |

### 2. No changes needed to

- `crates/lh-core/` — no new types, laws, or migration code
- `app/lion-heart/src/gui/mod.rs` — auto-discovers params via descriptor
- `plugin/lion-heart-plugin/src/lib.rs` — auto-discovers params via descriptor
- Preset schema — new params are optional (missing → defaults), no schema
  version bump

## Tests to add

- `evva_creates_even_harmonics` — asymmetric clipping produces 2nd harmonic
- `evva_eq_shelves_work` — verify low/high shelf boost/cut
- `evva_low_gain_is_clean` — gain < 3 stays near-clean
- `evva_passes_unity_at_defaults` — joins the existing level-matching test
- Existing generic tests (`every_model_is_finite...`, etc.) pass for evva
  automatically via the loop over MODELS

## Risks / trade-offs

- The EQ params show as knobs for **all** drive models in the GUI — they
  just don't do anything for ts9/blues-driver/classic/centaur. This mirrors
  how many real multi-effects units work (inactive knobs for certain models).
- No preset migration needed: the new params are optional and default to
  5.0 (flat EQ).
- The `Tone` knob on evva remains (it's the generic 3rd knob) but is unused
  — the 3-band EQ replaces it. The GUI shows it labeled "Tone" from the
  ModelDef knob captions but it has no effect.

# ADR 010: Filter family — a new chain slot, and the default-bypass flag

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: PRD 007 (`docs/PRD/007-filter-family-autowah.md`), ADR 005
(dynamic chain), PRD 006, white paper §3/§4

## Context

The auto-wah could have been a ninth mod pedal (zero structural cost — the
dynamic chain already allows dragging a mod slot pre-drive, and plugin
parity would be free). The user chose a new `filter` family instead,
anticipating more filter-type effects (LFO wah, sample & hold, formant).
A new family is a chain-level decision: it touches `DEFAULT_CHAIN`, the
session registry, the plugin's fixed chain, and — uniquely — it has **no
transparent knob position**, so it cannot ship active on the default board
the way gate/comp/limiter do.

## Decision

- **Family `filter`** (key deliberately broader than "wah"), first pedal
  `autowah`. Single-pedal today, built to grow: slots address families, so
  future pedals append without another chain change.
- **`DEFAULT_CHAIN` grows to 11** (cap is 12): `gate → filter → comp → …`.
  The filter sits **before the compressor** because its envelope follower
  feeds on playing dynamics — the thing the compressor exists to remove.
  Players who want the synthier post-drive squelch drag the card (ADR 005).
- **`lh_core::default_active(family_key)`** is the new shared flag: the
  app's `Session::start` bypasses flagged slots after building the board,
  and the plugin's per-slot `Active` BoolParam takes it as its default —
  one source of truth, pinned by the plugin's chain test. Presets are
  untouched: `SlotState.active` already round-trips, old presets simply
  don't contain a filter slot (reconcile removes it), and no schema bump is
  needed (a new family key is vocabulary, covered by the forward-compat
  skip rules).
- **The autowah DSP**: asymmetric envelope follower (fixed 2 ms attack,
  60–600 ms `decay` release, `sens` up to +30 dB pre-gain, mono-summed
  source) → geometric sweep 180 Hz–2.4 kHz (`direction` flips it) → a
  Chamberlin SVF per channel (one `sin` per-sample retune; LP/BP/HP from
  one structure = the `mode` switch is free; band state soft-clipped every
  sample so Q 12 saturates analog-style instead of diverging — RT rule 7).
  Both channels share one sweep: a quack is one event (the vibrato
  principle). Coefficients that depend only on knobs (sens gain, damp,
  release) rebuild at block rate; the sweep's own exp + sin are the
  per-sample cost.

## Consequences

- ~1.23 µs per 64-frame stereo block (0.09 % of the deadline).
- The default board is now 11 of 12 slots — one free index for user adds
  before hitting the cap. Accepted; the cap is an engine constant that can
  be raised in a future ADR if boards routinely fill.
- Plugin parameter surface grows a slot (`filter_autowah_*`,
  `filter_active` defaulting **off**) — another pre-v0.1 param-id break;
  re-run clap-validator.
- Chamberlin's bandpass is the constant-skirt variant: Q moves the peak
  (gain ≈ Q), not the skirts — documented here because the first character
  test probed a skirt and read no Q effect at all; the shipped test probes
  the resonance with a small signal (below the soft-clip knee).
- The promote-from-mod migration path discussed in PRD 007 §1 was never
  needed; nothing lives in two places.

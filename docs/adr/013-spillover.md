# ADR 013: Spillover — a spill lane in the engine output path

Status: **accepted — implemented**
Date: 2026-07-19
Relates to: PRD 010 (`docs/PRD/010-spillover.md`), ADR 005 (dynamic chain:
install/remove/retire), white paper §7 (M6 stretch "delay/reverb
spillover"), §3.3 (always-on safety limiter), §4.2 (click-freeness)

## Context

A preset switch currently kills reverb/delay tails two ways: a surviving
same-family slot gets the incoming preset's values (a delay whose `time`
jumps turns its tail into a pitch-sliding artifact), and a removed slot is
retired at the master-fade bottom. Pro units (Strymon, Helix) instead let
the old tail ring out while the new patch is immediately playable. This is
the first change since M5 to touch the **real-time lifecycle** — a second
exit for effects besides the retire chute — so it gets an ADR.

## Decision

- **The engine gains spill lanes** (`SPILL_LANES = 4`, preallocated),
  processed **after the master fade and before the output stage**, so a
  structure change's fade-to-silence cannot mute a tail and the always-on
  safety limiter + global EQ still cover the summed result.
- **`EngineMsg::SpillSlot { index }`** takes `slots[index]` **immediately**
  (a pointer move into a free lane — no allocation) rather than deferring
  to the fade bottom like `RemoveSlot`. The lane takes over the tail the
  instant the slot leaves the audible path, with no fade dip on the tail;
  the main loop skips the now-`None` slot even while the order still lists
  its index, and the deferred `SetOrder` drops the index later. `SpillSlot`
  is pushed before the `InstallSlot`s that may reuse the index, so FIFO
  ordering keeps reuse correct.
- **Each lane feeds silence through its effect** (dry = 0 → output is the
  wet tail), applies its gain, and sums into the bus. **Exits**: output
  peak below −80 dBFS for ~250 ms retires it down the existing chute;
  after an ~8 s grace the lane force-decays at −12 dB/s (bounded-drone
  insurance — a tape/vintage delay at feedback ≥ 1 self-oscillates
  forever with no input; forced decay drives it under the silence floor,
  which then evicts it), with the gain floored to 0 below 1e-7 for
  denormal safety (RT rule 7). A spill arriving with all lanes full evicts
  the **oldest** (hard cut — its tail is the most decayed; a fade would
  need an extra transient slot, and 4 lanes make full-overflow rare).
- **`Effect::tail_seconds()`** (default 0; delay/reverb override with a
  conservative static upper bound) is a *hint* cached into
  `SlotShadow.tail_secs` at install, so the control thread decides spill
  vs. hard-remove without touching the audio-thread effect. The engine's
  own silence detection, not this number, ends the tail.
- **`apply_preset_chain` takes `spillover: bool`.** Pass 2 spills an
  unclaimed slot when `spillover && tail_secs > 0` and hard-removes
  otherwise; tailed slots are always spilled-and-rebuilt-fresh rather than
  claimed, so a delay never gets the incoming preset's `time` glued onto
  its ringing buffer. The board-edit `remove` picks the same way. The flag
  is `AppConfig.spillover` (default **on**); off routes everything through
  `RemoveSlot` and the lanes sit idle.

## Consequences

- Worst case 4 lanes × the priciest reverb ≈ +18 µs per 64-frame block
  (~1.4 % of the 1333 µs deadline), only while tails ring;
  `spillover_worst` benches it. `assert_no_alloc` stays green — spilling
  is a pointer move and the lane scratch is preallocated.
- A rapid A/B between two space-heavy presets can outrun 4 lanes and hard-
  cut the oldest still-ringing tail. Rare and level-bounded by the safety
  limiter; a fade-to-make-room and a per-slot spill toggle are possible
  follow-ups.
- The plugin gets no spillover — hosts have their own tail/freeze handling
  and the chain there is host-driven. Standalone-first.
- Tails are transient, never persisted: a spilled lane is not part of any
  preset or snapshot.

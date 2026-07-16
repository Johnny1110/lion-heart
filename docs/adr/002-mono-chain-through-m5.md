# ADR 002: Mono chain through M5 — stereo deferred

Status: **accepted — implemented at M7** (the stereo bus landed as M7's first
step, exactly along the lines sketched below: FDN reverb grew a second
decorrelated tap mix, modulation a per-channel LFO offset; dynamics went
linked, drive/EQ dual-state, NAM mono-sums its input)
Date: 2026-07-16
Relates to: white paper §4 (DSP conventions), milestone M5

## Context

The white paper's convention says the chain is mono by default, with stereo
"where inherent — from the reverb/modulation outputs onward". M5 lands both
of those effect families, so the question is due.

Everything between the interface and the speakers is mono today: `lh-io`
taps one input channel, the `Chain` processes one `&mut [f32]` block in
place, and the duplex runner writes the result to both output channels. The
`Effect` trait, the bypass crossfade, the reorder fade, telemetry, and the
preset schema all assume one channel.

Making the tail of the chain stereo is not an incremental change: it needs a
stereo variant of the `Effect` trait (or interleaved buffers with channel
count negotiation), a mono→stereo split point that survives *reordering*
(any slot can move), stereo-aware bypass/order fades, doubled scratch
buffers, and an I/O contract change in `lh-io`. Bolting that onto M5 would
have put a structural refactor in the middle of a DSP milestone.

## Decision

M5 ships the modulation family and the FDN reverb **mono in / mono out**;
the chain stays single-channel end to end. The reverb's 8 delay-line tails
are mixed to one output; the modulation voices are single-path.

The stereo refactor is scheduled as its own step — target window is after
M6 ("on stage") and before or during M7 (the plugin build needs a stereo bus
anyway). The FDN was built with that in mind: taking two differently-weighted
tap mixes of the same 8 lines yields a decorrelated L/R pair without
touching the feedback structure, and the modulation LFO can grow a phase
offset per channel the same way.

## Consequences

- Live and recorded output through M5/M6 is dual-mono (same signal on both
  interface outputs). Width from reverb/chorus arrives with the stereo bus.
- The `Effect` trait stays simple for M5's five new DSP modules, and every
  existing test/bench keeps its shape.
- When the stereo bus lands, `lh-dsp` effects opt in gradually: mono effects
  keep working (processed per channel or up-mixed at the split point);
  reverb and modulation get true stereo outputs first.
- Revisit trigger: M7 plugin work begins, or recording use makes dual-mono
  reverb noticeably flat before then.

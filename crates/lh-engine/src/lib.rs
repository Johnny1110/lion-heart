//! Real-time chain runner: a linear chain with runtime **structure editing**
//! (PRD 002) — reorder, install, and remove slots while the stream runs.
//!
//! Split ownership: [`Chain`] moves onto the audio thread and is the only
//! thing that touches effects while streams run; [`ChainHandle`] stays on the
//! control thread. They communicate exclusively through a lock-free SPSC ring
//! of [`EngineMsg`] and a couple of atomics — no locks, no allocation on the
//! audio side (white paper §4.1). Retired effects travel back on a second
//! ring (the garbage chute) to be dropped off the audio thread.
//!
//! Click-freeness (white paper §4.2): params are smoothed inside effects,
//! bypass crossfades per slot, and topology changes ride a short master fade
//! through silence. Installs land immediately (the control side only targets
//! indices outside the audible order, so they are silent by construction);
//! removals and the new order apply together at the bottom of the fade —
//! untouched slots keep their state, so delay/reverb tails survive edits.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use std::collections::BTreeMap;

use lh_core::global_eq::{BAND_COUNT, Band, GlobalEqState};
use lh_core::preset::{SlotState, Snapshot, SnapshotSlot};
use lh_core::{FamilyDesc, ParamDesc, ParamId, lin_to_db};
use lh_dsp::Effect;
use lh_dsp::blocks::smooth::Smoothed;
use lh_dsp::dynamics::Limiter;
use lh_dsp::eq::global::GlobalEq;
use thiserror::Error;

/// Engine-internal processing granularity. Device callbacks may hand us
/// bigger blocks; `Chain::process` slices them down to this.
pub const MAX_BLOCK: usize = 1024;
/// Upper bound on chain length (fits the order message on the ring).
pub const MAX_SLOTS: usize = 12;
const MSG_CAPACITY: usize = 256;
/// Bypass toggles crossfade over this window instead of hard-switching.
const BYPASS_FADE_MS: f32 = 10.0;
/// Master fade time constant for reorders (out through ~-60 dB, then back).
const ORDER_FADE_MS: f32 = 4.0;
/// Upper bound of control messages applied per process call, so a message
/// flood can never starve the audio deadline.
const MAX_MSGS_PER_BLOCK: usize = 64;

pub enum EngineMsg {
    SetParam {
        id: ParamId,
        normalized: f32,
    },
    /// Switch a slot's active pedal (PRD 001). The control side follows up
    /// with the incoming pedal's param values from its shadow.
    SelectPedal {
        slot: u8,
        pedal: u8,
    },
    /// `active == false` bypasses the slot (crossfaded).
    SetActive {
        slot: u8,
        active: bool,
    },
    /// New processing order (slot indices); applied at the bottom of a
    /// master fade so the routing switch is inaudible.
    SetOrder {
        order: [u8; MAX_SLOTS],
        len: u8,
    },
    /// Install a control-side-prepared effect at a slot index. Applied
    /// immediately: the control side only targets indices outside the
    /// audible order, so the swap is silent. Replaces (and retires) any
    /// occupant and cancels a pending removal of the same index — a
    /// re-install supersedes the removal it raced with.
    InstallSlot {
        index: u8,
        effect: Box<dyn Effect>,
    },
    /// Remove a slot's effect. Deferred to the bottom of the master fade
    /// (after the pending order lands) so the occupant never vanishes from
    /// an audible chain; the effect is retired down the chute.
    RemoveSlot {
        index: u8,
    },
    /// Move a slot's effect into a spill lane **immediately** (PRD 010): it
    /// leaves the audible chain now (the main loop skips the emptied slot)
    /// and keeps ringing in the lane, fed silence, until it decays. Pushed
    /// before any `InstallSlot` that reuses the index, so FIFO order keeps
    /// reuse correct. Retired down the chute once the tail goes silent.
    SpillSlot {
        index: u8,
    },
    /// Update one global-EQ band on the output stage (PRD 003).
    SetEqBand {
        band: u8,
        state: Band,
    },
    /// Master toggle of the output-stage global EQ (crossfaded).
    SetEqActive(bool),
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown slot {0:?}")]
    UnknownSlot(String),
    #[error("unknown param {param:?} on slot {slot:?}")]
    UnknownParam { slot: String, param: String },
    #[error("unknown pedal {pedal:?} on slot {slot:?}")]
    UnknownPedal { slot: String, pedal: String },
    #[error("bad chain order: {0}")]
    BadOrder(String),
    #[error("chain is full ({MAX_SLOTS} slots)")]
    ChainFull,
    #[error("control queue full — engine not draining?")]
    QueueFull,
}

/// `slot.pedal` is the virtual param that selects a slot's pedal; `model`
/// (drive) and `type` (mod) are accepted as pre-v3 aliases so existing
/// `midi.json` mappings keep working.
pub fn is_pedal_selector(param_key: &str) -> bool {
    matches!(param_key, "pedal" | "model" | "type")
}

/// Block peaks published by the audio thread (f32 bits in atomics).
#[derive(Debug, Default)]
pub struct Telemetry {
    peak_in_bits: AtomicU32,
    peak_out_bits: AtomicU32,
}

impl Telemetry {
    pub fn peak_in(&self) -> f32 {
        f32::from_bits(self.peak_in_bits.load(Ordering::Relaxed))
    }

    pub fn peak_out(&self) -> f32 {
        f32::from_bits(self.peak_out_bits.load(Ordering::Relaxed))
    }
}

struct Slot {
    effect: Box<dyn Effect>,
    /// 1.0 = active, 0.0 = bypassed; smoothed for click-free toggling.
    wet: Smoothed,
}

/// Number of spill lanes (PRD 010): tails kept ringing after their slot
/// leaves the chain. Four gives comfortable headroom for rapid A/B between
/// space-heavy presets; each idle lane costs nothing.
pub const SPILL_LANES: usize = 4;
/// A lane whose output stays below this (≈ −80 dBFS) is considered silent.
const SPILL_SILENCE: f32 = 1e-4;
/// Silence must persist this long before a lane retires (seconds).
const SPILL_SILENCE_SECS: f32 = 0.25;
/// A tail rings freely this long before forced decay begins (seconds) —
/// insurance against a self-oscillating feedback delay that never decays.
const SPILL_GRACE_SECS: f32 = 8.0;
/// Forced-decay slope once the grace elapses.
const SPILL_DECAY_DB_PER_S: f32 = 12.0;

/// One spill lane: an effect ringing out on silence, summed into the output
/// bus after the master fade. Preallocated; `effect` is `None` when free.
struct SpillLane {
    effect: Option<Box<dyn Effect>>,
    /// Samples since the spill began (grace timer before forced decay).
    age: u64,
    /// Consecutive silent samples (retire when this passes the threshold).
    quiet: u64,
    /// Forced-decay gain — 1.0 until the grace elapses, then ramps to 0.
    decay_gain: f32,
}

impl SpillLane {
    fn free() -> Self {
        Self {
            effect: None,
            age: 0,
            quiet: 0,
            decay_gain: 1.0,
        }
    }

    /// Take on a spilled effect (its internal tail is preserved — never
    /// reset). Resets only the lane's own timers/gain.
    fn start(&mut self, effect: Box<dyn Effect>) {
        self.effect = Some(effect);
        self.age = 0;
        self.quiet = 0;
        self.decay_gain = 1.0;
    }

    /// Run one chunk of the tail (already rendered into `wet_*`) into the
    /// bus: apply the forced-decay gain, sum, and advance the timers.
    /// `sr` is the sample rate. Returns nothing; call [`Self::finished`].
    fn mix_into(
        &mut self,
        bus_l: &mut [f32],
        bus_r: &mut [f32],
        wet_l: &[f32],
        wet_r: &[f32],
        sr: f32,
    ) {
        let grace = (SPILL_GRACE_SECS * sr) as u64;
        let per_sample = 10f32.powf(-SPILL_DECAY_DB_PER_S / 20.0 / sr);
        let mut peak = 0.0f32;
        for i in 0..bus_l.len() {
            if self.age >= grace {
                self.decay_gain *= per_sample;
                if self.decay_gain < 1e-7 {
                    self.decay_gain = 0.0; // floor: no sustained denormals
                }
            }
            self.age += 1;
            let g = self.decay_gain;
            let l = wet_l[i] * g;
            let r = wet_r[i] * g;
            bus_l[i] += l;
            bus_r[i] += r;
            peak = peak.max(l.abs()).max(r.abs());
        }
        if peak < SPILL_SILENCE {
            self.quiet += bus_l.len() as u64;
        } else {
            self.quiet = 0;
        }
    }

    /// The tail has gone silent long enough to retire.
    fn finished(&self, sr: f32) -> bool {
        self.quiet >= (SPILL_SILENCE_SECS * sr) as u64
    }
}

/// The always-on ceiling of the output stage. With the chain limiter now
/// optional (PRD 002), white paper §3.3's "no patch, setting, or bug may
/// slam the monitors" guarantee lives here — and it catches global-EQ
/// boosts too.
const SAFETY_CEILING_DB: f32 = -0.3;

/// Fixed output stage (PRD 003): chain → global EQ → safety limiter →
/// spectrum tap. Runs after the master fade, so structure edits are already
/// silent by the time they reach it.
struct OutputStage {
    eq: GlobalEq,
    safety: Limiter,
    /// Post-stage mono sum for the GUI spectrum analyzer. Lock-free,
    /// drop-on-full: an unread tap must never stall the callback.
    tap: Option<rtrb::Producer<f32>>,
    /// Preallocated mono-sum scratch (MAX_BLOCK; input is chunked to fit).
    mono: Vec<f32>,
}

impl OutputStage {
    fn new() -> Self {
        Self {
            eq: GlobalEq::new(),
            safety: Limiter::new(),
            tap: None,
            mono: Vec::new(),
        }
    }

    fn prepare(&mut self, sample_rate: u32) {
        self.eq.prepare(sample_rate);
        self.safety.prepare(sample_rate);
        // Look the ceiling up by key: a faceplate reorder must fail loudly
        // here (covered by engine tests), never silently misconfigure the
        // one limiter that is always on.
        let desc = &lh_dsp::dynamics::limiter::DESC;
        let index = desc
            .param_index("ceiling")
            .expect("safety limiter has a ceiling param");
        self.safety
            .set_param(index, desc.params[index].range.to_norm(SAFETY_CEILING_DB));
        self.mono = vec![0.0; MAX_BLOCK];
    }

    fn reset(&mut self) {
        self.eq.reset();
        self.safety.reset();
    }

    fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        for (chunk_l, chunk_r) in left.chunks_mut(MAX_BLOCK).zip(right.chunks_mut(MAX_BLOCK)) {
            self.eq.process(chunk_l, chunk_r);
            self.safety.process(chunk_l, chunk_r);
            if let Some(tap) = &mut self.tap {
                let n = chunk_l.len().min(self.mono.len()).min(tap.slots());
                if n > 0
                    && let Ok(mut chunk) = tap.write_chunk(n)
                {
                    for (m, (l, r)) in self.mono[..n]
                        .iter_mut()
                        .zip(chunk_l.iter().zip(chunk_r.iter()))
                    {
                        *m = 0.5 * (l + r);
                    }
                    let (a, b) = chunk.as_mut_slices();
                    a.copy_from_slice(&self.mono[..a.len()]);
                    b.copy_from_slice(&self.mono[a.len()..a.len() + b.len()]);
                    chunk.commit_all();
                }
            }
        }
    }
}

/// The audio-thread side. RT rules apply to [`Chain::process`] and
/// [`Chain::reset`]; [`Chain::prepare`] allocates and runs before streams start.
pub struct Chain {
    /// Fixed-capacity slot table (`MAX_SLOTS` entries); `None` = free index.
    slots: Vec<Option<Slot>>,
    /// Processing order (occupied indices into `slots`).
    order: Vec<u8>,
    /// A reorder waiting for the master fade to reach the bottom.
    pending_order: Option<([u8; MAX_SLOTS], u8)>,
    /// Removals waiting for the same fade bottom (after the order lands).
    pending_removes: [bool; MAX_SLOTS],
    /// Master output fade: 1.0 in normal operation, dips to ~0 across
    /// structure changes.
    fade: Smoothed,
    rx: rtrb::Consumer<EngineMsg>,
    /// Replaced/removed effects go back to the control thread to die.
    retired: rtrb::Producer<Box<dyn Effect>>,
    /// Effects the full chute couldn't take yet (preallocated, bounded by
    /// MAX_SLOTS — pushes never allocate).
    parked: Vec<Box<dyn Effect>>,
    sample_rate: u32,
    dry_l: Vec<f32>,
    dry_r: Vec<f32>,
    /// Spill lanes (PRD 010): tails ringing out after their slot left the
    /// chain. Preallocated; processed after the master fade.
    spill: Vec<SpillLane>,
    /// Preallocated scratch the spill lanes render their tails into.
    spill_l: Vec<f32>,
    spill_r: Vec<f32>,
    /// Always-on output stage: global EQ → safety limiter → spectrum tap.
    output: OutputStage,
    telemetry: Arc<Telemetry>,
    /// Optional raw-input copy for control-thread analyzers (tuner).
    tap: Option<rtrb::Producer<f32>>,
}

impl Chain {
    pub fn prepare(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
        for slot in self.slots.iter_mut().flatten() {
            slot.effect.prepare(sample_rate);
            slot.wet.configure(BYPASS_FADE_MS, sample_rate);
            slot.wet.snap_to_target();
        }
        self.fade.configure(ORDER_FADE_MS, sample_rate);
        self.fade.snap_to_target();
        self.dry_l = vec![0.0; MAX_BLOCK];
        self.dry_r = vec![0.0; MAX_BLOCK];
        for lane in self.spill.iter_mut().flat_map(|l| l.effect.as_mut()) {
            lane.prepare(sample_rate);
        }
        self.spill_l = vec![0.0; MAX_BLOCK];
        self.spill_r = vec![0.0; MAX_BLOCK];
        self.output.prepare(sample_rate);
    }

    pub fn reset(&mut self) {
        for slot in self.slots.iter_mut().flatten() {
            slot.effect.reset();
        }
        // A hard reset drops spill tails too (transient, not state). reset
        // runs off the audio thread, so dropping the boxes here is allowed.
        for lane in &mut self.spill {
            *lane = SpillLane::free();
        }
        self.output.reset();
    }

    /// Install the post-output-stage tap for the GUI spectrum analyzer.
    /// Call before the chain moves to the audio thread.
    pub fn set_output_tap(&mut self, tap: rtrb::Producer<f32>) {
        self.output.tap = Some(tap);
    }

    /// Send an effect down the chute; park it if the chute is full (retried
    /// each block — `parked` capacity is preallocated, never grows).
    fn retire(&mut self, effect: Box<dyn Effect>) {
        match self.retired.push(effect) {
            Ok(()) => {}
            Err(rtrb::PushError::Full(effect)) => {
                if self.parked.len() < self.parked.capacity() {
                    self.parked.push(effect);
                } else {
                    // Both chute and parking full (control thread not
                    // collecting): leak rather than deallocate on the audio
                    // thread. Unreachable with a live control loop.
                    std::mem::forget(effect);
                }
            }
        }
    }

    /// Install at `index` immediately (silent by protocol: the index is not
    /// in the audible order). Cancels a racing pending removal.
    fn install(&mut self, index: usize, effect: Box<dyn Effect>) {
        if index >= self.slots.len() {
            self.retire(effect);
            return;
        }
        let mut wet = Smoothed::new(1.0);
        wet.configure(BYPASS_FADE_MS, self.sample_rate);
        wet.snap_to_target();
        self.pending_removes[index] = false;
        if let Some(old) = self.slots[index].replace(Slot { effect, wet }) {
            self.retire(old.effect);
        }
    }

    /// Hand a spilled effect to a free lane; if all lanes are busy, evict
    /// the oldest (its tail is the most decayed) and take its place. No
    /// allocation — a pointer move (PRD 010).
    fn spill_effect(&mut self, effect: Box<dyn Effect>) {
        if let Some(lane) = self.spill.iter_mut().find(|l| l.effect.is_none()) {
            lane.start(effect);
            return;
        }
        // All lanes busy: retire the oldest, reuse its lane.
        if let Some((idx, _)) = self.spill.iter().enumerate().max_by_key(|(_, l)| l.age) {
            if let Some(old) = self.spill[idx].effect.take() {
                self.retire(old);
            }
            self.spill[idx].start(effect);
        } else {
            // No lanes configured at all: nothing to do but retire it.
            self.retire(effect);
        }
    }

    /// Render and sum all active spill lanes into the bus, then retire any
    /// whose tail has gone silent. Runs after the master fade so a
    /// structure-change fade never mutes a tail; before the output stage so
    /// the safety limiter still covers the sum. Idle (early return) when no
    /// lane is ringing.
    fn process_spill(&mut self, left: &mut [f32], right: &mut [f32]) {
        if self.spill.iter().all(|l| l.effect.is_none()) {
            return;
        }
        let sr = self.sample_rate as f32;
        // Borrow the scratch out of `self` so the lane loop can hold both it
        // and `self.spill` (disjoint, but the borrow checker needs the move).
        let mut scratch_l = std::mem::take(&mut self.spill_l);
        let mut scratch_r = std::mem::take(&mut self.spill_r);
        for (chunk_l, chunk_r) in left.chunks_mut(MAX_BLOCK).zip(right.chunks_mut(MAX_BLOCK)) {
            let n = chunk_l.len();
            for lane in self.spill.iter_mut() {
                let Some(effect) = lane.effect.as_mut() else {
                    continue;
                };
                scratch_l[..n].fill(0.0);
                scratch_r[..n].fill(0.0);
                effect.process(&mut scratch_l[..n], &mut scratch_r[..n]);
                lane.mix_into(chunk_l, chunk_r, &scratch_l[..n], &scratch_r[..n], sr);
            }
        }
        self.spill_l = scratch_l;
        self.spill_r = scratch_r;
        // Retire finished lanes (scratch is back, so `self.retire` is free
        // to borrow). A silent-long-enough tail dies down the chute.
        for i in 0..self.spill.len() {
            if self.spill[i].finished(sr)
                && let Some(effect) = self.spill[i].effect.take()
            {
                self.retire(effect);
            }
        }
    }

    /// Install a raw-input tap. Call before the chain moves to the audio
    /// thread; the consumer side is drained by a control thread (tuner).
    pub fn set_input_tap(&mut self, tap: rtrb::Producer<f32>) {
        self.tap = Some(tap);
    }

    pub fn process(&mut self, left: &mut [f32], right: &mut [f32]) {
        debug_assert_eq!(left.len(), right.len());

        // Finish retiring anything the chute couldn't take last block.
        while let Some(effect) = self.parked.pop() {
            if let Err(rtrb::PushError::Full(effect)) = self.retired.push(effect) {
                self.parked.push(effect);
                break;
            }
        }

        for _ in 0..MAX_MSGS_PER_BLOCK {
            match self.rx.pop() {
                Ok(EngineMsg::SetParam { id, normalized }) => {
                    if let Some(slot) = self
                        .slots
                        .get_mut(id.slot as usize)
                        .and_then(Option::as_mut)
                    {
                        slot.effect.set_param(id.param as usize, normalized);
                    }
                }
                Ok(EngineMsg::SelectPedal { slot, pedal }) => {
                    if let Some(slot) = self.slots.get_mut(slot as usize).and_then(Option::as_mut) {
                        slot.effect.select_pedal(pedal as usize);
                    }
                }
                Ok(EngineMsg::SetActive { slot, active }) => {
                    if let Some(slot) = self.slots.get_mut(slot as usize).and_then(Option::as_mut) {
                        slot.wet.set_target(if active { 1.0 } else { 0.0 });
                    }
                }
                Ok(EngineMsg::SetOrder { order, len }) => {
                    self.pending_order = Some((order, len));
                    self.fade.set_target(0.0);
                }
                Ok(EngineMsg::InstallSlot { index, effect }) => {
                    self.install(index as usize, effect);
                }
                Ok(EngineMsg::RemoveSlot { index }) => {
                    if (index as usize) < MAX_SLOTS {
                        self.pending_removes[index as usize] = true;
                        self.fade.set_target(0.0);
                    }
                }
                Ok(EngineMsg::SpillSlot { index }) => {
                    // Immediate: take the slot now (the main loop already
                    // skips an emptied slot) and hand its ringing effect to
                    // a lane. Cancel any pending removal of this index — a
                    // spill supersedes it.
                    if (index as usize) < MAX_SLOTS {
                        self.pending_removes[index as usize] = false;
                        if let Some(slot) = self.slots[index as usize].take() {
                            self.spill_effect(slot.effect);
                        }
                    }
                }
                Ok(EngineMsg::SetEqBand { band, state }) => {
                    self.output.eq.set_band(band as usize, state);
                }
                Ok(EngineMsg::SetEqActive(on)) => {
                    self.output.eq.set_enabled(on);
                }
                Err(_) => break,
            }
        }

        // Apply pending structure once the fade is inaudible (≤ -60 dB):
        // the new order first, then removals (nothing audible references
        // them anymore); then ride back up. Same-order messages still dip —
        // harmless.
        let structure_pending =
            self.pending_order.is_some() || self.pending_removes.iter().any(|&r| r);
        if structure_pending && self.fade.current() <= 1e-3 {
            if let Some((order, len)) = self.pending_order.take() {
                self.order.clear();
                self.order.extend_from_slice(&order[..len as usize]);
            }
            for index in 0..MAX_SLOTS {
                if self.pending_removes[index] {
                    self.pending_removes[index] = false;
                    if let Some(slot) = self.slots[index].take() {
                        self.retire(slot.effect);
                    }
                }
            }
            self.fade.set_target(1.0);
        }

        self.telemetry
            .peak_in_bits
            .store(peak(left).to_bits(), Ordering::Relaxed);

        // Copy the raw input into the analysis tap (tuner) — left channel;
        // the source is mono so both channels are identical at chain entry.
        // Lock-free chunk write, drop-on-full: an unread tap must never
        // stall the callback.
        if let Some(tap) = &mut self.tap {
            let n = left.len().min(tap.slots());
            if n > 0
                && let Ok(mut chunk) = tap.write_chunk(n)
            {
                let (a, b) = chunk.as_mut_slices();
                a.copy_from_slice(&left[..a.len()]);
                b.copy_from_slice(&left[a.len()..a.len() + b.len()]);
                chunk.commit_all();
            }
        }

        for (chunk_l, chunk_r) in left.chunks_mut(MAX_BLOCK).zip(right.chunks_mut(MAX_BLOCK)) {
            for i in 0..self.order.len() {
                let Some(slot) = self
                    .slots
                    .get_mut(self.order[i] as usize)
                    .and_then(Option::as_mut)
                else {
                    continue;
                };
                // A fade-out below -60 dB is inaudible: snap it so the
                // skip fast-path engages instead of blending forever.
                if slot.wet.target() == 0.0 && slot.wet.current() <= 1e-3 {
                    slot.wet.snap_to_target();
                }
                if slot.wet.is_settled() && slot.wet.target() == 0.0 {
                    continue; // fully bypassed: skip the work entirely
                }
                if slot.wet.is_settled() && slot.wet.target() == 1.0 {
                    slot.effect.process(chunk_l, chunk_r);
                    continue;
                }
                // Mid-crossfade: blend processed against the dry copies.
                let dry_l = &mut self.dry_l[..chunk_l.len()];
                let dry_r = &mut self.dry_r[..chunk_r.len()];
                dry_l.copy_from_slice(chunk_l);
                dry_r.copy_from_slice(chunk_r);
                slot.effect.process(chunk_l, chunk_r);
                for (i, (l, r)) in chunk_l.iter_mut().zip(chunk_r.iter_mut()).enumerate() {
                    let w = slot.wet.tick();
                    *l = dry_l[i] + (*l - dry_l[i]) * w;
                    *r = dry_r[i] + (*r - dry_r[i]) * w;
                }
            }
        }

        if !(self.fade.is_settled() && self.fade.target() == 1.0) {
            for (l, r) in left.iter_mut().zip(right.iter_mut()) {
                let g = self.fade.tick();
                *l *= g;
                *r *= g;
            }
        }

        // Spill lanes (PRD 010): sum ringing-out tails on top of the faded
        // chain. After the fade so a structure change cannot mute them.
        self.process_spill(left, right);

        // Output stage (PRD 003): global EQ → safety limiter → spectrum tap.
        self.output.process(left, right);

        self.telemetry
            .peak_out_bits
            .store(peak(left).max(peak(right)).to_bits(), Ordering::Relaxed);
    }
}

fn peak(block: &[f32]) -> f32 {
    block.iter().fold(0.0f32, |m, s| m.max(s.abs()))
}

/// Real-world value actually applied after clamping, for CLI/UI echo.
#[derive(Debug, Clone, Copy)]
pub struct Applied {
    pub real: f32,
    pub unit: &'static str,
}

/// Control-side mirror of one occupied slot.
struct SlotShadow {
    family: &'static FamilyDesc,
    /// Active pedal index.
    pedal: usize,
    /// pedal → param norms — the per-pedal knob memory (PRD 001).
    norms: Vec<Vec<f32>>,
    active: bool,
    /// The effect's tail hint (PRD 010), cached at install so the control
    /// side can choose spill-vs-remove without touching the audio effect.
    tail_secs: f32,
}

impl SlotShadow {
    fn from_effect(effect: &dyn Effect) -> Self {
        let family = effect.family();
        Self {
            family,
            pedal: effect.pedal_index(),
            norms: family
                .pedals
                .iter()
                .map(|p| p.params.iter().map(|pd| pd.default_norm()).collect())
                .collect(),
            active: true,
            tail_secs: effect.tail_seconds(),
        }
    }
}

/// Split an instance handle: `"drive2"` → `("drive", 2)`, `"drive"` →
/// `("drive", 1)`. Family keys contain no trailing digits.
fn split_handle(handle: &str) -> (&str, usize) {
    let base = handle.trim_end_matches(|c: char| c.is_ascii_digit());
    if base.len() == handle.len() || base.is_empty() {
        (handle, 1)
    } else {
        (base, handle[base.len()..].parse().unwrap_or(1))
    }
}

/// The control-thread side: validates handles, tracks a shadow of the state
/// for display, and feeds the ring.
///
/// Slots are instances (PRD 002): the same family may appear several times.
/// Handles address them by family key and 1-based rank in chain order —
/// `"drive"` is the first drive, `"drive2"` the second. The shadow keeps
/// values **per pedal** (PRD 001): it is the knob memory that makes pedal
/// switches restore each pedal's own settings — the engine side only ever
/// holds the active pedal's live values.
pub struct ChainHandle {
    tx: rtrb::Producer<EngineMsg>,
    /// Fixed-capacity mirror of the engine's slot table.
    slots: Vec<Option<SlotShadow>>,
    /// Chain order (occupied indices).
    order: Vec<u8>,
    /// Rate used to prepare freshly installed effects (the session updates
    /// it once the stream is up).
    sample_rate: u32,
    /// Round-robin cursor for free indices — avoids immediately reusing a
    /// just-removed index whose engine-side removal may still be pending.
    next_free: usize,
    /// Retired effects come back here to die on this thread.
    retired_rx: rtrb::Consumer<Box<dyn Effect>>,
    /// Output-stage global EQ shadow (PRD 003).
    eq: GlobalEqState,
    telemetry: Arc<Telemetry>,
}

impl ChainHandle {
    /// Families in chain order (one entry per instance).
    pub fn families(&self) -> Vec<&'static FamilyDesc> {
        self.order
            .iter()
            .map(|&i| self.shadow(i as usize).family)
            .collect()
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    /// The stream sample rate, used to prepare effects installed later.
    pub fn set_sample_rate(&mut self, sample_rate: u32) {
        self.sample_rate = sample_rate;
    }

    /// The audio thread never deallocates: retired effects die here. Call
    /// periodically from the control loop / frame tick. Returns how many.
    pub fn collect_garbage(&mut self) -> usize {
        let mut n = 0;
        while self.retired_rx.pop().is_ok() {
            n += 1;
        }
        n
    }

    /// The output-stage global EQ shadow (PRD 003).
    pub fn eq_state(&self) -> &GlobalEqState {
        &self.eq
    }

    /// Update one global-EQ band (values clamped into range).
    pub fn set_eq_band(&mut self, index: usize, band: Band) -> Result<(), EngineError> {
        if index >= BAND_COUNT {
            return Err(EngineError::UnknownParam {
                slot: "global eq".to_string(),
                param: format!("band {index}"),
            });
        }
        let band = band.clamped();
        self.tx
            .push(EngineMsg::SetEqBand {
                band: index as u8,
                state: band,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.eq.bands[index] = band;
        Ok(())
    }

    /// Master toggle of the output-stage global EQ.
    pub fn set_eq_active(&mut self, enabled: bool) -> Result<(), EngineError> {
        self.tx
            .push(EngineMsg::SetEqActive(enabled))
            .map_err(|_| EngineError::QueueFull)?;
        self.eq.enabled = enabled;
        Ok(())
    }

    /// Apply a whole EQ state (startup load / stream resume).
    pub fn apply_eq_state(&mut self, state: &GlobalEqState) -> Result<(), EngineError> {
        self.set_eq_active(state.enabled)?;
        for (index, band) in state.bands.iter().enumerate() {
            self.set_eq_band(index, *band)?;
        }
        Ok(())
    }

    pub fn slot_count(&self) -> usize {
        self.order.len()
    }

    pub fn is_full(&self) -> bool {
        self.order.len() >= MAX_SLOTS
    }

    /// Whether any instance of `family_key` is in the chain.
    pub fn contains_family(&self, family_key: &str) -> bool {
        self.order
            .iter()
            .any(|&i| self.shadow(i as usize).family.key == family_key)
    }

    fn shadow(&self, index: usize) -> &SlotShadow {
        self.slots[index]
            .as_ref()
            .expect("order references occupied slots")
    }

    fn shadow_mut(&mut self, index: usize) -> &mut SlotShadow {
        self.slots[index]
            .as_mut()
            .expect("order references occupied slots")
    }

    /// Resolve an instance handle to its slot index.
    fn slot_index(&self, handle: &str) -> Result<usize, EngineError> {
        let (base, nth) = split_handle(handle);
        let mut seen = 0;
        for &i in &self.order {
            if self.shadow(i as usize).family.key == base {
                seen += 1;
                if seen == nth {
                    return Ok(i as usize);
                }
            }
        }
        Err(EngineError::UnknownSlot(handle.to_string()))
    }

    /// The handle of the slot at `position` in chain order.
    fn handle_at(&self, position: usize) -> String {
        let key = self.shadow(self.order[position] as usize).family.key;
        let nth = self.order[..position]
            .iter()
            .filter(|&&i| self.shadow(i as usize).family.key == key)
            .count()
            + 1;
        if nth == 1 {
            key.to_string()
        } else {
            format!("{key}{nth}")
        }
    }

    /// Instance handles in current processing order.
    pub fn order_handles(&self) -> Vec<String> {
        (0..self.order.len()).map(|p| self.handle_at(p)).collect()
    }

    /// The active pedal's descriptor entry for `param_key`, if any.
    /// (`None` also for the virtual `pedal` selector — check
    /// [`is_pedal_selector`] first when both are possible.)
    pub fn param_desc(&self, handle: &str, param_key: &str) -> Option<&'static ParamDesc> {
        let slot = self.slot_index(handle).ok()?;
        let shadow = self.shadow(slot);
        let desc = shadow.family.pedals[shadow.pedal];
        desc.params.get(desc.param_index(param_key)?)
    }

    /// The current normalized value of the active pedal's `param_key`,
    /// from the shadow (soft-takeover compares the pedal against this).
    pub fn param_norm(&self, handle: &str, param_key: &str) -> Option<f32> {
        let slot = self.slot_index(handle).ok()?;
        let shadow = self.shadow(slot);
        let desc = shadow.family.pedals[shadow.pedal];
        let param = desc.param_index(param_key)?;
        Some(shadow.norms[shadow.pedal][param])
    }

    /// The active pedal's key for a slot.
    pub fn active_pedal(&self, handle: &str) -> Result<&'static str, EngineError> {
        let slot = self.slot_index(handle)?;
        let shadow = self.shadow(slot);
        Ok(shadow.family.pedals[shadow.pedal].key)
    }

    /// Whether a slot is currently active (not bypassed), from the shadow.
    pub fn is_active(&self, handle: &str) -> Result<bool, EngineError> {
        Ok(self.shadow(self.slot_index(handle)?).active)
    }

    /// Set a parameter of the slot's **active pedal** from a real-world
    /// value (clamped into range). The virtual `pedal` key (and its pre-v3
    /// aliases) selects a pedal by index instead.
    pub fn set_param(
        &mut self,
        handle: &str,
        param_key: &str,
        real: f32,
    ) -> Result<Applied, EngineError> {
        let slot = self.slot_index(handle)?;
        if is_pedal_selector(param_key) {
            let count = self.shadow(slot).family.pedals.len();
            let index = (real.max(0.0).round() as usize).min(count - 1);
            self.select_pedal_index(slot, index)?;
            return Ok(Applied {
                real: index as f32,
                unit: "",
            });
        }
        let shadow = self.shadow(slot);
        let pedal = shadow.pedal;
        let desc = shadow.family.pedals[pedal];
        let param = desc
            .param_index(param_key)
            .ok_or_else(|| EngineError::UnknownParam {
                slot: handle.to_string(),
                param: param_key.to_string(),
            })?;
        let p = &desc.params[param];
        let clamped = p.range.clamp(real);
        let normalized = p.range.to_norm(clamped);
        self.tx
            .push(EngineMsg::SetParam {
                id: ParamId {
                    slot: slot as u8,
                    param: param as u8,
                },
                normalized,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.shadow_mut(slot).norms[pedal][param] = normalized;
        Ok(Applied {
            real: clamped,
            unit: p.unit,
        })
    }

    /// Select a pedal by key, display name, or numeric index. Returns the
    /// selected pedal's key.
    pub fn select_pedal(
        &mut self,
        handle: &str,
        selector: &str,
    ) -> Result<&'static str, EngineError> {
        let slot = self.slot_index(handle)?;
        let family = self.shadow(slot).family;
        let index = family
            .pedal_index(selector)
            .or_else(|| {
                selector
                    .parse::<usize>()
                    .ok()
                    .filter(|i| *i < family.pedals.len())
            })
            .ok_or_else(|| EngineError::UnknownPedal {
                slot: handle.to_string(),
                pedal: selector.to_string(),
            })?;
        self.select_pedal_index(slot, index)
    }

    /// Select a pedal from a normalized `0..=1` controller position
    /// (MIDI CC mapped to `slot.pedal`).
    pub fn select_pedal_norm(
        &mut self,
        handle: &str,
        norm: f32,
    ) -> Result<&'static str, EngineError> {
        let slot = self.slot_index(handle)?;
        let count = self.shadow(slot).family.pedals.len();
        let index = (norm.clamp(0.0, 1.0) * (count - 1) as f32).round() as usize;
        self.select_pedal_index(slot, index)
    }

    fn select_pedal_index(
        &mut self,
        slot: usize,
        index: usize,
    ) -> Result<&'static str, EngineError> {
        self.tx
            .push(EngineMsg::SelectPedal {
                slot: slot as u8,
                pedal: index as u8,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.shadow_mut(slot).pedal = index;
        // Restore the incoming pedal's knobs from the shadow — the engine
        // never carries values across pedals (PRD 001 §5). Ring ordering
        // guarantees these land after the switch.
        for param in 0..self.shadow(slot).norms[index].len() {
            let normalized = self.shadow(slot).norms[index][param];
            self.tx
                .push(EngineMsg::SetParam {
                    id: ParamId {
                        slot: slot as u8,
                        param: param as u8,
                    },
                    normalized,
                })
                .map_err(|_| EngineError::QueueFull)?;
        }
        Ok(self.shadow(slot).family.pedals[index].key)
    }

    /// Enable (`true`) or bypass (`false`) a slot.
    pub fn set_active(&mut self, handle: &str, active: bool) -> Result<(), EngineError> {
        let slot = self.slot_index(handle)?;
        self.push_active(slot, active)
    }

    fn push_active(&mut self, slot: usize, active: bool) -> Result<(), EngineError> {
        self.tx
            .push(EngineMsg::SetActive {
                slot: slot as u8,
                active,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.shadow_mut(slot).active = active;
        Ok(())
    }

    /// Send the current shadow order to the engine.
    fn push_order(&mut self) -> Result<(), EngineError> {
        let mut order = [0u8; MAX_SLOTS];
        order[..self.order.len()].copy_from_slice(&self.order);
        self.tx
            .push(EngineMsg::SetOrder {
                order,
                len: self.order.len() as u8,
            })
            .map_err(|_| EngineError::QueueFull)
    }

    /// Reorder the chain. `handles` must name every slot exactly once; the
    /// engine fades through silence while switching.
    pub fn set_order(&mut self, handles: &[&str]) -> Result<(), EngineError> {
        if handles.len() != self.order.len() {
            return Err(EngineError::BadOrder(format!(
                "need all {} slots, got {}",
                self.order.len(),
                handles.len()
            )));
        }
        // Handles resolve against the *current* order; verify a permutation.
        let mut new_order: Vec<u8> = Vec::with_capacity(handles.len());
        for handle in handles {
            let index = self.slot_index(handle)? as u8;
            if new_order.contains(&index) {
                return Err(EngineError::BadOrder(format!("duplicate slot {handle:?}")));
            }
            new_order.push(index);
        }
        self.order = new_order;
        self.push_order()
    }

    /// Move the slot at chain position `from` to position `to`.
    pub fn move_position(&mut self, from: usize, to: usize) -> Result<(), EngineError> {
        if from >= self.order.len() {
            return Err(EngineError::BadOrder(format!("no slot at position {from}")));
        }
        let index = self.order.remove(from);
        let to = to.min(self.order.len());
        self.order.insert(to, index);
        self.push_order()
    }

    /// Install a **control-side built** effect at chain position `position`
    /// (clamped to the end). The effect is prepared here with the stream
    /// rate — the engine only pointer-swaps, silently (the index is not in
    /// the audible order until the faded reorder lands). Returns the new
    /// instance's handle.
    pub fn install_slot(
        &mut self,
        mut effect: Box<dyn Effect>,
        position: usize,
    ) -> Result<String, EngineError> {
        if self.is_full() {
            return Err(EngineError::ChainFull);
        }
        let index = self.free_index().ok_or(EngineError::ChainFull)?;
        effect.prepare(self.sample_rate);
        self.slots[index] = Some(SlotShadow::from_effect(effect.as_ref()));
        if self
            .tx
            .push(EngineMsg::InstallSlot {
                index: index as u8,
                effect,
            })
            .is_err()
        {
            self.slots[index] = None;
            return Err(EngineError::QueueFull);
        }
        let position = position.min(self.order.len());
        self.order.insert(position, index as u8);
        self.push_order()?;
        Ok(self.handle_at(position))
    }

    /// Remove a slot: it leaves the order now and is retired at the bottom
    /// of the fade. Untouched slots keep their state (tails survive).
    pub fn remove_slot(&mut self, handle: &str) -> Result<(), EngineError> {
        let index = self.slot_index(handle)?;
        self.order.retain(|&i| i as usize != index);
        self.push_order()?;
        self.tx
            .push(EngineMsg::RemoveSlot { index: index as u8 })
            .map_err(|_| EngineError::QueueFull)?;
        self.slots[index] = None;
        Ok(())
    }

    /// Spill a slot: it leaves the chain now but keeps ringing out in a
    /// spill lane until its tail decays (PRD 010). Same control-side
    /// bookkeeping as [`Self::remove_slot`]; the engine keeps the effect.
    pub fn spill_slot(&mut self, handle: &str) -> Result<(), EngineError> {
        let index = self.slot_index(handle)?;
        self.order.retain(|&i| i as usize != index);
        self.push_order()?;
        self.tx
            .push(EngineMsg::SpillSlot { index: index as u8 })
            .map_err(|_| EngineError::QueueFull)?;
        self.slots[index] = None;
        Ok(())
    }

    /// Whether a slot has a tail worth spilling (delay/reverb, PRD 010).
    pub fn slot_has_tail(&self, handle: &str) -> bool {
        self.slot_index(handle)
            .map(|i| self.shadow(i).tail_secs > 0.0)
            .unwrap_or(false)
    }

    fn free_index(&mut self) -> Option<usize> {
        for step in 0..MAX_SLOTS {
            let i = (self.next_free + step) % MAX_SLOTS;
            if self.slots[i].is_none() {
                self.next_free = (i + 1) % MAX_SLOTS;
                return Some(i);
            }
        }
        None
    }

    /// Capture the chain (order, bypass, selected pedal, and **every**
    /// pedal's real values) for a preset — structure and the whole knob
    /// memory survive a save/load round trip (PRD 001 §7.3, PRD 002 §7.3).
    pub fn snapshot_chain(&self) -> Vec<SlotState> {
        self.order
            .iter()
            .map(|&i| {
                let shadow = self.shadow(i as usize);
                let family = shadow.family;
                SlotState {
                    key: family.key.to_string(),
                    active: shadow.active,
                    pedal: Some(family.pedals[shadow.pedal].key.to_string()),
                    pedals: family
                        .pedals
                        .iter()
                        .enumerate()
                        .map(|(p, desc)| {
                            (
                                desc.key.to_string(),
                                desc.params
                                    .iter()
                                    .enumerate()
                                    .map(|(j, pd)| {
                                        (pd.key.to_string(), pd.range.to_real(shadow.norms[p][j]))
                                    })
                                    .collect(),
                            )
                        })
                        .collect(),
                    params: BTreeMap::new(),
                }
            })
            .collect()
    }

    /// Capture the current **scene** (PRD 009): per handle, the bypass flag
    /// and the *selected* pedal's real values. Unlike [`snapshot_chain`]
    /// this is a value+bypass overlay — no structure, no unselected pedals'
    /// memory — the unit a snapshot stores and morphs.
    pub fn capture_scene(&self) -> Snapshot {
        let mut slots = BTreeMap::new();
        for position in 0..self.order.len() {
            let i = self.order[position] as usize;
            let shadow = self.shadow(i);
            let pedal = shadow.pedal;
            let desc = shadow.family.pedals[pedal];
            let values = desc
                .params
                .iter()
                .enumerate()
                .map(|(j, pd)| (pd.key.to_string(), pd.range.to_real(shadow.norms[pedal][j])))
                .collect();
            slots.insert(
                self.handle_at(position),
                SnapshotSlot {
                    active: shadow.active,
                    values,
                },
            );
        }
        Snapshot { slots }
    }

    /// Update one pedal's shadow values from a preset map, collecting
    /// warnings for unknown params.
    fn absorb_pedal_values(
        &mut self,
        slot: usize,
        pedal: usize,
        values: &BTreeMap<String, f32>,
        warnings: &mut Vec<String>,
    ) {
        let desc = self.shadow(slot).family.pedals[pedal];
        for (param_key, value) in values {
            match desc.param_index(param_key) {
                Some(j) => {
                    let range = &desc.params[j].range;
                    self.shadow_mut(slot).norms[pedal][j] = range.to_norm(range.clamp(*value));
                }
                None => warnings.push(format!(
                    "unknown param {:?} on pedal {:?} skipped",
                    param_key, desc.key
                )),
            }
        }
    }

    /// Apply one preset slot onto an occupied index: bypass flag, per-pedal
    /// values into the shadow, then select + push the active pedal.
    fn apply_slot_state(
        &mut self,
        slot: usize,
        state: &SlotState,
        warnings: &mut Vec<String>,
    ) -> Result<(), EngineError> {
        self.push_active(slot, state.active)?;
        let family = self.shadow(slot).family;

        // All pedal values land in the shadow first — values for unselected
        // pedals only refresh the knob memory.
        for (pedal_key, values) in &state.pedals {
            match family.pedal_index(pedal_key) {
                Some(p) => self.absorb_pedal_values(slot, p, values, warnings),
                None => warnings.push(format!(
                    "unknown pedal {:?} on {:?} skipped",
                    pedal_key, state.key
                )),
            }
        }

        // Resolve the selection: explicit pedal, else the first.
        let target = match &state.pedal {
            Some(key) => match family.pedal_index(key) {
                Some(p) => p,
                None => {
                    warnings.push(format!(
                        "unknown pedal {:?} on {:?} — selection kept",
                        key, state.key
                    ));
                    self.shadow(slot).pedal
                }
            },
            None => 0,
        };

        // Pre-v3 flat params are values for the selected pedal.
        if !state.params.is_empty() {
            let flat = state.params.clone();
            self.absorb_pedal_values(slot, target, &flat, warnings);
        }

        self.select_pedal_index(slot, target)?;
        Ok(())
    }

    /// Apply a preset's chain **including its structure** (PRD 002): each
    /// preset slot claims the first unclaimed surviving instance of its
    /// family (state and tails survive), leftovers are removed, and missing
    /// instances are built via `build` and installed. One fade covers the
    /// whole transition.
    ///
    /// `build` constructs an effect for a family key — the session owns the
    /// concrete effect crates and the asset seams; `None` marks the family
    /// unknown/unbuildable and skips the slot with a warning.
    pub fn apply_preset_chain(
        &mut self,
        chain: &[SlotState],
        spillover: bool,
        build: &mut dyn FnMut(&str) -> Option<Box<dyn Effect>>,
    ) -> Result<Vec<String>, EngineError> {
        let mut warnings = Vec::new();

        // Pass 1: claim surviving instances, first-come in chain order. A
        // tailed slot (delay/reverb) is deliberately *not* claimed when
        // spillover is on — reusing it would glue the incoming preset's
        // values onto a ringing buffer (a delay-time glide artifact). It is
        // spilled in pass 2 and rebuilt fresh in pass 3 instead.
        let mut available = self.order.clone();
        let claims: Vec<Option<u8>> = chain
            .iter()
            .map(|state| {
                available
                    .iter()
                    .position(|&i| {
                        let shadow = self.shadow(i as usize);
                        shadow.family.key == state.key && !(spillover && shadow.tail_secs > 0.0)
                    })
                    .map(|pos| available.remove(pos))
            })
            .collect();

        // Pass 2: release unclaimed instances — the preset defines the
        // structure. Tailed slots spill (keep ringing) when spillover is on;
        // the rest are removed. Freed indices become available for the
        // installs below; the engine cancels a pending removal when an index
        // is re-used.
        for &index in &available {
            let shadow = self.shadow(index as usize);
            let key = shadow.family.key;
            let spill = spillover && shadow.tail_secs > 0.0;
            let msg = if spill {
                EngineMsg::SpillSlot { index }
            } else {
                EngineMsg::RemoveSlot { index }
            };
            self.tx.push(msg).map_err(|_| EngineError::QueueFull)?;
            self.slots[index as usize] = None;
            warnings.push(format!(
                "slot {key:?} not in preset — {}",
                if spill { "spilled" } else { "removed" }
            ));
        }

        // Pass 3: build the new order, installing what the claims missed.
        let mut new_order: Vec<u8> = Vec::new();
        for (state, claim) in chain.iter().zip(&claims) {
            let index = match claim {
                Some(index) => *index as usize,
                None => {
                    if new_order.len() >= MAX_SLOTS {
                        warnings.push(format!("chain full — slot {:?} skipped", state.key));
                        continue;
                    }
                    let Some(mut effect) = build(&state.key) else {
                        warnings.push(format!("unknown slot {:?} skipped", state.key));
                        continue;
                    };
                    let Some(index) = self.free_index() else {
                        warnings.push(format!("chain full — slot {:?} skipped", state.key));
                        continue;
                    };
                    effect.prepare(self.sample_rate);
                    self.slots[index] = Some(SlotShadow::from_effect(effect.as_ref()));
                    if self
                        .tx
                        .push(EngineMsg::InstallSlot {
                            index: index as u8,
                            effect,
                        })
                        .is_err()
                    {
                        self.slots[index] = None;
                        return Err(EngineError::QueueFull);
                    }
                    index
                }
            };
            new_order.push(index as u8);
            self.apply_slot_state(index, state, &mut warnings)?;
        }

        self.order = new_order;
        self.push_order()?;
        Ok(warnings)
    }

    /// Human-readable state of every slot and parameter, in processing order.
    pub fn state_lines(&self) -> Vec<String> {
        (0..self.order.len())
            .map(|position| {
                let handle = self.handle_at(position);
                let shadow = self.shadow(self.order[position] as usize);
                let desc = shadow.family.pedals[shadow.pedal];
                let params = desc
                    .params
                    .iter()
                    .enumerate()
                    .map(|(j, p)| {
                        let real = p.range.to_real(shadow.norms[shadow.pedal][j]);
                        if let Some(label) = p.range.label(real) {
                            format!("{} {}", p.key, label)
                        } else if p.unit.is_empty() {
                            format!("{} {:.2}", p.key, real)
                        } else {
                            format!("{} {:.1} {}", p.key, real, p.unit)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                let name = if shadow.family.pedals.len() > 1 {
                    format!("{handle}:{}", desc.key)
                } else {
                    handle
                };
                format!(
                    "{:<12} [{}]  {}",
                    name,
                    if shadow.active { "on " } else { "off" },
                    params
                )
            })
            .collect()
    }

    /// Current block peaks, formatted for the `meter` command.
    pub fn meter_line(&self) -> String {
        format!(
            "in {:>7.1} dB | out {:>7.1} dB",
            lin_to_db(self.telemetry.peak_in()),
            lin_to_db(self.telemetry.peak_out()),
        )
    }
}

/// How many retired effects the chute holds before the audio thread parks
/// them (a whole chain replacement plus headroom).
const RETIRE_CAPACITY: usize = 2 * MAX_SLOTS + 8;

/// Wire up a chain and its control handle. Call [`Chain::prepare`] with the
/// stream's sample rate before processing (and mirror it into the handle via
/// [`ChainHandle::set_sample_rate`] for later installs).
pub fn build_chain(effects: Vec<Box<dyn Effect>>) -> (Chain, ChainHandle) {
    assert!(
        effects.len() <= MAX_SLOTS,
        "chain of {} exceeds MAX_SLOTS {}",
        effects.len(),
        MAX_SLOTS
    );
    let (tx, rx) = rtrb::RingBuffer::new(MSG_CAPACITY);
    let (retired, retired_rx) = rtrb::RingBuffer::new(RETIRE_CAPACITY);
    let telemetry = Arc::new(Telemetry::default());

    let count = effects.len();
    let mut shadows: Vec<Option<SlotShadow>> = Vec::with_capacity(MAX_SLOTS);
    let mut slots: Vec<Option<Slot>> = Vec::with_capacity(MAX_SLOTS);
    for effect in effects {
        shadows.push(Some(SlotShadow::from_effect(effect.as_ref())));
        slots.push(Some(Slot {
            effect,
            wet: Smoothed::new(1.0),
        }));
    }
    while slots.len() < MAX_SLOTS {
        shadows.push(None);
        slots.push(None);
    }
    let order: Vec<u8> = (0..count as u8).collect();

    (
        Chain {
            slots,
            order: order.clone(),
            pending_order: None,
            pending_removes: [false; MAX_SLOTS],
            fade: Smoothed::new(1.0),
            rx,
            retired,
            parked: Vec::with_capacity(MAX_SLOTS),
            sample_rate: 48_000,
            dry_l: Vec::new(),
            dry_r: Vec::new(),
            spill: (0..SPILL_LANES).map(|_| SpillLane::free()).collect(),
            spill_l: Vec::new(),
            spill_r: Vec::new(),
            output: OutputStage::new(),
            telemetry: Arc::clone(&telemetry),
            tap: None,
        },
        ChainHandle {
            tx,
            slots: shadows,
            order,
            sample_rate: 48_000,
            next_free: count % MAX_SLOTS,
            retired_rx,
            eq: GlobalEqState::default(),
            telemetry,
        },
    )
}

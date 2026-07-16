//! Real-time chain runner: a linear chain with runtime reordering.
//!
//! Split ownership: [`Chain`] moves onto the audio thread and is the only
//! thing that touches effects while streams run; [`ChainHandle`] stays on the
//! control thread. They communicate exclusively through a lock-free SPSC ring
//! of [`EngineMsg`] and a couple of atomics — no locks, no allocation on the
//! audio side (white paper §4.1).
//!
//! Click-freeness (white paper §4.2): params are smoothed inside effects,
//! bypass crossfades per slot, and a **reorder** — the one true topology
//! change — rides a short master fade through silence before the new order
//! takes effect.

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use lh_core::preset::SlotState;
use lh_core::{EffectDesc, ParamId, lin_to_db};
use lh_dsp::Effect;
use lh_dsp::smooth::Smoothed;
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

#[derive(Debug, Clone, Copy)]
pub enum EngineMsg {
    SetParam {
        id: ParamId,
        normalized: f32,
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
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown slot {0:?}")]
    UnknownSlot(String),
    #[error("unknown param {param:?} on slot {slot:?}")]
    UnknownParam { slot: String, param: String },
    #[error("bad chain order: {0}")]
    BadOrder(String),
    #[error("control queue full — engine not draining?")]
    QueueFull,
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

/// The audio-thread side. RT rules apply to [`Chain::process`] and
/// [`Chain::reset`]; [`Chain::prepare`] allocates and runs before streams start.
pub struct Chain {
    slots: Vec<Slot>,
    /// Processing order (indices into `slots`).
    order: Vec<u8>,
    /// A reorder waiting for the master fade to reach the bottom.
    pending_order: Option<([u8; MAX_SLOTS], u8)>,
    /// Master output fade: 1.0 in normal operation, dips to ~0 across reorders.
    fade: Smoothed,
    rx: rtrb::Consumer<EngineMsg>,
    dry: Vec<f32>,
    telemetry: Arc<Telemetry>,
    /// Optional raw-input copy for control-thread analyzers (tuner).
    tap: Option<rtrb::Producer<f32>>,
}

impl Chain {
    pub fn prepare(&mut self, sample_rate: u32) {
        for slot in &mut self.slots {
            slot.effect.prepare(sample_rate);
            slot.wet.configure(BYPASS_FADE_MS, sample_rate);
            slot.wet.snap_to_target();
        }
        self.fade.configure(ORDER_FADE_MS, sample_rate);
        self.fade.snap_to_target();
        self.dry = vec![0.0; MAX_BLOCK];
    }

    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.effect.reset();
        }
    }

    /// Install a raw-input tap. Call before the chain moves to the audio
    /// thread; the consumer side is drained by a control thread (tuner).
    pub fn set_input_tap(&mut self, tap: rtrb::Producer<f32>) {
        self.tap = Some(tap);
    }

    pub fn process(&mut self, block: &mut [f32]) {
        for _ in 0..MAX_MSGS_PER_BLOCK {
            match self.rx.pop() {
                Ok(EngineMsg::SetParam { id, normalized }) => {
                    if let Some(slot) = self.slots.get_mut(id.slot as usize) {
                        slot.effect.set_param(id.param as usize, normalized);
                    }
                }
                Ok(EngineMsg::SetActive { slot, active }) => {
                    if let Some(slot) = self.slots.get_mut(slot as usize) {
                        slot.wet.set_target(if active { 1.0 } else { 0.0 });
                    }
                }
                Ok(EngineMsg::SetOrder { order, len }) => {
                    self.pending_order = Some((order, len));
                    self.fade.set_target(0.0);
                }
                Err(_) => break,
            }
        }

        // Apply a pending reorder once the fade is inaudible (≤ -60 dB);
        // then ride back up. Same-order messages still dip — harmless.
        if let Some((order, len)) = self.pending_order
            && self.fade.current() <= 1e-3
        {
            self.order.clear();
            self.order.extend_from_slice(&order[..len as usize]);
            self.pending_order = None;
            self.fade.set_target(1.0);
        }

        self.telemetry
            .peak_in_bits
            .store(peak(block).to_bits(), Ordering::Relaxed);

        // Copy the raw input into the analysis tap (tuner). Lock-free chunk
        // write, drop-on-full: an unread tap must never stall the callback.
        if let Some(tap) = &mut self.tap {
            let n = block.len().min(tap.slots());
            if n > 0
                && let Ok(mut chunk) = tap.write_chunk(n)
            {
                let (a, b) = chunk.as_mut_slices();
                a.copy_from_slice(&block[..a.len()]);
                b.copy_from_slice(&block[a.len()..a.len() + b.len()]);
                chunk.commit_all();
            }
        }

        for chunk in block.chunks_mut(MAX_BLOCK) {
            for i in 0..self.order.len() {
                let Some(slot) = self.slots.get_mut(self.order[i] as usize) else {
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
                    slot.effect.process(chunk);
                    continue;
                }
                // Mid-crossfade: blend processed against the dry copy.
                let dry = &mut self.dry[..chunk.len()];
                dry.copy_from_slice(chunk);
                slot.effect.process(chunk);
                for (s, d) in chunk.iter_mut().zip(dry.iter()) {
                    let w = slot.wet.tick();
                    *s = d + (*s - d) * w;
                }
            }
        }

        if !(self.fade.is_settled() && self.fade.target() == 1.0) {
            for x in block.iter_mut() {
                *x *= self.fade.tick();
            }
        }

        self.telemetry
            .peak_out_bits
            .store(peak(block).to_bits(), Ordering::Relaxed);
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

/// The control-thread side: validates keys, tracks a shadow of the state for
/// display, and feeds the ring.
pub struct ChainHandle {
    tx: rtrb::Producer<EngineMsg>,
    descs: Vec<&'static EffectDesc>,
    norms: Vec<Vec<f32>>,
    active: Vec<bool>,
    order: Vec<u8>,
    telemetry: Arc<Telemetry>,
}

impl ChainHandle {
    pub fn descriptors(&self) -> &[&'static EffectDesc] {
        &self.descs
    }

    pub fn telemetry(&self) -> &Telemetry {
        &self.telemetry
    }

    fn slot_index(&self, slot_key: &str) -> Result<usize, EngineError> {
        self.descs
            .iter()
            .position(|d| d.key == slot_key)
            .ok_or_else(|| EngineError::UnknownSlot(slot_key.to_string()))
    }

    /// Set a parameter from a real-world value (clamped into range).
    pub fn set_param(
        &mut self,
        slot_key: &str,
        param_key: &str,
        real: f32,
    ) -> Result<Applied, EngineError> {
        let slot = self.slot_index(slot_key)?;
        let desc = self.descs[slot];
        let param = desc
            .param_index(param_key)
            .ok_or_else(|| EngineError::UnknownParam {
                slot: slot_key.to_string(),
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
        self.norms[slot][param] = normalized;
        Ok(Applied {
            real: clamped,
            unit: p.unit,
        })
    }

    /// Enable (`true`) or bypass (`false`) a slot.
    pub fn set_active(&mut self, slot_key: &str, active: bool) -> Result<(), EngineError> {
        let slot = self.slot_index(slot_key)?;
        self.tx
            .push(EngineMsg::SetActive {
                slot: slot as u8,
                active,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.active[slot] = active;
        Ok(())
    }

    /// Reorder the chain. `keys` must name every slot exactly once; the
    /// engine fades through silence while switching.
    pub fn set_order(&mut self, keys: &[&str]) -> Result<(), EngineError> {
        if keys.len() != self.descs.len() {
            return Err(EngineError::BadOrder(format!(
                "need all {} slots, got {}",
                self.descs.len(),
                keys.len()
            )));
        }
        let mut order = [0u8; MAX_SLOTS];
        let mut seen = [false; MAX_SLOTS];
        for (i, key) in keys.iter().enumerate() {
            let idx = self.slot_index(key)?;
            if seen[idx] {
                return Err(EngineError::BadOrder(format!("duplicate slot {key:?}")));
            }
            seen[idx] = true;
            order[i] = idx as u8;
        }
        self.tx
            .push(EngineMsg::SetOrder {
                order,
                len: keys.len() as u8,
            })
            .map_err(|_| EngineError::QueueFull)?;
        self.order = order[..keys.len()].to_vec();
        Ok(())
    }

    /// Slot keys in current processing order.
    pub fn order_keys(&self) -> Vec<&'static str> {
        self.order
            .iter()
            .map(|&i| self.descs[i as usize].key)
            .collect()
    }

    /// Capture the chain (order, bypass, real param values) for a preset.
    pub fn snapshot_chain(&self) -> Vec<SlotState> {
        self.order
            .iter()
            .map(|&i| {
                let i = i as usize;
                let desc = self.descs[i];
                SlotState {
                    key: desc.key.to_string(),
                    active: self.active[i],
                    params: desc
                        .params
                        .iter()
                        .enumerate()
                        .map(|(j, p)| (p.key.to_string(), p.range.to_real(self.norms[i][j])))
                        .collect(),
                }
            })
            .collect()
    }

    /// Apply a preset's chain: params, bypass flags, and order. Unknown
    /// slots/params are skipped with a warning (forward compatibility);
    /// slots the preset doesn't mention keep playing, appended at the end.
    pub fn apply_preset_chain(&mut self, chain: &[SlotState]) -> Result<Vec<String>, EngineError> {
        let mut warnings = Vec::new();
        let mut order_keys: Vec<&'static str> = Vec::new();

        for state in chain {
            let Ok(idx) = self.slot_index(&state.key) else {
                warnings.push(format!("unknown slot {:?} skipped", state.key));
                continue;
            };
            order_keys.push(self.descs[idx].key);
            self.set_active(&state.key, state.active)?;
            for (param_key, value) in &state.params {
                match self.set_param(&state.key, param_key, *value) {
                    Ok(_) => {}
                    Err(EngineError::UnknownParam { .. }) => warnings.push(format!(
                        "unknown param {:?} on {:?} skipped",
                        param_key, state.key
                    )),
                    Err(e) => return Err(e),
                }
            }
        }

        for &i in &self.order.clone() {
            let key = self.descs[i as usize].key;
            if !order_keys.contains(&key) {
                order_keys.push(key);
                warnings.push(format!(
                    "slot {key:?} missing from preset — kept at the end"
                ));
            }
        }
        self.set_order(&order_keys)?;
        Ok(warnings)
    }

    /// Human-readable state of every slot and parameter, in processing order.
    pub fn state_lines(&self) -> Vec<String> {
        self.order
            .iter()
            .map(|&i| {
                let i = i as usize;
                let desc = self.descs[i];
                let params = desc
                    .params
                    .iter()
                    .enumerate()
                    .map(|(j, p)| {
                        let real = p.range.to_real(self.norms[i][j]);
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
                format!(
                    "{:<7} [{}]  {}",
                    desc.key,
                    if self.active[i] { "on " } else { "off" },
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

/// Wire up a chain and its control handle. Call [`Chain::prepare`] with the
/// stream's sample rate before processing.
pub fn build_chain(effects: Vec<Box<dyn Effect>>) -> (Chain, ChainHandle) {
    assert!(
        effects.len() <= MAX_SLOTS,
        "chain of {} exceeds MAX_SLOTS {}",
        effects.len(),
        MAX_SLOTS
    );
    let (tx, rx) = rtrb::RingBuffer::new(MSG_CAPACITY);
    let telemetry = Arc::new(Telemetry::default());

    let descs: Vec<&'static EffectDesc> = effects.iter().map(|e| e.descriptor()).collect();
    let norms = descs
        .iter()
        .map(|d| d.params.iter().map(|p| p.default_norm()).collect())
        .collect();
    let active = vec![true; descs.len()];
    let order: Vec<u8> = (0..effects.len() as u8).collect();
    let slots = effects
        .into_iter()
        .map(|effect| Slot {
            effect,
            wet: Smoothed::new(1.0),
        })
        .collect();

    (
        Chain {
            slots,
            order: order.clone(),
            pending_order: None,
            fade: Smoothed::new(1.0),
            rx,
            dry: Vec::new(),
            telemetry: Arc::clone(&telemetry),
            tap: None,
        },
        ChainHandle {
            tx,
            descs,
            norms,
            active,
            order,
            telemetry,
        },
    )
}

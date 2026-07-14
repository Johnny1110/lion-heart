//! Real-time chain runner (M1: fixed linear chain).
//!
//! Split ownership: [`Chain`] moves onto the audio thread and is the only
//! thing that touches effects while streams run; [`ChainHandle`] stays on the
//! control thread. They communicate exclusively through a lock-free SPSC ring
//! of [`EngineMsg`] and a couple of atomics — no locks, no allocation on the
//! audio side (white paper §4.1).

use std::sync::Arc;
use std::sync::atomic::{AtomicU32, Ordering};

use lh_core::{EffectDesc, ParamId, lin_to_db};
use lh_dsp::Effect;
use lh_dsp::smooth::Smoothed;
use thiserror::Error;

/// Engine-internal processing granularity. Device callbacks may hand us
/// bigger blocks; `Chain::process` slices them down to this.
pub const MAX_BLOCK: usize = 1024;
const MSG_CAPACITY: usize = 256;
/// Bypass toggles crossfade over this window instead of hard-switching.
const BYPASS_FADE_MS: f32 = 10.0;
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
}

#[derive(Debug, Error)]
pub enum EngineError {
    #[error("unknown slot {0:?}")]
    UnknownSlot(String),
    #[error("unknown param {param:?} on slot {slot:?}")]
    UnknownParam { slot: String, param: String },
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
    rx: rtrb::Consumer<EngineMsg>,
    dry: Vec<f32>,
    telemetry: Arc<Telemetry>,
}

impl Chain {
    pub fn prepare(&mut self, sample_rate: u32) {
        for slot in &mut self.slots {
            slot.effect.prepare(sample_rate);
            slot.wet.configure(BYPASS_FADE_MS, sample_rate);
            slot.wet.snap_to_target();
        }
        self.dry = vec![0.0; MAX_BLOCK];
    }

    pub fn reset(&mut self) {
        for slot in &mut self.slots {
            slot.effect.reset();
        }
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
                Err(_) => break,
            }
        }

        self.telemetry
            .peak_in_bits
            .store(peak(block).to_bits(), Ordering::Relaxed);

        for chunk in block.chunks_mut(MAX_BLOCK) {
            for slot in &mut self.slots {
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

    /// Human-readable state of every slot and parameter (for `list`).
    pub fn state_lines(&self) -> Vec<String> {
        self.descs
            .iter()
            .enumerate()
            .map(|(i, desc)| {
                let params = desc
                    .params
                    .iter()
                    .enumerate()
                    .map(|(j, p)| {
                        let real = p.range.to_real(self.norms[i][j]);
                        if p.unit.is_empty() {
                            format!("{} {:.2}", p.key, real)
                        } else {
                            format!("{} {:.1} {}", p.key, real, p.unit)
                        }
                    })
                    .collect::<Vec<_>>()
                    .join(" | ");
                format!(
                    "{:<6} [{}]  {}",
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
    let (tx, rx) = rtrb::RingBuffer::new(MSG_CAPACITY);
    let telemetry = Arc::new(Telemetry::default());

    let descs: Vec<&'static EffectDesc> = effects.iter().map(|e| e.descriptor()).collect();
    let norms = descs
        .iter()
        .map(|d| d.params.iter().map(|p| p.default_norm()).collect())
        .collect();
    let active = vec![true; descs.len()];
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
            rx,
            dry: Vec::new(),
            telemetry: Arc::clone(&telemetry),
        },
        ChainHandle {
            tx,
            descs,
            norms,
            active,
            telemetry,
        },
    )
}

//! Lion-Heart as a CLAP/VST3 plugin (M7).
//!
//! The same stereo chain as the standalone app — gate → comp → drive → NAM
//! amp → EQ → mod → delay → reverb → cab → limiter — hosted by nih-plug.
//! Every pedal parameter and per-slot bypass is a host parameter
//! (automatable); values flow through the same lock-free `ChainHandle` ring
//! the app uses, and the effects do their own smoothing.
//!
//! Per-pedal params (PRD 001): multi-pedal slots expose **every pedal's**
//! params statically (`Drive · TS9 · Drive`, `Drive · Evva · Low`, …) plus a
//! stepped pedal selector. Host state per pedal *is* the knob memory —
//! params of unselected pedals rest host-side and land in the chain when
//! their pedal is selected.
//!
//! v1 scope (no custom editor yet):
//! - The chain order is fixed (the default order).
//! - The **preset** parameter loads the NAM capture and cab IR from a
//!   Lion-Heart preset in `~/.lion-heart/presets/` (sorted, 1-based; 0 loads
//!   nothing). Assets are parsed on nih-plug's background thread and
//!   hot-swapped through the same `AssetHandle` seam as the app — the audio
//!   thread never blocks. Knob values stay host-owned: dial the tone in the
//!   standalone app, save a preset, pick it here, then shape with host
//!   automation.
//! - NAM captures are rate-locked (usually 48 kHz): run the host at the
//!   capture's rate or the amp slot politely refuses to load.

use std::num::NonZeroU32;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;

use lh_dsp::Effect;
use lh_dsp::cab::{CabIr, IrAsset};
use lh_dsp::comp::Compressor;
use lh_dsp::delay::Delay;
use lh_dsp::drive::Drive;
use lh_dsp::eq::Eq;
use lh_dsp::gate::NoiseGate;
use lh_dsp::limiter::Limiter;
use lh_dsp::modulation::Modulation;
use lh_dsp::reverb::Reverb;
use lh_dsp::swap::AssetHandle;
use lh_engine::{Chain, ChainHandle, build_chain};
use lh_nam::{NamAmp, NamAsset, load_nam_file};

/// Everything the background task needs to hot-swap assets.
struct AssetRuntime {
    nam: AssetHandle<NamAsset>,
    cab: AssetHandle<IrAsset>,
}

pub struct LionHeartPlugin {
    params: Arc<LionParams>,
    chain: Chain,
    handle: ChainHandle,
    assets: Arc<Mutex<AssetRuntime>>,
    sample_rate: Arc<AtomicU32>,
    /// Last values pushed into the chain, to send only actual changes.
    last_float: Vec<f32>,
    last_pedal: Vec<i32>,
    last_active: Vec<bool>,
    last_preset: i32,
}

#[derive(Debug, Clone, Copy)]
pub enum Task {
    /// Load NAM + IR assets from the n-th preset (0-based, sorted).
    LoadPresetAssets(usize),
}

impl Default for LionHeartPlugin {
    fn default() -> Self {
        let (nam_amp, nam_handle) = NamAmp::new();
        let (cab, cab_handle) = CabIr::new();
        let effects: Vec<Box<dyn Effect>> = vec![
            Box::new(NoiseGate::new()),
            Box::new(Compressor::new()),
            Box::new(Drive::new()),
            Box::new(nam_amp),
            Box::new(Eq::new()),
            Box::new(Modulation::new()),
            Box::new(Delay::new()),
            Box::new(Reverb::new()),
            Box::new(cab),
            Box::new(Limiter::new()),
        ];
        let (chain, handle) = build_chain(effects);
        let params = Arc::new(LionParams::from_families(&handle.families()));
        let last_float = params.floats.iter().map(|sp| sp.param.value()).collect();
        let last_pedal = params.selectors.iter().map(|s| s.param.value()).collect();
        let last_active = params.bypasses.iter().map(|sb| sb.param.value()).collect();
        Self {
            params,
            chain,
            handle,
            assets: Arc::new(Mutex::new(AssetRuntime {
                nam: nam_handle,
                cab: cab_handle,
            })),
            sample_rate: Arc::new(AtomicU32::new(48_000)),
            last_float,
            last_pedal,
            last_active,
            last_preset: 0,
        }
    }
}

impl Plugin for LionHeartPlugin {
    const NAME: &'static str = "Lion-Heart";
    const VENDOR: &'static str = "Lion-Heart project";
    const URL: &'static str = "https://github.com/Johnny1110/lion-heart";
    const EMAIL: &'static str = "jarvan1110@gmail.com";
    const VERSION: &'static str = env!("CARGO_PKG_VERSION");

    const AUDIO_IO_LAYOUTS: &'static [AudioIOLayout] = &[AudioIOLayout {
        main_input_channels: NonZeroU32::new(2),
        main_output_channels: NonZeroU32::new(2),
        ..AudioIOLayout::const_default()
    }];

    type SysExMessage = ();
    type BackgroundTask = Task;

    fn params(&self) -> Arc<dyn Params> {
        self.params.clone()
    }

    fn task_executor(&mut self) -> TaskExecutor<Self> {
        let assets = Arc::clone(&self.assets);
        let sample_rate = Arc::clone(&self.sample_rate);
        Box::new(move |task| match task {
            Task::LoadPresetAssets(index) => {
                let Ok(mut assets) = assets.lock() else {
                    return;
                };
                // Whatever the audio thread retired since last time dies
                // here, on this background thread.
                assets.nam.collect_garbage();
                assets.cab.collect_garbage();
                load_preset_assets(&mut assets, index, sample_rate.load(Ordering::Relaxed));
            }
        })
    }

    fn initialize(
        &mut self,
        _audio_io_layout: &AudioIOLayout,
        buffer_config: &BufferConfig,
        _context: &mut impl InitContext<Self>,
    ) -> bool {
        let sr = buffer_config.sample_rate as u32;
        self.sample_rate.store(sr, Ordering::Relaxed);
        self.chain.prepare(sr);
        true
    }

    fn reset(&mut self) {
        self.chain.reset();
    }

    fn process(
        &mut self,
        buffer: &mut Buffer,
        _aux: &mut AuxiliaryBuffers,
        context: &mut impl ProcessContext<Self>,
    ) -> ProcessStatus {
        // Host parameters → chain messages, changes only. The effects smooth
        // internally, so unsmoothed values are what we want here.
        //
        // Selectors first: a pedal switch re-sends the incoming pedal's
        // host values so the chain matches host state (the host's per-pedal
        // params are the knob memory).
        for (last, sel) in self.last_pedal.iter_mut().zip(&self.params.selectors) {
            let value = sel.param.value();
            if value != *last {
                *last = value;
                let _ = self.handle.set_param(sel.slot, "pedal", value as f32);
                for sp in self
                    .params
                    .floats
                    .iter()
                    .filter(|sp| sp.slot == sel.slot && sp.pedal == value as usize)
                {
                    let _ = self.handle.set_param(sp.slot, sp.key, sp.param.value());
                }
            }
        }
        for (last, sp) in self.last_float.iter_mut().zip(&self.params.floats) {
            let value = sp.param.value();
            if value != *last {
                *last = value;
                // Only the active pedal's knobs reach the chain; the rest
                // stay host-side until their pedal is selected.
                let active = self
                    .params
                    .selectors
                    .iter()
                    .find(|sel| sel.slot == sp.slot)
                    .map(|sel| sel.param.value() as usize)
                    .unwrap_or(0);
                if sp.pedal == active {
                    let _ = self.handle.set_param(sp.slot, sp.key, value);
                }
            }
        }
        for (last, sb) in self.last_active.iter_mut().zip(&self.params.bypasses) {
            let active = sb.param.value();
            if active != *last {
                *last = active;
                let _ = self.handle.set_active(sb.slot, active);
            }
        }
        let preset = self.params.preset.value();
        if preset != self.last_preset {
            self.last_preset = preset;
            if preset > 0 {
                context.execute_background(Task::LoadPresetAssets(preset as usize - 1));
            }
        }

        let channels = buffer.as_slice();
        if channels.len() >= 2 {
            let (left, right) = channels.split_at_mut(1);
            self.chain.process(&mut *left[0], &mut *right[0]);
        }
        ProcessStatus::Normal
    }
}

// --- parameters -------------------------------------------------------------

struct SlotParam {
    slot: &'static str,
    /// Which pedal of the slot's family this param belongs to.
    pedal: usize,
    pedal_key: &'static str,
    key: &'static str,
    param: FloatParam,
}

/// Stepped pedal selector for a multi-pedal slot.
struct SlotSelector {
    slot: &'static str,
    param: IntParam,
}

struct SlotBypass {
    slot: &'static str,
    param: BoolParam,
}

pub struct LionParams {
    floats: Vec<SlotParam>,
    selectors: Vec<SlotSelector>,
    bypasses: Vec<SlotBypass>,
    preset: IntParam,
}

impl LionParams {
    fn from_families(families: &[&'static lh_core::FamilyDesc]) -> Self {
        let mut floats = Vec::new();
        let mut selectors = Vec::new();
        let mut bypasses = Vec::new();
        for family in families {
            let multi = family.pedals.len() > 1;
            for (pi, pedal) in family.pedals.iter().enumerate() {
                for pd in pedal.params {
                    floats.push(SlotParam {
                        slot: family.key,
                        pedal: pi,
                        pedal_key: pedal.key,
                        key: pd.key,
                        param: float_param(family, pedal, multi, pd),
                    });
                }
            }
            if multi {
                let display = *family;
                let parse = *family;
                selectors.push(SlotSelector {
                    slot: family.key,
                    param: IntParam::new(
                        format!("{} · Pedal", family.name),
                        0,
                        IntRange::Linear {
                            min: 0,
                            max: family.pedals.len() as i32 - 1,
                        },
                    )
                    .with_value_to_string(Arc::new(move |v| {
                        display
                            .pedals
                            .get(v as usize)
                            .map(|p| p.name.to_string())
                            .unwrap_or_default()
                    }))
                    .with_string_to_value(Arc::new(move |s| {
                        parse.pedal_index(s.trim()).map(|i| i as i32)
                    })),
                });
            }
            bypasses.push(SlotBypass {
                slot: family.key,
                param: BoolParam::new(format!("{} · Active", family.name), true),
            });
        }
        let preset_names = Arc::new(list_presets());
        let names_for_display = Arc::clone(&preset_names);
        let preset = IntParam::new("Preset (assets)", 0, IntRange::Linear { min: 0, max: 99 })
            .with_value_to_string(Arc::new(move |v| match v {
                0 => "(none)".to_string(),
                v => names_for_display
                    .get(v as usize - 1)
                    .cloned()
                    .unwrap_or_else(|| format!("{v} (no such preset)")),
            }))
            .with_string_to_value(Arc::new(move |s| {
                let s = s.trim();
                if s == "(none)" {
                    return Some(0);
                }
                preset_names
                    .iter()
                    .position(|n| n == s)
                    .map(|i| i as i32 + 1)
                    // Fall back to a leading integer, which also round-trips the
                    // "<n> (no such preset)" display of out-of-range values.
                    .or_else(|| s.split_whitespace().next()?.parse::<i32>().ok())
            }));
        Self {
            floats,
            selectors,
            bypasses,
            preset,
        }
    }
}

/// Build a host parameter from a pedal descriptor entry. Values are real
/// units end to end — the chain re-normalizes internally. Display rounds to
/// two decimals so value ↔ string conversions are a fixed point (required by
/// hosts that let users type values).
fn float_param(
    family: &'static lh_core::FamilyDesc,
    pedal: &'static lh_core::EffectDesc,
    multi: bool,
    pd: &'static lh_core::ParamDesc,
) -> FloatParam {
    let name = if multi {
        format!("{} · {} · {}", family.name, pedal.name, pd.name)
    } else {
        format!("{} · {}", family.name, pd.name)
    };
    let param = match pd.range {
        lh_core::Range::Linear { min, max } => {
            FloatParam::new(name, pd.default, FloatRange::Linear { min, max })
        }
        lh_core::Range::Log { min, max } => {
            // Choose the skew so the host knob's midpoint lands on the
            // geometric mean, matching the engine's log mapping feel.
            let mid = (min * max).sqrt();
            let factor = ((mid - min) / (max - min)).ln() / 0.5f32.ln();
            FloatParam::new(name, pd.default, FloatRange::Skewed { min, max, factor })
        }
        lh_core::Range::Stepped { labels } => {
            let max = (labels.len().max(1) - 1) as f32;
            return FloatParam::new(name, pd.default, FloatRange::Linear { min: 0.0, max })
                .with_step_size(1.0)
                .with_value_to_string(Arc::new(move |v| {
                    labels
                        .get(v.round() as usize)
                        .map(|l| l.to_string())
                        .unwrap_or_default()
                }))
                .with_string_to_value(Arc::new(move |s| {
                    labels
                        .iter()
                        .position(|l| l.eq_ignore_ascii_case(s.trim()))
                        .map(|i| i as f32)
                }));
        }
    };
    param
        .with_unit(unit_static(pd.unit))
        .with_value_to_string(formatters::v2s_f32_rounded(2))
}

/// `FloatParam::with_unit` wants `&'static str`; map our known units.
fn unit_static(unit: &str) -> &'static str {
    match unit {
        "dB" => " dB",
        "ms" => " ms",
        "Hz" => " Hz",
        "s" => " s",
        ":1" => " :1",
        _ => "",
    }
}

// SAFETY: all parameters live inside `self` behind an `Arc` for the plugin's
// lifetime and the vectors are never mutated after construction, so the
// `ParamPtr`s handed out below stay valid as required by the trait.
unsafe impl Params for LionParams {
    fn param_map(&self) -> Vec<(String, ParamPtr, String)> {
        let mut map = Vec::new();
        for sp in &self.floats {
            // Multi-pedal slots qualify the id with the pedal key; single
            // pedal slots keep their pre-v3 ids so host automation holds.
            let multi = self.selectors.iter().any(|s| s.slot == sp.slot);
            let id = if multi {
                format!("{}_{}_{}", sp.slot, sp.pedal_key, sp.key)
            } else {
                format!("{}_{}", sp.slot, sp.key)
            };
            map.push((id, sp.param.as_ptr(), group_for(sp.slot)));
        }
        for sel in &self.selectors {
            map.push((
                format!("{}_pedal", sel.slot),
                sel.param.as_ptr(),
                group_for(sel.slot),
            ));
        }
        for sb in &self.bypasses {
            map.push((
                format!("{}_active", sb.slot),
                sb.param.as_ptr(),
                group_for(sb.slot),
            ));
        }
        map.push(("preset".to_string(), self.preset.as_ptr(), String::new()));
        map
    }
}

fn group_for(slot: &str) -> String {
    slot.to_string()
}

// --- preset asset loading (background thread) --------------------------------

fn presets_dir() -> Option<PathBuf> {
    std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".lion-heart").join("presets"))
}

/// Sorted preset names, same order the standalone app shows.
fn list_presets() -> Vec<String> {
    let Some(dir) = presets_dir() else {
        return Vec::new();
    };
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };
    let mut names: Vec<String> = entries
        .filter_map(|e| e.ok())
        .filter_map(|e| {
            let p = e.path();
            (p.extension().is_some_and(|x| x == "json"))
                .then(|| p.file_stem().map(|s| s.to_string_lossy().into_owned()))
                .flatten()
        })
        .collect();
    names.sort();
    names
}

fn load_preset_assets(assets: &mut AssetRuntime, index: usize, sample_rate: u32) {
    let names = list_presets();
    let Some(name) = names.get(index) else {
        nih_log!("lion-heart: no preset at index {index}");
        return;
    };
    let Some(dir) = presets_dir() else { return };
    let path = dir.join(format!("{name}.json"));
    let preset = match std::fs::read_to_string(&path)
        .map_err(|e| e.to_string())
        .and_then(|json| lh_core::preset::Preset::from_json(&json).map_err(|e| e.to_string()))
    {
        Ok(p) => p,
        Err(e) => {
            nih_log!("lion-heart: preset {name:?}: {e}");
            return;
        }
    };

    match &preset.assets.nam {
        Some(reference) => match lh_assets::resolve_asset(reference, Some(&dir))
            .map_err(|e| e.to_string())
            .and_then(|(p, _)| load_nam_file(&p, sample_rate).map_err(|e| e.to_string()))
        {
            Ok((asset, info)) => {
                if assets.nam.install(asset).is_ok() {
                    nih_log!(
                        "lion-heart: nam loaded ({} @ {} Hz)",
                        info.architecture,
                        info.sample_rate
                    );
                }
            }
            Err(e) => nih_log!("lion-heart: nam: {e}"),
        },
        None => {
            assets.nam.clear();
        }
    }
    match &preset.assets.ir {
        Some(reference) => match lh_assets::resolve_asset(reference, Some(&dir))
            .map_err(|e| e.to_string())
            .and_then(|(p, _)| lh_assets::load_ir(&p, sample_rate).map_err(|e| e.to_string()))
        {
            Ok((asset, info)) => {
                if assets.cab.install(asset).is_ok() {
                    nih_log!("lion-heart: ir loaded ({} samples)", info.used_samples);
                }
            }
            Err(e) => nih_log!("lion-heart: ir: {e}"),
        },
        None => {
            assets.cab.clear();
        }
    }
}

// --- plugin formats -----------------------------------------------------------

impl ClapPlugin for LionHeartPlugin {
    const CLAP_ID: &'static str = "com.github.johnny1110.lion-heart";
    const CLAP_DESCRIPTION: Option<&'static str> =
        Some("Guitar amp & multi-effects: NAM captures, cab IRs, hand-written pedals");
    const CLAP_MANUAL_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_SUPPORT_URL: Option<&'static str> = Some(Self::URL);
    const CLAP_FEATURES: &'static [ClapFeature] = &[
        ClapFeature::AudioEffect,
        ClapFeature::Stereo,
        ClapFeature::Distortion,
    ];
}

impl Vst3Plugin for LionHeartPlugin {
    const VST3_CLASS_ID: [u8; 16] = *b"LionHeartAmpFx01";
    const VST3_SUBCATEGORIES: &'static [Vst3SubCategory] =
        &[Vst3SubCategory::Fx, Vst3SubCategory::Distortion];
}

nih_export_clap!(LionHeartPlugin);
nih_export_vst3!(LionHeartPlugin);

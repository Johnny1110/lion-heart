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
use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::{Arc, Mutex};

use nih_plug::prelude::*;

use lh_dsp::Effect;
use lh_dsp::blocks::swap::AssetHandle;
use lh_dsp::cab::{CabIr, IrAsset};
use lh_dsp::drive::Drive;
use lh_dsp::dynamics::Compressor;
use lh_dsp::dynamics::Limiter;
use lh_dsp::dynamics::NoiseGate;
use lh_dsp::eq::Eq;
use lh_dsp::filter::Filter;
use lh_dsp::modulation::Modulation;
use lh_dsp::time::Delay;
use lh_dsp::time::Reverb;
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
    /// Host-BPM sync (ADR 014): `(pedal index, time_ms)` last pushed onto
    /// the delay slot's `time` param via the sync override, so a repeat
    /// isn't resent every block and a pedal switch is detected even when
    /// the derived time happens to coincide. `None` while not overriding.
    synced_delay: Option<(usize, f32)>,
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
            Box::new(Filter::new()),
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
            synced_delay: None,
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
        // Host-BPM sync (ADR 014): overrides the delay slot's `time` for
        // whichever pedal is active and has `sync` on, ignoring that
        // pedal's own `time` automation until sync goes off again.
        self.apply_tempo_sync(context);

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

impl LionHeartPlugin {
    /// Host-BPM sync for the delay slot (ADR 014). The engine's `sync`
    /// param is a control-side no-op (like `subdivision`) — this is the
    /// "control side" for the plugin, the counterpart of the standalone
    /// session's `apply_tempo_sync`/`tick_tempo`.
    ///
    /// Only the currently active delay pedal matters: its own `sync` division
    /// host param, read directly (no smoothing needed, this runs once per
    /// block, not per sample). When sync is a note division (not *Free*) and
    /// the host reports a tempo, `time = 60000/bpm × the division's beat
    /// ratio` (`lh_core::tempo`), clamped to that pedal's own range, overrides
    /// whatever the host's `time` automation says — the host param stays put,
    /// just ignored, so returning to *Free* snaps cleanly back to it. No host
    /// tempo (e.g. a standalone-hosted CLAP scanner) leaves `time` exactly
    /// where the normal float-forwarding loop above already put it.
    fn apply_tempo_sync(&mut self, context: &mut impl ProcessContext<Self>) {
        let Some(sel) = self.params.selectors.iter().find(|s| s.slot == "delay") else {
            return; // defensive: the fixed chain always has one
        };
        let active = sel.param.value() as usize;
        let find = |key: &str| {
            self.params
                .floats
                .iter()
                .find(|sp| sp.slot == "delay" && sp.pedal == active && sp.key == key)
        };
        let div = find("sync").map_or(0, |sp| sp.param.value().round() as usize);

        if !lh_core::tempo::is_synced(div) {
            // *Free*: hand the slot back to the host's own `time` value for
            // this pedal — the float loop won't, since (from its point of
            // view) that value never changed while we overrode it.
            if self.synced_delay.take().is_some()
                && let Some(sp) = find("time")
            {
                let _ = self.handle.set_param("delay", "time", sp.param.value());
            }
            return;
        }

        let Some(bpm) = context.transport().tempo else {
            return; // no host tempo to follow — leave time where it is
        };
        let Some(time_ms) = synced_time_ms(bpm, div, active) else {
            return;
        };

        // `(active, time_ms)`, not just `time_ms`: a pedal switch whose new
        // derived time coincides with the old one must still repush — the
        // selector-switch code above already clobbered the slot with the
        // new pedal's raw host `time`.
        if self.synced_delay != Some((active, time_ms)) {
            self.synced_delay = Some((active, time_ms));
            let _ = self.handle.set_param("delay", "time", time_ms);
        }
    }
}

/// Pure ADR-014 time math — `lh_core::tempo::synced_time_ms(bpm, div)` clamped
/// to `pedal`'s own range — split out of [`LionHeartPlugin::apply_tempo_sync`]
/// so it is unit-testable without a full `ProcessContext` mock. `None` for a
/// non-finite/non-positive bpm, a *Free* (or out-of-range) division, or an
/// out-of-range pedal index.
fn synced_time_ms(bpm: f64, div: usize, pedal: usize) -> Option<f32> {
    if !(bpm.is_finite() && bpm > 0.0) {
        return None;
    }
    let base = lh_core::tempo::synced_time_ms(bpm as f32, div)?;
    let desc = lh_dsp::time::delay::FAMILY.pedals.get(pedal)?;
    let time = &desc.params[desc.param_index("time")?];
    Some(time.range.clamp(base))
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
                // Filters ship bypassed — no transparent knob position
                // (lh-core owns the flag, shared with the app's default
                // board).
                param: BoolParam::new(
                    format!("{} · Active", family.name),
                    lh_core::default_active(family.key),
                ),
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

// Shared with the standalone app via lh-assets: the sorted preset list is a
// cross-binary contract (the preset parameter indexes into it exactly like
// the app's MIDI PC numbers do).
use lh_assets::{list_presets, presets_dir};

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
    // Cab: primary IR plus an optional blend IR (a second mic, ADR 015).
    let load_pair = |reference: &lh_core::preset::AssetRef| {
        lh_assets::resolve_asset(reference, Some(&dir))
            .map_err(|e| e.to_string())
            .and_then(|(p, _)| lh_assets::load_ir_pair(&p, sample_rate).map_err(|e| e.to_string()))
    };
    match &preset.assets.ir {
        Some(reference) => match load_pair(reference) {
            Ok((a, info)) => {
                // A blend IR that fails to load just leaves the cab single-mic.
                let b = preset
                    .assets
                    .ir_b
                    .as_ref()
                    .and_then(|r| match load_pair(r) {
                        Ok((pair, _)) => Some(pair),
                        Err(e) => {
                            nih_log!("lion-heart: ir blend: {e}");
                            None
                        }
                    });
                let blended = b.is_some();
                if assets.cab.install(Box::new(IrAsset { a, b })).is_ok() {
                    nih_log!(
                        "lion-heart: ir loaded ({} samples{})",
                        info.used_samples,
                        if blended { " + blend" } else { "" }
                    );
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn fixed_chain_matches_the_shared_default() {
        // The plugin's v1 chain is fixed; the app's session registry is
        // pinned to the same constant, so the two binaries cannot drift.
        let plugin = LionHeartPlugin::default();
        let keys: Vec<&str> = plugin.handle.families().iter().map(|f| f.key).collect();
        assert_eq!(keys, lh_core::DEFAULT_CHAIN);
        // The filter slot ships bypassed (PRD 007) — same flag as the app.
        for sb in plugin.params.bypasses.iter() {
            assert_eq!(
                sb.param.value(),
                lh_core::default_active(sb.slot),
                "{} bypass default",
                sb.slot
            );
        }
    }

    // --- host-BPM sync (ADR 014) ---

    #[test]
    fn synced_time_follows_bpm_and_division() {
        // `sync` divisions index into lh_core::tempo::SYNC_DIVISIONS: index 4
        // is "1/4" (a quarter = 500 ms at 120 bpm); digital (pedal 0) spans
        // well past that (20..2000), so no clamping kicks in.
        assert_eq!(synced_time_ms(120.0, 4, 0).unwrap(), 500.0);
        // Index 5 is "1/8." (dotted eighth, ratio 0.75) → 375 ms.
        assert_eq!(synced_time_ms(120.0, 5, 0).unwrap(), 375.0);
        // Index 0 is *Free* — the knob rules, so no synced time.
        assert_eq!(synced_time_ms(120.0, 0, 0), None, "Free division");
    }

    #[test]
    fn synced_time_clamps_to_the_active_pedals_own_range() {
        // Vintage (pedal 2) tops out at 600 ms; a slow 40 bpm quarter note
        // (1500 ms) must clamp down to the pedal's own ceiling, not
        // digital's 2000 ms.
        let vintage_index = lh_dsp::time::delay::FAMILY.pedal_index("vintage").unwrap();
        let time = synced_time_ms(40.0, 4, vintage_index).unwrap();
        assert_eq!(time, 600.0);
    }

    #[test]
    fn synced_time_rejects_absent_or_invalid_tempo() {
        assert_eq!(synced_time_ms(0.0, 4, 0), None, "zero bpm");
        assert_eq!(synced_time_ms(-10.0, 4, 0), None, "negative bpm");
        assert_eq!(synced_time_ms(f64::NAN, 4, 0), None, "NaN bpm");
        assert_eq!(synced_time_ms(120.0, 4, 99), None, "out-of-range pedal");
    }
}

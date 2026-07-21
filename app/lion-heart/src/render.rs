//! Offline re-amp (PRD 014): process a DI recording through a preset entirely
//! off the device. Every effect is buffer-in / buffer-out by the project's
//! real-time rule, so this reuses the *exact* live signal path ([`build_chain`],
//! `apply_preset_chain`, and the same asset loaders), driven by hand instead of
//! a stream. That equivalence is the whole point: a render sounds like the live
//! rig would.
//!
//! A render depends only on the preset and the DI (it is reproducible): the
//! app-global output EQ (environment, not tone) is deliberately left flat. The
//! always-on safety limiter is intrinsic to the chain and still applies.

use std::path::Path;

use lh_assets::wav::WavData;
use lh_core::preset::{AssetRef, Preset};
use lh_dsp::blocks::swap::asset_channel;
use lh_dsp::cab::IrAsset;
use lh_engine::build_chain;
use lh_nam::{NamAsset, load_nam_file};

use crate::session::build_family_effect;

/// The engine's canonical rate — NAM models are rate-locked here (white paper
/// §5.3), so a DI must already be at this rate.
pub const ENGINE_RATE: u32 = 48_000;
/// Offline processing block; ≤ `lh_engine::MAX_BLOCK`.
const BLOCK: usize = 512;
/// Silence run before the DI so the chain finishes installing/settling. Most
/// smoothers settle in tens of ms; slow modulation (e.g. a rotary rotor) keeps
/// spinning up into the take, exactly as it would on a live preset load.
const WARMUP_SECS: f32 = 0.5;

#[derive(Debug)]
pub enum RenderError {
    /// The DI's sample rate is not the engine rate (no offline resampling —
    /// mismatches are surfaced, matching NAM's rate-lock policy).
    RateMismatch { file: u32, engine: u32 },
    /// The engine refused the chain (e.g. the control queue overflowed on a
    /// pathologically large preset — the same ceiling a live load hits).
    Engine(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            RenderError::RateMismatch { file, engine } => write!(
                f,
                "DI is {file} Hz but the engine runs at {engine} Hz — resample the DI to {engine} Hz first"
            ),
            RenderError::Engine(e) => write!(f, "engine: {e}"),
        }
    }
}

impl std::error::Error for RenderError {}

/// The result of a render: interleaved stereo wet output plus any non-fatal
/// warnings (missing/relocated assets, skipped slots).
pub struct Rendered {
    pub samples: Vec<f32>,
    pub warnings: Vec<String>,
}

/// Render `input` (a DI take) through `preset`. `preset_dir` is the fallback
/// directory for resolving the preset's NAM/IR assets (usually the preset's own
/// directory). `tail_secs` of extra silence is rendered after the DI so
/// delay/reverb tails finish.
pub fn render(
    preset: &Preset,
    input: &WavData,
    preset_dir: Option<&Path>,
    tail_secs: f32,
) -> Result<Rendered, RenderError> {
    if input.sample_rate != ENGINE_RATE {
        return Err(RenderError::RateMismatch {
            file: input.sample_rate,
            engine: ENGINE_RATE,
        });
    }
    let mut warnings = Vec::new();

    // Placeholder asset seams; the amp/cab builders replace them with the real
    // handles when `apply_preset_chain` builds those slots.
    let (_, mut nam_handle) = asset_channel::<NamAsset>();
    let (_, mut cab_handle) = asset_channel::<IrAsset>();
    let mut rebuilt = (false, false);

    // Empty chain → the preset defines the whole structure. No global EQ is
    // applied (reproducibility, see the module docs). spillover=false: offline
    // there is nothing to ring out.
    let (mut chain, mut handle) = build_chain(vec![]);
    chain.prepare(ENGINE_RATE);
    handle.set_sample_rate(ENGINE_RATE);
    match handle.apply_preset_chain(&preset.chain, false, &mut |key| {
        build_family_effect(&mut nam_handle, &mut cab_handle, &mut rebuilt, key)
    }) {
        Ok(w) => warnings.extend(w),
        Err(e) => return Err(RenderError::Engine(e.to_string())),
    }

    // Mount the preset's assets into the (just-built) amp/cab. The handles are
    // linked to the effect objects already; the effects pick the asset up on
    // their first process during warm-up.
    if let Some(nam_ref) = &preset.assets.nam {
        match resolve_and_load_nam(nam_ref, preset_dir) {
            Ok((asset, w)) => {
                warnings.extend(w);
                let _ = nam_handle.install(asset);
            }
            Err(e) => warnings.push(format!("nam: {e}")),
        }
    }
    if let Some(ir_ref) = &preset.assets.ir {
        match resolve_and_load_ir(ir_ref, preset.assets.ir_b.as_ref(), preset_dir) {
            Ok((asset, w)) => {
                warnings.extend(w);
                let _ = cab_handle.install(asset);
            }
            Err(e) => warnings.push(format!("ir: {e}")),
        }
    }

    let mut left = [0.0f32; BLOCK];
    let mut right = [0.0f32; BLOCK];

    // Warm-up: drain the install/param messages, swap in the assets, and settle
    // the fades — all on silence, so no tail contaminates the DI output.
    let warm_blocks = (WARMUP_SECS * ENGINE_RATE as f32 / BLOCK as f32).ceil() as usize;
    for _ in 0..warm_blocks {
        left.fill(0.0);
        right.fill(0.0);
        chain.process(&mut left, &mut right);
    }

    // Feed the DI, then the tail. `chain.process` works in place: after it,
    // left/right hold the wet output.
    let channels = input.channels.max(1) as usize;
    let frames = input.frames();
    let tail_frames = (tail_secs.max(0.0) * ENGINE_RATE as f32) as usize;
    let mut out = Vec::with_capacity((frames + tail_frames) * 2);

    let mut i = 0;
    while i < frames {
        let n = BLOCK.min(frames - i);
        for f in 0..n {
            let base = (i + f) * channels;
            let l = input.samples[base];
            // Mono DI feeds both channels; a stereo DI keeps L/R.
            right[f] = if channels >= 2 {
                input.samples[base + 1]
            } else {
                l
            };
            left[f] = l;
        }
        chain.process(&mut left[..n], &mut right[..n]);
        for f in 0..n {
            out.push(left[f]);
            out.push(right[f]);
        }
        i += n;
    }

    let mut t = 0;
    while t < tail_frames {
        let n = BLOCK.min(tail_frames - t);
        left[..n].fill(0.0);
        right[..n].fill(0.0);
        chain.process(&mut left[..n], &mut right[..n]);
        for f in 0..n {
            out.push(left[f]);
            out.push(right[f]);
        }
        t += n;
    }

    Ok(Rendered {
        samples: out,
        warnings,
    })
}

fn resolve_and_load_nam(
    reference: &AssetRef,
    dir: Option<&Path>,
) -> Result<(Box<NamAsset>, Vec<String>), String> {
    let (path, warnings) = lh_assets::resolve_asset(reference, dir).map_err(|e| e.to_string())?;
    let (asset, _info) = load_nam_file(&path, ENGINE_RATE).map_err(|e| e.to_string())?;
    Ok((asset, warnings))
}

fn resolve_and_load_ir(
    a: &AssetRef,
    b: Option<&AssetRef>,
    dir: Option<&Path>,
) -> Result<(Box<IrAsset>, Vec<String>), String> {
    let mut warnings = Vec::new();
    let (a_path, w) = lh_assets::resolve_asset(a, dir).map_err(|e| e.to_string())?;
    warnings.extend(w);
    let (a_pair, _) = lh_assets::load_ir_pair(&a_path, ENGINE_RATE).map_err(|e| e.to_string())?;

    let b_pair = match b {
        Some(b_ref) => match lh_assets::resolve_asset(b_ref, dir) {
            Ok((b_path, w)) => {
                warnings.extend(w);
                match lh_assets::load_ir_pair(&b_path, ENGINE_RATE) {
                    Ok((pair, _)) => Some(pair),
                    Err(e) => {
                        warnings.push(format!("blend ir: {e}"));
                        None
                    }
                }
            }
            Err(e) => {
                warnings.push(format!("blend ir: {e}"));
                None
            }
        },
        None => None,
    };
    Ok((
        Box::new(IrAsset {
            a: a_pair,
            b: b_pair,
        }),
        warnings,
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use lh_core::preset::{PRESET_SCHEMA_VERSION, PresetAssets, SlotState};
    use std::collections::BTreeMap;

    fn preset_with(name: &str, chain: Vec<SlotState>) -> Preset {
        Preset {
            schema_version: PRESET_SCHEMA_VERSION,
            name: name.into(),
            chain,
            assets: PresetAssets::default(),
            snapshots: BTreeMap::new(),
            active_snapshot: None,
        }
    }

    fn di(frames: usize, freq: f32) -> WavData {
        // Mono DI: a tone the drive can bite into.
        let samples: Vec<f32> = (0..frames)
            .map(|i| (i as f32 * std::f32::consts::TAU * freq / ENGINE_RATE as f32).sin() * 0.5)
            .collect();
        WavData {
            samples,
            channels: 1,
            sample_rate: ENGINE_RATE,
        }
    }

    /// A no-asset preset (gate → drive), so the render needs no files on disk.
    fn drive_preset() -> Preset {
        let mut drive_vals = BTreeMap::new();
        drive_vals.insert("drive".to_string(), 8.0);
        drive_vals.insert("level".to_string(), 7.0);
        let mut pedals = BTreeMap::new();
        pedals.insert("ts9".to_string(), drive_vals);
        preset_with(
            "test",
            vec![
                SlotState {
                    key: "gate".into(),
                    ..Default::default()
                },
                SlotState {
                    key: "drive".into(),
                    pedal: Some("ts9".into()),
                    pedals,
                    ..Default::default()
                },
            ],
        )
    }

    #[test]
    fn renders_processed_stereo_output() {
        let input = di(ENGINE_RATE as usize / 2, 220.0); // 0.5 s
        let out = render(&drive_preset(), &input, None, 0.0).unwrap();
        assert!(
            out.warnings.is_empty(),
            "no-asset preset renders cleanly: {:?}",
            out.warnings
        );
        // Stereo interleaved, one frame per input frame (tail 0).
        assert_eq!(out.samples.len(), input.frames() * 2);
        assert!(out.samples.iter().all(|s| s.is_finite()), "no NaN/inf");
        let energy: f32 = out.samples.iter().map(|s| s * s).sum();
        assert!(energy > 0.0, "a driven tone must produce output");
    }

    #[test]
    fn rate_mismatch_is_rejected() {
        let mut input = di(1_000, 220.0);
        input.sample_rate = 44_100;
        assert!(matches!(
            render(&drive_preset(), &input, None, 0.0),
            Err(RenderError::RateMismatch {
                file: 44_100,
                engine: 48_000
            })
        ));
    }

    #[test]
    fn tail_extends_the_output() {
        let input = di(4_800, 220.0); // 0.1 s
        let no_tail = render(&drive_preset(), &input, None, 0.0).unwrap();
        let with_tail = render(&drive_preset(), &input, None, 0.25).unwrap();
        assert_eq!(no_tail.samples.len(), input.frames() * 2);
        assert_eq!(
            with_tail.samples.len(),
            (input.frames() + (0.25 * ENGINE_RATE as f32) as usize) * 2
        );
    }

    #[test]
    fn empty_preset_is_passthrough() {
        // No slots: the DI comes back essentially unchanged (only the always-on
        // safety limiter sits in the path, transparent below its ceiling).
        let input = di(2_048, 110.0);
        let preset = preset_with("empty", vec![]);
        let out = render(&preset, &input, None, 0.0).unwrap();
        assert_eq!(out.samples.len(), input.frames() * 2);
        // L channel should track the (small-signal, unlimited) input closely.
        let mut max_err = 0.0f32;
        for f in 0..input.frames() {
            max_err = max_err.max((out.samples[2 * f] - input.samples[f]).abs());
        }
        assert!(
            max_err < 1e-3,
            "empty chain is near-transparent (err {max_err})"
        );
    }
}

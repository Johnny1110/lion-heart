//! Control-thread asset loading (white paper §4.1: heavy work happens off the
//! audio thread; finished objects are swapped in atomically).
//!
//! Currently: cabinet IRs — WAV decode (via hound), offline sinc resampling
//! to the engine rate, length capping, energy normalization, and partitioned
//! convolver construction. NAM loading lives next to its effect in `lh-nam`.

use std::path::Path;

use fft_convolver::FFTConvolver;
use lh_dsp::cab::{IrAsset, IrPair};
use thiserror::Error;

/// Cab IRs are 20–200 ms; anything longer is reverb, which this block is not.
pub const MAX_IR_SECONDS: f32 = 0.5;
/// Uniform partition size for the convolver — a latency-free compromise
/// between FFT efficiency and per-block cost at 64-frame callbacks.
pub const CONV_BLOCK: usize = 128;

#[derive(Debug, Error)]
pub enum AssetError {
    #[error("cannot read {path}: {source}")]
    Wav { path: String, source: hound::Error },

    #[error("{path} decodes to silence/empty audio")]
    Empty { path: String },

    #[error("convolver rejected the impulse response: {0}")]
    Convolver(String),

    #[error("cannot hash {path}: {source}")]
    Hash {
        path: String,
        source: std::io::Error,
    },

    #[error(
        "asset not found: {path} (sha256 {sha256}).\n\
         Move the file back, or place a file with the same name next to the preset."
    )]
    Missing { path: String, sha256: String },
}

#[derive(Debug, Clone)]
pub struct IrInfo {
    pub source_rate: u32,
    pub source_samples: usize,
    /// Samples actually loaded into the convolver (post resample/trim).
    pub used_samples: usize,
    pub engine_rate: u32,
    pub resampled: bool,
    pub trimmed: bool,
}

impl IrInfo {
    pub fn seconds(&self) -> f32 {
        self.used_samples as f32 / self.engine_rate as f32
    }
}

/// Decode a WAV impulse response and build a single ready-to-swap cabinet
/// (`b` empty). Control-thread only: allocates freely.
pub fn load_ir(path: &Path, engine_rate: u32) -> Result<(Box<IrAsset>, IrInfo), AssetError> {
    let (pair, info) = load_ir_pair(path, engine_rate)?;
    Ok((Box::new(IrAsset { a: pair, b: None }), info))
}

/// Decode a WAV impulse response into one channel-pair of convolvers — the
/// unit the cab's primary or blend IR is built from (ADR 015). Control-thread
/// only: allocates freely.
pub fn load_ir_pair(path: &Path, engine_rate: u32) -> Result<(IrPair, IrInfo), AssetError> {
    let display = path.display().to_string();
    let mut reader = hound::WavReader::open(path).map_err(|source| AssetError::Wav {
        path: display.clone(),
        source,
    })?;
    let spec = reader.spec();

    // First channel only — cab IRs are effectively mono.
    let step = spec.channels.max(1) as usize;
    let mut ir: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader
            .samples::<f32>()
            .step_by(step)
            .collect::<Result<_, _>>(),
        (hound::SampleFormat::Int, bits) => {
            let scale = 1.0 / (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .step_by(step)
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
        }
    }
    .map_err(|source| AssetError::Wav {
        path: display.clone(),
        source,
    })?;

    let source_samples = ir.len();
    let source_rate = spec.sample_rate;
    if source_samples == 0 {
        return Err(AssetError::Empty { path: display });
    }

    let resampled = source_rate != engine_rate;
    if resampled {
        ir = resample_sinc(&ir, source_rate, engine_rate);
    }

    let max_len = (MAX_IR_SECONDS * engine_rate as f32) as usize;
    let trimmed = ir.len() > max_len;
    if trimmed {
        ir.truncate(max_len);
    }

    // Energy normalization: convolution then neither booms nor vanishes,
    // regardless of how hot the IR file was rendered.
    let energy = ir
        .iter()
        .map(|s| f64::from(*s) * f64::from(*s))
        .sum::<f64>();
    if energy > 0.0 {
        let scale = (1.0 / energy.sqrt()) as f32;
        for s in ir.iter_mut() {
            *s *= scale;
        }
    } else {
        return Err(AssetError::Empty { path: display });
    }

    // One convolver per channel of the stereo bus, same IR.
    let build = || -> Result<FFTConvolver<f32>, AssetError> {
        let mut convolver = FFTConvolver::<f32>::default();
        convolver
            .init(CONV_BLOCK, &ir)
            .map_err(|e| AssetError::Convolver(format!("{e:?}")))?;
        Ok(convolver)
    };
    let left = build()?;
    let right = build()?;

    let info = IrInfo {
        source_rate,
        source_samples,
        used_samples: ir.len(),
        engine_rate,
        resampled,
        trimmed,
    };
    Ok((IrPair { left, right }, info))
}

// --- ~/.lion-heart disk layout -----------------------------------------------
//
// Shared by the standalone app and the plugin. Keeping these here (rather
// than copied into each binary) is what guarantees both sides see the same
// preset list in the same order — MIDI PC numbers and the plugin's preset
// parameter both index into it.

/// `~/.lion-heart`, the app-global config/preset directory.
pub fn app_dir() -> Option<std::path::PathBuf> {
    std::env::var_os("HOME").map(|home| std::path::PathBuf::from(home).join(".lion-heart"))
}

/// `~/.lion-heart/presets`.
pub fn presets_dir() -> Option<std::path::PathBuf> {
    app_dir().map(|d| d.join("presets"))
}

/// Preset names on disk in display order (empty when none). The order is part
/// of a cross-binary contract — PC `n` and the plugin's preset parameter
/// address the n-th entry of exactly this list. It is the user's custom
/// arrangement from [`preset_order_path`] where present, with any unlisted
/// presets (e.g. freshly saved ones) sorted in alphabetically after it — so
/// the result is always deterministic and shared by the app and the plugin.
pub fn list_presets() -> Vec<String> {
    apply_preset_order(scan_preset_files(), &read_preset_order())
}

/// The raw alphabetical scan of `presets_dir` for `*.json` stems.
fn scan_preset_files() -> Vec<String> {
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

/// `~/.lion-heart/preset_order`: the user's custom preset order, one name per
/// line. Absent ⇒ plain alphabetical.
pub fn preset_order_path() -> Option<std::path::PathBuf> {
    app_dir().map(|d| d.join("preset_order"))
}

/// Read the saved custom order (one name per line); empty when absent.
pub fn read_preset_order() -> Vec<String> {
    preset_order_path()
        .map(|path| read_order_at(&path))
        .unwrap_or_default()
}

/// Parse an order file: one name per line, trimmed, blanks skipped; an absent
/// or unreadable file is an empty order. Split out for unit testing.
fn read_order_at(path: &Path) -> Vec<String> {
    std::fs::read_to_string(path)
        .map(|text| {
            text.lines()
                .map(str::trim)
                .filter(|l| !l.is_empty())
                .map(String::from)
                .collect()
        })
        .unwrap_or_default()
}

/// Persist a custom preset order (one name per line). Silently no-ops when
/// `$HOME` is unavailable — ordering is a preference, never load-bearing.
pub fn save_preset_order(order: &[String]) {
    let Some(path) = preset_order_path() else {
        return;
    };
    if let Some(dir) = app_dir() {
        let _ = std::fs::create_dir_all(&dir);
    }
    let _ = std::fs::write(&path, order.join("\n"));
}

/// Order `names` (an alphabetical scan) by `saved`: names in `saved` come
/// first, in saved order, skipping any that no longer exist; the rest keep
/// their alphabetical order after. Pure — unit-testable without the disk.
fn apply_preset_order(names: Vec<String>, saved: &[String]) -> Vec<String> {
    let mut ordered: Vec<String> = saved
        .iter()
        .filter(|n| names.contains(n))
        .cloned()
        .collect();
    for n in names {
        if !ordered.contains(&n) {
            ordered.push(n);
        }
    }
    ordered
}

/// SHA-256 of a file's contents, hex-encoded — the identity presets use to
/// reference external assets (white paper §4.3).
pub fn hash_file(path: &Path) -> Result<String, AssetError> {
    use sha2::{Digest, Sha256};
    let map_err = |source| AssetError::Hash {
        path: path.display().to_string(),
        source,
    };
    let mut file = std::fs::File::open(path).map_err(map_err)?;
    let mut hasher = Sha256::new();
    std::io::copy(&mut file, &mut hasher).map_err(map_err)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Resolve a preset's [`AssetRef`] to a real file. Tries the stored path
/// first, then a same-named file in `fallback_dir` (usually the preset's own
/// directory). Hash mismatches load anyway but warn — the user may have
/// legitimately updated the file.
pub fn resolve_asset(
    reference: &lh_core::preset::AssetRef,
    fallback_dir: Option<&Path>,
) -> Result<(std::path::PathBuf, Vec<String>), AssetError> {
    let mut warnings = Vec::new();
    let stored = Path::new(&reference.path);

    let found = if stored.is_file() {
        stored.to_path_buf()
    } else if let Some(dir) = fallback_dir
        && let Some(name) = stored.file_name()
        && dir.join(name).is_file()
    {
        let relocated = dir.join(name);
        warnings.push(format!(
            "asset {} not at its stored path — using {}",
            reference.path,
            relocated.display()
        ));
        relocated
    } else {
        return Err(AssetError::Missing {
            path: reference.path.clone(),
            sha256: reference.sha256.clone(),
        });
    };

    match hash_file(&found) {
        Ok(hash) if hash != reference.sha256 => warnings.push(format!(
            "{} content changed since the preset was saved (hash mismatch) — loading anyway",
            found.display()
        )),
        Ok(_) => {}
        Err(e) => warnings.push(format!("could not verify hash: {e}")),
    }
    Ok((found, warnings))
}

/// Offline windowed-sinc resampler (arbitrary ratio). Quality is bounded by
/// the 64-tap Blackman-windowed kernel (~-74 dB stopband) — plenty for IRs and
/// for a monitor backing track (the song player reuses it per channel);
/// audio-path resampling would want a proper polyphase design.
pub fn resample_sinc(input: &[f32], src_rate: u32, dst_rate: u32) -> Vec<f32> {
    const HALF_TAPS: isize = 32;
    let ratio = f64::from(dst_rate) / f64::from(src_rate);
    // Anti-alias when downsampling: cut at the destination Nyquist.
    let fc = ratio.min(1.0);
    let out_len = (input.len() as f64 * ratio).round() as usize;
    let len = input.len() as isize;

    (0..out_len)
        .map(|n| {
            let t = n as f64 / ratio;
            let i0 = t.floor() as isize;
            let frac = t - i0 as f64;
            let mut acc = 0.0f64;
            for k in (-HALF_TAPS + 1)..=HALF_TAPS {
                let idx = i0 + k;
                if idx < 0 || idx >= len {
                    continue;
                }
                let x = k as f64 - frac;
                let sinc = if x.abs() < 1e-12 {
                    fc
                } else {
                    (std::f64::consts::PI * fc * x).sin() / (std::f64::consts::PI * x)
                };
                let w = 0.42
                    + 0.5 * (std::f64::consts::PI * x / HALF_TAPS as f64).cos()
                    + 0.08 * (2.0 * std::f64::consts::PI * x / HALF_TAPS as f64).cos();
                acc += f64::from(input[idx as usize]) * sinc * w;
            }
            acc as f32
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp_wav(name: &str, rate: u32, channels: u16, samples: &[f32]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("lion-heart-test-{name}.wav"));
        let spec = hound::WavSpec {
            channels,
            sample_rate: rate,
            bits_per_sample: 32,
            sample_format: hound::SampleFormat::Float,
        };
        let mut writer = hound::WavWriter::create(&path, spec).unwrap();
        for s in samples {
            for _ in 0..channels {
                writer.write_sample(*s).unwrap();
            }
        }
        writer.finalize().unwrap();
        path
    }

    #[test]
    fn loads_matching_rate_ir_and_normalizes_energy() {
        // A scaled delta: energy 0.09 → normalization must rescale to 1.0.
        let mut ir = vec![0.0f32; 64];
        ir[0] = 0.3;
        let path = temp_wav("delta", 48_000, 1, &ir);
        let (mut asset, info) = load_ir(&path, 48_000).unwrap();
        assert!(!info.resampled);
        assert!(!info.trimmed);
        assert_eq!(info.used_samples, 64);

        // Convolving a delta input returns the normalized IR itself.
        let input = {
            let mut v = vec![0.0f32; 128];
            v[0] = 1.0;
            v
        };
        let mut out = vec![0.0f32; 128];
        asset.a.left.process(&input, &mut out).unwrap();
        let peak = out.iter().fold(0.0f32, |m, s| m.max(s.abs()));
        assert!((peak - 1.0).abs() < 1e-3, "energy-normalized, got {peak}");
    }

    #[test]
    fn resamples_from_44100() {
        let ir: Vec<f32> = (0..4_410)
            .map(|n| ((n as f32) * 0.001).sin() * 0.5)
            .collect();
        let path = temp_wav("resample", 44_100, 1, &ir);
        let (_asset, info) = load_ir(&path, 48_000).unwrap();
        assert!(info.resampled);
        let expected = (4_410.0f64 * 48_000.0 / 44_100.0).round() as usize;
        assert_eq!(info.used_samples, expected);
    }

    #[test]
    fn takes_first_channel_of_stereo_and_trims_long_files() {
        let ir = vec![0.01f32; 48_000]; // 1 s, over the 0.5 s cap
        let path = temp_wav("stereo-long", 48_000, 2, &ir);
        let (_asset, info) = load_ir(&path, 48_000).unwrap();
        assert!(info.trimmed);
        assert_eq!(info.used_samples, (0.5 * 48_000.0) as usize);
        assert_eq!(info.source_samples, 48_000);
    }

    #[test]
    fn missing_file_and_silence_are_rejected() {
        assert!(load_ir(Path::new("/nonexistent.wav"), 48_000).is_err());
        let path = temp_wav("silent", 48_000, 1, &vec![0.0f32; 256]);
        assert!(matches!(
            load_ir(&path, 48_000),
            Err(AssetError::Empty { .. })
        ));
    }

    #[test]
    fn hashes_and_resolves_assets() {
        let path = temp_wav("hashme", 48_000, 1, &[0.5, 0.25, 0.1]);
        let hash = hash_file(&path).unwrap();
        assert_eq!(hash.len(), 64, "hex sha256");
        assert_eq!(hash_file(&path).unwrap(), hash, "deterministic");

        let reference = lh_core::preset::AssetRef {
            path: path.display().to_string(),
            sha256: hash.clone(),
        };
        // Exact path, matching hash: no warnings.
        let (found, warnings) = resolve_asset(&reference, None).unwrap();
        assert_eq!(found, path);
        assert!(warnings.is_empty());

        // Hash mismatch: loads with a warning.
        let stale = lh_core::preset::AssetRef {
            path: path.display().to_string(),
            sha256: "0".repeat(64),
        };
        let (_, warnings) = resolve_asset(&stale, None).unwrap();
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].contains("hash mismatch"));

        // Stored path gone: relocates via the fallback dir by file name.
        let moved = lh_core::preset::AssetRef {
            path: format!(
                "/nonexistent/dir/{}",
                path.file_name().unwrap().to_string_lossy()
            ),
            sha256: hash,
        };
        let dir = path.parent().unwrap();
        let (found, warnings) = resolve_asset(&moved, Some(dir)).unwrap();
        assert_eq!(found, dir.join(path.file_name().unwrap()));
        assert!(warnings.iter().any(|w| w.contains("stored path")));

        // Nowhere to be found: actionable error.
        assert!(matches!(
            resolve_asset(&moved, None),
            Err(AssetError::Missing { .. })
        ));
    }

    #[test]
    fn sinc_resampler_preserves_a_tone() {
        let sr_in = 44_100;
        let sr_out = 48_000;
        let len = 8_192;
        let tone: Vec<f32> = (0..len)
            .map(|n| (2.0 * std::f32::consts::PI * 1_000.0 * n as f32 / sr_in as f32).sin())
            .collect();
        let out = resample_sinc(&tone, sr_in, sr_out);

        // Interior RMS stays ~ -3.01 dBFS (sine RMS = 1/√2).
        let interior = &out[256..out.len() - 256];
        let rms = (interior.iter().map(|s| f64::from(*s).powi(2)).sum::<f64>()
            / interior.len() as f64)
            .sqrt();
        assert!(
            (rms - std::f64::consts::FRAC_1_SQRT_2).abs() < 0.02,
            "rms {rms}"
        );

        // Zero-crossing count ⇒ frequency preserved at the new rate.
        let crossings = interior
            .windows(2)
            .filter(|w| w[0] < 0.0 && w[1] >= 0.0)
            .count();
        let seconds = interior.len() as f64 / f64::from(sr_out);
        let freq = crossings as f64 / seconds;
        assert!((freq - 1_000.0).abs() < 10.0, "freq {freq}");
    }

    #[test]
    fn preset_order_puts_saved_first_then_new_alphabetically() {
        // Disk scan is alphabetical; saved order is a custom arrangement.
        let names = vec!["a".into(), "b".into(), "c".into(), "d".into()];
        let saved = vec!["c".to_string(), "a".to_string()];
        // Saved names first in saved order; the rest (b, d) keep alpha order.
        assert_eq!(apply_preset_order(names, &saved), ["c", "a", "b", "d"]);

        // A saved name that no longer exists is skipped.
        let names = vec!["a".into(), "b".into()];
        let saved = vec!["gone".to_string(), "b".to_string()];
        assert_eq!(apply_preset_order(names, &saved), ["b", "a"]);

        // No saved order ⇒ the scan order is preserved verbatim (callers pass
        // an already-alphabetical scan, so that means alphabetical).
        let names = vec!["b".into(), "a".into()];
        assert_eq!(apply_preset_order(names.clone(), &[]), names);
    }

    #[test]
    fn reads_order_file_trimming_blanks_and_missing() {
        let path = std::env::temp_dir().join("lion-heart-order-test");
        std::fs::write(&path, "c\n a \n\nb\n").unwrap();
        assert_eq!(read_order_at(&path), ["c", "a", "b"]);
        let _ = std::fs::remove_file(&path);
        assert!(read_order_at(&path).is_empty(), "absent file ⇒ empty order");
    }
}

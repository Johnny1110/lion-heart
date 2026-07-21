//! Plain interleaved-PCM WAV read/write (PRD 014: recording + offline re-amp).
//!
//! `lh-assets` already owns WAV decode for cabinet IRs; this is the general
//! read/write both the recorder and the offline `render` path share. Unlike
//! [`crate::load_ir_pair`], it keeps every channel (a DI take is 1–2 ch) and
//! does not resample, trim, or normalize — a faithful sample round-trip is the
//! whole point. Control-thread only: allocates freely.

use std::path::Path;

use thiserror::Error;

#[derive(Debug, Error)]
pub enum WavError {
    #[error("cannot read {path}: {source}")]
    Read { path: String, source: hound::Error },
    #[error("cannot write {path}: {source}")]
    Write { path: String, source: hound::Error },
    #[error("{path} decodes to silence/empty audio")]
    Empty { path: String },
}

/// Sample format a WAV is written in. Reading auto-detects; writing picks one.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WavBits {
    /// 16-bit signed PCM.
    Int16,
    /// 24-bit signed PCM (the recorder default — plenty of headroom, half the
    /// size of float).
    Int24,
    /// 32-bit IEEE float — bit-exact for our `f32` engine, the render default.
    Float32,
}

impl WavBits {
    /// Map a persisted bit-depth number (16/24/32) onto a format; anything else
    /// falls back to the 24-bit recorder default.
    pub fn from_number(bits: u16) -> Self {
        match bits {
            16 => WavBits::Int16,
            32 => WavBits::Float32,
            _ => WavBits::Int24,
        }
    }

    fn spec(self, channels: u16, sample_rate: u32) -> hound::WavSpec {
        let (bits_per_sample, sample_format) = match self {
            WavBits::Int16 => (16, hound::SampleFormat::Int),
            WavBits::Int24 => (24, hound::SampleFormat::Int),
            WavBits::Float32 => (32, hound::SampleFormat::Float),
        };
        hound::WavSpec {
            channels,
            sample_rate,
            bits_per_sample,
            sample_format,
        }
    }

    /// Convert and write one sample. Integer formats clamp to [-1, 1] so a hot
    /// input cannot wrap; float is written verbatim (lossless).
    fn write_one<W: std::io::Write + std::io::Seek>(
        self,
        writer: &mut hound::WavWriter<W>,
        x: f32,
    ) -> Result<(), hound::Error> {
        match self {
            WavBits::Int16 => writer.write_sample((x.clamp(-1.0, 1.0) * 32_767.0).round() as i16),
            WavBits::Int24 => {
                writer.write_sample((x.clamp(-1.0, 1.0) * 8_388_607.0).round() as i32)
            }
            WavBits::Float32 => writer.write_sample(x),
        }
    }
}

/// Decoded WAV: interleaved `f32` samples plus the format it was read in.
#[derive(Debug, Clone)]
pub struct WavData {
    /// Interleaved samples, `channels` per frame, range roughly [-1, 1].
    pub samples: Vec<f32>,
    pub channels: u16,
    pub sample_rate: u32,
}

impl WavData {
    /// Number of frames (samples per channel).
    pub fn frames(&self) -> usize {
        if self.channels == 0 {
            0
        } else {
            self.samples.len() / self.channels as usize
        }
    }
}

/// Read a WAV into interleaved `f32`, keeping every channel and the source
/// rate (no resampling — the caller decides what a rate mismatch means).
pub fn read(path: &Path) -> Result<WavData, WavError> {
    let display = path.display().to_string();
    let mut reader = hound::WavReader::open(path).map_err(|source| WavError::Read {
        path: display.clone(),
        source,
    })?;
    let spec = reader.spec();
    let samples: Vec<f32> = match (spec.sample_format, spec.bits_per_sample) {
        (hound::SampleFormat::Float, _) => reader.samples::<f32>().collect::<Result<_, _>>(),
        (hound::SampleFormat::Int, bits) => {
            let scale = 1.0 / (1i64 << (bits - 1)) as f32;
            reader
                .samples::<i32>()
                .map(|s| s.map(|v| v as f32 * scale))
                .collect::<Result<_, _>>()
        }
    }
    .map_err(|source| WavError::Read {
        path: display.clone(),
        source,
    })?;
    if samples.is_empty() {
        return Err(WavError::Empty { path: display });
    }
    Ok(WavData {
        samples,
        channels: spec.channels.max(1),
        sample_rate: spec.sample_rate,
    })
}

/// Write interleaved `f32` samples to a WAV in `bits`. `channels` frames the
/// interleaving; the length need not be an exact multiple (a trailing partial
/// frame is written as-is).
pub fn write(
    path: &Path,
    samples: &[f32],
    channels: u16,
    sample_rate: u32,
    bits: WavBits,
) -> Result<(), WavError> {
    let display = path.display().to_string();
    let map_write = |source| WavError::Write {
        path: display.clone(),
        source,
    };
    let mut writer =
        hound::WavWriter::create(path, bits.spec(channels, sample_rate)).map_err(map_write)?;
    for &x in samples {
        bits.write_one(&mut writer, x).map_err(map_write)?;
    }
    writer.finalize().map_err(map_write)?;
    Ok(())
}

/// A streaming WAV writer for the recorder's disk thread: open once, push
/// interleaved chunks as they drain off the tap ring, finalize on stop. Thin
/// over `hound` so the recorder and the offline path share one conversion.
pub struct WavStream<W: std::io::Write + std::io::Seek> {
    writer: hound::WavWriter<W>,
    bits: WavBits,
    path: String,
}

impl WavStream<std::io::BufWriter<std::fs::File>> {
    /// Create a stereo (or `channels`-channel) stream on disk.
    pub fn create(
        path: &Path,
        channels: u16,
        sample_rate: u32,
        bits: WavBits,
    ) -> Result<Self, WavError> {
        let display = path.display().to_string();
        let writer =
            hound::WavWriter::create(path, bits.spec(channels, sample_rate)).map_err(|source| {
                WavError::Write {
                    path: display.clone(),
                    source,
                }
            })?;
        Ok(Self {
            writer,
            bits,
            path: display,
        })
    }
}

impl<W: std::io::Write + std::io::Seek> WavStream<W> {
    /// Append interleaved samples.
    pub fn write(&mut self, samples: &[f32]) -> Result<(), WavError> {
        for &x in samples {
            self.bits
                .write_one(&mut self.writer, x)
                .map_err(|source| WavError::Write {
                    path: self.path.clone(),
                    source,
                })?;
        }
        Ok(())
    }

    /// Flush the WAV header/length and close the file.
    pub fn finalize(self) -> Result<(), WavError> {
        let path = self.path.clone();
        self.writer
            .finalize()
            .map_err(|source| WavError::Write { path, source })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(name: &str) -> std::path::PathBuf {
        std::env::temp_dir().join(format!("lion-heart-wav-{name}.wav"))
    }

    #[test]
    fn float_round_trip_is_bit_exact() {
        // Acceptance #1: a known signal survives write→read with no loss.
        let signal: Vec<f32> = (0..2_000).map(|i| (i as f32 * 0.03).sin() * 0.7).collect();
        let path = tmp("float-rt");
        write(&path, &signal, 2, 48_000, WavBits::Float32).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.channels, 2);
        assert_eq!(back.sample_rate, 48_000);
        assert_eq!(back.samples, signal, "float WAV must round-trip bit-exact");
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn interleave_is_preserved() {
        // L rises, R falls: deinterleaving the read-back must recover both.
        let mut inter = Vec::new();
        for i in 0..500 {
            inter.push(i as f32 / 500.0); // L
            inter.push(-(i as f32) / 500.0); // R
        }
        let path = tmp("interleave");
        write(&path, &inter, 2, 44_100, WavBits::Float32).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.frames(), 500);
        for f in 0..500 {
            assert_eq!(back.samples[2 * f], f as f32 / 500.0);
            assert_eq!(back.samples[2 * f + 1], -(f as f32) / 500.0);
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn int24_round_trip_within_quantization() {
        let signal: Vec<f32> = (0..1_000).map(|i| (i as f32 * 0.05).sin() * 0.9).collect();
        let path = tmp("int24-rt");
        write(&path, &signal, 1, 48_000, WavBits::Int24).unwrap();
        let back = read(&path).unwrap();
        assert_eq!(back.channels, 1);
        let lsb = 1.0 / 8_388_607.0;
        for (a, b) in signal.iter().zip(&back.samples) {
            assert!(
                (a - b).abs() <= 2.0 * lsb,
                "24-bit error {} exceeds 2 LSB",
                (a - b).abs()
            );
        }
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn streaming_write_matches_whole_write() {
        let signal: Vec<f32> = (0..1_500).map(|i| (i as f32 * 0.02).cos() * 0.5).collect();
        let path = tmp("stream");
        {
            let mut s = WavStream::create(&path, 2, 48_000, WavBits::Float32).unwrap();
            // Push in ragged chunks to prove chunk boundaries don't matter.
            s.write(&signal[..300]).unwrap();
            s.write(&signal[300..301]).unwrap();
            s.write(&signal[301..]).unwrap();
            s.finalize().unwrap();
        }
        let back = read(&path).unwrap();
        assert_eq!(back.samples, signal);
        let _ = std::fs::remove_file(&path);
    }

    #[test]
    fn missing_file_errors() {
        let err = read(Path::new("/nonexistent/lion-heart/nope.wav"));
        assert!(matches!(err, Err(WavError::Read { .. })));
    }
}

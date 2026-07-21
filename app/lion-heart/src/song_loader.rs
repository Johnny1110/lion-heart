//! Song-file decoding for the practice song player (PRD 019, Phase 3).
//!
//! Decodes WAV/MP3 via `symphonia` (pure Rust, permissive) into a stereo
//! [`SongBuffer`] resampled to the engine rate. Runs on a background loader
//! thread — **never** the audio thread — so the multi-MB decode + resample is
//! off the real-time path (the player thread only reads the finished buffer).

use std::path::Path;

use lh_dsp::practice::SongBuffer;
use symphonia::core::audio::SampleBuffer;
use symphonia::core::codecs::DecoderOptions;
use symphonia::core::errors::Error as SymError;
use symphonia::core::formats::FormatOptions;
use symphonia::core::io::MediaSourceStream;
use symphonia::core::meta::MetadataOptions;
use symphonia::core::probe::Hint;

/// Decode a song file into a stereo buffer at `engine_rate`. Blocking; call from
/// a loader thread. Mono is duplicated; >2 channels keep the first two.
pub fn load_song(path: &Path, engine_rate: u32) -> Result<SongBuffer, String> {
    let (interleaved, channels, src_rate) = decode(path)?;
    if interleaved.is_empty() {
        return Err("no audio decoded".into());
    }

    // Deinterleave to stereo L/R.
    let (mut l, mut r) = if channels <= 1 {
        (interleaved.clone(), interleaved)
    } else {
        let frames = interleaved.len() / channels;
        let mut l = Vec::with_capacity(frames);
        let mut r = Vec::with_capacity(frames);
        for f in 0..frames {
            l.push(interleaved[f * channels]);
            r.push(interleaved[f * channels + 1]);
        }
        (l, r)
    };

    // Resample each channel to the engine rate (reuse the assets sinc kernel).
    if src_rate != engine_rate {
        l = lh_assets::resample_sinc(&l, src_rate, engine_rate);
        r = lh_assets::resample_sinc(&r, src_rate, engine_rate);
    }

    Ok(SongBuffer {
        l,
        r,
        sample_rate: engine_rate,
    })
}

/// Decode all packets of the default track to interleaved f32.
/// Returns `(samples, channels, sample_rate)`.
fn decode(path: &Path) -> Result<(Vec<f32>, usize, u32), String> {
    let file = std::fs::File::open(path).map_err(|e| format!("open {path:?}: {e}"))?;
    let mss = MediaSourceStream::new(Box::new(file), Default::default());

    let mut hint = Hint::new();
    if let Some(ext) = path.extension().and_then(|e| e.to_str()) {
        hint.with_extension(ext);
    }

    let probed = symphonia::default::get_probe()
        .format(
            &hint,
            mss,
            &FormatOptions::default(),
            &MetadataOptions::default(),
        )
        .map_err(|e| format!("unsupported / unreadable file: {e}"))?;
    let mut format = probed.format;

    let track = format
        .default_track()
        .ok_or_else(|| "no audio track".to_string())?;
    let track_id = track.id;
    let src_rate = track
        .codec_params
        .sample_rate
        .ok_or_else(|| "unknown sample rate".to_string())?;
    let channels = track
        .codec_params
        .channels
        .map(|c| c.count())
        .unwrap_or(2)
        .max(1);

    let mut decoder = symphonia::default::get_codecs()
        .make(&track.codec_params, &DecoderOptions::default())
        .map_err(|e| format!("no decoder: {e}"))?;

    let mut samples: Vec<f32> = Vec::new();
    let mut sample_buf: Option<SampleBuffer<f32>> = None;

    loop {
        let packet = match format.next_packet() {
            Ok(p) => p,
            // Symphonia signals end-of-stream as an IO UnexpectedEof.
            Err(SymError::IoError(e)) if e.kind() == std::io::ErrorKind::UnexpectedEof => break,
            Err(SymError::ResetRequired) => break,
            Err(e) => return Err(format!("read error: {e}")),
        };
        if packet.track_id() != track_id {
            continue;
        }
        match decoder.decode(&packet) {
            Ok(audio) => {
                if sample_buf.is_none() {
                    let spec = *audio.spec();
                    let duration = audio.capacity() as u64;
                    sample_buf = Some(SampleBuffer::new(duration, spec));
                }
                if let Some(buf) = sample_buf.as_mut() {
                    buf.copy_interleaved_ref(audio);
                    samples.extend_from_slice(buf.samples());
                }
            }
            // Decode errors on a single packet are recoverable — skip it.
            Err(SymError::DecodeError(_)) => continue,
            Err(e) => return Err(format!("decode error: {e}")),
        }
    }

    Ok((samples, channels, src_rate))
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Write a mono 16-bit WAV of a 220 Hz sine and decode it back; the
    /// round-tripped signal must carry the tone at the requested engine rate.
    #[test]
    fn round_trips_a_known_wav() {
        let dir = std::env::temp_dir();
        let path = dir.join("lh_song_loader_test.wav");
        let src_rate = 44_100u32;
        let spec = hound::WavSpec {
            channels: 1,
            sample_rate: src_rate,
            bits_per_sample: 16,
            sample_format: hound::SampleFormat::Int,
        };
        {
            let mut w = hound::WavWriter::create(&path, spec).unwrap();
            for i in 0..src_rate {
                // half a second of 220 Hz
                if i >= src_rate / 2 {
                    break;
                }
                let s = (std::f32::consts::TAU * 220.0 * i as f32 / src_rate as f32).sin();
                w.write_sample((s * 30_000.0) as i16).unwrap();
            }
            w.finalize().unwrap();
        }

        let engine_rate = 48_000u32;
        let song = load_song(&path, engine_rate).expect("decode");
        let _ = std::fs::remove_file(&path);

        assert_eq!(song.sample_rate, engine_rate);
        assert_eq!(song.l.len(), song.r.len(), "mono duplicated to stereo");
        // ~0.5 s resampled to 48k ≈ 24000 frames (± resampler edge).
        assert!(
            (song.frames() as i64 - 24_000).abs() < 500,
            "length ~0.5s @ 48k, got {}",
            song.frames()
        );
        // The 220 Hz tone survives decode + resample.
        let g = goertzel(&song.l[2_000..20_000], 48_000.0, 220.0);
        assert!(g > 0.2, "220 Hz survives the round trip: {g}");
    }

    fn goertzel(block: &[f32], sr: f32, freq: f32) -> f32 {
        let w = std::f32::consts::TAU * freq / sr;
        let coeff = 2.0 * w.cos();
        let (mut s1, mut s2) = (0.0f32, 0.0f32);
        for &x in block {
            let s0 = x + coeff * s1 - s2;
            s2 = s1;
            s1 = s0;
        }
        (s1 * s1 + s2 * s2 - coeff * s1 * s2).sqrt() / block.len() as f32
    }
}

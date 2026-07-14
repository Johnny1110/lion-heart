//! Duplex passthrough: input device → lock-free ring → output device.
//!
//! The audio path is mono: one input channel is tapped and written to every
//! output channel. Output starts with a soft-start gain ramp so a hot signal
//! never hits the monitors instantly (white paper §3.3).

use std::sync::Arc;
use std::sync::atomic::Ordering;
use std::time::Instant;

use cpal::traits::StreamTrait;

use crate::stats::{Snapshot, Stats};
use crate::stream::{self, DuplexSpec};
use crate::{DEFAULT_SAMPLE_RATE, IoError};

/// Startup window during which ring under/overruns are expected (streams are
/// still settling) and therefore not counted.
const GRACE_SECONDS: f64 = 0.25;
/// Soft-start ramp length.
const RAMP_SECONDS: f64 = 0.1;

#[derive(Debug, Clone)]
pub struct PassthroughOpts {
    pub input: Option<String>,
    pub output: Option<String>,
    pub sample_rate: u32,
    /// Requested buffer size in frames; `None` = device default.
    pub buffer: Option<u32>,
    /// Input channel to tap, 1-based.
    pub in_channel: u16,
    /// Output gain applied after the ramp, in dB.
    pub gain_db: f32,
    /// Blocks of silence pre-filled into the ring. Each block adds one buffer
    /// period of latency but absorbs callback jitter between the two streams.
    pub prefill_blocks: u32,
}

impl Default for PassthroughOpts {
    fn default() -> Self {
        Self {
            input: None,
            output: None,
            sample_rate: DEFAULT_SAMPLE_RATE,
            buffer: Some(64),
            in_channel: 1,
            gain_db: 0.0,
            prefill_blocks: 1,
        }
    }
}

/// A running passthrough. Dropping it stops the streams.
pub struct Passthrough {
    _in_stream: cpal::Stream,
    _out_stream: cpal::Stream,
    stats: Arc<Stats>,
    pub description: String,
}

impl Passthrough {
    pub fn start(opts: &PassthroughOpts) -> Result<Self, IoError> {
        let setup = stream::resolve(&DuplexSpec {
            input: opts.input.clone(),
            output: opts.output.clone(),
            sample_rate: opts.sample_rate,
            buffer: opts.buffer,
            in_channel: opts.in_channel,
        })?;

        let sr = opts.sample_rate;
        let block = opts.buffer.filter(|&n| n > 0).unwrap_or(256) as usize;
        let grace_frames = (sr as f64 * GRACE_SECONDS) as u64;

        // Capacity is headroom against transient stalls; only occupancy
        // (the prefill) adds latency.
        let capacity = (block * 16).max(4096);
        let (mut producer, mut consumer) = rtrb::RingBuffer::<f32>::new(capacity);
        let prefill = block * opts.prefill_blocks as usize;
        for _ in 0..prefill.min(capacity) {
            let _ = producer.push(0.0);
        }

        let stats = Arc::new(Stats::default());

        let in_channels = setup.in_channels;
        let tap = setup.tap;
        let data_in = {
            let stats = Arc::clone(&stats);
            move |data: &[f32], _info: &cpal::InputCallbackInfo| {
                let t0 = Instant::now();
                stats.in_callbacks.fetch_add(1, Ordering::Relaxed);
                let mut dropped = 0u64;
                for frame in data.chunks_exact(in_channels) {
                    if producer.push(frame[tap]).is_err() {
                        dropped += 1;
                    }
                }
                let frames = (data.len() / in_channels) as u64;
                let total = stats.in_frames.fetch_add(frames, Ordering::Relaxed) + frames;
                if dropped > 0 && total > grace_frames {
                    stats.overrun_frames.fetch_add(dropped, Ordering::Relaxed);
                    stats.overrun_events.fetch_add(1, Ordering::Relaxed);
                }
                stats.record_callback_nanos(t0.elapsed().as_nanos() as u64);
            }
        };

        let out_channels = setup.out_channels;
        let target_gain = db_to_lin(opts.gain_db);
        let ramp_step = target_gain / (sr as f64 * RAMP_SECONDS) as f32;
        let data_out = {
            let stats = Arc::clone(&stats);
            let mut gain = 0.0f32;
            move |data: &mut [f32], _info: &cpal::OutputCallbackInfo| {
                let t0 = Instant::now();
                stats.out_callbacks.fetch_add(1, Ordering::Relaxed);
                let mut missing = 0u64;
                for frame in data.chunks_exact_mut(out_channels) {
                    let sample = consumer.pop().unwrap_or_else(|_| {
                        missing += 1;
                        0.0
                    });
                    if gain < target_gain {
                        gain = (gain + ramp_step).min(target_gain);
                    }
                    let value = sample * gain;
                    for out in frame {
                        *out = value;
                    }
                }
                let frames = (data.len() / out_channels) as u64;
                let total = stats.out_frames.fetch_add(frames, Ordering::Relaxed) + frames;
                if missing > 0 && total > grace_frames {
                    stats.underrun_frames.fetch_add(missing, Ordering::Relaxed);
                    stats.underrun_events.fetch_add(1, Ordering::Relaxed);
                }
                stats.record_callback_nanos(t0.elapsed().as_nanos() as u64);
            }
        };

        let (in_stream, out_stream) = stream::build_pair(&setup, &stats, data_in, data_out)?;
        // Input first: the ring should be filling before the output drains it.
        in_stream.play()?;
        out_stream.play()?;

        let mut description = setup.describe();
        description.push_str(&format!(
            "\nring  : prefill {} block(s) = {:.2} ms added latency\nactual: {} / {} frames per callback (in/out)",
            opts.prefill_blocks,
            prefill as f64 / sr as f64 * 1e3,
            describe_actual_buffer(&in_stream),
            describe_actual_buffer(&out_stream),
        ));

        Ok(Self {
            _in_stream: in_stream,
            _out_stream: out_stream,
            stats,
            description,
        })
    }

    pub fn stats(&self) -> Snapshot {
        self.stats.snapshot()
    }
}

fn db_to_lin(db: f32) -> f32 {
    10f32.powf(db / 20.0)
}

/// The buffer size the backend actually granted, when it can report one.
fn describe_actual_buffer(stream: &cpal::Stream) -> String {
    stream
        .buffer_size()
        .map(|n| n.to_string())
        .unwrap_or_else(|_| "?".into())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn db_to_lin_reference_points() {
        assert!((db_to_lin(0.0) - 1.0).abs() < 1e-6);
        assert!((db_to_lin(-6.0) - 0.5012).abs() < 1e-3);
        assert!((db_to_lin(-60.0) - 0.001).abs() < 1e-6);
        assert!((db_to_lin(6.0) - 1.9953).abs() < 1e-3);
    }
}

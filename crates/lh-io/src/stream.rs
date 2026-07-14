//! Shared duplex-stream plumbing for passthrough and latency measurement.

use std::sync::Arc;
use std::sync::atomic::Ordering;

use cpal::traits::DeviceTrait;

use crate::IoError;
use crate::devices::{self, Direction};
use crate::stats::Stats;

/// User-facing selection of a duplex configuration.
#[derive(Debug, Clone)]
pub struct DuplexSpec {
    pub input: Option<String>,
    pub output: Option<String>,
    pub sample_rate: u32,
    /// Requested buffer size in frames; `None` (or 0 upstream) = device default.
    pub buffer: Option<u32>,
    /// Input channel to tap, 1-based (guitar usually sits on channel 1).
    pub in_channel: u16,
}

/// Resolved devices and stream configs, ready to build streams from.
pub struct DuplexSetup {
    pub in_device: cpal::Device,
    pub out_device: cpal::Device,
    pub in_config: cpal::StreamConfig,
    pub out_config: cpal::StreamConfig,
    pub in_name: String,
    pub out_name: String,
    pub in_channels: usize,
    pub out_channels: usize,
    /// 0-based index of the tapped channel within an input frame.
    pub tap: usize,
    /// Human note when the requested buffer size could not be honoured.
    pub buffer_note: Option<String>,
}

impl DuplexSetup {
    pub fn describe(&self) -> String {
        let buffer = match self.in_config.buffer_size {
            cpal::BufferSize::Fixed(n) => format!("{n} frames (fixed)"),
            cpal::BufferSize::Default => "device default".to_string(),
        };
        let mut s = format!(
            "input : {} — tap ch {} of {} @ {} Hz\noutput: {} — {} ch @ {} Hz\nbuffer: {}",
            self.in_name,
            self.tap + 1,
            self.in_channels,
            self.in_config.sample_rate,
            self.out_name,
            self.out_channels,
            self.out_config.sample_rate,
            buffer,
        );
        if let Some(note) = &self.buffer_note {
            s.push_str(&format!("\nnote  : {note}"));
        }
        s
    }
}

pub fn resolve(spec: &DuplexSpec) -> Result<DuplexSetup, IoError> {
    let host = cpal::default_host();
    let in_device = devices::select(&host, spec.input.as_deref(), Direction::Input)?;
    let out_device = devices::select(&host, spec.output.as_deref(), Direction::Output)?;
    let in_name = devices::device_name(&in_device);
    let out_name = devices::device_name(&out_device);

    let in_default = in_device.default_input_config()?;
    let out_default = out_device.default_output_config()?;
    for format in [in_default.sample_format(), out_default.sample_format()] {
        if format != cpal::SampleFormat::F32 {
            return Err(IoError::UnsupportedFormat(format.to_string()));
        }
    }

    let in_channels = in_default.channels();
    if spec.in_channel == 0 || spec.in_channel > in_channels {
        return Err(IoError::BadChannel {
            requested: spec.in_channel,
            available: in_channels,
        });
    }

    let (buffer_size, buffer_note) = choose_buffer_size(
        spec.buffer,
        buffer_range(in_default.buffer_size()),
        buffer_range(out_default.buffer_size()),
    );

    let in_config = cpal::StreamConfig {
        channels: in_channels,
        sample_rate: spec.sample_rate,
        buffer_size,
    };
    let out_config = cpal::StreamConfig {
        channels: out_default.channels(),
        sample_rate: spec.sample_rate,
        buffer_size,
    };

    Ok(DuplexSetup {
        in_channels: in_channels as usize,
        out_channels: out_default.channels() as usize,
        tap: (spec.in_channel - 1) as usize,
        in_device,
        out_device,
        in_config,
        out_config,
        in_name,
        out_name,
        buffer_note,
    })
}

/// Build the stream pair with xrun/error accounting attached. Streams are
/// returned un-started; callers decide play order (input first, so the ring
/// has data before the first output callback).
pub fn build_pair<DI, DO>(
    setup: &DuplexSetup,
    stats: &Arc<Stats>,
    data_in: DI,
    data_out: DO,
) -> Result<(cpal::Stream, cpal::Stream), IoError>
where
    DI: FnMut(&[f32], &cpal::InputCallbackInfo) + Send + 'static,
    DO: FnMut(&mut [f32], &cpal::OutputCallbackInfo) + Send + 'static,
{
    let err_in = {
        let stats = Arc::clone(stats);
        move |_err| {
            stats.stream_errors.fetch_add(1, Ordering::Relaxed);
        }
    };
    let err_out = {
        let stats = Arc::clone(stats);
        move |_err| {
            stats.stream_errors.fetch_add(1, Ordering::Relaxed);
        }
    };

    let in_stream = setup
        .in_device
        .build_input_stream(setup.in_config, data_in, err_in, None)?;
    let out_stream =
        setup
            .out_device
            .build_output_stream(setup.out_config, data_out, err_out, None)?;
    Ok((in_stream, out_stream))
}

fn buffer_range(size: &cpal::SupportedBufferSize) -> Option<(u32, u32)> {
    match size {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    }
}

/// Decide between `Fixed(n)` and `Default` up front, instead of building a
/// stream and falling back on failure (the data callbacks are consumed by a
/// failed build attempt).
fn choose_buffer_size(
    requested: Option<u32>,
    in_range: Option<(u32, u32)>,
    out_range: Option<(u32, u32)>,
) -> (cpal::BufferSize, Option<String>) {
    let Some(n) = requested.filter(|&n| n > 0) else {
        return (cpal::BufferSize::Default, None);
    };
    for (label, range) in [("input", in_range), ("output", out_range)] {
        if let Some((min, max)) = range
            && !(min..=max).contains(&n)
        {
            return (
                cpal::BufferSize::Default,
                Some(format!(
                    "requested buffer {n} is outside the {label} device range {min}–{max}; \
                     using the device default instead"
                )),
            );
        }
    }
    (cpal::BufferSize::Fixed(n), None)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn buffer_choice_honours_ranges() {
        // No request → default.
        let (size, note) = choose_buffer_size(None, Some((32, 4096)), Some((32, 4096)));
        assert!(matches!(size, cpal::BufferSize::Default));
        assert!(note.is_none());

        // Zero means "device default" too.
        let (size, _) = choose_buffer_size(Some(0), None, None);
        assert!(matches!(size, cpal::BufferSize::Default));

        // In range on both sides → fixed.
        let (size, note) = choose_buffer_size(Some(64), Some((32, 4096)), Some((32, 4096)));
        assert!(matches!(size, cpal::BufferSize::Fixed(64)));
        assert!(note.is_none());

        // Out of range on one side → default, with an explanation.
        let (size, note) = choose_buffer_size(Some(16), Some((32, 4096)), Some((32, 4096)));
        assert!(matches!(size, cpal::BufferSize::Default));
        assert!(note.unwrap().contains("input"));

        // Unknown ranges → trust the request.
        let (size, _) = choose_buffer_size(Some(64), None, None);
        assert!(matches!(size, cpal::BufferSize::Fixed(64)));
    }
}

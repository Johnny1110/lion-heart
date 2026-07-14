//! Device enumeration and selection.

use cpal::traits::{DeviceTrait, HostTrait};

use crate::IoError;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

impl Direction {
    pub fn label(self) -> &'static str {
        match self {
            Direction::Input => "input",
            Direction::Output => "output",
        }
    }
}

/// Capabilities of one direction of a device, derived from its default config.
#[derive(Debug, Clone)]
pub struct PortDesc {
    pub channels: u16,
    pub default_rate: u32,
    pub min_rate: u32,
    pub max_rate: u32,
    pub sample_format: String,
    /// Supported buffer size range in frames, when the backend reports one.
    pub buffer_range: Option<(u32, u32)>,
}

#[derive(Debug, Clone)]
pub struct DeviceDesc {
    /// Position in the host's device list; usable as a selector in [`select`].
    pub index: usize,
    pub name: String,
    pub is_default_input: bool,
    pub is_default_output: bool,
    pub input: Option<PortDesc>,
    pub output: Option<PortDesc>,
}

pub fn host_name() -> String {
    format!("{:?}", cpal::default_host().id())
}

/// Best-effort human-readable device name.
pub(crate) fn device_name(device: &cpal::Device) -> String {
    device
        .description()
        .map(|d| d.name().to_string())
        .unwrap_or_else(|_| "<unknown>".to_string())
}

/// Describe every device on the default host.
pub fn enumerate() -> Result<Vec<DeviceDesc>, IoError> {
    let host = cpal::default_host();
    let default_in = host.default_input_device().map(|d| device_name(&d));
    let default_out = host.default_output_device().map(|d| device_name(&d));

    let mut out = Vec::new();
    for (index, device) in host.devices()?.enumerate() {
        let name = device_name(&device);
        out.push(DeviceDesc {
            index,
            is_default_input: default_in.as_deref() == Some(name.as_str()),
            is_default_output: default_out.as_deref() == Some(name.as_str()),
            input: port_desc(&device, Direction::Input),
            output: port_desc(&device, Direction::Output),
            name,
        });
    }
    Ok(out)
}

/// Pick a device by `spec`: `None` = system default, an integer = index from
/// [`enumerate`], anything else = case-insensitive name substring.
pub fn select(
    host: &cpal::Host,
    spec: Option<&str>,
    dir: Direction,
) -> Result<cpal::Device, IoError> {
    let Some(spec) = spec else {
        let default = match dir {
            Direction::Input => host.default_input_device(),
            Direction::Output => host.default_output_device(),
        };
        return default.ok_or(IoError::NoDefaultDevice(dir.label()));
    };

    if let Ok(index) = spec.parse::<usize>() {
        let device = host
            .devices()?
            .nth(index)
            .ok_or_else(|| IoError::DeviceNotFound(spec.to_string()))?;
        return if supports(&device, dir) {
            Ok(device)
        } else {
            Err(IoError::DirectionUnsupported(
                device_name(&device),
                dir.label(),
            ))
        };
    }

    let needle = spec.to_lowercase();
    for device in host.devices()? {
        if device_name(&device).to_lowercase().contains(&needle) && supports(&device, dir) {
            return Ok(device);
        }
    }
    Err(IoError::DeviceNotFound(spec.to_string()))
}

fn supports(device: &cpal::Device, dir: Direction) -> bool {
    match dir {
        Direction::Input => device.supports_input(),
        Direction::Output => device.supports_output(),
    }
}

fn port_desc(device: &cpal::Device, dir: Direction) -> Option<PortDesc> {
    let default = match dir {
        Direction::Input => device.default_input_config().ok()?,
        Direction::Output => device.default_output_config().ok()?,
    };

    let (mut min_rate, mut max_rate) = (u32::MAX, 0u32);
    let ranges: Vec<_> = match dir {
        Direction::Input => device
            .supported_input_configs()
            .map(|it| it.collect())
            .unwrap_or_default(),
        Direction::Output => device
            .supported_output_configs()
            .map(|it| it.collect())
            .unwrap_or_default(),
    };
    for range in &ranges {
        min_rate = min_rate.min(range.min_sample_rate());
        max_rate = max_rate.max(range.max_sample_rate());
    }
    if ranges.is_empty() {
        min_rate = default.sample_rate();
        max_rate = default.sample_rate();
    }

    let buffer_range = match default.buffer_size() {
        cpal::SupportedBufferSize::Range { min, max } => Some((*min, *max)),
        cpal::SupportedBufferSize::Unknown => None,
    };

    Some(PortDesc {
        channels: default.channels(),
        default_rate: default.sample_rate(),
        min_rate,
        max_rate,
        sample_format: format!("{}", default.sample_format()),
        buffer_range,
    })
}

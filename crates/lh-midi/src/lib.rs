//! MIDI foot control (M6): parse Program Change / Control Change messages,
//! map them to pedalboard actions, and forward events from a midir input
//! connection to the control thread.
//!
//! Threading: midir delivers messages on its own thread. The callback here
//! only parses and forwards over an `mpsc` channel — the control thread
//! (REPL loop / GUI frame tick) drains the channel and applies actions
//! through its own `ChainHandle`/session, so the single-producer engine
//! queue stays single-producer. Nothing in this crate touches the audio
//! thread.

use std::collections::BTreeMap;
use std::sync::mpsc::Sender;

use midir::{MidiInput, MidiInputConnection};
use serde::{Deserialize, Serialize};
use thiserror::Error;

const CLIENT_NAME: &str = "lion-heart";

#[derive(Debug, Error)]
pub enum MidiError {
    #[error("no MIDI input ports available")]
    NoInputs,
    #[error("no MIDI input matches {0:?} (run `lion-heart midi` to list ports)")]
    NotFound(String),
    #[error("MIDI backend error: {0}")]
    Backend(String),
}

/// The two message kinds a foot controller sends. Channels are 0-based.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MidiEvent {
    ProgramChange {
        channel: u8,
        program: u8,
    },
    ControlChange {
        channel: u8,
        controller: u8,
        value: u8,
    },
}

/// Parse one raw MIDI message; anything but PC/CC returns `None`.
pub fn parse_message(bytes: &[u8]) -> Option<MidiEvent> {
    let status = *bytes.first()?;
    let channel = status & 0x0F;
    match status & 0xF0 {
        0xC0 => Some(MidiEvent::ProgramChange {
            channel,
            program: *bytes.get(1)? & 0x7F,
        }),
        0xB0 => Some(MidiEvent::ControlChange {
            channel,
            controller: *bytes.get(1)? & 0x7F,
            value: *bytes.get(2)? & 0x7F,
        }),
        _ => None,
    }
}

/// What the control thread should do in response to an event.
#[derive(Debug, Clone, PartialEq)]
pub enum Action {
    /// PC hit an entry in `pc_presets`.
    LoadPreset(String),
    /// PC beyond the explicit list — callers may fall back to the sorted
    /// preset list at this index.
    LoadPresetIndex(u8),
    /// A CC mapped to `slot.param`; `norm` is the 0..1 controller position.
    SetParam {
        slot: String,
        param: String,
        norm: f32,
    },
    /// A CC mapped to a bare slot key: value ≥ 64 enables, < 64 bypasses.
    SetActive { slot: String, active: bool },
}

/// User mapping, persisted as `~/.lion-heart/midi.json`.
///
/// ```json
/// {
///   "input": "FCB1010",
///   "channel": 1,
///   "pc_presets": ["lead", "rhythm"],
///   "cc": { "11": "drive.level", "80": "gate", "81": "mod.type" }
/// }
/// ```
#[derive(Debug, Default, Clone, Serialize, Deserialize)]
pub struct MidiMap {
    /// Input port to connect to (name substring); `None` = first port.
    #[serde(default)]
    pub input: Option<String>,
    /// Listen on this channel only (1–16); `None` = omni.
    #[serde(default)]
    pub channel: Option<u8>,
    /// Program Change `n` loads the `n`-th name here (0-based).
    #[serde(default)]
    pub pc_presets: Vec<String>,
    /// Controller number (as a JSON key) → `"slot.param"` or `"slot"`.
    #[serde(default)]
    pub cc: BTreeMap<String, String>,
}

impl MidiMap {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("midi map: {e}"))
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("midi map serializes")
    }

    pub fn action_for(&self, event: &MidiEvent) -> Option<Action> {
        let channel = match *event {
            MidiEvent::ProgramChange { channel, .. } | MidiEvent::ControlChange { channel, .. } => {
                channel
            }
        };
        if let Some(only) = self.channel
            && channel != only.saturating_sub(1)
        {
            return None;
        }
        match *event {
            MidiEvent::ProgramChange { program, .. } => {
                match self.pc_presets.get(program as usize) {
                    Some(name) => Some(Action::LoadPreset(name.clone())),
                    None => Some(Action::LoadPresetIndex(program)),
                }
            }
            MidiEvent::ControlChange {
                controller, value, ..
            } => {
                let target = self.cc.get(&controller.to_string())?;
                match target.split_once('.') {
                    Some((slot, param)) => Some(Action::SetParam {
                        slot: slot.to_string(),
                        param: param.to_string(),
                        norm: value as f32 / 127.0,
                    }),
                    None => Some(Action::SetActive {
                        slot: target.clone(),
                        active: value >= 64,
                    }),
                }
            }
        }
    }
}

/// Names of all MIDI input ports, in system order.
pub fn list_inputs() -> Result<Vec<String>, MidiError> {
    let input = MidiInput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(input
        .ports()
        .iter()
        .map(|p| input.port_name(p).unwrap_or_else(|_| "?".into()))
        .collect())
}

/// A live input connection; dropping it disconnects.
pub struct MidiConnection {
    _conn: MidiInputConnection<()>,
    pub port_name: String,
}

/// Connect to a MIDI input and forward parsed events into `tx`.
/// `selector`: name substring or index; `None` takes the first port.
pub fn connect(selector: Option<&str>, tx: Sender<MidiEvent>) -> Result<MidiConnection, MidiError> {
    let input = MidiInput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))?;
    let ports = input.ports();
    if ports.is_empty() {
        return Err(MidiError::NoInputs);
    }
    let names: Vec<String> = ports
        .iter()
        .map(|p| input.port_name(p).unwrap_or_else(|_| "?".into()))
        .collect();
    let index = match selector {
        None => 0,
        Some(sel) => match sel.parse::<usize>() {
            Ok(i) if i < ports.len() => i,
            _ => names
                .iter()
                .position(|n| n.to_lowercase().contains(&sel.to_lowercase()))
                .ok_or_else(|| MidiError::NotFound(sel.to_string()))?,
        },
    };
    let port_name = names[index].clone();
    let conn = input
        .connect(
            &ports[index],
            CLIENT_NAME,
            move |_timestamp, bytes, _| {
                if let Some(event) = parse_message(bytes) {
                    let _ = tx.send(event); // receiver gone = session over
                }
            },
            (),
        )
        .map_err(|e| MidiError::Backend(e.to_string()))?;
    Ok(MidiConnection {
        _conn: conn,
        port_name,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_pc_and_cc_on_any_channel() {
        assert_eq!(
            parse_message(&[0xC2, 5]),
            Some(MidiEvent::ProgramChange {
                channel: 2,
                program: 5
            })
        );
        assert_eq!(
            parse_message(&[0xB0, 11, 127]),
            Some(MidiEvent::ControlChange {
                channel: 0,
                controller: 11,
                value: 127
            })
        );
        // Note-on, truncated messages, empty: ignored.
        assert_eq!(parse_message(&[0x90, 60, 100]), None);
        assert_eq!(parse_message(&[0xC0]), None);
        assert_eq!(parse_message(&[]), None);
    }

    fn map() -> MidiMap {
        MidiMap::from_json(
            r#"{
                "channel": 1,
                "pc_presets": ["lead", "rhythm"],
                "cc": { "11": "drive.level", "80": "gate" }
            }"#,
        )
        .unwrap()
    }

    #[test]
    fn pc_maps_to_named_then_indexed_presets() {
        let m = map();
        let pc = |program| MidiEvent::ProgramChange {
            channel: 0,
            program,
        };
        assert_eq!(
            m.action_for(&pc(0)),
            Some(Action::LoadPreset("lead".into()))
        );
        assert_eq!(
            m.action_for(&pc(1)),
            Some(Action::LoadPreset("rhythm".into()))
        );
        assert_eq!(m.action_for(&pc(7)), Some(Action::LoadPresetIndex(7)));
    }

    #[test]
    fn cc_maps_to_params_and_bypass() {
        let m = map();
        let cc = |controller, value| MidiEvent::ControlChange {
            channel: 0,
            controller,
            value,
        };
        assert_eq!(
            m.action_for(&cc(11, 127)),
            Some(Action::SetParam {
                slot: "drive".into(),
                param: "level".into(),
                norm: 1.0
            })
        );
        assert_eq!(
            m.action_for(&cc(80, 100)),
            Some(Action::SetActive {
                slot: "gate".into(),
                active: true
            })
        );
        assert_eq!(
            m.action_for(&cc(80, 10)),
            Some(Action::SetActive {
                slot: "gate".into(),
                active: false
            })
        );
        assert_eq!(m.action_for(&cc(42, 64)), None, "unmapped CC is ignored");
    }

    #[test]
    fn channel_filter_drops_other_channels() {
        let m = map(); // channel: 1 → 0-based 0
        let on_ch = |channel| MidiEvent::ProgramChange {
            channel,
            program: 0,
        };
        assert!(m.action_for(&on_ch(0)).is_some());
        assert_eq!(m.action_for(&on_ch(1)), None);

        let omni = MidiMap::default();
        assert!(omni.action_for(&on_ch(9)).is_some(), "omni hears all");
    }

    #[test]
    fn default_map_round_trips_and_pc_falls_back_to_index() {
        let m = MidiMap::from_json(&MidiMap::default().to_json_pretty()).unwrap();
        assert_eq!(
            m.action_for(&MidiEvent::ProgramChange {
                channel: 3,
                program: 2
            }),
            Some(Action::LoadPresetIndex(2)),
            "zero-config: PC n loads the n-th preset"
        );
    }
}

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

use midir::{Ignore, MidiInput, MidiInputConnection};
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

/// The message kinds the control thread cares about: the two channel-voice
/// messages a foot controller sends (channels are 0-based) and the system
/// realtime clock family (PRD 012 — the global tempo's MIDI source).
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
    /// 0xF8 timing tick, 24 per quarter note. `stamp_us` is the driver's
    /// arrival timestamp — tick *intervals* carry the tempo, and the control
    /// thread drains events in frame-sized batches, so wall-clock-at-drain
    /// would quantize a 20 ms interval to nothing.
    Clock {
        stamp_us: u64,
    },
    /// 0xFA — the sender restarts; the tick phase begins fresh.
    Start,
    /// 0xFC — the clock pauses; tempo freezes at its last value.
    Stop,
}

/// Parse one raw MIDI message; `stamp_us` is attached to clock ticks.
/// Anything but PC/CC/realtime-clock returns `None`.
pub fn parse_message(bytes: &[u8], stamp_us: u64) -> Option<MidiEvent> {
    let status = *bytes.first()?;
    match status {
        0xF8 => return Some(MidiEvent::Clock { stamp_us }),
        0xFA => return Some(MidiEvent::Start),
        0xFC => return Some(MidiEvent::Stop),
        _ => {}
    }
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
    /// A CC mapped to `slot.param`; `norm` is the shaped 0..1 landing
    /// position. `pickup` asks the applier for soft-takeover: hold the
    /// value until the controller crosses the parameter (PRD 008).
    SetParam {
        slot: String,
        param: String,
        norm: f32,
        pickup: bool,
    },
    /// A CC mapped to a bare slot key: value ≥ 64 enables, < 64 bypasses.
    SetActive { slot: String, active: bool },
}

/// One CC assignment: the legacy bare target string, or the shaped form
/// with a landing range, taper, and soft-takeover (PRD 008). Serde-untagged
/// keeps old `midi.json` files (and learn-written entries) as plain strings.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CcMapping {
    /// `"slot.param"` or `"slot"` — full range, linear, no pickup.
    Target(String),
    Shaped(CcShape),
}

impl CcMapping {
    /// The `"slot.param"` / `"slot"` string this mapping points at.
    pub fn target(&self) -> &str {
        match self {
            CcMapping::Target(t) => t,
            CcMapping::Shaped(s) => &s.target,
        }
    }
}

/// The shaped form of a CC entry.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct CcShape {
    pub target: String,
    /// Normalized landing zone endpoints; `min > max` inverts the pedal.
    #[serde(default = "shape_min_default")]
    pub min: f32,
    #[serde(default = "shape_max_default")]
    pub max: f32,
    #[serde(default)]
    pub curve: Curve,
    /// Soft-takeover: stay silent after a desync until the pedal sweeps
    /// across the parameter's current value.
    #[serde(default)]
    pub pickup: bool,
}

fn shape_min_default() -> f32 {
    0.0
}
fn shape_max_default() -> f32 {
    1.0
}

impl CcShape {
    /// Shape a raw 0..1 controller position into the landing zone.
    pub fn apply(&self, raw: f32) -> f32 {
        let curved = match self.curve {
            Curve::Linear => raw,
            Curve::Audio => raw * raw,
        };
        (self.min + (self.max - self.min) * curved).clamp(0.0, 1.0)
    }
}

/// Controller-to-parameter taper.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Curve {
    #[default]
    Linear,
    /// x² — the log-taper feel a volume pedal wants.
    Audio,
}

/// User mapping, persisted as `~/.lion-heart/midi.json`.
///
/// ```json
/// {
///   "input": "FCB1010",
///   "channel": 1,
///   "pc_presets": ["lead", "rhythm"],
///   "cc": {
///     "11": { "target": "filter.pos", "pickup": true },
///     "7":  { "target": "amp.output", "curve": "audio" },
///     "80": "gate",
///     "81": "mod.type"
///   }
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
    /// Controller number (as a JSON key) → target, bare or shaped.
    #[serde(default)]
    pub cc: BTreeMap<String, CcMapping>,
}

impl MidiMap {
    pub fn from_json(json: &str) -> Result<Self, String> {
        serde_json::from_str(json).map_err(|e| format!("midi map: {e}"))
    }

    pub fn to_json_pretty(&self) -> String {
        serde_json::to_string_pretty(self).expect("midi map serializes")
    }

    /// Whether an event passes the channel filter (learn listens through
    /// the same gate the mappings do).
    pub fn on_channel(&self, event: &MidiEvent) -> bool {
        let channel = match *event {
            MidiEvent::ProgramChange { channel, .. } | MidiEvent::ControlChange { channel, .. } => {
                channel
            }
            // System realtime has no channel — it always passes.
            MidiEvent::Clock { .. } | MidiEvent::Start | MidiEvent::Stop => return true,
        };
        self.channel
            .is_none_or(|only| channel == only.saturating_sub(1))
    }

    pub fn action_for(&self, event: &MidiEvent) -> Option<Action> {
        if !self.on_channel(event) {
            return None;
        }
        match *event {
            // Clock is tempo, not a mappable action — the session's drain
            // loop consumes it before asking for an action.
            MidiEvent::Clock { .. } | MidiEvent::Start | MidiEvent::Stop => None,
            MidiEvent::ProgramChange { program, .. } => {
                match self.pc_presets.get(program as usize) {
                    Some(name) => Some(Action::LoadPreset(name.clone())),
                    None => Some(Action::LoadPresetIndex(program)),
                }
            }
            MidiEvent::ControlChange {
                controller, value, ..
            } => {
                let mapping = self.cc.get(&controller.to_string())?;
                let shape = match mapping {
                    CcMapping::Target(_) => None,
                    CcMapping::Shaped(s) => Some(s),
                };
                match mapping.target().split_once('.') {
                    Some((slot, param)) => {
                        let raw = value as f32 / 127.0;
                        Some(Action::SetParam {
                            slot: slot.to_string(),
                            param: param.to_string(),
                            norm: shape.map_or(raw, |s| s.apply(raw)),
                            pickup: shape.is_some_and(|s| s.pickup),
                        })
                    }
                    // Shaping is for continuous params; a bare-slot bypass
                    // target keeps the value ≥ 64 rule whatever the form.
                    None => Some(Action::SetActive {
                        slot: mapping.target().to_string(),
                        active: value >= 64,
                    }),
                }
            }
        }
    }

    /// The controller bound to `slot.param`, if any (GUI badges).
    pub fn cc_for_param(&self, slot: &str, param: &str) -> Option<u8> {
        let want = format!("{slot}.{param}");
        self.cc.iter().find_map(|(cc, mapping)| {
            (mapping.target() == want)
                .then(|| cc.parse().ok())
                .flatten()
        })
    }

    /// Bind a controller to `slot.param` (MIDI learn writes the simple
    /// string form); returns the target it displaced, if any.
    pub fn bind_cc(&mut self, controller: u8, slot: &str, param: &str) -> Option<String> {
        self.cc
            .insert(
                controller.to_string(),
                CcMapping::Target(format!("{slot}.{param}")),
            )
            .map(|old| old.target().to_string())
    }

    /// Remove the binding for `slot.param`; returns the controller it sat on.
    pub fn unbind_param(&mut self, slot: &str, param: &str) -> Option<u8> {
        let controller = self.cc_for_param(slot, param)?;
        self.cc.remove(&controller.to_string());
        Some(controller)
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
    let mut input = MidiInput::new(CLIENT_NAME).map_err(|e| MidiError::Backend(e.to_string()))?;
    // Explicit: never filter system realtime — clock ticks are the tempo
    // source (PRD 012).
    input.ignore(Ignore::None);
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
            move |timestamp, bytes, _| {
                if let Some(event) = parse_message(bytes, timestamp) {
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
            parse_message(&[0xC2, 5], 0),
            Some(MidiEvent::ProgramChange {
                channel: 2,
                program: 5
            })
        );
        assert_eq!(
            parse_message(&[0xB0, 11, 127], 0),
            Some(MidiEvent::ControlChange {
                channel: 0,
                controller: 11,
                value: 127
            })
        );
        // Note-on, truncated messages, empty: ignored.
        assert_eq!(parse_message(&[0x90, 60, 100], 0), None);
        assert_eq!(parse_message(&[0xC0], 0), None);
        assert_eq!(parse_message(&[], 0), None);
    }

    /// System realtime (PRD 012): clock ticks carry their driver timestamp,
    /// pass every channel filter, and map to no action.
    #[test]
    fn realtime_clock_parses_with_stamp_and_stays_actionless() {
        assert_eq!(
            parse_message(&[0xF8], 12_345),
            Some(MidiEvent::Clock { stamp_us: 12_345 })
        );
        assert_eq!(parse_message(&[0xFA], 7), Some(MidiEvent::Start));
        assert_eq!(parse_message(&[0xFC], 7), Some(MidiEvent::Stop));
        // 0xFE active sense / 0xFB continue: not ours, ignored.
        assert_eq!(parse_message(&[0xFE], 7), None);
        assert_eq!(parse_message(&[0xFB], 7), None);

        let m = map(); // channel-filtered map
        let tick = MidiEvent::Clock { stamp_us: 1 };
        assert!(m.on_channel(&tick), "realtime has no channel");
        assert_eq!(m.action_for(&tick), None);
        assert_eq!(m.action_for(&MidiEvent::Start), None);
        assert_eq!(m.action_for(&MidiEvent::Stop), None);
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
                norm: 1.0,
                pickup: false
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

    /// The shaped object form: range, inversion, taper, pickup (PRD 008).
    #[test]
    fn shaped_cc_lands_in_its_zone() {
        let m = MidiMap::from_json(
            r#"{
                "cc": {
                    "11": { "target": "filter.pos", "min": 0.25, "max": 0.75,
                            "pickup": true },
                    "7":  { "target": "amp.output", "curve": "audio" },
                    "12": { "target": "mod.depth", "min": 1.0, "max": 0.0 }
                }
            }"#,
        )
        .unwrap();
        let cc = |controller, value| MidiEvent::ControlChange {
            channel: 0,
            controller,
            value,
        };
        // min/max bound the landing zone; pickup surfaces on the action.
        assert_eq!(
            m.action_for(&cc(11, 127)),
            Some(Action::SetParam {
                slot: "filter".into(),
                param: "pos".into(),
                norm: 0.75,
                pickup: true
            })
        );
        assert_eq!(
            m.action_for(&cc(11, 0)),
            Some(Action::SetParam {
                slot: "filter".into(),
                param: "pos".into(),
                norm: 0.25,
                pickup: true
            })
        );
        // Audio taper: half travel lands at a quarter (x²).
        let Some(Action::SetParam { norm, .. }) = m.action_for(&cc(7, 64)) else {
            panic!("cc 7 must map");
        };
        assert!((norm - (64.0 / 127.0f32).powi(2)).abs() < 1e-6);
        // min > max inverts the pedal.
        let Some(Action::SetParam { norm, .. }) = m.action_for(&cc(12, 127)) else {
            panic!("cc 12 must map");
        };
        assert_eq!(norm, 0.0);
    }

    #[test]
    fn shaped_and_bare_entries_round_trip() {
        let mut m = MidiMap::default();
        m.cc.insert("80".into(), CcMapping::Target("gate".into()));
        m.cc.insert(
            "11".into(),
            CcMapping::Shaped(CcShape {
                target: "filter.pos".into(),
                min: 0.1,
                max: 0.9,
                curve: Curve::Audio,
                pickup: true,
            }),
        );
        let json = m.to_json_pretty();
        let back = MidiMap::from_json(&json).unwrap();
        assert_eq!(back.cc, m.cc);
        // The bare form stays a plain string on disk (old files, and the
        // entries learn writes, keep the simple shape).
        assert!(json.contains("\"80\": \"gate\""), "bare form: {json}");
    }

    #[test]
    fn learn_binds_unbinds_and_reports_displacement() {
        let mut m = map();
        assert_eq!(m.cc_for_param("drive", "level"), Some(11));
        assert_eq!(m.cc_for_param("filter", "pos"), None);
        // Learning CC 11 onto a new target displaces the old one.
        assert_eq!(m.bind_cc(11, "filter", "pos"), Some("drive.level".into()));
        assert_eq!(m.cc_for_param("filter", "pos"), Some(11));
        assert_eq!(m.cc_for_param("drive", "level"), None);
        assert_eq!(m.unbind_param("filter", "pos"), Some(11));
        assert_eq!(m.cc_for_param("filter", "pos"), None);
        assert_eq!(m.unbind_param("filter", "pos"), None);
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

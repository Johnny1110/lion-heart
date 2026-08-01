//! MIDI runtime: connection, soft-takeover, and map I/O.

use super::*;

use std::collections::HashMap;

/// A live MIDI input: the connection, its event stream, and the mapping.
pub(crate) struct MidiRuntime {
    pub(super) _conn: lh_midi::MidiConnection,
    pub(super) rx: std::sync::mpsc::Receiver<lh_midi::MidiEvent>,
    pub(super) map: lh_midi::MidiMap,
    /// Soft-takeover state per controller number (PRD 008).
    pub(super) pickup: HashMap<u8, PickupState>,
    /// Armed MIDI-learn target: the next on-channel CC binds to it.
    pub(super) learn: Option<(String, String)>,
}

/// One controller's soft-takeover engagement.
#[derive(Default)]
pub(super) struct PickupState {
    pub(super) engaged: bool,
    /// The last shaped position, for crossing detection.
    pub(super) last: Option<f32>,
}

/// How close (normalized) a pickup-gated pedal must land to the parameter
/// to engage without sweeping across it.
pub(super) const PICKUP_WINDOW: f32 = 0.02;

impl PickupState {
    /// Feed one shaped pedal position given the parameter's current value;
    /// returns whether the controller is (now) engaged. Engagement happens
    /// on a sweep across the value or a landing within [`PICKUP_WINDOW`].
    pub(super) fn feed(&mut self, current: f32, shaped: f32) -> bool {
        if self.engaged {
            return true;
        }
        let crossed = self
            .last
            .is_some_and(|prev| (prev - current) * (shaped - current) <= 0.0);
        let close = (shaped - current).abs() <= PICKUP_WINDOW;
        self.last = Some(shaped);
        self.engaged = crossed || close;
        self.engaged
    }
}

/// Write `~/.lion-heart/midi.json` (learn/unbind persist the whole map,
/// keeping input/channel/pc_presets). A warning line on failure.
pub(crate) fn save_midi_map(map: &lh_midi::MidiMap) -> Option<String> {
    let Some(dir) = lh_assets::app_dir() else {
        return Some("warning: cannot determine home directory — midi map not saved".into());
    };
    let write = || -> std::io::Result<()> {
        std::fs::create_dir_all(&dir)?;
        std::fs::write(dir.join("midi.json"), map.to_json_pretty())
    };
    write()
        .err()
        .map(|e| format!("warning: could not save midi map: {e}"))
}

/// Read `~/.lion-heart/midi.json` (defaults when absent, warning on bad JSON).
pub(crate) fn load_midi_map() -> (lh_midi::MidiMap, Option<String>) {
    let Some(path) = lh_assets::app_dir().map(|d| d.join("midi.json")) else {
        return (lh_midi::MidiMap::default(), None);
    };
    match std::fs::read_to_string(&path) {
        Ok(json) => match lh_midi::MidiMap::from_json(&json) {
            Ok(map) => (map, None),
            Err(e) => (
                lh_midi::MidiMap::default(),
                Some(format!("{}: {e}", path.display())),
            ),
        },
        Err(_) => (lh_midi::MidiMap::default(), None),
    }
}

/// Try to bring up MIDI: never fatal — a pedalboard without a foot
/// controller must still start. Zero config connects the first port; PC `n`
/// then loads the n-th preset.
pub(crate) fn connect_midi(override_port: Option<&str>) -> (Option<MidiRuntime>, String) {
    let (map, warning) = load_midi_map();
    let selector = override_port
        .map(str::to_string)
        .or_else(|| map.input.clone());
    let (tx, rx) = std::sync::mpsc::channel();
    let result = lh_midi::connect(selector.as_deref(), tx);
    let with_warning = |status: String| match &warning {
        Some(w) => format!("{status} — warning: {w}"),
        None => status,
    };
    match result {
        Ok(conn) => {
            let status = with_warning(format!("midi: {}", conn.port_name));
            (
                Some(MidiRuntime {
                    _conn: conn,
                    rx,
                    map,
                    pickup: HashMap::new(),
                    learn: None,
                }),
                status,
            )
        }
        Err(e) => (None, with_warning(format!("midi: none ({e})"))),
    }
}

impl super::Session {
    /// Apply everything the foot controller sent since the last call.
    /// Returns user-facing lines describing what happened.
    pub fn drain_midi(&mut self) -> Vec<String> {
        let Some(midi) = &self.midi else {
            return Vec::new();
        };
        let events: Vec<lh_midi::MidiEvent> = midi.rx.try_iter().collect();
        let mut lines = Vec::new();
        for event in events {
            self.apply_midi_event(event, &mut lines);
        }
        lines
    }

    fn apply_midi_event(&mut self, event: lh_midi::MidiEvent, lines: &mut Vec<String>) {
        // System realtime is tempo, not mapping (PRD 012).
        match event {
            lh_midi::MidiEvent::Clock { stamp_us } => {
                self.on_clock_tick(stamp_us, lines);
                return;
            }
            lh_midi::MidiEvent::Start | lh_midi::MidiEvent::Stop => {
                // Fresh phase on start; a stop freezes the tempo where it is.
                self.tempo.clock_last_us = None;
                self.tempo.clock_intervals.clear();
                return;
            }
            _ => {}
        }
        let controller = match event {
            lh_midi::MidiEvent::ControlChange { controller, .. } => Some(controller),
            _ => None,
        };
        // An armed learn eats the first on-channel CC (PRD 008).
        if let Some(midi) = self.midi.as_mut()
            && midi.learn.is_some()
            && midi.map.on_channel(&event)
            && let Some(controller) = controller
        {
            let (slot, param) = midi.learn.take().expect("checked above");
            let displaced = midi.map.bind_cc(controller, &slot, &param);
            midi.pickup.remove(&controller);
            let target = format!("{slot}.{param}");
            lines.push(match displaced.filter(|old| *old != target) {
                Some(old) => format!("midi: learned CC {controller} → {target} (was {old})"),
                None => format!("midi: learned CC {controller} → {target}"),
            });
            if let Some(warning) = save_midi_map(&midi.map) {
                lines.push(warning);
            }
            return;
        }
        let Some(action) = self.midi.as_ref().and_then(|m| m.map.action_for(&event)) else {
            return;
        };
        match action {
            lh_midi::Action::LoadPreset(name) => match self.load_preset(&name) {
                Ok(mut msgs) => lines.append(&mut msgs),
                Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
            },
            lh_midi::Action::LoadPresetIndex(index) => match self.preset_for_pc(index as usize) {
                // The active setlist (if any) drives the walk; otherwise the
                // sorted directory — the existing cross-binary contract (PRD 016).
                Some(name) => match self.load_preset(&name) {
                    Ok(mut msgs) => lines.append(&mut msgs),
                    Err(e) => lines.push(format!("midi: preset {name:?}: {e}")),
                },
                None => lines.push(format!("midi: no preset at PC {index}")),
            },
            lh_midi::Action::SetParam {
                slot,
                param,
                norm,
                pickup,
            } => {
                // Virtual `snapshot.<anything>` target (PRD 009): the CC
                // position picks a scene A–D. Only switch on change, or a
                // held pedal would re-trigger the morph every frame.
                if slot == "snapshot" {
                    let n = SNAPSHOT_SLOTS.len();
                    let idx = ((norm * n as f32) as usize).min(n - 1);
                    let letter = SNAPSHOT_SLOTS[idx];
                    if self.active_snapshot.as_deref() != Some(letter) {
                        match self.switch_snapshot(letter) {
                            Ok(msg) => lines.push(format!("midi: {msg}")),
                            Err(e) => lines.push(format!("midi: {e}")),
                        }
                    }
                    return;
                }
                // Virtual `tempo.tap` target (PRD 012): a press (value ≥ 64)
                // is one tap; the release half of a momentary switch is not.
                if slot == "tempo" {
                    if param == "tap" {
                        if norm >= 0.5 {
                            let line = self.tap_tempo(None);
                            if !line.is_empty() {
                                lines.push(format!("midi: {line}"));
                            }
                        }
                    } else {
                        lines.push(format!(
                            "midi: unknown tempo target tempo.{param} — use \"tempo.tap\""
                        ));
                    }
                    return;
                }
                // `slot.pedal` (and the pre-v3 aliases) selects a pedal;
                // everything else lands on the active pedal's knobs.
                if lh_engine::is_pedal_selector(&param) {
                    match self.chain.select_pedal_norm(&slot, norm) {
                        Ok(pedal) => lines.push(format!("midi: {slot}.pedal = {pedal}")),
                        Err(e) => lines.push(format!("midi: {e}")),
                    }
                    return;
                }
                // Soft-takeover: a desynced pedal stays silent until it
                // sweeps across the value it is mapped to (PRD 008).
                if pickup
                    && let Some(controller) = controller
                    && !self.pickup_engaged(controller, &slot, &param, norm)
                {
                    return;
                }
                match self.chain.param_desc(&slot, &param) {
                    Some(p) => {
                        let real = p.range.to_real(norm);
                        match self.chain.set_param(&slot, &param, real) {
                            Ok(applied) => {
                                // A momentary footswitch press (norm ≥ 0.5) on
                                // a looper advances the LED mirror (PRD 013).
                                if norm >= 0.5
                                    && matches!(param.as_str(), "rec" | "clear")
                                    && self.is_looper(&slot)
                                {
                                    self.note_looper_transport(&slot, &param);
                                }
                                lines.push(match p.range.label(applied.real) {
                                    Some(label) => format!("midi: {slot}.{param} = {label}"),
                                    None => format!(
                                        "midi: {slot}.{param} = {:.2} {}",
                                        applied.real, applied.unit
                                    ),
                                })
                            }
                            Err(e) => lines.push(format!("midi: {e}")),
                        }
                    }
                    None => lines.push(format!("midi: unknown target {slot}.{param}")),
                }
            }
            lh_midi::Action::SetActive { slot, active } => {
                if slot == "tempo" {
                    lines.push(
                        "midi: map a controller to \"tempo.tap\" (each press taps), \
                         not bare \"tempo\""
                            .into(),
                    );
                    return;
                }
                if slot == "snapshot" {
                    lines.push(
                        "midi: map a controller to \"snapshot.select\" (a value \
                         picks scene A–D), not bare \"snapshot\""
                            .into(),
                    );
                    return;
                }
                match self.chain.set_active(&slot, active) {
                    Ok(()) => lines.push(format!(
                        "midi: {slot} {}",
                        if active { "on" } else { "off" }
                    )),
                    Err(e) => lines.push(format!("midi: {e}")),
                }
            }
        }
    }

    /// Soft-takeover gate: `true` once this controller has engaged — swept
    /// across the parameter's current value (or landed within
    /// [`PICKUP_WINDOW`] of it) since the last desync.
    fn pickup_engaged(&mut self, controller: u8, slot: &str, param: &str, shaped: f32) -> bool {
        let current = self.chain.param_norm(slot, param);
        let Some(midi) = self.midi.as_mut() else {
            return true;
        };
        // Unknown target: don't gate — the apply path owns the error line.
        let Some(current) = current else {
            return true;
        };
        midi.pickup
            .entry(controller)
            .or_default()
            .feed(current, shaped)
    }

    /// Forget every controller's soft-takeover engagement (a preset load
    /// re-seats all values under the hardware).
    pub fn midi_desync_all(&mut self) {
        if let Some(midi) = self.midi.as_mut() {
            midi.pickup.clear();
        }
    }

    /// Desync the controllers riding one param (a GUI knob moved it away
    /// from under the pedal).
    pub fn midi_desync_param(&mut self, slot: &str, param: &str) {
        let target = format!("{slot}.{param}");
        if let Some(midi) = self.midi.as_mut() {
            let map = &midi.map;
            midi.pickup.retain(|cc, _| {
                map.cc
                    .get(&cc.to_string())
                    .is_none_or(|m| m.target() != target)
            });
        }
    }

    /// Desync every controller riding a slot (its pedal switched — the
    /// incoming pedal's values re-seat from its shadow memory).
    pub fn midi_desync_slot(&mut self, slot: &str) {
        let prefix = format!("{slot}.");
        if let Some(midi) = self.midi.as_mut() {
            let map = &midi.map;
            midi.pickup.retain(|cc, _| {
                map.cc
                    .get(&cc.to_string())
                    .is_none_or(|m| !m.target().starts_with(&prefix))
            });
        }
    }

    /// Arm MIDI learn: the next on-channel CC binds to `slot.param` and is
    /// persisted to `midi.json` (PRD 008).
    pub fn arm_midi_learn(&mut self, slot: &str, param: &str) -> Result<String, String> {
        if self.chain.param_desc(slot, param).is_none() {
            return Err(format!("unknown target {slot}.{param}"));
        }
        let Some(midi) = self.midi.as_mut() else {
            return Err("no MIDI input connected".into());
        };
        midi.learn = Some((slot.to_string(), param.to_string()));
        Ok(format!("midi: learning {slot}.{param} — move a controller"))
    }

    /// The armed learn target, if any.
    pub fn midi_learn_target(&self) -> Option<(&str, &str)> {
        self.midi
            .as_ref()
            .and_then(|m| m.learn.as_ref())
            .map(|(s, p)| (s.as_str(), p.as_str()))
    }

    /// Disarm learn; `true` if something was armed.
    pub fn cancel_midi_learn(&mut self) -> bool {
        self.midi.as_mut().and_then(|m| m.learn.take()).is_some()
    }

    /// The controller bound to `slot.param`, if any (knob badges).
    pub fn cc_binding(&self, slot: &str, param: &str) -> Option<u8> {
        self.midi
            .as_ref()
            .and_then(|m| m.map.cc_for_param(slot, param))
    }

    /// Remove `slot.param`'s binding and persist the map.
    pub fn clear_cc_binding(&mut self, slot: &str, param: &str) -> Result<String, String> {
        let Some(midi) = self.midi.as_mut() else {
            return Err("no MIDI input connected".into());
        };
        match midi.map.unbind_param(slot, param) {
            Some(cc) => {
                midi.pickup.remove(&cc);
                let mut msg = format!("midi: cleared CC {cc} → {slot}.{param}");
                if let Some(warning) = save_midi_map(&midi.map) {
                    msg.push_str(&format!(" ({warning})"));
                }
                Ok(msg)
            }
            None => Err(format!("no CC bound to {slot}.{param}")),
        }
    }

    // --- snapshots / scenes (PRD 009) ---
}

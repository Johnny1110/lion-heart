//! `Session` looper transport (PRD 013) and its LED mirror.

/// GUI-facing mirror of a looper slot's transport state (PRD 013). The DSP
/// effect owns the authoritative state on the audio thread; the session
/// mirrors it from the same rec/clear presses it forwards, so the GUI can
/// color the LED without a status tap out of the engine. Best-effort: a
/// pathological instant double-tap (recording < 2 samples) can drift the
/// mirror by one step, which only mistints an LED — never the audio.
#[derive(Clone, Copy, PartialEq, Eq, Default, Debug)]
pub enum LooperLed {
    #[default]
    Empty,
    Recording,
    Playing,
    Overdubbing,
}

impl super::Session {
    /// The looper transport LED for a slot handle (PRD 013) — `Empty` for a
    /// non-looper or unknown handle.
    pub fn looper_led(&self, handle: &str) -> LooperLed {
        self.looper_leds.get(handle).copied().unwrap_or_default()
    }

    /// Whether a slot handle is a looper (single-pedal family: pedal key ==
    /// family key == "looper").
    pub(super) fn is_looper(&self, handle: &str) -> bool {
        self.chain.active_pedal(handle).is_ok_and(|k| k == "looper")
    }

    /// Fire a looper transport momentary (`rec`/`undo`/`clear`) as a 1.0→0.0
    /// pulse — the rising edge triggers the effect, the falling edge re-arms
    /// it, and the shadow settles at 0 so a preset never stores a held button
    /// (PRD 013). The LED mirror advances on the press.
    pub fn looper_press(&mut self, handle: &str, action: &str) -> Result<(), String> {
        self.chain
            .set_param(handle, action, 1.0)
            .map_err(|e| e.to_string())?;
        self.chain
            .set_param(handle, action, 0.0)
            .map_err(|e| e.to_string())?;
        self.note_looper_transport(handle, action);
        Ok(())
    }

    /// Advance the LED mirror for a genuine transport press (once per GUI /
    /// REPL press; on the rising edge of a MIDI-driven one).
    pub(super) fn note_looper_transport(&mut self, handle: &str, action: &str) {
        let led = self.looper_leds.entry(handle.to_string()).or_default();
        *led = match action {
            // The one-button state machine (mirrors the effect's on_rec).
            "rec" => match *led {
                LooperLed::Empty => LooperLed::Recording,
                LooperLed::Recording => LooperLed::Playing,
                LooperLed::Playing => LooperLed::Overdubbing,
                LooperLed::Overdubbing => LooperLed::Playing,
            },
            "clear" => LooperLed::Empty,
            // undo/reverse/half don't move the play/record state.
            _ => *led,
        };
    }
}

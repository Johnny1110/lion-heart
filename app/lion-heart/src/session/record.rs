//! `Session` monitor recording (PRD 014) and the raw-signal taps that
//! feed the tuner and spectrum analyzer.

use super::*;

impl super::Session {
    /// The tuner's raw-input consumer; the GUI takes it once at startup.
    pub fn take_tuner_tap(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.tuner_tap.take()
    }

    /// The spectrum analyzer's post-output consumer (GUI, once at startup).
    pub fn take_spectrum_tap(&mut self) -> Option<rtrb::Consumer<f32>> {
        self.spectrum_tap.take()
    }

    // --- recording (PRD 014) ---

    /// Start a take: DI + wet WAVs under the recordings directory. Returns the
    /// two paths.
    pub fn start_recording(&mut self) -> Result<(PathBuf, PathBuf), String> {
        self.recorder.start()
    }

    /// Stop the current take and finalize the WAVs. Returns the summary.
    pub fn stop_recording(&mut self) -> Result<RecSummary, String> {
        self.recorder.stop()
    }

    /// Toggle recording; returns a user-facing message.
    pub fn toggle_recording(&mut self) -> String {
        if self.recorder.is_recording() {
            match self.stop_recording() {
                Ok(summary) => summary.human(),
                Err(e) => format!("error: {e}"),
            }
        } else {
            match self.start_recording() {
                Ok((di, _wet)) => format!(
                    "recording → {}",
                    di.parent()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default()
                ),
                Err(e) => format!("error: {e}"),
            }
        }
    }

    pub fn is_recording(&self) -> bool {
        self.recorder.is_recording()
    }

    /// Live take status (elapsed, dropped frames) for the UI; `None` when idle.
    pub fn recording_status(&self) -> Option<RecStatus> {
        self.recorder.status()
    }

    // --- global output EQ (PRD 003) ---
}

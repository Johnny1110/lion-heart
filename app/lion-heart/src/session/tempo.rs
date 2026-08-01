//! Global tempo state: tap tempo and MIDI clock accumulators.

use super::*;

use std::time::Instant;

/// Taps further apart than this start a fresh tempo (the pre-012 GUI
/// behavior, moved here).
pub(super) const TAP_TIMEOUT_SECS: f32 = 2.0;
/// Inter-tap intervals averaged for the reading.
pub(super) const TAP_HISTORY: usize = 4;
/// 24-ppqn tick intervals kept for the median (~2 beats at the middle).
pub(super) const CLOCK_HISTORY: usize = 48;
/// Ticks needed before the clock claims a tempo (half a beat).
pub(super) const CLOCK_MIN_TICKS: usize = 12;
/// Plausible tick intervals: 24 ppqn over ~20–300 bpm. Outside the window
/// means a stream gap or garbage — accumulation restarts.
pub(super) const CLOCK_TICK_MIN_US: u64 = 4_000;
pub(super) const CLOCK_TICK_MAX_US: u64 = 120_000;
/// Clock wobble below this (relative) does not rewrite synced times.
pub(super) const TEMPO_HYSTERESIS: f32 = 0.005;
pub const TEMPO_MIN_BPM: f32 = 30.0;
pub const TEMPO_MAX_BPM: f32 = 300.0;

/// Global tempo **source** state (PRD 012): the tap-history and MIDI-clock
/// accumulators that resolve to the rig BPM. The BPM itself lives in
/// `config.tempo_bpm` (persisted, ADR 014); this is the transient plumbing
/// that feeds it. A synced slot's `time`/`rate` param is the durable result,
/// so presets stay portable.
#[derive(Default)]
pub(super) struct TempoState {
    /// Last BPM written onto synced slots (the hysteresis anchor).
    pub(super) applied_bpm: Option<f32>,
    pub(super) tap_last: Option<Instant>,
    /// Recent inter-tap intervals (seconds), newest last.
    pub(super) tap_intervals: Vec<f32>,
    pub(super) clock_last_us: Option<u64>,
    /// Recent inter-tick intervals (seconds), newest last.
    pub(super) clock_intervals: Vec<f32>,
}

/// Mean of the recent taps → BPM; `None` until two taps land.
pub(super) fn tap_bpm(intervals: &[f32]) -> Option<f32> {
    if intervals.is_empty() {
        return None;
    }
    let period = intervals.iter().sum::<f32>() / intervals.len() as f32;
    Some((60.0 / period).clamp(TEMPO_MIN_BPM, TEMPO_MAX_BPM))
}

/// Median of the recent 24-ppqn tick intervals → BPM; `None` until
/// [`CLOCK_MIN_TICKS`] land. Median over mean: one late tick (a USB
/// scheduling hiccup) cannot bend the tempo.
pub(super) fn clock_bpm(intervals: &[f32]) -> Option<f32> {
    if intervals.len() < CLOCK_MIN_TICKS {
        return None;
    }
    let mut sorted = intervals.to_vec();
    sorted.sort_by(f32::total_cmp);
    let tick = sorted[sorted.len() / 2];
    if tick <= 0.0 {
        return None;
    }
    Some((60.0 / (24.0 * tick)).clamp(TEMPO_MIN_BPM, TEMPO_MAX_BPM))
}

impl super::Session {
    /// One tap (footer chip, faceplate TAP, REPL `tap`, MIDI `tempo.tap`).
    /// Two-plus taps in rhythm set the rig tempo. `slot` is the faceplate whose
    /// TAP was hit: a *Free* (unsynced) delay there also takes its `time` from
    /// this tap × its subdivision — the pre-sync per-slot tap workflow
    /// (PRD 004), preserved. A synced slot ignores it (its division owns time).
    pub fn tap_tempo(&mut self, slot: Option<&str>) -> String {
        let now = Instant::now();
        {
            let t = &mut self.tempo;
            if let Some(last) = t.tap_last {
                let gap = now.duration_since(last).as_secs_f32();
                if gap < TAP_TIMEOUT_SECS {
                    t.tap_intervals.push(gap);
                    if t.tap_intervals.len() > TAP_HISTORY {
                        t.tap_intervals.remove(0);
                    }
                } else {
                    t.tap_intervals.clear(); // stale — start a fresh tempo
                }
            }
            t.tap_last = Some(now);
        }
        let Some(bpm) = tap_bpm(&self.tempo.tap_intervals) else {
            return String::new(); // first tap of a run — nothing to say yet
        };
        self.set_tempo_bpm(bpm); // persist + re-derive the synced slots
        // Legacy per-slot tap: a Free delay under the tapped faceplate follows
        // the tap directly (via its own subdivision), since its Time knob still
        // rules while sync = Free.
        if let Some(slot) = slot
            && self.slot_is_free_delay(slot)
        {
            self.retime_delay(slot, 60_000.0 / self.config.tempo_bpm);
        }
        format!("tap: ♩ = {:.0} bpm", self.config.tempo_bpm)
    }

    /// A slot's `subdivision`/`sync` selector just moved — re-derive its locked
    /// control from the current tempo. Returns whether anything moved (so the
    /// GUI can refresh that faceplate). No-op for a *Free* slot.
    pub fn apply_tempo_to(&mut self, _slot: &str) -> bool {
        self.chain.apply_tempo_sync(self.config.tempo_bpm)
    }

    /// Whether `slot` is a delay currently on *Free* sync (its Time knob rules,
    /// so a per-slot tap may set its time directly).
    fn slot_is_free_delay(&self, slot: &str) -> bool {
        self.chain.param_desc(slot, "time").is_some()
            && self
                .chain
                .param_desc(slot, "sync")
                .zip(self.chain.param_norm(slot, "sync"))
                .map(|(d, n)| !lh_core::tempo::is_synced(d.range.to_real(n) as usize))
                .unwrap_or(true)
    }

    /// `time = quarter-note period × the slot's subdivision`, clamped into
    /// the voice's range — the per-slot tap path (PRD 004).
    fn retime_delay(&mut self, slot: &str, period_ms: f32) {
        let ratio = self
            .chain
            .param_desc(slot, "subdivision")
            .zip(self.chain.param_norm(slot, "subdivision"))
            .map(|(d, n)| lh_dsp::time::delay::subdivision_ratio(d.range.to_real(n) as usize))
            .unwrap_or(1.0);
        let Some(desc) = self.chain.param_desc(slot, "time") else {
            return; // not a delay — nothing to retime
        };
        let time = desc.range.clamp(period_ms * ratio);
        let _ = self.chain.set_param(slot, "time", time);
    }

    /// One 0xF8 tick: interval → ring → median BPM, applied **live** (not
    /// persisted — a MIDI clock is transient). Absurd or gapped intervals
    /// restart the accumulation; sub-[`TEMPO_HYSTERESIS`] wobble on a steady
    /// clock does not requeue chain messages (PRD 012).
    pub(super) fn on_clock_tick(&mut self, stamp_us: u64, lines: &mut Vec<String>) {
        let bpm = {
            let t = &mut self.tempo;
            if let Some(last) = t.clock_last_us {
                let dt = stamp_us.saturating_sub(last);
                if (CLOCK_TICK_MIN_US..=CLOCK_TICK_MAX_US).contains(&dt) {
                    t.clock_intervals.push(dt as f32 / 1e6);
                    if t.clock_intervals.len() > CLOCK_HISTORY {
                        t.clock_intervals.remove(0);
                    }
                } else {
                    t.clock_intervals.clear();
                }
            }
            t.clock_last_us = Some(stamp_us);
            clock_bpm(&t.clock_intervals)
        };
        let Some(bpm) = bpm else { return };
        let bpm = lh_core::tempo::clamp_bpm(bpm);
        // A steady clock's sub-0.5% wobble must not repaint the status bar or
        // requeue chain messages 120 times a second.
        if self
            .tempo
            .applied_bpm
            .is_some_and(|a| (bpm - a).abs() / a < TEMPO_HYSTERESIS)
        {
            return;
        }
        let announce = (self.config.tempo_bpm - bpm).abs() >= 1.0;
        self.apply_tempo_now(bpm);
        if announce {
            lines.push(format!("midi: clock ♩ = {bpm:.0} bpm"));
        }
    }

    /// The rig's global tempo (ADR 014).
    pub fn tempo_bpm(&self) -> f32 {
        self.config.tempo_bpm
    }

    /// Set the global tempo (clamped to the musical range), re-derive the
    /// sync-locked controls, and **persist** it (a typed/tapped tempo survives
    /// a restart). The MIDI-clock path uses [`Session::apply_tempo_now`]
    /// instead — a clock is transient, so it applies without persisting.
    pub fn set_tempo_bpm(&mut self, bpm: f32) -> String {
        self.apply_tempo_now(bpm);
        save_config(&self.config);
        format!("tempo ♩ = {:.0} bpm", self.config.tempo_bpm)
    }

    /// Set the live tempo and re-derive every sync-locked control, **without**
    /// persisting — the shared core of `set_tempo_bpm` (which persists) and the
    /// MIDI-clock path (which does not).
    fn apply_tempo_now(&mut self, bpm: f32) {
        self.config.tempo_bpm = lh_core::tempo::clamp_bpm(bpm);
        self.tempo.applied_bpm = Some(self.config.tempo_bpm);
        self.chain.apply_tempo_sync(self.config.tempo_bpm);
        // The metronome click follows the rig tempo (PRD 019).
        self.metronome.set_bpm(self.config.tempo_bpm);
    }

    // --- practice tools: metronome (PRD 019, Phase 1) ---

    /// Re-derive every tempo-locked control from the global BPM. Called on the
    /// control loop (GUI frame tick / REPL poll) after [`Session::tick_morph`].
    /// Delegates to [`lh_engine::ChainHandle::apply_tempo_sync`]; returns
    /// whether any control moved, so the GUI can refresh just the faceplate
    /// that changed.
    pub fn tick_tempo(&mut self) -> bool {
        self.chain.apply_tempo_sync(self.config.tempo_bpm)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // --- global tempo (PRD 012) ---
    // --- global tempo (PRD 012) ---

    #[test]
    fn tap_bpm_needs_two_taps_and_averages_the_rest() {
        assert_eq!(tap_bpm(&[]), None, "one tap: no interval yet");
        // Exactly 0.5 s apart = 120 bpm.
        let bpm = tap_bpm(&[0.5]).unwrap();
        assert!((bpm - 120.0).abs() < 0.1);
        // A steadier run of taps averages out a slightly early one: mean
        // period 0.4875 s ⇒ 123.08 bpm.
        let bpm = tap_bpm(&[0.5, 0.5, 0.45, 0.5]).unwrap();
        assert!((bpm - 123.08).abs() < 0.1, "got {bpm}");
    }

    #[test]
    fn tap_bpm_clamps_to_the_supported_range() {
        // A wild single tap (2 s later ⇒ 30 bpm floor is fine, but a
        // near-instant double tap must not report an absurd bpm).
        let bpm = tap_bpm(&[0.02]).unwrap(); // 3000 bpm, uncapped
        assert_eq!(bpm, TEMPO_MAX_BPM);
        let bpm = tap_bpm(&[5.0]).unwrap(); // 12 bpm, uncapped
        assert_eq!(bpm, TEMPO_MIN_BPM);
    }

    #[test]
    fn clock_bpm_needs_min_ticks_and_uses_the_median() {
        // 24 ppqn @ 120 bpm ⇒ 20.833... ms/tick.
        let tick = 60.0 / 120.0 / 24.0;
        let steady = vec![tick; CLOCK_MIN_TICKS - 1];
        assert_eq!(clock_bpm(&steady), None, "one short of the minimum");

        let mut ticks = vec![tick; CLOCK_MIN_TICKS];
        assert!(clock_bpm(&ticks).is_some());
        let bpm = clock_bpm(&ticks).unwrap();
        assert!((bpm - 120.0).abs() < 0.1, "got {bpm}");

        // One wild outlier (a scheduling hiccup) must not move the median.
        ticks.push(tick * 20.0);
        let bpm = clock_bpm(&ticks).unwrap();
        assert!((bpm - 120.0).abs() < 0.1, "outlier moved the median: {bpm}");
    }

    #[test]
    fn clock_bpm_clamps_to_the_supported_range() {
        let fast = vec![0.001f32; CLOCK_MIN_TICKS]; // absurdly fast ticks
        assert_eq!(clock_bpm(&fast), Some(TEMPO_MAX_BPM));
        let slow = vec![1.0f32; CLOCK_MIN_TICKS]; // absurdly slow ticks
        assert_eq!(clock_bpm(&slow), Some(TEMPO_MIN_BPM));
    }
}

//! Global tempo & note-division sync (PRD 011 / ADR 014).
//!
//! The rig carries one **BPM** (app-global, like `morph_ms`; see the standalone
//! session). Any effect that exposes a stepped `sync` param can lock a
//! time-based control to that tempo instead of its own knob: a delay locks its
//! **time** to a note length, an LFO effect (e.g. tremolo) locks its **rate**
//! so one cycle spans that note. The derivation is pure and control-side — the
//! DSP still just receives a `time`/`rate` value through the smoothing layer,
//! exactly as if a knob had moved (there is no audio-path tempo state).
//!
//! Divisions are expressed as multiples of a **quarter-note beat** (the tempo
//! unit): a quarter note is one beat = `60 / bpm` seconds.

/// The `sync` selector's labels, in menu order. Index 0 is **Free** (the
/// effect's own knob rules); the rest are note lengths locked to the tempo.
/// Append-only — presets and plugin ids reference these by position.
pub const SYNC_DIVISIONS: &[&str] = &[
    "Free", "1/1", "1/2", "1/4.", "1/4", "1/8.", "1/8T", "1/8", "1/16",
];

/// Length of each [`SYNC_DIVISIONS`] entry in quarter-note beats, index-aligned.
/// Index 0 (Free) is a placeholder — [`beat_ratio`] returns `None` for it.
const BEAT_RATIOS: [f32; 9] = [
    0.0,       // Free (unused)
    4.0,       // 1/1  whole
    2.0,       // 1/2  half
    1.5,       // 1/4. dotted quarter
    1.0,       // 1/4  quarter
    0.75,      // 1/8. dotted eighth
    1.0 / 3.0, // 1/8T eighth triplet
    0.5,       // 1/8  eighth
    0.25,      // 1/16 sixteenth
];

/// Tempo bounds and default (BPM). The default matches a fresh `AppConfig`.
pub const MIN_BPM: f32 = 30.0;
pub const MAX_BPM: f32 = 300.0;
pub const DEFAULT_BPM: f32 = 120.0;

/// Clamp a tempo into the musically useful range.
pub fn clamp_bpm(bpm: f32) -> f32 {
    bpm.clamp(MIN_BPM, MAX_BPM)
}

/// The note length (in quarter-note beats) of a `sync` division index, or
/// `None` for **Free** (index 0) and out-of-range indices — the caller then
/// leaves the effect's own knob in charge.
pub fn beat_ratio(div_index: usize) -> Option<f32> {
    match BEAT_RATIOS.get(div_index).copied() {
        Some(r) if r > 0.0 => Some(r),
        _ => None,
    }
}

/// Whether a division index means "locked to tempo" (anything but Free).
pub fn is_synced(div_index: usize) -> bool {
    beat_ratio(div_index).is_some()
}

/// Delay **time** in milliseconds for a division at `bpm`: the note's length.
/// `None` for Free. (Not range-clamped — the caller's `time` param clamps to
/// the voice's own ceiling.)
pub fn synced_time_ms(bpm: f32, div_index: usize) -> Option<f32> {
    let bpm = clamp_bpm(bpm);
    beat_ratio(div_index).map(|r| 60_000.0 / bpm * r)
}

/// LFO **rate** in Hz for a division at `bpm`: one cycle spans the note, so a
/// `1/4` sync pulses once per beat (`bpm/60` Hz). `None` for Free.
pub fn synced_rate_hz(bpm: f32, div_index: usize) -> Option<f32> {
    let bpm = clamp_bpm(bpm);
    beat_ratio(div_index).map(|r| bpm / 60.0 / r)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_and_ratios_stay_aligned() {
        assert_eq!(SYNC_DIVISIONS.len(), BEAT_RATIOS.len());
        assert_eq!(SYNC_DIVISIONS[0], "Free");
        assert!(beat_ratio(0).is_none()); // Free
        assert!(!is_synced(0));
        // Every non-Free entry resolves to a positive ratio.
        for (i, label) in SYNC_DIVISIONS.iter().enumerate().skip(1) {
            assert!(is_synced(i), "{label} should be synced");
        }
        assert!(beat_ratio(SYNC_DIVISIONS.len()).is_none()); // out of range
    }

    #[test]
    fn delay_times_at_120_bpm() {
        // 120 BPM: one beat (quarter) = 500 ms.
        let ms = |div| synced_time_ms(120.0, div).unwrap();
        assert!((ms(4) - 500.0).abs() < 1e-3, "1/4"); // quarter
        assert!((ms(7) - 250.0).abs() < 1e-3, "1/8"); // eighth
        assert!((ms(5) - 375.0).abs() < 1e-3, "1/8."); // dotted eighth
        assert!((ms(8) - 125.0).abs() < 1e-3, "1/16"); // sixteenth
        assert!((ms(2) - 1000.0).abs() < 1e-3, "1/2"); // half
        assert!((ms(1) - 2000.0).abs() < 1e-3, "1/1"); // whole
        // Eighth triplet: three in the space of a quarter → 500/3 ms.
        assert!((ms(6) - 500.0 / 3.0).abs() < 1e-3, "1/8T");
        assert!(synced_time_ms(120.0, 0).is_none()); // Free
    }

    #[test]
    fn lfo_rates_are_the_reciprocal_period() {
        // 120 BPM quarter = 2 Hz; eighth = 4 Hz; the LFO period equals the
        // delay time for the same division.
        assert!((synced_rate_hz(120.0, 4).unwrap() - 2.0).abs() < 1e-4);
        assert!((synced_rate_hz(120.0, 7).unwrap() - 4.0).abs() < 1e-4);
        for div in 1..SYNC_DIVISIONS.len() {
            let ms = synced_time_ms(90.0, div).unwrap();
            let hz = synced_rate_hz(90.0, div).unwrap();
            assert!((hz - 1000.0 / ms).abs() < 1e-4, "rate=1/period at {div}");
        }
    }

    #[test]
    fn tempo_is_clamped() {
        assert_eq!(clamp_bpm(1.0), MIN_BPM);
        assert_eq!(clamp_bpm(10_000.0), MAX_BPM);
        // Below-range tempo still yields a finite, clamped-tempo time.
        assert!(synced_time_ms(1.0, 4).unwrap().is_finite());
    }
}

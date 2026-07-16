//! Knob-position laws for the drive pedal family.
//!
//! The drive slot's knobs are pedal-style positions `0..=10`, like the face
//! of a real pedal. Each model maps positions onto circuit values with the
//! functions here. They live in `lh-core` — not `lh-dsp` — because the
//! preset v1→v2 migration needs the inverse mappings and `lh-core` cannot
//! depend on the DSP crate; keeping law and inverse side by side is what
//! guarantees migrated presets sound identical.

/// Level pot ceiling. `LEVEL_MAX_LIN` is its precomputed linear gain so the
/// per-sample level path never calls `powf`.
pub const LEVEL_MAX_DB: f32 = 9.0;
pub const LEVEL_MAX_LIN: f32 = 2.818_383;

/// Output level: audio-taper pot — silent at 0, unity near 6, +9 dB at 10.
#[inline]
pub fn level_lin(pos: f32) -> f32 {
    let n = (pos * 0.1).clamp(0.0, 1.0);
    n * n * LEVEL_MAX_LIN
}

/// Inverse of [`level_lin`] (migration): linear gain → knob position.
pub fn level_pos(lin: f32) -> f32 {
    ((lin / LEVEL_MAX_LIN).max(0.0)).sqrt() * 10.0
}

/// "classic" model pre-gain: 0..10 → 0..40 dB (the pre-registry drive range).
#[inline]
pub fn classic_drive_db(pos: f32) -> f32 {
    pos * 4.0
}

pub fn classic_drive_pos(db: f32) -> f32 {
    db / 4.0
}

/// "classic" model tone lowpass corner: 0..10 → 500..8000 Hz, geometric
/// (the pre-registry `Range::Log` mapping).
#[inline]
pub fn classic_tone_hz(pos: f32) -> f32 {
    500.0 * 16f32.powf(pos * 0.1)
}

pub fn classic_tone_pos(hz: f32) -> f32 {
    10.0 * (hz / 500.0).ln() / 16f32.ln()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db_to_lin;

    #[test]
    fn level_constant_matches_the_db_ceiling() {
        assert!((LEVEL_MAX_LIN - db_to_lin(LEVEL_MAX_DB)).abs() < 1e-4);
    }

    #[test]
    fn laws_round_trip_over_the_old_ranges() {
        for db in [0.0f32, 16.0, 24.0, 40.0] {
            assert!((classic_drive_db(classic_drive_pos(db)) - db).abs() < 1e-4);
        }
        for hz in [500.0f32, 3_200.0, 8_000.0] {
            assert!((classic_tone_hz(classic_tone_pos(hz)) - hz).abs() < hz * 1e-4);
        }
        for db in [-24.0f32, -6.0, 0.0, 6.0] {
            let pos = level_pos(db_to_lin(db));
            assert!(
                (0.0..=10.0).contains(&pos),
                "old level {db} dB must land on the pot: {pos}"
            );
            assert!((level_lin(pos) - db_to_lin(db)).abs() < 1e-3);
        }
    }

    #[test]
    fn level_pot_endpoints() {
        assert_eq!(level_lin(0.0), 0.0, "pot at zero is silence");
        assert!((level_lin(10.0) - LEVEL_MAX_LIN).abs() < 1e-6);
        // Unity sits just below 6 — where a real pedal's unity tends to live.
        assert!((level_lin(5.956) - 1.0).abs() < 1e-2);
    }
}

//! Snapshot chip state and scene matching.

use super::*;

use lh_core::preset::{SNAPSHOT_SLOTS, Snapshot};

/// One snapshot chip's state for the GUI (PRD 009).
pub struct SnapshotChip {
    pub letter: &'static str,
    /// A scene is stored in this slot.
    pub populated: bool,
    /// The active scene.
    pub active: bool,
    /// Active and the live values have drifted from what is stored.
    pub dirty: bool,
}

/// Normalize a snapshot selector to a canonical letter, or an error.
pub(crate) fn normalize_snapshot_letter(letter: &str) -> Result<String, String> {
    let up = letter.trim().to_uppercase();
    if SNAPSHOT_SLOTS.contains(&up.as_str()) {
        Ok(up)
    } else {
        Err(format!(
            "snapshot must be one of {}",
            SNAPSHOT_SLOTS.join("/")
        ))
    }
}

/// Whether a stored scene matches the live one within value tolerance
/// (same active flags and real values on every slot the scene names).
pub(crate) fn scenes_match(stored: &Snapshot, live: &Snapshot) -> bool {
    stored.slots.iter().all(|(handle, s)| {
        live.slots.get(handle).is_some_and(|l| {
            s.active == l.active
                && s.values.iter().all(|(param, v)| {
                    l.values
                        .get(param)
                        .is_some_and(|lv| (lv - v).abs() <= v.abs().max(1.0) * 1e-3)
                })
        })
    })
}

impl super::Session {
    /// Store the current live scene (per-slot active + selected pedal's
    /// values) into slot `letter` (A–D). Becomes the active scene.
    pub fn store_snapshot(&mut self, letter: &str) -> Result<String, String> {
        let letter = normalize_snapshot_letter(letter)?;
        let scene = self.chain.capture_scene();
        self.snapshots.insert(letter.clone(), scene);
        self.active_snapshot = Some(letter.clone());
        self.morph = None;
        Ok(format!("snapshot {letter} stored"))
    }

    /// Switch to scene `letter`, morphing over the app's `morph_ms`.
    pub fn switch_snapshot(&mut self, letter: &str) -> Result<String, String> {
        let letter = normalize_snapshot_letter(letter)?;
        if !self.snapshots.contains_key(&letter) {
            return Err(format!("snapshot {letter} is empty — store it first"));
        }
        let secs = self.config.morph_ms as f32 / 1000.0;
        self.apply_snapshot(&letter, secs);
        Ok(if secs > 0.0 {
            format!("snapshot {letter} (morph {} ms)", self.config.morph_ms)
        } else {
            format!("snapshot {letter}")
        })
    }

    /// Apply scene `letter` over `morph_secs` (0 = instant). Flips bypass now
    /// (the engine crossfades it) and either sets every value immediately or
    /// starts a morph the control loop advances. A no-op if the letter is
    /// empty; handles/params the board no longer has are skipped.
    pub(super) fn apply_snapshot(&mut self, letter: &str, morph_secs: f32) {
        let Some(target) = self.snapshots.get(letter).cloned() else {
            return;
        };
        let mut steps = Vec::new();
        for (handle, slot) in &target.slots {
            let _ = self.chain.set_active(handle, slot.active);
            for (param, real) in &slot.values {
                let Some(desc) = self.chain.param_desc(handle, param) else {
                    continue; // unknown handle/param: forward-compat skip
                };
                let to = desc.range.to_norm(desc.range.clamp(*real));
                let from = self.chain.param_norm(handle, param).unwrap_or(to);
                steps.push(MorphStep {
                    handle: handle.clone(),
                    param: param.clone(),
                    from,
                    to,
                });
            }
        }
        if morph_secs > 0.0 {
            let morph = Morph::build(Instant::now(), morph_secs, steps);
            // t=0 is the current state; let the loop advance from here.
            self.morph = (!morph.is_empty()).then_some(morph);
        } else {
            for step in &steps {
                if let Some(desc) = self.chain.param_desc(&step.handle, &step.param) {
                    let real = desc.range.to_real(step.to);
                    let _ = self.chain.set_param(&step.handle, &step.param, real);
                }
            }
            self.morph = None;
        }
        self.active_snapshot = Some(letter.to_string());
        // Scene values moved out from under the pedals: pickup re-engages.
        self.midi_desync_all();
    }

    /// Advance an in-flight morph to `now`; clears it when complete. Called
    /// on the control loop (GUI frame tick / REPL poll). Cheap and idle when
    /// no morph is running.
    pub fn tick_morph(&mut self, now: Instant) {
        let (updates, done) = {
            let Some(morph) = &self.morph else {
                return;
            };
            let t = if morph.dur_secs <= 0.0 {
                1.0
            } else {
                (now.duration_since(morph.started).as_secs_f32() / morph.dur_secs).clamp(0.0, 1.0)
            };
            let updates: Vec<(String, String, f32)> = morph
                .at(t)
                .into_iter()
                .map(|(h, p, n)| (h.to_string(), p.to_string(), n))
                .collect();
            (updates, t >= 1.0)
        };
        for (handle, param, norm) in updates {
            if let Some(desc) = self.chain.param_desc(&handle, &param) {
                let real = desc.range.to_real(norm);
                let _ = self.chain.set_param(&handle, &param, real);
            }
        }
        if done {
            self.morph = None;
        }
    }

    /// Whether a morph is currently animating (the GUI keeps redrawing knobs
    /// while it is).
    pub fn is_morphing(&self) -> bool {
        self.morph.is_some()
    }

    pub fn morph_ms(&self) -> u32 {
        self.config.morph_ms
    }

    /// Set the morph time (clamped 0–2000 ms) and persist it.
    pub fn set_morph_ms(&mut self, ms: u32) -> String {
        self.config.morph_ms = ms.min(2_000);
        save_config(&self.config);
        format!("morph time {} ms", self.config.morph_ms)
    }

    /// Per-letter chip state for the GUI (PRD 009): populated, active, and
    /// (for the active one) whether the live scene has drifted from stored.
    pub fn snapshot_chips(&self) -> Vec<SnapshotChip> {
        let live = self.chain.capture_scene();
        SNAPSHOT_SLOTS
            .iter()
            .map(|&letter| {
                let stored = self.snapshots.get(letter);
                let active = self.active_snapshot.as_deref() == Some(letter);
                SnapshotChip {
                    letter,
                    populated: stored.is_some(),
                    active,
                    dirty: active && stored.is_some_and(|s| !scenes_match(s, &live)),
                }
            })
            .collect()
    }
}

/// An in-progress snapshot morph (PRD 009): the value trajectory from the
/// pre-switch scene to the target, plus its wall-clock window. The
/// interpolation math is pure and unit-tested; the session feeds it a
/// progress fraction each control-loop tick and pushes the resulting norms.
pub(super) struct Morph {
    steps: Vec<MorphStep>,
    started: Instant,
    dur_secs: f32,
}

pub(super) struct MorphStep {
    handle: String,
    param: String,
    /// Normalized endpoints — log-ranged params morph musically in norm.
    from: f32,
    to: f32,
}

/// A param whose norm moves by less than this over a morph is dropped (a
/// switch only touches what actually differs).
const MORPH_EPS: f32 = 1e-4;

impl Morph {
    /// Keep only the steps that actually move.
    fn build(started: Instant, dur_secs: f32, steps: Vec<MorphStep>) -> Self {
        let steps = steps
            .into_iter()
            .filter(|s| (s.to - s.from).abs() > MORPH_EPS)
            .collect();
        Self {
            steps,
            started,
            dur_secs,
        }
    }

    /// The (handle, param, norm) each step should hold at progress `t`.
    fn at(&self, t: f32) -> Vec<(&str, &str, f32)> {
        let t = t.clamp(0.0, 1.0);
        self.steps
            .iter()
            .map(|s| {
                (
                    s.handle.as_str(),
                    s.param.as_str(),
                    s.from + (s.to - s.from) * t,
                )
            })
            .collect()
    }

    fn is_empty(&self) -> bool {
        self.steps.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn morph_step(handle: &str, from: f32, to: f32) -> MorphStep {
        MorphStep {
            handle: handle.into(),
            param: "x".into(),
            from,
            to,
        }
    }

    /// Morph (PRD 009): unchanged params drop out; the rest interpolate
    /// monotonically from the current value (t=0) to the target (t=1).
    #[test]
    fn morph_drops_noops_and_interpolates_endpoints() {
        let now = Instant::now();
        let m = Morph::build(
            now,
            1.0,
            vec![
                morph_step("drive", 0.2, 0.8),  // moves
                morph_step("comp", 0.5, 0.5),   // no-op, dropped
                morph_step("reverb", 0.9, 0.1), // moves down
            ],
        );
        assert_eq!(m.steps.len(), 2, "the no-op step is dropped");

        // t=0 is the starting values, t=1 the targets.
        let at0 = m.at(0.0);
        assert!((at0[0].2 - 0.2).abs() < 1e-6 && (at0[1].2 - 0.9).abs() < 1e-6);
        let at1 = m.at(1.0);
        assert!((at1[0].2 - 0.8).abs() < 1e-6 && (at1[1].2 - 0.1).abs() < 1e-6);

        // The midpoint sits strictly between, and motion is monotone.
        let mid = m.at(0.5);
        assert!(
            (mid[0].2 - 0.5).abs() < 1e-6,
            "up step halfway: {}",
            mid[0].2
        );
        assert!(
            (mid[1].2 - 0.5).abs() < 1e-6,
            "down step halfway: {}",
            mid[1].2
        );
        let (mut prev_up, mut prev_dn) = (at0[0].2, at0[1].2);
        for i in 1..=10 {
            let v = m.at(i as f32 / 10.0);
            assert!(v[0].2 >= prev_up - 1e-6, "up must not backtrack");
            assert!(v[1].2 <= prev_dn + 1e-6, "down must not backtrack");
            prev_up = v[0].2;
            prev_dn = v[1].2;
        }

        // t clamps: past the end stays at the target.
        assert!((m.at(1.5)[0].2 - 0.8).abs() < 1e-6);
    }

    use std::collections::BTreeMap;

    #[test]
    fn snapshot_letters_are_validated() {
        assert_eq!(normalize_snapshot_letter("a").unwrap(), "A");
        assert_eq!(normalize_snapshot_letter(" c ").unwrap(), "C");
        assert!(normalize_snapshot_letter("E").is_err());
        assert!(normalize_snapshot_letter("").is_err());
    }

    #[test]
    fn scenes_match_within_tolerance() {
        use lh_core::preset::{Snapshot, SnapshotSlot};
        let scene = |gain: f32, active: bool| Snapshot {
            slots: BTreeMap::from([(
                "drive".to_string(),
                SnapshotSlot {
                    active,
                    values: BTreeMap::from([("gain".to_string(), gain)]),
                },
            )]),
        };
        assert!(scenes_match(&scene(5.0, true), &scene(5.0, true)));
        assert!(
            scenes_match(&scene(5.0, true), &scene(5.0005, true)),
            "tiny drift ok"
        );
        assert!(
            !scenes_match(&scene(5.0, true), &scene(6.0, true)),
            "value drift"
        );
        assert!(
            !scenes_match(&scene(5.0, true), &scene(5.0, false)),
            "bypass drift"
        );
    }
}

//! Family registry: the full rig's buildable chain families.

use lh_dsp::Effect;
use lh_dsp::blocks::swap::AssetHandle;
use lh_dsp::cab::IrAsset;
use lh_nam::NamAsset;

/// What kind of asset a family mounts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssetKind {
    Nam,
    Ir,
    IrB,
    /// A practice backing track (PRD 019 Phase 3) — not a chain asset; the
    /// browser routing loads it into the song player.
    Song,
}

/// One buildable chain family: its descriptor (the key, display name, and
/// pedal faceplates all come from here), the asset it mounts — mounting
/// families stay chain singletons — and its constructor. `build` may rewire
/// the session's asset seams and flags which it replaced (amp, cab), so the
/// caller re-applies the loaded asset afterwards.
pub struct FamilyEntry {
    pub desc: &'static lh_core::FamilyDesc,
    pub asset: Option<AssetKind>,
    #[allow(clippy::type_complexity)]
    pub(super) build: fn(
        &mut AssetHandle<NamAsset>,
        &mut AssetHandle<IrAsset>,
        &mut (bool, bool),
    ) -> Box<dyn Effect>,
}

/// Every chain family the session can build, in add-menu order — the one
/// place that knows the full rig. [`lh_core::DEFAULT_CHAIN`] (the board that
/// ships) is an in-order **subsequence** of this: the registry may carry
/// extra opt-in families that ship *off* the board and are added from the ＋
/// menu — `pitch` (ADR 016), the standalone-only `looper` (PRD 013, also
/// absent from the host-driven plugin), and the `acoustic` simulator. A test
/// pins the subsequence relation
/// and the invariants; the plugin's fixed chain is pinned to `DEFAULT_CHAIN`
/// directly.
pub static FAMILY_REGISTRY: [FamilyEntry; 15] = [
    FamilyEntry {
        desc: &lh_dsp::dynamics::gate::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::dynamics::NoiseGate::new()),
    },
    // `pitch` is opt-in: registered (so the ＋ menu and REPL can add it) but
    // absent from DEFAULT_CHAIN, so it does not eat a default-board slot.
    FamilyEntry {
        desc: &lh_dsp::pitch::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::pitch::Pitch::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::filter::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::filter::Filter::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::dynamics::comp::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::dynamics::Compressor::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::drive::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::drive::Drive::new()),
    },
    FamilyEntry {
        desc: &lh_nam::FAMILY,
        asset: Some(AssetKind::Nam),
        build: |nam, _, rebuilt| {
            let (amp, handle) = lh_nam::NamAmp::new();
            *nam = handle;
            rebuilt.0 = true;
            Box::new(amp)
        },
    },
    // Hand-written valve power stage (PRD 017): after the amp (NAM preamp),
    // before the cab. Ships bypassed on the default board (see `default_active`).
    FamilyEntry {
        desc: &lh_dsp::power::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::power::PowerAmp::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::eq::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::eq::Eq::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::modulation::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::modulation::Modulation::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::time::delay::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::time::Delay::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::time::reverb::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::time::Reverb::new()),
    },
    FamilyEntry {
        desc: &lh_dsp::cab::FAMILY,
        asset: Some(AssetKind::Ir),
        build: |_, cab, rebuilt| {
            let (cab_fx, handle) = lh_dsp::cab::CabIr::new();
            *cab = handle;
            rebuilt.1 = true;
            Box::new(cab_fx)
        },
    },
    FamilyEntry {
        desc: &lh_dsp::dynamics::limiter::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::dynamics::Limiter::new()),
    },
    // --- add-only families (past DEFAULT_CHAIN) ---
    FamilyEntry {
        desc: &lh_dsp::looper::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::looper::Looper::new()),
    },
    // `acoustic` is opt-in like `pitch`: the acoustic simulator colors wherever
    // it sits (no transparent position), so it ships off the default board and
    // is added from the ＋ menu — but active when added (you want the sound).
    FamilyEntry {
        desc: &lh_dsp::acoustic::FAMILY,
        asset: None,
        build: |_, _, _| Box::new(lh_dsp::acoustic::Acoustic::new()),
    },
];

/// The registry entry for a family key, `None` when unknown.
pub fn family_entry(key: &str) -> Option<&'static FamilyEntry> {
    FAMILY_REGISTRY.iter().find(|e| e.desc.key == key)
}

/// The asset a family mounts, if any. Instance handles equal family keys
/// for the mounting families (they are singletons), so slot handles work.
pub fn asset_kind(family_key: &str) -> Option<AssetKind> {
    family_entry(family_key).and_then(|e| e.asset)
}

/// Build a fresh effect for a family key (PRD 002's factory seam — the
/// registry owns the concrete effect crates). `pub(crate)` so the offline
/// re-amp path (PRD 014) can reconstruct a preset's chain headlessly, exactly
/// as the live session does.
pub(crate) fn build_family_effect(
    nam: &mut AssetHandle<NamAsset>,
    cab: &mut AssetHandle<IrAsset>,
    rebuilt: &mut (bool, bool),
    key: &str,
) -> Option<Box<dyn Effect>> {
    family_entry(key).map(|entry| (entry.build)(nam, cab, rebuilt))
}

//! The "Backline" design system: palette, per-pedal identity colors, and
//! widget styles.
//!
//! Look: a late-night rehearsal space — warm charcoal (never blue-gray),
//! one tube-amber accent for focus/selection, and a signature color per
//! pedal (like a real board: TS9 green, BD-2 blue, Centaur gold…) that
//! runs through its chain card, faceplate rule, and knob arcs. Hardware
//! mood, software precision — no skeuomorphic cosplay.
//!
//! Conventions: `panel` frames a view, `inset` is a recessed readout (LCD
//! feel, monospace content), `chip` is a selectable tab/toggle, `action`
//! a quiet ghost button, `primary` the one filled amber button per view,
//! `danger` a ghost that only turns hot on hover.

use iced::overlay::menu;
use iced::widget::{button, container, pick_list, text_input};
use iced::{Background, Border, Color, Shadow, Theme, Vector};

// --- base palette (warm charcoal) ------------------------------------------

pub const BG: Color = Color::from_rgb(0.070, 0.062, 0.058);
pub const PANEL: Color = Color::from_rgb(0.106, 0.096, 0.089);
pub const PANEL_HI: Color = Color::from_rgb(0.150, 0.136, 0.125);
/// Recessed readout wells (asset "LCD", knob value pills, meter troughs).
pub const INSET: Color = Color::from_rgb(0.048, 0.043, 0.040);
pub const TRACK: Color = Color::from_rgb(0.245, 0.220, 0.198);
pub const ACCENT: Color = Color::from_rgb(1.0, 0.62, 0.25);
pub const TEXT_BRIGHT: Color = Color::from_rgb(0.95, 0.92, 0.87);
pub const TEXT: Color = Color::from_rgb(0.78, 0.74, 0.68);
pub const TEXT_DIM: Color = Color::from_rgb(0.54, 0.50, 0.45);
pub const METER_OK: Color = Color::from_rgb(0.38, 0.77, 0.43);
pub const METER_HOT: Color = Color::from_rgb(0.94, 0.33, 0.28);

/// `color` at `alpha` — the glow/tint workhorse.
pub const fn dim(color: Color, alpha: f32) -> Color {
    Color { a: alpha, ..color }
}

// --- pedal identity colors ---------------------------------------------------

const GATE: Color = Color::from_rgb(0.60, 0.63, 0.66);
/// Acid lime — the funk box (PRD 007; autowah wears it as the family's
/// founding pedal).
const FILTER: Color = Color::from_rgb(0.72, 0.83, 0.32);
/// Chrome — the Crybaby treadle (PRD 008): the manual wah reads as
/// hardware, not another green box.
const WAH: Color = Color::from_rgb(0.76, 0.79, 0.84);
const COMP: Color = Color::from_rgb(0.44, 0.64, 0.94);
const AMP: Color = Color::from_rgb(1.0, 0.70, 0.36);
const EQ: Color = Color::from_rgb(0.36, 0.77, 0.72);
const MODULATION: Color = Color::from_rgb(0.67, 0.53, 0.93);
const DELAY: Color = Color::from_rgb(0.35, 0.69, 0.90);
const REVERB: Color = Color::from_rgb(0.58, 0.63, 0.93);
const CAB: Color = Color::from_rgb(0.79, 0.57, 0.37);
const LIMITER: Color = Color::from_rgb(0.94, 0.46, 0.31);
/// Orchid magenta — the pitch family (ADR 016): an otherworldly octave color,
/// distinct from the modulation/reverb violets.
const PITCH: Color = Color::from_rgb(0.85, 0.42, 0.78);

const TS9: Color = Color::from_rgb(0.35, 0.77, 0.45);
const BD2: Color = Color::from_rgb(0.42, 0.58, 0.94);
const CENTAUR: Color = Color::from_rgb(0.86, 0.72, 0.38);
const EVVA: Color = Color::from_rgb(0.93, 0.42, 0.58);
const RED_CHARLIE: Color = Color::from_rgb(0.89, 0.32, 0.28);
const MONSTER5150: Color = Color::from_rgb(0.88, 0.86, 0.82);
/// Hot scarlet-orange: the JHS enclosure (and its red-LED clippers),
/// deliberately hotter than the red-charlie's crimson.
const ANGRY_CHARLIE: Color = Color::from_rgb(0.97, 0.44, 0.20);
/// Warm bronze — the Vemuram's raw brass hardware, and a nod to the vintage
/// Fender chime it chases. Darker and more coppery than the centaur's gold so
/// the two transparent boosts don't read as the same box.
const JAN_RAY: Color = Color::from_rgb(0.82, 0.60, 0.30);
/// Dallas Arbiter turquoise — the round enclosure everyone pictures. Cyan
/// enough that no other drive box comes near it.
const FUZZ_FACE: Color = Color::from_rgb(0.15, 0.65, 0.70);

// Delay voices: digital reads cold and clean, tape warm sepia, vintage a
// dusty analog teal.
const DIGITAL: Color = Color::from_rgb(0.36, 0.72, 0.90);
const TAPE: Color = Color::from_rgb(0.85, 0.66, 0.42);
const VINTAGE: Color = Color::from_rgb(0.46, 0.68, 0.70);

// Mod pedals (PRD 006): the box colors everyone knows where one exists —
// Phase 90 orange, CE-x blue-gray, blonde-amp tremolo — and era-true guesses
// for the rest (Leslie walnut, brownface brown, psychedelic vibe green).
const MOD_CHORUS: Color = Color::from_rgb(0.62, 0.74, 0.88);
const MOD_FLANGER: Color = Color::from_rgb(0.72, 0.62, 0.88);
const MOD_PHASER: Color = Color::from_rgb(0.95, 0.52, 0.18);
const MOD_TREMOLO: Color = Color::from_rgb(0.90, 0.83, 0.62);
const MOD_VIBRATO: Color = Color::from_rgb(0.92, 0.55, 0.75);
const MOD_HARMONIC: Color = Color::from_rgb(0.72, 0.52, 0.38);
const MOD_ROTARY: Color = Color::from_rgb(0.62, 0.30, 0.34);
const MOD_UNIVIBE: Color = Color::from_rgb(0.34, 0.80, 0.62);

// Reverb voices (PRD 005): a mostly cool, airy family — hall wears the big
// sky blue — with deliberate outliers for the machines that are really
// something else in disguise (magneto's tape copper, chorale's choir gold,
// nonlinear's alarm orange).
const HALL: Color = Color::from_rgb(0.45, 0.71, 0.96);
const ROOM: Color = Color::from_rgb(0.76, 0.68, 0.55);
const PLATE: Color = Color::from_rgb(0.72, 0.76, 0.80);
const SPRING: Color = Color::from_rgb(0.42, 0.82, 0.66);
const SWELL: Color = Color::from_rgb(0.62, 0.50, 0.90);
const BLOOM: Color = Color::from_rgb(0.88, 0.48, 0.78);
const CLOUD: Color = Color::from_rgb(0.78, 0.86, 0.94);
const CHORALE: Color = Color::from_rgb(0.90, 0.78, 0.42);
const SHIMMER: Color = Color::from_rgb(0.55, 0.90, 0.82);
const MAGNETO: Color = Color::from_rgb(0.80, 0.50, 0.32);
const NONLINEAR: Color = Color::from_rgb(0.95, 0.58, 0.25);
const REFLECTIONS: Color = Color::from_rgb(0.45, 0.60, 0.62);

/// A family's signature color (chain card band, fallbacks).
pub fn family_color(key: &str) -> Color {
    match key {
        "gate" => GATE,
        "filter" => FILTER,
        "comp" => COMP,
        "amp" => AMP,
        "eq" => EQ,
        "mod" => MODULATION,
        "delay" => DELAY,
        "reverb" => REVERB,
        "cab" => CAB,
        "limiter" => LIMITER,
        "pitch" => PITCH,
        _ => ACCENT, // drive family: the pedal picks the color
    }
}

/// The signature color of one pedal — drive and delay pedals wear their own
/// liveries; everything else inherits the family color.
pub fn pedal_color(family_key: &str, pedal_key: &str) -> Color {
    match family_key {
        "drive" => match pedal_key {
            "ts9" => TS9,
            "bd2" => BD2,
            "centaur" => CENTAUR,
            "evva" => EVVA,
            "red-charlie" => RED_CHARLIE,
            "monster5150" => MONSTER5150,
            "angry-charlie" => ANGRY_CHARLIE,
            "jan-ray" => JAN_RAY,
            "fuzz-face" => FUZZ_FACE,
            _ => ACCENT, // classic wears the house amber
        },
        "filter" => match pedal_key {
            "wah" => WAH,
            _ => FILTER, // autowah wears the family lime
        },
        "mod" => match pedal_key {
            "chorus" => MOD_CHORUS,
            "flanger" => MOD_FLANGER,
            "phaser" => MOD_PHASER,
            "tremolo" => MOD_TREMOLO,
            "vibrato" => MOD_VIBRATO,
            "harmonic" => MOD_HARMONIC,
            "rotary" => MOD_ROTARY,
            "univibe" => MOD_UNIVIBE,
            _ => MODULATION,
        },
        "delay" => match pedal_key {
            "digital" => DIGITAL,
            "tape" => TAPE,
            "vintage" => VINTAGE,
            _ => DELAY,
        },
        "reverb" => match pedal_key {
            "hall" => HALL,
            "room" => ROOM,
            "plate" => PLATE,
            "spring" => SPRING,
            "swell" => SWELL,
            "bloom" => BLOOM,
            "cloud" => CLOUD,
            "chorale" => CHORALE,
            "shimmer" => SHIMMER,
            "magneto" => MAGNETO,
            "nonlinear" => NONLINEAR,
            "reflections" => REFLECTIONS,
            _ => REVERB,
        },
        _ => family_color(family_key),
    }
}

// --- shared shapes -----------------------------------------------------------

fn rounded(color: Color, width: f32) -> Border {
    Border {
        color,
        width,
        radius: 8.0.into(),
    }
}

fn soft_shadow() -> Shadow {
    Shadow {
        color: Color::from_rgba(0.0, 0.0, 0.0, 0.35),
        offset: Vector::new(0.0, 2.0),
        blur_radius: 10.0,
    }
}

// --- containers ---------------------------------------------------------------

pub fn root(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG)),
        text_color: Some(TEXT),
        ..container::Style::default()
    }
}

/// A view panel: the main content frame.
pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: rounded(Color::TRANSPARENT, 0.0),
        shadow: soft_shadow(),
        ..container::Style::default()
    }
}

/// A recessed readout well — the "LCD" for asset names, running config.
pub fn inset(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(INSET)),
        border: Border {
            color: dim(TRACK, 0.6),
            width: 1.0,
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

/// The grouped pill behind the header's view tabs.
pub fn tab_group(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: Border {
            color: dim(TRACK, 0.5),
            width: 1.0,
            radius: 9.0.into(),
        },
        ..container::Style::default()
    }
}

/// A pedal's identity rule under the faceplate header (2 px line).
pub fn identity_rule(color: Color) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(dim(color, 0.85))),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 1.0.into(),
        },
        ..container::Style::default()
    }
}

/// A chain-strip pedal card. `accent` is the pedal's identity color:
/// selected cards frame in it, the drop target glows in it, bypassed
/// cards mute toward the background.
pub fn drag_card(
    accent: Color,
    selected: bool,
    active: bool,
    dragging: bool,
    target: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let (border_color, border_width) = if target {
            (accent, 2.0)
        } else if selected {
            (dim(accent, 0.9), 1.5)
        } else {
            (dim(TRACK, 0.7), 1.0)
        };
        container::Style {
            background: Some(Background::Color(if active { PANEL_HI } else { PANEL })),
            text_color: Some(if dragging || !active {
                TEXT_DIM
            } else {
                TEXT_BRIGHT
            }),
            border: Border {
                color: border_color,
                width: border_width,
                radius: 8.0.into(),
            },
            shadow: if selected || target {
                Shadow {
                    color: dim(accent, 0.25),
                    offset: Vector::new(0.0, 0.0),
                    blur_radius: 12.0,
                }
            } else {
                Shadow::default()
            },
            ..container::Style::default()
        }
    }
}

/// A preset management row: a recessed well (like [`inset`]) that frames in
/// the accent when it is a drag's drop target, and mutes while it is the row
/// being dragged.
pub fn preset_row(target: bool, dragged: bool) -> impl Fn(&Theme) -> container::Style {
    move |_| container::Style {
        background: Some(Background::Color(if target { PANEL_HI } else { INSET })),
        text_color: dragged.then_some(TEXT_DIM),
        border: Border {
            color: if target { ACCENT } else { dim(TRACK, 0.6) },
            width: if target { 2.0 } else { 1.0 },
            radius: 6.0.into(),
        },
        ..container::Style::default()
    }
}

// --- buttons -------------------------------------------------------------------

fn lift(bg: Color, hovered: bool) -> Color {
    if hovered {
        Color {
            r: (bg.r + 0.035).min(1.0),
            g: (bg.g + 0.035).min(1.0),
            b: (bg.b + 0.035).min(1.0),
            a: bg.a,
        }
    } else {
        bg
    }
}

/// A view tab / toggle chip; `lit` renders it engaged (amber on raised).
pub fn chip(lit: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let (bg, fg, border) = if lit {
            (PANEL_HI, ACCENT, dim(ACCENT, 0.55))
        } else if hovered {
            (dim(PANEL_HI, 0.75), TEXT, Color::TRANSPARENT)
        } else {
            (Color::TRANSPARENT, TEXT_DIM, Color::TRANSPARENT)
        };
        button::Style {
            background: Some(Background::Color(bg)),
            text_color: fg,
            border: Border {
                color: border,
                width: 1.0,
                radius: 7.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// A quiet ghost button (load, move, prev/next…).
pub fn action(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(Background::Color(if hovered {
            PANEL_HI
        } else {
            Color::TRANSPARENT
        })),
        text_color: if disabled { dim(TEXT_DIM, 0.5) } else { TEXT },
        border: Border {
            color: if disabled { dim(TRACK, 0.4) } else { TRACK },
            width: 1.0,
            radius: 7.0.into(),
        },
        ..button::Style::default()
    }
}

/// The one filled button per view (save / apply).
pub fn primary(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    let disabled = matches!(status, button::Status::Disabled);
    let bg = if disabled { dim(ACCENT, 0.25) } else { ACCENT };
    button::Style {
        background: Some(Background::Color(lift(bg, hovered))),
        text_color: Color::from_rgb(0.12, 0.07, 0.02),
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 7.0.into(),
        },
        ..button::Style::default()
    }
}

/// A ghost that only turns hot when you commit to it (remove, unload).
pub fn danger(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    let disabled = matches!(status, button::Status::Disabled);
    button::Style {
        background: Some(Background::Color(if hovered {
            dim(METER_HOT, 0.12)
        } else {
            Color::TRANSPARENT
        })),
        text_color: if disabled {
            dim(TEXT_DIM, 0.5)
        } else if hovered {
            METER_HOT
        } else {
            TEXT_DIM
        },
        border: Border {
            color: if hovered { dim(METER_HOT, 0.6) } else { TRACK },
            width: 1.0,
            radius: 7.0.into(),
        },
        ..button::Style::default()
    }
}

/// The pedal power toggle: a recessed switch whose text reads like an LED.
pub fn power(on: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let fg = if on { METER_OK } else { TEXT_DIM };
        button::Style {
            background: Some(Background::Color(lift(INSET, hovered))),
            text_color: fg,
            border: Border {
                color: if on { dim(METER_OK, 0.45) } else { TRACK },
                width: 1.0,
                radius: 7.0.into(),
            },
            ..button::Style::default()
        }
    }
}

/// A file-browser / list row: flat, hover-lit, no chrome.
pub fn list_row(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    button::Style {
        background: Some(Background::Color(if hovered {
            PANEL_HI
        } else {
            Color::TRANSPARENT
        })),
        text_color: TEXT,
        border: Border {
            color: Color::TRANSPARENT,
            width: 0.0,
            radius: 6.0.into(),
        },
        ..button::Style::default()
    }
}

// --- form controls ---------------------------------------------------------------

pub fn pick(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let open = !matches!(status, pick_list::Status::Active);
    pick_list::Style {
        text_color: TEXT_BRIGHT,
        placeholder_color: TEXT_DIM,
        handle_color: if open { ACCENT } else { TEXT_DIM },
        background: Background::Color(PANEL_HI),
        border: rounded(if open { dim(ACCENT, 0.7) } else { TRACK }, 1.0),
    }
}

pub fn menu(_: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(PANEL_HI),
        border: rounded(TRACK, 1.0),
        text_color: TEXT,
        selected_text_color: ACCENT,
        selected_background: Background::Color(PANEL),
        shadow: soft_shadow(),
    }
}

pub fn input(_: &Theme, status: text_input::Status) -> text_input::Style {
    let focused = matches!(status, text_input::Status::Focused { .. });
    text_input::Style {
        background: Background::Color(INSET),
        border: rounded(if focused { dim(ACCENT, 0.7) } else { TRACK }, 1.0),
        icon: TEXT_DIM,
        placeholder: TEXT_DIM,
        value: TEXT_BRIGHT,
        selection: dim(ACCENT, 0.4),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every selectable pedal in a multi-pedal family must wear its own
    /// livery. An unregistered pedal falls back to the family color (drive:
    /// the house amber, worn by "classic") — so a forgotten entry shows up
    /// here as a pairwise clash instead of silently shipping two identical
    /// cards.
    #[test]
    fn every_selectable_pedal_wears_its_own_livery() {
        let families: [&lh_core::FamilyDesc; 6] = [
            &lh_dsp::drive::FAMILY,
            &lh_dsp::filter::FAMILY,
            &lh_dsp::modulation::FAMILY,
            &lh_dsp::pitch::FAMILY,
            &lh_dsp::time::delay::FAMILY,
            &lh_dsp::time::reverb::FAMILY,
        ];
        for family in families {
            let mut seen: Vec<(&str, [u8; 3])> = Vec::new();
            for pedal in family.pedals {
                let c = pedal_color(family.key, pedal.key);
                let rgb = [
                    (c.r * 255.0).round() as u8,
                    (c.g * 255.0).round() as u8,
                    (c.b * 255.0).round() as u8,
                ];
                for (other, other_rgb) in &seen {
                    assert_ne!(
                        rgb, *other_rgb,
                        "{}: pedals {:?} and {:?} share a color — register a \
                         livery in pedal_color",
                        family.key, pedal.key, other
                    );
                }
                seen.push((pedal.key, rgb));
            }
        }
    }
}

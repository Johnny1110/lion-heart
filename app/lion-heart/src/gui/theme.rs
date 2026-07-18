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
const COMP: Color = Color::from_rgb(0.44, 0.64, 0.94);
const AMP: Color = Color::from_rgb(1.0, 0.70, 0.36);
const EQ: Color = Color::from_rgb(0.36, 0.77, 0.72);
const MODULATION: Color = Color::from_rgb(0.67, 0.53, 0.93);
const DELAY: Color = Color::from_rgb(0.35, 0.69, 0.90);
const REVERB: Color = Color::from_rgb(0.58, 0.63, 0.93);
const CAB: Color = Color::from_rgb(0.79, 0.57, 0.37);
const LIMITER: Color = Color::from_rgb(0.94, 0.46, 0.31);

const TS9: Color = Color::from_rgb(0.35, 0.77, 0.45);
const BD2: Color = Color::from_rgb(0.42, 0.58, 0.94);
const CENTAUR: Color = Color::from_rgb(0.86, 0.72, 0.38);
const EVVA: Color = Color::from_rgb(0.93, 0.42, 0.58);
const RED_CHARLIE: Color = Color::from_rgb(0.89, 0.32, 0.28);
const MONSTER5150: Color = Color::from_rgb(0.88, 0.86, 0.82);

/// A family's signature color (chain card band, fallbacks).
pub fn family_color(key: &str) -> Color {
    match key {
        "gate" => GATE,
        "comp" => COMP,
        "amp" => AMP,
        "eq" => EQ,
        "mod" => MODULATION,
        "delay" => DELAY,
        "reverb" => REVERB,
        "cab" => CAB,
        "limiter" => LIMITER,
        _ => ACCENT, // drive family: the pedal picks the color
    }
}

/// The signature color of one pedal — drive pedals wear their real-world
/// liveries; everything else inherits the family color.
pub fn pedal_color(family_key: &str, pedal_key: &str) -> Color {
    if family_key != "drive" {
        return family_color(family_key);
    }
    match pedal_key {
        "ts9" => TS9,
        "bd2" => BD2,
        "centaur" => CENTAUR,
        "evva" => EVVA,
        "red-charlie" => RED_CHARLIE,
        "monster5150" => MONSTER5150,
        _ => ACCENT, // classic wears the house amber
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

// --- buttons -------------------------------------------------------------------

fn lift(bg: Color, hovered: bool) -> Color {
    if hovered {
        Color {
            r: (bg.r + 0.035).min(1.0),
            g: (bg.g + 0.035).min(1.0),
            b: (bg.b + 0.035).min(1.0),
            a: bg.a.max(1.0),
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
        } else {
            (Color::TRANSPARENT, TEXT_DIM, Color::TRANSPARENT)
        };
        button::Style {
            background: Some(Background::Color(if lit { bg } else { lift(bg, hovered) })),
            text_color: if hovered && !lit { TEXT } else { fg },
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

//! Palette and widget styles — dark amp-faceplate look, one amber accent.

use iced::overlay::menu;
use iced::widget::{button, container, pick_list, text_input};
use iced::{Background, Border, Color, Shadow, Theme};

pub const BG: Color = Color::from_rgb(0.075, 0.075, 0.095);
pub const PANEL: Color = Color::from_rgb(0.115, 0.115, 0.145);
pub const PANEL_HI: Color = Color::from_rgb(0.165, 0.165, 0.205);
pub const TRACK: Color = Color::from_rgb(0.23, 0.23, 0.27);
pub const ACCENT: Color = Color::from_rgb(1.0, 0.55, 0.24);
pub const TEXT_DIM: Color = Color::from_rgb(0.55, 0.55, 0.60);
pub const TEXT_BRIGHT: Color = Color::from_rgb(0.92, 0.92, 0.94);
pub const METER_OK: Color = Color::from_rgb(0.30, 0.78, 0.42);
pub const METER_HOT: Color = Color::from_rgb(0.92, 0.26, 0.21);

fn rounded(color: Color, width: f32) -> Border {
    Border {
        color,
        width,
        radius: 6.0.into(),
    }
}

pub fn root(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(BG)),
        text_color: Some(TEXT_BRIGHT),
        ..container::Style::default()
    }
}

pub fn panel(_: &Theme) -> container::Style {
    container::Style {
        background: Some(Background::Color(PANEL)),
        border: rounded(Color::TRANSPARENT, 0.0),
        ..container::Style::default()
    }
}

fn button_style(bg: Color, fg: Color, border: Color, hovered: bool) -> button::Style {
    let lift = if hovered { 0.03 } else { 0.0 };
    button::Style {
        background: Some(Background::Color(Color {
            r: (bg.r + lift).min(1.0),
            g: (bg.g + lift).min(1.0),
            b: (bg.b + lift).min(1.0),
            a: bg.a,
        })),
        text_color: fg,
        border: rounded(border, 1.0),
        ..button::Style::default()
    }
}

/// A chain-strip card (the board editor): accent frame when selected, the
/// dragged source dims, the drop target gets a thick accent frame.
pub fn drag_card(
    selected: bool,
    active: bool,
    dragging: bool,
    target: bool,
) -> impl Fn(&Theme) -> container::Style {
    move |_| {
        let border = if target || selected { ACCENT } else { TRACK };
        container::Style {
            background: Some(Background::Color(if active { PANEL_HI } else { PANEL })),
            text_color: Some(if dragging || !active {
                TEXT_DIM
            } else {
                TEXT_BRIGHT
            }),
            border: rounded(border, if target { 2.0 } else { 1.0 }),
            ..container::Style::default()
        }
    }
}

/// Header chips and list rows; `lit` draws it in the accent color.
pub fn chip(lit: bool) -> impl Fn(&Theme, button::Status) -> button::Style {
    move |_, status| {
        let hovered = matches!(status, button::Status::Hovered);
        let (fg, border) = if lit {
            (ACCENT, ACCENT)
        } else {
            (TEXT_DIM, TRACK)
        };
        button_style(PANEL, fg, border, hovered)
    }
}

/// Plain action button (load, save, unload, move).
pub fn action(_: &Theme, status: button::Status) -> button::Style {
    let hovered = matches!(status, button::Status::Hovered);
    let disabled = matches!(status, button::Status::Disabled);
    let fg = if disabled { TRACK } else { TEXT_BRIGHT };
    button_style(PANEL_HI, fg, TRACK, hovered)
}

pub fn pick(_: &Theme, status: pick_list::Status) -> pick_list::Style {
    let lit = !matches!(status, pick_list::Status::Active);
    pick_list::Style {
        text_color: TEXT_BRIGHT,
        placeholder_color: TEXT_DIM,
        handle_color: if lit { ACCENT } else { TEXT_DIM },
        background: Background::Color(PANEL_HI),
        border: rounded(if lit { ACCENT } else { TRACK }, 1.0),
    }
}

pub fn menu(_: &Theme) -> menu::Style {
    menu::Style {
        background: Background::Color(PANEL_HI),
        border: rounded(TRACK, 1.0),
        text_color: TEXT_BRIGHT,
        selected_text_color: ACCENT,
        selected_background: Background::Color(PANEL),
        shadow: Shadow::default(),
    }
}

pub fn input(_: &Theme, _status: text_input::Status) -> text_input::Style {
    text_input::Style {
        background: Background::Color(PANEL_HI),
        border: rounded(TRACK, 1.0),
        icon: TEXT_DIM,
        placeholder: TEXT_DIM,
        value: TEXT_BRIGHT,
        selection: ACCENT,
    }
}

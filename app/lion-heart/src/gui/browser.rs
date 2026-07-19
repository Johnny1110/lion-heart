//! A minimal file browser for picking NAM captures and cabinet IRs:
//! directories first, files filtered by extension, remembers the last
//! directory per asset kind (via `AppConfig`).

use std::path::{Path, PathBuf};

use iced::widget::{button, column, container, row, scrollable, text};
use iced::{Element, Length};

use super::theme;
use super::{AssetKind, Message};

pub struct Browser {
    pub kind: AssetKind,
    pub cwd: PathBuf,
    entries: Vec<Entry>,
    error: Option<String>,
}

struct Entry {
    name: String,
    path: PathBuf,
    is_dir: bool,
}

impl Browser {
    pub fn open(kind: AssetKind, start: PathBuf) -> Self {
        let mut browser = Self {
            kind,
            cwd: start,
            entries: Vec::new(),
            error: None,
        };
        browser.refresh();
        browser
    }

    pub fn navigate(&mut self, path: PathBuf) {
        self.cwd = path;
        self.refresh();
    }

    fn wanted(&self, path: &Path) -> bool {
        let ext = path.extension().map(|e| e.to_string_lossy().to_lowercase());
        match self.kind {
            AssetKind::Nam => ext.as_deref() == Some("nam"),
            AssetKind::Ir | AssetKind::IrB => ext.as_deref() == Some("wav"),
        }
    }

    fn refresh(&mut self) {
        self.entries.clear();
        self.error = None;
        let entries = match std::fs::read_dir(&self.cwd) {
            Ok(e) => e,
            Err(e) => {
                self.error = Some(format!("cannot read {}: {e}", self.cwd.display()));
                return;
            }
        };
        for entry in entries.filter_map(|e| e.ok()) {
            let path = entry.path();
            let name = entry.file_name().to_string_lossy().into_owned();
            if name.starts_with('.') {
                continue;
            }
            let is_dir = entry.file_type().map(|t| t.is_dir()).unwrap_or(false);
            if is_dir || self.wanted(&path) {
                self.entries.push(Entry { name, path, is_dir });
            }
        }
        self.entries
            .sort_by(|a, b| b.is_dir.cmp(&a.is_dir).then(a.name.cmp(&b.name)));
    }

    pub fn view(&self) -> Element<'_, Message> {
        let title = match self.kind {
            AssetKind::Nam => "load amp capture (.nam)",
            AssetKind::Ir => "load cabinet IR — mic A (.wav)",
            AssetKind::IrB => "load blend IR — mic B (.wav)",
        };
        let header = row![
            text(title).size(15).color(theme::TEXT_BRIGHT),
            text(self.cwd.display().to_string())
                .size(12)
                .color(theme::TEXT_DIM)
                .font(iced::Font::MONOSPACE)
                .width(Length::Fill),
            button(text("close").size(12))
                .padding([4, 12])
                .on_press(Message::BrowserClose)
                .style(theme::action),
        ]
        .spacing(12)
        .align_y(iced::Alignment::Center);

        let mut listing = column![].spacing(1);
        if let Some(parent) = self.cwd.parent() {
            listing = listing.push(
                button(text("⬑ ..").size(13).color(theme::TEXT_DIM))
                    .width(Length::Fill)
                    .padding([5, 10])
                    .on_press(Message::BrowserNav(parent.to_path_buf()))
                    .style(theme::list_row),
            );
        }
        if let Some(error) = &self.error {
            listing = listing.push(text(error.clone()).size(13).color(theme::METER_HOT));
        }
        for entry in &self.entries {
            let label = if entry.is_dir {
                format!("▸ {}/", entry.name)
            } else {
                entry.name.clone()
            };
            let message = if entry.is_dir {
                Message::BrowserNav(entry.path.clone())
            } else {
                Message::BrowserPick(entry.path.clone())
            };
            listing = listing.push(
                button(text(label).size(13).color(if entry.is_dir {
                    theme::TEXT_DIM
                } else {
                    theme::TEXT_BRIGHT
                }))
                .width(Length::Fill)
                .padding([5, 10])
                .on_press(message)
                .style(theme::list_row),
            );
        }

        container(column![header, scrollable(listing).height(Length::Fill)].spacing(10))
            .style(theme::panel)
            .padding(16)
            .width(Length::Fill)
            .height(Length::Fill)
            .into()
    }
}

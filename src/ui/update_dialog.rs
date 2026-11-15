//! The update dialog (canvas `MapCraft — Updates`, artboards 2 and 3; design.md §3.10): the
//! confirmation, then the download, and what can happen. The work itself runs in
//! `crate::update`; this only draws the stage the app hands it and reports the buttons.

use egui::{ProgressBar, RichText};

use crate::theme::Palette;
use crate::ui::widgets::{self, Tone, banner};
use crate::update::{Release, Version};

#[derive(Debug, Clone, PartialEq)]
pub enum Stage {
    /// Both versions and the download size: Not now / Update and restart.
    Confirm,
    /// MapCraft's folder isn't writable; shown before anything is downloaded.
    NotWritable {
        folder: String,
    },
    Downloading {
        done: u64,
        total: u64,
    },
    /// Downloaded and checked. `waiting` names the work the restart waits for, if any.
    Ready {
        waiting: Option<String>,
    },
    /// The download failed; `checksum` when the file didn't match.
    Failed {
        checksum: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateAction {
    Start,
    /// Cancel the download (the dialog closes once it has stopped).
    Cancel,
    Close,
    RestartNow,
    RestartLater,
    ReleasePage,
}

pub struct UpdateDialog {
    pub release: Release,
    pub stage: Stage,
}

impl UpdateDialog {
    pub fn new(release: Release, stage: Stage) -> Self {
        Self { release, stage }
    }

    pub fn show(&self, ctx: &egui::Context, p: &Palette) -> Option<UpdateAction> {
        let new = self.release.version;
        let mut action = None;
        let (mut primary, mut secondary): (Option<(&str, UpdateAction, bool)>, Option<_>) =
            (None, None);
        match &self.stage {
            Stage::Confirm => {
                primary = Some(("Update and restart", UpdateAction::Start, true));
                secondary = Some(("Not now", UpdateAction::Close));
            }
            Stage::NotWritable { .. } => {
                primary = Some(("Open release page", UpdateAction::ReleasePage, true));
                secondary = Some(("Close", UpdateAction::Close));
            }
            Stage::Downloading { .. } => {
                primary = Some(("Update and restart", UpdateAction::Start, false));
                secondary = Some(("Cancel", UpdateAction::Cancel));
            }
            Stage::Ready { waiting: Some(_) } => {
                primary = Some(("Restart now", UpdateAction::RestartNow, true));
                secondary = Some(("Restart later", UpdateAction::RestartLater));
            }
            Stage::Ready { waiting: None } => {}
            Stage::Failed { .. } => {
                primary = Some(("Try again", UpdateAction::Start, true));
                secondary = Some(("Close", UpdateAction::Close));
            }
        }
        // Esc and × count as the secondary button, except while restarting.
        let closable = !matches!(self.stage, Stage::Ready { waiting: None });
        let mut notes = false;
        let close = widgets::dialog(
            ctx,
            p,
            "update-dialog",
            "Update MapCraft",
            440.0,
            closable,
            |ui| notes = self.body(ui, p, new),
            |ui| {
                if let Some((label, a, enabled)) = primary
                    && widgets::primary(ui, p, None, label, enabled).clicked()
                {
                    action = Some(a);
                }
                if let Some((label, a)) = secondary
                    && widgets::secondary(ui, p, None, label, true).clicked()
                {
                    action = Some(a);
                }
            },
        );
        if notes {
            action = Some(UpdateAction::ReleasePage);
        }
        if close && action.is_none() {
            action = Some(match self.stage {
                Stage::Downloading { .. } => UpdateAction::Cancel,
                Stage::Ready { .. } => UpdateAction::RestartLater,
                _ => UpdateAction::Close,
            });
        }
        action
    }

    /// The stage's content. True when "What's new" was clicked.
    fn body(&self, ui: &mut egui::Ui, p: &Palette, new: Version) -> bool {
        let mut notes = false;
        let text = |ui: &mut egui::Ui, t: &str| {
            ui.label(RichText::new(t).color(p.text_normal));
        };
        match &self.stage {
            Stage::Confirm => {
                egui::Grid::new("update-facts")
                    .num_columns(2)
                    .spacing([p.space_3, 4.0])
                    .show(ui, |ui| {
                        for (k, v) in [
                            ("New version", format!("MapCraft {new}")),
                            ("You have", format!("MapCraft {}", Version::current())),
                            (
                                "Download",
                                format!("{} from GitHub", mb(self.release.exe_size)),
                            ),
                        ] {
                            ui.label(RichText::new(k).color(p.text_weak));
                            ui.label(RichText::new(v).color(p.text_strong));
                            ui.end_row();
                        }
                    });
                ui.label(
                    RichText::new(
                        "MapCraft downloads the new version, checks it, then closes and starts \
                         again. Your library, thumbnails and settings stay as they are.",
                    )
                    .small()
                    .color(p.text_weak),
                );
                notes = ui.link(format!("What’s new in {new}")).clicked();
            }
            Stage::NotWritable { folder } => {
                text(
                    ui,
                    &format!("MapCraft can’t change its own file in {folder}."),
                );
                banner(
                    ui,
                    p,
                    Tone::Warning,
                    "Download mapcraft.exe from the release page and replace this one, or keep \
                     MapCraft in a folder of your own, such as AppData\\Local\\Programs\\MapCraft.",
                );
            }
            Stage::Downloading { done, total } => {
                text(ui, &format!("Downloading MapCraft {new}…"));
                progress(ui, p, *done, *total, "");
            }
            Stage::Ready { waiting } => match waiting {
                None => {
                    text(ui, &format!("MapCraft {new} is ready. Restarting…"));
                    let size = self.release.exe_size;
                    progress(ui, p, size, size, " · checksum OK");
                }
                Some(work) => {
                    text(ui, &format!("MapCraft {new} is downloaded and checked."));
                    banner(
                        ui,
                        p,
                        Tone::Info,
                        &format!("{work}. MapCraft restarts when it’s done."),
                    );
                }
            },
            Stage::Failed { checksum: true } => banner(
                ui,
                p,
                Tone::Error,
                "The downloaded file doesn’t match its checksum, so it was deleted. MapCraft \
                 wasn’t changed.",
            ),
            Stage::Failed { checksum: false } => banner(
                ui,
                p,
                Tone::Error,
                "The download broke off. MapCraft wasn’t changed.",
            ),
        }
        notes
    }
}

/// "27.5 MB".
pub fn mb(bytes: u64) -> String {
    format!("{:.1} MB", bytes as f64 / 1_000_000.0)
}

fn progress(ui: &mut egui::Ui, p: &Palette, done: u64, total: u64, suffix: &str) {
    let f = if total > 0 {
        (done as f32 / total as f32).min(1.0)
    } else {
        0.0
    };
    ui.add(
        ProgressBar::new(f)
            .desired_height(6.0)
            .fill(p.accent)
            .corner_radius(3),
    );
    let total_text = if total > 0 { mb(total) } else { "?".into() };
    ui.label(
        RichText::new(format!(
            "{} of {total_text}{suffix}",
            mb(done).trim_end_matches(" MB")
        ))
        .small()
        .color(p.text_weak),
    );
}

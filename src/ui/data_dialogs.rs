//! The data folder's dialogs (canvas `MapCraft — Data folder`, artboards 2 and 3; design.md
//! M8): changing the folder, freeing space, and removing the old data after a move. The work
//! runs in `crate::datafolder` on background threads; these draw the stage the app hands them
//! and report the buttons.

use std::path::{Path, PathBuf};

use egui::{ProgressBar, RichText, Ui};

use crate::datafolder::{self, Kind, Progress, Usage};
use crate::import::scope::Check;
use crate::theme::Palette;
use crate::ui::widgets::{self, Tone, banner};

/// What Change… leads to.
#[derive(Debug, Clone, PartialEq)]
pub enum ChangeStage {
    /// An empty or new folder: move the data there or start empty. `inside` when the picked
    /// folder held other files; `network` for the §3.3 warning.
    Choose {
        dir: PathBuf,
        inside: bool,
        network: bool,
        picked: PathBuf,
    },
    /// A folder with a MapCraft library: use it as it is.
    Existing {
        dir: PathBuf,
        photos: Option<i64>,
    },
    /// The thumbnails being made pause before the copy starts.
    Pausing,
    Copying {
        dir: PathBuf,
        at: Option<Progress>,
    },
    /// Refused or failed; nothing changed.
    Failed(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeAction {
    Move,
    StartEmpty,
    UseExisting,
    ChooseAnother,
    Cancel,
}

pub struct ChangeDialog {
    pub stage: ChangeStage,
    /// The data's size, for "Move my data there (4.82 GB)".
    pub size: Option<u64>,
    /// The chosen option in `Choose`: move (true) or start empty.
    moving: bool,
}

impl ChangeDialog {
    pub fn new(stage: ChangeStage, size: Option<u64>) -> Self {
        Self {
            stage,
            size,
            moving: true,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> Option<ChangeAction> {
        let mut action = None;
        let mut body_action = None;
        let copying = matches!(
            self.stage,
            ChangeStage::Copying { .. } | ChangeStage::Pausing
        );
        // The footer can't look at `self` while the body changes it: what it needs, first.
        #[derive(Clone, Copy)]
        enum Foot {
            Choose(bool),
            Existing,
            Busy,
            Failed,
        }
        let foot = match &self.stage {
            ChangeStage::Choose { .. } => Foot::Choose(self.moving),
            ChangeStage::Existing { .. } => Foot::Existing,
            ChangeStage::Pausing | ChangeStage::Copying { .. } => Foot::Busy,
            ChangeStage::Failed(_) => Foot::Failed,
        };
        let close = widgets::dialog(
            ctx,
            p,
            "change-data-folder",
            "Change data folder",
            440.0,
            true,
            |ui| body_action = self.body(ui, p),
            |ui| match foot {
                Foot::Choose(moving) => {
                    let label = if moving {
                        "Move and restart"
                    } else {
                        "Start empty and restart"
                    };
                    if widgets::primary(ui, p, None, label, true).clicked() {
                        action = Some(if moving {
                            ChangeAction::Move
                        } else {
                            ChangeAction::StartEmpty
                        });
                    }
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(ChangeAction::Cancel);
                    }
                }
                Foot::Existing => {
                    if widgets::primary(ui, p, None, "Use it and restart", true).clicked() {
                        action = Some(ChangeAction::UseExisting);
                    }
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(ChangeAction::Cancel);
                    }
                    left(ui, |ui| {
                        if widgets::secondary(ui, p, None, "Choose another…", true).clicked() {
                            action = Some(ChangeAction::ChooseAnother);
                        }
                    });
                }
                Foot::Busy => {
                    widgets::primary(ui, p, None, "Move and restart", false);
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(ChangeAction::Cancel);
                    }
                }
                Foot::Failed => {
                    if widgets::primary(ui, p, None, "Close", true).clicked() {
                        action = Some(ChangeAction::Cancel);
                    }
                    left(ui, |ui| {
                        if widgets::secondary(ui, p, None, "Choose another…", true).clicked() {
                            action = Some(ChangeAction::ChooseAnother);
                        }
                    });
                }
            },
        );
        if close && action.is_none() && !copying {
            action = Some(ChangeAction::Cancel);
        }
        if close && copying {
            action = Some(ChangeAction::Cancel);
        }
        action.or(body_action)
    }

    fn body(&mut self, ui: &mut Ui, p: &Palette) -> Option<ChangeAction> {
        match &self.stage {
            ChangeStage::Choose {
                dir,
                inside,
                network,
                picked,
            } => {
                folder_line(ui, p, "New folder: ", dir);
                if *inside {
                    banner(
                        ui,
                        p,
                        Tone::Info,
                        &format!(
                            "{} holds other files, so MapCraft uses a new folder inside it.",
                            picked.display()
                        ),
                    );
                }
                if *network {
                    banner(
                        ui,
                        p,
                        Tone::Warning,
                        "This folder is on the network. MapCraft is slower there, and two \
                         computers must never open its library at the same time.",
                    );
                }
                let size = self
                    .size
                    .map(|b| format!(" ({})", datafolder::bytes_text(b)))
                    .unwrap_or_default();
                if option(
                    ui,
                    p,
                    self.moving,
                    &format!("Move my data there{size}"),
                    "MapCraft copies it, then restarts there. The old folder stays until you \
                     remove it.",
                ) {
                    self.moving = true;
                }
                if option(
                    ui,
                    p,
                    !self.moving,
                    "Start empty there",
                    "MapCraft restarts with an empty library. Your current data stays where \
                     it is.",
                ) {
                    self.moving = false;
                }
            }
            ChangeStage::Existing { dir, photos } => {
                let n = photos
                    .map(|n| format!(" with {} photos", widgets::thousands(n as usize)))
                    .unwrap_or_default();
                ui.label(
                    RichText::new(format!(
                        "{} already holds a MapCraft library{n}.",
                        dir.display()
                    ))
                    .color(p.text_normal),
                );
                ui.label(
                    RichText::new(
                        "MapCraft restarts with that library. Your current data stays where it \
                         is.",
                    )
                    .small()
                    .color(p.text_weak),
                );
            }
            ChangeStage::Pausing => {
                ui.label(RichText::new("Pausing the thumbnails…").color(p.text_normal));
            }
            ChangeStage::Copying { dir, at } => {
                let what = at.map_or("data", |a| match a.kind {
                    Kind::Thumbnails => "thumbnails",
                    Kind::Previews => "previews",
                    Kind::Tiles => "map tiles",
                    Kind::Library => "the library",
                    Kind::Places => "place names",
                });
                folder_line(ui, p, &format!("Copying {what} to "), dir);
                let (done, total) = at.map_or((0, 0), |a| (a.done.bytes, a.total.bytes));
                let f = if total > 0 {
                    done as f32 / total as f32
                } else {
                    0.0
                };
                ui.add(
                    ProgressBar::new(f.min(1.0))
                        .desired_height(6.0)
                        .fill(p.accent)
                        .corner_radius(3),
                );
                if let Some(a) = at {
                    ui.label(
                        RichText::new(format!(
                            "{} of {} · {} of {} files",
                            datafolder::bytes_text(a.done.bytes),
                            datafolder::bytes_text(a.total.bytes),
                            widgets::thousands(a.done.files as usize),
                            widgets::thousands(a.total.files as usize)
                        ))
                        .small()
                        .color(p.text_weak),
                    );
                }
                ui.label(
                    RichText::new("MapCraft restarts there when the copy is complete.")
                        .small()
                        .color(p.text_weak),
                );
            }
            ChangeStage::Failed(msg) => banner(ui, p, Tone::Error, msg),
        }
        None
    }
}

fn left(ui: &mut Ui, add: impl FnOnce(&mut Ui)) {
    ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), add);
}

fn folder_line(ui: &mut Ui, p: &Palette, lead: &str, dir: &Path) {
    ui.horizontal_wrapped(|ui| {
        ui.spacing_mut().item_spacing.x = 0.0;
        ui.label(RichText::new(lead).color(p.text_normal));
        ui.label(
            RichText::new(dir.display().to_string())
                .monospace()
                .color(p.text_strong),
        );
    });
}

/// A radio option with its explanation. True when clicked.
fn option(ui: &mut Ui, p: &Palette, on: bool, title: &str, text: &str) -> bool {
    let mut clicked = false;
    ui.horizontal_top(|ui| {
        ui.spacing_mut().item_spacing.x = p.space_2;
        let r = ui.radio(on, "");
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::RadioButton, true, on, title));
        clicked |= r.clicked();
        ui.vertical(|ui| {
            clicked |= ui
                .add(
                    egui::Label::new(RichText::new(title).color(p.text_strong))
                        .sense(egui::Sense::click()),
                )
                .clicked();
            ui.label(RichText::new(text).small().color(p.text_weak));
        });
    });
    clicked
}

/// Free up space (canvas artboard 3, first board): what MapCraft can make again.
pub struct FreeSpaceDialog {
    /// Ticked kinds, in [`Kind::REMAKEABLE`] order: previews, tiles, thumbnails.
    ticked: [bool; 3],
    usage: Usage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FreeAction {
    Delete(Vec<Kind>),
    Cancel,
}

impl FreeSpaceDialog {
    pub fn new(usage: Usage) -> Self {
        Self {
            ticked: [true, true, false],
            usage,
        }
    }

    pub fn show(&mut self, ctx: &egui::Context, p: &Palette) -> Option<FreeAction> {
        let mut action = None;
        let bytes: u64 = Kind::REMAKEABLE
            .iter()
            .zip(self.ticked)
            .filter(|(_, t)| *t)
            .map(|(k, _)| self.usage.get(*k).bytes)
            .sum();
        let any = self.ticked.iter().any(|t| *t);
        let chosen: Vec<Kind> = Kind::REMAKEABLE
            .into_iter()
            .zip(self.ticked)
            .filter(|(_, t)| *t)
            .map(|(k, _)| k)
            .collect();
        let usage = &self.usage;
        let ticked = &mut self.ticked;
        let close = widgets::dialog(
            ctx,
            p,
            "free-space",
            "Free up space",
            440.0,
            true,
            |ui| {
                ui.label(
                    RichText::new(
                        "MapCraft makes these again when they’re needed. Your photos and the \
                         library aren’t touched.",
                    )
                    .small()
                    .color(p.text_weak),
                );
                for (i, k) in Kind::REMAKEABLE.into_iter().enumerate() {
                    let note = match k {
                        Kind::Previews => {
                            "Made again when you open a photo, which then takes a moment longer."
                        }
                        Kind::Tiles => {
                            "Downloaded again from OpenStreetMap as you move around the map."
                        }
                        _ => {
                            "Made again in the background by reading every photo from its \
                             drive: hours for a large library on a network share. Until then \
                             the map shows dots. Photos on drives that aren’t connected stay \
                             without one."
                        }
                    };
                    ui.add_space(p.space_1);
                    ui.horizontal_top(|ui| {
                        ui.spacing_mut().item_spacing.x = p.space_2;
                        let state = if ticked[i] { Check::On } else { Check::Off };
                        let mut clicked =
                            widgets::tri_check(ui, p, state, true, k.label()).clicked();
                        ui.vertical(|ui| {
                            ui.horizontal(|ui| {
                                clicked |= ui
                                    .add(
                                        egui::Label::new(
                                            RichText::new(k.label()).color(p.text_strong),
                                        )
                                        .sense(egui::Sense::click()),
                                    )
                                    .clicked();
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        ui.label(
                                            RichText::new(datafolder::bytes_text(
                                                usage.get(k).bytes,
                                            ))
                                            .color(p.text_strong),
                                        );
                                    },
                                );
                            });
                            ui.label(RichText::new(note).small().color(p.text_weak));
                        });
                        if clicked {
                            ticked[i] = !ticked[i];
                        }
                    });
                }
            },
            |ui| {
                let label = format!("Delete {}", datafolder::bytes_text(bytes));
                if widgets::danger(ui, p, &label, any).clicked() {
                    action = Some(FreeAction::Delete(chosen.clone()));
                }
                if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                    action = Some(FreeAction::Cancel);
                }
            },
        );
        if close && action.is_none() {
            action = Some(FreeAction::Cancel);
        }
        action
    }
}

/// Remove old data (canvas artboard 3, third board). True: remove; false: keep.
pub fn remove_old_dialog(
    ctx: &egui::Context,
    p: &Palette,
    old: &Path,
    current: &Path,
    size: Option<u64>,
) -> Option<bool> {
    let mut answer = None;
    let size_text = size.map(datafolder::bytes_text);
    let close = widgets::dialog(
        ctx,
        p,
        "remove-old-data",
        "Remove old data",
        440.0,
        true,
        |ui| {
            ui.horizontal_top(|ui| {
                ui.add(Tone::Warning.icon().image(20.0, p.status_warning));
                ui.add(
                    egui::Label::new(
                        RichText::new(format!(
                            "Delete MapCraft’s old library, thumbnails, previews, map tiles and \
                             place names from {}{}? The data in {} is what MapCraft uses now.",
                            old.display(),
                            size_text
                                .as_ref()
                                .map(|s| format!(" ({s})"))
                                .unwrap_or_default(),
                            current.display()
                        ))
                        .color(p.text_normal),
                    )
                    .wrap(),
                );
            });
        },
        |ui| {
            let label = match &size_text {
                Some(s) => format!("Remove {s}"),
                None => "Remove".to_owned(),
            };
            if widgets::danger(ui, p, &label, true).clicked() {
                answer = Some(true);
            }
            if widgets::secondary(ui, p, None, "Keep it", true).clicked() {
                answer = Some(false);
            }
        },
    );
    if close && answer.is_none() {
        answer = Some(false);
    }
    answer
}

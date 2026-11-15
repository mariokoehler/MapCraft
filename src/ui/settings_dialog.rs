//! The Settings dialog: where the companion apps are (canvas `MapCraft — Companion apps`,
//! artboard 1) and updates (canvas `MapCraft — Updates`, artboard 1); design.md §2.7.
//!
//! Whether each path leads to a file is checked as it's typed (a local `stat`). Versions come
//! from each app's command-line tool on a background thread, when the dialog opens and on Test.
//! The native file picker runs on its own thread too, as in the folder dialog.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Align, Layout, RichText, TextEdit, Ui};

use crate::companions::{self, Companion};
use crate::import::scope::Check;
use crate::settings::CompanionSettings;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets;
use crate::update::{self, CheckStatus};

const APPS: [Companion; 2] = [Companion::PhotoCraft, Companion::LightCraft];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    /// Save these paths (the rest of the settings stays as it is).
    Save {
        photocraft: PathBuf,
        lightcraft: PathBuf,
        check_updates: bool,
    },
    Close,
    /// Ask GitHub now (the dialog stays open and shows the answer).
    CheckNow,
    /// Open the update dialog for the release found.
    Update,
}

/// A version read in the background: which app, for which path, and the version.
type Version = (usize, String, Option<String>);

#[derive(Debug, Clone, PartialEq, Eq)]
enum Status {
    Missing,
    Checking,
    /// The file is there; its version, once read (`None`: not read, or not readable).
    Found(Option<String>),
}

pub struct SettingsDialog {
    paths: [String; 2],
    status: [Status; 2],
    /// The path each status is for: a typed change is checked again.
    checked: [String; 2],
    versions: (Sender<Version>, Receiver<Version>),
    picker: Option<(usize, Receiver<Option<PathBuf>>)>,
    /// Whether the version tools may be run (off in tests).
    run_tools: bool,
    check_updates: bool,
}

impl SettingsDialog {
    pub fn open(
        c: &CompanionSettings,
        check_updates: bool,
        run_tools: bool,
        ctx: &egui::Context,
    ) -> Self {
        let mut d = Self {
            paths: [
                c.photocraft.display().to_string(),
                c.lightcraft.display().to_string(),
            ],
            status: [Status::Missing, Status::Missing],
            checked: [String::new(), String::new()],
            versions: mpsc::channel(),
            picker: None,
            run_tools,
            check_updates,
        };
        d.test(ctx);
        d
    }

    /// Checks both paths and reads their versions.
    fn test(&mut self, ctx: &egui::Context) {
        for i in 0..2 {
            self.check(i, true, ctx);
        }
    }

    fn check(&mut self, i: usize, version: bool, ctx: &egui::Context) {
        let path = self.paths[i].trim().to_owned();
        self.checked[i] = self.paths[i].clone();
        self.status[i] = if !companions::is_app(Path::new(&path)) {
            Status::Missing
        } else if version && self.run_tools {
            let (tx, c) = (self.versions.0.clone(), ctx.clone());
            std::thread::spawn(move || {
                let v = companions::version(Path::new(&path));
                let _ = tx.send((i, path, v));
                c.request_repaint();
            });
            Status::Checking
        } else {
            Status::Found(None)
        };
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        status: CheckStatus,
    ) -> Option<SettingsAction> {
        for (i, path, v) in self.versions.1.try_iter() {
            if self.paths[i].trim() == path {
                self.status[i] = Status::Found(v);
            }
        }
        if let Some((i, rx)) = &self.picker
            && let Ok(picked) = rx.try_recv()
        {
            let i = *i;
            self.picker = None;
            if let Some(f) = picked {
                self.paths[i] = f.display().to_string();
                self.check(i, true, ctx);
            }
        }
        for i in 0..2 {
            if self.paths[i] != self.checked[i] {
                self.check(i, false, ctx);
            }
        }

        let (mut save, mut test, mut cancel, mut browse) = (false, false, false, None);
        let mut update_action = None;
        let close = widgets::dialog(
            ctx,
            p,
            "settings-dialog",
            "Settings",
            560.0,
            true,
            |ui| {
                ui.label(
                    RichText::new("Companion apps")
                        .strong()
                        .color(p.text_strong),
                );
                ui.label(
                    RichText::new(
                        "The detail panel opens photos in them. MapCraft never changes your \
                         photos.",
                    )
                    .small()
                    .color(p.text_weak),
                );
                ui.add_space(p.space_2);
                for (i, app) in APPS.into_iter().enumerate() {
                    if self.app_row(ui, p, i, app) {
                        browse = Some(i);
                    }
                    if app == Companion::LightCraft {
                        ui.label(
                            RichText::new(
                                "To show a photo, MapCraft starts LightCraft with remote control \
                                 on: a local port, without a password, that any program on this \
                                 computer could use while LightCraft runs.",
                            )
                            .small()
                            .color(p.text_weak),
                        );
                    }
                    ui.add_space(p.space_2);
                }
                update_action = self.updates(ui, p, status);
            },
            |ui| {
                save = widgets::primary(ui, p, None, "Save", true).clicked();
                cancel = widgets::secondary(ui, p, None, "Cancel", true).clicked();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    test = widgets::secondary(ui, p, None, "Test", true).clicked();
                });
            },
        );
        if test {
            self.test(ctx);
        }
        if let Some(i) = browse
            && self.picker.is_none()
        {
            let (tx, rx) = mpsc::channel();
            let (c, title) = (ctx.clone(), format!("Where is {}?", APPS[i].name()));
            std::thread::spawn(move || {
                let f = rfd::FileDialog::new()
                    .set_title(title)
                    .add_filter("App", &["exe"])
                    .pick_file();
                let _ = tx.send(f);
                c.request_repaint();
            });
            self.picker = Some((i, rx));
        }
        if save {
            Some(SettingsAction::Save {
                photocraft: PathBuf::from(self.paths[0].trim()),
                lightcraft: PathBuf::from(self.paths[1].trim()),
                check_updates: self.check_updates,
            })
        } else if update_action.is_some() {
            update_action
        } else if close || cancel {
            Some(SettingsAction::Close)
        } else {
            None
        }
    }

    /// The Updates section: the version, the checkbox, Check now and what the check found.
    fn updates(&mut self, ui: &mut Ui, p: &Palette, status: CheckStatus) -> Option<SettingsAction> {
        let mut action = None;
        let (r, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), p.stroke_hairline),
            egui::Sense::hover(),
        );
        ui.painter().rect_filled(r, 0.0, p.border_subtle);
        ui.add_space(p.space_2);
        ui.horizontal(|ui| {
            ui.label(RichText::new("Updates").strong().color(p.text_strong));
            ui.label(
                RichText::new(format!("MapCraft {}", update::Version::current()))
                    .small()
                    .color(p.text_weak),
            );
        });
        let label = "Check for updates when MapCraft starts";
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = p.space_2;
            let state = if self.check_updates {
                Check::On
            } else {
                Check::Off
            };
            let mut clicked = widgets::tri_check(ui, p, state, true, label).clicked();
            clicked |= ui
                .add(
                    egui::Label::new(RichText::new(label).color(p.text_normal))
                        .sense(egui::Sense::click()),
                )
                .clicked();
            if clicked {
                self.check_updates = !self.check_updates;
            }
        });
        ui.horizontal(|ui| {
            ui.set_min_height(24.0);
            ui.spacing_mut().item_spacing.x = 6.0;
            let busy = status == CheckStatus::Checking;
            if widgets::secondary(ui, p, None, "Check now", !busy).clicked() {
                action = Some(SettingsAction::CheckNow);
            }
            ui.add_space(p.space_1);
            match status {
                CheckStatus::Idle => {}
                CheckStatus::Checking => {
                    ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
                    ui.label(RichText::new("Asking GitHub…").small().color(p.text_weak));
                }
                CheckStatus::Latest => {
                    ui.add(Icon::CircleCheck.image(14.0, p.status_success));
                    ui.label(
                        RichText::new("You have the latest version.")
                            .small()
                            .color(p.text_weak),
                    );
                }
                CheckStatus::Available(v) => {
                    ui.add(Icon::Download.image(14.0, p.accent));
                    ui.label(
                        RichText::new(format!("MapCraft {v} is available."))
                            .small()
                            .color(p.text_normal),
                    );
                    if widgets::primary(ui, p, None, "Update…", true).clicked() {
                        action = Some(SettingsAction::Update);
                    }
                }
                CheckStatus::Failed => {
                    ui.add(Icon::CircleAlert.image(14.0, p.status_error));
                    ui.label(
                        RichText::new("Couldn’t reach GitHub. Try again later.")
                            .small()
                            .color(p.status_error),
                    );
                }
            }
        });
        ui.label(
            RichText::new(
                "MapCraft asks GitHub for its latest release. GitHub sees your IP address and                  that MapCraft runs; nothing about your photos is sent.",
            )
            .small()
            .color(p.text_weak),
        );
        action
    }

    /// One app: its path, Browse…, and what was found. Returns whether Browse… was clicked.
    fn app_row(&mut self, ui: &mut Ui, p: &Palette, i: usize, app: Companion) -> bool {
        widgets::label(ui, p, app.name());
        let mut browse = false;
        ui.horizontal(|ui| {
            let r = ui.add(
                TextEdit::singleline(&mut self.paths[i]).desired_width(ui.available_width() - 90.0),
            );
            let name = format!("{} app", app.name());
            ui.ctx()
                .accesskit_node_builder(r.id, |n| n.set_label(name.as_str()));
            browse = widgets::secondary(ui, p, None, "Browse…", self.picker.is_none()).clicked();
        });
        let (icon, color, text) = match &self.status[i] {
            Status::Missing => (
                Icon::CircleAlert,
                p.status_error,
                "Not found at this path".into(),
            ),
            Status::Checking => (
                Icon::CircleCheck,
                p.status_success,
                "Found; checking…".into(),
            ),
            Status::Found(Some(v)) => (
                Icon::CircleCheck,
                p.status_success,
                format!("{} {v}", app.name()),
            ),
            Status::Found(None) => (Icon::CircleCheck, p.status_success, "Found".into()),
        };
        ui.horizontal(|ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            ui.add(icon.image(14.0, color));
            let ink = if self.status[i] == Status::Missing {
                p.status_error
            } else {
                p.text_weak
            };
            ui.label(RichText::new(text).small().color(ink));
        });
        browse
    }
}

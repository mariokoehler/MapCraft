//! The Settings dialog, in sections (canvas `MapCraft — Data folder`, artboard 1; design.md
//! §2.7): where the companion apps are (canvas `MapCraft — Companion apps`), updates (canvas
//! `MapCraft — Updates`) and the data folder (M8).
//!
//! Whether each path leads to a file is checked as it's typed (a local `stat`). Versions come
//! from each app's command-line tool on a background thread, when the dialog opens and on Test.
//! The native file picker runs on its own thread too, as in the folder dialog.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Align, Layout, RichText, TextEdit, Ui};

use crate::companions::{self, Companion};
use crate::datafolder::{self, Kind, Usage};
use crate::import::scope::Check;
use crate::settings::CompanionSettings;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Tone};
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
    /// A folder was picked as the new data folder.
    ChangeDataFolder(PathBuf),
    ShowDataFolder,
    FreeSpace,
    RemoveOldData,
}

/// The sections, listed on the left.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Section {
    Companions,
    Updates,
    Data,
}

impl Section {
    const ALL: [Section; 3] = [Section::Companions, Section::Updates, Section::Data];

    fn label(self) -> &'static str {
        match self {
            Section::Companions => "Companion apps",
            Section::Updates => "Updates",
            Section::Data => "Data folder",
        }
    }

    fn icon(self) -> Icon {
        match self {
            Section::Companions => Icon::Brush,
            Section::Updates => Icon::Download,
            Section::Data => Icon::Folder,
        }
    }
}

/// What the Data folder section shows, from the app.
pub struct DataView<'a> {
    pub folder: &'a Path,
    /// Counted on a background thread; `None` while counting.
    pub usage: Option<&'a Usage>,
    /// The data folder MapCraft moved away from, and its size once counted.
    pub old: Option<(&'a Path, Option<u64>)>,
    /// Why the folder can't be changed now, if it can't.
    pub blocked: Option<&'a str>,
}

/// The colours of the kinds in the usage bar, in [`Kind::ALL`] order.
fn kind_color(p: &Palette, k: Kind) -> egui::Color32 {
    match k {
        Kind::Thumbnails => p.accent,
        Kind::Previews => p.status_info,
        Kind::Tiles => p.status_success,
        Kind::Library => p.status_warning,
        Kind::Places => p.text_weak,
    }
}

/// The dialog is 760 px: its margins, the 160 px section list, the line and a gap leave this.
const CONTENT_WIDTH: f32 = 760.0 - 24.0 - 160.0 - 1.0 - 12.0 - 8.0;

/// A section in the list on the left: icon and name, left-aligned, highlighted when shown.
fn nav_item(ui: &mut Ui, p: &Palette, s: Section, on: bool) -> egui::Response {
    let (rect, r) = ui.allocate_exact_size(egui::vec2(160.0, 28.0), egui::Sense::click());
    r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, on, s.label()));
    let fill = if on {
        p.accent_subtle
    } else if r.hovered() {
        p.widget_hovered_weak
    } else {
        egui::Color32::TRANSPARENT
    };
    ui.painter().rect_filled(rect, p.radius_sm, fill);
    let ink = if on { p.text_strong } else { p.text_normal };
    let icon = egui::Rect::from_center_size(
        egui::pos2(rect.left() + 8.0 + 8.0, rect.center().y),
        egui::vec2(16.0, 16.0),
    );
    s.icon().image(16.0, ink).paint_at(ui, icon);
    ui.painter().text(
        egui::pos2(rect.left() + 32.0, rect.center().y),
        egui::Align2::LEFT_CENTER,
        s.label(),
        egui::FontId::proportional(13.0),
        ink,
    );
    r.on_hover_cursor(egui::CursorIcon::PointingHand)
}

/// The picker for the data folder (the apps use 0 and 1).
const DATA_PICKER: usize = 2;

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
    pub section: Section,
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
            section: Section::Companions,
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
        data: &DataView<'_>,
    ) -> Option<SettingsAction> {
        for (i, path, v) in self.versions.1.try_iter() {
            if self.paths[i].trim() == path {
                self.status[i] = Status::Found(v);
            }
        }
        let mut picked_data = None;
        if let Some((i, rx)) = &self.picker
            && let Ok(picked) = rx.try_recv()
        {
            let i = *i;
            self.picker = None;
            if let Some(f) = picked {
                if i == DATA_PICKER {
                    picked_data = Some(f);
                } else {
                    self.paths[i] = f.display().to_string();
                    self.check(i, true, ctx);
                }
            }
        }
        for i in 0..2 {
            if self.paths[i] != self.checked[i] {
                self.check(i, false, ctx);
            }
        }

        let (mut save, mut test, mut cancel, mut browse) = (false, false, false, None);
        let mut action = None;
        let section = self.section;
        let close = widgets::dialog(
            ctx,
            p,
            "settings-dialog",
            "Settings",
            760.0,
            true,
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.vertical(|ui| {
                        ui.set_width(160.0);
                        ui.spacing_mut().item_spacing.y = 2.0;
                        for s in Section::ALL {
                            if nav_item(ui, p, s, s == section).clicked() {
                                self.section = s;
                            }
                        }
                    });
                    let (r, _) = ui.allocate_exact_size(
                        egui::vec2(p.stroke_hairline, 440.0),
                        egui::Sense::hover(),
                    );
                    ui.painter().rect_filled(r, 0.0, p.border_subtle);
                    ui.add_space(p.space_3);
                    ui.vertical(|ui| {
                        // A fixed column: the path field and the bar would widen the dialog.
                        ui.set_width(CONTENT_WIDTH);
                        ui.set_min_height(440.0);
                        match section {
                            Section::Companions => {
                                browse = self.companions(ui, p);
                            }
                            Section::Updates => action = self.updates(ui, p, status),
                            Section::Data => {
                                let (a, pick) = self.data(ui, p, data);
                                action = a;
                                if pick {
                                    browse = Some(DATA_PICKER);
                                }
                            }
                        }
                    });
                });
            },
            |ui| {
                save = widgets::primary(ui, p, None, "Save", true).clicked();
                cancel = widgets::secondary(ui, p, None, "Cancel", true).clicked();
                if section == Section::Companions {
                    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                        test = widgets::secondary(ui, p, None, "Test", true).clicked();
                    });
                }
            },
        );
        if test {
            self.test(ctx);
        }
        if let Some(i) = browse
            && self.picker.is_none()
        {
            let (tx, rx) = mpsc::channel();
            let c = ctx.clone();
            if i == DATA_PICKER {
                let start = data.folder.to_path_buf();
                std::thread::spawn(move || {
                    let f = rfd::FileDialog::new()
                        .set_title("Where should MapCraft keep its data?")
                        .set_directory(start)
                        .pick_folder();
                    let _ = tx.send(f);
                    c.request_repaint();
                });
            } else {
                let title = format!("Where is {}?", APPS[i].name());
                std::thread::spawn(move || {
                    let f = rfd::FileDialog::new()
                        .set_title(title)
                        .add_filter("App", &["exe"])
                        .pick_file();
                    let _ = tx.send(f);
                    c.request_repaint();
                });
            }
            self.picker = Some((i, rx));
        }
        if save {
            Some(SettingsAction::Save {
                photocraft: PathBuf::from(self.paths[0].trim()),
                lightcraft: PathBuf::from(self.paths[1].trim()),
                check_updates: self.check_updates,
            })
        } else if let Some(f) = picked_data {
            Some(SettingsAction::ChangeDataFolder(f))
        } else if action.is_some() {
            action
        } else if close || cancel {
            Some(SettingsAction::Close)
        } else {
            None
        }
    }

    /// The Companion apps section. Returns which app's Browse… was clicked.
    fn companions(&mut self, ui: &mut Ui, p: &Palette) -> Option<usize> {
        let mut browse = None;
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
        browse
    }

    /// The Data folder section: where it is, the space each kind takes, Change… and Free up
    /// space…. Returns an action, and whether the folder picker should open.
    fn data(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        d: &DataView<'_>,
    ) -> (Option<SettingsAction>, bool) {
        let mut action = None;
        let mut pick = false;
        if let Some((old, size)) = d.old {
            let text = match size {
                Some(b) => format!(
                    "The old data folder {} still holds {}.",
                    old.display(),
                    datafolder::bytes_text(b)
                ),
                None => format!(
                    "The old data folder {} still holds MapCraft’s data.",
                    old.display()
                ),
            };
            widgets::banner(ui, p, Tone::Info, &text);
            if widgets::secondary(ui, p, None, "Remove old data…", true).clicked() {
                action = Some(SettingsAction::RemoveOldData);
            }
            ui.add_space(p.space_3);
        }

        ui.label(RichText::new("Location").strong().color(p.text_strong));
        ui.horizontal(|ui| {
            let mut text = d.folder.display().to_string();
            let r = ui.add(
                TextEdit::singleline(&mut text)
                    .interactive(false)
                    .desired_width(ui.available_width() - 130.0),
            );
            ui.ctx()
                .accesskit_node_builder(r.id, |n| n.set_label("Data folder"));
            let can = d.blocked.is_none() && self.picker.is_none();
            pick = widgets::secondary(ui, p, None, "Change…", can).clicked();
            if widgets::ghost(ui, p, Icon::FolderOpen, "Show in Explorer", true).clicked() {
                action = Some(SettingsAction::ShowDataFolder);
            }
        });
        if let Some(why) = d.blocked {
            ui.label(RichText::new(why).small().color(p.text_weak));
        }
        ui.label(
            RichText::new(
                "The library, thumbnails, previews, map tiles and place names. MapCraft’s \
                 settings stay in AppData\\Roaming\\MapCraft, since they record where this \
                 folder is.",
            )
            .small()
            .color(p.text_weak),
        );
        ui.add_space(p.space_4);

        ui.horizontal(|ui| {
            ui.label(RichText::new("Space used").strong().color(p.text_strong));
            match d.usage {
                Some(u) => {
                    ui.label(
                        RichText::new(datafolder::bytes_text(u.total().bytes)).color(p.text_strong),
                    );
                }
                None => {
                    ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
                    ui.label(RichText::new("Counting…").small().color(p.text_weak));
                }
            }
        });
        if let Some(u) = d.usage {
            usage_bar(ui, p, u);
            ui.add_space(p.space_1);
            for k in Kind::ALL {
                let s = u.get(k);
                let note = match k {
                    Kind::Library => "photos and places on the map".to_owned(),
                    Kind::Places => "search index".to_owned(),
                    _ => format!("{} files", widgets::thousands(s.files as usize)),
                };
                // Fixed columns: dot, name, note, then the size at the right edge.
                ui.horizontal(|ui| {
                    ui.set_height(22.0);
                    let (r, _) =
                        ui.allocate_exact_size(egui::vec2(10.0, 10.0), egui::Sense::hover());
                    ui.painter().rect_filled(r, 3, kind_color(p, k));
                    let cell = Layout::left_to_right(Align::Center);
                    ui.allocate_ui_with_layout(egui::vec2(130.0, 20.0), cell, |ui| {
                        ui.set_min_width(130.0);
                        ui.label(RichText::new(k.label()).color(p.text_normal));
                    });
                    ui.allocate_ui_with_layout(egui::vec2(220.0, 20.0), cell, |ui| {
                        ui.set_min_width(220.0);
                        ui.label(RichText::new(note).small().color(p.text_weak));
                    });
                    ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                        ui.label(
                            RichText::new(datafolder::bytes_text(s.bytes)).color(p.text_strong),
                        );
                    });
                });
            }
        }
        ui.add_space(p.space_4);
        if widgets::secondary(ui, p, None, "Free up space…", d.usage.is_some()).clicked() {
            action = Some(SettingsAction::FreeSpace);
        }
        ui.label(
            RichText::new(
                "Deletes thumbnails, previews and map tiles. MapCraft makes them again when \
                 they’re needed.",
            )
            .small()
            .color(p.text_weak),
        );
        (action, pick)
    }

    /// The Updates section: the version, the checkbox, Check now and what the check found.
    fn updates(&mut self, ui: &mut Ui, p: &Palette, status: CheckStatus) -> Option<SettingsAction> {
        let mut action = None;
        ui.horizontal(|ui| {
            ui.label(RichText::new("Version").strong().color(p.text_strong));
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
                "MapCraft asks GitHub for its latest release. GitHub sees your IP address and \
                 that MapCraft runs; nothing about your photos is sent.",
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

/// The usage bar: each kind's share of the space, in its colour.
fn usage_bar(ui: &mut Ui, p: &Palette, u: &Usage) {
    let (rect, _) =
        ui.allocate_exact_size(egui::vec2(ui.available_width(), 10.0), egui::Sense::hover());
    ui.painter().rect_filled(rect, 5, p.bg_faint);
    let total = u.total().bytes.max(1) as f32;
    let mut x = rect.left();
    for k in Kind::ALL {
        let w = rect.width() * u.get(k).bytes as f32 / total;
        if w >= 1.0 {
            let r = egui::Rect::from_min_size(egui::pos2(x, rect.top()), egui::vec2(w, 10.0));
            ui.painter()
                .rect_filled(r.shrink2(egui::vec2(1.0, 0.0)), 2, kind_color(p, k));
        }
        x += w;
    }
}

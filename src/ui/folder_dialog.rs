//! "Add photo folder" and "Change folders…" (canvas `MapCraft — Import`, artboard 1): a folder
//! field with Browse…, the tri-state folder tree, and the photo count to import.
//!
//! Everything that touches the disk runs off the UI thread: the native picker (`rfd`) and the
//! tree's walk (`import::browse`). Closing the dialog drops the walk, which cancels it.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use egui::{Align, Layout, RichText, ScrollArea, Sense, TextEdit, Ui};

use crate::import::browse::{Tree, Walk};
use crate::import::scope::{Check, Scope};
use crate::library::{Source, key};
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, thousands};

#[derive(Debug, Clone, PartialEq)]
pub enum Mode {
    Add,
    Change { source_id: i64, label: String },
}

#[derive(Debug, Clone, PartialEq)]
pub enum FolderAction {
    /// Add `root` as a new source with `scope` and import it.
    Import {
        root: PathBuf,
        scope: Scope,
    },
    /// Store a changed scope for an existing source.
    Apply {
        source_id: i64,
        scope: Scope,
    },
    Close,
}

pub struct FolderDialog {
    pub mode: Mode,
    path_text: String,
    root: Option<PathBuf>,
    walk: Option<Walk>,
    tree: Tree,
    picker: Option<Receiver<Option<PathBuf>>>,
}

impl FolderDialog {
    pub fn add() -> Self {
        Self {
            mode: Mode::Add,
            path_text: String::new(),
            root: None,
            walk: None,
            tree: Tree::new(Scope::all()),
            picker: None,
        }
    }

    /// "Change folders…" for `source`, starting from its current decisions.
    pub fn change(source: &Source, root: PathBuf, ctx: &egui::Context) -> Self {
        let mut d = Self {
            mode: Mode::Change {
                source_id: source.id,
                label: source.label.clone(),
            },
            path_text: source.root_path.to_string_lossy().into_owned(),
            root: None,
            walk: None,
            tree: Tree::new(source.scope.clone()),
            picker: None,
        };
        d.browse(root, ctx);
        d
    }

    /// Chooses `folder` as if picked with Browse… (also the dev flag `--open add-folder=<path>`).
    pub fn choose(&mut self, folder: PathBuf, ctx: &egui::Context) {
        self.path_text = folder.to_string_lossy().into_owned();
        self.browse(folder, ctx);
    }

    fn browse(&mut self, root: PathBuf, ctx: &egui::Context) {
        let scope = self.tree.scope.clone();
        self.tree = Tree::new(scope);
        let c = ctx.clone();
        self.walk = Some(Walk::start(root.clone(), move || c.request_repaint()));
        self.root = Some(root);
    }

    fn pick(&mut self, ctx: &egui::Context) {
        let (tx, rx) = mpsc::channel();
        let c = ctx.clone();
        std::thread::spawn(move || {
            let folder = rfd::FileDialog::new()
                .set_title("Choose a photo folder")
                .pick_folder();
            let _ = tx.send(folder);
            c.request_repaint();
        });
        self.picker = Some(rx);
    }

    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        sources: &[Source],
    ) -> Option<FolderAction> {
        if let Some(rx) = &self.picker
            && let Ok(choice) = rx.try_recv()
        {
            self.picker = None;
            if let Some(folder) = choice {
                self.path_text = folder.to_string_lossy().into_owned();
                self.browse(folder, ctx);
            }
        }
        if let Some(w) = &self.walk {
            for e in w.poll() {
                self.tree.apply(e);
            }
        }

        let overlap = match &self.mode {
            Mode::Add => self
                .root
                .as_deref()
                .and_then(|r| overlapping_source(r, sources)),
            Mode::Change { .. } => None,
        };
        let (photos, folders) = self.tree.selected();
        let title = match &self.mode {
            Mode::Add => "Add photo folder".to_owned(),
            Mode::Change { label, .. } => format!("Folders of “{label}”"),
        };
        let ready = self.root.is_some() && !self.tree.root_unreadable() && overlap.is_none();
        let (text, go) = match &self.mode {
            Mode::Add => (format!("Import {} photos", thousands(photos)), photos > 0),
            Mode::Change { .. } => ("Apply".to_owned(), true),
        };
        // The body borrows `self` mutably; the button row only sets these flags (two closures
        // can't both hold `&mut self`), and the action is built after the dialog has drawn.
        let (mut confirm, mut cancel) = (false, false);
        let close = widgets::dialog(
            ctx,
            p,
            "folder-dialog",
            &title,
            600.0,
            true,
            |ui| self.body(ui, p, ctx, overlap.as_deref(), photos, folders),
            |ui| {
                confirm = widgets::primary(ui, p, None, &text, ready && go).clicked();
                cancel = widgets::secondary(ui, p, None, "Cancel", true).clicked();
            },
        );
        if confirm {
            let scope = self.tree.scope.clone();
            return Some(match &self.mode {
                Mode::Add => FolderAction::Import {
                    root: self.root.clone()?,
                    scope,
                },
                Mode::Change { source_id, .. } => FolderAction::Apply {
                    source_id: *source_id,
                    scope,
                },
            });
        }
        (cancel || close).then_some(FolderAction::Close)
    }

    fn body(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        ctx: &egui::Context,
        overlap: Option<&str>,
        photos: usize,
        folders: usize,
    ) {
        widgets::label(ui, p, "Folder");
        ui.horizontal(|ui| {
            let editable = self.mode == Mode::Add;
            let field = ui.add_enabled(
                editable,
                TextEdit::singleline(&mut self.path_text)
                    .font(egui::TextStyle::Monospace)
                    .hint_text(r"e.g. Z:\Photos")
                    .desired_width(470.0),
            );
            if editable && field.lost_focus() && ui.input(|i| i.key_pressed(egui::Key::Enter)) {
                let path = PathBuf::from(self.path_text.trim());
                self.browse(path, ctx);
            }
            if editable
                && widgets::secondary(ui, p, None, "Browse…", self.picker.is_none()).clicked()
            {
                self.pick(ctx);
            }
        });
        let note = if let Some(label) = overlap {
            Some((
                format!("This folder is already in the library, as part of “{label}”."),
                p.status_error,
            ))
        } else if self.tree.root_unreadable() {
            Some((
                "This folder can't be read. Check the path, or whether the drive is connected."
                    .into(),
                p.status_error,
            ))
        } else if self
            .root
            .as_deref()
            .is_some_and(|r| crate::paths::unc_target(r).is_some())
        {
            Some(("On a network drive; MapCraft also remembers its network path, in case the drive letter changes.".into(), p.text_weak))
        } else {
            None
        };
        if let Some((text, color)) = note {
            ui.label(RichText::new(text).small().color(color));
        }

        ui.add_space(p.space_2);
        widgets::label(ui, p, "Folders to import");
        egui::Frame::NONE
            .fill(p.bg_extreme)
            .stroke(egui::Stroke::new(1.0, p.border_normal))
            .corner_radius(p.radius_sm)
            .inner_margin(4)
            .show(ui, |ui| {
                ScrollArea::vertical()
                    .max_height(300.0)
                    .min_scrolled_height(300.0)
                    .auto_shrink([false, false])
                    .show(ui, |ui| self.rows(ui, p));
            });
        let counting = self.root.is_some() && !self.tree.done;
        let mut help = format!(
            "{} photos in {} folders selected",
            thousands(photos),
            thousands(folders)
        );
        if counting {
            help.push_str(", more while counting");
        }
        help.push_str(" · Your files are only read, never changed.");
        ui.label(RichText::new(help).small().color(p.text_weak));
    }

    /// The visible rows: depth-first through expanded folders.
    fn rows(&mut self, ui: &mut Ui, p: &Palette) {
        if self.root.is_none() {
            ui.label(RichText::new("Choose a folder to see what's in it.").color(p.text_weak));
            return;
        }
        let mut visible = Vec::new();
        visible_rows(&self.tree, "", 0, &mut visible);
        let mut toggle = None;
        let mut expand = None;
        for (k, depth) in visible {
            let Some(f) = self.tree.folders.get(&k) else {
                continue;
            };
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 6.0;
                ui.add_space(depth as f32 * p.indent);
                let has_children = !f.children.is_empty() && !f.never;
                let open = self.tree.expanded.contains(&k);
                let (rect, resp) = ui.allocate_exact_size(egui::vec2(14.0, 14.0), Sense::click());
                if has_children {
                    let icon = if open {
                        Icon::ChevronDown
                    } else {
                        Icon::ChevronRight
                    };
                    icon.image(14.0, p.text_weak).paint_at(ui, rect);
                    if resp.clicked() {
                        expand = Some(k.clone());
                    }
                }
                let enabled = !f.never && !f.unreadable;
                let state = if enabled {
                    self.tree.check(&k)
                } else {
                    Check::Off
                };
                if widgets::tri_check(ui, p, state, enabled).clicked() && enabled {
                    toggle = Some(k.clone());
                }
                ui.add(Icon::Folder.image(
                    p.size_icon_sm,
                    if enabled {
                        p.text_weak
                    } else {
                        p.text_disabled
                    },
                ));
                let name = if k.is_empty() {
                    self.root
                        .as_deref()
                        .map_or(String::new(), |r| r.to_string_lossy().into_owned())
                } else {
                    f.name.clone()
                };
                let ink = if enabled {
                    p.text_normal
                } else {
                    p.text_disabled
                };
                // The count is right-aligned; the name gets the rest and is truncated with "…",
                // so a long path never widens the dialog or runs into the count.
                let count = if f.never {
                    "never imported".to_owned()
                } else if f.unreadable {
                    "can't be read".to_owned()
                } else {
                    self.tree.total(&k).map_or("…".to_owned(), thousands)
                };
                let name_w = (ui.available_width() - 90.0).max(40.0);
                ui.allocate_ui(egui::vec2(name_w, 18.0), |ui| {
                    ui.add(egui::Label::new(RichText::new(name).color(ink)).truncate());
                });
                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                    ui.label(RichText::new(count).small().color(p.text_weak));
                });
            });
        }
        if let Some(k) = toggle {
            self.tree.toggle(&k);
        }
        if let Some(k) = expand
            && !self.tree.expanded.remove(&k)
        {
            self.tree.expanded.insert(k);
        }
    }
}

fn visible_rows(tree: &Tree, k: &str, depth: usize, out: &mut Vec<(String, usize)>) {
    let Some(f) = tree.folders.get(k) else {
        return;
    };
    out.push((k.to_owned(), depth));
    if tree.expanded.contains(k) {
        for c in &f.children {
            visible_rows(tree, c, depth + 1, out);
        }
    }
}

/// The label of an existing source that `root` equals, lies inside, or contains, comparing
/// case-insensitively and through UNC targets: adding it again would duplicate photos.
pub fn overlapping_source(root: &Path, sources: &[Source]) -> Option<String> {
    let forms = |p: &Path, unc: Option<String>| {
        let mut v = vec![norm(&p.to_string_lossy())];
        if let Some(u) = unc.or_else(|| crate::paths::unc_target(p)) {
            v.push(norm(&u));
        }
        v
    };
    let mine = forms(root, None);
    sources.iter().find_map(|s| {
        let theirs = forms(&s.root_path, s.unc_path.clone());
        let hit = mine.iter().any(|a| {
            theirs.iter().any(|b| {
                a == b || a.starts_with(&format!("{b}\\")) || b.starts_with(&format!("{a}\\"))
            })
        });
        hit.then(|| s.label.clone())
    })
}

/// Lower case, no trailing separator: `Z:\Photos\` → `z:\photos`.
fn norm(p: &str) -> String {
    key(p.trim_end_matches(['\\', '/']).replace('/', "\\").as_str())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(root: &str, unc: Option<&str>) -> Source {
        Source {
            id: 1,
            root_path: PathBuf::from(root),
            unc_path: unc.map(str::to_owned),
            label: "Photos".into(),
            last_scan_at: None,
            scope: Scope::all(),
        }
    }

    #[test]
    fn the_same_or_nested_folder_is_refused() {
        let s = [source(r"Z:\Photos", Some(r"\\nas\share\Photos"))];
        assert!(
            overlapping_source(Path::new(r"z:\photos\"), &s).is_some(),
            "same, other case"
        );
        assert!(
            overlapping_source(Path::new(r"Z:\Photos\2023"), &s).is_some(),
            "inside"
        );
        assert!(
            overlapping_source(Path::new(r"Z:\"), &s).is_some(),
            "around it"
        );
        assert!(
            overlapping_source(Path::new(r"\\NAS\share\Photos\x"), &s).is_some(),
            "via UNC"
        );
        assert!(
            overlapping_source(Path::new(r"Z:\Photos2"), &s).is_none(),
            "a sibling"
        );
        assert!(overlapping_source(Path::new(r"C:\Pictures"), &s).is_none());
    }
}

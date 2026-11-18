//! Removal mode in the app (design.md §2.9(n), canvas `MapCraft — Remove position`): starting
//! it from the detail panel, keeping the map in step, the confirmation, removing on a background
//! thread (with each changed photo read again as a rescan would), and Undo.

use std::collections::{HashMap, HashSet};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc;

use tracing::{error, info};

use super::MapCraftApp;
use super::geotag::{GeoEvent, GeoWork};
use crate::geotag::journal::{self, Kind, Prior, Report, Task};
use crate::geotag::removal::{Removal, Scope};
use crate::import::{position_of, reachable_root};
use crate::library::{GpsRow, PlacedIn, PlacedPhoto};
use crate::metadata::Gps;
use crate::ui::map_view::{MapAction, TagLayer};
use crate::ui::remove_panel::RemoveAction;
use crate::ui::toasts::{self, ToastAction};
use crate::ui::widgets::{self, Tone, thousands};

/// What a removed photo reads as afterwards: its position, where from, and its sidecar's name
/// (`None` once deleted). `Err`: it couldn't be read (then it has none until a rescan).
pub type Reread = Result<(Option<Gps>, Option<&'static str>), String>;

impl MapCraftApp {
    /// Why removal mode can't start now, if it can't (the detail panel's hover text).
    pub(super) fn removal_blocked(&self) -> Option<&'static str> {
        if self.lib.is_none() {
            Some("The library isn’t open")
        } else if self.job.is_some() || !self.waiting.is_empty() {
            Some("An import is running. Remove positions when it’s done.")
        } else if self.geo_work.is_some() {
            Some("Positions are being written")
        } else {
            None
        }
    }

    /// Removal mode from this photo: its folder's photos, the photo ticked (design.md §2.9(n)).
    pub fn start_removal(&mut self, around: i64) {
        if self.removal_blocked().is_some() || self.mode_open() {
            return;
        }
        let Some(lib) = &self.lib else { return };
        let photos = match lib.placed(PlacedIn::FolderOf(around)) {
            Ok(p) => p,
            Err(e) => {
                error!("can't read the folder's photos: {e}");
                return;
            }
        };
        let folder = photos
            .iter()
            .find(|p| p.id == around)
            .map(|p| folder_of(p).display().to_string())
            .unwrap_or_default();
        let r = Removal::new(around, folder, photos);
        info!(
            "removal mode: {} photos in the folder, {} removable",
            r.photos.len(),
            r.removable()
        );
        self.removal = Some(r);
        self.select(None);
        self.filters.open = false;
        self.search.close();
        self.map.set_tag(Some(TagLayer::default()));
        self.removal_background();
        self.removal_sync();
        self.map.select(Some(around));
        self.zoom_to_scope();
    }

    /// The scope's photos in view, with room above the top ones for their markers (a marker
    /// stands above its spot).
    fn zoom_to_scope(&mut self) {
        if let Some(mut b) = self.map.all_bounds() {
            b.max_lat += (b.max_lat - b.min_lat) * 0.2;
            self.map.zoom_to(b);
        }
    }

    /// The library's other photos as dim dots under the scope's.
    fn removal_background(&mut self) {
        let (Some(lib), Some(r)) = (&self.lib, &self.removal) else {
            return;
        };
        let mine: HashSet<i64> = r.photos.iter().map(|p| p.id).collect();
        let background = lib
            .positions()
            .map(|p| {
                p.iter()
                    .filter(|p| !mine.contains(&p.id))
                    .map(|p| (p.lat, p.lon))
                    .collect()
            })
            .unwrap_or_default();
        if let Some(tag) = self.map.tag_mut() {
            tag.background = background;
        }
    }

    /// The map follows the session: its photos, the ticks, the photos that can't be ticked.
    pub(super) fn removal_sync(&mut self) {
        let Some(r) = &self.removal else { return };
        if let Some(tag) = self.map.tag_mut() {
            tag.off = r.off();
            tag.locked = r.locked();
        }
        let points = r.points();
        self.map.set_points(points);
        if let Some(r) = &mut self.removal {
            r.selected = self.map.selected();
        }
    }

    /// Reads the scope's photos again (a refresh, the scope changed), keeping the ticks.
    pub(super) fn removal_reload(&mut self) {
        let (Some(lib), Some(r)) = (&self.lib, &mut self.removal) else {
            return;
        };
        let scope = match r.scope {
            Scope::Folder => PlacedIn::FolderOf(r.around),
            Scope::Map => PlacedIn::Shown(&self.filter),
        };
        match lib.placed(scope) {
            Ok(photos) => r.set_photos(photos),
            Err(e) => error!("can't read the photos: {e}"),
        }
        self.removal_sync();
    }

    pub(super) fn leave_removal(&mut self) {
        self.removal = None;
        self.rm_confirm = false;
        self.rm_leave = false;
        self.map.set_tag(None);
        self.map.select(None);
        self.refresh();
    }

    /// Esc in removal mode: the strip, then the selection, then leaving.
    pub(super) fn removal_escape(&mut self) {
        let Some(r) = &mut self.removal else { return };
        if self.map.close_strip() {
            return;
        }
        if r.selected.is_some() {
            r.selected = None;
            self.map.select(None);
        } else if r.changed {
            self.rm_leave = true;
        } else {
            self.leave_removal();
        }
    }

    pub(super) fn removal_action(&mut self, a: RemoveAction) {
        let Some(r) = &mut self.removal else { return };
        match a {
            RemoveAction::Leave => {
                if r.changed {
                    self.rm_leave = true;
                } else {
                    self.leave_removal();
                }
                return;
            }
            RemoveAction::Scope(s) => {
                r.scope = s;
                self.removal_reload();
                self.removal_background();
                self.zoom_to_scope();
                return;
            }
            RemoveAction::TickAll => r.tick_all(),
            RemoveAction::UntickAll => r.untick_all(),
            RemoveAction::Remove => {
                self.rm_confirm = r.ticked() > 0 && self.geo_work.is_none();
            }
            RemoveAction::CancelWrite => {
                if let Some(w) = &self.geo_work {
                    w.cancel.store(true, Ordering::Relaxed);
                }
            }
            RemoveAction::UndoLast => self.undo_journal(Kind::Remove),
        }
        self.removal_sync();
    }

    /// The map's actions in removal mode: select a photo, flip ticks.
    pub(super) fn removal_map(&mut self, a: MapAction) {
        let Some(r) = &mut self.removal else { return };
        match a {
            MapAction::Select(id) => {
                r.selected = id;
                self.map.select(id);
            }
            MapAction::Toggle(ids) => r.toggle(&ids),
            MapAction::Picked(..)
            | MapAction::AddFolder
            | MapAction::Style(_)
            | MapAction::MapSettings => {}
        }
        self.removal_sync();
    }

    /// The confirmation before removing (canvas artboard 3).
    pub(super) fn removal_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.rm_confirm {
            return;
        }
        let Some(r) = &self.removal else {
            self.rm_confirm = false;
            return;
        };
        let p = self.palette.clone();
        let todo = r.removals();
        let n = todo.len();
        let mut folders: Vec<String> = todo
            .iter()
            .map(|g| folder_of(g).display().to_string())
            .collect();
        folders.sort();
        folders.dedup();
        let title = if n == 1 {
            "Remove the position from 1 photo?".to_owned()
        } else {
            format!("Remove positions from {} photos?", thousands(n))
        };
        let (mut go, mut back) = (false, false);
        let closed = widgets::dialog(
            ctx,
            &p,
            "remove-confirm",
            &title,
            460.0,
            true,
            |ui| {
                ui.label(
                    "MapCraft takes the GPS position out of each photo’s XMP sidecar. The \
                     photos themselves aren’t changed.",
                );
                let row = |ui: &mut egui::Ui, k: &str, v: String| {
                    ui.horizontal(|ui| {
                        ui.add_sized(
                            [120.0, 18.0],
                            egui::Label::new(egui::RichText::new(k).small().color(p.text_weak)),
                        );
                        ui.add(egui::Label::new(v).truncate());
                    });
                };
                row(
                    ui,
                    "Sidecars",
                    format!(
                        "{} change: only their GPS goes, the rest is kept",
                        thousands(n)
                    ),
                );
                let f = match folders.as_slice() {
                    [one] => one.clone(),
                    [first, ..] => format!("{first} and {} more", folders.len() - 1),
                    [] => String::new(),
                };
                row(ui, "Folder", f);
                ui.add_space(p.space_1);
                widgets::banner(
                    ui,
                    &p,
                    Tone::Info,
                    "A photo whose own file also holds a position shows that one instead. \
                     Lightroom Classic’s catalogue may still hold the old position, and \
                     LightCraft may write it back if it writes its own sidecars. Undo stays \
                     available from the result.",
                );
            },
            |ui| {
                go = widgets::danger(ui, &p, "Remove positions", n > 0).clicked();
                back = widgets::secondary(ui, &p, None, "Back", true).clicked();
            },
        );
        if closed || back {
            self.rm_confirm = false;
        } else if go {
            self.rm_confirm = false;
            self.start_remove();
        }
    }

    /// Asks before leaving with ticks changed.
    pub(super) fn removal_leave_dialog(&mut self, ctx: &egui::Context) {
        if !self.rm_leave {
            return;
        }
        let p = self.palette.clone();
        let (mut leave, mut stay) = (false, false);
        let closed = widgets::dialog(
            ctx,
            &p,
            "remove-leave",
            "Leave removal mode?",
            380.0,
            true,
            |ui| {
                ui.label("The ticks you set are lost. Nothing was changed.");
            },
            |ui| {
                leave = widgets::primary(ui, &p, None, "Leave", true).clicked();
                stay = widgets::secondary(ui, &p, None, "Stay", true).clicked();
            },
        );
        if leave {
            self.leave_removal();
        } else if closed || stay {
            self.rm_leave = false;
        }
    }

    /// Removes the ticked photos' GPS on a background thread, then reads each changed photo
    /// again as a rescan would (design.md §2.9(n)).
    fn start_remove(&mut self) {
        if self.geo_work.is_some() {
            return; // an undo still runs
        }
        let Some(r) = &self.removal else { return };
        let todo = r.removals();
        if todo.is_empty() {
            return;
        }
        info!("removing the positions of {} photos", todo.len());
        let data = self.data_folder.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("mapcraft-remove".into())
            .spawn(move || {
                let mut roots: HashMap<(PathBuf, Option<String>), Option<PathBuf>> = HashMap::new();
                let mut tasks = Vec::new();
                let mut dirs: HashMap<i64, (PathBuf, PlacedPhoto)> = HashMap::new();
                let mut failed = Vec::new();
                for g in todo {
                    let base = roots
                        .entry((g.root.clone(), g.unc.clone()))
                        .or_insert_with(|| reachable_root(&g.root, g.unc.as_deref()))
                        .clone();
                    let Some(base) = base else {
                        failed.push((g.id, "its folder can’t be reached".to_owned()));
                        continue;
                    };
                    // Only the sidecar the scan found: never a guessed name.
                    let Some(name) = g.sidecar_name.clone() else {
                        failed.push((g.id, "it has no sidecar".to_owned()));
                        continue;
                    };
                    let dir = base.join(&g.rel_dir);
                    tasks.push(Task {
                        photo: g.id,
                        path: dir.join(&name),
                        gps: None,
                        prior: Prior {
                            lat: Some(g.lat),
                            lon: Some(g.lon),
                            alt: g.alt,
                            gps_source: g.gps_source.clone(),
                            sidecar_name: g.sidecar_name.clone(),
                        },
                    });
                    dirs.insert(g.id, (dir, g));
                }
                let progress = |done, total| {
                    let _ = tx.send(GeoEvent::Progress(done, total));
                    ctx.request_repaint();
                };
                let mut report = journal::run(Kind::Remove, &tasks, &data, &progress, &flag);
                report.failed.extend(failed);
                // Each changed photo (and one whose sidecar had no position after all) as a
                // rescan will read it.
                let changed = report
                    .written
                    .iter()
                    .map(|w| (w.photo, w.deleted))
                    .chain(report.untouched.iter().map(|&id| (id, false)));
                let reread: Vec<(i64, Option<String>, Reread)> = changed
                    .filter_map(|(id, deleted)| {
                        let (dir, g) = dirs.get(&id)?;
                        let sidecar = g.sidecar_name.clone().filter(|_| !deleted);
                        let r = position_of(
                            dir,
                            &g.original_name,
                            g.preview_name.as_deref(),
                            sidecar.as_deref(),
                        );
                        Some((id, sidecar, r))
                    })
                    .collect();
                let _ = tx.send(GeoEvent::Removed(report, reread));
                ctx.request_repaint();
            })
            .expect("spawn the removal thread");
        self.geo_work = Some(GeoWork {
            rx,
            cancel,
            progress: Some((0, 0)),
            undo: false,
            kind: Kind::Remove,
        });
    }

    /// The removal finished: the library records what each photo reads as now, and the map
    /// returns.
    pub(super) fn removed(&mut self, r: Report, reread: Vec<(i64, Option<String>, Reread)>) {
        let mut from_file = 0;
        let mut rows = Vec::new();
        for (id, sidecar, read) in reread {
            let (gps, src) = match read {
                Ok(v) => v,
                Err(e) => {
                    // The sidecar's position is gone for certain; a rescan reads the rest.
                    error!("can't read a photo again after removing its position: {e}");
                    (None, None)
                }
            };
            from_file += usize::from(src == Some("exif"));
            rows.push(GpsRow {
                id,
                pos: gps.map(|g| (g.lat, g.lon)),
                alt: gps.and_then(|g| g.alt),
                gps_source: src.map(str::to_owned),
                sidecar_name: sidecar,
            });
        }
        if let Some(lib) = &mut self.lib
            && let Err(e) = lib.set_gps_all(&rows)
        {
            error!("can't record the removed positions: {e}");
        }
        let (n, failed) = (r.written.len(), r.failed.len());
        let deleted = r.written.iter().filter(|w| w.deleted).count();
        info!(
            "positions removed: {n} ({deleted} sidecars deleted), {} had none, {failed} failed{}",
            r.untouched.len(),
            if r.cancelled { ", cancelled" } else { "" }
        );
        let total = n + failed + r.untouched.len();
        let title = if failed == 0 && !r.cancelled && r.untouched.is_empty() {
            format!(
                "Positions removed from {} photo{}",
                thousands(n),
                if n == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "Positions removed from {} of {} photos",
                thousands(n),
                thousands(total)
            )
        };
        let plural = |k: usize, one: &str, many: &str| {
            format!("{} {}", thousands(k), if k == 1 { one } else { many })
        };
        let mut parts = Vec::new();
        if n > deleted {
            parts.push(format!(
                "{} changed",
                plural(n - deleted, "sidecar", "sidecars")
            ));
        }
        if deleted > 0 {
            parts.push(format!(
                "{} deleted: {} nothing else",
                plural(deleted, "sidecar", "sidecars"),
                if deleted == 1 { "it held" } else { "they held" }
            ));
        }
        let mut text = if parts.is_empty() {
            String::new()
        } else {
            format!("{}.", parts.join(", "))
        };
        if from_file > 0 {
            text.push_str(&format!(
                " {} now {} the position saved in {} own file.",
                plural(from_file, "photo", "photos"),
                if from_file == 1 { "shows" } else { "show" },
                if from_file == 1 { "its" } else { "their" }
            ));
        }
        if !r.untouched.is_empty() {
            text.push_str(&format!(
                " {} no position in {} sidecar any more.",
                plural(r.untouched.len(), "photo had", "photos had"),
                if r.untouched.len() == 1 {
                    "its"
                } else {
                    "their"
                }
            ));
        }
        if let Some((_, why)) = r.failed.first() {
            text.push_str(&format!(
                " {} couldn’t be changed: {why}{}.",
                thousands(failed),
                if failed > 1 { " (the first)" } else { "" }
            ));
        }
        if r.cancelled {
            text.push_str(" Stopped before the rest.");
        }
        if n > 0 {
            text.push_str(" Undo restores the sidecars as they were.");
        }
        self.rm_last = journal::latest(&self.data_folder, Kind::Remove);
        let tone = if failed == 0 {
            Tone::Success
        } else {
            Tone::Warning
        };
        let action = (n > 0).then_some(("Undo", ToastAction::UndoRemoval));
        self.toasts
            .replace(vec![toasts::note(tone, &title, text.trim(), action)]);
        self.leave_removal();
    }
}

/// A photo's folder as the library knows it.
fn folder_of(p: &PlacedPhoto) -> PathBuf {
    if p.rel_dir.is_empty() {
        p.root.clone()
    } else {
        p.root.join(&p.rel_dir)
    }
}

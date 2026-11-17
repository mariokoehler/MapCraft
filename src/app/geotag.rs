//! Tagging mode in the app (design.md §2.9): starting a session from track files, keeping the map
//! in step with it, the confirmation, writing on a background thread, and Undo.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use tracing::{error, info};

use super::MapCraftApp;
use crate::geotag::journal::{self, Prior, Report, Task, Undone};
use crate::geotag::session::Session;
use crate::geotag::{self, Track, gpx, time};
use crate::import::reachable_root;
use crate::ui::geotag_panel::GeoAction;
use crate::ui::map_view::TagLayer;
use crate::ui::toasts::{self, ToastAction};
use crate::ui::widgets::{self, Tone, thousands};

/// What the geotag thread reports.
pub enum GeoEvent {
    Progress(usize, usize),
    Written(Report),
    Undone(Result<Undone, String>),
}

/// A write (or an undo) running in the background.
pub struct GeoWork {
    pub rx: Receiver<GeoEvent>,
    pub cancel: Arc<AtomicBool>,
    pub progress: Option<(usize, usize)>,
    /// An undo, not a write (the panel shows no progress for it).
    pub undo: bool,
}

impl MapCraftApp {
    /// The track file picker: GPX and NMEA logs (none in tests: they start sessions directly).
    pub(super) fn pick_gpx(&self) -> Option<Vec<PathBuf>> {
        if !self.launch {
            return None;
        }
        rfd::FileDialog::new()
            .set_title("Tag photos from a GPS track")
            .add_filter("GPS tracks (GPX, NMEA)", &["gpx", "nmea", "log"])
            .add_filter("All files", &["*"])
            .pick_files()
    }

    /// Why tagging can't start now, if it can't.
    pub(super) fn geotag_blocked(&self) -> Option<&'static str> {
        if self.lib.is_none() {
            Some("The library isn’t open")
        } else if self.job.is_some() || !self.waiting.is_empty() {
            Some("An import is running. Tag photos when it’s done.")
        } else if self.geo_work.is_some() {
            Some("Positions are being written")
        } else {
            None
        }
    }

    /// Reads track files (GPX or NMEA) into a track; a file that can't be read says so in a toast.
    fn read_tracks(&mut self, files: &[PathBuf]) -> Vec<(gpx::Gpx, String)> {
        let mut out = Vec::new();
        for f in files {
            let name = f
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let stem = f
                .file_stem()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let read = std::fs::read(f)
                .map_err(|e| e.to_string())
                .and_then(|b| geotag::read_track(&b));
            match read {
                Ok(g) => out.push((g, stem)),
                Err(e) => {
                    error!("can't read a track file: {e}");
                    self.toasts.push(toasts::note(
                        Tone::Error,
                        "Can’t read the track file",
                        &format!("{name}: {e}"),
                        None,
                    ));
                }
            }
        }
        out
    }

    /// Starts tagging mode with these track files, or (with a session open) adds them to its
    /// track. Ticks and the shift stay when files are added.
    pub fn start_geotag(&mut self, files: Vec<PathBuf>) {
        if self.geotag_blocked().is_some() && self.geotag.is_none() {
            return;
        }
        let mut all = self
            .geotag
            .as_ref()
            .map(|s| s.files.clone())
            .unwrap_or_default();
        for f in files {
            if !all.contains(&f) {
                all.push(f);
            }
        }
        let parsed = self.read_tracks(&all);
        if parsed.is_empty() {
            return;
        }
        let track = Track::new(parsed);
        info!(
            "geotagging: {} file(s), {} points, {} timed segments",
            track.files,
            track.points,
            track.segments.len()
        );
        let Some(lib) = &self.lib else { return };
        let retag = self.geotag.as_ref().is_some_and(|s| s.retag);
        let photos = match Session::pool(&track) {
            Some((from, to)) => lib
                .geotag_candidates(&from, &to, retag)
                .unwrap_or_else(|e| {
                    error!("can't read the candidates: {e}");
                    Vec::new()
                }),
            None => Vec::new(),
        };
        let undated = lib.undated_without_position().unwrap_or(0);
        let background: Vec<(f64, f64)> = lib
            .positions()
            .map(|p| p.iter().map(|p| (p.lat, p.lon)).collect())
            .unwrap_or_default();
        let mut session = Session::new(all, track, photos, undated);
        let first = self.geotag.is_none();
        if let Some(old) = self.geotag.take() {
            session.shift = old.shift;
            session.max_gap_min = old.max_gap_min;
            session.margin_h = old.margin_h;
            session.retag = old.retag;
            session.write_alt = old.write_alt;
            session.off = old.off;
            session.changed = true;
            session.rematch();
        }
        self.geotag = Some(session);
        // The map turns to the track: no selection, panel, filters panel or search.
        self.select(None);
        self.filters.open = false;
        self.search.close();
        self.map.set_tag(Some(TagLayer {
            background,
            ..Default::default()
        }));
        self.geotag_sync();
        if first && let Some(b) = self.map.all_bounds() {
            self.map.zoom_to(b);
        }
        self.thumbs_for_candidates();
    }

    /// Thumbnails for the candidates that have none, before the library's (design.md §2.9(g)).
    fn thumbs_for_candidates(&mut self) {
        let Some(s) = &self.geotag else { return };
        // Those on the track or near it: the pool reaches a day either way, mostly unseen.
        let near: std::collections::HashSet<i64> = s
            .matched
            .on
            .iter()
            .map(|o| o.0)
            .chain(s.matched.outside.iter().map(|o| o.0))
            .collect();
        let want: Vec<i64> = s
            .photos
            .iter()
            .filter(|p| p.thumb == crate::library::Thumb::Pending && near.contains(&p.id))
            .map(|p| p.id)
            .filter(|id| !self.thumbs_for.contains(id))
            .collect();
        if want.is_empty() {
            return;
        }
        self.thumbs_for.extend(want);
        if let Some(t) = &self.thumbs {
            t.cancel(); // it restarts with the candidates first
            self.restart_thumbs = true;
        } else {
            self.start_thumbs();
        }
    }

    /// The map follows the session: its photos where they're matched, the track, the ticks.
    pub(super) fn geotag_sync(&mut self) {
        let Some(s) = &self.geotag else { return };
        if let Some(tag) = self.map.tag_mut() {
            tag.lines = s.track.lines.clone();
            tag.timed = s.track.has_times();
            tag.pauses = s
                .track
                .pauses(s.max_gap())
                .iter()
                .map(|p| (p.lat, p.lon))
                .collect();
            tag.off = s.off.clone();
            tag.picking = s.picking;
        }
        let points = s.points();
        if self
            .map
            .selected()
            .is_some_and(|id| !points.iter().any(|p| p.id == id))
        {
            self.map.select(None);
        }
        self.map.set_points(points);
        if let Some(s) = &mut self.geotag {
            s.selected = self.map.selected();
        }
        // Photos the shift brought onto the track get their thumbnails too.
        self.thumbs_for_candidates();
    }

    /// Re-reads the candidates (their thumbnails; the re-tag option), keeping the settings.
    pub(super) fn geotag_reload(&mut self) {
        let (Some(lib), Some(s)) = (&self.lib, &mut self.geotag) else {
            return;
        };
        if let Some((from, to)) = Session::pool(&s.track) {
            match lib.geotag_candidates(&from, &to, s.retag) {
                Ok(photos) => s.set_photos(photos),
                Err(e) => error!("can't read the candidates: {e}"),
            }
        }
        self.geotag_sync();
    }

    /// Leaves tagging mode; the map shows the library again.
    pub(super) fn leave_geotag(&mut self) {
        self.geotag = None;
        self.geo_confirm = false;
        self.geo_leave = false;
        self.thumbs_for.clear();
        self.map.set_tag(None);
        self.map.select(None);
        self.refresh();
    }

    /// Esc in tagging mode: picking, then the strip, then the selection, then leaving.
    pub(super) fn geotag_escape(&mut self) {
        let Some(s) = &mut self.geotag else { return };
        if s.picking {
            s.picking = false;
        } else if self.map.close_strip() {
            return;
        } else if s.selected.is_some() {
            s.selected = None;
            self.map.select(None);
        } else if s.changed {
            self.geo_leave = true;
        } else {
            self.leave_geotag();
            return;
        }
        self.geotag_sync();
    }

    pub(super) fn geotag_action(&mut self, a: GeoAction) {
        if matches!(a, GeoAction::Leave) {
            if self.geotag.as_ref().is_some_and(|s| s.changed) {
                self.geo_leave = true;
            } else {
                self.leave_geotag();
            }
            return;
        }
        if matches!(a, GeoAction::AddFile) {
            if let Some(files) = self.pick_gpx() {
                self.start_geotag(files);
            }
            return;
        }
        if matches!(a, GeoAction::UndoLast) {
            self.undo_geotag();
            return;
        }
        if matches!(a, GeoAction::CancelWrite) {
            if let Some(w) = &self.geo_work {
                w.cancel.store(true, Ordering::Relaxed);
            }
            return;
        }
        let Some(s) = &mut self.geotag else { return };
        let mut reload = false;
        match a {
            GeoAction::Zone(z) => {
                let shift = crate::geotag::Shift {
                    zone: z.clamp(-12 * 3600, 14 * 3600),
                    ..s.shift.clone()
                };
                s.anchored = None;
                s.set_shift(shift);
            }
            GeoAction::Correction(c) => {
                let shift = crate::geotag::Shift {
                    correction: c,
                    ..s.shift.clone()
                };
                s.anchored = None;
                s.set_shift(shift);
            }
            GeoAction::IgnoreOwn(b) => {
                let shift = crate::geotag::Shift {
                    ignore_own: b,
                    ..s.shift.clone()
                };
                s.set_shift(shift);
            }
            GeoAction::StartPick => s.picking = s.selected.is_some(),
            GeoAction::CancelPick => s.picking = false,
            GeoAction::UndoAnchor => s.undo_anchor(),
            GeoAction::MaxGap(m) => {
                s.max_gap_min = m;
                s.changed = true;
                s.rematch();
            }
            GeoAction::Margin(h) => {
                s.margin_h = h;
                s.rematch();
            }
            GeoAction::Retag(b) => {
                s.retag = b;
                s.changed = true;
                reload = true;
            }
            GeoAction::WriteAlt(b) => {
                s.write_alt = b;
                s.changed = true;
            }
            GeoAction::Write => self.geo_confirm = s.ticked() > 0,
            GeoAction::Leave
            | GeoAction::AddFile
            | GeoAction::CancelWrite
            | GeoAction::UndoLast => {}
        }
        if reload {
            self.geotag_reload();
            self.thumbs_for_candidates();
        } else {
            self.geotag_sync();
        }
    }

    /// The map's tagging actions: select a photo, flip ticks, pick a spot.
    pub(super) fn geotag_map(&mut self, a: crate::ui::map_view::MapAction) {
        use crate::ui::map_view::MapAction;
        let Some(s) = &mut self.geotag else { return };
        match a {
            MapAction::Select(id) => {
                s.selected = id;
                self.map.select(id);
            }
            MapAction::Toggle(ids) => s.toggle(&ids),
            MapAction::Picked(lat, lon) => {
                s.anchor_at(lat, lon);
            }
            MapAction::AddFolder | MapAction::Style(_) | MapAction::MapSettings => {}
        }
        self.geotag_sync();
    }

    /// The time on the track under the pointer, as a tip beside it.
    pub(super) fn geotag_track_tip(&self, ctx: &egui::Context) {
        let (Some(s), Some(((lat, lon), pos))) = (&self.geotag, self.map.track_hover()) else {
            return;
        };
        let Some((t, _)) = s.track.time_near(lat, lon) else {
            return;
        };
        let p = &self.palette;
        let shift = s.shift.total() as f64;
        let text = if s.picking {
            format!("Here at {} UTC: click to set the shift", time::clock(t))
        } else {
            format!(
                "{} UTC · {} on the photos’ clocks",
                time::clock(t),
                time::clock(t + shift)
            )
        };
        egui::Area::new(egui::Id::unique("track-tip"))
            .order(egui::Order::Tooltip)
            .fixed_pos(pos + egui::vec2(14.0, 10.0))
            .interactable(false)
            .show(ctx, |ui| {
                egui::Frame::NONE
                    .fill(p.bg_window)
                    .stroke(egui::Stroke::new(p.stroke_hairline, p.border_subtle))
                    .corner_radius(p.radius_md)
                    .shadow(p.elevation_popup)
                    .inner_margin(egui::Margin::symmetric(8, 4))
                    .show(ui, |ui| {
                        ui.label(egui::RichText::new(text).small().color(p.text_normal));
                    });
            });
    }

    /// The confirmation before writing (canvas artboard 5).
    pub(super) fn geotag_confirm_dialog(&mut self, ctx: &egui::Context) {
        if !self.geo_confirm {
            return;
        }
        let Some(s) = &self.geotag else {
            self.geo_confirm = false;
            return;
        };
        let p = self.palette.clone();
        let writes = s.writes();
        let n = writes.len();
        let updated = writes
            .iter()
            .filter(|(g, _)| g.sidecar_name.is_some())
            .count();
        let mut folders: Vec<String> = writes
            .iter()
            .map(|(g, _)| g.root.join(&g.rel_dir).display().to_string())
            .collect();
        folders.sort();
        folders.dedup();
        let title = if n == 1 {
            "Write the position to 1 photo?".to_owned()
        } else {
            format!("Write positions to {} photos?", thousands(n))
        };
        let (mut write, mut back) = (false, false);
        let closed = widgets::dialog(
            ctx,
            &p,
            "geotag-confirm",
            &title,
            460.0,
            true,
            |ui| {
                ui.label(
                    "MapCraft adds the GPS position to each photo’s XMP sidecar. The photos \
                     themselves aren’t changed.",
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
                if n > updated {
                    row(ui, "New sidecars", thousands(n - updated));
                }
                if updated > 0 {
                    row(
                        ui,
                        "Updated sidecars",
                        format!(
                            "{}: only their GPS changes, the rest is kept",
                            thousands(updated)
                        ),
                    );
                }
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
                    "Lightroom Classic shows the new positions after Read Metadata from Files, \
                     and only for camera RAW files. LightCraft may overwrite the sidecars if it \
                     writes its own. Undo stays available from the result.",
                );
            },
            |ui| {
                write = widgets::primary(ui, &p, None, "Write positions", n > 0).clicked();
                back = widgets::secondary(ui, &p, None, "Back", true).clicked();
            },
        );
        if closed || back {
            self.geo_confirm = false;
        } else if write {
            self.geo_confirm = false;
            self.start_write();
        }
    }

    /// Asks before leaving with changes (ticks, the shift).
    pub(super) fn geotag_leave_dialog(&mut self, ctx: &egui::Context) {
        if !self.geo_leave {
            return;
        }
        let p = self.palette.clone();
        let (mut leave, mut stay) = (false, false);
        let closed = widgets::dialog(
            ctx,
            &p,
            "geotag-leave",
            "Leave tagging?",
            380.0,
            true,
            |ui| {
                ui.label("The ticks and the shift you set are lost. Nothing was written.");
            },
            |ui| {
                leave = widgets::primary(ui, &p, None, "Leave", true).clicked();
                stay = widgets::secondary(ui, &p, None, "Stay", true).clicked();
            },
        );
        if leave {
            self.leave_geotag();
        } else if closed || stay {
            self.geo_leave = false;
        }
    }

    /// Writes the ticked photos' positions on a background thread (design.md §2.9(i)).
    fn start_write(&mut self) {
        let Some(s) = &self.geotag else { return };
        let writes = s.writes();
        if writes.is_empty() {
            return;
        }
        info!("geotagging: writing {} sidecars", writes.len());
        let data = self.data_folder.clone();
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("mapcraft-geotag".into())
            .spawn(move || {
                // Each source's root once: a drive letter, else its UNC path.
                let mut roots: HashMap<(PathBuf, Option<String>), Option<PathBuf>> = HashMap::new();
                let mut tasks = Vec::new();
                let mut unreachable = Vec::new();
                for (g, gps) in writes {
                    let base = roots
                        .entry((g.root.clone(), g.unc.clone()))
                        .or_insert_with(|| reachable_root(&g.root, g.unc.as_deref()))
                        .clone();
                    let Some(base) = base else {
                        unreachable.push((g.id, "its folder can’t be reached".to_owned()));
                        continue;
                    };
                    let name = g
                        .sidecar_name
                        .clone()
                        .unwrap_or_else(|| format!("{}.xmp", g.stem));
                    tasks.push(Task {
                        photo: g.id,
                        path: base.join(&g.rel_dir).join(name),
                        gps,
                        prior: Prior {
                            lat: g.lat,
                            lon: g.lon,
                            alt: g.alt,
                            gps_source: g.gps_source.clone(),
                            sidecar_name: g.sidecar_name.clone(),
                        },
                    });
                }
                let progress = |done, total| {
                    let _ = tx.send(GeoEvent::Progress(done, total));
                    ctx.request_repaint();
                };
                let mut report = journal::run(&tasks, &data, &progress, &flag);
                report.failed.extend(unreachable);
                let _ = tx.send(GeoEvent::Written(report));
                ctx.request_repaint();
            })
            .expect("spawn the geotag thread");
        self.geo_work = Some(GeoWork {
            rx,
            cancel,
            progress: Some((0, 0)),
            undo: false,
        });
    }

    /// Undoes the last write (the result toast's Undo).
    pub(super) fn undo_geotag(&mut self) {
        if self.geo_work.is_some() || self.job.is_some() {
            return; // the toast's Undo stays for later
        }
        let Some((path, _)) = self.geo_last.take() else {
            return;
        };
        let (tx, rx) = mpsc::channel();
        let ctx = self.ctx.clone();
        std::thread::Builder::new()
            .name("mapcraft-geotag-undo".into())
            .spawn(move || {
                let _ = tx.send(GeoEvent::Undone(journal::undo(&path)));
                ctx.request_repaint();
            })
            .expect("spawn the geotag undo thread");
        self.geo_work = Some(GeoWork {
            rx,
            cancel: Arc::new(AtomicBool::new(false)),
            progress: None,
            undo: true,
        });
    }

    pub(super) fn poll_geotag(&mut self) {
        let Some(w) = &mut self.geo_work else { return };
        let mut done = None;
        while let Ok(e) = w.rx.try_recv() {
            match e {
                GeoEvent::Progress(d, t) => w.progress = Some((d, t)),
                other => done = Some(other),
            }
        }
        match done {
            Some(GeoEvent::Written(r)) => {
                self.geo_work = None;
                self.written(r);
            }
            Some(GeoEvent::Undone(r)) => {
                self.geo_work = None;
                self.undone(r);
            }
            _ => {}
        }
    }

    /// The write finished: the library records what was written, and the map returns.
    fn written(&mut self, r: Report) {
        if let Some(lib) = &self.lib {
            for w in &r.written {
                if let Err(e) = lib.set_gps(
                    w.photo,
                    Some((w.lat, w.lon)),
                    w.alt,
                    Some("xmp"),
                    Some(&w.sidecar_name),
                ) {
                    error!("can't record a written position: {e}");
                }
            }
        }
        let (n, failed) = (r.written.len(), r.failed.len());
        let created = r.written.iter().filter(|w| w.created).count();
        info!(
            "geotagging: {n} written ({created} created), {failed} failed{}",
            if r.cancelled { ", cancelled" } else { "" }
        );
        let total = n + failed;
        let title = if failed == 0 && !r.cancelled {
            format!(
                "Positions written to {} photo{}",
                thousands(n),
                if n == 1 { "" } else { "s" }
            )
        } else {
            format!(
                "Positions written to {} of {} photos",
                thousands(n),
                thousands(total)
            )
        };
        let mut parts = Vec::new();
        if created > 0 {
            parts.push(format!(
                "{} sidecar{} created",
                thousands(created),
                if created == 1 { "" } else { "s" }
            ));
        }
        if n > created {
            parts.push(format!("{} updated", thousands(n - created)));
        }
        let mut text = if parts.is_empty() {
            String::new()
        } else {
            format!("{}.", parts.join(", "))
        };
        if let Some((_, why)) = r.failed.first() {
            text.push_str(&format!(
                " {} couldn’t be written: {why}{}.",
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
        self.geo_last = journal::latest(&self.data_folder);
        let tone = if failed == 0 {
            Tone::Success
        } else {
            Tone::Warning
        };
        let action = (n > 0).then_some(("Undo", ToastAction::UndoGeotag));
        self.toasts
            .replace(vec![toasts::note(tone, &title, &text, action)]);
        self.leave_geotag();
    }

    fn undone(&mut self, r: Result<Undone, String>) {
        match r {
            Ok(u) => {
                if let Some(lib) = &self.lib {
                    for (id, prior) in &u.restored {
                        if let Err(e) = lib.set_gps(
                            *id,
                            prior.lat.zip(prior.lon),
                            prior.alt,
                            prior.gps_source.as_deref(),
                            prior.sidecar_name.as_deref(),
                        ) {
                            error!("can't restore a position: {e}");
                        }
                    }
                }
                info!(
                    "geotagging undone: {} restored, {} skipped",
                    u.restored.len(),
                    u.skipped.len()
                );
                let mut text = format!(
                    "{} sidecar{} restored.",
                    thousands(u.restored.len()),
                    if u.restored.len() == 1 { "" } else { "s" }
                );
                if let Some((_, why)) = u.skipped.first() {
                    text.push_str(&format!(
                        " {} left as {}: {why}.",
                        thousands(u.skipped.len()),
                        if u.skipped.len() == 1 {
                            "it is"
                        } else {
                            "they are"
                        }
                    ));
                }
                self.geo_last = journal::latest(&self.data_folder);
                self.toasts.replace(vec![toasts::note(
                    if u.skipped.is_empty() {
                        Tone::Success
                    } else {
                        Tone::Warning
                    },
                    "Geotagging undone",
                    &text,
                    None,
                )]);
            }
            Err(e) => {
                error!("can't undo geotagging: {e}");
                self.toasts.replace(vec![toasts::note(
                    Tone::Error,
                    "Can’t undo",
                    &format!("The journal can’t be read: {e}"),
                    None,
                )]);
            }
        }
        self.refresh();
    }
}

//! The main window: top bar above, the map filling the rest (design system "Map layout"), and
//! the import's dialogs and toasts (canvas `MapCraft — Import`).
//!
//! The UI thread only reads the library (its own connection, WAL mode) and never touches the
//! photo folders: imports, folder walks and online checks run on background threads.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Frame, Panel, RichText, Stroke};
use tracing::{error, info};

use crate::import::scope::Scope;
use crate::jobs::{ImportJob, JobEvent, ThumbEvent, ThumbJob};
use crate::launch::{self, Launch};
use crate::library::{Counts, Library, PhotoDetail, Source};
use crate::provider::Provider;
use crate::theme::{self, Palette, ThemeKind, Tokens};
use crate::ui::details::{self, DetailPanel, PanelAction, PreviewFace, Previews};
use crate::ui::folder_dialog::{FolderAction, FolderDialog};
use crate::ui::map_view::{MapAction, MapView};
use crate::ui::sources_dialog::{SourcesAction, SourcesDialog};
use crate::ui::toasts::{self, ToastAction, Toasts};
use crate::ui::top_bar::{self, Running, Task, TopBarAction};
use crate::ui::widgets::{self, thousands};

/// Parallel metadata readers per import (design.md §3.4, user-confirmed).
const READERS: usize = 4;
/// Parallel thumbnail renderers (design.md §2.2(b): 4 workers on the NAS).
const THUMB_WORKERS: usize = 4;
/// How often the map's dots refresh while an import writes.
const RELOAD_EVERY: Duration = Duration::from_secs(1);

/// A dialog to open at start (dev flag `--open`), for comparing screens with the mockups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartDialog {
    /// Optionally with a folder already chosen, so its tree shows.
    AddFolder(Option<PathBuf>),
    Sources,
    /// The first photo on the map selected, its detail panel open.
    Details,
}

pub struct Config {
    pub data_folder: PathBuf,
    /// Draw OSM tiles (off in tests: no network).
    pub tiles: bool,
    pub open: Option<StartDialog>,
    /// Log the average frame time every few seconds (dev flag `--perf`).
    pub perf: bool,
    /// Really start Explorer for Show in Explorer and Open (off in tests, which record them).
    pub launch: bool,
}

/// A change to the library that waits until the thumbnail job has stopped. The job writes
/// thumbnail states and files by photo id, so it must not run while photos are rewritten or
/// deleted: it could mark a stale thumbnail as made, or give a removed photo's id to a new one.
#[derive(Debug, Clone, PartialEq)]
enum Change {
    Import { source_id: i64, first: bool },
    Scope { source_id: i64, scope: Scope },
    Remove(i64),
}

/// Leaving folders out of a source removes photos, so it's confirmed first.
struct NarrowScope {
    source_id: i64,
    label: String,
    scope: Scope,
    removes: usize,
}

pub struct MapCraftApp {
    palette: Palette,
    map: MapView,
    db_path: PathBuf,
    data_folder: PathBuf,
    ctx: egui::Context,
    lib: Option<Library>,
    sources: Vec<Source>,
    source_counts: HashMap<i64, Counts>,
    counts: Counts,
    job: Option<ImportJob>,
    thumbs: Option<ThumbJob>,
    /// Changes waiting for the thumbnail job (or an import) to finish, in order.
    waiting: VecDeque<Change>,
    last_reload: Instant,
    folder_dialog: Option<FolderDialog>,
    sources_dialog: Option<SourcesDialog>,
    narrow: Option<NarrowScope>,
    toasts: Toasts,
    perf: Option<(Instant, Duration, u32)>,
    /// The selected photo's record, while one is selected.
    detail: Option<PhotoDetail>,
    panel_open: bool,
    panel: DetailPanel,
    previews: Previews,
    /// The hovered marker's record, kept while the pointer stays on it.
    hover: Option<PhotoDetail>,
    launch: bool,
    /// What Show in Explorer and Open asked for, when `launch` is off (for the tests).
    launched: Vec<(Launch, i64)>,
    launch_errors: (Sender<String>, Receiver<String>),
}

impl MapCraftApp {
    pub fn new(ctx: &egui::Context, config: Config) -> Self {
        // The tokens are compiled in and covered by tests, so a failure here is a build defect.
        let tokens = Tokens::embedded(ThemeKind::Dark).expect("embedded design tokens");
        theme::apply(ctx, &tokens, ThemeKind::Dark);
        let tile_cache = config.data_folder.join("tiles");
        let db_path = config.data_folder.join("library.sqlite");
        let mut toasts = Toasts::default();
        let lib = match Library::open(&db_path) {
            Ok(l) => Some(l),
            Err(e) => {
                error!("can't open the library: {e}");
                toasts.replace(vec![toasts::for_failure(&format!(
                    "Can't open the library ({e})"
                ))]);
                None
            }
        };
        let mut app = Self {
            palette: Palette::new(&tokens),
            map: MapView::new(
                Provider::default(),
                config.tiles.then_some(tile_cache.as_path()),
                &config.data_folder,
                ctx.clone(),
            ),
            db_path,
            data_folder: config.data_folder.clone(),
            ctx: ctx.clone(),
            lib,
            sources: Vec::new(),
            source_counts: HashMap::new(),
            counts: Counts::default(),
            job: None,
            thumbs: None,
            waiting: VecDeque::new(),
            last_reload: Instant::now(),
            folder_dialog: None,
            sources_dialog: None,
            narrow: None,
            toasts,
            perf: config.perf.then(|| (Instant::now(), Duration::ZERO, 0)),
            detail: None,
            panel_open: false,
            panel: DetailPanel::default(),
            previews: Previews::new(config.data_folder.clone(), ctx.clone()),
            hover: None,
            launch: config.launch,
            launched: Vec::new(),
            launch_errors: mpsc::channel(),
        };
        app.refresh();
        // Thumbnails left over from the last session (or never made) are made now.
        if let Some(lib) = &app.lib
            && lib.thumb_counts().is_ok_and(|(_, pending)| pending > 0)
        {
            app.start_thumbs();
        }
        match config.open {
            Some(StartDialog::AddFolder(folder)) => {
                let mut d = FolderDialog::add();
                if let Some(f) = folder {
                    d.choose(f, ctx);
                }
                app.folder_dialog = Some(d);
            }
            Some(StartDialog::Sources) => {
                app.sources_dialog = Some(SourcesDialog::open(&app.sources, ctx));
            }
            Some(StartDialog::Details) => {
                let first = app
                    .lib
                    .as_ref()
                    .and_then(|l| l.positions().ok())
                    .and_then(|p| p.iter().map(|p| p.id).min());
                app.select(first);
            }
            None => {}
        }
        app
    }

    /// Reloads what the UI shows from the library.
    fn refresh(&mut self) {
        let Some(lib) = &self.lib else { return };
        let loaded = (|| {
            Ok::<_, crate::library::Error>((
                lib.sources()?,
                lib.source_counts()?,
                lib.counts()?,
                lib.positions()?,
            ))
        })();
        match loaded {
            Ok((sources, per_source, counts, points)) => {
                self.sources = sources;
                self.source_counts = per_source;
                self.counts = counts;
                self.map.set_points(points);
            }
            Err(e) => error!("can't read the library: {e}"),
        }
        // The selected photo may have changed (a rescan) or gone (a removal).
        self.hover = None;
        let id = self.map.selected();
        let fresh = id.and_then(|id| lib.photo_detail(id).ok().flatten());
        if fresh != self.detail {
            if let Some(id) = id {
                self.previews.forget(id);
            }
            if fresh.is_none() {
                self.map.select(None);
            }
            self.detail = fresh;
        }
        self.last_reload = Instant::now();
    }

    /// Selects a photo (`None`: nothing) and opens the panel for it.
    fn select(&mut self, id: Option<i64>) {
        self.map.select(id);
        self.detail = id.and_then(|id| {
            self.lib
                .as_ref()
                .and_then(|lib| lib.photo_detail(id).ok().flatten())
        });
        self.panel_open = self.detail.is_some();
    }

    /// Show in Explorer or Open for the selected photo. The source's root is resolved on a
    /// background thread: a sleeping NAS can take seconds to answer.
    fn launch(&mut self, kind: Launch) {
        let Some(d) = self.detail.clone() else { return };
        if !self.launch {
            self.launched.push((kind, d.id));
            return;
        }
        let (errors, ctx) = (self.launch_errors.0.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = match crate::import::reachable_root(&d.root, d.unc.as_deref()) {
                Some(root) => launch::spawn(kind, &details::original_path(&root, &d))
                    .map_err(|e| format!("Can't start Explorer ({e})")),
                None => Err(format!("Can't reach “{}” right now", d.source_label)),
            };
            if let Err(msg) = result {
                let _ = errors.send(msg);
                ctx.request_repaint();
            }
        });
    }

    /// Makes `change` now, or once the running jobs have stopped (the thumbnail job is asked to).
    fn change(&mut self, change: Change) {
        if let Some(t) = &self.thumbs {
            t.cancel();
        }
        self.waiting.push_back(change);
        if self.thumbs.is_none() && self.job.is_none() {
            self.resume();
        }
    }

    /// Makes the waiting changes until one starts an import; with none left, makes thumbnails.
    fn resume(&mut self) {
        while let Some(change) = self.waiting.pop_front() {
            match change {
                Change::Import { source_id, first } => {
                    self.start_import(source_id, first);
                    return;
                }
                Change::Scope { source_id, scope } => {
                    let Some(lib) = &mut self.lib else { continue };
                    match lib.set_scope(source_id, &scope) {
                        Ok(_) => {
                            self.refresh();
                            self.start_import(source_id, false);
                            return;
                        }
                        Err(e) => error!("can't change the folders: {e}"),
                    }
                }
                Change::Remove(id) => {
                    if let Some(lib) = &self.lib
                        && let Err(e) = lib.remove_source(id)
                    {
                        error!("can't remove the source: {e}");
                    }
                    self.refresh();
                }
            }
        }
        // The job first deletes the files of removed photos, then makes the missing thumbnails.
        self.start_thumbs();
    }

    fn start_import(&mut self, source_id: i64, first: bool) {
        info!("import of source {source_id} started");
        self.toasts.replace(Vec::new());
        self.job = Some(ImportJob::start(
            self.db_path.clone(),
            source_id,
            first,
            READERS,
            self.ctx.clone(),
        ));
    }

    /// Started only on events (start-up, after an import or a change), never because thumbnails
    /// are pending: photos on an unreachable drive stay pending, and would restart it forever.
    fn start_thumbs(&mut self) {
        if self.thumbs.is_some() || self.job.is_some() || self.lib.is_none() {
            return;
        }
        self.thumbs = Some(ThumbJob::start(
            self.db_path.clone(),
            self.data_folder.clone(),
            THUMB_WORKERS,
            self.ctx.clone(),
        ));
    }

    fn poll_thumbs(&mut self) {
        let Some(job) = &mut self.thumbs else { return };
        let mut finished = false;
        for e in job.poll() {
            match e {
                ThumbEvent::Progress { .. } => {}
                ThumbEvent::Done(s) => {
                    info!(
                        "thumbnails: {} made, {} placeholders, {} skipped{}",
                        s.made,
                        s.placeholders,
                        s.skipped,
                        if s.cancelled { ", cancelled" } else { "" }
                    );
                    finished = true;
                }
                ThumbEvent::Failed(msg) => {
                    error!("thumbnails failed: {msg}");
                    finished = true;
                }
            }
        }
        if finished {
            self.thumbs = None;
            self.refresh();
            if !self.waiting.is_empty() {
                self.resume();
            }
        } else if self.job.is_none() && self.last_reload.elapsed() >= RELOAD_EVERY {
            self.refresh(); // new thumbnails show as they're made
        }
    }

    fn poll_job(&mut self) {
        let Some(job) = &mut self.job else { return };
        let events = job.poll();
        let (first, took) = (job.first, job.started.elapsed());
        let mut finished = false;
        for e in events {
            match e {
                JobEvent::Progress(_) => {}
                JobEvent::Done(summary) => {
                    // Counts only: `bounds` holds coordinates (design.md §3.8).
                    info!("import finished: {}", summary.counts_only());
                    self.toasts
                        .replace(toasts::for_summary(&summary, first, took));
                    finished = true;
                }
                JobEvent::Failed(msg) => {
                    error!("import failed: {msg}");
                    self.toasts.replace(vec![toasts::for_failure(&msg)]);
                    finished = true;
                }
            }
        }
        if finished {
            self.job = None;
            self.refresh();
            self.resume(); // the next waiting change, or thumbnails for what was imported
        } else if self.last_reload.elapsed() >= RELOAD_EVERY {
            self.refresh();
        }
    }

    fn running(&self) -> Option<Running> {
        if let Some(job) = &self.job {
            let progress = match &job.last {
                Some(crate::import::Progress::Reading { done, total }) => Some((*done, *total)),
                _ => None,
            };
            return Some(Running {
                task: Task::Import,
                progress,
                cancelling: job.cancelling(),
            });
        }
        let t = self.thumbs.as_ref()?;
        Some(Running {
            task: Task::Thumbnails,
            progress: (t.total > 0).then_some((t.done, t.total)),
            cancelling: t.cancelling(),
        })
    }

    fn folder_dialog(&mut self, ctx: &egui::Context) {
        let Some(dialog) = &mut self.folder_dialog else {
            return;
        };
        let Some(action) = dialog.show(ctx, &self.palette, &self.sources) else {
            return;
        };
        self.folder_dialog = None;
        let Some(lib) = &mut self.lib else { return };
        match action {
            FolderAction::Close => {}
            FolderAction::Import { root, scope } => {
                let label = root.file_name().map_or_else(
                    || root.to_string_lossy().into_owned(),
                    |n| n.to_string_lossy().into_owned(),
                );
                let unc = crate::paths::unc_target(&root);
                match lib.add_source(&root, unc.as_deref(), &label, &scope) {
                    Ok(id) => {
                        self.refresh();
                        self.change(Change::Import {
                            source_id: id,
                            first: true,
                        });
                    }
                    Err(e) => self.toasts.replace(vec![toasts::for_failure(&format!(
                        "Can't add the folder ({e})"
                    ))]),
                }
            }
            FolderAction::Apply { source_id, scope } => {
                match lib.count_out_of_scope(source_id, &scope) {
                    Ok(0) => self.change(Change::Scope { source_id, scope }),
                    Ok(removes) => {
                        let label = self
                            .sources
                            .iter()
                            .find(|s| s.id == source_id)
                            .map_or_else(String::new, |s| s.label.clone());
                        self.narrow = Some(NarrowScope {
                            source_id,
                            label,
                            scope,
                            removes,
                        });
                    }
                    Err(e) => error!("can't count the photos to leave out: {e}"),
                }
            }
        }
    }

    fn narrow_dialog(&mut self, ctx: &egui::Context) {
        let Some(n) = &self.narrow else { return };
        let p = &self.palette;
        let (mut go, mut keep) = (false, false);
        let close = widgets::dialog(
            ctx,
            p,
            "narrow-scope",
            &format!("Leave folders of “{}” out?", n.label),
            420.0,
            false,
            |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} photos in those folders leave MapCraft's library and the map.",
                        thousands(n.removes)
                    ))
                    .color(p.text_strong),
                );
                ui.label(
                    RichText::new(
                        "Your files are not touched. Tick the folders again to bring them back.",
                    )
                    .color(p.text_weak),
                );
            },
            |ui| {
                go = widgets::danger(
                    ui,
                    p,
                    &format!("Remove {} photos", thousands(n.removes)),
                    true,
                )
                .clicked();
                keep = widgets::secondary(ui, p, None, "Keep them", true).clicked();
            },
        );
        if go {
            let n = self.narrow.take().expect("shown");
            self.change(Change::Scope {
                source_id: n.source_id,
                scope: n.scope,
            });
        } else if keep || close {
            self.narrow = None;
        }
    }

    fn sources_dialog(&mut self, ctx: &egui::Context) {
        let busy = self.job.is_some();
        let Some(dialog) = &mut self.sources_dialog else {
            return;
        };
        let Some(action) =
            dialog.show(ctx, &self.palette, &self.sources, &self.source_counts, busy)
        else {
            return;
        };
        match action {
            SourcesAction::Close => self.sources_dialog = None,
            SourcesAction::AddFolder => {
                self.sources_dialog = None;
                self.folder_dialog = Some(FolderDialog::add());
            }
            SourcesAction::Rescan(id) => {
                self.sources_dialog = None;
                self.change(Change::Import {
                    source_id: id,
                    first: false,
                });
            }
            SourcesAction::ChangeFolders(id) => {
                if let Some(s) = self.sources.iter().find(|s| s.id == id) {
                    let root = s.root_path.clone();
                    self.folder_dialog = Some(FolderDialog::change(s, root, ctx));
                    self.sources_dialog = None;
                }
            }
            SourcesAction::Remove(id) => self.change(Change::Remove(id)),
        }
    }
}

impl eframe::App for MapCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        self.frame(ui);
    }
}

impl MapCraftApp {
    /// One frame of the whole window. Separate from `eframe::App::ui` so the headless UI tests
    /// (`egui_kittest`) can drive it without a native window.
    pub fn frame(&mut self, ui: &mut egui::Ui) {
        let started = Instant::now();
        let ctx = ui.ctx().clone();
        self.poll_job();
        self.poll_thumbs();

        let p = self.palette.clone();
        let running = self.running();
        let counts = self.counts;
        let mut bar_action = None;
        let bar = Panel::top("top-bar")
            .exact_size(p.size_topbar)
            .frame(Frame::NONE.fill(p.bg_panel))
            .show(ui, |ui| bar_action = top_bar::show(ui, &p, counts, running));
        let r = bar.response.rect;
        ui.painter().hline(
            r.x_range(),
            r.bottom() - p.stroke_hairline / 2.0,
            Stroke::new(p.stroke_hairline, p.border_subtle),
        );
        match bar_action {
            Some(TopBarAction::AddFolder) => self.folder_dialog = Some(FolderDialog::add()),
            Some(TopBarAction::Sources) => {
                self.sources_dialog = Some(SourcesDialog::open(&self.sources, &ctx))
            }
            Some(TopBarAction::Cancel) => {
                if let Some(j) = &self.job {
                    j.cancel();
                } else if let Some(t) = &self.thumbs {
                    t.cancel();
                }
            }
            None => {}
        }

        while let Ok(msg) = self.launch_errors.1.try_recv() {
            self.toasts.replace(vec![toasts::for_failure(&msg)]);
        }

        let empty = self.counts.photos == 0 && self.job.is_none();
        let dialog_open =
            self.folder_dialog.is_some() || self.sources_dialog.is_some() || self.narrow.is_some();

        // Keys: Ctrl+I shows or hides the panel, Esc clears the selection.
        if !dialog_open && !ctx.egui_wants_keyboard_input() {
            let (ctrl_i, esc) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::I),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if ctrl_i && self.detail.is_some() {
                self.panel_open = !self.panel_open;
            }
            if esc && self.detail.is_some() {
                self.select(None);
            }
        }

        // The detail panel, right of the map (design system "Map layout": 320 px).
        let mut panel_action = None;
        if self.panel_open
            && let Some(d) = &self.detail
        {
            let shown = Panel::right("details")
                .exact_size(p.size_panel)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::NONE.fill(p.bg_panel))
                .show(ui, |ui| {
                    let face = match self.previews.get(&ctx, d) {
                        Some(Ok(t)) => PreviewFace::Image(t),
                        Some(Err(())) => PreviewFace::None,
                        None => PreviewFace::Loading(self.map.thumbnail(d.id)),
                    };
                    panel_action = self.panel.show(ui, &p, d, face);
                });
            let r = shown.response.rect;
            ui.painter().vline(
                r.left() + p.stroke_hairline / 2.0,
                r.y_range(),
                Stroke::new(p.stroke_hairline, p.border_subtle),
            );
        }
        match panel_action {
            Some(PanelAction::Close) => self.panel_open = false,
            Some(PanelAction::ShowInExplorer) => self.launch(Launch::ShowInExplorer),
            Some(PanelAction::Open) => self.launch(Launch::Open),
            None => {}
        }

        let mut map_rect = egui::Rect::NOTHING;
        let mut map_action = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.map_ref_land))
            .show(ui, |ui| {
                let (rect, action) = self.map.show(ui, &p, empty, !dialog_open);
                map_rect = rect;
                map_action = action;
            });
        match map_action {
            Some(MapAction::AddFolder) => self.folder_dialog = Some(FolderDialog::add()),
            Some(MapAction::Select(id)) => self.select(id),
            None => {}
        }
        // File names for the markers' accessible names, looked up once each.
        let wanted = self.map.wanted_names();
        if !wanted.is_empty()
            && let Some(lib) = &self.lib
            && let Ok(names) = lib.names(&wanted)
        {
            self.map.add_names(names);
        }
        // The hover card.
        match self.map.hovered() {
            Some((id, marker)) if !dialog_open => {
                if self.hover.as_ref().is_none_or(|h| h.id != id) {
                    self.hover = self
                        .lib
                        .as_ref()
                        .and_then(|lib| lib.photo_detail(id).ok().flatten());
                }
                if let Some(d) = &self.hover {
                    details::hover_card(&ctx, &p, d, marker, map_rect);
                }
            }
            _ => self.hover = None,
        }
        if let Some(ToastAction::ZoomTo(b)) = self.toasts.show(&ctx, &p, map_rect) {
            self.map.zoom_to(b);
        }

        self.folder_dialog(&ctx);
        self.sources_dialog(&ctx);
        self.narrow_dialog(&ctx);

        if let Some((since, total, frames)) = &mut self.perf {
            *total += started.elapsed();
            *frames += 1;
            if since.elapsed() >= Duration::from_secs(3) {
                info!(
                    "perf: {} dots, {:.2} ms per frame over {frames} frames",
                    self.map
                        .all_bounds()
                        .map_or(0, |_| self.counts.with_position),
                    total.as_secs_f64() * 1000.0 / *frames as f64
                );
                *since = Instant::now();
                *total = Duration::ZERO;
                *frames = 0;
            }
            ctx.request_repaint(); // measure continuously
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use egui_kittest::Harness;
    use egui_kittest::kittest::{NodeT, Queryable};

    use super::*;
    use crate::thumbs;

    /// The whole app, headless: no tiles, no network, a temp data folder.
    fn harness(data: &Path) -> Harness<'static, Option<MapCraftApp>> {
        harness_with(data, None)
    }

    fn harness_with(
        data: &Path,
        open: Option<StartDialog>,
    ) -> Harness<'static, Option<MapCraftApp>> {
        let data = data.to_path_buf();
        Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                move |ui, app: &mut Option<MapCraftApp>| {
                    let app = app.get_or_insert_with(|| {
                        egui_extras::install_image_loaders(ui.ctx());
                        MapCraftApp::new(
                            ui.ctx(),
                            Config {
                                data_folder: data.clone(),
                                tiles: false,
                                open: open.clone(),
                                perf: false,
                                launch: false,
                            },
                        )
                    });
                    app.frame(ui);
                },
                None,
            )
    }

    /// Steps frames (with short pauses for the background threads) until `done` holds.
    fn step_until(
        h: &mut Harness<'static, Option<MapCraftApp>>,
        what: &str,
        done: impl Fn(&Harness<'static, Option<MapCraftApp>>) -> bool,
    ) {
        let deadline = Instant::now() + Duration::from_secs(20);
        h.run_steps(2);
        while !done(h) {
            assert!(Instant::now() < deadline, "timed out waiting for {what}");
            std::thread::sleep(Duration::from_millis(20));
            h.step();
        }
        h.run_steps(2);
    }

    fn job_running(h: &Harness<'static, Option<MapCraftApp>>) -> bool {
        h.state().as_ref().is_some_and(|a| a.job.is_some())
    }

    /// A data folder with `n` generated photos already imported as source "Demo trips".
    fn imported(dir: &Path, n: usize) -> std::path::PathBuf {
        let photos = dir.join("photos");
        crate::demo::make_demo(&photos, n).unwrap();
        let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
        let src = lib
            .add_source(&photos, None, "Demo trips", &Scope::all())
            .unwrap();
        crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        photos
    }

    #[test]
    fn importing_a_folder_from_the_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        crate::demo::make_demo(&photos, 16).unwrap();
        let mut h = harness_with(
            &dir.path().join("data"),
            Some(StartDialog::AddFolder(Some(photos))),
        );
        step_until(&mut h, "the count", |h| {
            h.query_by_label("Import 16 photos").is_some()
        });
        h.get_by_label("Import 16 photos").click();
        step_until(&mut h, "the import", |h| {
            !job_running(h) && h.query_by_label("Import finished").is_some()
        });
        h.get_by_label_contains("16 photos · 14 on the map");
        h.get_by_label("16 photos · 2 without a position");
        h.get_by_label("2 photos have no position");
        assert!(
            h.query_by_label("Add photo folder").is_none(),
            "the dialog closed"
        );
    }

    #[test]
    fn thumbnails_are_made_at_start_and_their_files_go_with_the_source() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let thumbs_dir = dir.path().join("thumbs");
        let files = walkdir_count;
        let mut h = harness(dir.path());
        step_until(&mut h, "the thumbnails", |h| {
            h.state().as_ref().unwrap().thumbs.is_none()
        });
        let app = h.state().as_ref().unwrap();
        let (made, pending) = app.lib.as_ref().unwrap().thumb_counts().unwrap();
        assert_eq!((made, pending), (7, 0), "every photo on the map has one");
        assert_eq!(files(&thumbs_dir), 7);
        let source = app.sources[0].id;
        h.state_mut()
            .as_mut()
            .unwrap()
            .change(Change::Remove(source));
        step_until(&mut h, "the sweep", |h| {
            h.state().as_ref().unwrap().thumbs.is_none() && files(&thumbs_dir) == 0
        });
    }

    /// The number of files below `dir`.
    fn walkdir_count(dir: &Path) -> usize {
        let Ok(rd) = std::fs::read_dir(dir) else {
            return 0;
        };
        rd.flatten()
            .map(|e| {
                if e.path().is_dir() {
                    walkdir_count(&e.path())
                } else {
                    1
                }
            })
            .sum()
    }

    #[test]
    fn clicking_a_marker_shows_the_photo_and_its_actions() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness(dir.path());
        step_until(&mut h, "the markers", |h| {
            h.state().as_ref().unwrap().thumbs.is_none()
                && h.query_all_by_label_contains("Photo IMG_").count() > 0
        });
        let name = h
            .query_all_by_label_contains("Photo IMG_")
            .next()
            .unwrap()
            .accesskit_node()
            .label()
            .unwrap()
            .trim_start_matches("Photo ")
            .to_owned();
        // Hovering shows the card: the name, and the capture date from the demo's EXIF.
        h.get_by_label(&format!("Photo {name}")).hover();
        step_until(&mut h, "the hover card", |h| {
            h.query_by_label(&name).is_some()
        });
        assert!(
            h.query_by_label_contains(", 12:").is_some(),
            "the capture time"
        );
        h.get_by_label(&format!("Photo {name}")).click();
        step_until(&mut h, "the panel", |h| {
            h.query_by_label("Photo details").is_some()
        });
        assert!(h.query_all_by_label(&name).count() >= 1, "the name row");
        h.get_by_label("Demo trips"); // the source row
        // A click in the panel's first few frames is lost under kittest (the button doesn't
        // move; the cause isn't pinned down, and no person clicks that fast), so let it settle.
        h.run_steps(5);
        h.get_by_label("Show in Explorer").click();
        h.run_steps(2);
        h.get_by_label("Open").click();
        h.run_steps(2);
        let app = h.state().as_ref().unwrap();
        let id = app.detail.as_ref().unwrap().id;
        assert_eq!(
            app.launched,
            vec![(Launch::ShowInExplorer, id), (Launch::Open, id)]
        );
        // The preview is made from the demo photo in the background.
        step_until(&mut h, "the preview", |_| {
            thumbs::path(dir.path(), id, thumbs::PREVIEW).exists()
        });
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(h.query_by_label("Photo details").is_none());
        assert_eq!(h.state().as_ref().unwrap().map.selected(), None);
    }

    #[test]
    fn removing_a_source_empties_the_map() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness_with(dir.path(), Some(StartDialog::Sources));
        step_until(&mut h, "the online check", |h| {
            h.query_by_label("Online").is_some()
        });
        h.get_by_label("Remove…").click();
        h.run_steps(4); // a new window is laid out invisibly on its first frame
        h.get_by_label("Remove “Demo trips”?");
        h.get_by_label("Remove source").click();
        // The removal waits for the thumbnail job, which started with the app.
        step_until(&mut h, "the removal", |h| {
            h.state().as_ref().unwrap().counts.photos == 0
        });
        h.get_by_label("Close (Esc)").click();
        h.run_steps(3);
        h.get_by_label("No photos on the map yet");
    }

    #[test]
    fn leaving_a_folder_out_asks_and_removes_its_photos() {
        let dir = tempfile::tempdir().unwrap();
        let photos = imported(dir.path(), 16);
        let mut h = harness(dir.path());
        h.run_steps(2);
        {
            let ctx = h.ctx.clone();
            let app = h.state_mut().as_mut().unwrap();
            let source = app.sources[0].clone();
            app.folder_dialog = Some(FolderDialog::change(&source, photos, &ctx));
        }
        step_until(&mut h, "the folder walk", |h| {
            h.query_by_label("Include 2019-05-04_Lisbon").is_some()
        });
        h.get_by_label("Include 2019-05-04_Lisbon").click();
        h.run_steps(2);
        h.get_by_label("Apply").click();
        h.run_steps(2);
        h.get_by_label("Leave folders of “Demo trips” out?");
        h.get_by_label("Remove 2 photos").click();
        step_until(&mut h, "the rescan", |h| {
            !job_running(h) && h.query_by_label("Rescan finished").is_some()
        });
        let app = h.state().as_ref().unwrap();
        assert_eq!(app.counts.photos, 14);
        assert!(!app.sources[0].scope.includes("2019-05-04_lisbon"));
    }

    #[test]
    fn the_add_folder_dialog_walks_a_chosen_folder() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        crate::demo::make_demo(&photos, 16).unwrap();
        let data = dir.path().join("data");
        let mut h = Harness::builder()
            .with_size(egui::vec2(1280.0, 800.0))
            .build_ui_state(
                move |ui, app: &mut Option<MapCraftApp>| {
                    let app = app.get_or_insert_with(|| {
                        egui_extras::install_image_loaders(ui.ctx());
                        MapCraftApp::new(
                            ui.ctx(),
                            Config {
                                data_folder: data.clone(),
                                tiles: false,
                                open: Some(StartDialog::AddFolder(Some(photos.clone()))),
                                perf: false,
                                launch: false,
                            },
                        )
                    });
                    app.frame(ui);
                },
                None,
            );
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            h.run_steps(2);
            if h.query_by_label("Import 16 photos").is_some() {
                break;
            }
            assert!(
                Instant::now() < deadline,
                "the dialog never counted the photos"
            );
            std::thread::sleep(Duration::from_millis(20));
        }
        h.get_by_label("Add photo folder");
        h.get_by_label("2019-05-04_Lisbon");
    }

    #[test]
    fn the_empty_map_offers_to_add_a_folder() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path());
        h.run();
        h.get_by_label("No photos on the map yet");
        // Two "Add folder…" buttons: the top bar's and the card's; either opens the dialog.
        let buttons: Vec<_> = h.get_all_by_label("Add folder…").collect();
        assert_eq!(buttons.len(), 2);
        buttons[1].click();
        h.run();
        h.get_by_label("Add photo folder");
        h.get_by_label("Choose a folder to see what's in it.");
    }

    #[test]
    fn imported_photos_show_in_the_counts_and_the_sources() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        crate::demo::make_demo(&photos, 24).unwrap();
        {
            let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
            let src = lib
                .add_source(&photos, None, "Demo trips", &Scope::all())
                .unwrap();
            crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        }
        let mut h = harness(dir.path());
        step_until(&mut h, "the thumbnails", |h| {
            h.state().as_ref().unwrap().thumbs.is_none()
        });
        h.get_by_label("24 photos · 3 without a position");
        assert!(h.query_by_label("No photos on the map yet").is_none());
        // Open the sources list from the state, as the top bar's icon button would.
        let ctx = h.ctx.clone();
        h.state_mut().as_mut().unwrap().sources_dialog = Some(SourcesDialog::open(
            &h.state().as_ref().unwrap().sources,
            &ctx,
        ));
        h.run();
        assert_eq!(
            h.get_all_by_label("Sources").count(),
            2,
            "the button and the dialog's title"
        );
        h.get_by_label("Demo trips");
        h.get_by_label("Rescan").click();
        // The progress chip spins while the rescan runs, so step frames until it's done.
        let deadline = Instant::now() + Duration::from_secs(20);
        h.run_steps(2);
        while h.state().as_ref().unwrap().job.is_some() {
            assert!(Instant::now() < deadline, "the rescan never finished");
            std::thread::sleep(Duration::from_millis(20));
            h.step();
        }
        h.run_steps(2);
        h.get_by_label("Rescan finished");
        h.get_by_label_contains("24 unchanged");
    }
}

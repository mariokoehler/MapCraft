//! The main window: top bar above, the map filling the rest (design system "Map layout"), and
//! the import's dialogs and toasts (canvas `MapCraft — Import`).
//!
//! The UI thread only reads the library (its own connection, WAL mode) and never touches the
//! photo folders: imports, folder walks and online checks run on background threads.

use std::collections::HashMap;
use std::path::PathBuf;
use std::time::{Duration, Instant};

use egui::{Frame, Panel, RichText, Stroke};
use tracing::{error, info};

use crate::import::scope::Scope;
use crate::jobs::{ImportJob, JobEvent};
use crate::library::{Counts, Library, Source};
use crate::provider::Provider;
use crate::theme::{self, Palette, ThemeKind, Tokens};
use crate::ui::folder_dialog::{FolderAction, FolderDialog};
use crate::ui::map_view::{MapAction, MapView};
use crate::ui::sources_dialog::{SourcesAction, SourcesDialog};
use crate::ui::toasts::{self, ToastAction, Toasts};
use crate::ui::top_bar::{self, Running, TopBarAction};
use crate::ui::widgets::{self, thousands};

/// Parallel metadata readers per import (design.md §3.4, user-confirmed).
const READERS: usize = 4;
/// How often the map's dots refresh while an import writes.
const RELOAD_EVERY: Duration = Duration::from_secs(1);

/// A dialog to open at start (dev flag `--open`), for comparing screens with the mockups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartDialog {
    /// Optionally with a folder already chosen, so its tree shows.
    AddFolder(Option<PathBuf>),
    Sources,
}

pub struct Config {
    pub data_folder: PathBuf,
    /// Draw OSM tiles (off in tests: no network).
    pub tiles: bool,
    pub open: Option<StartDialog>,
    /// Log the average frame time every few seconds (dev flag `--perf`).
    pub perf: bool,
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
    lib: Option<Library>,
    sources: Vec<Source>,
    source_counts: HashMap<i64, Counts>,
    counts: Counts,
    job: Option<ImportJob>,
    last_reload: Instant,
    folder_dialog: Option<FolderDialog>,
    sources_dialog: Option<SourcesDialog>,
    narrow: Option<NarrowScope>,
    toasts: Toasts,
    perf: Option<(Instant, Duration, u32)>,
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
                ctx.clone(),
            ),
            db_path,
            lib,
            sources: Vec::new(),
            source_counts: HashMap::new(),
            counts: Counts::default(),
            job: None,
            last_reload: Instant::now(),
            folder_dialog: None,
            sources_dialog: None,
            narrow: None,
            toasts,
            perf: config.perf.then(|| (Instant::now(), Duration::ZERO, 0)),
        };
        app.refresh();
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
        self.last_reload = Instant::now();
    }

    fn start_job(&mut self, source_id: i64, first: bool, ctx: &egui::Context) {
        if self.job.is_some() {
            return; // one job at a time; the UI doesn't offer a second
        }
        info!("import of source {source_id} started");
        self.toasts.replace(Vec::new());
        self.job = Some(ImportJob::start(
            self.db_path.clone(),
            source_id,
            first,
            READERS,
            ctx.clone(),
        ));
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
                    info!("import finished: {summary:?}");
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
        } else if self.last_reload.elapsed() >= RELOAD_EVERY {
            self.refresh();
        }
    }

    fn running(&self) -> Option<Running> {
        let job = self.job.as_ref()?;
        let progress = match &job.last {
            Some(crate::import::Progress::Reading { done, total }) => Some((*done, *total)),
            _ => None,
        };
        Some(Running {
            progress,
            cancelling: job.cancelling(),
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
                        self.start_job(id, true, ctx);
                    }
                    Err(e) => self.toasts.replace(vec![toasts::for_failure(&format!(
                        "Can't add the folder ({e})"
                    ))]),
                }
            }
            FolderAction::Apply { source_id, scope } => {
                match lib.count_out_of_scope(source_id, &scope) {
                    Ok(0) => self.apply_scope(source_id, &scope, ctx),
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

    fn apply_scope(&mut self, source_id: i64, scope: &Scope, ctx: &egui::Context) {
        let Some(lib) = &mut self.lib else { return };
        match lib.set_scope(source_id, scope) {
            Ok(_) => {
                self.refresh();
                self.start_job(source_id, false, ctx);
            }
            Err(e) => error!("can't change the folders: {e}"),
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
            self.apply_scope(n.source_id, &n.scope, ctx);
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
                self.start_job(id, false, ctx);
            }
            SourcesAction::ChangeFolders(id) => {
                if let Some(s) = self.sources.iter().find(|s| s.id == id) {
                    let root = s.root_path.clone();
                    self.folder_dialog = Some(FolderDialog::change(s, root, ctx));
                    self.sources_dialog = None;
                }
            }
            SourcesAction::Remove(id) => {
                if let Some(lib) = &self.lib
                    && let Err(e) = lib.remove_source(id)
                {
                    error!("can't remove the source: {e}");
                }
                self.refresh();
            }
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
            Some(TopBarAction::CancelImport) => {
                if let Some(j) = &self.job {
                    j.cancel();
                }
            }
            None => {}
        }

        let empty = self.counts.photos == 0 && self.job.is_none();
        let mut map_rect = egui::Rect::NOTHING;
        let mut map_action = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.map_ref_land))
            .show(ui, |ui| {
                let (rect, action) = self.map.show(ui, &p, empty);
                map_rect = rect;
                map_action = action;
            });
        if map_action == Some(MapAction::AddFolder) {
            self.folder_dialog = Some(FolderDialog::add());
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
    use egui_kittest::kittest::Queryable;

    use super::*;

    /// The whole app, headless: no tiles, no network, a temp data folder.
    fn harness(data: &Path) -> Harness<'static, Option<MapCraftApp>> {
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
                                open: None,
                                perf: false,
                            },
                        )
                    });
                    app.frame(ui);
                },
                None,
            )
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
        h.run();
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

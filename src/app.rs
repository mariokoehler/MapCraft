//! The main window: top bar above, the map filling the rest (design system "Map layout"), and
//! the import's dialogs and toasts (canvas `MapCraft — Import`).
//!
//! The UI thread only reads the library (its own connection, WAL mode) and never touches the
//! photo folders: imports, folder walks and online checks run on background threads.

use std::collections::{HashMap, VecDeque};
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, Sender};
use std::time::{Duration, Instant};

use egui::{Frame, Panel, RichText, Stroke};
use tracing::{error, info};

use crate::companions::{self, Companion, Event as LcEvent, Failure};
use crate::datafolder::{self, CopyError, Kind, Progress, Size, Target, Usage};
use crate::filter::{Facets, Filter};
use crate::import::scope::Scope;
use crate::jobs::{ImportJob, JobEvent, ThumbEvent, ThumbJob};
use crate::launch::{self, Launch};
use crate::library::{Counts, Library, PhotoDetail, Source};
use crate::provider::{KeyCheck, Style};
use crate::settings::Settings;
use crate::theme::{self, Palette, ThemeKind, Tokens};
use crate::ui::data_dialogs::{
    ChangeAction, ChangeDialog, ChangeStage, FreeAction, FreeSpaceDialog,
};
use crate::ui::details::{self, CompanionButtons, DetailPanel, PanelAction, PreviewFace, Previews};
use crate::ui::filters::{self, FilterPanel, FiltersAction};
use crate::ui::folder_dialog::{FolderAction, FolderDialog};
use crate::ui::map_view::{MapAction, MapView};
use crate::ui::search::{SearchAction, SearchBox};
use crate::ui::settings_dialog::{DataView, Section, SettingsAction, SettingsDialog};
use crate::ui::sources_dialog::{SourcesAction, SourcesDialog};
use crate::ui::toasts::{self, Toast, ToastAction, Toasts};
use crate::ui::top_bar::{self, BarFilters, Running, Task, TopBarAction};
use crate::ui::update_dialog::{Stage, UpdateAction, UpdateDialog};
use crate::ui::widgets::{self, Tone, thousands};
use crate::update::{self, CheckStatus, DownloadError, Release, Updater, Version};

mod geotag;

/// Parallel metadata readers per import (design.md §3.4, user-confirmed).
const READERS: usize = 4;
/// Parallel thumbnail renderers (design.md §2.2(b): 4 workers on the NAS).
const THUMB_WORKERS: usize = 4;
/// How often the map's dots refresh while an import writes.
const RELOAD_EVERY: Duration = Duration::from_secs(1);

/// The toast while LightCraft starts; it gives way to the outcome.
const STARTING: &str = "Opening in LightCraft…";

/// The toast for a photo whose source can't be reached (a sleeping or unplugged drive).
fn unreachable(d: &PhotoDetail) -> Toast {
    toasts::note(
        Tone::Error,
        &format!("Can't reach “{}” right now", d.source_label),
        "Its drive or share isn't answering. Try again when it's reachable.",
        None,
    )
}

/// The toast after "Restart later".
fn update_ready(v: Version) -> Toast {
    toasts::note(
        Tone::Info,
        "Update ready",
        &format!("MapCraft {v} is installed when you close MapCraft."),
        None,
    )
}

/// The folder of the running exe, where an update is downloaded and swapped in.
fn exe_dir() -> Option<PathBuf> {
    std::env::current_exe().ok()?.parent().map(PathBuf::from)
}

/// A dialog to open at start (dev flag `--open`), for comparing screens with the mockups.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartDialog {
    /// Optionally with a folder already chosen, so its tree shows.
    AddFolder(Option<PathBuf>),
    Sources,
    /// The first photo on the map selected, its detail panel open.
    Details,
    /// The strip of the first group in view that can't split, once the groups are built.
    Strip,
    /// This text typed into the search field.
    Search(String),
    /// The filter panel open, or (with a year) that year's photos shown, the panel closed.
    Filters(Option<u32>),
    Settings,
    /// Settings on its Data folder section.
    DataFolder,
    /// The Change data folder dialog, as if this folder had been picked.
    ChangeData(PathBuf),
    /// Tagging mode with this GPX file (design.md §2.9).
    Geotag(PathBuf),
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
    /// Downloads other than tiles may run when the user asks (the place names; off in tests).
    pub network: bool,
    /// The settings as loaded, and where to save them when they change (`None`: not saved).
    pub settings: Settings,
    pub settings_file: Option<PathBuf>,
    /// Ask for a newer release at start (design.md §3.10; off in tests and debug builds).
    pub check_updates: bool,
    /// Where releases come from (GitHub, or a pretend one for trials).
    pub update_endpoint: update::Endpoint,
    /// The data folder came from `--data`: Settings doesn't offer to change it (design.md M8).
    pub data_override: bool,
}

/// The map style's background work (design.md M10).
enum MapEvent {
    /// MapTiler's answer to the key (with the key it was for).
    Key(String, KeyCheck),
    Logo(Arc<[u8]>),
}

/// Work on the data folder that waits for the thumbnail job to stop (design.md M8).
#[derive(Debug, Clone, PartialEq)]
enum DataTask {
    Copy(PathBuf),
    Purge(Vec<Kind>),
}

/// What the data folder's background work reports.
enum DataEvent {
    /// The usage, and the old folder's size and photos (after a move).
    Counted(Usage, Option<(u64, Option<i64>)>),
    Progress(Progress),
    Copied(Result<(), CopyError>),
    Purged(Size, Vec<Kind>),
    /// The thumbnails couldn't be marked as not made; nothing was deleted.
    PurgeFailed,
    Removed(Result<(), String>),
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
    /// What the companion buttons asked for, when `launch` is off: app, photo, add to LightCraft.
    companion_calls: Vec<(Companion, i64, bool)>,
    launch_errors: (Sender<Toast>, Receiver<Toast>),
    settings: Settings,
    settings_file: Option<PathBuf>,
    settings_dialog: Option<SettingsDialog>,
    /// Whether each companion app's file is there (PhotoCraft, LightCraft).
    found: [bool; 2],
    /// LightCraft's answers, from the thread that talks to it.
    lightcraft: (Sender<LcEvent>, Receiver<LcEvent>),
    /// A request to LightCraft is running, for this photo.
    lc_busy: bool,
    lc_photo: Option<PhotoDetail>,
    /// The photo LightCraft doesn't have, while asking whether to add it; and whether
    /// LightCraft writes XMP sidecars.
    lc_add: Option<(PhotoDetail, bool)>,
    search: SearchBox,
    /// The filters (session only, design.md §2.3) and what the panel offers.
    filter: Filter,
    facets: Facets,
    filters: FilterPanel,
    /// Photos on the map the filter shows.
    shown: usize,
    /// Updates (design.md §3.10): the background work, what the last check found, the release
    /// on offer, the dialog, the verified download, and whether the running check was the one
    /// at start (which announces itself with a toast).
    updater: Updater,
    update_status: CheckStatus,
    offer: Option<Release>,
    update_dialog: Option<UpdateDialog>,
    downloaded: Option<PathBuf>,
    auto_check: bool,
    /// What the update steps asked for, when `launch` is off (for the tests).
    update_calls: Vec<String>,
    /// "Restart later" was chosen: the download waits for the window to close.
    update_pending: bool,
    /// The data folder (design.md M8): what it takes (and the old folder, after a move), the
    /// dialogs, the background work and its cancel flag, and work waiting for the thumbnails.
    data_override: bool,
    data_usage: Option<Usage>,
    old_size: Option<u64>,
    /// The photos in the old folder's library, to tell whether the new place has them all.
    old_photos: Option<i64>,
    data_events: (Sender<DataEvent>, Receiver<DataEvent>),
    data_cancel: Arc<AtomicBool>,
    change_dialog: Option<ChangeDialog>,
    free_dialog: Option<FreeSpaceDialog>,
    remove_old_open: bool,
    data_waiting: Option<DataTask>,
    /// What the map style's background work found: a MapTiler key's answer, its logo.
    map_events: (Sender<MapEvent>, Receiver<MapEvent>),
    /// Geotagging (design.md §2.9): the session while tagging mode is on, its dialogs, the
    /// write or undo running, the last write's journal (for Undo), and the candidates whose
    /// thumbnails the thumbnail job makes first.
    geotag: Option<crate::geotag::session::Session>,
    geo_confirm: bool,
    geo_leave: bool,
    geo_work: Option<geotag::GeoWork>,
    /// The newest journal and what it holds, for Undo (from the toast or the panel).
    geo_last: Option<(PathBuf, String)>,
    thumbs_for: Vec<i64>,
    /// The thumbnail job was stopped to start again with `thumbs_for` first.
    restart_thumbs: bool,
}

impl MapCraftApp {
    pub fn new(ctx: &egui::Context, config: Config) -> Self {
        // The tokens are compiled in and covered by tests, so a failure here is a build defect.
        let tokens = Tokens::embedded(ThemeKind::Dark).expect("embedded design tokens");
        theme::apply(ctx, &tokens, ThemeKind::Dark);
        let db_path = config.data_folder.join("library.sqlite");
        let mut toasts = Toasts::default();
        let lib = match Library::open(&db_path) {
            Ok(l) => Some(l),
            Err(e) if crate::library::is_too_new(&e) => {
                error!("can't open the library: {e}");
                toasts.replace(vec![toasts::note(
                    Tone::Error,
                    "This library needs a newer MapCraft",
                    "A newer version of MapCraft opened it last. Update MapCraft to open it \
                     (Settings → Updates); nothing was changed.",
                    Some(("Settings…", ToastAction::OpenSettings)),
                )]);
                None
            }
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
                config.settings.map.style,
                config.settings.map.key().map(str::to_owned),
                config.tiles.then_some(config.data_folder.as_path()),
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
            companion_calls: Vec::new(),
            launch_errors: mpsc::channel(),
            found: [false; 2],
            settings: config.settings,
            settings_file: config.settings_file,
            settings_dialog: None,
            lightcraft: mpsc::channel(),
            lc_busy: false,
            lc_photo: None,
            lc_add: None,
            search: SearchBox::new(
                config.data_folder.join("library.sqlite"),
                &config.data_folder,
                config.network,
                ctx.clone(),
            ),
            filter: Filter::default(),
            facets: Facets::default(),
            filters: FilterPanel::default(),
            shown: 0,
            updater: Updater::new(config.update_endpoint, config.network),
            update_status: CheckStatus::Idle,
            offer: None,
            update_dialog: None,
            downloaded: None,
            auto_check: false,
            update_calls: Vec::new(),
            update_pending: false,
            data_override: config.data_override,
            data_usage: None,
            old_size: None,
            old_photos: None,
            data_events: mpsc::channel(),
            data_cancel: Arc::new(AtomicBool::new(false)),
            change_dialog: None,
            free_dialog: None,
            remove_old_open: false,
            data_waiting: None,
            map_events: mpsc::channel(),
            geotag: None,
            geo_confirm: false,
            geo_leave: false,
            geo_work: None,
            geo_last: crate::geotag::journal::latest(&config.data_folder),
            thumbs_for: Vec::new(),
            restart_thumbs: false,
        };
        if config.check_updates {
            app.auto_check = true;
            app.check_updates(ctx);
        }
        if app.map.style().needs_key() {
            app.map_style_work(app.map.style());
        }
        app.refresh();
        app.find_companions();
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
            Some(StartDialog::Strip) => app.map.open_first_strip(),
            Some(StartDialog::Search(text)) => app.search.type_in(&text),
            Some(StartDialog::Filters(None)) => app.filters.open = true,
            Some(StartDialog::Filters(Some(year))) => {
                app.filter.dates = Some((year * 100 + 1, year * 100 + 12));
                app.filter_changed();
            }
            Some(StartDialog::Settings) => app.open_settings(ctx),
            Some(StartDialog::DataFolder) => {
                app.open_settings(ctx);
                if let Some(s) = &mut app.settings_dialog {
                    s.section = Section::Data;
                }
            }
            Some(StartDialog::ChangeData(folder)) => {
                app.count_usage();
                app.pick_data_folder(&folder);
            }
            Some(StartDialog::Geotag(file)) => app.start_geotag(vec![file]),
            None => {}
        }
        app
    }

    /// Switches the map's style and remembers it; a MapTiler style is checked in the
    /// background (and falls back to Standard if MapTiler refuses the key).
    fn set_map_style(&mut self, style: Style, ctx: &egui::Context) {
        let key = self.settings.map.key().map(str::to_owned);
        if !self.map.set_style(style, key, ctx) {
            return;
        }
        if self.settings.map.style != style {
            self.settings.map.style = style;
            self.save_settings();
        }
        if style.needs_key() {
            self.map_style_work(style);
        }
    }

    /// For a MapTiler style: its key tested once, and its logo loaded (from the data folder,
    /// else from MapTiler). Nothing reaches the network in tests.
    fn map_style_work(&mut self, _style: Style) {
        let (Some(key), true) = (self.settings.map.key().map(str::to_owned), self.launch) else {
            return;
        };
        let tx = self.map_events.0.clone();
        let ctx = self.ctx.clone();
        let logo = crate::provider::logo_path(&self.data_folder);
        std::thread::spawn(move || {
            let answer = crate::provider::check_key(&key);
            let _ = tx.send(MapEvent::Key(key, answer));
            let bytes = std::fs::read(&logo).ok().or_else(|| {
                let b = reqwest::blocking::Client::builder()
                    .user_agent(crate::provider::USER_AGENT)
                    .timeout(Duration::from_secs(15))
                    .build()
                    .ok()?
                    .get(crate::provider::MAPTILER_LOGO)
                    .send()
                    .ok()
                    .filter(|r| r.status().is_success())?
                    .bytes()
                    .ok()?
                    .to_vec();
                if let Some(dir) = logo.parent() {
                    let _ = std::fs::create_dir_all(dir);
                }
                let _ = std::fs::write(&logo, &b);
                Some(b)
            });
            if let Some(b) = bytes {
                let _ = tx.send(MapEvent::Logo(Arc::from(b)));
            }
            ctx.request_repaint();
        });
    }

    fn poll_map(&mut self, ctx: &egui::Context) {
        while let Ok(e) = self.map_events.1.try_recv() {
            match e {
                MapEvent::Logo(svg) => self.map.set_logo(Some(svg)),
                MapEvent::Key(key, answer) => {
                    if self.settings.map.key() != Some(key.as_str()) {
                        continue; // a key changed since
                    }
                    let why = match answer {
                        KeyCheck::Works | KeyCheck::Failed(_) => continue,
                        KeyCheck::Refused => {
                            "MapTiler doesn’t accept the key: it may have been changed or \
                             removed on MapTiler’s site."
                        }
                        KeyCheck::UsedUp => "This month’s free MapTiler tiles are used up.",
                    };
                    info!("MapTiler refused the key ({answer:?}); back to Standard");
                    self.set_map_style(Style::Standard, ctx);
                    self.toasts.push(toasts::note(
                        Tone::Warning,
                        "MapTiler refused the key",
                        &format!("{why} The map is back to Standard."),
                        Some(("Settings…", ToastAction::OpenMapSettings)),
                    ));
                }
            }
        }
    }

    /// Reloads what the UI shows from the library.
    fn refresh(&mut self) {
        let Some(lib) = &self.lib else { return };
        let loaded = (|| {
            Ok::<_, crate::library::Error>((
                lib.sources()?,
                lib.source_counts()?,
                lib.counts()?,
                lib.facets()?,
            ))
        })();
        match loaded {
            Ok((sources, per_source, counts, facets)) => {
                // A removed source's id may come back for a new one: forget it.
                let before = self.filter.clone();
                self.filter
                    .prune_sources(|id| sources.iter().any(|s| s.id == id));
                if self.filter != before {
                    self.search.set_filter(&self.filter);
                }
                self.sources = sources;
                self.source_counts = per_source;
                self.counts = counts;
                self.facets = facets;
                match lib.positions_shown(&self.filter) {
                    // In tagging mode the map shows the session's photos (design.md §2.9).
                    Ok(points) if self.geotag.is_some() => self.shown = points.len(),
                    Ok(points) => {
                        self.shown = points.len();
                        self.map.set_filtered(self.filter.is_active());
                        self.map.set_points(points);
                    }
                    Err(e) => error!("can't read the positions: {e}"),
                }
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
                self.panel_open = false;
            }
            self.detail = fresh;
        }
        self.last_reload = Instant::now();
        if self.geotag.is_some() {
            self.geotag_reload();
        }
    }

    /// The filter changed: the map, the counts and search follow it.
    fn filter_changed(&mut self) {
        self.search.set_filter(&self.filter);
        self.refresh();
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
                Some(root) => {
                    launch::spawn(kind, &details::original_path(&root, &d)).map_err(|e| {
                        toasts::note(
                            Tone::Error,
                            "Explorer couldn't be started",
                            &e.to_string(),
                            None,
                        )
                    })
                }
                None => Err(unreachable(&d)),
            };
            if let Err(t) = result {
                let _ = errors.send(t);
                ctx.request_repaint();
            }
        });
    }

    /// Whether the companion apps are where the settings say.
    fn find_companions(&mut self) {
        let c = &self.settings.companions;
        self.found = [
            companions::is_app(&c.photocraft),
            companions::is_app(&c.lightcraft),
        ];
    }

    fn save_settings(&self) {
        if let Some(f) = &self.settings_file
            && let Err(e) = self.settings.save_to(f)
        {
            error!("can't save the settings: {e}");
        }
    }

    /// Whether the companion buttons work for `d`, and why not (canvas artboard 2).
    fn companion_buttons(&self, d: &PhotoDetail) -> CompanionButtons {
        let missing =
            |app: Companion| format!("{} wasn't found. Set where it is in Settings.", app.name());
        let photocraft = if !self.found[0] {
            Some(missing(Companion::PhotoCraft))
        } else if !companions::photocraft_opens(&d.original_name) {
            Some(if companions::is_raw(&d.original_name) {
                "PhotoCraft can't open camera RAW files".to_owned()
            } else {
                "PhotoCraft can't open this type of file".to_owned()
            })
        } else {
            None
        };
        let lightcraft = if !self.found[1] {
            Some(missing(Companion::LightCraft))
        } else if self.lc_busy {
            Some("Opening in LightCraft…".to_owned())
        } else {
            None
        };
        CompanionButtons {
            photocraft,
            lightcraft,
        }
    }

    /// Opens the selected photo in PhotoCraft, from a background thread (the NAS may sleep).
    fn open_in_photocraft(&mut self) {
        let Some(d) = self.detail.clone() else { return };
        if !self.launch {
            self.companion_calls
                .push((Companion::PhotoCraft, d.id, false));
            return;
        }
        let app = self.settings.companions.photocraft.clone();
        let (errors, ctx) = (self.launch_errors.0.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let result = match crate::import::reachable_root(&d.root, d.unc.as_deref()) {
                Some(root) => {
                    companions::open_in_photocraft(&app, &details::original_path(&root, &d))
                        .map_err(|e| {
                            error!("can't start PhotoCraft: {e}");
                            toasts::note(
                                Tone::Error,
                                "PhotoCraft couldn't be started",
                                "Check where it is in Settings.",
                                Some(("Settings…", ToastAction::OpenSettings)),
                            )
                        })
                }
                None => Err(unreachable(&d)),
            };
            if let Err(t) = result {
                let _ = errors.send(t);
                ctx.request_repaint();
            }
        });
    }

    /// Shows `d` (default: the selected photo) in LightCraft; `add`: add it to LightCraft's
    /// library first, which the user agreed to.
    fn show_in_lightcraft(&mut self, d: Option<PhotoDetail>, add: bool) {
        let Some(d) = d.or_else(|| self.detail.clone()) else {
            return;
        };
        if self.lc_busy {
            return;
        }
        if !self.launch {
            self.companion_calls
                .push((Companion::LightCraft, d.id, add));
            return;
        }
        self.lc_busy = true;
        self.lc_photo = Some(d.clone());
        let mut req = companions::Request {
            app: self.settings.companions.lightcraft.clone(),
            port: self.settings.companions.lightcraft_port,
            file: PathBuf::new(),
            add,
            front: true,
        };
        let (tx, ctx) = (self.lightcraft.0.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let send = |e| {
                let _ = tx.send(e);
                ctx.request_repaint();
            };
            match crate::import::reachable_root(&d.root, d.unc.as_deref()) {
                Some(root) => {
                    req.file = details::original_path(&root, &d);
                    companions::show_in_lightcraft(&req, send);
                }
                None => send(LcEvent::Failed(Failure::Refused(format!(
                    "Can't reach “{}” right now.",
                    d.source_label
                )))),
            }
        });
    }

    /// LightCraft's answers: the toasts and the add dialog (canvas artboards 3 and 4).
    fn poll_lightcraft(&mut self) {
        while let Ok(e) = self.lightcraft.1.try_recv() {
            self.lightcraft_event(e);
        }
    }

    fn lightcraft_event(&mut self, e: LcEvent) {
        let retry = Some(("Try again", ToastAction::RetryLightCraft));
        let toast = match e {
            LcEvent::Port(port) => {
                self.settings.companions.lightcraft_port = Some(port);
                self.save_settings();
                return;
            }
            LcEvent::Starting => Some(toasts::note(
                Tone::Info,
                STARTING,
                "Starting LightCraft. It opens the library you used last.",
                None,
            )),
            LcEvent::Shown { front } => {
                self.lc_busy = false;
                (!front).then(|| {
                    let name = self.lc_photo.as_ref().map_or("", |d| &d.original_name);
                    toasts::note(
                        Tone::Success,
                        "Shown in LightCraft",
                        &format!("{name} is open in LightCraft's Detail view."),
                        None,
                    )
                })
            }
            LcEvent::NotInLibrary { sidecars } => {
                self.lc_busy = false;
                self.lc_add = self.lc_photo.clone().map(|d| (d, sidecars));
                None
            }
            LcEvent::Failed(f) => {
                self.lc_busy = false;
                Some(match f {
                    Failure::AlreadyOpen => toasts::note(
                        Tone::Warning,
                        "LightCraft is already open",
                        "MapCraft can only show photos in a LightCraft it started. Close \
                         LightCraft, then try again.",
                        retry,
                    ),
                    Failure::NoLibrary => toasts::note(
                        Tone::Warning,
                        "LightCraft has no library open",
                        "Open your library in LightCraft, then try again.",
                        retry,
                    ),
                    Failure::NoAnswer => toasts::note(
                        Tone::Error,
                        "LightCraft didn't answer",
                        "It may still be starting. Try again in a moment.",
                        retry,
                    ),
                    Failure::CouldNotStart(e) => {
                        error!("can't start LightCraft: {e}");
                        toasts::note(
                            Tone::Error,
                            "LightCraft couldn't be started",
                            "Check where it is in Settings.",
                            Some(("Settings…", ToastAction::OpenSettings)),
                        )
                    }
                    Failure::Refused(msg) => toasts::note(
                        Tone::Error,
                        "LightCraft couldn't show the photo",
                        &msg,
                        None,
                    ),
                })
            }
        };
        // The "starting" toast gives way to whatever comes next.
        self.toasts.remove_titled(STARTING);
        if let Some(t) = toast {
            self.toasts.replace(vec![t]);
        }
    }

    fn open_settings(&mut self, ctx: &egui::Context) {
        self.settings_dialog = Some(SettingsDialog::open(
            &self.settings.companions,
            self.settings.updates.check,
            self.settings.map.key(),
            self.launch,
            ctx,
        ));
        self.count_usage();
    }

    fn settings_dialog(&mut self, ctx: &egui::Context) {
        let blocked = self.data_blocked();
        let Some(dialog) = &mut self.settings_dialog else {
            return;
        };
        let here_ok =
            self.lib.is_some() && self.old_photos.is_some_and(|n| self.counts.photos >= n);
        let old = self
            .settings
            .old_data_folder
            .as_deref()
            .filter(|o| here_ok && o.exists());
        let view = DataView {
            folder: &self.data_folder,
            usage: self.data_usage.as_ref(),
            old: old.map(|o| (o, self.old_size)),
            blocked,
        };
        let shown = dialog.show(ctx, &self.palette, self.update_status, &view);
        match shown {
            Some(SettingsAction::Save {
                photocraft,
                lightcraft,
                check_updates,
                maptiler_key,
            }) => {
                self.settings.updates.check = check_updates;
                if maptiler_key != self.settings.map.maptiler_key {
                    self.settings.map.maptiler_key = maptiler_key;
                    // A removed key takes its styles with it; a new one is used at once.
                    let style = self.map.style();
                    let style = if style.needs_key() && self.settings.map.key().is_none() {
                        Style::Standard
                    } else {
                        style
                    };
                    self.set_map_style(style, ctx);
                }
                let c = &mut self.settings.companions;
                if c.lightcraft != lightcraft {
                    c.lightcraft_port = None;
                }
                c.photocraft = photocraft;
                c.lightcraft = lightcraft;
                self.save_settings();
                self.find_companions();
                self.settings_dialog = None;
            }
            Some(SettingsAction::Close) => self.settings_dialog = None,
            Some(SettingsAction::CheckNow) => {
                self.auto_check = false;
                self.check_updates(ctx);
            }
            Some(SettingsAction::Update) => {
                self.settings_dialog = None;
                self.open_update();
            }
            Some(SettingsAction::ChangeDataFolder(picked)) => {
                self.settings_dialog = None;
                self.pick_data_folder(&picked);
            }
            Some(SettingsAction::ShowDataFolder) => {
                if self.launch {
                    if let Err(e) = launch::spawn(Launch::Open, &self.data_folder) {
                        error!("can't show the data folder: {e}");
                    }
                } else {
                    self.update_calls.push("show data folder".into());
                }
            }
            Some(SettingsAction::FreeSpace) => {
                if let Some(u) = self.data_usage.clone() {
                    self.settings_dialog = None;
                    self.free_dialog = Some(FreeSpaceDialog::new(u));
                }
            }
            Some(SettingsAction::RemoveOldData) => {
                self.settings_dialog = None;
                self.remove_old_open = true;
            }
            None => {}
        }
    }

    /// Why the data folder can't change now, if it can't (design.md M8).
    fn data_blocked(&self) -> Option<&'static str> {
        if self.data_override {
            Some("This run uses --data, so the data folder can’t be changed here.")
        } else if self.geotag.is_some() || self.geo_work.is_some() {
            Some("Geotagging is open. Change the data folder when it’s done.")
        } else if self.job.is_some() {
            Some("An import is running. Change the data folder when it’s done.")
        } else if self.search.downloading() {
            Some("Place names are downloading. Change the data folder when they’re done.")
        } else {
            None
        }
    }

    /// Counts the data folder (and the old one, after a move) on a background thread.
    fn count_usage(&mut self) {
        self.data_usage = None;
        let (data, old, tx, ctx) = (
            self.data_folder.clone(),
            self.settings.old_data_folder.clone(),
            self.data_events.0.clone(),
            self.ctx.clone(),
        );
        std::thread::spawn(move || {
            let u = datafolder::usage(&data);
            let old = old.filter(|o| o.exists()).map(|o| {
                (
                    datafolder::usage(&o).total().bytes,
                    datafolder::library_photos(&o),
                )
            });
            let _ = tx.send(DataEvent::Counted(u, old));
            ctx.request_repaint();
        });
    }

    /// A folder was picked as the new data folder: what it means (canvas artboard 2).
    fn pick_data_folder(&mut self, picked: &std::path::Path) {
        let stage = if let Some(why) = self.data_blocked() {
            ChangeStage::Failed(why.into())
        } else {
            match datafolder::classify(&self.data_folder, picked) {
                Target::Refused(why) => ChangeStage::Failed(why),
                Target::Empty { dir, inside } => ChangeStage::Choose {
                    network: datafolder::is_network(&dir),
                    dir,
                    inside,
                    picked: picked.to_path_buf(),
                },
                Target::Existing { dir, photos } => ChangeStage::Existing { dir, photos },
            }
        };
        let size = self.data_usage.as_ref().map(|u| u.total().bytes);
        self.change_dialog = Some(ChangeDialog::new(stage, size));
    }

    fn change_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.change_dialog else {
            return;
        };
        let Some(action) = d.show(ctx, &self.palette) else {
            return;
        };
        let stage = d.stage.clone();
        match (action, stage) {
            (ChangeAction::Move, ChangeStage::Choose { dir, .. }) => {
                if let Some(t) = &self.thumbs {
                    // The thumbnail job writes into the library and the folder: it pauses
                    // first, and the copy starts once it has stopped.
                    t.cancel();
                    self.data_waiting = Some(DataTask::Copy(dir));
                    self.set_change_stage(ChangeStage::Pausing);
                } else {
                    self.start_copy(dir);
                }
            }
            (ChangeAction::StartEmpty, ChangeStage::Choose { dir, .. }) => {
                self.switch_data_folder(dir, false, ctx);
            }
            (ChangeAction::UseExisting, ChangeStage::Existing { dir, .. }) => {
                self.switch_data_folder(dir, false, ctx);
            }
            (ChangeAction::ChooseAnother, _) => {
                self.change_dialog = None;
                self.open_settings(ctx);
                if let Some(s) = &mut self.settings_dialog {
                    s.section = Section::Data;
                }
            }
            (ChangeAction::Cancel, ChangeStage::Copying { .. }) => {
                // The dialog closes once the copy has stopped and cleaned up.
                self.data_cancel.store(true, Ordering::Relaxed);
            }
            (ChangeAction::Cancel, ChangeStage::Pausing) => {
                self.data_waiting = None;
                self.change_dialog = None;
                self.start_thumbs();
            }
            (ChangeAction::Cancel, _) => self.change_dialog = None,
            _ => {}
        }
    }

    fn set_change_stage(&mut self, stage: ChangeStage) {
        if let Some(d) = &mut self.change_dialog {
            d.stage = stage;
        }
    }

    /// Copies the data to `dir` on a background thread (canvas artboard 2, Copying).
    fn start_copy(&mut self, dir: PathBuf) {
        self.set_change_stage(ChangeStage::Copying {
            dir: dir.clone(),
            at: None,
        });
        self.data_cancel.store(false, Ordering::Relaxed);
        let (from, tx, ctx, cancel) = (
            self.data_folder.clone(),
            self.data_events.0.clone(),
            self.ctx.clone(),
            self.data_cancel.clone(),
        );
        std::thread::spawn(move || {
            let progress = |p| {
                let _ = tx.send(DataEvent::Progress(p));
                ctx.request_repaint();
                !cancel.load(Ordering::Relaxed)
            };
            let r = datafolder::copy(&from, &dir, &progress);
            if let Err(CopyError::Failed(e)) = &r {
                error!("copying the data folder: {e}");
            }
            let _ = tx.send(DataEvent::Copied(r));
            ctx.request_repaint();
        });
    }

    /// Makes `dir` the data folder and restarts there. The setting must be saved first: if it
    /// can't be, nothing switches (and a copy made for the move is removed again).
    fn switch_data_folder(&mut self, dir: PathBuf, moved: bool, ctx: &egui::Context) {
        let mut new = self.settings.clone();
        new.data_folder = Some(dir.clone());
        if moved {
            new.old_data_folder = Some(self.data_folder.clone());
        } else if new.old_data_folder.as_deref().is_some_and(|o| {
            datafolder::same_or_inside(&dir, o) || datafolder::same_or_inside(o, &dir)
        }) {
            new.old_data_folder = None;
        }
        let saved = match &self.settings_file {
            Some(f) => new.save_to(f).map_err(|e| e.to_string()),
            None => Err("the settings file couldn’t be read at start".into()),
        };
        if let Err(e) = saved {
            error!("can't save the new data folder: {e}");
            if moved {
                let _ = datafolder::remove_owned(&dir, true);
            }
            self.set_change_stage(ChangeStage::Failed(format!(
                "MapCraft can’t save its settings ({e}), so the data folder stays where it is."
            )));
            return;
        }
        info!("data folder changes; restarting");
        self.settings = new;
        if self.launch {
            update::request_restart();
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        } else {
            self.update_calls.push("restart".into());
        }
    }

    fn free_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &mut self.free_dialog else {
            return;
        };
        let Some(action) = d.show(ctx, &self.palette) else {
            return;
        };
        self.free_dialog = None;
        let FreeAction::Delete(kinds) = action else {
            return;
        };
        match &self.thumbs {
            Some(t) if kinds.contains(&Kind::Thumbnails) => {
                t.cancel();
                self.data_waiting = Some(DataTask::Purge(kinds));
            }
            _ => self.start_purge(kinds),
        }
    }

    /// Deletes what MapCraft makes again, on a background thread. Thumbnails are marked as not
    /// made in the library first, then their files go.
    fn start_purge(&mut self, kinds: Vec<Kind>) {
        let (data, db, tx, ctx) = (
            self.data_folder.clone(),
            self.db_path.clone(),
            self.data_events.0.clone(),
            self.ctx.clone(),
        );
        std::thread::spawn(move || {
            if kinds.contains(&Kind::Thumbnails) {
                match Library::open(&db).and_then(|mut l| l.forget_thumbnails()) {
                    Ok(n) => info!("{n} thumbnails to be made again"),
                    Err(e) => {
                        error!("can't reset the thumbnails: {e}");
                        let _ = tx.send(DataEvent::PurgeFailed);
                        ctx.request_repaint();
                        return;
                    }
                }
            }
            let freed = datafolder::purge(&data, &kinds);
            let _ = tx.send(DataEvent::Purged(freed, kinds));
            ctx.request_repaint();
        });
    }

    fn remove_old_dialog(&mut self, ctx: &egui::Context) {
        if !self.remove_old_open {
            return;
        }
        let Some(old) = self.settings.old_data_folder.clone() else {
            self.remove_old_open = false;
            return;
        };
        let Some(remove) = crate::ui::data_dialogs::remove_old_dialog(
            ctx,
            &self.palette,
            &old,
            &self.data_folder,
            self.old_size,
        ) else {
            return;
        };
        self.remove_old_open = false;
        if !remove {
            return;
        }
        // Never the folder in use, nor one holding it or held by it.
        if datafolder::same_or_inside(&old, &self.data_folder)
            || datafolder::same_or_inside(&self.data_folder, &old)
        {
            self.settings.old_data_folder = None;
            self.save_settings();
            return;
        }
        let (tx, c) = (self.data_events.0.clone(), ctx.clone());
        std::thread::spawn(move || {
            let r = datafolder::remove_owned(&old, true).map_err(|e| e.to_string());
            let _ = tx.send(DataEvent::Removed(r));
            c.request_repaint();
        });
    }

    /// Takes the data folder work's news, and starts work that waited for the thumbnails.
    fn poll_data(&mut self, ctx: &egui::Context) {
        if self.thumbs.is_none()
            && let Some(task) = self.data_waiting.take()
        {
            match task {
                DataTask::Copy(dir) => self.start_copy(dir),
                DataTask::Purge(kinds) => self.start_purge(kinds),
            }
        }
        let events: Vec<DataEvent> = self.data_events.1.try_iter().collect();
        for e in events {
            match e {
                DataEvent::Counted(u, old) => {
                    if let Some(d) = &mut self.change_dialog
                        && d.size.is_none()
                    {
                        d.size = Some(u.total().bytes);
                    }
                    self.data_usage = Some(u);
                    self.old_size = old.map(|o| o.0);
                    self.old_photos = old.and_then(|o| o.1);
                }
                DataEvent::Progress(p) => {
                    if let Some(d) = &mut self.change_dialog
                        && let ChangeStage::Copying { at, .. } = &mut d.stage
                    {
                        *at = Some(p);
                    }
                }
                DataEvent::Copied(r) => {
                    let dir = match self.change_dialog.as_ref().map(|d| &d.stage) {
                        Some(ChangeStage::Copying { dir, .. }) => Some(dir.clone()),
                        _ => None,
                    };
                    match (r, dir) {
                        (Ok(()), Some(dir)) => self.switch_data_folder(dir, true, ctx),
                        (Err(CopyError::Failed(e)), _) => {
                            self.set_change_stage(ChangeStage::Failed(e));
                            self.start_thumbs();
                        }
                        _ => {
                            self.change_dialog = None;
                            self.start_thumbs();
                        }
                    }
                }
                DataEvent::Purged(freed, kinds) => {
                    if !kinds.is_empty() {
                        let names: Vec<&str> = kinds
                            .iter()
                            .map(|k| match k {
                                Kind::Previews => "detail previews",
                                Kind::Tiles => "map tiles",
                                _ => "thumbnails",
                            })
                            .collect();
                        let what = match names.as_slice() {
                            [one] => one.to_string(),
                            [a, b] => format!("{a} and {b}"),
                            [rest @ .., last] => format!("{} and {last}", rest.join(", ")),
                            [] => String::new(),
                        };
                        let what: String = {
                            let mut c = what.chars();
                            c.next()
                                .map(|f| f.to_uppercase().chain(c).collect())
                                .unwrap_or_default()
                        };
                        self.toasts.push(toasts::note(
                            Tone::Success,
                            &format!("Freed {}", datafolder::bytes_text(freed.bytes)),
                            &format!("{what} are made again when they’re needed."),
                            None,
                        ));
                    }
                    if kinds.contains(&Kind::Thumbnails) {
                        self.refresh();
                        self.start_thumbs();
                    }
                    self.count_usage();
                }
                DataEvent::PurgeFailed => {
                    self.toasts.push(toasts::note(
                        Tone::Error,
                        "Nothing was deleted",
                        "MapCraft couldn’t update its library. Try again later.",
                        None,
                    ));
                    self.start_thumbs();
                }
                DataEvent::Removed(r) => {
                    match r {
                        Ok(()) => {
                            self.settings.old_data_folder = None;
                            self.save_settings();
                            self.toasts.push(toasts::note(
                                Tone::Success,
                                "Old data removed",
                                "MapCraft’s files are gone from the old data folder.",
                                None,
                            ));
                        }
                        Err(e) => {
                            error!("removing the old data: {e}");
                            self.toasts.push(toasts::note(
                                Tone::Error,
                                "Some old data couldn’t be removed",
                                "A file may be in use. Try again later.",
                                None,
                            ));
                        }
                    }
                    self.count_usage();
                }
            }
        }
    }

    fn check_updates(&mut self, ctx: &egui::Context) {
        self.update_status = CheckStatus::Checking;
        self.updater.check(ctx);
    }

    /// The work a restart waits for, if any runs (canvas `MapCraft — Updates`, artboard 3).
    fn update_waits_for(&self) -> Option<String> {
        if self.job.is_some() {
            Some("An import is running".into())
        } else if self.geo_work.is_some() {
            Some("Positions are being written".into())
        } else if self.geotag.is_some() {
            Some("Geotagging is open".into())
        } else if self.thumbs.is_some() {
            Some("Thumbnails are being made".into())
        } else {
            None
        }
    }

    /// Takes the updater's news: a check's answer, the download's progress and outcome.
    fn poll_updates(&mut self, ctx: &egui::Context) {
        for e in self.updater.events() {
            match e {
                update::Event::Checked(Ok(Some(r))) if r.version > Version::current() => {
                    self.update_status = CheckStatus::Available(r.version);
                    if self.auto_check && self.update_dialog.is_none() {
                        self.toasts.push(Toast {
                            tone: Tone::Info,
                            title: format!("MapCraft {} is available", r.version),
                            text: format!("You have {}.", Version::current()),
                            action: Some(("Update…".into(), ToastAction::Update)),
                            second: Some(("What’s new".into(), ToastAction::ReleaseNotes)),
                        });
                    }
                    self.offer = Some(r);
                }
                update::Event::Checked(Ok(_)) => self.update_status = CheckStatus::Latest,
                update::Event::Checked(Err(_)) => self.update_status = CheckStatus::Failed,
                update::Event::Progress(done, total) => {
                    if let Some(d) = &mut self.update_dialog
                        && matches!(d.stage, Stage::Downloading { .. })
                    {
                        d.stage = Stage::Downloading { done, total };
                    }
                }
                update::Event::Downloaded(r) => {
                    let Some(d) = &mut self.update_dialog else {
                        // Cancelled meanwhile: a finished file isn't wanted.
                        if let Ok(f) = r {
                            let _ = std::fs::remove_file(f);
                        }
                        continue;
                    };
                    match r {
                        Ok(f) => {
                            self.downloaded = Some(f);
                            d.stage = Stage::Ready { waiting: None };
                        }
                        Err(DownloadError::Cancelled) => self.update_dialog = None,
                        Err(DownloadError::Checksum) => d.stage = Stage::Failed { checksum: true },
                        Err(DownloadError::Failed(_)) => {
                            d.stage = Stage::Failed { checksum: false }
                        }
                    }
                }
            }
        }
        // Ready: restart now, or once the running work is done.
        let waits = self.update_waits_for();
        let mut restart = false;
        if let Some(d) = &mut self.update_dialog
            && let Stage::Ready { waiting } = &mut d.stage
        {
            if waits.is_none() {
                restart = true;
            } else {
                *waiting = waits;
            }
        }
        if restart {
            if let Some(d) = &mut self.update_dialog {
                d.stage = Stage::Ready { waiting: None };
            }
            self.finish_update(true, ctx);
        }
    }

    /// Opens the update dialog for the release on offer: its confirmation, or why MapCraft
    /// can't update itself where it is.
    fn open_update(&mut self) {
        let Some(r) = self.offer.clone() else { return };
        if self.update_pending {
            // Downloaded already; a second download would overwrite the waiting file.
            self.toasts.push(update_ready(r.version));
            return;
        }
        self.toasts
            .remove_titled(&format!("MapCraft {} is available", r.version));
        let dir = exe_dir();
        let stage = match &dir {
            Some(d) if update::folder_writable(d) => Stage::Confirm,
            _ => Stage::NotWritable {
                folder: dir.map_or_else(|| "its folder".into(), |d| d.display().to_string()),
            },
        };
        self.update_dialog = Some(UpdateDialog::new(r, stage));
    }

    fn update_dialog(&mut self, ctx: &egui::Context) {
        let Some(d) = &self.update_dialog else { return };
        let Some(action) = d.show(ctx, &self.palette) else {
            return;
        };
        let release = d.release.clone();
        match action {
            UpdateAction::Start => {
                let Some(dir) = exe_dir() else { return };
                self.updater.download(&release, &dir, ctx);
                if let Some(d) = &mut self.update_dialog {
                    d.stage = Stage::Downloading {
                        done: 0,
                        total: release.exe_size,
                    };
                }
            }
            UpdateAction::Cancel => {
                self.updater.cancel();
                self.update_dialog = None;
            }
            UpdateAction::Close => self.update_dialog = None,
            UpdateAction::RestartNow => self.finish_update(true, ctx),
            UpdateAction::RestartLater => {
                self.finish_update(false, ctx);
                self.update_pending = true;
                self.toasts.push(update_ready(release.version));
            }
            UpdateAction::ReleasePage => self.release_notes(),
        }
    }

    /// Hands the verified download to `main`, which swaps it in once the window has closed,
    /// and closes the window when `restart`.
    fn finish_update(&mut self, restart: bool, ctx: &egui::Context) {
        let Some(f) = self.downloaded.take() else {
            return;
        };
        if !restart {
            self.update_dialog = None;
        }
        if self.launch {
            update::set_pending(f, restart);
            if restart {
                ctx.send_viewport_cmd(egui::ViewportCommand::Close);
            }
        } else {
            self.update_calls
                .push(format!("install, restart: {restart}"));
        }
    }

    fn release_notes(&mut self) {
        let Some(r) = &self.offer else { return };
        if self.launch {
            if let Err(e) = launch::open_url(&r.page_url) {
                error!("can't open the release page: {e}");
            }
        } else {
            self.update_calls.push(format!("open {}", r.page_url));
        }
    }

    /// Asks whether to add a photo to LightCraft's library (canvas artboard 4).
    fn add_dialog(&mut self, ctx: &egui::Context) {
        let Some((d, sidecars)) = &self.lc_add else {
            return;
        };
        let p = &self.palette;
        let (mut add, mut cancel) = (false, false);
        let close = widgets::dialog(
            ctx,
            p,
            "lightcraft-add",
            "Add this photo to LightCraft?",
            460.0,
            true,
            |ui| {
                ui.label(
                    RichText::new(format!(
                        "{} isn't in your LightCraft library yet.",
                        d.original_name
                    ))
                    .color(p.text_strong),
                );
                ui.label(
                    RichText::new(
                        "LightCraft adds it where it is: nothing is copied, moved or renamed. \
                         Then MapCraft shows it there.",
                    )
                    .color(p.text_weak),
                );
                if *sidecars {
                    let stem = std::path::Path::new(&d.original_name)
                        .file_stem()
                        .map_or(String::new(), |s| s.to_string_lossy().into_owned());
                    ui.add_space(p.space_1);
                    ui.horizontal_top(|ui| {
                        ui.add(Tone::Warning.icon().image(p.size_icon_sm, p.status_warning));
                        ui.label(
                            RichText::new(format!(
                                "Your LightCraft library writes changes into XMP sidecars: once \
                                 you edit this photo there, LightCraft puts an {stem}.xmp next \
                                 to it."
                            ))
                            .color(p.text_normal),
                        );
                    });
                }
            },
            |ui| {
                add = widgets::primary(ui, p, None, "Add to LightCraft", true).clicked();
                cancel = widgets::secondary(ui, p, None, "Cancel", true).clicked();
            },
        );
        if add {
            let (d, _) = self.lc_add.take().expect("shown");
            self.show_in_lightcraft(Some(d), true);
        } else if cancel || close {
            self.lc_add = None;
        }
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
            self.thumbs_for.clone(),
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
            } else if std::mem::take(&mut self.restart_thumbs) {
                self.start_thumbs(); // again, the geotag candidates first
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
        self.poll_updates(&ctx);
        self.poll_data(&ctx);
        self.poll_geotag();
        self.poll_map(&ctx);

        let p = self.palette.clone();
        let running = self.running();
        let counts = self.counts;
        let mut bar_action = None;
        let tagging = top_bar::Tagging {
            on: self.geotag.is_some(),
            blocked: self.geotag_blocked(),
        };
        let bar = Panel::top("top-bar")
            .exact_size(p.size_topbar)
            .frame(Frame::NONE.fill(p.bg_panel))
            .show(ui, |ui| {
                let filters = BarFilters {
                    panel: &mut self.filters,
                    active: self.filter.active_count(),
                    shown: self.filter.is_active().then_some(self.shown),
                };
                bar_action =
                    top_bar::show(ui, &p, counts, running, &mut self.search, filters, tagging)
            });
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
            Some(TopBarAction::Filters) => self.search.close(),
            Some(TopBarAction::Settings) => self.open_settings(&ctx),
            Some(TopBarAction::Geotag) => {
                if let Some(files) = self.pick_gpx() {
                    self.start_geotag(files);
                }
            }
            None => {}
        }
        // The search dropdown and the panel would overlap: the newer one stays.
        if self.search.is_open() && self.filters.open {
            self.filters.open = false;
        }
        // The filter panel takes Esc first while open, before the strip and the selection.
        if self.geotag.is_none()
            && self.filters.panel(
                &ctx,
                &p,
                &mut self.filter,
                &self.facets,
                &self.sources,
                self.shown,
                self.counts.with_position.max(0) as usize,
            )
        {
            self.filter_changed();
        }

        while let Ok(t) = self.launch_errors.1.try_recv() {
            self.toasts.replace(vec![t]);
        }
        self.poll_lightcraft();

        let empty = self.counts.photos == 0 && self.job.is_none();
        let dialog_open = self.folder_dialog.is_some()
            || self.sources_dialog.is_some()
            || self.narrow.is_some()
            || self.settings_dialog.is_some()
            || self.update_dialog.is_some()
            || self.change_dialog.is_some()
            || self.free_dialog.is_some()
            || self.remove_old_open
            || self.lc_add.is_some()
            || self.geo_confirm
            || self.geo_leave;

        // Keys: Ctrl+I shows or hides the panel, Esc closes the strip or clears the selection.
        if !dialog_open && !ctx.egui_wants_keyboard_input() {
            let (ctrl_i, esc) = ctx.input_mut(|i| {
                (
                    i.consume_key(egui::Modifiers::COMMAND, egui::Key::I),
                    i.consume_key(egui::Modifiers::NONE, egui::Key::Escape),
                )
            });
            if self.geotag.is_some() {
                if esc && self.geo_work.is_none() {
                    self.geotag_escape();
                }
            } else {
                if ctrl_i && self.detail.is_some() {
                    self.panel_open = !self.panel_open;
                }
                // Esc closes the strip first, then clears the selection.
                if esc && !self.map.close_strip() && self.detail.is_some() {
                    self.select(None);
                }
            }
        }

        // The detail panel, right of the map (design system "Map layout": 320 px).
        let mut panel_action = None;
        let apps = self
            .detail
            .as_ref()
            .map(|d| self.companion_buttons(d))
            .unwrap_or_default();
        // Tagging mode's panel takes the detail panel's place (design.md §2.9).
        let mut geo_action = None;
        if let Some(s) = &self.geotag {
            let writing = self
                .geo_work
                .as_ref()
                .filter(|w| !w.undo)
                .map(|w| w.progress.unwrap_or((0, 0)));
            let shown = Panel::right("geotag")
                .exact_size(p.size_panel)
                .resizable(false)
                .show_separator_line(false)
                .frame(Frame::NONE.fill(p.bg_panel))
                .show(ui, |ui| {
                    let last = self.geo_last.as_ref().map(|l| l.1.as_str());
                    geo_action = crate::ui::geotag_panel::show(ui, &p, s, writing, last);
                });
            let r = shown.response.rect;
            ui.painter().vline(
                r.left() + p.stroke_hairline / 2.0,
                r.y_range(),
                Stroke::new(p.stroke_hairline, p.border_subtle),
            );
        }
        if let Some(a) = geo_action {
            self.geotag_action(a);
        }
        if self.geotag.is_none()
            && self.panel_open
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
                        // Can't be made now (the source offline): the thumbnail, if there's one.
                        Some(Err(())) if d.thumb_made => match self.map.thumbnail(d.id) {
                            Some(t) => PreviewFace::Image(t),
                            None => PreviewFace::None,
                        },
                        Some(Err(())) => PreviewFace::None,
                        None => PreviewFace::Loading(self.map.thumbnail(d.id)),
                    };
                    panel_action = self.panel.show(ui, &p, d, face, &apps);
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
            Some(PanelAction::PhotoCraft) => self.open_in_photocraft(),
            Some(PanelAction::LightCraft) => self.show_in_lightcraft(None, false),
            None => {}
        }

        let mut map_rect = egui::Rect::NOTHING;
        let mut map_action = None;
        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.map_ref_land))
            .show(ui, |ui| {
                let keys = !dialog_open && self.geo_work.is_none();
                let (rect, action) = self.map.show(ui, &p, empty && self.geotag.is_none(), keys);
                map_rect = rect;
                map_action = action;
            });
        match map_action {
            Some(MapAction::Style(s)) => self.set_map_style(s, &ctx),
            Some(MapAction::MapSettings) => {
                if self.geotag.is_none() {
                    self.open_settings(&ctx);
                    if let Some(d) = &mut self.settings_dialog {
                        d.section = Section::Map;
                    }
                }
            }
            Some(a) if self.geotag.is_some() => {
                if self.geo_work.is_none() {
                    self.geotag_map(a);
                }
            }
            Some(MapAction::AddFolder) => self.folder_dialog = Some(FolderDialog::add()),
            Some(MapAction::Select(id)) => self.select(id),
            Some(MapAction::Toggle(_) | MapAction::Picked(..)) | None => {}
        }
        self.geotag_track_tip(&ctx);
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
        // The active filters over the map, and the card when they hide everything.
        let on_map = self.counts.with_position.max(0) as usize;
        let tagging = self.geotag.is_some();
        let mut filter_action = if tagging {
            None
        } else {
            filters::chips(
                &ctx,
                &p,
                map_rect,
                &mut self.filter,
                &self.facets,
                &self.sources,
            )
        };
        if !tagging && self.filter.is_active() && self.shown == 0 && on_map > 0 && !dialog_open {
            filter_action = filter_action.or(filters::no_match(
                &ctx,
                &p,
                map_rect,
                &mut self.filter,
                on_map,
            ));
        }
        match filter_action {
            Some(FiltersAction::Changed) => self.filter_changed(),
            Some(FiltersAction::OpenPanel) => {
                self.search.close();
                self.filters.open = true;
            }
            None => {}
        }
        match self.search.dropdown(&ctx, &p) {
            Some(SearchAction::GoTo { lat, lon, zoom }) => self.map.go_to(lat, lon, zoom),
            Some(SearchAction::ZoomTo(b)) => self.map.zoom_to(b),
            Some(SearchAction::Photo { id, lat, lon }) => {
                // Close enough to see it; it may still sit in a group (marked as selected).
                self.map.go_to(lat, lon, self.map.zoom().max(15.0));
                self.select(Some(id));
            }
            None => {}
        }
        match self.toasts.show(&ctx, &p, map_rect) {
            Some(ToastAction::ZoomTo(b)) => self.map.zoom_to(b),
            Some(ToastAction::RetryLightCraft) => {
                self.toasts.replace(Vec::new());
                let d = self.lc_photo.clone();
                self.show_in_lightcraft(d, false);
            }
            Some(ToastAction::Update) => self.open_update(),
            Some(ToastAction::ReleaseNotes) => self.release_notes(),
            Some(ToastAction::OpenSettings) => {
                self.toasts.replace(Vec::new());
                self.open_settings(&ctx);
            }
            Some(ToastAction::OpenMapSettings) => {
                self.toasts.replace(Vec::new());
                self.open_settings(&ctx);
                if let Some(d) = &mut self.settings_dialog {
                    d.section = Section::Map;
                }
            }
            Some(ToastAction::UndoGeotag) => {
                self.toasts.replace(Vec::new());
                self.undo_geotag();
            }
            None => {}
        }

        self.folder_dialog(&ctx);
        self.sources_dialog(&ctx);
        self.narrow_dialog(&ctx);
        self.settings_dialog(&ctx);
        self.change_dialog(&ctx);
        self.free_dialog(&ctx);
        self.remove_old_dialog(&ctx);
        self.update_dialog(&ctx);
        self.add_dialog(&ctx);
        self.geotag_confirm_dialog(&ctx);
        self.geotag_leave_dialog(&ctx);

        if let Some((since, total, frames)) = &mut self.perf {
            *total += started.elapsed();
            *frames += 1;
            if since.elapsed() >= Duration::from_secs(3) {
                info!(
                    "perf: {} photos, {:.2} ms per frame over {frames} frames",
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
                                network: false,
                                settings: no_apps(),
                                settings_file: None,
                                check_updates: false,
                                update_endpoint: crate::update::Endpoint::github(),
                                data_override: false,
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
        // Removing the source deletes the kept preview with the thumbnails.
        let source = h.state().as_ref().unwrap().sources[0].id;
        h.state_mut()
            .as_mut()
            .unwrap()
            .change(Change::Remove(source));
        step_until(&mut h, "the sweep", |h| {
            h.state().as_ref().unwrap().thumbs.is_none()
                && !thumbs::path(dir.path(), id, thumbs::PREVIEW).exists()
        });
    }

    /// Waits until thumbnails are made and the groups built for them.
    fn settled(h: &mut Harness<'static, Option<MapCraftApp>>) {
        step_until(h, "thumbnails and groups", |h| {
            let app = h.state().as_ref().unwrap();
            app.thumbs.is_none() && app.map.grouped()
        });
    }

    /// The markers in view: (photos, groups).
    fn markers(h: &Harness<'static, Option<MapCraftApp>>) -> (usize, usize) {
        (
            h.query_all_by_label_contains("Photo").count(),
            h.query_all_by_label_contains("Group of").count(),
        )
    }

    #[test]
    fn clicking_a_group_zooms_in_until_it_falls_apart() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 40); // five photos around each of seven cities
        let mut h = harness(dir.path());
        settled(&mut h);
        h.run_steps(2);
        assert_eq!(markers(&h), (0, 7), "a group per city at continent zoom");
        h.get_all_by_label("Group of 5 photos")
            .next()
            .unwrap()
            .click();
        step_until(&mut h, "the zoom", |h| {
            let (photos, groups) = markers(h);
            photos + groups >= 2
        });
        assert_eq!(
            h.query_all_by_label("Group of 5 photos").count(),
            0,
            "the group fell apart (and the other cities are out of view)"
        );
    }

    #[test]
    fn a_group_at_one_spot_opens_its_strip() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        // Twelve photos at one spot, a minute apart; the last on the next day.
        for i in 0..12 {
            let date = if i == 11 {
                "2023:05:20 08:00:00".to_owned()
            } else {
                format!("2023:05:19 21:{i:02}:00")
            };
            let jpeg = crate::demo::photo_with_gps(53.5436, 9.9893, &date, i as f64 / 12.0);
            std::fs::write(photos.join(format!("SPOT_{i:02}.jpg")), jpeg).unwrap();
        }
        {
            let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
            let src = lib
                .add_source(&photos, None, "Spot", &Scope::all())
                .unwrap();
            crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        }
        let mut h = harness(dir.path());
        settled(&mut h);
        // Zoomed far out, where the view is wider than the world, the group still shows.
        h.state_mut().as_mut().unwrap().map.set_zoom(1.0);
        h.run_steps(3);
        h.get_by_label("Group of 12 photos").click();
        step_until(&mut h, "the strip", |h| {
            h.query_by_label("12 photos at this spot").is_some()
                && h.query_by_label("Photo SPOT_01.jpg").is_some()
        });
        h.get_by_label("19 May – 20 May 2023");
        // Eight per page, oldest first; the next page has the rest.
        assert!(h.query_by_label("Photo SPOT_08.jpg").is_none());
        h.get_by_label("Next photos").click();
        step_until(&mut h, "the next page", |h| {
            h.query_by_label("Photo SPOT_08.jpg").is_some()
        });
        assert!(h.query_by_label("Photo SPOT_01.jpg").is_none());
        h.get_by_label("Photo SPOT_11.jpg").click();
        step_until(&mut h, "the panel", |h| {
            h.query_by_label("Photo details").is_some()
        });
        assert!(
            h.query_by_label("12 photos at this spot").is_some(),
            "the strip stays"
        );
        // Esc closes the strip first, then clears the selection.
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(h.query_by_label("12 photos at this spot").is_none());
        assert!(h.query_by_label("Photo details").is_some());
        // The group holding the selected photo stays marked as selected.
        assert!(
            h.state_mut()
                .as_mut()
                .unwrap()
                .map
                .selected_group()
                .is_some()
        );
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(h.query_by_label("Photo details").is_none());
    }

    #[test]
    fn searching_finds_places_and_photos() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 40);
        let mut h = harness(dir.path());
        settled(&mut h);
        // Without the place index, the dropdown offers the download: only on a click. Enter
        // goes to the first result (the trip's folder), never to the download.
        h.get_by_label("Search").click();
        h.run_steps(2);
        h.get_by_label("Search").type_text("hamb");
        step_until(&mut h, "the results", |h| {
            h.query_by_label("Download place names (14 MB, from geonames.org)")
                .is_some()
        });
        h.get_by_label_contains("2023-05-19_Hamburg"); // the demo trip's folder
        h.key_press(egui::Key::Enter);
        h.run_steps(3);
        let app = h.state().as_ref().unwrap();
        assert_eq!(app.search.download_asked, 0, "Enter never downloads");
        assert!(!app.search.is_open(), "Enter went to the folder");
        h.state_mut().as_mut().unwrap().search.type_in("hamb");
        step_until(&mut h, "the results again", |h| {
            h.query_by_label("Download place names (14 MB, from geonames.org)")
                .is_some()
        });
        h.get_by_label("Download place names (14 MB, from geonames.org)")
            .click();
        h.run_steps(3);
        assert_eq!(h.state().as_ref().unwrap().search.download_asked, 1);

        // With an index (a few rows of our own), places show, ranked by photos nearby.
        let index = crate::places::index_path(dir.path());
        std::fs::create_dir_all(index.parent().unwrap()).unwrap();
        crate::places::build(
            crate::places::tests::CITIES.as_bytes(),
            crate::places::tests::ADMIN1.as_bytes(),
            crate::places::tests::COUNTRIES.as_bytes(),
            &index,
        )
        .unwrap();
        let search = &mut h.state_mut().as_mut().unwrap().search;
        search.reopen_for_tests();
        search.type_in("hamb");
        step_until(&mut h, "the place results", |h| {
            h.query_by_label("Hamburg, Germany").is_some()
        });
        h.get_by_label("Hamburg, Germany").click();
        h.run_steps(3);
        let app = h.state().as_ref().unwrap();
        assert_eq!(app.map.zoom(), 11.0, "a city's zoom");
        assert!(!app.search.is_open());

        // A file: Enter goes to the first result and selects the photo.
        h.state_mut().as_mut().unwrap().search.type_in("IMG_00012");
        step_until(&mut h, "the file", |h| {
            h.query_by_label("IMG_00012.jpg").is_some()
        });
        h.key_press(egui::Key::Enter);
        step_until(&mut h, "the panel", |h| {
            h.query_by_label("Photo details").is_some()
        });
        assert!(!h.state().as_ref().unwrap().search.is_open());
    }

    #[test]
    fn filters_narrow_the_map_and_say_so() {
        let dir = tempfile::tempdir().unwrap();
        // 24 demo photos, 21 on the map; trips by year, cameras taking turns.
        imported(dir.path(), 24);
        let mut h = harness(dir.path());
        settled(&mut h);
        let shown = |h: &Harness<'static, Option<MapCraftApp>>| h.state().as_ref().unwrap().shown;
        let click_first = |h: &mut Harness<'static, Option<MapCraftApp>>, label: &str| {
            // A checklist row is named twice (box and text): either toggles it.
            h.query_all_by_label(label).next().unwrap().click();
            h.run_steps(3);
        };
        h.get_by_label("Filters").click();
        h.run_steps(3);
        h.get_by_label("21 of 21 photos on the map");
        h.get_by_label("Date from");
        h.get_by_label("Date to");

        // A year: the map, the counts and the chip follow.
        h.get_by_label("2022").click();
        h.run_steps(3);
        assert_eq!(shown(&h), 3);
        h.get_by_label_contains("3 of 21 photos shown");
        assert!(h.query_all_by_label("Jan 2022 – Dec 2022").count() >= 1);
        // Older years wait behind Earlier….
        assert!(h.query_by_label("2019").is_none());
        h.get_by_label("Earlier…").click();
        h.run_steps(3);
        h.get_by_label("2019");
        step_until(&mut h, "the groups", |h| {
            h.state().as_ref().unwrap().map.grouped()
        });

        // Cameras: hiding one names it on the chip.
        click_first(&mut h, "Apple iPhone 13 mini");
        assert_eq!(shown(&h), 2);
        h.get_by_label("All cameras but Apple iPhone 13 mini");
        h.get_by_label("Filters (2 set)");

        // Esc closes the panel first.
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        assert!(!h.state().as_ref().unwrap().filters.open);
        assert!(h.query_by_label("21 of 21 photos on the map").is_none());

        // Hiding every camera leaves nothing: the card offers to clear.
        h.get_by_label("Filters (2 set)").click();
        h.run_steps(3);
        click_first(&mut h, "Canon EOS R5");
        click_first(&mut h, "FUJIFILM X-T3");
        assert_eq!(shown(&h), 0);
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        h.get_by_label("No photo matches the filters");
        h.get_by_label("Clear all filters").click();
        h.run_steps(3);
        assert_eq!(shown(&h), 21);
        assert!(!h.state().as_ref().unwrap().filter.is_active());
        assert!(h.query_by_label("No photo matches the filters").is_none());
        h.get_by_label("Filters");

        // A chip's remove button takes its filter away.
        h.get_by_label("Filters").click();
        h.run_steps(3);
        h.get_by_label("2021").click();
        h.run_steps(3);
        assert_eq!(shown(&h), 3);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        h.get_by_label("Remove the date filter").click();
        h.run_steps(3);
        assert_eq!(shown(&h), 21);
    }

    #[test]
    fn file_types_filter_by_the_original() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 24); // 21 on the map, all JPEG
        {
            // Make some of them RAW and DNG originals (the demo writes only JPEGs).
            let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
            let tx = lib.transaction().unwrap();
            let raws = tx
                .execute(
                    "UPDATE photo SET original_name = stem || '.CR2', original_ext = 'cr2'
                     WHERE id IN (SELECT id FROM photo WHERE lat IS NOT NULL ORDER BY id LIMIT 5)",
                    [],
                )
                .unwrap();
            assert_eq!(raws, 5);
            tx.execute(
                "UPDATE photo SET original_name = stem || '.dng', original_ext = 'dng'
                 WHERE id IN (SELECT id FROM photo WHERE lat IS NOT NULL
                              AND original_name NOT LIKE '%.CR2' ORDER BY id LIMIT 2)",
                [],
            )
            .unwrap();
            tx.commit().unwrap();
        }
        let mut h = harness(dir.path());
        settled(&mut h);
        let shown = |h: &Harness<'static, Option<MapCraftApp>>| h.state().as_ref().unwrap().shown;
        h.get_by_label("Filters").click();
        h.run_steps(3);
        h.get_by_label("File types");
        h.get_by_label("Earlier…").click(); // all years, so every photo is in one
        h.run_steps(3);
        h.get_by_label("2019").click();
        h.run_steps(3);
        let in_2019 = shown(&h);
        h.get_by_label("Any time").click();
        h.run_steps(3);
        h.query_all_by_label("RAW").next().unwrap().click();
        h.run_steps(3);
        assert_eq!(shown(&h), 16);
        h.get_by_label("All file types but RAW");
        h.query_all_by_label("JPEG").next().unwrap().click();
        h.run_steps(3);
        assert_eq!(shown(&h), 2, "only the DNGs");
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        h.get_by_label("DNG");
        // Removing the chip takes only the file types away.
        h.get_by_label("Filters (1 set)").click();
        h.run_steps(3);
        h.get_by_label("2019").click();
        h.run_steps(3);
        h.key_press(egui::Key::Escape);
        h.run_steps(2);
        h.get_by_label("Remove the file type filter").click();
        h.run_steps(3);
        assert_eq!(shown(&h), in_2019);
        assert!(h.state().as_ref().unwrap().filter.dates.is_some());
    }

    /// A press and release at `pos`, moving by `by` in between (zero: a click).
    fn press_drag(h: &mut Harness<'static, Option<MapCraftApp>>, pos: egui::Pos2, by: egui::Vec2) {
        let button = |pos, pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(egui::Event::PointerMoved(pos));
        h.run_steps(1);
        h.event(button(pos, true));
        h.run_steps(1);
        for k in 1..=4 {
            h.event(egui::Event::PointerMoved(pos + by * k as f32 / 4.0));
            h.run_steps(1);
        }
        h.event(button(pos + by, false));
        h.run_steps(3);
    }

    #[test]
    fn the_date_handles_drag_and_a_press_alone_changes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 16); // May 2019 to June 2024
        let mut h = harness_with(dir.path(), Some(StartDialog::Filters(None)));
        settled(&mut h);
        let from = h.get_by_label("Date from").rect();
        press_drag(
            &mut h,
            from.center() + egui::vec2(5.0, 0.0),
            egui::Vec2::ZERO,
        );
        assert_eq!(h.state().as_ref().unwrap().filter.dates, None);
        let to = h.get_by_label("Date to").rect();
        press_drag(&mut h, to.center() - egui::vec2(5.0, 0.0), egui::Vec2::ZERO);
        assert_eq!(h.state().as_ref().unwrap().filter.dates, None);
        // Dragging From a third of the way in: the range starts later and ends where it did.
        let rail = to.center().x - from.center().x;
        press_drag(&mut h, from.center(), egui::vec2(rail / 3.0, 0.0));
        let (a, b) = h.state().as_ref().unwrap().filter.dates.expect("a range");
        assert!(a > 201905 && a < 202406, "{a}");
        assert_eq!(b, 202406);
        assert!(h.state().as_ref().unwrap().shown < 14);
        // Typing a search closes the panel.
        h.state_mut().as_mut().unwrap().search.type_in("img");
        h.run_steps(3);
        assert!(!h.state().as_ref().unwrap().filters.open);
    }

    #[test]
    fn clicks_on_the_chips_and_the_panel_keep_the_selection() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 16);
        let mut h = harness_with(dir.path(), Some(StartDialog::Details));
        step_until(&mut h, "the panel", |h| {
            h.query_by_label("Photo details").is_some()
        });
        // The selected photo is from 2019: it stays shown.
        let app = h.state_mut().as_mut().unwrap();
        app.filter.dates = Some((201901, 201912));
        app.filter_changed();
        h.run_steps(3);
        let chip = h.get_by_label("Jan 2019 – Dec 2019").rect();
        press_drag(&mut h, chip.center(), egui::Vec2::ZERO);
        assert!(
            h.state().as_ref().unwrap().detail.is_some(),
            "a chip's text"
        );
        h.get_by_label("Filters (1 set)").click();
        h.run_steps(3);
        // The histogram, above the range's handles.
        let from = h.get_by_label("Date from").rect();
        press_drag(
            &mut h,
            from.center() + egui::vec2(60.0, -40.0),
            egui::Vec2::ZERO,
        );
        let app = h.state().as_ref().unwrap();
        assert!(app.detail.is_some(), "the histogram");
        assert!(app.filters.open, "a press inside keeps the panel");
        assert_eq!(app.filter.dates, Some((201901, 201912)));
    }

    #[test]
    fn sources_and_many_cameras_in_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8); // 7 on the map, 3 cameras
        // A second source: 8 photos from 8 more cameras.
        let more = dir.path().join("more");
        std::fs::create_dir_all(&more).unwrap();
        for i in 0..8 {
            let model = format!("Cam {i}");
            let jpeg = crate::demo::photo_with(
                47.0,
                8.0 + i as f64 * 0.01,
                "2018:03:03 10:00:00",
                0.3,
                Some(("Test", &model)),
            );
            std::fs::write(more.join(format!("M_{i}.jpg")), jpeg).unwrap();
        }
        let more_id = {
            let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
            let id = lib.add_source(&more, None, "More", &Scope::all()).unwrap();
            crate::import::run(&mut lib, id, &Default::default(), 2, |_| {}).unwrap();
            id
        };
        let mut h = harness_with(dir.path(), Some(StartDialog::Filters(None)));
        settled(&mut h);
        h.get_by_label("15 of 15 photos on the map");
        h.query_all_by_label("More").next().unwrap().click();
        h.run_steps(3);
        assert_eq!(h.state().as_ref().unwrap().shown, 7);
        // The chip names the source left: the row's box and text, and the chip.
        assert_eq!(h.query_all_by_label("Demo trips").count(), 3);
        // Six cameras, then the rest on request.
        assert!(h.query_by_label("Test Cam 7").is_none());
        h.get_by_label("Show all 11 cameras").click();
        h.run_steps(3);
        assert!(h.query_all_by_label("Test Cam 7").count() >= 1);
        // The hidden source is removed: its filter goes with it.
        let app = h.state_mut().as_mut().unwrap();
        app.lib.as_ref().unwrap().remove_source(more_id).unwrap();
        app.refresh();
        h.run_steps(3);
        let app = h.state().as_ref().unwrap();
        assert!(!app.filter.is_active());
        assert_eq!(app.shown, 7);
    }

    #[test]
    fn a_selected_photo_the_filter_hides_is_let_go() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 16);
        let mut h = harness_with(dir.path(), Some(StartDialog::Details));
        step_until(&mut h, "the panel", |h| {
            h.query_by_label("Photo details").is_some()
        });
        let app = h.state_mut().as_mut().unwrap();
        // The first photo is from 2019: show 2020 only.
        app.filter.dates = Some((202001, 202012));
        app.filter_changed();
        h.run_steps(3);
        let app = h.state().as_ref().unwrap();
        assert!(app.detail.is_none() && app.map.selected().is_none());
        assert!(h.query_by_label("Photo details").is_none());
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
                                network: false,
                                settings: no_apps(),
                                settings_file: None,
                                check_updates: false,
                                update_endpoint: crate::update::Endpoint::github(),
                                data_override: false,
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

    /// Settings whose companion apps aren't there: the real ones may be installed on this machine.
    fn no_apps() -> Settings {
        let mut s = Settings::default();
        s.companions.photocraft = PathBuf::from(
            r"C:
owhere\photocraft.exe",
        );
        s.companions.lightcraft = PathBuf::from(
            r"C:
owhere\lightcraft.exe",
        );
        s
    }

    /// Points the companion settings at empty stand-in files, so the apps count as found.
    fn fake_apps(h: &mut Harness<'static, Option<MapCraftApp>>, dir: &Path) {
        let apps = dir.join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        for f in ["photocraft.exe", "lightcraft.exe"] {
            std::fs::write(apps.join(f), b"").unwrap();
        }
        let app = h.state_mut().as_mut().unwrap();
        app.settings.companions.photocraft = apps.join("photocraft.exe");
        app.settings.companions.lightcraft = apps.join("lightcraft.exe");
        app.find_companions();
    }

    fn disabled(h: &Harness<'static, Option<MapCraftApp>>, label: &str) -> bool {
        h.get_by_label(label).accesskit_node().is_disabled()
    }

    #[test]
    fn the_companion_buttons_and_what_lightcraft_answers() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness_with(dir.path(), Some(StartDialog::Details));
        settled(&mut h);
        // Not installed where the settings say: both off.
        assert!(disabled(&h, "PhotoCraft") && disabled(&h, "LightCraft"));
        fake_apps(&mut h, dir.path());
        h.run_steps(2);
        assert!(!disabled(&h, "PhotoCraft") && !disabled(&h, "LightCraft"));
        h.get_by_label("PhotoCraft").click();
        h.run_steps(2);
        h.get_by_label("LightCraft").click();
        h.run_steps(2);
        let id = h.state().as_ref().unwrap().detail.as_ref().unwrap().id;
        assert_eq!(
            h.state().as_ref().unwrap().companion_calls,
            vec![
                (Companion::PhotoCraft, id, false),
                (Companion::LightCraft, id, false)
            ]
        );

        // LightCraft doesn't have the photo and writes sidecars: ask, and say so.
        let send = |h: &mut Harness<'static, Option<MapCraftApp>>, e| {
            let app = h.state_mut().as_mut().unwrap();
            app.lc_photo = app.detail.clone();
            app.lightcraft.0.send(e).unwrap();
        };
        send(&mut h, LcEvent::NotInLibrary { sidecars: true });
        h.run_steps(3);
        h.get_by_label("Add this photo to LightCraft?");
        h.get_by_label_contains(".xmp next to it");
        h.get_by_label("Add to LightCraft").click();
        h.run_steps(3);
        assert!(h.query_by_label("Add this photo to LightCraft?").is_none());
        assert_eq!(
            h.state().as_ref().unwrap().companion_calls.last(),
            Some(&(Companion::LightCraft, id, true))
        );
        // Without sidecars, the dialog doesn't mention them; Cancel adds nothing.
        send(&mut h, LcEvent::NotInLibrary { sidecars: false });
        h.run_steps(3);
        assert!(h.query_by_label_contains(".xmp next to it").is_none());
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert_eq!(h.state().as_ref().unwrap().companion_calls.len(), 3);

        // Starting, then a LightCraft opened by hand: the toast offers to try again.
        send(&mut h, LcEvent::Starting);
        h.run_steps(2);
        h.get_by_label("Opening in LightCraft…");
        send(&mut h, LcEvent::Failed(Failure::AlreadyOpen));
        h.run_steps(2);
        assert!(h.query_by_label("Opening in LightCraft…").is_none());
        h.get_by_label("LightCraft is already open");
        h.get_by_label("Try again").click();
        h.run_steps(2);
        assert_eq!(
            h.state().as_ref().unwrap().companion_calls.last(),
            Some(&(Companion::LightCraft, id, false))
        );
        // A port MapCraft started LightCraft on is remembered.
        send(&mut h, LcEvent::Port(51234));
        h.run_steps(2);
        assert_eq!(
            h.state()
                .as_ref()
                .unwrap()
                .settings
                .companions
                .lightcraft_port,
            Some(51234)
        );
    }

    #[test]
    fn a_raw_photo_cant_go_to_photocraft() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness_with(dir.path(), Some(StartDialog::Details));
        settled(&mut h);
        fake_apps(&mut h, dir.path());
        let app = h.state_mut().as_mut().unwrap();
        let mut d = app.detail.clone().unwrap();
        d.original_name = "IMG_0001.CR3".into();
        let b = app.companion_buttons(&d);
        assert_eq!(
            b.photocraft.as_deref(),
            Some("PhotoCraft can't open camera RAW files")
        );
        assert_eq!(b.lightcraft, None);
        app.lc_busy = true;
        assert!(
            app.companion_buttons(&d).lightcraft.is_some(),
            "one at a time"
        );
    }

    #[test]
    fn settings_say_what_was_found_and_save_the_paths() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 4);
        let mut h = harness_with(dir.path(), Some(StartDialog::Settings));
        h.run_steps(2);
        assert_eq!(h.get_all_by_label("Not found at this path").count(), 2);
        let settings_file = dir.path().join("settings.toml");
        h.state_mut().as_mut().unwrap().settings_file = Some(settings_file.clone());
        h.state_mut()
            .as_mut()
            .unwrap()
            .settings
            .companions
            .lightcraft_port = Some(4000);
        h.run_steps(3);
        // The defaults point into Program Files; the test can't rely on them, so only the
        // stand-in below is checked as found.
        let apps = dir.path().join("apps");
        std::fs::create_dir_all(&apps).unwrap();
        std::fs::write(apps.join("lightcraft.exe"), b"").unwrap();
        h.get_by_label("LightCraft app").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("LightCraft app")
            .type_text(&apps.join("lightcraft.exe").display().to_string());
        h.run_steps(3);
        h.get_by_label("Found");
        h.get_by_label("PhotoCraft app").click();
        h.run_steps(2);
        h.key_press_modifiers(egui::Modifiers::COMMAND, egui::Key::A);
        h.get_by_label("PhotoCraft app")
            .type_text(&apps.join("nothing-here.exe").display().to_string());
        h.run_steps(3);
        h.get_by_label("Not found at this path");
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert!(h.query_by_label("Companion apps").is_none(), "closed");
        let app = h.state().as_ref().unwrap();
        assert_eq!(
            app.settings.companions.lightcraft,
            apps.join("lightcraft.exe")
        );
        assert_eq!(
            app.settings.companions.lightcraft_port, None,
            "another LightCraft: the old port means nothing"
        );
        assert_eq!(app.found, [false, true]);
        let saved = Settings::load_from(&settings_file).unwrap();
        assert_eq!(saved.companions, app.settings.companions);
    }

    fn release() -> Release {
        let base = "https://github.com/mariokoehler/MapCraft/releases";
        Release {
            version: Version(99, 0, 0),
            page_url: format!("{base}/tag/v99.0.0"),
            exe_url: format!("{base}/download/v99.0.0/mapcraft.exe"),
            exe_size: 27_500_000,
            sha_url: format!("{base}/download/v99.0.0/mapcraft.exe.sha256"),
        }
    }

    /// Feeds the app what the background updater would report.
    fn inject(h: &mut Harness<'static, Option<MapCraftApp>>, e: update::Event) {
        h.state_mut().as_mut().unwrap().updater.inject(e);
        h.run_steps(3);
    }

    #[test]
    fn an_update_is_offered_confirmed_downloaded_and_installed() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path());
        h.run_steps(2);
        // The check at start finds a newer release: a toast offers it.
        h.state_mut().as_mut().unwrap().auto_check = true;
        inject(&mut h, update::Event::Checked(Ok(Some(release()))));
        h.get_by_label("MapCraft 99.0.0 is available");
        h.get_by_label("What’s new").click();
        h.run_steps(2);
        assert_eq!(
            h.state().as_ref().unwrap().update_calls,
            ["open https://github.com/mariokoehler/MapCraft/releases/tag/v99.0.0"]
        );

        // Update… asks first, with both versions and the size.
        h.get_by_label("Update…").click();
        h.run_steps(3);
        assert!(
            h.query_by_label("MapCraft 99.0.0 is available").is_none(),
            "the toast gives way to the dialog"
        );
        h.get_by_label("Update MapCraft");
        h.get_by_label("27.5 MB from GitHub");
        h.get_by_label("Not now").click();
        h.run_steps(3);
        assert!(h.query_by_label("Update MapCraft").is_none());

        // Settings can open it again; this time it downloads, fails its checksum, then works.
        h.state_mut().as_mut().unwrap().open_update();
        h.run_steps(3);
        h.get_by_label("Update and restart").click();
        h.run_steps(3);
        h.get_by_label("Downloading MapCraft 99.0.0…");
        inject(&mut h, update::Event::Progress(12_400_000, 27_500_000));
        h.get_by_label("12.4 of 27.5 MB");
        inject(
            &mut h,
            update::Event::Downloaded(Err(DownloadError::Checksum)),
        );
        h.get_by_label_contains("doesn’t match its checksum");
        h.get_by_label("Try again").click();
        h.run_steps(3);
        let file = dir.path().join("mapcraft.exe.download");
        std::fs::write(&file, b"new").unwrap();
        inject(&mut h, update::Event::Downloaded(Ok(file)));
        // Nothing runs, so it installs and restarts at once.
        assert_eq!(
            h.state().as_ref().unwrap().update_calls.last().unwrap(),
            "install, restart: true"
        );
    }

    #[test]
    fn a_cancelled_download_closes_the_dialog() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path());
        h.run_steps(2);
        inject(&mut h, update::Event::Checked(Ok(Some(release()))));
        assert!(
            h.query_by_label("MapCraft 99.0.0 is available").is_none(),
            "only the check at start announces itself"
        );
        h.state_mut().as_mut().unwrap().open_update();
        h.run_steps(3);
        h.get_by_label("Update and restart").click();
        h.run_steps(3);
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        assert!(h.query_by_label("Update MapCraft").is_none());
        // A download that finishes anyway is thrown away.
        let file = dir.path().join("mapcraft.exe.download");
        std::fs::write(&file, b"new").unwrap();
        inject(&mut h, update::Event::Downloaded(Ok(file.clone())));
        assert!(!file.exists());
        assert!(h.state().as_ref().unwrap().update_calls.is_empty());
        // Once "Restart later" holds a download, Update… doesn't start another.
        h.state_mut().as_mut().unwrap().update_pending = true;
        h.state_mut().as_mut().unwrap().open_update();
        h.run_steps(3);
        assert!(h.query_by_label("Update MapCraft").is_none());
        h.get_by_label("Update ready");
    }

    #[test]
    fn settings_check_for_updates_and_turn_the_check_off() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness_with(dir.path(), Some(StartDialog::Settings));
        h.run_steps(2);
        h.get_by_label("Updates").click();
        h.run_steps(2);
        h.get_by_label(&format!("MapCraft {}", env!("CARGO_PKG_VERSION")));
        h.get_by_label("Check now").click();
        h.run_steps(2);
        h.get_by_label("Asking GitHub…");
        inject(&mut h, update::Event::Checked(Ok(None)));
        h.get_by_label("You have the latest version.");
        h.get_by_label("Check now").click();
        h.run_steps(2);
        inject(&mut h, update::Event::Checked(Err("offline".into())));
        h.get_by_label("Couldn’t reach GitHub. Try again later.");
        h.get_by_label("Check now").click();
        h.run_steps(2);
        inject(&mut h, update::Event::Checked(Ok(Some(release()))));
        h.get_by_label("MapCraft 99.0.0 is available.");

        h.query_all_by_label("Check for updates when MapCraft starts")
            .next()
            .unwrap()
            .click();
        h.run_steps(2);
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert!(!h.state().as_ref().unwrap().settings.updates.check);

        // Update… in Settings opens the update dialog.
        h.state_mut()
            .as_mut()
            .unwrap()
            .open_settings(&egui::Context::default());
        h.run_steps(3);
        h.get_by_label("Updates").click();
        h.run_steps(2);
        h.get_by_label("Update…").click();
        h.run_steps(3);
        h.get_by_label("Update MapCraft");
        assert!(h.query_by_label("Companion apps").is_none());
    }

    /// Opens Settings on the Data folder section and waits for the count.
    fn data_section(h: &mut Harness<'static, Option<MapCraftApp>>) {
        h.state_mut()
            .as_mut()
            .unwrap()
            .open_settings(&egui::Context::default());
        h.run_steps(2);
        h.get_by_label("Data folder").click();
        step_until(h, "the count", |h| {
            h.state().as_ref().unwrap().data_usage.is_some()
        });
    }

    #[test]
    fn the_data_folder_shows_its_space_and_frees_previews() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness(dir.path());
        settled(&mut h);
        // A preview and a tile beside the thumbnails the import made.
        let previews = dir.path().join("previews/aa/bb");
        std::fs::create_dir_all(&previews).unwrap();
        std::fs::write(previews.join("1_1024.jpg"), vec![1u8; 2_000_000]).unwrap();
        data_section(&mut h);
        h.get_by_label("Thumbnails");
        h.get_by_label("Detail previews");
        assert!(h.query_all_by_label("2 MB").count() >= 1);
        h.get_by_label("Show in Explorer").click();
        h.run_steps(2);
        assert_eq!(
            h.state().as_ref().unwrap().update_calls,
            ["show data folder"]
        );

        h.get_by_label("Free up space…").click();
        h.run_steps(3);
        h.get_by_label("Free up space");
        assert!(
            h.query_by_label("Data folder").is_none(),
            "Settings gave way"
        );
        h.get_by_label_contains("Delete 2 MB").click();
        step_until(&mut h, "the toast", |h| {
            h.query_by_label("Freed 2 MB").is_some()
        });
        assert!(!dir.path().join("previews").exists());
        assert!(
            dir.path().join("thumbs").exists(),
            "thumbnails weren't ticked"
        );
    }

    #[test]
    fn freeing_thumbnails_makes_them_again() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 8);
        let mut h = harness(dir.path());
        settled(&mut h);
        let made = |h: &Harness<'static, Option<MapCraftApp>>| {
            let app = h.state().as_ref().unwrap();
            app.lib.as_ref().unwrap().thumb_counts().unwrap()
        };
        assert_eq!(made(&h).1, 0, "all made");
        data_section(&mut h);
        h.get_by_label("Free up space…").click();
        h.run_steps(3);
        h.query_all_by_label("Thumbnails").next().unwrap().click();
        h.run_steps(2);
        h.get_by_label_contains("Delete").click();
        // Forgotten, deleted, then made again by the thumbnail job.
        step_until(&mut h, "the toast", |h| {
            h.query_by_label_contains("Freed").is_some()
        });
        step_until(&mut h, "the thumbnails again", |h| {
            let app = h.state().as_ref().unwrap();
            app.thumbs.is_none() && made(h).1 == 0
        });
        assert!(dir.path().join("thumbs").exists());
    }

    #[test]
    fn moving_the_data_folder_copies_saves_and_restarts() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        std::fs::create_dir_all(&data).unwrap();
        imported(&data, 4);
        let mut h = harness(&data);
        settled(&mut h);
        let settings_file = dir.path().join("settings.toml");
        h.state_mut().as_mut().unwrap().settings_file = Some(settings_file.clone());
        data_section(&mut h);

        // A folder with other files: a MapCraft folder inside it.
        let picked = dir.path().join("D");
        std::fs::create_dir_all(&picked).unwrap();
        std::fs::write(picked.join("notes.txt"), b"x").unwrap();
        h.state_mut().as_mut().unwrap().settings_dialog = None;
        h.state_mut().as_mut().unwrap().pick_data_folder(&picked);
        h.run_steps(3);
        h.get_by_label("Change data folder");
        h.get_by_label_contains("holds other files");
        h.get_by_label("Move and restart").click();
        step_until(&mut h, "the copy and the restart", |h| {
            !h.state().as_ref().unwrap().update_calls.is_empty()
        });
        let new = picked.join("MapCraft");
        assert_eq!(h.state().as_ref().unwrap().update_calls, ["restart"]);
        assert!(datafolder::has_library(&new));
        assert!(new.join("thumbs").exists());
        let saved = Settings::load_from(&settings_file).unwrap();
        assert_eq!(saved.data_folder.as_deref(), Some(new.as_path()));
        assert_eq!(saved.old_data_folder.as_deref(), Some(data.as_path()));
        assert!(datafolder::has_library(&data), "the old copy stays");
        assert!(picked.join("notes.txt").is_file());
    }

    #[test]
    fn the_data_folder_isnt_changed_where_it_cant_be() {
        let dir = tempfile::tempdir().unwrap();
        let mut h = harness(dir.path());
        h.run_steps(2);
        // Without a settings file to save to, nothing switches.
        let other = tempfile::tempdir().unwrap();
        let empty = other.path().join("elsewhere");
        h.state_mut().as_mut().unwrap().pick_data_folder(&empty);
        h.run_steps(3);
        // The option is named twice (radio and text): either chooses it.
        h.query_all_by_label("Start empty there")
            .next()
            .unwrap()
            .click();
        h.run_steps(2);
        h.get_by_label("Start empty and restart").click();
        h.run_steps(3);
        h.get_by_label_contains("can’t save its settings");
        assert!(h.state().as_ref().unwrap().update_calls.is_empty());
        h.get_by_label("Close").click();
        h.run_steps(2);

        // The folder in use: refused.
        h.state_mut().as_mut().unwrap().pick_data_folder(dir.path());
        h.run_steps(3);
        h.get_by_label_contains("uses now");
        h.get_by_label("Close").click();
        h.run_steps(2);

        // A run with --data doesn't offer Change….
        h.state_mut().as_mut().unwrap().data_override = true;
        data_section(&mut h);
        h.get_by_label_contains("uses --data");
    }

    #[test]
    fn the_old_data_is_offered_for_removal_only_when_this_library_has_it_all() {
        let dir = tempfile::tempdir().unwrap();
        let (here, old) = (dir.path().join("here"), dir.path().join("old"));
        imported(&here, 4);
        imported(&old, 8);
        let mut h = harness(&here);
        settled(&mut h);
        h.state_mut().as_mut().unwrap().settings.old_data_folder = Some(old.clone());
        data_section(&mut h);
        step_until(&mut h, "the old folder's count", |h| {
            h.state().as_ref().unwrap().old_photos.is_some()
        });
        assert!(
            h.query_by_label("Remove old data…").is_none(),
            "the old library has more photos than this one: it may be the only good copy"
        );
        // An old folder with fewer photos than this library: offered.
        let older = dir.path().join("older");
        imported(&older, 2);
        h.state_mut().as_mut().unwrap().settings.old_data_folder = Some(older);
        data_section(&mut h);
        step_until(&mut h, "the old folder's count", |h| {
            h.state().as_ref().unwrap().old_photos == Some(2)
        });
        h.get_by_label("Remove old data…");
    }

    /// A generated track: an hour north from (47, 8), a point a minute, from 08:00Z; `times`
    /// off makes it a planned route.
    fn gpx_file(dir: &Path, times: bool) -> PathBuf {
        let mut pts = String::new();
        for i in 0..=60 {
            let time = if times {
                format!("<time>2025-06-12T08:{:02}:00Z</time>", i.min(59))
            } else {
                String::new()
            };
            let (i, t) = if i == 60 {
                (60, time.replace("08:59", "09:00"))
            } else {
                (i, time)
            };
            pts.push_str(&format!(
                "<trkpt lat=\"{}\" lon=\"8.0\"><ele>500</ele>{t}</trkpt>\n",
                47.0 + f64::from(i) * 0.0005
            ));
        }
        let path = dir.join(if times { "walk.gpx" } else { "route.gpx" });
        std::fs::write(
            &path,
            format!(
                "<gpx version=\"1.1\"><metadata><name>Test walk</name></metadata>\
                 <trk><trkseg>\n{pts}</trkseg></trk></gpx>"
            ),
        )
        .unwrap();
        path
    }

    /// Six photos without a position taken during the walk (camera on UTC+2), one the evening
    /// before, one with a position (it stays dim), and a sidecar of Lightroom's for TAG_01.
    fn tagging_library(dir: &Path) -> PathBuf {
        let photos = dir.join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        for i in 0..6 {
            let date = format!("2025:06:12 10:{:02}:00", 5 + i * 8);
            let jpeg = crate::demo::photo_with(0.0, 0.0, &date, i as f64 / 6.0, None);
            std::fs::write(photos.join(format!("TAG_{i:02}.jpg")), jpeg).unwrap();
        }
        let evening = crate::demo::photo_with(0.0, 0.0, "2025:06:11 20:00:00", 0.5, None);
        std::fs::write(photos.join("EVE_00.jpg"), evening).unwrap();
        let placed = crate::demo::photo_with_gps(47.1, 8.2, "2025:06:12 09:00:00", 0.3);
        std::fs::write(photos.join("PLACED.jpg"), placed).unwrap();
        std::fs::write(
            photos.join("TAG_01.xmp"),
            "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF \
             xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description \
             rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"4\"/>\
             </rdf:RDF></x:xmpmeta>",
        )
        .unwrap();
        let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
        let src = lib
            .add_source(&photos, None, "Walk", &Scope::all())
            .unwrap();
        crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        photos
    }

    fn session<'a>(
        h: &'a mut Harness<'static, Option<MapCraftApp>>,
    ) -> &'a mut crate::geotag::session::Session {
        h.state_mut().as_mut().unwrap().geotag.as_mut().unwrap()
    }

    /// The camera's zone, set as the panel would (the PC's zone differs between machines).
    fn utc_plus_2(h: &mut Harness<'static, Option<MapCraftApp>>) {
        let app = h.state_mut().as_mut().unwrap();
        app.geotag_action(crate::ui::geotag_panel::GeoAction::Zone(7200));
        app.geotag_action(crate::ui::geotag_panel::GeoAction::Correction(0));
        h.run_steps(3);
    }

    #[test]
    fn tagging_from_a_track_writes_sidecars_and_undo_restores_them() {
        let dir = tempfile::tempdir().unwrap();
        let photos = tagging_library(dir.path());
        let gpx = gpx_file(dir.path(), true);
        let mut h = harness_with(dir.path(), Some(StartDialog::Geotag(gpx)));
        h.run_steps(3);
        h.get_by_label("Tag from a GPX track");
        h.get_by_label("Test walk");
        utc_plus_2(&mut h);
        h.get_by_label("UTC+2:00");
        h.get_by_label("6 on the track, 6 ticked");
        h.get_by_label("1 more photo is further than 3 h from the track.");
        // The thumbnails of the candidates are made, then each marker has its tick.
        step_until(&mut h, "the candidates' thumbnails", |h| {
            let app = h.state().as_ref().unwrap();
            app.thumbs.is_none() && app.map.grouped()
        });
        let s = session(&mut h);
        for p in &s.photos {
            let far = p.original_name == "EVE_00.jpg";
            assert_eq!(
                p.thumb == crate::library::Thumb::Pending,
                far,
                "{}: near the track a thumbnail, further than the margin none",
                p.original_name
            );
        }
        // Zoomed in, the photos are apart; untick TAG_03.
        h.state_mut().as_mut().unwrap().map.set_zoom(17.0);
        h.run_steps(3);
        let wanted = |h: &Harness<'static, Option<MapCraftApp>>| {
            h.query_by_label("Untick TAG_03.jpg").is_some()
        };
        if !wanted(&h) {
            // The names come a frame later.
            step_until(&mut h, "the ticks' names", wanted);
        }
        h.get_by_label("Untick TAG_03.jpg").click();
        h.run_steps(3);
        h.get_by_label("6 on the track, 5 ticked");
        h.get_by_label("Tick TAG_03.jpg");

        // A clock correction moves the photos along the track (a point a minute, 0.0005°).
        let before = session(&mut h).points()[0].lat;
        h.state_mut()
            .as_mut()
            .unwrap()
            .geotag_action(crate::ui::geotag_panel::GeoAction::Correction(120));
        h.run_steps(2);
        let after = session(&mut h).points()[0].lat;
        assert!((before - after - 0.001).abs() < 1e-9, "{before} → {after}");
        h.state_mut()
            .as_mut()
            .unwrap()
            .geotag_action(crate::ui::geotag_panel::GeoAction::Correction(0));
        h.run_steps(2);

        // Write: the confirmation, then the sidecars.
        h.get_by_label("Write positions to 5 photos…").click();
        h.run_steps(3);
        h.get_by_label("Write positions to 5 photos?");
        h.get_by_label("Write positions").click();
        step_until(&mut h, "the write", |h| {
            let app = h.state().as_ref().unwrap();
            app.geo_work.is_none() && app.geotag.is_none()
        });
        h.get_by_label("Positions written to 5 photos");
        let read = |n: &str| std::fs::read_to_string(photos.join(n)).ok();
        let tag01 = read("TAG_01.xmp").unwrap();
        assert!(tag01.contains("xmp:Rating=\"4\"") && tag01.contains("exif:GPSLatitude"));
        assert!(
            read("TAG_00.xmp")
                .unwrap()
                .contains("exif:GPSLongitude=\"8,0.000000E\"")
        );
        assert!(read("TAG_03.xmp").is_none(), "unticked");
        assert!(read("EVE_00.xmp").is_none(), "not on the track");
        // The library places them; a rescan reads the same positions.
        let lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
        assert_eq!(lib.counts().unwrap().with_position, 6);
        let pos = |lib: &Library| {
            let mut p: Vec<_> = lib
                .positions()
                .unwrap()
                .iter()
                .map(|p| (p.id, (p.lat * 1e6).round() as i64))
                .collect();
            p.sort();
            p
        };
        let written = pos(&lib);
        let mut lib = lib;
        let src = lib.sources().unwrap()[0].id;
        crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        assert_eq!(pos(&lib), written, "a rescan reads what was written");

        // Undo restores the sidecars and the library.
        h.get_by_label("Undo").click();
        step_until(&mut h, "the undo", |h| {
            h.state().as_ref().unwrap().geo_work.is_none()
                && h.query_by_label("Geotagging undone").is_some()
        });
        assert!(read("TAG_00.xmp").is_none(), "a created sidecar is removed");
        let tag01 = read("TAG_01.xmp").unwrap();
        assert!(!tag01.contains("GPSLatitude") && tag01.contains("xmp:Rating=\"4\""));
        assert_eq!(lib.counts().unwrap().with_position, 1);
    }

    #[test]
    fn a_route_without_times_says_why_and_leaving_asks_after_changes() {
        let dir = tempfile::tempdir().unwrap();
        tagging_library(dir.path());
        let route = gpx_file(dir.path(), false);
        let mut h = harness_with(dir.path(), Some(StartDialog::Geotag(route)));
        h.run_steps(3);
        assert!(
            h.query_by_label_contains("This track has no times")
                .is_some(),
            "the reason shows"
        );
        assert!(h.query_by_label("Time zone").is_none(), "nothing to shift");
        // Esc without changes leaves at once.
        h.key_press(egui::Key::Escape);
        h.run_steps(3);
        assert!(h.state().as_ref().unwrap().geotag.is_none());

        // With a track and a change, leaving asks first.
        let walk = gpx_file(dir.path(), true);
        h.state_mut().as_mut().unwrap().start_geotag(vec![walk]);
        h.state_mut()
            .as_mut()
            .unwrap()
            .geotag_action(crate::ui::geotag_panel::GeoAction::MaxGap(15));
        h.run_steps(2);
        h.get_by_label("Cancel").click();
        h.run_steps(3);
        h.get_by_label("Leave tagging?");
        h.get_by_label("Stay").click();
        h.run_steps(3);
        assert!(h.state().as_ref().unwrap().geotag.is_some());
        h.get_by_label("Leave tagging (Esc)").click();
        h.run_steps(3);
        h.get_by_label("Leave").click();
        h.run_steps(3);
        assert!(h.state().as_ref().unwrap().geotag.is_none());
        h.get_by_label("Tag photos from a GPX track…");
    }

    /// A click (press and release) at a screen position.
    fn click_at(h: &mut Harness<'static, Option<MapCraftApp>>, pos: egui::Pos2) {
        let button = |pressed| egui::Event::PointerButton {
            pos,
            button: egui::PointerButton::Primary,
            pressed,
            modifiers: egui::Modifiers::NONE,
        };
        h.event(egui::Event::PointerMoved(pos));
        h.run_steps(1);
        h.event(button(true));
        h.run_steps(1);
        h.event(button(false));
        h.run_steps(3);
    }

    #[test]
    fn the_zone_buttons_taken_here_the_strip_and_undo_from_the_panel() {
        let dir = tempfile::tempdir().unwrap();
        let photos = tagging_library(dir.path());
        // Two more at one spot: their group never splits, so it opens a strip.
        for name in ["SAME_A.jpg", "SAME_B.jpg"] {
            let jpeg = crate::demo::photo_with(0.0, 0.0, "2025:06:12 10:50:00", 0.7, None);
            std::fs::write(photos.join(name), jpeg).unwrap();
        }
        {
            let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
            let src = lib.sources().unwrap()[0].id;
            crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        }
        let gpx = gpx_file(dir.path(), true);
        let mut h = harness_with(dir.path(), Some(StartDialog::Geotag(gpx.clone())));
        h.run_steps(3);
        utc_plus_2(&mut h);
        step_until(&mut h, "the candidates' thumbnails", |h| {
            let app = h.state().as_ref().unwrap();
            app.thumbs.is_none() && app.map.grouped()
        });
        h.get_by_label("8 on the track, 8 ticked");

        // The zone's buttons: an hour later, the photos fall before the track.
        h.get_by_label("+1 h").click();
        h.run_steps(3);
        h.get_by_label("UTC+3:00");
        h.get_by_label("0 on the track, 0 ticked");
        h.get_by_label("−1 h").click();
        h.run_steps(3);
        h.get_by_label("8 on the track, 8 ticked");

        // "Taken here": TAG_02 (10:21 on its clock) was really taken where the track was at
        // 08:25 UTC, four minutes later: the correction becomes −4 min.
        {
            let app = h.state_mut().as_mut().unwrap();
            app.map.set_zoom(17.0);
            app.map.center_at(47.0115, 8.0);
        }
        h.run_steps(3);
        step_until(&mut h, "TAG_02's marker", |h| {
            h.query_by_label("Photo TAG_02.jpg").is_some()
        });
        h.get_by_label("Photo TAG_02.jpg").click();
        h.run_steps(3);
        h.get_by_label("Set from a photo’s place…").click();
        h.run_steps(3);
        assert!(session(&mut h).picking);
        let spot = h
            .state()
            .as_ref()
            .unwrap()
            .map
            .screen_of(47.0 + 25.0 * 0.0005, 8.0);
        click_at(&mut h, spot);
        let s = session(&mut h);
        assert!(!s.picking);
        assert_eq!((s.shift.zone, s.shift.correction), (7200, -240));
        h.get_by_label("UTC+1:56");
        h.get_by_label("Undo").click();
        h.run_steps(3);
        h.get_by_label("UTC+2:00");

        // The strip of the two at one spot, and a tick in it.
        {
            let app = h.state_mut().as_mut().unwrap();
            app.map.center_at(47.025, 8.0);
        }
        h.run_steps(3);
        step_until(&mut h, "the group at one spot", |h| {
            h.query_by_label("Group of 2 photos").is_some()
        });
        h.get_by_label("Group of 2 photos").click();
        step_until(&mut h, "the strip", |h| {
            h.query_all_by_label("Untick this photo").count() == 2
        });
        h.query_all_by_label("Untick this photo")
            .next()
            .unwrap()
            .click();
        h.run_steps(3);
        h.get_by_label("8 on the track, 7 ticked");
        h.get_by_label("Tick this photo");

        // Write, then undo from the panel of a later session (the toast gone).
        h.get_by_label("Write positions to 7 photos…").click();
        h.run_steps(3);
        h.get_by_label("Write positions").click();
        step_until(&mut h, "the write", |h| {
            let app = h.state().as_ref().unwrap();
            app.geo_work.is_none() && app.geotag.is_none()
        });
        assert_eq!(
            std::fs::read_dir(&photos)
                .unwrap()
                .filter(|e| e
                    .as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "xmp"))
                .count(),
            7,
            "TAG_01's sidecar was there already and is updated: 6 new, 1 updated"
        );
        h.state_mut().as_mut().unwrap().toasts.replace(Vec::new());
        h.state_mut().as_mut().unwrap().start_geotag(vec![gpx]);
        h.run_steps(3);
        h.get_by_label("Last written");
        h.get_by_label("Undo it").click();
        step_until(&mut h, "the undo", |h| {
            h.state().as_ref().unwrap().geo_work.is_none()
                && h.query_by_label("Geotagging undone").is_some()
        });
        let left = std::fs::read_dir(&photos)
            .unwrap()
            .filter(|e| {
                e.as_ref()
                    .unwrap()
                    .path()
                    .extension()
                    .is_some_and(|x| x == "xmp")
            })
            .count();
        assert_eq!(left, 1, "only TAG_01's own sidecar, as it was");
        assert!(h.query_by_label("Last written").is_none());
    }

    #[test]
    fn the_map_style_menu_the_key_and_its_styles() {
        let dir = tempfile::tempdir().unwrap();
        imported(dir.path(), 4);
        let mut h = harness(dir.path());
        settled(&mut h);
        let style = |h: &Harness<'static, Option<MapCraftApp>>| {
            let app = h.state().as_ref().unwrap();
            (app.map.style(), app.settings.map.style)
        };
        // Without a key, the satellite styles are listed but can't be picked.
        h.get_by_label("Map style").click();
        h.run_steps(3);
        assert!(disabled(&h, "Satellite") && disabled(&h, "Hybrid"));
        h.get_by_label("Topographic").click();
        h.run_steps(3);
        assert_eq!(style(&h), (Style::Topographic, Style::Topographic));
        assert!(
            h.query_by_label("Satellite").is_none(),
            "a choice closes the menu"
        );
        h.get_by_label_contains("© OpenTopoMap (CC-BY-SA)");

        // Add a key… opens Settings on Map; a saved key makes Satellite available.
        h.get_by_label("Map style").click();
        h.run_steps(3);
        h.get_by_label("Add a key…").click();
        h.run_steps(3);
        h.get_by_label("MapTiler key").click();
        h.run_steps(1);
        h.get_by_label("MapTiler key").type_text("test-key-123");
        h.run_steps(2);
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert_eq!(
            h.state().as_ref().unwrap().settings.map.key(),
            Some("test-key-123")
        );
        h.get_by_label("Map style").click();
        h.run_steps(3);
        assert!(!disabled(&h, "Satellite"));
        h.get_by_label("Satellite").click();
        h.run_steps(3);
        assert_eq!(style(&h), (Style::Satellite, Style::Satellite));
        h.get_by_label_contains("© OpenStreetMap contributors");

        // Removing the key takes its styles with it.
        h.state_mut()
            .as_mut()
            .unwrap()
            .open_settings(&egui::Context::default());
        h.state_mut()
            .as_mut()
            .unwrap()
            .settings_dialog
            .as_mut()
            .unwrap()
            .section = Section::Map;
        h.run_steps(3);
        h.get_by_label("Remove the key").click();
        h.run_steps(2);
        h.get_by_label("Save").click();
        h.run_steps(3);
        assert_eq!(style(&h), (Style::Standard, Style::Standard));
        assert_eq!(h.state().as_ref().unwrap().settings.map.key(), None);
    }
}

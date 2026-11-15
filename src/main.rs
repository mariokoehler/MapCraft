//! MapCraft: shows your photos on a world map. See design.md for the concept and architecture.
//!
//! The code lives in the library crate (`lib.rs`, which declares the modules; a module is
//! roughly a Java package). This file only starts the app.

// No console window behind the app in release builds; debug builds keep it for the log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use std::fs;

use mapcraft::app;
use mapcraft::settings::{Locations, Settings};
use tracing::{error, info, warn};

fn main() -> eframe::Result {
    init_logging();

    let args: Vec<String> = std::env::args().collect();
    match args.get(1).map(String::as_str) {
        Some("--import") => std::process::exit(dev_import(&args[2..])),
        Some("--make-demo") => std::process::exit(dev_make_demo(&args[2..])),
        Some("--fill-library") => std::process::exit(dev_fill_library(&args[2..])),
        _ => {}
    }
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    // `--open add-folder[=<folder>]`, `--open sources`, `--open details`, `--open strip`,
    // `--open filters[=<year>]`, `--open settings` or `--open search=<text>`: start with that
    // dialog, the first
    // photo selected, the first strip open, a year's photos only, or the text searched
    // (screenshots).
    let open = match flag("--open").map(String::as_str) {
        Some("sources") => Some(app::StartDialog::Sources),
        Some("details") => Some(app::StartDialog::Details),
        Some("strip") => Some(app::StartDialog::Strip),
        Some("filters") => Some(app::StartDialog::Filters(None)),
        Some("settings") => Some(app::StartDialog::Settings),
        Some("data") => Some(app::StartDialog::DataFolder),
        Some(s) if s.starts_with("change-data=") => Some(app::StartDialog::ChangeData(
            std::path::PathBuf::from(&s["change-data=".len()..]),
        )),
        Some(s) if s.starts_with("filters=") => {
            Some(app::StartDialog::Filters(s[8..].parse().ok()))
        }
        Some(s) if s.starts_with("search=") => Some(app::StartDialog::Search(s[7..].to_owned())),
        Some(o) if o.starts_with("add-folder") => Some(app::StartDialog::AddFolder(
            o.strip_prefix("add-folder=").map(std::path::PathBuf::from),
        )),
        _ => None,
    };
    let perf = args.iter().any(|a| a == "--perf");
    // `--update-from <base url>`: a pretend GitHub for trying the updater (design.md §3.10).
    let update_from = flag("--update-from").cloned();
    if open.is_some() {
        info!("start dialog: {open:?}");
    }

    let Some(locations) = Locations::from_system() else {
        error!("Windows didn't report the AppData folders; can't start");
        std::process::exit(1);
    };
    // `--settings <file>`: another settings file, for trials that must not touch the real one
    // (design.md M8). It's kept across a restart like every other option.
    let mut locations = locations;
    if let Some(f) = flag("--settings") {
        locations.settings_file = std::path::PathBuf::from(f);
    }
    let (settings, writable) = load_settings(&locations);
    // `--data <folder>` overrides the data folder for this run only (tests and trials with
    // generated photos never touch the real library).
    let data_from_flag = flag("--data").map(std::path::PathBuf::from);
    let data_override = data_from_flag.is_some();
    let data_folder =
        data_from_flag.unwrap_or_else(|| settings.data_folder_or(&locations.default_data_folder));
    let tile_cache = data_folder.join("tiles");
    if let Err(e) = fs::create_dir_all(&tile_cache) {
        warn!(
            "can't create the tile cache folder {}: {e}",
            tile_cache.display()
        );
    }
    info!("settings file: {}", locations.settings_file.display());
    info!("data folder: {}", data_folder.display());

    // The window's icon (title bar, taskbar); the exe's own comes from build.rs.
    let icon =
        eframe::icon_data::from_png_bytes(include_bytes!("../assets/brand/mapcraft-icon-256.png"))
            .expect("the embedded icon is a valid PNG");
    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MapCraft")
            .with_icon(icon)
            .with_inner_size([settings.window.width, settings.window.height])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    // The update check at start: on in release builds (or with `--update-from`), unless the
    // user turned it off in Settings.
    let check_updates =
        settings.updates.check && (!cfg!(debug_assertions) || update_from.is_some());
    let update_endpoint = update_from.as_deref().map_or_else(
        mapcraft::update::Endpoint::github,
        mapcraft::update::Endpoint::at,
    );
    let result = eframe::run_native(
        "MapCraft",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            let config = app::Config {
                data_folder,
                tiles: true,
                open,
                perf,
                launch: true,
                network: true,
                settings,
                settings_file: writable.then_some(locations.settings_file),
                check_updates,
                update_endpoint,
                data_override,
            };
            Ok(Box::new(app::MapCraftApp::new(&cc.egui_ctx, config)))
        }),
    );
    install_pending_update(&args);
    result
}

/// Swaps in an update the app downloaded and checked (design.md §3.10), now that the window
/// has closed, and starts the new version when the user chose to restart. The new version gets
/// the same options, `--data` included, so it opens the same library.
fn install_pending_update(args: &[String]) {
    let pending = mapcraft::update::take_pending();
    // A restart for the update, or for a new data folder (even after "Restart later").
    let restart = pending.as_ref().is_some_and(|p| p.1) || mapcraft::update::restart_requested();
    if pending.is_none() && !restart {
        return;
    }
    // The path before the swap: afterwards it holds the new version.
    let Ok(exe) = std::env::current_exe() else {
        error!("can't tell where MapCraft's exe is; no update or restart");
        if let Some((file, _)) = pending {
            let _ = std::fs::remove_file(file);
        }
        return;
    };
    if let Some((file, _)) = pending {
        match mapcraft::update::install(&file) {
            Ok(()) => info!("update installed"),
            // The old version stays; when a restart was asked for, it starts again, so the
            // window doesn't just vanish (the release build has no console to say why).
            Err(e) => error!("{e}"),
        }
    }
    if restart
        && let Err(e) = mapcraft::update::restart(&exe, &mapcraft::update::restart_args(&args[1..]))
    {
        error!("{e}");
    }
}

/// Dev command: `mapcraft --fill-library <data folder> <count>` writes `count` synthetic photo
/// records (no files) spread over Europe into `<data folder>\library.sqlite`, to check how the
/// map copes with a large library. Never point it at the real data folder.
fn dev_fill_library(args: &[String]) -> i32 {
    let (Some(data), Some(count)) = (
        args.first(),
        args.get(1).and_then(|n| n.parse::<usize>().ok()),
    ) else {
        eprintln!("usage: mapcraft --fill-library <data folder> <count>");
        return 2;
    };
    let db = std::path::Path::new(data).join("library.sqlite");
    let result = (|| {
        let mut lib = mapcraft::library::Library::open(&db)?;
        let src = lib.add_source(
            std::path::Path::new(r"C:\synthetic"),
            None,
            "Synthetic",
            &mapcraft::import::scope::Scope::all(),
        )?;
        let tx = lib.transaction()?;
        for i in 0..count {
            // A deterministic scatter over Europe, denser around a few cities.
            let f = (i as f64 * 0.618_033_988_75).fract();
            let g = (i as f64 * 0.754_877_666_25).fract();
            let (lat, lon) = (36.0 + 24.0 * f, -10.0 + 40.0 * g);
            let rec = mapcraft::library::PhotoRecord {
                rel_dir: format!("{}", i / 500),
                stem: format!("IMG_{i:06}"),
                original_name: format!("IMG_{i:06}.jpg"),
                files_sig: i as i64,
                file_size: 1,
                lat: Some(lat),
                lon: Some(lon),
                gps_source: Some("exif"),
                ..Default::default()
            };
            mapcraft::library::write_photo(&tx, src, None, &rec)?;
        }
        tx.commit()
    })();
    match result {
        Ok(()) => {
            println!("wrote {count} synthetic photos to {}", db.display());
            0
        }
        Err(e) => {
            eprintln!("can't fill the library: {e}");
            1
        }
    }
}

/// Dev command: `mapcraft --make-demo <folder> [count]` writes generated photos (default 400)
/// with positions around Europe, for trying the import without real photos.
fn dev_make_demo(args: &[String]) -> i32 {
    let Some(folder) = args.first() else {
        eprintln!("usage: mapcraft --make-demo <folder> [count]");
        return 2;
    };
    let count = args.get(1).and_then(|n| n.parse().ok()).unwrap_or(400);
    match mapcraft::demo::make_demo(std::path::Path::new(folder), count) {
        Ok(()) => {
            println!("wrote {count} demo photos to {folder}");
            0
        }
        Err(e) => {
            eprintln!("can't write the demo photos: {e}");
            1
        }
    }
}

/// Reads the settings. A missing file is created with the defaults; an unreadable one is left
/// untouched (never overwritten) and the defaults are used for this run.
/// The settings, and whether MapCraft may write them back: never over a file it couldn't read,
/// so a typo is never replaced by the defaults.
fn load_settings(locations: &Locations) -> (Settings, bool) {
    let path = &locations.settings_file;
    let existed = path.exists();
    match Settings::load_from(path) {
        Ok(settings) => {
            if !existed && let Err(e) = settings.save_to(path) {
                warn!("can't write the default settings: {e}");
            }
            (settings, true)
        }
        Err(e) => {
            error!("{e}; using the defaults, and changes this session aren't saved");
            (Settings::default(), false)
        }
    }
}

/// Dev command: `mapcraft --import <folder> --data <data folder> [--readers N]`.
/// Imports (or rescans) one folder headless and prints the summary and GPS rates per file type.
/// Reads the folder only; writes only `<data folder>\library.sqlite`.
fn dev_import(args: &[String]) -> i32 {
    let usage = "usage: mapcraft --import <folder> --data <data folder> [--readers N]";
    let flag = |name: &str| {
        args.iter()
            .position(|a| a == name)
            .and_then(|i| args.get(i + 1))
    };
    let (Some(folder), Some(data)) = (args.first(), flag("--data")) else {
        eprintln!("{usage}");
        return 2;
    };
    let readers = flag("--readers").and_then(|n| n.parse().ok()).unwrap_or(8);
    let folder = std::path::PathBuf::from(folder);
    let mut lib = match mapcraft::library::Library::open(
        &std::path::Path::new(data).join("library.sqlite"),
    ) {
        Ok(l) => l,
        Err(e) => {
            eprintln!("can't open the library: {e}");
            return 1;
        }
    };
    let existing = lib
        .sources()
        .unwrap_or_default()
        .into_iter()
        .find(|s| s.root_path == folder);
    let id = match existing {
        Some(s) => s.id,
        None => {
            let unc = mapcraft::paths::unc_target(&folder);
            let label = folder
                .file_name()
                .map_or("source".into(), |n| n.to_string_lossy().into_owned());
            match lib.add_source(
                &folder,
                unc.as_deref(),
                &label,
                &mapcraft::import::scope::Scope::all(),
            ) {
                Ok(id) => id,
                Err(e) => {
                    eprintln!("can't add the source: {e}");
                    return 1;
                }
            }
        }
    };
    let started = std::time::Instant::now();
    let cancel = std::sync::atomic::AtomicBool::new(false);
    let result = mapcraft::import::run(&mut lib, id, &cancel, readers, |p| {
        if let mapcraft::import::Progress::Reading { done, total } = p
            && (done % 1000 == 0 || done == total)
        {
            eprintln!(
                "  read {done} / {total} at {:.1} s",
                started.elapsed().as_secs_f64()
            );
        }
    });
    match result {
        Ok(s) => {
            println!("{}", s.counts_only());
            println!(
                "took {:.1} s with {readers} readers",
                started.elapsed().as_secs_f64()
            );
            for (ext, n, gps, xmp) in lib.gps_by_type().unwrap_or_default() {
                println!(
                    "  {ext:5} {n:6} photos, {:5.1} % with position ({xmp} from sidecars)",
                    100.0 * gps as f64 / n.max(1) as f64
                );
            }
            0
        }
        Err(e) => {
            eprintln!("import failed: {e}");
            1
        }
    }
}

/// Log to the console. `RUST_LOG` overrides the default, which keeps other crates at `warn`:
/// their debug output would include tile URLs, and tile URLs give away where the user looks
/// (design.md §3.8: no coordinates in logs).
fn init_logging() {
    let filter = tracing_subscriber::EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("warn,mapcraft=info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();
}

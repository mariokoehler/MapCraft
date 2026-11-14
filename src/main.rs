//! MapCraft: shows your photos on a world map. See design.md for the concept and architecture.
//!
//! `mod x;` declares a module (roughly a Java package) whose code is in `x.rs` or `x/mod.rs`.

// No console window behind the app in release builds; debug builds keep it for the log.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod app;
mod provider;
mod settings;
mod theme;
mod ui;

use std::fs;

use settings::{Locations, Settings};
use tracing::{error, info, warn};

fn main() -> eframe::Result {
    init_logging();

    let Some(locations) = Locations::from_system() else {
        error!("Windows didn't report the AppData folders; can't start");
        std::process::exit(1);
    };
    let settings = load_settings(&locations);
    let data_folder = settings.data_folder_or(&locations.default_data_folder);
    let tile_cache = data_folder.join("tiles");
    if let Err(e) = fs::create_dir_all(&tile_cache) {
        warn!(
            "can't create the tile cache folder {}: {e}",
            tile_cache.display()
        );
    }
    info!("settings file: {}", locations.settings_file.display());
    info!("data folder: {}", data_folder.display());

    let options = eframe::NativeOptions {
        viewport: egui::ViewportBuilder::default()
            .with_title("MapCraft")
            .with_inner_size([settings.window.width, settings.window.height])
            .with_min_inner_size([960.0, 600.0]),
        ..Default::default()
    };
    eframe::run_native(
        "MapCraft",
        options,
        Box::new(move |cc| {
            egui_extras::install_image_loaders(&cc.egui_ctx);
            Ok(Box::new(app::MapCraftApp::new(cc, &tile_cache)))
        }),
    )
}

/// Reads the settings. A missing file is created with the defaults; an unreadable one is left
/// untouched (never overwritten) and the defaults are used for this run.
fn load_settings(locations: &Locations) -> Settings {
    let path = &locations.settings_file;
    let existed = path.exists();
    match Settings::load_from(path) {
        Ok(settings) => {
            if !existed && let Err(e) = settings.save_to(path) {
                warn!("can't write the default settings: {e}");
            }
            settings
        }
        Err(e) => {
            error!("{e}; using the defaults and leaving the file as it is");
            Settings::default()
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

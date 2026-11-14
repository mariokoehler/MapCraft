//! The bootstrap settings file, `%APPDATA%\MapCraft\settings.toml` (design.md §3.3).
//!
//! It has a fixed place because it holds the one setting that can't live in the data folder:
//! where the data folder is. Every field has a default, so a missing or partial file still loads
//! (`#[serde(default)]`, roughly Jackson's "ignore missing properties" plus field defaults).

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The folder name used under `%APPDATA%` and `%LOCALAPPDATA%`.
const APP_FOLDER: &str = "MapCraft";

#[derive(Debug, Clone, PartialEq, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Where thumbnails, the library and the tile cache live. `None` means the default,
    /// `%LOCALAPPDATA%\MapCraft`.
    pub data_folder: Option<PathBuf>,
    pub window: WindowSettings,
    pub companions: CompanionSettings,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct WindowSettings {
    pub width: f32,
    pub height: f32,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CompanionSettings {
    pub photocraft_cli: PathBuf,
    pub lightcraft_cli: PathBuf,
}

impl Default for WindowSettings {
    fn default() -> Self {
        Self {
            width: 1280.0,
            height: 800.0,
        }
    }
}

impl Default for CompanionSettings {
    fn default() -> Self {
        Self {
            photocraft_cli: PathBuf::from(r"C:\Program Files\PhotoCraft\photocraft-cli.exe"),
            lightcraft_cli: PathBuf::from(r"C:\Program Files\LightCraft\lightcraft-cli.exe"),
        }
    }
}

impl Settings {
    /// Reads the settings from `path`. A missing file gives the defaults; a file that can't be
    /// parsed is an error, so a typo never silently resets the user's settings.
    pub fn load_from(path: &Path) -> Result<Self, SettingsError> {
        match fs::read_to_string(path) {
            Ok(text) => toml::from_str(&text).map_err(|e| SettingsError::Parse(e.to_string())),
            Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(Self::default()),
            Err(e) => Err(SettingsError::Io(e)),
        }
    }

    /// Writes the settings to `path`, creating its folder if needed.
    pub fn save_to(&self, path: &Path) -> Result<(), SettingsError> {
        if let Some(dir) = path.parent() {
            fs::create_dir_all(dir).map_err(SettingsError::Io)?;
        }
        let text = toml::to_string_pretty(self).map_err(|e| SettingsError::Parse(e.to_string()))?;
        fs::write(path, text).map_err(SettingsError::Io)
    }

    /// The data folder in effect: the configured one, else `default_data_folder`.
    pub fn data_folder_or(&self, default_data_folder: &Path) -> PathBuf {
        self.data_folder
            .clone()
            .unwrap_or_else(|| default_data_folder.to_path_buf())
    }
}

/// The two fixed locations, resolved once from Windows' known folders.
#[derive(Debug, Clone)]
pub struct Locations {
    /// `%APPDATA%\MapCraft\settings.toml`
    pub settings_file: PathBuf,
    /// `%LOCALAPPDATA%\MapCraft`
    pub default_data_folder: PathBuf,
}

impl Locations {
    /// Uses `BaseDirs`, not `ProjectDirs`: the latter would add `\config` and `\data` subfolders.
    pub fn from_system() -> Option<Self> {
        let base = directories::BaseDirs::new()?;
        Some(Self {
            settings_file: base.config_dir().join(APP_FOLDER).join("settings.toml"),
            default_data_folder: base.data_local_dir().join(APP_FOLDER),
        })
    }
}

#[derive(Debug)]
pub enum SettingsError {
    Io(io::Error),
    Parse(String),
}

impl std::fmt::Display for SettingsError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "can't read or write the settings file: {e}"),
            Self::Parse(e) => write!(f, "the settings file isn't valid: {e}"),
        }
    }
}

impl std::error::Error for SettingsError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn missing_file_gives_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let s = Settings::load_from(&dir.path().join("settings.toml")).unwrap();
        assert_eq!(s, Settings::default());
    }

    #[test]
    fn round_trip() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("sub").join("settings.toml");
        let s = Settings {
            data_folder: Some(PathBuf::from(r"D:\MapCraftData")),
            window: WindowSettings {
                width: 1600.0,
                ..WindowSettings::default()
            },
            ..Settings::default()
        };
        s.save_to(&path).unwrap();
        assert_eq!(Settings::load_from(&path).unwrap(), s);
    }

    #[test]
    fn partial_file_fills_in_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "[window]\nwidth = 1500.0\n").unwrap();
        let s = Settings::load_from(&path).unwrap();
        assert_eq!(s.window.width, 1500.0);
        assert_eq!(s.window.height, WindowSettings::default().height);
        assert_eq!(s.data_folder, None);
    }

    #[test]
    fn invalid_file_is_an_error_not_a_reset() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.toml");
        fs::write(&path, "window = [this is not toml").unwrap();
        assert!(matches!(
            Settings::load_from(&path),
            Err(SettingsError::Parse(_))
        ));
    }

    #[test]
    fn system_locations_have_the_documented_shape() {
        let loc = Locations::from_system().expect("known folders");
        assert!(loc.settings_file.ends_with(r"MapCraft\settings.toml"));
        assert!(loc.default_data_folder.ends_with("MapCraft"));
        assert!(!loc.default_data_folder.ends_with(r"MapCraft\data"));
    }
}

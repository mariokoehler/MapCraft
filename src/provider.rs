//! The map styles and where their tiles come from (design.md §3.5, M10): OpenStreetMap's
//! Standard and OpenTopoMap's Topographic without a key, MapTiler's Satellite and Hybrid with
//! the user's own key.
//!
//! An `enum` here is like a Java enum whose constants could each carry their own data, and
//! `match` over it must handle every variant.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use walkers::sources::{self, Attribution as WalkersAttribution, OpenTopoServer, TileSource};
use walkers::{HeaderValue, HttpOptions, HttpTiles, TileId};

/// Sent with every tile request. OSM's tile policy requires an identifying User-Agent; a library
/// default gets blocked (verified 2026-11-14, https://operations.osmfoundation.org/policies/tiles/).
pub const USER_AGENT: &str = concat!(
    "MapCraft/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/mariokoehler/MapCraft)"
);

/// MapTiler's logo, shown on the map while its tiles are (its free plan asks for it); loaded
/// from MapTiler at run time rather than kept in the repo.
pub const MAPTILER_LOGO: &str = "https://api.maptiler.com/resources/logo.svg";

/// Where a user gets a MapTiler key.
pub const MAPTILER_SIGN_UP: &str = "https://cloud.maptiler.com/";

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Style {
    #[default]
    Standard,
    Topographic,
    Satellite,
    Hybrid,
}

/// What the map must show for a style: its required credit, `before` + `link` + `after`,
/// where `link` is drawn as a link to `url`.
pub struct Attribution {
    pub before: &'static str,
    pub link: &'static str,
    pub after: &'static str,
    pub url: &'static str,
}

impl Style {
    pub const ALL: [Style; 4] = [
        Style::Standard,
        Style::Topographic,
        Style::Satellite,
        Style::Hybrid,
    ];

    pub fn name(self) -> &'static str {
        match self {
            Self::Standard => "Standard",
            Self::Topographic => "Topographic",
            Self::Satellite => "Satellite",
            Self::Hybrid => "Hybrid",
        }
    }

    /// The line under its name in the menu.
    pub fn note(self, has_key: bool) -> &'static str {
        match self {
            Self::Standard => "OpenStreetMap",
            Self::Topographic => "OpenTopoMap · contours and paths",
            _ if !has_key => "Needs a MapTiler key",
            Self::Satellite => "MapTiler",
            Self::Hybrid => "MapTiler · satellite with roads and names",
        }
    }

    pub fn needs_key(self) -> bool {
        matches!(self, Self::Satellite | Self::Hybrid)
    }

    /// Its tile cache, in the data folder (Standard keeps the folder it always had).
    pub fn cache_folder(self) -> &'static str {
        match self {
            Self::Standard => "tiles",
            Self::Topographic => "tiles-topo",
            Self::Satellite => "tiles-satellite",
            Self::Hybrid => "tiles-hybrid",
        }
    }

    /// MapTiler's map id (https://docs.maptiler.com/cloud/api/maps/).
    fn maptiler_map(self) -> Option<&'static str> {
        match self {
            Self::Satellite => Some("satellite"),
            Self::Hybrid => Some("hybrid"),
            _ => None,
        }
    }

    pub fn attribution(self) -> Attribution {
        match self {
            Self::Standard => Attribution {
                before: "© ",
                link: "OpenStreetMap",
                after: " contributors",
                url: "https://www.openstreetmap.org/copyright",
            },
            // The credit OpenTopoMap asks for (verified 2026-11-15, https://opentopomap.org/about).
            Self::Topographic => Attribution {
                before: "Map data: © OpenStreetMap contributors, SRTM | Map style: © ",
                link: "OpenTopoMap",
                after: " (CC-BY-SA)",
                url: "https://opentopomap.org/about",
            },
            Self::Satellite | Self::Hybrid => Attribution {
                before: "© ",
                link: "MapTiler",
                after: " © OpenStreetMap contributors",
                url: "https://www.maptiler.com/copyright/",
            },
        }
    }

    /// A tile at zoom 0 with this key, to test it (`None` for styles without a key).
    pub fn test_url(self, key: &str) -> Option<String> {
        self.maptiler_map()
            .map(|m| format!("https://api.maptiler.com/maps/{m}/256/0/0/0.jpg?key={key}"))
    }

    /// The tile loader, with our User-Agent and an HTTP cache in the style's folder of `data`
    /// that honours the server's caching headers. `None` for a MapTiler style without a key.
    pub fn tiles(self, data: &Path, key: Option<&str>, ctx: egui::Context) -> Option<HttpTiles> {
        let options = HttpOptions {
            cache: Some(data.join(self.cache_folder())),
            user_agent: Some(HeaderValue::from_static(USER_AGENT)),
            ..HttpOptions::default()
        };
        Some(match self {
            Self::Standard => HttpTiles::with_options(sources::OpenStreetMap, options, ctx),
            Self::Topographic => {
                HttpTiles::with_options(sources::OpenTopoMap(OpenTopoServer::A), options, ctx)
            }
            Self::Satellite | Self::Hybrid => HttpTiles::with_options(
                MapTiler {
                    map: self.maptiler_map()?,
                    key: key.filter(|k| !k.trim().is_empty())?.trim().to_owned(),
                },
                options,
                ctx,
            ),
        })
    }
}

/// MapTiler's raster tiles of one map (https://docs.maptiler.com/cloud/api/maps/).
struct MapTiler {
    map: &'static str,
    key: String,
}

impl TileSource for MapTiler {
    fn tile_url(&self, t: TileId) -> String {
        format!(
            "https://api.maptiler.com/maps/{}/256/{}/{}/{}.jpg?key={}",
            self.map, t.zoom, t.x, t.y, self.key
        )
    }

    fn attribution(&self) -> WalkersAttribution {
        WalkersAttribution {
            text: "© MapTiler © OpenStreetMap contributors",
            url: "https://www.maptiler.com/copyright/",
            logo_light: None,
            logo_dark: None,
        }
    }
}

/// What a test of a MapTiler key found.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    Works,
    /// Refused (401/403): wrong, restricted or removed.
    Refused,
    /// The month's free tiles are used up (429).
    UsedUp,
    /// No answer, or an unexpected one.
    Failed(String),
}

/// Asks MapTiler for one tile with this key (blocking: call it off the UI thread).
pub fn check_key(key: &str) -> KeyCheck {
    let Some(url) = Style::Satellite.test_url(key.trim()) else {
        return KeyCheck::Failed("no URL".into());
    };
    let client = match reqwest::blocking::Client::builder()
        .user_agent(USER_AGENT)
        .timeout(std::time::Duration::from_secs(15))
        .build()
    {
        Ok(c) => c,
        Err(e) => return KeyCheck::Failed(e.to_string()),
    };
    match client.get(url).send() {
        Ok(r) => match r.status().as_u16() {
            200..=299 => KeyCheck::Works,
            401 | 403 => KeyCheck::Refused,
            429 => KeyCheck::UsedUp,
            s => KeyCheck::Failed(format!("MapTiler answered {s}")),
        },
        // The error text holds the URL, and the URL the key: say only what kind it was.
        Err(e) if e.is_timeout() => KeyCheck::Failed("no answer from MapTiler".into()),
        Err(_) => KeyCheck::Failed("MapTiler can’t be reached".into()),
    }
}

/// Every style's cache folder, for the data folder's bookkeeping.
pub fn cache_folders() -> impl Iterator<Item = &'static str> {
    Style::ALL.iter().map(|s| s.cache_folder())
}

/// The MapTiler logo as cached in the data folder.
pub fn logo_path(data: &Path) -> PathBuf {
    data.join("tiles-satellite").join("maptiler-logo.svg")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn user_agent_names_the_app_and_its_repo() {
        assert!(USER_AGENT.starts_with("MapCraft/"));
        assert!(USER_AGENT.contains("github.com/mariokoehler/MapCraft"));
        assert!(
            !USER_AGENT.contains('@'),
            "no e-mail address in the User-Agent"
        );
        HeaderValue::from_static(USER_AGENT); // panics if not a valid header value
    }

    #[test]
    fn styles_their_caches_and_keys() {
        let dir = tempfile::tempdir().unwrap();
        let ctx = egui::Context::default();
        assert!(
            Style::Satellite
                .tiles(dir.path(), None, ctx.clone())
                .is_none()
        );
        assert!(
            Style::Hybrid
                .tiles(dir.path(), Some("  "), ctx.clone())
                .is_none()
        );
        assert!(
            Style::Topographic
                .tiles(dir.path(), None, ctx.clone())
                .is_some()
        );
        let folders: std::collections::HashSet<_> = cache_folders().collect();
        assert_eq!(folders.len(), 4, "each style its own cache");
        assert_eq!(
            Style::Standard.cache_folder(),
            "tiles",
            "the cache it always had"
        );
        let t = MapTiler {
            map: "satellite",
            key: "K".into(),
        };
        assert_eq!(
            t.tile_url(TileId {
                x: 1,
                y: 2,
                zoom: 3
            }),
            "https://api.maptiler.com/maps/satellite/256/3/1/2.jpg?key=K"
        );
        assert_eq!(
            toml::to_string(&Holder {
                s: Style::Topographic
            })
            .unwrap()
            .trim(),
            "s = \"topographic\""
        );
    }

    #[derive(Serialize)]
    struct Holder {
        s: Style,
    }
}

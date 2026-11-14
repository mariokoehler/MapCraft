//! The tile-provider abstraction (design.md §3.5(b)). For v1 there is one provider,
//! OpenStreetMap; keyed providers and offline PMTiles come later as more variants.
//!
//! An `enum` here is like a Java enum whose constants could each carry their own data (a later
//! `MapTiler { api_key }` variant, say), and `match` over it must handle every variant.

use std::path::Path;

use walkers::{HeaderValue, HttpOptions, HttpTiles, sources};

/// Sent with every tile request. OSM's tile policy requires an identifying User-Agent; a library
/// default gets blocked (verified 2026-11-14, https://operations.osmfoundation.org/policies/tiles/).
pub const USER_AGENT: &str = concat!(
    "MapCraft/",
    env!("CARGO_PKG_VERSION"),
    " (+https://github.com/mariokoehler/MapCraft)"
);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Provider {
    #[default]
    OpenStreetMap,
}

/// What the map must show for a provider: its required credit, `before` + `link` + `after`,
/// where `link` is drawn as a link to `url`.
pub struct Attribution {
    pub before: &'static str,
    pub link: &'static str,
    pub after: &'static str,
    pub url: &'static str,
}

impl Provider {
    pub fn attribution(self) -> Attribution {
        match self {
            Self::OpenStreetMap => Attribution {
                before: "© ",
                link: "OpenStreetMap",
                after: " contributors",
                url: "https://www.openstreetmap.org/copyright",
            },
        }
    }

    /// The tile loader, with our User-Agent and an HTTP cache in `cache_dir` that honours the
    /// server's caching headers (walkers uses `http-cache` in its default mode).
    pub fn tiles(self, cache_dir: &Path, ctx: egui::Context) -> HttpTiles {
        let options = HttpOptions {
            cache: Some(cache_dir.to_path_buf()),
            user_agent: Some(HeaderValue::from_static(USER_AGENT)),
            ..HttpOptions::default()
        };
        match self {
            Self::OpenStreetMap => HttpTiles::with_options(sources::OpenStreetMap, options, ctx),
        }
    }
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
}

//! Search (design.md §2.5): places from the offline GeoNames index, and the user's photos by
//! folder, period, camera and file name. Runs on its own thread with its own connections; each
//! answer carries the text it answers, so the UI only acts on results for what's typed now.
//!
//! Nothing typed or found is logged: place and folder names are as personal as coordinates
//! (design.md §3.8).

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use crate::filter::Filter;
use crate::import::Bounds;
use crate::library::{FoundPhoto, FoundSet, Library};
use crate::places::{Place, Places};

/// A place with how many of the user's photos are near it.
#[derive(Debug, Clone, PartialEq)]
pub struct PlaceHit {
    pub place: Place,
    pub photos: usize,
    /// Whether to name the region too (another hit has the same name and country).
    pub with_region: bool,
}

/// One result about the user's photos.
#[derive(Debug, Clone, PartialEq)]
pub enum PhotoHit {
    Folder(FoundSet),
    Period(FoundSet),
    Camera(FoundSet),
    File(FoundPhoto),
}

#[derive(Debug, Clone, PartialEq)]
pub struct Results {
    pub text: String,
    /// The filter generation it was answered for (a filter change makes older answers stale).
    pub generation: u64,
    pub places: Vec<PlaceHit>,
    /// The place index isn't there (not downloaded yet).
    pub no_places: bool,
    pub photos: Vec<PhotoHit>,
}

/// Places with a name like the typed text, at most.
const PLACES: usize = 4;
/// Candidates looked at before ranking by photos nearby.
const PLACE_CANDIDATES: usize = 40;

/// The radius in km around a place within which a photo counts as "near" it, by population
/// (design.md §2.5, unconfirmed): 2 km for a village up to 25 km for a big city.
pub fn near_km(population: i64) -> f64 {
    (2.0 + (population.max(0) as f64).sqrt() / 100.0).min(25.0)
}

/// The box `km` around a position.
pub fn around(lat: f64, lon: f64, km: f64) -> Bounds {
    let dlat = km / 111.0;
    let dlon = km / (111.0 * lat.to_radians().cos().max(0.01));
    Bounds {
        min_lat: lat - dlat,
        max_lat: lat + dlat,
        min_lon: lon - dlon,
        max_lon: lon + dlon,
    }
}

/// A typed period: `2019`, `2019-08` or `2019-08-14` (ISO order), returned in the form the
/// library stores dates in.
pub fn period(text: &str) -> Option<String> {
    let t = text.trim();
    let parts: Vec<&str> = t.split('-').collect();
    let digits = |s: &str, n: usize| s.len() == n && s.bytes().all(|b| b.is_ascii_digit());
    let ok = match parts.as_slice() {
        [y] => digits(y, 4),
        [y, m] => digits(y, 4) && digits(m, 2) && (1..=12).contains(&m.parse::<u32>().ok()?),
        [y, m, d] => {
            digits(y, 4)
                && digits(m, 2)
                && digits(d, 2)
                && (1..=12).contains(&m.parse::<u32>().ok()?)
                && (1..=31).contains(&d.parse::<u32>().ok()?)
        }
        _ => false,
    };
    ok.then(|| t.to_owned())
}

/// Answers one query from `lib` and `places`, about the photos `filter` shows.
pub fn answer(lib: &Library, places: Option<&Places>, text: &str, filter: &Filter) -> Results {
    let t = text.trim();
    let mut out = Results {
        text: text.to_owned(),
        generation: 0,
        places: Vec::new(),
        no_places: places.is_none(),
        photos: Vec::new(),
    };
    if t.chars().count() < 2 {
        return out;
    }
    if let Some(places) = places {
        let mut hits: Vec<PlaceHit> = places
            .search(t, PLACE_CANDIDATES)
            .into_iter()
            .map(|place| {
                let b = around(place.lat, place.lon, near_km(place.population));
                let photos = lib.count_in(b, filter).unwrap_or(0);
                PlaceHit {
                    place,
                    photos,
                    with_region: false,
                }
            })
            .collect();
        // Places with the user's photos first (most first); the rest keep the index's order.
        hits.sort_by_key(|h| std::cmp::Reverse(h.photos));
        hits.truncate(PLACES);
        for i in 0..hits.len() {
            let same = |j: usize| {
                hits[j].place.name == hits[i].place.name
                    && hits[j].place.country == hits[i].place.country
            };
            hits[i].with_region = (0..hits.len()).any(|j| j != i && same(j));
        }
        out.places = hits;
    }
    if let Some(p) = period(t)
        && let Ok(sets) = lib.search_period(&p, filter)
    {
        out.photos.extend(sets.into_iter().map(PhotoHit::Period));
    }
    if let Ok(sets) = lib.search_folders(t, 2, filter) {
        out.photos.extend(sets.into_iter().map(PhotoHit::Folder));
    }
    if let Ok(sets) = lib.search_cameras(t, 1, filter) {
        out.photos.extend(sets.into_iter().map(PhotoHit::Camera));
    }
    if let Ok(files) = lib.search_files(t, 3, filter) {
        out.photos.extend(files.into_iter().map(PhotoHit::File));
    }
    out
}

enum Request {
    Query {
        text: String,
        filter: Filter,
        generation: u64,
    },
    /// The place index appeared (downloaded): open it.
    Reopen,
}

/// The search thread: requests in, newest answer out.
pub struct Searcher {
    requests: Sender<Request>,
    results: Receiver<Results>,
}

impl Searcher {
    pub fn new(db: PathBuf, places: PathBuf, ctx: egui::Context) -> Self {
        let (requests, rx) = mpsc::channel::<Request>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("mapcraft-search".into())
            .spawn(move || run(&db, &places, rx, tx, ctx))
            .expect("spawn the search thread");
        Self { requests, results }
    }

    /// Asks for `text` among the photos `filter` shows; the answer carries `generation`.
    pub fn ask(&self, text: &str, filter: &Filter, generation: u64) {
        let _ = self.requests.send(Request::Query {
            text: text.to_owned(),
            filter: filter.clone(),
            generation,
        });
    }

    /// The place index was (re)built.
    pub fn reopen_places(&self) {
        let _ = self.requests.send(Request::Reopen);
    }

    /// The newest answer since last asked, if any.
    pub fn latest(&self) -> Option<Results> {
        self.results.try_iter().last()
    }
}

fn run(db: &Path, places: &Path, rx: Receiver<Request>, tx: Sender<Results>, ctx: egui::Context) {
    let Ok(lib) = Library::open(db) else {
        return;
    };
    let mut index = Places::open(places);
    while let Ok(first) = rx.recv() {
        // Only the newest query matters; a reopen is never skipped.
        let mut query = None;
        for r in std::iter::once(first).chain(rx.try_iter()) {
            match r {
                Request::Query {
                    text,
                    filter,
                    generation,
                } => query = Some((text, filter, generation)),
                Request::Reopen => index = Places::open(places),
            }
        }
        let Some((q, filter, generation)) = query else {
            continue;
        };
        let answer = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            self::answer(&lib, index.as_ref(), &q, &filter)
        }));
        let Ok(mut a) = answer else { continue };
        a.generation = generation;
        if tx.send(a).is_err() {
            break;
        }
        ctx.request_repaint();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::scope::Scope;

    #[test]
    fn periods_and_radii() {
        assert_eq!(period("2019").as_deref(), Some("2019"));
        assert_eq!(period(" 2019-08 ").as_deref(), Some("2019-08"));
        assert_eq!(period("2019-08-14").as_deref(), Some("2019-08-14"));
        for no in [
            "19",
            "2019-13",
            "2019-8",
            "2019-08-32",
            "Hamburg",
            "2019-08-14-1",
        ] {
            assert_eq!(period(no), None, "{no}");
        }
        assert!((near_km(750) - 2.27).abs() < 0.01);
        assert_eq!(near_km(10_000_000), 25.0);
        let b = around(64.0, -21.0, 10.0);
        assert!((b.max_lat - b.min_lat - 0.18).abs() < 0.01);
        assert!(
            b.max_lon - b.min_lon > 0.4,
            "longitude degrees are short up north"
        );
    }

    #[test]
    fn photos_near_a_place_rank_it_first_and_photos_are_found() {
        let dir = tempfile::tempdir().unwrap();
        // Demo photos: folders named after trips, e.g. 2023-05-19_Hamburg, around the cities.
        let photos = dir.path().join("photos");
        crate::demo::make_demo(&photos, 32).unwrap();
        let mut lib = Library::open(&dir.path().join("library.sqlite")).unwrap();
        let src = lib
            .add_source(&photos, None, "Demo", &Scope::all())
            .unwrap();
        crate::import::run(&mut lib, src, &Default::default(), 2, |_| {}).unwrap();
        let places = Places::open(&crate::places::tests::index(dir.path())).unwrap();

        let r = answer(&lib, Some(&places), "hamb", &Filter::default());
        assert_eq!(r.places[0].place.name, "Hamburg");
        assert!(r.places[0].photos > 0, "the demo photos around Hamburg");
        assert_eq!(r.places[0].place.country, "Germany");
        // The two US Hamburgs share name and country: they say their state.
        let us: Vec<_> = r
            .places
            .iter()
            .filter(|h| h.place.country == "United States")
            .collect();
        assert!(us.iter().all(|h| h.with_region));
        // The trip folder, by a part of its name.
        assert!(r.photos.iter().any(
            |h| matches!(h, PhotoHit::Folder(f) if f.label == "2023-05-19_Hamburg" && f.photos == 4)
        ));
        // Periods and file names.
        let r = answer(&lib, Some(&places), "2023-05", &Filter::default());
        assert!(matches!(&r.photos[0], PhotoHit::Period(p) if p.photos == 4));
        let r = answer(&lib, None, "img_0000", &Filter::default());
        assert!(r.no_places && r.places.is_empty());
        let files: Vec<&str> = r
            .photos
            .iter()
            .filter_map(|h| match h {
                PhotoHit::File(f) => Some(f.name.as_str()),
                _ => None,
            })
            .collect();
        assert_eq!(files.len(), 3);
        assert!(files.iter().all(|f| f.starts_with("IMG_0000")));
        // Wildcards are text.
        assert!(
            answer(&lib, None, "%", &Filter::default())
                .photos
                .is_empty()
        );
        assert!(
            answer(&lib, None, "IMG_", &Filter::default())
                .photos
                .iter()
                .all(|h| !matches!(h, PhotoHit::Folder(_)))
        );
        // Filters narrow the positions, the place counts and every kind of photo result.
        let facets = lib.facets().unwrap();
        // Every eighth demo photo (the Barcelona trip) has no position.
        assert_eq!(facets.months.len(), 7, "one month per trip");
        assert_eq!(facets.years(), [2024, 2023, 2022, 2021, 2020, 2019]);
        assert_eq!(facets.cameras.len(), 3);
        assert_eq!(facets.sources[&src], 28);
        assert_eq!(lib.positions().unwrap().len(), 28);
        let year_2023 = Filter {
            dates: Some((202301, 202312)),
            ..Filter::default()
        };
        assert_eq!(lib.positions_shown(&year_2023).unwrap().len(), 8);
        let r = answer(&lib, Some(&places), "hamb", &year_2023);
        assert!(r.places[0].photos > 0, "Hamburg's photos are from 2023");
        let year_2019 = Filter {
            dates: Some((201901, 201912)),
            ..Filter::default()
        };
        let r = answer(&lib, Some(&places), "hamb", &year_2019);
        assert!(r.places.iter().all(|h| h.photos == 0));
        assert!(r.photos.is_empty(), "no folder of 2023 in 2019");
        assert!(answer(&lib, None, "2023-05", &year_2019).photos.is_empty());
        let mut no_canon = Filter::default();
        no_canon.hidden_cameras.insert(crate::filter::camera_key(
            Some("Canon"),
            Some("Canon EOS R5"),
        ));
        assert!(
            answer(&lib, None, "canon", &Filter::default())
                .photos
                .iter()
                .any(|h| matches!(h, PhotoHit::Camera(_)))
        );
        assert!(
            !answer(&lib, None, "canon", &no_canon)
                .photos
                .iter()
                .any(|h| matches!(h, PhotoHit::Camera(_)))
        );
        let mut no_source = Filter::default();
        no_source.hidden_sources.insert(src);
        assert!(lib.positions_shown(&no_source).unwrap().is_empty());
        assert!(answer(&lib, None, "img_0000", &no_source).photos.is_empty());
    }
}

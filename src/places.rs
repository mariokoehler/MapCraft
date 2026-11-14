//! Offline place search (design.md §2.5): the GeoNames cities dataset (CC BY 4.0,
//! https://download.geonames.org/export/dump/, checked 2026-11-14) in a SQLite full-text index
//! of MapCraft's own, `places.sqlite` in the data folder.
//!
//! - `build` makes the index from the three GeoNames files (`cities500.txt`, unpacked;
//!   `admin1CodesASCII.txt`; `countryInfo.txt`), given as readers, so tests use tiny inline files
//!   and never the network.
//! - `download` fetches them; only when the user asks (it's a connection to geonames.org).
//! - `Places::search` answers a typed prefix with the most populous matches.
//!
//! The data never goes into the repo; GeoNames is credited wherever place results show.

use std::collections::{HashMap, HashSet};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};

use rusqlite::{Connection, params};

/// The index format; a different number rebuilds it.
const FORMAT: i32 = 4;

/// Where the files live, below the data folder.
pub fn dir(data: &Path) -> PathBuf {
    data.join("geonames")
}

pub fn index_path(data: &Path) -> PathBuf {
    dir(data).join("places.sqlite")
}

/// A place as search shows it.
#[derive(Debug, Clone, PartialEq)]
pub struct Place {
    pub id: i64,
    pub name: String,
    /// The state or region (GeoNames' admin1), when known.
    pub region: Option<String>,
    pub country: String,
    pub lat: f64,
    pub lon: f64,
    pub population: i64,
    /// One of its names starts with the typed text (not just a later word).
    pub starts: bool,
}

impl Place {
    /// "Hamburg, Germany"; with the region when `with_region` ("Hamburg, New York, United
    /// States"), for telling apart places of one name in one country.
    pub fn label(&self, with_region: bool) -> String {
        match (&self.region, with_region) {
            (Some(r), true) if r != &self.name => format!("{}, {r}, {}", self.name, self.country),
            _ => format!("{}, {}", self.name, self.country),
        }
    }
}

/// Alternate names kept in the index: Latin-script ones (`Wien`, `München`, `Lisboa`), up to
/// 40 characters. Other scripts would multiply the index for names nobody here types.
fn keep_alternate(name: &str) -> bool {
    !name.is_empty()
        && name.chars().count() <= 40
        && name
            .chars()
            .all(|c| (c as u32) < 0x0250 && !c.is_ascii_digit())
}

/// Builds the index at `target` (written to `<target>.tmp`, then renamed). Returns the number of
/// places.
pub fn build(
    cities: impl Read,
    admin1: impl Read,
    countries: impl Read,
    target: &Path,
) -> Result<usize, String> {
    let e = |e: rusqlite::Error| e.to_string();
    // Country code → name; "DE.04" → region name.
    let mut country_names = HashMap::new();
    for line in BufReader::new(countries).lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.starts_with('#') {
            continue;
        }
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() > 4 {
            country_names.insert(f[0].to_owned(), f[4].to_owned());
        }
    }
    let mut regions = HashMap::new();
    for line in BufReader::new(admin1).lines() {
        let line = line.map_err(|e| e.to_string())?;
        let f: Vec<&str> = line.split('\t').collect();
        if f.len() > 1 {
            regions.insert(f[0].to_owned(), f[1].to_owned());
        }
    }

    let tmp = target.with_extension("sqlite.tmp");
    let _ = std::fs::remove_file(&tmp);
    let mut conn = Connection::open(&tmp).map_err(e)?;
    conn.execute_batch(
        "PRAGMA journal_mode = OFF;
         PRAGMA synchronous = OFF;
         CREATE TABLE place (
             id INTEGER PRIMARY KEY, name TEXT NOT NULL, region TEXT, country TEXT NOT NULL,
             lat REAL NOT NULL, lon REAL NOT NULL, population INTEGER NOT NULL);
         -- One row per name (own, ASCII, alternates), so `^` can match a name's start. Names
         -- only, no copy of the text (contentless); `name_of` maps a row to its place.
         -- unicode61 with remove_diacritics folds `Vík` to `vik`; the ASCII name covers Þ, ð.
         CREATE TABLE name_of (rowid INTEGER PRIMARY KEY, place INTEGER NOT NULL,
                               own INTEGER NOT NULL);
         CREATE VIRTUAL TABLE place_name USING fts5(
             name, content = '', tokenize = 'unicode61 remove_diacritics 2');",
    )
    .map_err(e)?;
    let mut n = 0;
    {
        let tx = conn.transaction().map_err(e)?;
        {
            let mut place = tx
                .prepare("INSERT INTO place VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7)")
                .map_err(e)?;
            let mut name_of = tx
                .prepare("INSERT INTO name_of (place, own) VALUES (?1, ?2)")
                .map_err(e)?;
            let mut names = tx
                .prepare("INSERT INTO place_name (rowid, name) VALUES (?1, ?2)")
                .map_err(e)?;
            for line in BufReader::new(cities).lines() {
                let line = line.map_err(|e| e.to_string())?;
                let f: Vec<&str> = line.split('\t').collect();
                if f.len() < 15 {
                    continue; // not a geoname row
                }
                let (Ok(id), Ok(lat), Ok(lon)) = (
                    f[0].parse::<i64>(),
                    f[4].parse::<f64>(),
                    f[5].parse::<f64>(),
                ) else {
                    continue;
                };
                let population = f[14].parse::<i64>().unwrap_or(0);
                let country = country_names
                    .get(f[8])
                    .cloned()
                    .unwrap_or_else(|| f[8].to_owned());
                let region = regions.get(&format!("{}.{}", f[8], f[10])).cloned();
                place
                    .execute(params![id, f[1], region, country, lat, lon, population])
                    .map_err(e)?;
                let mut seen = HashSet::new();
                let all = [(f[1], true), (f[2], true)]
                    .into_iter()
                    .chain(
                        f[3].split(',')
                            .filter(|a| keep_alternate(a))
                            .map(|a| (a, false)),
                    )
                    .filter(|(a, _)| seen.insert(a.to_lowercase()));
                for (name, own) in all {
                    name_of.execute(params![id, own]).map_err(e)?;
                    names
                        .execute(params![tx.last_insert_rowid(), name])
                        .map_err(e)?;
                }
                n += 1;
            }
        }
        tx.commit().map_err(e)?;
    }
    conn.execute_batch(&format!(
        "INSERT INTO place_name (place_name) VALUES ('optimize'); PRAGMA user_version = {FORMAT};"
    ))
    .map_err(e)?;
    drop(conn);
    std::fs::rename(&tmp, target).map_err(|e| e.to_string())?;
    Ok(n)
}

/// The FTS5 query for typed text: every word as a quoted prefix (`"hamb"*`), so quotes,
/// dashes, colons, brackets and words like AND are text, never query syntax.
pub fn match_query(text: &str) -> Option<String> {
    let words: Vec<String> = text
        .split(|c: char| !c.is_alphanumeric())
        .filter(|w| !w.is_empty())
        .map(|w| format!("\"{}\"*", w.replace('"', "\"\"")))
        .collect();
    (!words.is_empty()).then(|| words.join(" "))
}

/// An open index.
pub struct Places {
    conn: Connection,
}

impl Places {
    /// `None` if there's no index yet (or one of an older format).
    pub fn open(path: &Path) -> Option<Self> {
        if !path.exists() {
            return None;
        }
        let conn =
            Connection::open_with_flags(path, rusqlite::OpenFlags::SQLITE_OPEN_READ_ONLY).ok()?;
        let v: i32 = conn
            .query_row("PRAGMA user_version", [], |r| r.get(0))
            .ok()?;
        (v == FORMAT).then_some(Self { conn })
    }

    /// Up to `limit` places matching `text` (two characters at least), by population weighted
    /// by how they match (design.md §2.5, unconfirmed): the place's own name starts with it × 1,
    /// another of its names does × 0.3 (`Wien` → Vienna), a later word of its own name × 0.1,
    /// of another name × 0.03. The search ranks them further by the user's photos nearby.
    pub fn search(&self, text: &str, limit: usize) -> Vec<Place> {
        let Some(any) = match_query(text) else {
            return Vec::new();
        };
        if text.trim().chars().count() < 2 {
            return Vec::new();
        }
        let start = format!("^{any}");
        let run = || -> rusqlite::Result<Vec<Place>> {
            let mut stmt = self.conn.prepare_cached(
                "WITH hits(place, starts, w) AS (
                     SELECT n.place, 1, CASE WHEN n.own THEN 1.0 ELSE 0.3 END
                     FROM place_name f JOIN name_of n ON n.rowid = f.rowid
                     WHERE place_name MATCH ?1
                     UNION ALL
                     SELECT n.place, 0, CASE WHEN n.own THEN 0.1 ELSE 0.03 END
                     FROM place_name f JOIN name_of n ON n.rowid = f.rowid
                     WHERE place_name MATCH ?2)
                 SELECT p.id, p.name, p.region, p.country, p.lat, p.lon, p.population,
                        max(h.starts)
                 FROM hits h JOIN place p ON p.id = h.place
                 GROUP BY p.id ORDER BY max(h.w) * (p.population + 100) DESC, p.id LIMIT ?3",
            )?;
            stmt.query_map(params![start, any, limit as i64], |r| {
                Ok(Place {
                    id: r.get(0)?,
                    name: r.get(1)?,
                    region: r.get(2)?,
                    country: r.get(3)?,
                    lat: r.get(4)?,
                    lon: r.get(5)?,
                    population: r.get(6)?,
                    starts: r.get::<_, i64>(7)? == 1,
                })
            })?
            .collect()
        };
        run().unwrap_or_default()
    }
}

/// The first file in a zip archive, unpacked (GeoNames' zips hold one file). Untrusted input:
/// the signature and method are checked and the output is capped at `max` bytes.
pub fn unzip_first(mut zip: impl Read, max: u64) -> Result<impl Read, String> {
    let mut h = [0u8; 30];
    zip.read_exact(&mut h).map_err(|e| e.to_string())?;
    if h[..4] != [0x50, 0x4b, 0x03, 0x04] {
        return Err("not a zip file".into());
    }
    let method = u16::from_le_bytes([h[8], h[9]]);
    if method != 8 {
        return Err(format!("unexpected compression method {method}"));
    }
    let name_len = u16::from_le_bytes([h[26], h[27]]) as usize;
    let extra_len = u16::from_le_bytes([h[28], h[29]]) as usize;
    let mut skip = vec![0u8; name_len + extra_len];
    zip.read_exact(&mut skip).map_err(|e| e.to_string())?;
    // The deflate stream ends by itself, whatever the header says about its size.
    Ok(flate2::read::DeflateDecoder::new(zip).take(max))
}

/// The GeoNames files and where they come from.
const BASE: &str = "https://download.geonames.org/export/dump/";

/// Downloads the three files into `dir(data)` and builds the index. `progress` gets the bytes
/// of `cities500.zip` read so far and its size. Nothing about the user is sent: three fixed
/// URLs and a generic User-Agent.
pub fn download(data: &Path, progress: &dyn Fn(u64, u64)) -> Result<usize, String> {
    let dir = dir(data);
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let client = reqwest::blocking::Client::builder()
        .user_agent("MapCraft (photo map; https://github.com/mariokoehler/MapCraft)")
        .connect_timeout(std::time::Duration::from_secs(20))
        .timeout(std::time::Duration::from_secs(600))
        .build()
        .map_err(|e| e.to_string())?;
    let fetch = |name: &str, report: bool| -> Result<PathBuf, String> {
        let mut resp = client
            .get(format!("{BASE}{name}"))
            .send()
            .and_then(|r| r.error_for_status())
            .map_err(|e| format!("can't download {name}: {e}"))?;
        let total = resp.content_length().unwrap_or(0);
        let path = dir.join(name);
        let tmp = path.with_extension("part");
        let mut out = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        let mut buf = vec![0u8; 64 * 1024];
        let mut done = 0u64;
        loop {
            let n = resp.read(&mut buf).map_err(|e| e.to_string())?;
            if n == 0 {
                break;
            }
            std::io::Write::write_all(&mut out, &buf[..n]).map_err(|e| e.to_string())?;
            done += n as u64;
            if done > 200 << 20 {
                return Err(format!("{name} is larger than expected"));
            }
            if report {
                progress(done, total);
            }
        }
        drop(out);
        std::fs::rename(&tmp, &path).map_err(|e| e.to_string())?;
        Ok(path)
    };
    let admin1 = fetch("admin1CodesASCII.txt", false)?;
    let countries = fetch("countryInfo.txt", false)?;
    let zip = fetch("cities500.zip", true)?;
    let open = |p: &Path| std::fs::File::open(p).map_err(|e| e.to_string());
    let cities = unzip_first(BufReader::new(open(&zip)?), 400 << 20)?;
    let n = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        build(cities, open(&admin1)?, open(&countries)?, &index_path(data))
    }))
    .map_err(|_| "building the place index crashed".to_owned())??;
    for p in [zip, admin1, countries] {
        let _ = std::fs::remove_file(p);
    }
    Ok(n)
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;

    /// A few rows in GeoNames' format (19 columns), our own.
    pub(crate) const CITIES: &str = "\
2911298\tHamburg\tHamburg\tHamborg,Hambourg,Amburgo\t53.55073\t9.99302\tP\tPPLA\tDE\t\t04\t\t\t\t1845229\t\t8\tEurope/Berlin\t2024-01-01
2911297\tHamburg-Altona\tHamburg-Altona\tAltona\t53.5503\t9.9351\tP\tPPLX\tDE\t\t04\t\t\t\t250000\t\t15\tEurope/Berlin\t2024-01-01
5119347\tHamburg\tHamburg\t\t42.7159\t-78.8295\tP\tPPL\tUS\t\tNY\t\t\t\t9409\t\t197\tAmerica/New_York\t2024-01-01
5119348\tHamburg\tHamburg\t\t40.5559\t-75.9821\tP\tPPL\tUS\t\tPA\t\t\t\t4289\t\t118\tAmerica/New_York\t2024-01-01
2761369\tVienna\tVienna\tWien,Vienne,Вена\t48.20849\t16.37208\tP\tPPLC\tAT\t\t09\t\t\t\t1691468\t\t171\tEurope/Vienna\t2024-01-01
2867714\tMunich\tMunich\tMuenchen,München\t48.13743\t11.57549\tP\tPPLA\tDE\t\t02\t\t\t\t1505005\t\t524\tEurope/Berlin\t2024-01-01
3412870\tVík í Mýrdal\tVik i Myrdal\t\t63.4186\t-19.006\tP\tPPLA\tIS\t\t40\t\t\t\t750\t\t37\tAtlantic/Reykjavik\t2024-01-01
2628888\tÞorlákshöfn\tThorlakshoefn\t\t63.8558\t-21.3836\tP\tPPL\tIS\t\t41\t\t\t\t1700\t\t9\tAtlantic/Reykjavik\t2024-01-01
";
    pub(crate) const ADMIN1: &str = "DE.04\tHamburg\tHamburg\t2911297\nUS.NY\tNew York\tNew York\t5128638\nUS.PA\tPennsylvania\tPennsylvania\t6254927\nAT.09\tVienna\tVienna\t2761367\n";
    pub(crate) const COUNTRIES: &str = "#ISO\tISO3\tISO-Numeric\tfips\tCountry\n\
DE\tDEU\t276\tGM\tGermany\nUS\tUSA\t840\tUS\tUnited States\nAT\tAUT\t040\tAU\tAustria\nIS\tISL\t352\tIC\tIceland\n";

    pub(crate) fn index(dir: &Path) -> PathBuf {
        let path = dir.join("places.sqlite");
        build(
            CITIES.as_bytes(),
            ADMIN1.as_bytes(),
            COUNTRIES.as_bytes(),
            &path,
        )
        .unwrap();
        path
    }

    #[test]
    fn prefixes_find_the_most_populous_first() {
        let dir = tempfile::tempdir().unwrap();
        let p = Places::open(&index(dir.path())).unwrap();
        let hits = p.search("hamb", 5);
        let labels: Vec<String> = hits.iter().map(|h| h.label(false)).collect();
        assert_eq!(
            labels,
            [
                "Hamburg, Germany",
                "Hamburg-Altona, Germany",
                "Hamburg, United States",
                "Hamburg, United States"
            ]
        );
        assert_eq!(hits[2].label(true), "Hamburg, New York, United States");
        // Two words narrow it down.
        assert_eq!(p.search("hamburg alto", 5).len(), 1);
        assert!(p.search("h", 5).is_empty(), "one letter is too little");
    }

    #[test]
    fn other_names_and_diacritics_match() {
        let dir = tempfile::tempdir().unwrap();
        let p = Places::open(&index(dir.path())).unwrap();
        for (typed, found) in [
            ("Wien", "Vienna"),
            ("münchen", "Munich"),
            ("muench", "Munich"),
            ("vik", "Vík í Mýrdal"),
            ("Vík í", "Vík í Mýrdal"),
            ("thorlak", "Þorlákshöfn"),
        ] {
            let hits = p.search(typed, 3);
            assert_eq!(
                hits.first().map(|h| h.name.as_str()),
                Some(found),
                "{typed}"
            );
        }
        // Cyrillic alternate names aren't indexed.
        assert!(p.search("Вена", 3).is_empty());
    }

    #[test]
    fn typed_text_is_never_query_syntax() {
        assert_eq!(match_query("hamb").as_deref(), Some("\"hamb\"*"));
        assert_eq!(
            match_query("a\"b NOT (c) -d:e").as_deref(),
            Some("\"a\"* \"b\"* \"NOT\"* \"c\"* \"d\"* \"e\"*")
        );
        assert_eq!(match_query(" -\"() "), None);
        let dir = tempfile::tempdir().unwrap();
        let p = Places::open(&index(dir.path())).unwrap();
        for nasty in ["\"", "AND", "ham OR", "NEAR(", "*", "ham-", "a:b"] {
            let _ = p.search(nasty, 5); // never panics, never errors out
        }
    }

    #[test]
    fn a_zip_is_checked_and_unpacked() {
        use std::io::Write;
        // A one-file zip: local header, name, deflate data (no central directory needed).
        let mut deflated = Vec::new();
        let mut enc =
            flate2::write::DeflateEncoder::new(&mut deflated, flate2::Compression::default());
        enc.write_all(CITIES.as_bytes()).unwrap();
        enc.finish().unwrap();
        let mut zip = vec![0x50, 0x4b, 0x03, 0x04, 20, 0, 0, 0, 8, 0];
        zip.extend([0u8; 16]); // time, date, crc, sizes
        zip.extend((10u16).to_le_bytes());
        zip.extend(0u16.to_le_bytes());
        zip.extend(b"cities.txt");
        zip.extend(&deflated);
        let mut out = String::new();
        unzip_first(zip.as_slice(), 1 << 20)
            .unwrap()
            .read_to_string(&mut out)
            .unwrap();
        assert_eq!(out, CITIES);
        // Capped.
        let mut capped = Vec::new();
        unzip_first(zip.as_slice(), 100)
            .unwrap()
            .read_to_end(&mut capped)
            .unwrap();
        assert_eq!(capped.len(), 100);
        assert!(unzip_first(&b"PK\x05\x06 not a local header......"[..], 10).is_err());
    }
}

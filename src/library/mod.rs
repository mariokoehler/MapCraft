//! The library: a SQLite database in the data folder holding sources and photos (design.md §3.4).
//!
//! `rusqlite` is a thin JDBC-like wrapper: a `Connection`, prepared statements with `?` params,
//! rows mapped by closures. SQLite runs in WAL mode, so the import thread can write while the UI
//! thread reads through its own connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

use crate::import::scope::Scope;

pub use rusqlite::Error;
pub type Result<T> = rusqlite::Result<T>;

/// The schema migrations, in order. `PRAGMA user_version` records how many have run.
const MIGRATIONS: [&str; 1] = [include_str!("migrations/0001_init.sql")];

pub struct Library {
    conn: Connection,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Source {
    pub id: i64,
    pub root_path: PathBuf,
    pub unc_path: Option<String>,
    pub label: String,
    pub last_scan_at: Option<String>,
    /// Which folders are imported (§2.8(f)).
    pub scope: Scope,
}

/// What the import needs to know about a photo that is already in the library.
#[derive(Debug, Clone, PartialEq)]
pub struct Existing {
    pub id: i64,
    pub rel_dir_key: String,
    pub files_sig: i64,
    pub file_size: i64,
    pub quick_hash: Option<i64>,
    pub missing: bool,
    /// The last read recorded an error: read it again on the next rescan.
    pub has_error: bool,
}

/// One photo as the import writes it.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct PhotoRecord {
    pub rel_dir: String,
    pub stem: String,
    pub original_name: String,
    pub preview_name: Option<String>,
    pub sidecar_name: Option<String>,
    pub files_sig: i64,
    pub file_size: i64,
    pub file_mtime: i64,
    pub quick_hash: Option<i64>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt: Option<f64>,
    pub gps_source: Option<&'static str>,
    pub taken_at: Option<String>,
    pub taken_at_offset: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u16>,
    pub read_error: Option<String>,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Counts {
    pub photos: i64,
    pub with_position: i64,
    pub missing: i64,
}

impl Counts {
    /// Photos present (not missing) without a position: they're in the library but not on the
    /// map. `photos` includes missing ones, which are neither.
    pub fn without_position(&self) -> i64 {
        self.photos - self.missing - self.with_position
    }
}

/// A photo's position, for drawing it on the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoPoint {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
    pub thumb: Thumb,
    /// The capture time as a sortable number, `yyyymmddhhmmss` (0 when undated).
    pub taken: i64,
}

/// One photo as the hover card and the detail panel show it.
#[derive(Debug, Clone, PartialEq)]
pub struct PhotoDetail {
    pub id: i64,
    pub source_label: String,
    pub root: PathBuf,
    pub unc: Option<String>,
    pub rel_dir: String,
    pub original_name: String,
    pub preview_name: Option<String>,
    pub file_size: i64,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub taken_at: Option<String>,
    pub taken_at_offset: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt: Option<f64>,
    pub gps_source: Option<String>,
    pub orientation: Option<u16>,
    pub thumb_made: bool,
}

/// A set of photos search found (a folder, a period, a camera).
#[derive(Debug, Clone, PartialEq)]
pub struct FoundSet {
    /// What identifies it (source id and folder, the period, make and model).
    pub key: String,
    pub label: String,
    pub photos: usize,
    pub bounds: Option<crate::import::Bounds>,
}

/// One photo search found by its file name.
#[derive(Debug, Clone, PartialEq)]
pub struct FoundPhoto {
    pub id: i64,
    pub name: String,
    pub taken_at: Option<String>,
    pub lat: f64,
    pub lon: f64,
}

/// A `LIKE` pattern for "contains `text`", lower-cased (the `*_key` columns are), with the
/// wildcards `%` and `_` in the text escaped.
pub fn like(text: &str) -> String {
    let t = text
        .trim()
        .to_lowercase()
        .replace('\\', "\\\\")
        .replace('%', "\\%")
        .replace('_', "\\_");
    format!("%{t}%")
}

/// A photo's thumbnail, as the map draws it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Thumb {
    /// Not made yet (or being remade): the photo shows as a dot.
    Pending,
    Ok,
    /// No usable source: a placeholder marker.
    Failed,
}

/// The matching key of a path part: Windows compares file names case-insensitively.
pub fn key(s: &str) -> String {
    s.to_lowercase()
}

impl Library {
    /// Opens (or creates) the library at `path` and brings its schema up to date.
    pub fn open(path: &Path) -> Result<Self> {
        if let Some(dir) = path.parent() {
            // A missing folder surfaces as SQLite's "unable to open" error just below.
            let _ = std::fs::create_dir_all(dir);
        }
        let conn = Connection::open(path)?;
        conn.pragma_update(None, "journal_mode", "WAL")?;
        Self::init(conn)
    }

    /// A throwaway library for tests.
    pub fn open_in_memory() -> Result<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> Result<Self> {
        conn.busy_timeout(Duration::from_secs(10))?;
        conn.pragma_update(None, "foreign_keys", true)?;
        let lib = Self { conn };
        lib.migrate()?;
        Ok(lib)
    }

    fn migrate(&self) -> Result<()> {
        let done: i64 = self
            .conn
            .pragma_query_value(None, "user_version", |r| r.get(0))?;
        for (i, sql) in MIGRATIONS.iter().enumerate().skip(done as usize) {
            self.conn.execute_batch(&format!(
                "BEGIN; {sql}\nPRAGMA user_version = {}; COMMIT;",
                i + 1
            ))?;
        }
        Ok(())
    }

    pub fn add_source(
        &mut self,
        root: &Path,
        unc_path: Option<&str>,
        label: &str,
        scope: &Scope,
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO source (root_path, unc_path, label, added_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            params![root.to_string_lossy(), unc_path, label],
        )?;
        let id = tx.last_insert_rowid();
        write_scope(&tx, id, scope)?;
        tx.commit()?;
        Ok(id)
    }

    /// Replaces a source's folder decisions ("Change folders…"). Photos in folders that are no
    /// longer included are removed from the library (their files are untouched); returns how
    /// many **(unconfirmed, design.md §2.8(f))**.
    pub fn set_scope(&mut self, source_id: i64, scope: &Scope) -> Result<usize> {
        let tx = self.conn.transaction()?;
        write_scope(&tx, source_id, scope)?;
        let dirs: Vec<(i64, String)> = tx
            .prepare("SELECT id, rel_dir_key FROM photo WHERE source_id = ?1")?
            .query_map([source_id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_>>()?;
        let mut removed = 0;
        {
            let mut del = tx.prepare("DELETE FROM photo WHERE id = ?1")?;
            for (id, dir) in dirs {
                if !scope.includes(&dir) {
                    removed += del.execute([id])?;
                }
            }
        }
        tx.commit()?;
        Ok(removed)
    }

    /// How many photos a new scope would remove, for the confirmation.
    pub fn count_out_of_scope(&self, source_id: i64, scope: &Scope) -> Result<usize> {
        let dirs: Vec<String> = self
            .conn
            .prepare("SELECT rel_dir_key FROM photo WHERE source_id = ?1")?
            .query_map([source_id], |r| r.get(0))?
            .collect::<Result<_>>()?;
        Ok(dirs.iter().filter(|d| !scope.includes(d)).count())
    }

    pub fn source(&self, id: i64) -> Result<Option<Source>> {
        let source = self
            .conn
            .query_row(
                "SELECT id, root_path, unc_path, label, last_scan_at FROM source WHERE id = ?1",
                [id],
                |r| {
                    Ok(Source {
                        id: r.get(0)?,
                        root_path: PathBuf::from(r.get::<_, String>(1)?),
                        unc_path: r.get(2)?,
                        label: r.get(3)?,
                        last_scan_at: r.get(4)?,
                        scope: Scope::all(),
                    })
                },
            )
            .optional()?;
        let Some(mut source) = source else {
            return Ok(None);
        };
        let rules: Vec<(String, bool)> = self
            .conn
            .prepare("SELECT rel_dir, included FROM source_folder WHERE source_id = ?1")?
            .query_map([id], |r| Ok((r.get(0)?, r.get(1)?)))?
            .collect::<Result<_>>()?;
        source.scope = Scope::from_rules(rules);
        Ok(Some(source))
    }

    pub fn sources(&self) -> Result<Vec<Source>> {
        let ids: Vec<i64> = self
            .conn
            .prepare("SELECT id FROM source ORDER BY id")?
            .query_map([], |r| r.get(0))?
            .collect::<Result<_>>()?;
        ids.into_iter()
            .filter_map(|id| self.source(id).transpose())
            .collect()
    }

    /// Removes a source and its photo records. Never touches files (design.md §1.1).
    pub fn remove_source(&self, id: i64) -> Result<()> {
        self.conn
            .execute("DELETE FROM source WHERE id = ?1", [id])?;
        Ok(())
    }

    /// Every photo of a source, by (folder key, name key), for the import's comparison.
    pub fn existing(&self, source_id: i64) -> Result<HashMap<(String, String), Existing>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, rel_dir_key, stem_key, files_sig, file_size, quick_hash, state,
                    read_error IS NOT NULL
             FROM photo WHERE source_id = ?1",
        )?;
        let rows = stmt.query_map([source_id], |r| {
            Ok((
                (r.get::<_, String>(1)?, r.get::<_, String>(2)?),
                Existing {
                    id: r.get(0)?,
                    rel_dir_key: r.get(1)?,
                    files_sig: r.get(3)?,
                    file_size: r.get(4)?,
                    quick_hash: r.get(5)?,
                    missing: r.get::<_, String>(6)? == "missing",
                    has_error: r.get(7)?,
                },
            ))
        })?;
        rows.collect()
    }

    pub fn transaction(&mut self) -> Result<Transaction<'_>> {
        self.conn.transaction()
    }

    pub fn set_last_scan(&self, source_id: i64) -> Result<()> {
        self.conn.execute(
            "UPDATE source SET last_scan_at = strftime('%Y-%m-%dT%H:%M:%SZ', 'now') WHERE id = ?1",
            [source_id],
        )?;
        Ok(())
    }

    pub fn counts(&self) -> Result<Counts> {
        self.conn.query_row(
            "SELECT count(*),
                    count(lat) FILTER (WHERE state = 'ok'),
                    count(*) FILTER (WHERE state = 'missing')
             FROM photo",
            [],
            |r| {
                Ok(Counts {
                    photos: r.get(0)?,
                    with_position: r.get(1)?,
                    missing: r.get(2)?,
                })
            },
        )
    }

    /// Counts per source, for the sources list.
    pub fn source_counts(&self) -> Result<HashMap<i64, Counts>> {
        let mut stmt = self.conn.prepare(
            "SELECT source_id, count(*),
                    count(lat) FILTER (WHERE state = 'ok'),
                    count(*) FILTER (WHERE state = 'missing')
             FROM photo GROUP BY source_id",
        )?;
        stmt.query_map([], |r| {
            Ok((
                r.get(0)?,
                Counts {
                    photos: r.get(1)?,
                    with_position: r.get(2)?,
                    missing: r.get(3)?,
                },
            ))
        })?
        .collect()
    }

    /// Per original file type: photos, photos with a position, positions from sidecars.
    /// For the `--import` dev command's check against the library survey (design.md §2.8).
    pub fn gps_by_type(&self) -> Result<Vec<(String, i64, i64, i64)>> {
        // The extension after the *last* dot (`._X.CR2` is `cr2`), grouped in Rust: SQLite has
        // no "last index of".
        let mut stmt = self
            .conn
            .prepare("SELECT original_name, lat IS NOT NULL, gps_source = 'xmp' FROM photo")?;
        let mut by: HashMap<String, (i64, i64, i64)> = HashMap::new();
        for row in stmt.query_map([], |r| {
            Ok((
                r.get::<_, String>(0)?,
                r.get::<_, bool>(1)?,
                r.get::<_, Option<bool>>(2)?,
            ))
        })? {
            let (name, gps, xmp) = row?;
            let ext = name
                .rsplit_once('.')
                .map_or(String::new(), |(_, e)| e.to_lowercase());
            let e = by.entry(ext).or_default();
            e.0 += 1;
            e.1 += gps as i64;
            e.2 += xmp.unwrap_or(false) as i64;
        }
        let mut out: Vec<_> = by.into_iter().map(|(k, (a, b, c))| (k, a, b, c)).collect();
        out.sort_by_key(|r| std::cmp::Reverse(r.1));
        Ok(out)
    }

    /// Positions of every photo that has one and isn't missing.
    pub fn positions(&self) -> Result<Vec<PhotoPoint>> {
        let mut stmt = self.conn.prepare(
            "SELECT id, lat, lon, thumb_state,
                        coalesce(CAST(replace(replace(replace(substr(taken_at, 1, 19),
                            '-', ''), 'T', ''), ':', '') AS INTEGER), 0)
                 FROM photo WHERE lat IS NOT NULL AND state = 'ok'",
        )?;
        stmt.query_map([], |r| {
            Ok(PhotoPoint {
                id: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
                thumb: match r.get_ref(3)?.as_str()? {
                    THUMB_OK => Thumb::Ok,
                    THUMB_FAILED => Thumb::Failed,
                    _ => Thumb::Pending,
                },
                taken: r.get(4)?,
            })
        })?
        .collect()
    }
}

/// Inserts or updates a photo of `source_id`; `id` is set when the import matched a moved file
/// to its old record, so the record (and later its thumbnail) is kept.
pub fn write_photo(
    tx: &Transaction<'_>,
    source_id: i64,
    id: Option<i64>,
    p: &PhotoRecord,
) -> Result<()> {
    let values = params![
        source_id,
        p.rel_dir,
        key(&p.rel_dir),
        p.stem,
        key(&p.stem),
        p.original_name,
        p.preview_name,
        p.sidecar_name,
        p.files_sig,
        p.file_size,
        p.file_mtime,
        p.quick_hash,
        p.lat,
        p.lon,
        p.alt,
        p.gps_source,
        p.taken_at,
        p.taken_at_offset,
        p.camera_make,
        p.camera_model,
        p.lens,
        p.width,
        p.height,
        p.orientation,
        p.read_error,
    ];
    const COLS: &str = "source_id, rel_dir, rel_dir_key, stem, stem_key, original_name,
        preview_name, sidecar_name, files_sig, file_size, file_mtime, quick_hash, lat, lon, alt,
        gps_source, taken_at, taken_at_offset, camera_make, camera_model, lens, width, height,
        orientation, read_error";
    // A thumbnail stays valid while what it's made from stays the same: the image (its quick
    // hash), the paired JPEG and the orientation. A moved photo keeps it; a changed one, or one
    // whose sidecar changed only, is judged by these alone. In an UPDATE, a bare column is the
    // old value; `?N` are the new ones.
    const SAME_THUMB_INPUTS: &str =
        "quick_hash IS ?12 AND preview_name IS ?7 AND orientation IS ?24";
    match id {
        Some(id) => {
            // A moved photo: rewrite its row in place, which keeps its id.
            tx.execute(
                &format!(
                    "UPDATE photo SET ({COLS}, state) = (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                     'ok'), thumb_state = CASE WHEN {SAME_THUMB_INPUTS} THEN thumb_state
                     ELSE 'none' END WHERE id = ?26"
                ),
                rusqlite::params_from_iter(
                    values.iter().copied().chain([&id as &dyn rusqlite::ToSql]),
                ),
            )?;
        }
        None => {
            tx.execute(
                &format!(
                    "INSERT INTO photo ({COLS}) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25)
                     ON CONFLICT (source_id, rel_dir_key, stem_key) DO UPDATE SET
                     rel_dir = excluded.rel_dir, stem = excluded.stem,
                     original_name = excluded.original_name, preview_name = excluded.preview_name,
                     sidecar_name = excluded.sidecar_name, files_sig = excluded.files_sig,
                     file_size = excluded.file_size, file_mtime = excluded.file_mtime,
                     quick_hash = excluded.quick_hash, lat = excluded.lat, lon = excluded.lon,
                     alt = excluded.alt, gps_source = excluded.gps_source,
                     taken_at = excluded.taken_at, taken_at_offset = excluded.taken_at_offset,
                     camera_make = excluded.camera_make, camera_model = excluded.camera_model,
                     lens = excluded.lens, width = excluded.width, height = excluded.height,
                     orientation = excluded.orientation, read_error = excluded.read_error,
                     state = 'ok',
                     thumb_state = CASE WHEN {SAME_THUMB_INPUTS} THEN photo.thumb_state
                                        ELSE 'none' END"
                ),
                values,
            )?;
        }
    }
    Ok(())
}

fn write_scope(tx: &Transaction<'_>, source_id: i64, scope: &Scope) -> Result<()> {
    tx.execute(
        "DELETE FROM source_folder WHERE source_id = ?1",
        [source_id],
    )?;
    let mut ins =
        tx.prepare("INSERT INTO source_folder (source_id, rel_dir, included) VALUES (?1, ?2, ?3)")?;
    for (dir, included) in scope.rules() {
        ins.execute(params![source_id, dir, included])?;
    }
    Ok(())
}

/// Deletes photo records (their thumbnail files go with `thumbs::sweep`). Never touches files.
pub fn delete_photos(tx: &Transaction<'_>, ids: &[i64]) -> Result<()> {
    let mut stmt = tx.prepare("DELETE FROM photo WHERE id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

/// A photo whose thumbnail is still to be made.
#[derive(Debug, Clone, PartialEq)]
pub struct ThumbTask {
    pub id: i64,
    pub root: PathBuf,
    pub unc: Option<String>,
    pub rel_dir: String,
    pub original: String,
    pub preview: Option<String>,
    pub orientation: Option<u16>,
}

/// Thumbnail states: still to make, made, or no usable source (a placeholder).
pub const THUMB_NONE: &str = "none";
pub const THUMB_OK: &str = "ok";
pub const THUMB_FAILED: &str = "failed";

impl Library {
    /// Photos on the map (a position, not missing) without a thumbnail yet, oldest first.
    /// Photos without a position get none in v1 (design.md §2.1(d)).
    pub fn thumb_tasks(&self) -> Result<Vec<ThumbTask>> {
        let mut stmt = self.conn.prepare(
            "SELECT p.id, s.root_path, s.unc_path, p.rel_dir, p.original_name, p.preview_name,
                    p.orientation
             FROM photo p JOIN source s ON s.id = p.source_id
             WHERE p.lat IS NOT NULL AND p.state = 'ok' AND p.thumb_state = 'none'
             ORDER BY p.id",
        )?;
        stmt.query_map([], |r| {
            Ok(ThumbTask {
                id: r.get(0)?,
                root: PathBuf::from(r.get::<_, String>(1)?),
                unc: r.get(2)?,
                rel_dir: r.get(3)?,
                original: r.get(4)?,
                preview: r.get(5)?,
                orientation: r.get(6)?,
            })
        })?
        .collect()
    }

    /// (made, still to make) among the photos on the map.
    pub fn thumb_counts(&self) -> Result<(i64, i64)> {
        self.conn.query_row(
            "SELECT count(*) FILTER (WHERE thumb_state <> 'none'),
                    count(*) FILTER (WHERE thumb_state = 'none')
             FROM photo WHERE lat IS NOT NULL AND state = 'ok'",
            [],
            |r| Ok((r.get(0)?, r.get(1)?)),
        )
    }

    /// The ids of photos whose thumbnail is made, for the map's markers.
    pub fn thumbed(&self) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id FROM photo WHERE thumb_state = 'ok'")?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }

    /// The original's file name per photo, for the markers' accessible names.
    pub fn names(&self, ids: &[i64]) -> Result<Vec<(i64, String)>> {
        let mut stmt = self
            .conn
            .prepare_cached("SELECT original_name FROM photo WHERE id = ?1")?;
        let mut out = Vec::with_capacity(ids.len());
        for &id in ids {
            if let Some(name) = stmt.query_row([id], |r| r.get::<_, String>(0)).optional()? {
                out.push((id, name));
            }
        }
        Ok(out)
    }

    /// One photo with its source, for the hover card and the detail panel.
    pub fn photo_detail(&self, id: i64) -> Result<Option<PhotoDetail>> {
        self.conn
            .query_row(
                "SELECT p.id, s.label, s.root_path, s.unc_path, p.rel_dir, p.original_name,
                        p.preview_name, p.file_size, p.width, p.height, p.taken_at,
                        p.taken_at_offset, p.camera_make, p.camera_model, p.lens, p.lat, p.lon,
                        p.alt, p.gps_source, p.orientation, p.thumb_state
                 FROM photo p JOIN source s ON s.id = p.source_id WHERE p.id = ?1",
                [id],
                |r| {
                    Ok(PhotoDetail {
                        id: r.get(0)?,
                        source_label: r.get(1)?,
                        root: PathBuf::from(r.get::<_, String>(2)?),
                        unc: r.get(3)?,
                        rel_dir: r.get(4)?,
                        original_name: r.get(5)?,
                        preview_name: r.get(6)?,
                        file_size: r.get(7)?,
                        width: r.get(8)?,
                        height: r.get(9)?,
                        taken_at: r.get(10)?,
                        taken_at_offset: r.get(11)?,
                        camera_make: r.get(12)?,
                        camera_model: r.get(13)?,
                        lens: r.get(14)?,
                        lat: r.get(15)?,
                        lon: r.get(16)?,
                        alt: r.get(17)?,
                        gps_source: r.get(18)?,
                        orientation: r.get(19)?,
                        thumb_made: r.get::<_, String>(20)? == THUMB_OK,
                    })
                },
            )
            .optional()
    }

    /// Folders whose path contains `text` (lower case), with how many photos on the map they
    /// hold and where; the fullest first.
    pub fn search_folders(&self, text: &str, limit: usize) -> Result<Vec<FoundSet>> {
        self.found_sets(
            "SELECT s.id || char(31) || p.rel_dir, p.rel_dir, count(*),
                    min(p.lat), min(p.lon), max(p.lat), max(p.lon)
             FROM photo p JOIN source s ON s.id = p.source_id
             WHERE p.lat IS NOT NULL AND p.state = 'ok' AND p.rel_dir <> ''
               AND p.rel_dir_key LIKE ?1 ESCAPE '\\'
             GROUP BY p.source_id, p.rel_dir_key ORDER BY count(*) DESC LIMIT ?2",
            &like(text),
            limit,
        )
    }

    /// Photos on the map taken in a period: `2019`, `2019-08` or `2019-08-14`.
    pub fn search_period(&self, period: &str) -> Result<Vec<FoundSet>> {
        let sets = self.found_sets(
            "SELECT ?1, ?1, count(*), min(lat), min(lon), max(lat), max(lon)
             FROM photo WHERE lat IS NOT NULL AND state = 'ok' AND taken_at LIKE ?1 || '%'",
            period,
            1,
        )?;
        Ok(sets.into_iter().filter(|s| s.photos > 0).collect())
    }

    /// Cameras whose make or model contains `text`, with their photos on the map.
    pub fn search_cameras(&self, text: &str, limit: usize) -> Result<Vec<FoundSet>> {
        self.found_sets(
            "SELECT coalesce(camera_make, '') || char(31) || coalesce(camera_model, ''),
                    trim(coalesce(camera_make, '') || ' ' || coalesce(camera_model, '')),
                    count(*), min(lat), min(lon), max(lat), max(lon)
             FROM photo
             WHERE lat IS NOT NULL AND state = 'ok'
               AND lower(coalesce(camera_make, '') || ' ' || coalesce(camera_model, ''))
                   LIKE ?1 ESCAPE '\\'
             GROUP BY camera_make, camera_model ORDER BY count(*) DESC LIMIT ?2",
            &like(text),
            limit,
        )
    }

    fn found_sets(&self, sql: &str, arg: &str, limit: usize) -> Result<Vec<FoundSet>> {
        let mut stmt = self.conn.prepare_cached(sql)?;
        let limit = limit as i64;
        let bind: &[&dyn rusqlite::ToSql] = if sql.contains("?2") {
            &[&arg, &limit]
        } else {
            &[&arg]
        };
        stmt.query_map(bind, |r| {
            Ok(FoundSet {
                key: r.get(0)?,
                label: r.get(1)?,
                photos: r.get::<_, i64>(2)? as usize,
                bounds: match (r.get(3)?, r.get(4)?, r.get(5)?, r.get(6)?) {
                    (Some(min_lat), Some(min_lon), Some(max_lat), Some(max_lon)) => {
                        Some(crate::import::Bounds {
                            min_lat,
                            min_lon,
                            max_lat,
                            max_lon,
                        })
                    }
                    _ => None,
                },
            })
        })?
        .collect()
    }

    /// Photos on the map whose file name contains `text`, newest first.
    pub fn search_files(&self, text: &str, limit: usize) -> Result<Vec<FoundPhoto>> {
        let mut stmt = self.conn.prepare_cached(
            "SELECT id, original_name, taken_at, lat, lon FROM photo
             WHERE lat IS NOT NULL AND state = 'ok' AND stem_key LIKE ?1 ESCAPE '\\'
             ORDER BY taken_at DESC, id LIMIT ?2",
        )?;
        stmt.query_map(params![like(text), limit as i64], |r| {
            Ok(FoundPhoto {
                id: r.get(0)?,
                name: r.get(1)?,
                taken_at: r.get(2)?,
                lat: r.get(3)?,
                lon: r.get(4)?,
            })
        })?
        .collect()
    }

    /// How many photos on the map lie within `b`.
    pub fn count_in(&self, b: crate::import::Bounds) -> Result<usize> {
        self.conn
            .prepare_cached(
                "SELECT count(*) FROM photo WHERE lat BETWEEN ?1 AND ?2 AND lon BETWEEN ?3 AND ?4
                 AND state = 'ok'",
            )?
            .query_row(params![b.min_lat, b.max_lat, b.min_lon, b.max_lon], |r| {
                r.get::<_, i64>(0)
            })
            .map(|n| n as usize)
    }

    /// Every photo id, for removing the thumbnail files of deleted records.
    pub fn all_ids(&self) -> Result<std::collections::HashSet<i64>> {
        let mut stmt = self.conn.prepare("SELECT id FROM photo")?;
        stmt.query_map([], |r| r.get(0))?.collect()
    }
}

pub fn set_thumb_state(tx: &Transaction<'_>, id: i64, state: &str) -> Result<()> {
    tx.execute(
        "UPDATE photo SET thumb_state = ?2 WHERE id = ?1",
        params![id, state],
    )?;
    Ok(())
}

/// Marks photos missing: their folder was listed in this scan and their files weren't there.
pub fn mark_missing(tx: &Transaction<'_>, ids: &[i64]) -> Result<()> {
    let mut stmt = tx.prepare("UPDATE photo SET state = 'missing' WHERE id = ?1")?;
    for id in ids {
        stmt.execute([id])?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn record(dir: &str, stem: &str) -> PhotoRecord {
        PhotoRecord {
            rel_dir: dir.into(),
            stem: stem.into(),
            original_name: format!("{stem}.jpg"),
            files_sig: 1,
            file_size: 100,
            lat: Some(10.0),
            lon: Some(20.0),
            ..PhotoRecord::default()
        }
    }

    #[test]
    fn migrations_run_once_and_sources_round_trip() {
        let mut lib = Library::open_in_memory().unwrap();
        lib.migrate().unwrap(); // a second run is a no-op
        let mut scope = Scope::all();
        scope.set("Screenshots", false);
        let id = lib
            .add_source(
                Path::new(r"Z:\Photos"),
                Some(r"\\nas\photos"),
                "Photos",
                &scope,
            )
            .unwrap();
        let s = lib.source(id).unwrap().unwrap();
        assert_eq!(s.unc_path.as_deref(), Some(r"\\nas\photos"));
        assert_eq!(s.scope, scope);
        assert_eq!(lib.sources().unwrap().len(), 1);
    }

    #[test]
    fn narrowing_the_scope_removes_the_records_it_excludes() {
        let mut lib = Library::open_in_memory().unwrap();
        let src = lib
            .add_source(Path::new(r"C:\P"), None, "P", &Scope::all())
            .unwrap();
        let tx = lib.transaction().unwrap();
        write_photo(&tx, src, None, &record("keep", "a")).unwrap();
        write_photo(&tx, src, None, &record(r"drop\deep", "b")).unwrap();
        tx.commit().unwrap();
        let mut narrow = Scope::all();
        narrow.set("Drop", false);
        assert_eq!(lib.count_out_of_scope(src, &narrow).unwrap(), 1);
        assert_eq!(lib.set_scope(src, &narrow).unwrap(), 1);
        assert_eq!(lib.counts().unwrap().photos, 1);
        assert_eq!(lib.source(src).unwrap().unwrap().scope, narrow);
    }

    #[test]
    fn upsert_is_case_insensitive_and_remove_cascades() {
        let mut lib = Library::open_in_memory().unwrap();
        let src = lib
            .add_source(Path::new(r"C:\P"), None, "P", &Scope::all())
            .unwrap();
        let tx = lib.transaction().unwrap();
        write_photo(&tx, src, None, &record("Trip", "IMG_1")).unwrap();
        write_photo(&tx, src, None, &record("trip", "img_1")).unwrap(); // same photo
        write_photo(&tx, src, None, &record("Trip", "Ärger")).unwrap();
        write_photo(&tx, src, None, &record("Trip", "ärger")).unwrap(); // non-ASCII case
        tx.commit().unwrap();
        assert_eq!(lib.counts().unwrap().photos, 2);
        assert_eq!(lib.positions().unwrap().len(), 2);
        lib.remove_source(src).unwrap();
        assert_eq!(lib.counts().unwrap().photos, 0);
    }

    #[test]
    fn missing_photos_leave_the_map_and_moved_ones_keep_their_id() {
        let mut lib = Library::open_in_memory().unwrap();
        let src = lib
            .add_source(Path::new(r"C:\P"), None, "P", &Scope::all())
            .unwrap();
        let tx = lib.transaction().unwrap();
        write_photo(&tx, src, None, &record("a", "x")).unwrap();
        tx.commit().unwrap();
        let old = lib.existing(src).unwrap()[&("a".into(), "x".into())].id;

        let tx = lib.transaction().unwrap();
        mark_missing(&tx, &[old]).unwrap();
        tx.commit().unwrap();
        assert_eq!(lib.counts().unwrap().missing, 1);
        assert!(lib.positions().unwrap().is_empty());

        let tx = lib.transaction().unwrap();
        write_photo(&tx, src, Some(old), &record("b", "x")).unwrap(); // found again, moved
        tx.commit().unwrap();
        let now = lib.existing(src).unwrap();
        assert_eq!(now[&("b".into(), "x".into())].id, old);
        assert_eq!(
            lib.counts().unwrap(),
            Counts {
                photos: 1,
                with_position: 1,
                missing: 0
            }
        );
    }
}

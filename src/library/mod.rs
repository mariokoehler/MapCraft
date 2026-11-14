//! The library: a SQLite database in the data folder holding sources and photos (design.md §3.4).
//!
//! `rusqlite` is a thin JDBC-like wrapper: a `Connection`, prepared statements with `?` params,
//! rows mapped by closures. SQLite runs in WAL mode, so the import thread can write while the UI
//! thread reads through its own connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rusqlite::{Connection, OptionalExtension, Transaction, params};

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
    /// The picked folders (relative to the root); empty means the whole root.
    pub folders: Vec<String>,
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

/// A photo's position, for drawing it on the map.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PhotoPoint {
    pub id: i64,
    pub lat: f64,
    pub lon: f64,
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
        folders: &[String],
    ) -> Result<i64> {
        let tx = self.conn.transaction()?;
        tx.execute(
            "INSERT INTO source (root_path, unc_path, label, added_at)
             VALUES (?1, ?2, ?3, strftime('%Y-%m-%dT%H:%M:%SZ', 'now'))",
            params![root.to_string_lossy(), unc_path, label],
        )?;
        let id = tx.last_insert_rowid();
        for f in folders {
            tx.execute(
                "INSERT OR IGNORE INTO source_folder (source_id, rel_dir) VALUES (?1, ?2)",
                params![id, f],
            )?;
        }
        tx.commit()?;
        Ok(id)
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
                        folders: Vec::new(),
                    })
                },
            )
            .optional()?;
        let Some(mut source) = source else {
            return Ok(None);
        };
        let mut stmt = self
            .conn
            .prepare("SELECT rel_dir FROM source_folder WHERE source_id = ?1 ORDER BY rel_dir")?;
        source.folders = stmt.query_map([id], |r| r.get(0))?.collect::<Result<_>>()?;
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
            "SELECT id, rel_dir_key, stem_key, files_sig, file_size, quick_hash, state
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

    /// Per original file type: photos, photos with a position, positions from sidecars.
    /// For the `--import` dev command's check against the library survey (design.md §2.8).
    pub fn gps_by_type(&self) -> Result<Vec<(String, i64, i64, i64)>> {
        let mut stmt = self.conn.prepare(
            "SELECT lower(substr(original_name, instr(original_name, '.') + 1)) AS ext,
                    count(*), count(lat), count(*) FILTER (WHERE gps_source = 'xmp')
             FROM photo GROUP BY ext ORDER BY count(*) DESC",
        )?;
        stmt.query_map([], |r| Ok((r.get(0)?, r.get(1)?, r.get(2)?, r.get(3)?)))?
            .collect()
    }

    /// Positions of every photo that has one and isn't missing.
    pub fn positions(&self) -> Result<Vec<PhotoPoint>> {
        let mut stmt = self
            .conn
            .prepare("SELECT id, lat, lon FROM photo WHERE lat IS NOT NULL AND state = 'ok'")?;
        stmt.query_map([], |r| {
            Ok(PhotoPoint {
                id: r.get(0)?,
                lat: r.get(1)?,
                lon: r.get(2)?,
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
    match id {
        Some(id) => {
            // A moved photo: rewrite its row in place, which keeps its id.
            tx.execute(
                &format!(
                    "UPDATE photo SET ({COLS}, state) = (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9, ?10,
                     ?11, ?12, ?13, ?14, ?15, ?16, ?17, ?18, ?19, ?20, ?21, ?22, ?23, ?24, ?25,
                     'ok') WHERE id = ?26"
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
                     state = 'ok'"
                ),
                values,
            )?;
        }
    }
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
        let id = lib
            .add_source(
                Path::new(r"Z:\Photos"),
                Some(r"\\nas\photos"),
                "Photos",
                &["2023".into()],
            )
            .unwrap();
        let s = lib.source(id).unwrap().unwrap();
        assert_eq!(s.unc_path.as_deref(), Some(r"\\nas\photos"));
        assert_eq!(s.folders, vec!["2023".to_string()]);
        assert_eq!(lib.sources().unwrap().len(), 1);
    }

    #[test]
    fn upsert_is_case_insensitive_and_remove_cascades() {
        let mut lib = Library::open_in_memory().unwrap();
        let src = lib.add_source(Path::new(r"C:\P"), None, "P", &[]).unwrap();
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
        let src = lib.add_source(Path::new(r"C:\P"), None, "P", &[]).unwrap();
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

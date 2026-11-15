//! The data folder (design.md §3.3, M8): what MapCraft keeps in it, how much space that takes,
//! and moving, freeing and removing it.
//!
//! MapCraft owns a fixed list of entries in the folder ([`Kind::entries`]) and acts on those
//! only, never on the folder as a whole: the user may have picked `D:\`. The library is copied
//! with SQLite's own snapshot (`VACUUM INTO`), never as an open file, so a copy is consistent
//! even while the app has it open.

use std::fs;
use std::io;
use std::path::{Path, PathBuf};

use rusqlite::{Connection, OpenFlags};

/// What MapCraft keeps in the data folder, in the order Settings lists it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Kind {
    Thumbnails,
    Previews,
    Tiles,
    Library,
    Places,
}

impl Kind {
    pub const ALL: [Kind; 5] = [
        Kind::Thumbnails,
        Kind::Previews,
        Kind::Tiles,
        Kind::Library,
        Kind::Places,
    ];

    /// The kinds MapCraft makes again by itself, which Free up space may delete.
    pub const REMAKEABLE: [Kind; 3] = [Kind::Previews, Kind::Tiles, Kind::Thumbnails];

    pub fn label(self) -> &'static str {
        match self {
            Kind::Thumbnails => "Thumbnails",
            Kind::Previews => "Detail previews",
            Kind::Tiles => "Map tiles",
            Kind::Library => "Library",
            Kind::Places => "Place names",
        }
    }

    /// Its files and folders in the data folder.
    pub fn entries(self) -> &'static [&'static str] {
        match self {
            Kind::Thumbnails => &["thumbs"],
            Kind::Previews => &["previews"],
            Kind::Tiles => &["tiles"],
            // `backups`: snapshots taken before migrations (library::Library::migrate).
            Kind::Library => &[
                "library.sqlite",
                "library.sqlite-wal",
                "library.sqlite-shm",
                "backups",
                // The undo journals of geotagging (geotag::journal, design.md §2.9(i)).
                "geotag",
            ],
            Kind::Places => &["geonames"],
        }
    }

    fn index(self) -> usize {
        Kind::ALL.iter().position(|k| *k == self).expect("listed")
    }
}

/// Bytes and files.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Size {
    pub bytes: u64,
    pub files: u64,
}

impl Size {
    fn add(&mut self, other: Size) {
        self.bytes += other.bytes;
        self.files += other.files;
    }
}

/// How much space each kind takes.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Usage {
    sizes: [Size; 5],
}

impl Usage {
    pub fn get(&self, kind: Kind) -> Size {
        self.sizes[kind.index()]
    }

    pub fn total(&self) -> Size {
        let mut t = Size::default();
        for s in self.sizes {
            t.add(s);
        }
        t
    }
}

/// Counts what MapCraft keeps in `data` (runs on a background thread: it reads every entry).
pub fn usage(data: &Path) -> Usage {
    let mut u = Usage::default();
    for kind in Kind::ALL {
        for e in kind.entries() {
            u.sizes[kind.index()].add(size_of(&data.join(e)));
        }
    }
    u
}

/// The size of a file, or of everything under a folder; nothing for what isn't there.
fn size_of(path: &Path) -> Size {
    let Ok(meta) = fs::symlink_metadata(path) else {
        return Size::default();
    };
    if meta.is_file() {
        return Size {
            bytes: meta.len(),
            files: 1,
        };
    }
    let mut s = Size::default();
    if meta.is_dir()
        && let Ok(dir) = fs::read_dir(path)
    {
        for e in dir.flatten() {
            s.add(size_of(&e.path()));
        }
    }
    s
}

/// "4.82 GB", "288 MB", "12 KB" (decimal units, as Explorer's size column isn't: but as disks are
/// sold and as the update dialog says).
pub fn bytes_text(b: u64) -> String {
    let b = b as f64;
    if b >= 1e9 {
        format!("{:.2} GB", b / 1e9)
    } else if b >= 1e6 {
        format!("{:.0} MB", b / 1e6)
    } else if b >= 1e3 {
        format!("{:.0} KB", b / 1e3)
    } else {
        format!("{b} bytes")
    }
}

/// Whether `dir` holds a MapCraft library.
pub fn has_library(dir: &Path) -> bool {
    dir.join("library.sqlite").is_file()
}

/// The photos in the library in `dir`, read without changing it.
pub fn library_photos(dir: &Path) -> Option<i64> {
    let conn =
        Connection::open_with_flags(dir.join("library.sqlite"), OpenFlags::SQLITE_OPEN_READ_ONLY)
            .ok()?;
    conn.query_row("SELECT count(*) FROM photo", [], |r| r.get(0))
        .ok()
}

/// Where a picked folder leads.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Target {
    /// Not usable, and why.
    Refused(String),
    /// An empty or new folder: the data can be moved there, or MapCraft can start empty there.
    /// `inside` when the picked folder held other files, so a `MapCraft` folder inside it is used.
    Empty { dir: PathBuf, inside: bool },
    /// A folder with a MapCraft library: it can only be used as it is, never overwritten.
    Existing { dir: PathBuf, photos: Option<i64> },
}

/// Decides what a folder picked as the new data folder means (canvas `MapCraft — Data folder`,
/// artboard 2).
pub fn classify(current: &Path, picked: &Path) -> Target {
    let now = "That’s the data folder MapCraft uses now, or a folder inside it.";
    if same_or_inside(picked, current) {
        return Target::Refused(now.into());
    }
    if has_library(picked) {
        return Target::Existing {
            dir: picked.to_path_buf(),
            photos: library_photos(picked),
        };
    }
    if is_empty(picked) {
        return Target::Empty {
            dir: picked.to_path_buf(),
            inside: false,
        };
    }
    let dir = picked.join("MapCraft");
    if same_or_inside(&dir, current) {
        return Target::Refused(now.into());
    }
    if has_library(&dir) {
        return Target::Existing {
            photos: library_photos(&dir),
            dir,
        };
    }
    if is_empty(&dir) {
        Target::Empty { dir, inside: true }
    } else {
        Target::Refused(format!(
            "{} already holds other files. Choose another folder.",
            dir.display()
        ))
    }
}

/// Missing, or a folder with nothing in it.
fn is_empty(dir: &Path) -> bool {
    match fs::read_dir(dir) {
        Ok(mut d) => d.next().is_none(),
        Err(e) => e.kind() == io::ErrorKind::NotFound,
    }
}

/// Paths compared as Windows does: case-insensitive, either separator, no trailing one.
fn norm(p: &Path) -> String {
    let abs = std::path::absolute(p).unwrap_or_else(|_| p.to_path_buf());
    abs.to_string_lossy()
        .replace('/', "\\")
        .trim_end_matches('\\')
        .to_lowercase()
}

/// `a` is `b` or a folder inside it.
pub fn same_or_inside(a: &Path, b: &Path) -> bool {
    let (a, b) = (norm(a), norm(b));
    a == b || a.starts_with(&format!("{b}\\"))
}

/// Whether a folder is on the network (a UNC path or a mapped drive).
pub fn is_network(path: &Path) -> bool {
    let s = path.to_string_lossy();
    if s.starts_with(r"\\") {
        return true;
    }
    #[cfg(windows)]
    {
        let Some(root) = s.get(..3).filter(|r| r.ends_with(":\\")) else {
            return false;
        };
        let wide: Vec<u16> = root.encode_utf16().chain([0]).collect();
        // SAFETY: a valid, NUL-terminated wide string.
        let t = unsafe { windows_sys::Win32::Storage::FileSystem::GetDriveTypeW(wide.as_ptr()) };
        t == windows_sys::Win32::System::WindowsProgramming::DRIVE_REMOTE
    }
    #[cfg(not(windows))]
    false
}

/// The free space where `path` would be (its nearest existing folder), if Windows says.
pub fn free_space(path: &Path) -> Option<u64> {
    let mut at = path;
    while !at.exists() {
        at = at.parent()?;
    }
    #[cfg(windows)]
    {
        let wide: Vec<u16> = at
            .as_os_str()
            .to_string_lossy()
            .encode_utf16()
            .chain([0])
            .collect();
        let mut free = 0u64;
        // SAFETY: a valid, NUL-terminated wide string and a valid out pointer; the other two
        // outputs are optional.
        let ok = unsafe {
            windows_sys::Win32::Storage::FileSystem::GetDiskFreeSpaceExW(
                wide.as_ptr(),
                &mut free,
                std::ptr::null_mut(),
                std::ptr::null_mut(),
            )
        };
        (ok != 0).then_some(free)
    }
    #[cfg(not(windows))]
    None
}

/// How a copy is getting on.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Progress {
    pub kind: Kind,
    pub done: Size,
    pub total: Size,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CopyError {
    Cancelled,
    Failed(String),
}

/// Copies MapCraft's data from `from` to the empty or new folder `to`. `progress` returns
/// `false` to cancel. On any failure, what was written is removed again and `to` is as before.
pub fn copy(from: &Path, to: &Path, progress: &dyn Fn(Progress) -> bool) -> Result<(), CopyError> {
    let total = usage(from).total();
    if let Some(free) = free_space(to)
        && free < total.bytes
    {
        return Err(CopyError::Failed(format!(
            "{} has {} free; the data needs {}. Nothing was changed.",
            to.display(),
            bytes_text(free),
            bytes_text(total.bytes)
        )));
    }
    let created = !to.exists();
    let r = copy_all(from, to, total, progress);
    if r.is_err() {
        let _ = remove_owned(to, created);
    }
    r
}

fn copy_all(
    from: &Path,
    to: &Path,
    total: Size,
    progress: &dyn Fn(Progress) -> bool,
) -> Result<(), CopyError> {
    let failed = |what: &str, e: &dyn std::fmt::Display| {
        CopyError::Failed(format!("Can’t copy {what}: {e}. Nothing was changed."))
    };
    fs::create_dir_all(to).map_err(|e| failed("to the new folder", &e))?;
    let mut done = Size::default();
    for kind in Kind::ALL {
        if !progress(Progress { kind, done, total }) {
            return Err(CopyError::Cancelled);
        }
        match kind {
            Kind::Library => {
                let src = from.join("library.sqlite");
                if src.is_file() {
                    snapshot(&src, &to.join("library.sqlite"))
                        .map_err(|e| failed("the library", &e))?;
                    // The library's own files (the snapshot holds the WAL's content too).
                    for e in ["library.sqlite", "library.sqlite-wal", "library.sqlite-shm"] {
                        done.add(size_of(&from.join(e)));
                    }
                }
                // The backups are databases too: each goes through a snapshot. The geotag
                // journals are plain files.
                for dir in ["backups", "geotag"] {
                    copy_tree(
                        &from.join(dir),
                        &to.join(dir),
                        kind,
                        &mut done,
                        total,
                        progress,
                    )?;
                }
            }
            _ => {
                for e in kind.entries() {
                    copy_tree(&from.join(e), &to.join(e), kind, &mut done, total, progress)?;
                }
            }
        }
    }
    progress(Progress {
        kind: Kind::Places,
        done: total,
        total,
    });
    Ok(())
}

/// A consistent copy of a SQLite database, taken through SQLite while others may have it open.
fn snapshot(src: &Path, dest: &Path) -> rusqlite::Result<()> {
    let conn = Connection::open_with_flags(src, OpenFlags::SQLITE_OPEN_READ_ONLY)?;
    conn.execute("VACUUM INTO ?1", [dest.to_string_lossy()])?;
    Ok(())
}

/// Copies a file or a folder; databases in it (the place index) go through [`snapshot`], and
/// their `-wal`/`-shm` files are left behind (the snapshot holds their content).
fn copy_tree(
    src: &Path,
    dest: &Path,
    kind: Kind,
    done: &mut Size,
    total: Size,
    progress: &dyn Fn(Progress) -> bool,
) -> Result<(), CopyError> {
    let Ok(meta) = fs::symlink_metadata(src) else {
        return Ok(()); // not there: nothing to copy
    };
    let failed = |e: &dyn std::fmt::Display| {
        CopyError::Failed(format!(
            "Can’t copy {}: {e}. Nothing was changed.",
            src.display()
        ))
    };
    if meta.is_dir() {
        fs::create_dir_all(dest).map_err(|e| failed(&e))?;
        for e in fs::read_dir(src).map_err(|e| failed(&e))?.flatten() {
            copy_tree(
                &e.path(),
                &dest.join(e.file_name()),
                kind,
                done,
                total,
                progress,
            )?;
        }
        return Ok(());
    }
    if !meta.is_file() {
        return Ok(());
    }
    let name = src.file_name().unwrap_or_default().to_string_lossy();
    if name.ends_with(".sqlite-wal") || name.ends_with(".sqlite-shm") {
        done.add(Size {
            bytes: meta.len(),
            files: 1,
        });
        return Ok(());
    }
    if name.ends_with(".sqlite") {
        snapshot(src, dest).map_err(|e| failed(&e))?;
    } else {
        fs::copy(src, dest).map_err(|e| failed(&e))?;
    }
    done.add(Size {
        bytes: meta.len(),
        files: 1,
    });
    if !progress(Progress {
        kind,
        done: *done,
        total,
    }) {
        return Err(CopyError::Cancelled);
    }
    Ok(())
}

/// Removes MapCraft's entries from `dir`, then `dir` itself if nothing else is left in it and
/// `remove_dir` asks for that. Anything else in the folder stays.
pub fn remove_owned(dir: &Path, remove_dir: bool) -> io::Result<()> {
    let mut first_err = None;
    for kind in Kind::ALL {
        for e in kind.entries() {
            let p = dir.join(e);
            let r = match fs::symlink_metadata(&p) {
                Ok(m) if m.is_dir() => fs::remove_dir_all(&p),
                Ok(_) => fs::remove_file(&p),
                Err(_) => Ok(()),
            };
            if let Err(e) = r {
                first_err.get_or_insert(e);
            }
        }
    }
    if remove_dir && is_empty(dir) {
        let _ = fs::remove_dir(dir);
    }
    first_err.map_or(Ok(()), Err)
}

/// Deletes what MapCraft makes again by itself ([`Kind::REMAKEABLE`] only) and returns what
/// was freed. Thumbnails must be marked as missing in the library first, or the map would
/// look for files that are gone.
pub fn purge(data: &Path, kinds: &[Kind]) -> Size {
    let mut freed = Size::default();
    for &kind in kinds {
        if !Kind::REMAKEABLE.contains(&kind) {
            continue;
        }
        for e in kind.entries() {
            let p = data.join(e);
            let s = size_of(&p);
            if fs::remove_dir_all(&p).is_ok() {
                freed.add(s);
            } else {
                // Partly deleted (a file in use): count what's gone.
                let left = size_of(&p);
                freed.add(Size {
                    bytes: s.bytes.saturating_sub(left.bytes),
                    files: s.files.saturating_sub(left.files),
                });
            }
        }
    }
    freed
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::scope::Scope;
    use crate::library::{Library, PhotoRecord, write_photo};

    /// A data folder with a library of `n` photos and some thumbnails, previews and tiles.
    fn data_folder(dir: &Path, n: i64) {
        let mut lib = Library::open(&dir.join("library.sqlite")).unwrap();
        let src = lib
            .add_source(Path::new(r"C:\P"), None, "P", &Scope::all())
            .unwrap();
        let tx = lib.transaction().unwrap();
        for i in 0..n {
            let r = PhotoRecord {
                rel_dir: String::new(),
                stem: format!("IMG_{i}"),
                original_name: format!("IMG_{i}.jpg"),
                files_sig: i,
                file_size: 1,
                lat: Some(1.0),
                lon: Some(2.0),
                ..PhotoRecord::default()
            };
            write_photo(&tx, src, None, &r).unwrap();
        }
        tx.commit().unwrap();
        // A backup from a migration, as Library::migrate keeps it.
        fs::create_dir_all(dir.join("backups")).unwrap();
        Connection::open(dir.join("library.sqlite"))
            .unwrap()
            .execute(
                "VACUUM INTO ?1",
                [dir.join("backups/library-schema-1.sqlite")
                    .to_string_lossy()],
            )
            .unwrap();
        // A place index like the real one: a SQLite file in `geonames`, in WAL mode.
        fs::create_dir_all(dir.join("geonames")).unwrap();
        let places = Connection::open(dir.join("geonames/places.sqlite")).unwrap();
        places
            .execute_batch(
                "PRAGMA journal_mode = WAL; CREATE TABLE place (name TEXT);
                 INSERT INTO place VALUES ('Reykjavík'), ('Hamburg');",
            )
            .unwrap();
        drop(places);
        for (d, f, len) in [
            ("thumbs/aa/bb", "1_256.jpg", 100),
            ("thumbs/aa/cc", "2_256.jpg", 50),
            ("previews/aa/bb", "1_1024.jpg", 400),
            ("tiles/content-v2", "x", 30),
        ] {
            fs::create_dir_all(dir.join(d)).unwrap();
            fs::write(dir.join(d).join(f), vec![7u8; len]).unwrap();
        }
    }

    #[test]
    fn usage_counts_each_kind_and_nothing_else() {
        let t = tempfile::tempdir().unwrap();
        data_folder(t.path(), 3);
        fs::write(t.path().join("someone-elses.txt"), b"keep me").unwrap();
        let u = usage(t.path());
        assert_eq!(
            u.get(Kind::Thumbnails),
            Size {
                bytes: 150,
                files: 2
            }
        );
        assert_eq!(
            u.get(Kind::Previews),
            Size {
                bytes: 400,
                files: 1
            }
        );
        assert_eq!(u.get(Kind::Tiles).files, 1);
        assert!(u.get(Kind::Library).bytes > 0);
        assert!(u.get(Kind::Places).files >= 1);
        assert_eq!(
            u.total().files,
            4 + u.get(Kind::Library).files + u.get(Kind::Places).files,
            "the foreign file isn't counted"
        );
        assert_eq!(bytes_text(4_820_000_000), "4.82 GB");
        assert_eq!(bytes_text(288_000_000), "288 MB");
    }

    #[test]
    fn a_picked_folder_is_classified() {
        let t = tempfile::tempdir().unwrap();
        let current = t.path().join("current");
        data_folder(&current, 2);
        // The current folder, or one inside it.
        assert!(matches!(classify(&current, &current), Target::Refused(_)));
        assert!(matches!(
            classify(&current, &current.join("thumbs")),
            Target::Refused(_)
        ));
        let upper = PathBuf::from(current.to_string_lossy().to_uppercase());
        assert!(
            matches!(classify(&current, &upper), Target::Refused(_)),
            "case"
        );
        // New or empty: used as it is.
        let new = t.path().join("new");
        assert_eq!(
            classify(&current, &new),
            Target::Empty {
                dir: new.clone(),
                inside: false
            }
        );
        // Other files: a MapCraft folder inside.
        let busy = t.path().join("busy");
        fs::create_dir_all(&busy).unwrap();
        fs::write(busy.join("notes.txt"), b"x").unwrap();
        assert_eq!(
            classify(&current, &busy),
            Target::Empty {
                dir: busy.join("MapCraft"),
                inside: true
            }
        );
        // The parent of the current folder: its MapCraft subfolder isn't the current one here,
        // but a parent whose MapCraft folder *is* the current one is refused.
        let parent = t.path().join("parent");
        let inner = parent.join("MapCraft");
        data_folder(&inner, 1);
        fs::write(parent.join("other.txt"), b"x").unwrap();
        assert!(matches!(classify(&inner, &parent), Target::Refused(_)));
        // A library: only "use it".
        assert_eq!(
            classify(&current, &inner),
            Target::Existing {
                dir: inner.clone(),
                photos: Some(1)
            }
        );
        // A MapCraft folder inside that holds something else: refused.
        let odd = t.path().join("odd");
        fs::create_dir_all(odd.join("MapCraft")).unwrap();
        fs::write(odd.join("x.txt"), b"x").unwrap();
        fs::write(odd.join("MapCraft").join("y.txt"), b"y").unwrap();
        assert!(matches!(classify(&current, &odd), Target::Refused(_)));
    }

    #[test]
    fn a_copy_opens_as_the_same_library() {
        let t = tempfile::tempdir().unwrap();
        let (from, to) = (t.path().join("from"), t.path().join("to"));
        data_folder(&from, 5);
        let before = Library::open(&from.join("library.sqlite")).unwrap();
        let ids: Vec<i64> = before.positions().unwrap().iter().map(|p| p.id).collect();
        // The search thread keeps the place index open (with a write pending in its WAL).
        let held = Connection::open(from.join("geonames/places.sqlite")).unwrap();
        held.execute("INSERT INTO place VALUES ('Lisboa')", [])
            .unwrap();
        let seen = std::sync::Mutex::new(Vec::new());
        copy(&from, &to, &|p| {
            seen.lock().unwrap().push(p.done.files);
            true
        })
        .unwrap();
        // Library::open runs the migrations: a lost user_version would make them run again
        // over existing tables and fail.
        let after = Library::open(&to.join("library.sqlite")).unwrap();
        let ids_after: Vec<i64> = after.positions().unwrap().iter().map(|p| p.id).collect();
        assert_eq!(ids, ids_after, "photo ids name the thumbnails");
        assert_eq!(
            fs::read(to.join("thumbs/aa/cc/2_256.jpg")).unwrap(),
            vec![7u8; 50]
        );
        assert!(to.join("tiles/content-v2/x").is_file());
        let places = Connection::open(to.join("geonames/places.sqlite")).unwrap();
        let n: i64 = places
            .query_row("SELECT count(*) FROM place", [], |r| r.get(0))
            .unwrap();
        assert_eq!(n, 3, "the copy holds the open index's latest write");
        assert!(
            to.join("backups/library-schema-1.sqlite").is_file(),
            "backups go along"
        );
        drop(held);
        assert!(seen.lock().unwrap().windows(2).all(|w| w[0] <= w[1]));
        // The source stays as it was (a copy, not a move).
        assert!(from.join("thumbs/aa/bb/1_256.jpg").is_file());
    }

    #[test]
    fn a_cancelled_copy_leaves_nothing_behind() {
        let t = tempfile::tempdir().unwrap();
        let (from, to) = (t.path().join("from"), t.path().join("to"));
        data_folder(&from, 2);
        let calls = std::sync::atomic::AtomicUsize::new(0);
        let r = copy(&from, &to, &|_| {
            calls.fetch_add(1, std::sync::atomic::Ordering::Relaxed) < 3
        });
        assert_eq!(r, Err(CopyError::Cancelled));
        assert!(!to.exists(), "the new folder it made is gone too");
        // Into a folder that existed (empty): the folder stays, empty.
        fs::create_dir_all(&to).unwrap();
        let r = copy(&from, &to, &|p| p.kind != Kind::Tiles);
        assert_eq!(r, Err(CopyError::Cancelled));
        assert!(to.is_dir() && is_empty(&to));
    }

    #[test]
    fn removing_and_purging_touch_only_mapcraft_files() {
        let t = tempfile::tempdir().unwrap();
        let d = t.path().join("d");
        data_folder(&d, 2);
        fs::write(d.join("someone-elses.txt"), b"keep me").unwrap();
        let freed = purge(&d, &[Kind::Previews, Kind::Tiles, Kind::Library]);
        assert_eq!(freed.files, 2, "previews and tiles; never the library");
        assert!(!d.join("previews").exists() && !d.join("tiles").exists());
        assert!(d.join("thumbs").exists() && has_library(&d));
        remove_owned(&d, true).unwrap();
        assert!(!has_library(&d) && !d.join("thumbs").exists());
        assert!(!d.join("backups").exists(), "the backups are the library's");
        assert!(d.join("someone-elses.txt").is_file(), "not ours: kept");
        assert!(d.is_dir(), "not empty, so the folder stays");
    }
}

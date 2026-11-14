//! Importing a source: list it, group files into photos, compare with the library, read the
//! new and changed ones in parallel, and write the results (design.md §2.1, §3.7).
//!
//! `run` is headless: the GUI calls it on a worker thread, tests and the `--import` dev command
//! call it directly. It never writes to the source (design.md §1.1).
//!
//! Integrity rules:
//! - A photo becomes "missing" only if its folder was listed completely in this run. A folder
//!   that can't be read, a cancel, or an unreachable source changes nothing.
//! - Unchanged photos (same names, sizes and times) aren't read again, so a cancelled import
//!   resumes where it stopped.
//! - A file that can't be read is recorded with its error and counted; it never stops the run.

pub mod browse;
pub mod scan;
pub mod scope;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::{Read, Seek, SeekFrom};
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use xxhash_rust::xxh3::Xxh3;

use crate::library::{self, Library, PhotoRecord};
use crate::metadata::{self, Metadata, ReadError, xmp};
use scan::Group;

/// Photos written per database transaction.
const BATCH: usize = 200;

#[derive(Debug, Clone, PartialEq)]
pub enum Progress {
    /// Listing folders; `files` found so far.
    Listing { files: usize },
    /// Reading metadata: `done` of `total` new or changed photos.
    Reading { done: usize, total: usize },
}

/// What an import did, for the summary ("812 imported, 37 without a location").
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Summary {
    pub photos: usize,
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub moved: usize,
    pub restored: usize,
    pub missing: usize,
    /// Photos whose metadata is unreadable (recorded with the error, not fatal).
    pub failed: usize,
    /// Photos skipped because a file couldn't be read at all (NAS hiccup, locked file). Their
    /// records are left as they were and they are tried again on the next rescan.
    pub skipped: usize,
    /// Of the photos written in this run, those without a position.
    pub without_position: usize,
    pub unreadable_folders: usize,
    pub cancelled: bool,
    /// Records deleted because their files are now ignored (`._` files).
    pub removed: usize,
    /// The area covered by the positions written in this run ("Zoom to new photos").
    pub bounds: Option<Bounds>,
}

impl Summary {
    /// The summary without its coordinates, for logs and console output (design.md §3.8: no
    /// coordinates in logs).
    pub fn counts_only(&self) -> String {
        let Self {
            photos,
            new,
            changed,
            unchanged,
            moved,
            restored,
            missing,
            failed,
            skipped,
            without_position,
            unreadable_folders,
            cancelled,
            removed,
            bounds: _,
        } = self;
        format!(
            "{photos} photos: {new} new, {changed} changed, {unchanged} unchanged, {moved} moved, \
             {restored} back, {missing} missing, {failed} failed, {skipped} skipped, \
             {without_position} without position, {unreadable_folders} unreadable folders,              {removed} removed{}",
            if *cancelled { ", cancelled" } else { "" }
        )
    }
}

/// A latitude/longitude box.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Bounds {
    pub min_lat: f64,
    pub max_lat: f64,
    pub min_lon: f64,
    pub max_lon: f64,
}

impl Bounds {
    pub fn around(lat: f64, lon: f64) -> Self {
        Self {
            min_lat: lat,
            max_lat: lat,
            min_lon: lon,
            max_lon: lon,
        }
    }

    pub fn include(self, lat: f64, lon: f64) -> Self {
        Self {
            min_lat: self.min_lat.min(lat),
            max_lat: self.max_lat.max(lat),
            min_lon: self.min_lon.min(lon),
            max_lon: self.max_lon.max(lon),
        }
    }
}

#[derive(Debug)]
pub enum ImportError {
    SourceNotFound,
    /// Neither the root nor its UNC target can be read: the NAS is off, the drive unplugged.
    SourceOffline(PathBuf),
    Library(library::Error),
}

impl std::fmt::Display for ImportError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::SourceNotFound => write!(f, "the source isn't in the library"),
            Self::SourceOffline(p) => write!(f, "the source {} can't be reached", p.display()),
            Self::Library(e) => write!(f, "library error: {e}"),
        }
    }
}

impl std::error::Error for ImportError {}

impl From<library::Error> for ImportError {
    fn from(e: library::Error) -> Self {
        Self::Library(e)
    }
}

/// One photo to read: its files and, for a known photo, its record id.
struct Job {
    group: Group,
    id: Option<i64>,
    restored: bool,
    /// The files are unchanged; the photo is read again only because its metadata was
    /// unreadable last time.
    retry: bool,
}

struct Done {
    job: Job,
    /// The record, or the I/O error that kept the photo from being read.
    outcome: Result<PhotoRecord, String>,
}

/// Imports (or rescans) source `source_id` with `readers` parallel metadata readers.
pub fn run(
    lib: &mut Library,
    source_id: i64,
    cancel: &AtomicBool,
    readers: usize,
    mut progress: impl FnMut(Progress),
) -> Result<Summary, ImportError> {
    let source = lib.source(source_id)?.ok_or(ImportError::SourceNotFound)?;
    let root = reachable_root(&source.root_path, source.unc_path.as_deref())
        .ok_or_else(|| ImportError::SourceOffline(source.root_path.clone()))?;
    let cancelled = || cancel.load(Ordering::Relaxed);

    let mut listing = scan::list(&root, &source.scope, &cancelled);
    progress(Progress::Listing {
        files: listing.files.len(),
    });
    let mut summary = Summary {
        unreadable_folders: listing.unreadable,
        ..Summary::default()
    };
    if cancelled() {
        summary.cancelled = true;
        return Ok(summary);
    }

    let groups = scan::group(std::mem::take(&mut listing.files));
    summary.photos = groups.len();
    let mut existing = lib.existing(source_id)?;
    // Records of files MapCraft now ignores (`._` AppleDouble files, imported before they were
    // ignored) are deleted rather than left behind as "missing".
    let ignored: Vec<i64> = existing
        .iter()
        .filter(|((_, stem), _)| scan::is_ignored_file(stem))
        .map(|(_, e)| e.id)
        .collect();
    if !ignored.is_empty() {
        existing.retain(|(_, stem), _| !scan::is_ignored_file(stem));
        let tx = lib.transaction()?;
        library::delete_photos(&tx, &ignored)?;
        tx.commit()?;
        summary.removed = ignored.len();
    }
    let mut jobs = Vec::new();
    for group in groups {
        match existing.remove(&group.key()) {
            // Unchanged files are skipped, unless the last read failed: try those again.
            Some(e) if e.files_sig == group.signature() && !e.missing && !e.has_error => {
                summary.unchanged += 1
            }
            Some(e) => {
                let retry = !e.missing && e.files_sig == group.signature();
                jobs.push(Job {
                    group,
                    id: Some(e.id),
                    restored: e.missing,
                    retry,
                })
            }
            None => jobs.push(Job {
                group,
                id: None,
                restored: false,
                retry: false,
            }),
        }
    }
    // What's left in `existing` wasn't found. It may be missing, or it may have moved; the
    // listing decides whether its folder was seen well enough to judge (`can_judge`).
    let mut vanished: Vec<library::Existing> = existing
        .into_values()
        .filter(|e| !e.missing && listing.can_judge(&e.rel_dir_key))
        .collect();

    let total = jobs.len();
    progress(Progress::Reading { done: 0, total });
    let queue = Mutex::new(jobs.into_iter());
    let done_count = AtomicUsize::new(0);
    let (tx, rx) = mpsc::channel::<Done>();

    // `thread::scope` lets the reader threads borrow `queue` and `root`, and joins them all
    // before it returns (like an ExecutorService that is always shut down and awaited).
    let write_result: Result<(), library::Error> = std::thread::scope(|s| {
        for _ in 0..readers.max(1) {
            let tx = tx.clone();
            let (queue, root, cancel) = (&queue, &root, cancel);
            s.spawn(move || {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let next = queue.lock().map(|mut q| q.next());
                    let Ok(Some(job)) = next else { break };
                    // A malformed file must not take the import down (design.md §3.8).
                    let outcome = catch_unwind(AssertUnwindSafe(|| read_photo(root, &job.group)))
                        .unwrap_or_else(|_| Ok(failed_record(&job.group, "the reader crashed")));
                    if tx.send(Done { job, outcome }).is_err() {
                        break;
                    }
                }
            });
        }
        drop(tx); // the channel closes once every reader has finished

        let mut batch: Vec<Done> = Vec::with_capacity(BATCH);
        for done in rx {
            batch.push(done);
            let n = done_count.fetch_add(1, Ordering::Relaxed) + 1;
            if batch.len() == BATCH {
                write_batch(lib, source_id, &mut batch, &mut vanished, &mut summary)?;
                progress(Progress::Reading { done: n, total });
            }
        }
        write_batch(lib, source_id, &mut batch, &mut vanished, &mut summary)?;
        Ok(())
    });
    write_result?;
    progress(Progress::Reading {
        done: done_count.load(Ordering::Relaxed),
        total,
    });

    if cancelled() {
        summary.cancelled = true;
        return Ok(summary);
    }
    let missing: Vec<i64> = vanished.iter().map(|e| e.id).collect();
    let tx = lib.transaction()?;
    library::mark_missing(&tx, &missing)?;
    tx.commit()?;
    summary.missing = missing.len();
    lib.set_last_scan(source_id)?;
    Ok(summary)
}

fn write_batch(
    lib: &mut Library,
    source_id: i64,
    batch: &mut Vec<Done>,
    vanished: &mut Vec<library::Existing>,
    summary: &mut Summary,
) -> Result<(), library::Error> {
    if batch.is_empty() {
        return Ok(());
    }
    let tx = lib.transaction()?;
    for Done { job, outcome } in batch.drain(..) {
        let Ok(record) = outcome else {
            // Couldn't read a file at all: leave the record (or its absence) as it is.
            summary.skipped += 1;
            continue;
        };
        let mut id = job.id;
        if id.is_none() {
            // A new name: is it a vanished photo that moved? Same size and same quick hash.
            if let Some(i) = vanished.iter().position(|e| {
                e.file_size == record.file_size
                    && e.quick_hash.is_some()
                    && e.quick_hash == record.quick_hash
            }) {
                id = Some(vanished.swap_remove(i).id);
                summary.moved += 1;
            } else {
                summary.new += 1;
            }
        } else if job.restored {
            summary.restored += 1;
        } else if job.retry && record.read_error.is_some() {
            summary.unchanged += 1; // still unreadable: nothing changed
        } else {
            summary.changed += 1;
        }
        summary.failed += record.read_error.is_some() as usize;
        summary.without_position += record.lat.is_none() as usize;
        if let (Some(lat), Some(lon)) = (record.lat, record.lon) {
            summary.bounds = Some(match summary.bounds {
                Some(b) => b.include(lat, lon),
                None => Bounds::around(lat, lon),
            });
        }
        library::write_photo(&tx, source_id, id, &record)?;
    }
    tx.commit()
}

/// The root if it can be read, else the UNC target of its mapped drive (design.md §2.1(a)).
pub fn reachable_root(root: &Path, unc: Option<&str>) -> Option<PathBuf> {
    if std::fs::read_dir(root).is_ok() {
        return Some(root.to_path_buf());
    }
    let unc = PathBuf::from(unc?);
    std::fs::read_dir(&unc).is_ok().then_some(unc)
}

/// Reads one photo. `Err` is an I/O error (the file couldn't be opened or read at all): the
/// caller then leaves the photo's record alone, and the next rescan tries again.
///
/// The position comes from the XMP sidecar if it has one, else from the original's EXIF. The
/// preview JPEG is read only when the original's metadata is unreadable or has no capture date:
/// on the NAS every extra file costs a disk seek, and a camera that leaves GPS out of the RAW
/// leaves it out of the JPEG too **(unconfirmed, design.md §2.8(a))**.
fn read_photo(root: &Path, g: &Group) -> Result<PhotoRecord, String> {
    let dir = if g.rel_dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&g.rel_dir)
    };
    // One open per original: the quick hash and the metadata share the handle (and the OS
    // cache serves the head twice); on the NAS each open costs a disk seek.
    let mut file = File::open(dir.join(&g.original.name)).map_err(|e| e.to_string())?;
    let quick_hash = quick_hash(&mut file, g.original.size).map_err(|e| e.to_string())?;
    let (mut m, mut error) = match metadata::read_image_from(file) {
        Ok(m) => (m, None),
        Err(ReadError::Io(e)) => return Err(e),
        Err(e @ ReadError::Format(_)) => (Metadata::default(), Some(e.to_string())),
    };
    if let Some(p) = &g.preview
        && (error.is_some() || m.taken_at.is_none())
    {
        match metadata::read_image(&dir.join(&p.name)) {
            Ok(pm) => fill_gaps(&mut m, pm), // usable through its JPEG; a RAW error stays noted
            Err(ReadError::Io(e)) => return Err(e),
            Err(ReadError::Format(_)) => {}
        }
    }
    let mut gps_source = m.gps.map(|_| "exif");
    if let Some(sc) = &g.sidecar {
        let xml = std::fs::read(dir.join(&sc.name)).map_err(|e| e.to_string())?;
        match xmp::read_gps(&String::from_utf8_lossy(&xml)) {
            Ok(Some(gps)) => {
                m.gps = Some(gps);
                gps_source = Some("xmp");
            }
            Ok(None) => {}
            Err(e) => error = error.or(Some(format!("sidecar: {e}"))),
        }
    }
    Ok(PhotoRecord {
        rel_dir: g.rel_dir.clone(),
        stem: g.stem.clone(),
        original_name: g.original.name.clone(),
        preview_name: g.preview.as_ref().map(|p| p.name.clone()),
        sidecar_name: g.sidecar.as_ref().map(|s| s.name.clone()),
        files_sig: g.signature(),
        file_size: g.original.size as i64,
        file_mtime: g.original.mtime,
        quick_hash: Some(quick_hash),
        lat: m.gps.map(|p| p.lat),
        lon: m.gps.map(|p| p.lon),
        alt: m.gps.and_then(|p| p.alt),
        gps_source,
        taken_at: m.taken_at,
        taken_at_offset: m.taken_at_offset,
        camera_make: m.camera_make,
        camera_model: m.camera_model,
        lens: m.lens,
        width: m.width,
        height: m.height,
        orientation: m.orientation,
        read_error: error,
    })
}

fn fill_gaps(m: &mut Metadata, from: Metadata) {
    m.gps = m.gps.or(from.gps);
    m.taken_at = m.taken_at.take().or(from.taken_at);
    m.taken_at_offset = m.taken_at_offset.take().or(from.taken_at_offset);
    m.camera_make = m.camera_make.take().or(from.camera_make);
    m.camera_model = m.camera_model.take().or(from.camera_model);
    m.lens = m.lens.take().or(from.lens);
    m.orientation = m.orientation.or(from.orientation);
}

fn failed_record(g: &Group, why: &str) -> PhotoRecord {
    PhotoRecord {
        rel_dir: g.rel_dir.clone(),
        stem: g.stem.clone(),
        original_name: g.original.name.clone(),
        preview_name: g.preview.as_ref().map(|p| p.name.clone()),
        sidecar_name: g.sidecar.as_ref().map(|s| s.name.clone()),
        files_sig: g.signature(),
        file_size: g.original.size as i64,
        file_mtime: g.original.mtime,
        read_error: Some(why.to_owned()),
        ..PhotoRecord::default()
    }
}

/// xxh3 of the file's size, first 64 KB and last 64 KB: a cheap fingerprint to recognise a file
/// that moved (design.md §2.1(c), user-confirmed). Reads from an open handle, so the import opens
/// each original only once.
pub fn quick_hash(file: &mut File, size: u64) -> std::io::Result<i64> {
    const PART: u64 = 64 * 1024;
    let mut h = Xxh3::new();
    h.update(&size.to_le_bytes());
    file.seek(SeekFrom::Start(0))?;
    let mut head = vec![0; PART.min(size) as usize];
    file.read_exact(&mut head)?;
    h.update(&head);
    if size > PART {
        // The tail, without overlapping the head for files under 128 KB.
        let start = size.saturating_sub(PART).max(PART);
        let mut tail = vec![0; (size - start) as usize];
        file.seek(SeekFrom::Start(start))?;
        file.read_exact(&mut tail)?;
        h.update(&tail);
    }
    Ok(h.digest() as i64)
}

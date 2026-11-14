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

pub mod scan;
#[cfg(test)]
mod tests;

use std::fs::File;
use std::io::Read;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Mutex, mpsc};

use xxhash_rust::xxh3::Xxh3;

use crate::library::{self, Library, PhotoRecord};
use crate::metadata::{self, Metadata, xmp};
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
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Summary {
    pub photos: usize,
    pub new: usize,
    pub changed: usize,
    pub unchanged: usize,
    pub moved: usize,
    pub restored: usize,
    pub missing: usize,
    /// Photos whose metadata couldn't be read (recorded, not fatal).
    pub failed: usize,
    /// Of the photos written in this run, those without a position.
    pub without_position: usize,
    pub unreadable_folders: usize,
    pub cancelled: bool,
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
}

struct Done {
    job: Job,
    record: PhotoRecord,
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

    let listing = scan::list(&root, &source.folders, &cancelled);
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

    let groups = scan::group(listing.files);
    summary.photos = groups.len();
    let mut existing = lib.existing(source_id)?;
    let mut jobs = Vec::new();
    for group in groups {
        match existing.remove(&group.key()) {
            Some(e) if e.files_sig == group.signature() && !e.missing => summary.unchanged += 1,
            Some(e) => jobs.push(Job {
                group,
                id: Some(e.id),
                restored: e.missing,
            }),
            None => jobs.push(Job {
                group,
                id: None,
                restored: false,
            }),
        }
    }
    // What's left in `existing` wasn't found. It may be missing, or it may have moved.
    let mut vanished: Vec<library::Existing> = existing
        .into_values()
        .filter(|e| !e.missing && listing.listed.contains(&e.rel_dir_key))
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
                    let record = catch_unwind(AssertUnwindSafe(|| read_photo(root, &job.group)))
                        .unwrap_or_else(|_| failed_record(&job.group, "the reader crashed"));
                    if tx.send(Done { job, record }).is_err() {
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
    for Done { job, record } in batch.drain(..) {
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
        } else {
            summary.changed += 1;
        }
        summary.failed += record.read_error.is_some() as usize;
        summary.without_position += record.lat.is_none() as usize;
        library::write_photo(&tx, source_id, id, &record)?;
    }
    tx.commit()
}

/// The root if it can be read, else the UNC target of its mapped drive (design.md §2.1(a)).
fn reachable_root(root: &Path, unc: Option<&str>) -> Option<PathBuf> {
    if std::fs::read_dir(root).is_ok() {
        return Some(root.to_path_buf());
    }
    let unc = PathBuf::from(unc?);
    std::fs::read_dir(&unc).is_ok().then_some(unc)
}

/// Reads one photo's metadata. The position comes from the XMP sidecar if it has one, else
/// from the original's EXIF. The preview JPEG is read only when the original couldn't be read
/// or has no capture date: on the NAS every extra file costs a disk seek, and a camera that
/// leaves GPS out of the RAW leaves it out of the JPEG too **(unconfirmed, design.md §2.8(a))**.
fn read_photo(root: &Path, g: &Group) -> PhotoRecord {
    let dir = if g.rel_dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&g.rel_dir)
    };
    let original = dir.join(&g.original.name);
    let (mut m, mut error) = match metadata::read_image(&original) {
        Ok(m) => (m, None),
        Err(e) => (Metadata::default(), Some(e)),
    };
    if let Some(p) = &g.preview
        && (error.is_some() || m.taken_at.is_none())
        && let Ok(pm) = metadata::read_image(&dir.join(&p.name))
    {
        fill_gaps(&mut m, pm);
        // The photo is still usable through its JPEG; the error stays recorded for the RAW.
    }
    let mut gps_source = m.gps.map(|_| "exif");
    if let Some(sc) = &g.sidecar {
        match std::fs::read_to_string(dir.join(&sc.name)).map_err(|e| e.to_string()) {
            Ok(xml) => match xmp::read_gps(&xml) {
                Ok(Some(gps)) => {
                    m.gps = Some(gps);
                    gps_source = Some("xmp");
                }
                Ok(None) => {}
                Err(e) => error = error.or(Some(format!("sidecar: {e}"))),
            },
            Err(e) => error = error.or(Some(format!("sidecar: {e}"))),
        }
    }
    PhotoRecord {
        rel_dir: g.rel_dir.clone(),
        stem: g.stem.clone(),
        original_name: g.original.name.clone(),
        preview_name: g.preview.as_ref().map(|p| p.name.clone()),
        sidecar_name: g.sidecar.as_ref().map(|s| s.name.clone()),
        files_sig: g.signature(),
        file_size: g.original.size as i64,
        file_mtime: g.original.mtime,
        quick_hash: quick_hash(&original, g.original.size),
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
    }
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

/// xxh3 of the file's size and first 64 KB: a cheap fingerprint to recognise a file that moved
/// (design.md §2.1(c)). The start of a photo holds its EXIF, timestamp included, so two
/// different photos rarely share it. It's the same bytes the metadata reader just read, so the
/// OS cache serves them; reading the end as well cost an extra disk seek per photo, a third of
/// the import time on the NAS (measured 2026-11-14).
pub fn quick_hash(path: &Path, size: u64) -> Option<i64> {
    const PART: u64 = 64 * 1024;
    let f = File::open(path).ok()?;
    let mut buf = Vec::with_capacity(PART.min(size) as usize);
    f.take(PART).read_to_end(&mut buf).ok()?;
    let mut h = Xxh3::new();
    h.update(&size.to_le_bytes());
    h.update(&buf);
    Some(h.digest() as i64)
}

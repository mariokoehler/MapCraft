//! Background jobs for the UI (design.md §3.7): one import or rescan at a time, on its own
//! thread with its own library connection, reporting over a channel.
//!
//! In Java terms: a single-thread executor whose task posts events to a `BlockingQueue` the UI
//! drains every frame. The UI never waits on it; each event asks egui to repaint, so progress
//! shows without the mouse moving.

use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver, TryRecvError};
use std::time::Instant;

use crate::import::{self, Progress, Summary};
use crate::library::Library;

#[derive(Debug, Clone, PartialEq)]
pub enum JobEvent {
    Progress(Progress),
    Done(Summary),
    Failed(String),
}

/// A running import or rescan of one source.
pub struct ImportJob {
    pub source_id: i64,
    /// Whether this is the source's first import (the summary words it differently).
    pub first: bool,
    pub started: Instant,
    pub last: Option<Progress>,
    rx: Receiver<JobEvent>,
    cancel: Arc<AtomicBool>,
    finished: bool,
}

impl ImportJob {
    /// Starts importing `source_id` from the library at `db`, with `readers` parallel readers.
    pub fn start(
        db: PathBuf,
        source_id: i64,
        first: bool,
        readers: usize,
        ctx: egui::Context,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("mapcraft-import".into())
            .spawn(move || {
                let send = |e: JobEvent| {
                    let _ = tx.send(e);
                    ctx.request_repaint();
                };
                let result = Library::open(&db)
                    .map_err(|e| e.to_string())
                    .and_then(|mut lib| {
                        import::run(&mut lib, source_id, &flag, readers, |p| {
                            send(JobEvent::Progress(p))
                        })
                        .map_err(|e| e.to_string())
                    });
                send(match result {
                    Ok(summary) => JobEvent::Done(summary),
                    Err(e) => JobEvent::Failed(e),
                });
            })
            .expect("spawn the import thread");
        Self {
            source_id,
            first,
            started: Instant::now(),
            last: None,
            rx,
            cancel,
            finished: false,
        }
    }

    /// Asks the import to stop; it keeps what it has written (design.md §3.4: it resumes).
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    /// The events since the last call. A thread that ended without a result (it panicked)
    /// shows up as `Failed`, so the UI never waits forever.
    pub fn poll(&mut self) -> Vec<JobEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => {
                    if let JobEvent::Progress(p) = &e {
                        self.last = Some(p.clone());
                    }
                    if matches!(e, JobEvent::Done(_) | JobEvent::Failed(_)) {
                        self.finished = true;
                    }
                    out.push(e);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.finished {
                        self.finished = true;
                        out.push(JobEvent::Failed("the import stopped unexpectedly".into()));
                    }
                    break;
                }
            }
        }
        out
    }
}

/// What the thumbnail job reports.
#[derive(Debug, Clone, PartialEq)]
pub enum ThumbEvent {
    Progress { done: usize, total: usize },
    Done(ThumbSummary),
    Failed(String),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ThumbSummary {
    pub made: usize,
    /// No usable source (a placeholder marker): unsupported format, too large, damaged.
    pub placeholders: usize,
    /// Couldn't read a file now (NAS hiccup, offline source): tried again next time.
    pub skipped: usize,
    pub cancelled: bool,
}

/// Makes the missing thumbnails in the background (design.md §2.2, §3.7): the photos on the map
/// whose `thumb_state` is still `none`, with `workers` parallel renderers.
pub struct ThumbJob {
    pub done: usize,
    pub total: usize,
    rx: Receiver<ThumbEvent>,
    cancel: Arc<AtomicBool>,
    finished: bool,
}

/// Thumbnail state updates per database transaction.
const THUMB_BATCH: usize = 50;

impl ThumbJob {
    /// `extra`: photos without a position to make thumbnails for too, first (geotagging's
    /// candidates, design.md §2.9(g)).
    pub fn start(
        db: PathBuf,
        data: PathBuf,
        workers: usize,
        extra: Vec<i64>,
        ctx: egui::Context,
    ) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("mapcraft-thumbs".into())
            .spawn(move || {
                let send = |e: ThumbEvent| {
                    let _ = tx.send(e);
                    ctx.request_repaint();
                };
                let result = run_thumbs(&db, &data, workers, &extra, &flag, &send);
                send(match result {
                    Ok(s) => ThumbEvent::Done(s),
                    Err(e) => ThumbEvent::Failed(e),
                });
            })
            .expect("spawn the thumbnail thread");
        Self {
            done: 0,
            total: 0,
            rx,
            cancel,
            finished: false,
        }
    }

    /// Asks the job to stop; made thumbnails are kept and the rest are made next time.
    pub fn cancel(&self) {
        self.cancel.store(true, Ordering::Relaxed);
    }

    pub fn cancelling(&self) -> bool {
        self.cancel.load(Ordering::Relaxed)
    }

    pub fn poll(&mut self) -> Vec<ThumbEvent> {
        let mut out = Vec::new();
        loop {
            match self.rx.try_recv() {
                Ok(e) => {
                    if let ThumbEvent::Progress { done, total } = e {
                        (self.done, self.total) = (done, total);
                    }
                    if matches!(e, ThumbEvent::Done(_) | ThumbEvent::Failed(_)) {
                        self.finished = true;
                    }
                    out.push(e);
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    if !self.finished {
                        self.finished = true;
                        out.push(ThumbEvent::Failed(
                            "the thumbnail job stopped unexpectedly".into(),
                        ));
                    }
                    break;
                }
            }
        }
        out
    }
}

/// One thumbnail's outcome, from a worker to the writer.
enum Made {
    Ok,
    Placeholder,
    Skipped,
}

fn run_thumbs(
    db: &std::path::Path,
    data: &std::path::Path,
    workers: usize,
    extra: &[i64],
    cancel: &AtomicBool,
    send: &dyn Fn(ThumbEvent),
) -> Result<ThumbSummary, String> {
    use crate::library::{self, THUMB_FAILED, THUMB_OK};
    use crate::thumbs::{self, PhotoFiles, THUMB};
    use std::collections::HashMap;
    use std::panic::{AssertUnwindSafe, catch_unwind};
    use std::sync::Mutex;

    let mut lib = Library::open(db).map_err(|e| e.to_string())?;
    // First, remove the files of photos deleted since the last run (they're the user's images).
    thumbs::sweep(data, &lib.all_ids().map_err(|e| e.to_string())?);
    let tasks = lib.thumb_tasks_with(extra).map_err(|e| e.to_string())?;
    let total = tasks.len();
    let mut summary = ThumbSummary::default();
    send(ThumbEvent::Progress { done: 0, total });

    // Resolve each source's root once; an unreachable source's photos are skipped for now.
    type RootKey = (PathBuf, Option<String>);
    let mut roots: HashMap<RootKey, Option<PathBuf>> = HashMap::new();
    for t in &tasks {
        roots
            .entry((t.root.clone(), t.unc.clone()))
            .or_insert_with(|| import::reachable_root(&t.root, t.unc.as_deref()));
    }

    let queue = Mutex::new(tasks.iter());
    let heavy = Mutex::new(()); // one full decode at a time
    let (rtx, rrx) = mpsc::channel::<(i64, Made)>();
    let write: Result<(), String> = std::thread::scope(|s| {
        for _ in 0..workers.max(1) {
            let rtx = rtx.clone();
            let (queue, heavy, roots) = (&queue, &heavy, &roots);
            s.spawn(move || {
                loop {
                    if cancel.load(Ordering::Relaxed) {
                        break;
                    }
                    let Some(t) = queue.lock().ok().and_then(|mut q| q.next()) else {
                        break;
                    };
                    let Some(root) = roots[&(t.root.clone(), t.unc.clone())].clone() else {
                        let _ = rtx.send((t.id, Made::Skipped));
                        continue;
                    };
                    let dir = if t.rel_dir.is_empty() {
                        root
                    } else {
                        root.join(&t.rel_dir)
                    };
                    let files = PhotoFiles {
                        dir: &dir,
                        original: &t.original,
                        preview: t.preview.as_deref(),
                        orientation: t.orientation,
                    };
                    let _guard =
                        thumbs::is_heavy(&t.original, t.preview.as_deref()).then(|| heavy.lock());
                    // A malformed file must not take the job down (design.md §3.8).
                    let outcome = catch_unwind(AssertUnwindSafe(|| thumbs::render(&files, THUMB)));
                    let made = match outcome {
                        Ok(Ok((jpeg, _))) => {
                            match thumbs::store(&thumbs::path(data, t.id, THUMB), &jpeg) {
                                Ok(()) => {
                                    // A preview from before the photo changed (or from a former
                                    // photo with this id) is stale: the panel makes a new one.
                                    let _ = std::fs::remove_file(thumbs::path(
                                        data,
                                        t.id,
                                        thumbs::PREVIEW,
                                    ));
                                    Made::Ok
                                }
                                Err(_) => Made::Skipped,
                            }
                        }
                        Ok(Err(thumbs::ThumbError::Io(_))) => Made::Skipped,
                        Ok(Err(thumbs::ThumbError::Unusable(_))) | Err(_) => Made::Placeholder,
                    };
                    if rtx.send((t.id, made)).is_err() {
                        break;
                    }
                }
            });
        }
        drop(rtx);
        let mut batch = Vec::with_capacity(THUMB_BATCH);
        let mut done = 0;
        let mut flush = |batch: &mut Vec<(i64, Made)>| -> Result<(), String> {
            let tx = lib.transaction().map_err(|e| e.to_string())?;
            for (id, made) in batch.drain(..) {
                match made {
                    Made::Ok => library::set_thumb_state(&tx, id, THUMB_OK),
                    Made::Placeholder => library::set_thumb_state(&tx, id, THUMB_FAILED),
                    Made::Skipped => Ok(()),
                }
                .map_err(|e| e.to_string())?;
            }
            tx.commit().map_err(|e| e.to_string())
        };
        for (id, made) in rrx {
            match made {
                Made::Ok => summary.made += 1,
                Made::Placeholder => summary.placeholders += 1,
                Made::Skipped => summary.skipped += 1,
            }
            batch.push((id, made));
            done += 1;
            if batch.len() == THUMB_BATCH {
                flush(&mut batch)?;
                send(ThumbEvent::Progress { done, total });
            }
        }
        flush(&mut batch)?;
        send(ThumbEvent::Progress { done, total });
        Ok(())
    });
    write?;
    summary.cancelled = cancel.load(Ordering::Relaxed);
    Ok(summary)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::scope::Scope;

    #[test]
    fn the_thumbnail_job_makes_thumbnails_for_photos_on_the_map() {
        use crate::thumbs::{self, THUMB};
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        // Real pixels: one JPEG with a position (EXIF spliced in), one without.
        let img = image::RgbImage::from_fn(800, 400, |x, _| image::Rgb([(x / 4) as u8, 80, 160]));
        let with_gps = |jpeg: Vec<u8>| {
            let exif = crate::demo::jpeg_with_gps(64.1, -21.9, "2020:08:15 12:00:00");
            let mut out = exif[..exif.len() - 2].to_vec(); // SOI + APP1, without the EOI
            out.extend(&jpeg[2..]); // the image's segments, without its SOI
            out
        };
        std::fs::write(photos.join("a.jpg"), with_gps(thumbs::encode(&img))).unwrap();
        std::fs::write(photos.join("b.jpg"), thumbs::encode(&img)).unwrap();
        let db = dir.path().join("library.sqlite");
        let mut lib = Library::open(&db).unwrap();
        let src = lib.add_source(&photos, None, "t", &Scope::all()).unwrap();
        crate::import::run(&mut lib, src, &AtomicBool::new(false), 1, |_| {}).unwrap();
        assert_eq!(
            lib.thumb_tasks().unwrap().len(),
            1,
            "only the photo with a position"
        );

        let data = dir.path().join("data");
        let wait = |job: &mut ThumbJob| {
            let deadline = Instant::now() + std::time::Duration::from_secs(20);
            loop {
                if let Some(ThumbEvent::Done(s)) = job
                    .poll()
                    .into_iter()
                    .find(|e| matches!(e, ThumbEvent::Done(_)))
                {
                    return s;
                }
                assert!(Instant::now() < deadline, "the job never finished");
                std::thread::sleep(std::time::Duration::from_millis(20));
            }
        };
        let mut job = ThumbJob::start(
            db.clone(),
            data.clone(),
            2,
            Vec::new(),
            egui::Context::default(),
        );
        let summary = wait(&mut job);
        assert_eq!(
            (summary.made, summary.placeholders, summary.skipped),
            (1, 0, 0)
        );
        let id = lib.thumbed().unwrap().into_iter().next().unwrap();
        let thumb = std::fs::read(thumbs::path(&data, id, THUMB)).unwrap();
        assert_eq!(
            thumbs::decode_jpeg(&thumb, 1024).unwrap().dimensions(),
            (256, 128)
        );
        assert!(lib.thumb_tasks().unwrap().is_empty(), "nothing left to do");

        // Removing the source removes the thumbnail file on the next run (the sweep).
        lib.remove_source(src).unwrap();
        let mut job = ThumbJob::start(db, data.clone(), 1, Vec::new(), egui::Context::default());
        wait(&mut job);
        assert!(!thumbs::path(&data, id, THUMB).exists());
    }

    #[test]
    fn a_job_reports_progress_and_its_summary() {
        let dir = tempfile::tempdir().unwrap();
        let photos = dir.path().join("photos");
        crate::demo::make_demo(&photos, 24).unwrap();
        let db = dir.path().join("library.sqlite");
        let src = Library::open(&db)
            .unwrap()
            .add_source(&photos, None, "demo", &Scope::all())
            .unwrap();
        let mut job = ImportJob::start(db, src, true, 2, egui::Context::default());
        let deadline = Instant::now() + std::time::Duration::from_secs(20);
        let summary = loop {
            let events = job.poll();
            if let Some(JobEvent::Done(s)) = events.iter().find(|e| matches!(e, JobEvent::Done(_)))
            {
                break s.clone();
            }
            assert!(
                !events.iter().any(|e| matches!(e, JobEvent::Failed(_))),
                "{events:?}"
            );
            assert!(Instant::now() < deadline, "the job never finished");
            std::thread::sleep(std::time::Duration::from_millis(20));
        };
        assert_eq!((summary.new, summary.without_position), (24, 3));
        assert!(job.last.is_some(), "progress was reported");
        let b = summary.bounds.unwrap();
        assert!(b.min_lat < b.max_lat && b.min_lon < b.max_lon);
        assert!(job.poll().is_empty(), "nothing after the end");
    }
}

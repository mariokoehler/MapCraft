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

#[cfg(test)]
mod tests {
    use super::*;
    use crate::import::scope::Scope;

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

//! Writing a session's positions, and undoing it (design.md §2.9(i)). Before the first sidecar
//! changes, a journal in the data folder (`geotag/<time>.json`) records every target's earlier
//! content and the photo's earlier library values; after writing, each file's new hash. Undo
//! restores a sidecar only while it still holds exactly what was written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::sidecar::{self, Gps};
use super::time;

/// The journal folder inside the data folder.
pub const DIR: &str = "geotag";

/// A photo's position in the library before it was tagged, to put back on undo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prior {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt: Option<f64>,
    pub gps_source: Option<String>,
    pub sidecar_name: Option<String>,
}

/// One sidecar to write.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub photo: i64,
    pub path: PathBuf,
    pub gps: Gps,
    pub prior: Prior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub photo: i64,
    pub path: PathBuf,
    /// The sidecar as it was (`None`: there was none). Sidecars are only written when UTF-8.
    pub before: Option<String>,
    /// The SHA-256 of what was written, once it was.
    pub after: Option<String>,
    pub prior: Prior,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    /// When it was written (UTC).
    pub made: String,
    pub entries: Vec<Entry>,
}

impl Journal {
    pub fn load(path: &Path) -> Result<Self, String> {
        let text = std::fs::read_to_string(path).map_err(|e| e.to_string())?;
        serde_json::from_str(&text).map_err(|e| e.to_string())
    }

    /// Writes it and waits until it's on disk.
    fn save(&self, path: &Path) -> Result<(), String> {
        use std::io::Write as _;
        let json = serde_json::to_string_pretty(self).map_err(|e| e.to_string())?;
        let mut f = std::fs::File::create(path).map_err(|e| e.to_string())?;
        f.write_all(json.as_bytes()).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())
    }
}

fn sha(b: &[u8]) -> String {
    Sha256::digest(b)
        .iter()
        .map(|x| format!("{x:02x}"))
        .collect()
}

/// A sidecar that was written, with the position as it reads back (what the library records,
/// so a rescan finds the same).
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub photo: i64,
    pub sidecar_name: String,
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,
    pub created: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub written: Vec<Written>,
    /// Photos whose sidecar couldn't be written, and why.
    pub failed: Vec<(i64, String)>,
    /// The journal, if anything was written.
    pub journal: Option<PathBuf>,
    pub cancelled: bool,
}

/// Writes every task's sidecar. `progress(done, total)`; `cancel` stops between files.
pub fn run(
    tasks: &[Task],
    data: &Path,
    progress: &dyn Fn(usize, usize),
    cancel: &AtomicBool,
) -> Report {
    let mut report = Report::default();
    let dir = data.join(DIR);
    if let Err(e) = std::fs::create_dir_all(&dir) {
        report.failed = tasks
            .iter()
            .map(|t| (t.photo, format!("no journal: {e}")))
            .collect();
        return report;
    }
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0.0, |d| d.as_secs_f64());
    let stamp = time::format_utc(now).replace([':', '-'], "");
    let mut path = dir.join(format!("{stamp}.json"));
    let mut n = 1;
    while path.exists() {
        path = dir.join(format!("{stamp}-{n}.json"));
        n += 1;
    }

    // First what each target holds, all of it in the journal before anything is written.
    let mut journal = Journal {
        made: time::format_utc(now),
        entries: Vec::new(),
    };
    let mut todo = Vec::new();
    for t in tasks {
        match sidecar::read(&t.path) {
            Ok(before) => match before.as_deref().map(std::str::from_utf8) {
                Some(Err(_)) => report
                    .failed
                    .push((t.photo, "the sidecar isn't UTF-8".into())),
                b => {
                    let text = b.map(|r| r.unwrap_or_default().to_owned());
                    journal.entries.push(Entry {
                        photo: t.photo,
                        path: t.path.clone(),
                        before: text,
                        after: None,
                        prior: t.prior.clone(),
                    });
                    todo.push((t, before));
                }
            },
            Err(e) => report.failed.push((t.photo, e)),
        }
    }
    if todo.is_empty() {
        return report;
    }
    if let Err(e) = journal.save(&path) {
        report.failed.extend(
            todo.iter()
                .map(|(t, _)| (t.photo, format!("no journal: {e}"))),
        );
        return report;
    }

    let total = todo.len();
    for (i, (t, before)) in todo.into_iter().enumerate() {
        if cancel.load(Ordering::Relaxed) {
            report.cancelled = true;
            break;
        }
        progress(i, total);
        let r = sidecar::content(before.as_deref(), &t.gps).and_then(|bytes| {
            sidecar::replace(&t.path, before.as_deref(), Some(&bytes))?;
            let text = String::from_utf8_lossy(&bytes);
            let back = crate::metadata::xmp::read_gps(text.trim_start_matches('\u{feff}'))?
                .ok_or("the position doesn't read back")?;
            Ok((bytes, back))
        });
        match r {
            Ok((bytes, back)) => {
                if let Some(e) = journal.entries.iter_mut().find(|e| e.photo == t.photo) {
                    e.after = Some(sha(&bytes));
                }
                report.written.push(Written {
                    photo: t.photo,
                    sidecar_name: t
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    lat: back.lat,
                    lon: back.lon,
                    alt: back.alt,
                    created: before.is_none(),
                });
            }
            Err(e) => report.failed.push((t.photo, e)),
        }
    }
    progress(total, total);
    journal.entries.retain(|e| e.after.is_some());
    if journal.entries.is_empty() {
        let _ = std::fs::remove_file(&path);
    } else {
        if let Err(e) = journal.save(&path) {
            // The first save holds every earlier content; undo still has what it needs, but
            // can't tell which were written. Logged; the files themselves are fine.
            tracing::error!("can't finish the geotag journal: {e}");
        }
        report.journal = Some(path);
    }
    report
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Undone {
    /// Photos whose sidecar was restored, with their earlier library values.
    pub restored: Vec<(i64, Prior)>,
    /// Photos whose sidecar changed since (left alone), or couldn't be restored.
    pub skipped: Vec<(i64, String)>,
}

/// Restores every sidecar of a journal that still holds what was written, then removes the
/// journal if all were.
pub fn undo(path: &Path) -> Result<Undone, String> {
    let journal = Journal::load(path)?;
    let mut out = Undone::default();
    for e in &journal.entries {
        let Some(after) = &e.after else { continue };
        let now = match sidecar::read(&e.path) {
            Ok(Some(b)) => b,
            Ok(None) => {
                out.skipped.push((e.photo, "the sidecar is gone".into()));
                continue;
            }
            Err(err) => {
                out.skipped.push((e.photo, err));
                continue;
            }
        };
        if &sha(&now) != after {
            out.skipped
                .push((e.photo, "changed since it was written".into()));
            continue;
        }
        match sidecar::replace(&e.path, Some(&now), e.before.as_deref().map(str::as_bytes)) {
            Ok(()) => out.restored.push((e.photo, e.prior.clone())),
            Err(err) => out.skipped.push((e.photo, err)),
        }
    }
    if out.skipped.is_empty() {
        let _ = std::fs::remove_file(path);
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gps(lat: f64) -> Gps {
        Gps {
            lat,
            lon: 20.0,
            alt: Some(100.0),
            time: 1.75e9,
        }
    }

    #[test]
    fn writing_then_undoing_restores_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let photos = d.path().join("photos");
        std::fs::create_dir_all(&photos).unwrap();
        let existing = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"4\"/></rdf:RDF></x:xmpmeta>";
        std::fs::write(photos.join("B.xmp"), existing).unwrap();
        std::fs::write(photos.join("C.xmp"), b"\xff\xfe junk").unwrap();
        let task = |photo, name: &str, lat| Task {
            photo,
            path: photos.join(name),
            gps: gps(lat),
            prior: Prior {
                sidecar_name: None,
                ..Default::default()
            },
        };
        let tasks = [
            task(1, "A.xmp", 10.0),
            task(2, "B.xmp", 11.0),
            task(3, "C.xmp", 12.0),
        ];
        let data = d.path().join("data");
        let r = run(&tasks, &data, &|_, _| {}, &AtomicBool::new(false));
        assert_eq!(
            r.written
                .iter()
                .map(|w| (w.photo, w.created))
                .collect::<Vec<_>>(),
            [(1, true), (2, false)]
        );
        assert_eq!(r.failed.iter().map(|f| f.0).collect::<Vec<_>>(), [3]);
        assert_eq!(r.written[1].lat, 11.0);
        let b = std::fs::read_to_string(photos.join("B.xmp")).unwrap();
        assert!(b.contains("xmp:Rating=\"4\"") && b.contains("GPSLatitude"));
        let journal = r.journal.unwrap();
        assert_eq!(Journal::load(&journal).unwrap().entries.len(), 2);

        // Another app changes A afterwards: undo leaves A alone and restores B.
        std::fs::write(photos.join("A.xmp"), "edited").unwrap();
        let u = undo(&journal).unwrap();
        assert_eq!(u.restored.iter().map(|r| r.0).collect::<Vec<_>>(), [2]);
        assert_eq!(u.skipped.iter().map(|s| s.0).collect::<Vec<_>>(), [1]);
        assert_eq!(
            std::fs::read_to_string(photos.join("B.xmp")).unwrap(),
            existing
        );
        assert!(journal.exists(), "kept while something couldn't be undone");

        // A clean run undoes completely: a created sidecar is removed again.
        let r = run(
            &[task(4, "D.xmp", 13.0)],
            &data,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        let u = undo(r.journal.as_ref().unwrap()).unwrap();
        assert_eq!(u.restored.len(), 1);
        assert!(!photos.join("D.xmp").exists());
        assert!(!r.journal.unwrap().exists());
    }

    #[test]
    fn a_cancelled_run_writes_nothing_more() {
        let d = tempfile::tempdir().unwrap();
        let t = Task {
            photo: 1,
            path: d.path().join("A.xmp"),
            gps: gps(1.0),
            prior: Prior::default(),
        };
        let r = run(
            &[t],
            &d.path().join("data"),
            &|_, _| {},
            &AtomicBool::new(true),
        );
        assert!(r.cancelled && r.written.is_empty() && r.journal.is_none());
        assert!(!d.path().join("A.xmp").exists());
    }
}

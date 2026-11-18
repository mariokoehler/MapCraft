//! Writing a session's positions, or removing positions (design.md §2.9(i), (n)), and undoing
//! either. Before the first sidecar changes, a journal in the data folder (`geotag/<time>.json`)
//! records every target's earlier content and the photo's earlier library values; after
//! writing, each file's new hash (or that it was deleted). Undo restores a sidecar only while it
//! still holds exactly what was written.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};

use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::sidecar::{self, Gps};
use super::time;

/// The journal folder inside the data folder.
pub const DIR: &str = "geotag";

/// `after` for a sidecar that was deleted (it held nothing but its GPS).
const DELETED: &str = "deleted";

/// What a journal records: positions written (tagging), or positions removed.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Journals of 0.3–0.5 have no `kind`: they are all tagging.
    #[default]
    Tag,
    Remove,
}

/// A photo's position in the library before it was tagged, to put back on undo.
#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Prior {
    pub lat: Option<f64>,
    pub lon: Option<f64>,
    pub alt: Option<f64>,
    pub gps_source: Option<String>,
    pub sidecar_name: Option<String>,
}

/// One sidecar to write: the position to set, or `None` to remove its GPS.
#[derive(Debug, Clone, PartialEq)]
pub struct Task {
    pub photo: i64,
    pub path: PathBuf,
    pub gps: Option<Gps>,
    pub prior: Prior,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct Entry {
    pub photo: i64,
    pub path: PathBuf,
    /// The sidecar as it was (`None`: there was none). Sidecars are only written when UTF-8.
    pub before: Option<String>,
    /// The SHA-256 of what was written once it was, or `"deleted"`.
    pub after: Option<String>,
    pub prior: Prior,
}

#[derive(Debug, Clone, Default, PartialEq, Serialize, Deserialize)]
pub struct Journal {
    #[serde(default)]
    pub kind: Kind,
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
/// so a rescan finds the same); `None` once its GPS was removed.
#[derive(Debug, Clone, PartialEq)]
pub struct Written {
    pub photo: i64,
    pub sidecar_name: String,
    pub position: Option<crate::metadata::Gps>,
    pub created: bool,
    /// Removing its GPS left nothing in it, so it was deleted.
    pub deleted: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub struct Report {
    pub written: Vec<Written>,
    /// Photos whose sidecar couldn't be written, and why.
    pub failed: Vec<(i64, String)>,
    /// Removing: photos whose sidecar held no position after all (nothing was written).
    pub untouched: Vec<i64>,
    /// The journal, if anything was written.
    pub journal: Option<PathBuf>,
    pub cancelled: bool,
}

/// Writes every task's sidecar (all of one `kind`). `progress(done, total)`; `cancel` stops
/// between files.
pub fn run(
    kind: Kind,
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
        kind,
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
                None if t.gps.is_none() => {
                    report.failed.push((t.photo, "the sidecar is gone".into()))
                }
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
        let r = match &t.gps {
            Some(g) => sidecar::content(before.as_deref(), g).and_then(|bytes| {
                sidecar::replace(&t.path, before.as_deref(), Some(&bytes))?;
                let text = String::from_utf8_lossy(&bytes);
                let back = crate::metadata::xmp::read_gps(text.trim_start_matches('\u{feff}'))?
                    .ok_or("the position doesn't read back")?;
                Ok((Some(bytes), Some(back)))
            }),
            None => {
                let b = before.as_deref().unwrap_or_default();
                sidecar::stripped(b).and_then(|bytes| {
                    sidecar::replace(&t.path, Some(b), bytes.as_deref())?;
                    Ok((bytes, None))
                })
            }
        };
        match r {
            Err(e) if e == sidecar::NO_GPS => report.untouched.push(t.photo),
            Ok((bytes, back)) => {
                let after = bytes.as_deref().map_or_else(|| DELETED.to_owned(), sha);
                // On disk at once, so a run cut short (the window closed) can still be undone.
                if let Err(e) = append_done(&path, t.photo, &after) {
                    tracing::error!("can't note a written sidecar in the journal: {e}");
                }
                if let Some(e) = journal.entries.iter_mut().find(|e| e.photo == t.photo) {
                    e.after = Some(after);
                }
                report.written.push(Written {
                    photo: t.photo,
                    sidecar_name: t
                        .path
                        .file_name()
                        .map(|n| n.to_string_lossy().into_owned())
                        .unwrap_or_default(),
                    position: back,
                    created: before.is_none(),
                    deleted: bytes.is_none(),
                });
            }
            Err(e) => report.failed.push((t.photo, e)),
        }
    }
    progress(total, total);
    journal.entries.retain(|e| e.after.is_some());
    if journal.entries.is_empty() {
        let _ = std::fs::remove_file(&path);
        let _ = std::fs::remove_file(done_path(&path));
    } else {
        match journal.save(&path) {
            Ok(()) => {
                let _ = std::fs::remove_file(done_path(&path));
            }
            // The first save holds every earlier content and the `.done` file what was
            // written: undo still has what it needs.
            Err(e) => tracing::error!("can't finish the geotag journal: {e}"),
        }
        report.journal = Some(path);
    }
    report
}

/// Beside a journal: one line per sidecar written so far (`photo<TAB>sha256`), appended as
/// each is written, until the journal itself records them.
fn done_path(journal: &Path) -> PathBuf {
    journal.with_extension("done")
}

fn append_done(journal: &Path, photo: i64, after: &str) -> Result<(), String> {
    use std::io::Write as _;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(done_path(journal))
        .map_err(|e| e.to_string())?;
    writeln!(f, "{photo}\t{after}").map_err(|e| e.to_string())?;
    f.sync_data().map_err(|e| e.to_string())
}

/// The newest journal of this kind in a data folder, with a line saying what it holds
/// ("12 photos, 2025-06-12 14:02 UTC"), for Undo after the toast is gone. Only the newest of
/// each kind: an older one isn't offered once a newer one exists (design.md §4).
pub fn latest(data: &Path, kind: Kind) -> Option<(PathBuf, String)> {
    let mut all: Vec<(std::time::SystemTime, PathBuf)> = std::fs::read_dir(data.join(DIR))
        .ok()?
        .flatten()
        .filter(|e| e.path().extension().is_some_and(|x| x == "json"))
        .filter_map(|e| Some((e.metadata().and_then(|m| m.modified()).ok()?, e.path())))
        .collect();
    all.sort_by_key(|e| std::cmp::Reverse(e.0));
    let (path, j) = all.into_iter().find_map(|(_, p)| {
        let j = load_with_done(&p).ok().filter(|j| j.kind == kind)?;
        Some((p, j))
    })?;
    let n = j.entries.iter().filter(|e| e.after.is_some()).count();
    if n == 0 {
        return None;
    }
    let made = j.made.trim_end_matches('Z').replace('T', " ");
    let when = made.get(..16).unwrap_or(&made).to_owned();
    Some((
        path,
        format!("{n} photo{}, {when} UTC", if n == 1 { "" } else { "s" }),
    ))
}

/// The journal with what its `.done` file says was written.
fn load_with_done(path: &Path) -> Result<Journal, String> {
    let mut j = Journal::load(path)?;
    if let Ok(done) = std::fs::read_to_string(done_path(path)) {
        for line in done.lines() {
            if let Some((photo, after)) = line.split_once('\t')
                && let Ok(photo) = photo.parse::<i64>()
                && let Some(e) = j.entries.iter_mut().find(|e| e.photo == photo)
            {
                e.after = Some(after.trim().to_owned());
            }
        }
    }
    Ok(j)
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
    let journal = load_with_done(path)?;
    let mut out = Undone::default();
    for e in &journal.entries {
        let Some(after) = &e.after else { continue };
        let now = match sidecar::read(&e.path) {
            Ok(now) => now,
            Err(err) => {
                out.skipped.push((e.photo, err));
                continue;
            }
        };
        // A deleted sidecar must still be gone; a written one must hold what was written.
        let same = match (&now, after.as_str()) {
            (None, DELETED) => true,
            (Some(_), DELETED) => false,
            (None, _) => {
                out.skipped.push((e.photo, "the sidecar is gone".into()));
                continue;
            }
            (Some(b), a) => sha(b) == a,
        };
        if !same {
            out.skipped
                .push((e.photo, "changed since it was written".into()));
            continue;
        }
        match sidecar::replace(
            &e.path,
            now.as_deref(),
            e.before.as_deref().map(str::as_bytes),
        ) {
            Ok(()) => out.restored.push((e.photo, e.prior.clone())),
            Err(err) => out.skipped.push((e.photo, err)),
        }
    }
    if out.skipped.is_empty() {
        let _ = std::fs::remove_file(path);
        let _ = std::fs::remove_file(done_path(path));
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
            gps: Some(gps(lat)),
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
        let r = run(
            Kind::Tag,
            &tasks,
            &data,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert_eq!(
            r.written
                .iter()
                .map(|w| (w.photo, w.created))
                .collect::<Vec<_>>(),
            [(1, true), (2, false)]
        );
        assert_eq!(r.failed.iter().map(|f| f.0).collect::<Vec<_>>(), [3]);
        assert_eq!(r.written[1].position.unwrap().lat, 11.0);
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
            Kind::Tag,
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
    fn a_run_cut_short_can_still_be_undone() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        let t = Task {
            photo: 1,
            path: d.path().join("A.xmp"),
            gps: Some(gps(1.0)),
            prior: Prior::default(),
        };
        let r = run(
            Kind::Tag,
            std::slice::from_ref(&t),
            &data,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        let path = r.journal.unwrap();
        assert!(
            !done_path(&path).exists(),
            "the finished journal holds it all"
        );
        // As if the window closed before the journal was finished: only the first save and
        // the `.done` line.
        let mut j = Journal::load(&path).unwrap();
        let after = j.entries[0].after.take().unwrap();
        j.save(&path).unwrap();
        append_done(&path, 1, &after).unwrap();
        assert_eq!(latest(&data, Kind::Tag).unwrap().0, path);
        assert!(latest(&data, Kind::Tag).unwrap().1.starts_with("1 photo, "));
        assert!(latest(&data, Kind::Remove).is_none());
        let u = undo(&path).unwrap();
        assert_eq!(u.restored.len(), 1);
        assert!(!t.path.exists() && !path.exists() && !done_path(&path).exists());
        assert!(latest(&data, Kind::Tag).is_none());
    }

    #[test]
    fn removing_then_undoing_restores_the_folder() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        let kept = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\" xmp:Rating=\"4\"/></rdf:RDF></x:xmpmeta>";
        let rated = sidecar::merge(kept, &gps(1.0)).unwrap();
        let only_gps = sidecar::create(&gps(2.0));
        std::fs::write(d.path().join("A.xmp"), &rated).unwrap();
        std::fs::write(d.path().join("B.xmp"), &only_gps).unwrap();
        std::fs::write(d.path().join("C.xmp"), kept).unwrap();
        let task = |photo, name: &str| Task {
            photo,
            path: d.path().join(name),
            gps: None,
            prior: Prior {
                lat: Some(1.0),
                lon: Some(20.0),
                gps_source: Some("xmp".into()),
                sidecar_name: Some(name.into()),
                ..Default::default()
            },
        };
        let tasks = [
            task(1, "A.xmp"),
            task(2, "B.xmp"),
            task(3, "C.xmp"),
            task(4, "D.xmp"),
        ];
        let r = run(
            Kind::Remove,
            &tasks,
            &data,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        let w: Vec<_> = r.written.iter().map(|w| (w.photo, w.deleted)).collect();
        assert_eq!(w, [(1, false), (2, true)]);
        assert!(r.written.iter().all(|w| w.position.is_none()));
        assert_eq!(r.untouched, [3], "it held no position");
        assert_eq!(r.failed.iter().map(|f| f.0).collect::<Vec<_>>(), [4]);
        let a = std::fs::read_to_string(d.path().join("A.xmp")).unwrap();
        assert!(a.contains("xmp:Rating=\"4\"") && !a.contains("GPS"));
        assert!(!d.path().join("B.xmp").exists());
        let journal = r.journal.unwrap();
        assert_eq!(Journal::load(&journal).unwrap().kind, Kind::Remove);
        assert!(latest(&data, Kind::Tag).is_none());
        assert!(
            latest(&data, Kind::Remove)
                .unwrap()
                .1
                .starts_with("2 photos, ")
        );

        let u = undo(&journal).unwrap();
        assert_eq!(u.restored.len(), 2);
        assert_eq!(
            std::fs::read_to_string(d.path().join("A.xmp")).unwrap(),
            rated
        );
        assert_eq!(
            std::fs::read_to_string(d.path().join("B.xmp")).unwrap(),
            only_gps
        );
        assert_eq!(u.restored[0].1.gps_source.as_deref(), Some("xmp"));
        assert!(!journal.exists());
    }

    #[test]
    fn a_deleted_sidecar_made_again_since_is_left_alone() {
        let d = tempfile::tempdir().unwrap();
        let data = d.path().join("data");
        let p = d.path().join("A.xmp");
        std::fs::write(&p, sidecar::create(&gps(2.0))).unwrap();
        let t = Task {
            photo: 1,
            path: p.clone(),
            gps: None,
            prior: Prior::default(),
        };
        let r = run(
            Kind::Remove,
            &[t],
            &data,
            &|_, _| {},
            &AtomicBool::new(false),
        );
        assert!(r.written[0].deleted && !p.exists());
        std::fs::write(&p, "another app's").unwrap();
        let u = undo(&r.journal.unwrap()).unwrap();
        assert!(u.restored.is_empty() && u.skipped.len() == 1);
        assert_eq!(std::fs::read_to_string(&p).unwrap(), "another app's");
    }

    /// A journal as 0.3–0.5 wrote it, without `kind`: tagging.
    #[test]
    fn an_old_journal_is_a_tagging_one() {
        let d = tempfile::tempdir().unwrap();
        let dir = d.path().join(DIR);
        std::fs::create_dir_all(&dir).unwrap();
        let json = r#"{
  "made": "2026-11-16T10:00:00Z",
  "entries": [
    {
      "photo": 7,
      "path": "C:\\nowhere\\IMG_7.xmp",
      "before": null,
      "after": "00",
      "prior": { "lat": null, "lon": null, "alt": null, "gps_source": null, "sidecar_name": null }
    }
  ]
}"#;
        std::fs::write(dir.join("20261116T100000Z.json"), json).unwrap();
        let j = Journal::load(&dir.join("20261116T100000Z.json")).unwrap();
        assert_eq!(j.kind, Kind::Tag);
        assert!(latest(d.path(), Kind::Tag).is_some());
        assert!(latest(d.path(), Kind::Remove).is_none());
    }

    #[test]
    fn a_cancelled_run_writes_nothing_more() {
        let d = tempfile::tempdir().unwrap();
        let t = Task {
            photo: 1,
            path: d.path().join("A.xmp"),
            gps: Some(gps(1.0)),
            prior: Prior::default(),
        };
        let r = run(
            Kind::Tag,
            &[t],
            &d.path().join("data"),
            &|_, _| {},
            &AtomicBool::new(true),
        );
        assert!(r.cancelled && r.written.is_empty() && r.journal.is_none());
        assert!(!d.path().join("A.xmp").exists());
    }
}

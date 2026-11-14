//! Browsing a folder for "Add photo folder": a background walk that lists every subfolder and
//! counts its photos, and the tree model the dialog draws from it (design.md §2.8(f)).
//!
//! The walk runs on its own thread: on the NAS a full listing takes about 50 s, and the UI
//! thread never touches the NAS (design.md §3.1). Photos are counted with the import's own
//! grouping (`scan::group`), so a RAW+JPEG pair counts once and the button's "Import N photos"
//! matches the summary afterwards.

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, Receiver};

use super::scan::{self, FileEntry};
use super::scope::{Check, Scope};
use crate::library::key;

/// One folder as the walk found it.
#[derive(Debug, Clone, PartialEq)]
pub struct FolderInfo {
    /// Relative path as on disk (`` for the root).
    pub rel: String,
    pub name: String,
    /// Keys of its subfolders, in name order.
    pub children: Vec<String>,
    /// Photos directly in it (not in subfolders).
    pub photos: usize,
    /// A folder MapCraft never imports (`.dtrash`, Lightroom backups): shown, not walked.
    pub never: bool,
    /// The folder couldn't be listed.
    pub unreadable: bool,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WalkEvent {
    Folder(FolderInfo),
    /// The walk finished (or was cancelled).
    Done,
}

/// A running walk; dropping it cancels the walk.
pub struct Walk {
    rx: Receiver<WalkEvent>,
    cancel: Arc<AtomicBool>,
}

impl Drop for Walk {
    fn drop(&mut self) {
        self.cancel.store(true, Ordering::Relaxed);
    }
}

impl Walk {
    /// Starts walking `root`; `on_event` is called from the walk thread after each event (the
    /// UI passes egui's repaint request).
    pub fn start(root: PathBuf, on_event: impl Fn() + Send + 'static) -> Self {
        let (tx, rx) = mpsc::channel();
        let cancel = Arc::new(AtomicBool::new(false));
        let flag = cancel.clone();
        std::thread::Builder::new()
            .name("mapcraft-browse".into())
            .spawn(move || {
                walk(&root, &flag, &mut |e| {
                    let _ = tx.send(e);
                    on_event();
                });
            })
            .expect("spawn the browse thread");
        Self { rx, cancel }
    }

    /// The events since the last call.
    pub fn poll(&self) -> Vec<WalkEvent> {
        self.rx.try_iter().collect()
    }
}

/// Walks `root` breadth-first, reporting every folder.
pub fn walk(root: &Path, cancel: &AtomicBool, emit: &mut dyn FnMut(WalkEvent)) {
    let mut todo = std::collections::VecDeque::from([String::new()]);
    while let Some(rel) = todo.pop_front() {
        if cancel.load(Ordering::Relaxed) {
            break;
        }
        let name = rel.rsplit('\\').next().unwrap_or("").to_owned();
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let mut info = FolderInfo {
            rel: rel.clone(),
            name,
            children: Vec::new(),
            photos: 0,
            never: false,
            unreadable: false,
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            info.unreadable = true;
            emit(WalkEvent::Folder(info));
            continue;
        };
        let mut files = Vec::new();
        let mut subdirs = Vec::new();
        for entry in entries.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let n = entry.file_name().to_string_lossy().into_owned();
            if meta.is_dir() {
                subdirs.push(n);
            } else {
                files.push((
                    rel.clone(),
                    FileEntry {
                        name: n,
                        size: meta.len(),
                        mtime: 0,
                    },
                ));
            }
        }
        subdirs.sort_by_key(|s| s.to_lowercase());
        info.photos = scan::group(files).len();
        for s in subdirs {
            let sub = if rel.is_empty() {
                s.clone()
            } else {
                format!("{rel}\\{s}")
            };
            info.children.push(key(&sub));
            if scan::is_excluded_dir(&s) {
                emit(WalkEvent::Folder(FolderInfo {
                    rel: sub,
                    name: s,
                    children: Vec::new(),
                    photos: 0,
                    never: true,
                    unreadable: false,
                }));
            } else {
                todo.push_back(sub);
            }
        }
        emit(WalkEvent::Folder(info));
    }
    emit(WalkEvent::Done);
}

/// The tree the dialog shows: what the walk found so far, which rows are expanded, and the
/// folder decisions being made.
#[derive(Debug, Clone, Default)]
pub struct Tree {
    pub folders: HashMap<String, FolderInfo>,
    pub expanded: HashSet<String>,
    pub scope: Scope,
    pub done: bool,
}

impl Tree {
    pub fn new(scope: Scope) -> Self {
        Self {
            expanded: HashSet::from([String::new()]),
            scope,
            ..Self::default()
        }
    }

    pub fn apply(&mut self, e: WalkEvent) {
        match e {
            WalkEvent::Folder(f) => {
                self.folders.insert(key(&f.rel), f);
            }
            WalkEvent::Done => self.done = true,
        }
    }

    pub fn root_unreadable(&self) -> bool {
        self.folders.get("").is_some_and(|f| f.unreadable)
    }

    /// Photos in a folder and everything below it; `None` while part of it is still unknown.
    pub fn total(&self, k: &str) -> Option<usize> {
        let f = self.folders.get(k)?;
        let mut sum = f.photos;
        for c in &f.children {
            sum += self.total(c)?;
        }
        Some(sum)
    }

    /// The checkbox state of a folder.
    pub fn check(&self, k: &str) -> Check {
        self.scope.state(k)
    }

    /// Ticks or unticks a folder and everything below it.
    pub fn toggle(&mut self, k: &str) {
        let on = self.scope.state(k) != Check::On;
        self.scope.set(k, on);
    }

    /// Photos and folders that would be imported, from the folders found so far.
    pub fn selected(&self) -> (usize, usize) {
        let mut photos = 0;
        let mut folders = 0;
        for (k, f) in &self.folders {
            if !f.never && !f.unreadable && self.scope.includes(k) && f.photos > 0 {
                photos += f.photos;
                folders += 1;
            }
        }
        (photos, folders)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree_of(root: &Path) -> Tree {
        let mut t = Tree::new(Scope::all());
        walk(root, &AtomicBool::new(false), &mut |e| t.apply(e));
        t
    }

    fn layout(root: &Path) {
        for (dir, files) in [
            ("", &["top.jpg"][..]),
            ("Trip", &["A.CR2", "A.jpg", "B.jpg", "notes.txt"][..]),
            (r"Trip\Day 2", &["C.jpg"][..]),
            ("Lightroom Catalog Backup", &["x.jpg"][..]),
        ] {
            fs::create_dir_all(root.join(dir)).unwrap();
            for f in files {
                fs::write(root.join(dir).join(f), b"x").unwrap();
            }
        }
    }

    #[test]
    fn counts_photos_like_the_import_and_never_walks_excluded_folders() {
        let dir = tempfile::tempdir().unwrap();
        layout(dir.path());
        let t = tree_of(dir.path());
        assert!(t.done);
        assert_eq!(t.folders["trip"].photos, 2, "A.CR2 + A.jpg is one photo");
        assert_eq!(t.total("trip"), Some(3));
        assert_eq!(t.total(""), Some(4), "the backup folder counts nothing");
        assert!(t.folders["lightroom catalog backup"].never);
        assert_eq!(t.selected(), (4, 3));
    }

    #[test]
    fn unticking_and_mixed_states() {
        let dir = tempfile::tempdir().unwrap();
        layout(dir.path());
        let mut t = tree_of(dir.path());
        t.toggle(r"trip\day 2");
        assert_eq!(t.check("trip"), Check::Mixed);
        assert_eq!(t.selected(), (3, 2));
        t.toggle("trip"); // mixed → on: everything below
        assert_eq!(t.check(r"trip\day 2"), Check::On);
        t.toggle("");
        assert_eq!(t.selected(), (0, 0), "unticking everything imports nothing");
    }

    #[test]
    fn totals_wait_for_the_whole_subtree() {
        let mut t = Tree::new(Scope::all());
        t.apply(WalkEvent::Folder(FolderInfo {
            rel: String::new(),
            name: String::new(),
            children: vec!["a".into()],
            photos: 5,
            never: false,
            unreadable: false,
        }));
        assert_eq!(t.total(""), None, "`a` not walked yet");
        assert_eq!(t.selected(), (5, 1));
    }

    #[test]
    fn an_unreadable_root_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let t = tree_of(&dir.path().join("missing"));
        assert!(t.root_unreadable() && t.done);
    }
}

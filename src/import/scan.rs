//! Listing a source and grouping its files into photos (design.md §2.1, §2.8(c), (e)).
//!
//! Everything here only reads directory listings. Sizes and times come from the directory
//! entries themselves, which on Windows arrive with the listing (no extra request per file).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use xxhash_rust::xxh3::Xxh3;

use super::scope::Scope;
use crate::library::key;

/// Files never imported, whatever their extension: macOS "AppleDouble" files (`._IMG_1.CR2`, a
/// Mac's metadata beside every file it touched on a share; magic `00 05 16 07`), found by the
/// hundred in the user's library (2026-11-14).
pub fn is_ignored_file(name: &str) -> bool {
    name.starts_with("._")
}

/// Folders never imported: other tools' trash and Lightroom's own data (design.md §2.8(e)).
pub fn is_excluded_dir(name: &str) -> bool {
    let n = name.to_lowercase();
    n == ".dtrash" || n == "lightroom catalog backup" || n.ends_with(".lrdata")
}

/// Image types MapCraft imports; RAW formats first in the rank list below.
const IMAGE_EXTS: [&str; 25] = [
    "dng", "cr2", "cr3", "crw", "nef", "nrw", "arw", "srf", "sr2", "raf", "orf", "rw2", "pef",
    "srw", "x3f", "3fr", "iiq", "tif", "tiff", "psd", "png", "heic", "heif", "jpg", "jpeg",
];

/// Which file of a name is "the original" (the file "open" refers to). Lower ranks win:
/// DNG first, since in this library DNGs are mostly the converted, edited working copies next to
/// a camera RAW; then camera RAWs; then TIFF/PSD; then PNG/HEIC; JPEG last.
/// **(unconfirmed, design.md §2.8(c))**
fn rank(ext: &str) -> u8 {
    match ext {
        "dng" => 0,
        "tif" | "tiff" | "psd" => 2,
        "png" | "heic" | "heif" => 3,
        "jpg" | "jpeg" => 4,
        _ => 1, // camera RAW
    }
}

fn is_jpeg(ext: &str) -> bool {
    ext == "jpg" || ext == "jpeg"
}

#[derive(Debug, Clone, PartialEq)]
pub struct FileEntry {
    pub name: String,
    pub size: u64,
    /// Milliseconds since 1970.
    pub mtime: i64,
}

#[derive(Debug, Default)]
pub struct Listing {
    /// Every folder read completely (included ones and those only passed through), by key
    /// (relative path, lower case), with the keys of the subfolder names it contained
    /// (excluded ones too).
    pub listed: HashMap<String, HashSet<String>>,
    /// The included folders among `listed`: their files were collected.
    pub collected: HashSet<String>,
    /// Folders that couldn't be read (a NAS hiccup, permissions).
    pub unreadable: usize,
    /// Files by folder (relative path as on disk), from included folders only.
    pub files: Vec<(String, FileEntry)>,
    /// The scope the listing followed.
    pub scope: Scope,
}

impl Listing {
    /// Can a known photo in folder `dir_key` that wasn't found be called missing (or moved)?
    ///
    /// Only if the folder is in scope. Then yes if its files were collected; otherwise the
    /// nearest completely read ancestor decides: if it no longer contains the folder, the folder
    /// was renamed or deleted, so yes. If the folder is still there but couldn't be read, no: the
    /// photo stays as it is.
    pub fn can_judge(&self, dir_key: &str) -> bool {
        if !self.scope.includes(dir_key) {
            return false;
        }
        if self.collected.contains(dir_key) {
            return true;
        }
        let mut child = dir_key;
        while let Some((parent, name)) = split_parent(child) {
            if let Some(subdirs) = self.listed.get(parent) {
                return !subdirs.contains(name);
            }
            child = parent;
        }
        false
    }
}

/// `a\b\c` → (`a\b`, `c`); `a` → (``, `a`); the root `` has no parent.
fn split_parent(dir: &str) -> Option<(&str, &str)> {
    if dir.is_empty() {
        None
    } else {
        Some(dir.rsplit_once('\\').unwrap_or(("", dir)))
    }
}

/// Lists `root` following `scope`: collects the files of included folders, and passes through
/// left-out folders only to reach included ones below them. Folders that are never imported
/// (`is_excluded_dir`) are skipped. Stops early, with a partial listing, when `cancelled`
/// returns true.
pub fn list(root: &Path, scope: &Scope, cancelled: &dyn Fn() -> bool) -> Listing {
    let mut out = Listing {
        scope: scope.clone(),
        ..Listing::default()
    };
    let mut todo: Vec<String> = if scope.reaches("") {
        vec![String::new()]
    } else {
        Vec::new()
    };
    while let Some(rel) = todo.pop() {
        if cancelled() {
            break;
        }
        let collect = scope.includes(&rel);
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            out.unreadable += 1;
            continue;
        };
        let mut complete = true;
        let mut subdirs = HashSet::new();
        for entry in entries {
            let Ok(entry) = entry else {
                complete = false;
                continue;
            };
            let name = entry.file_name().to_string_lossy().into_owned();
            let Ok(meta) = entry.metadata() else {
                complete = false;
                continue;
            };
            if meta.is_dir() {
                subdirs.insert(key(&name));
                let sub = if rel.is_empty() {
                    name.clone()
                } else {
                    format!("{rel}\\{name}")
                };
                if !is_excluded_dir(&name) && scope.reaches(&sub) {
                    todo.push(sub);
                }
            } else if collect {
                let mtime = meta
                    .modified()
                    .ok()
                    .and_then(|t| t.duration_since(UNIX_EPOCH).ok())
                    .map_or(0, |d| d.as_millis() as i64);
                out.files.push((
                    rel.clone(),
                    FileEntry {
                        name,
                        size: meta.len(),
                        mtime,
                    },
                ));
            }
        }
        if complete {
            if collect {
                out.collected.insert(key(&rel));
            }
            out.listed.insert(key(&rel), subdirs);
        } else {
            // A partly read folder doesn't count as listed: nothing in it becomes "missing".
            out.unreadable += 1;
        }
    }
    out
}

/// The files of one name in one folder: one photo.
#[derive(Debug, Clone, PartialEq)]
pub struct Group {
    pub rel_dir: String,
    pub stem: String,
    pub original: FileEntry,
    /// A JPEG beside a non-JPEG original: the fast thumbnail source.
    pub preview: Option<FileEntry>,
    /// Any other image files of the same name (e.g. the camera RAW beside a DNG).
    pub others: Vec<FileEntry>,
    pub sidecar: Option<FileEntry>,
}

impl Group {
    pub fn key(&self) -> (String, String) {
        (key(&self.rel_dir), key(&self.stem))
    }

    /// A hash over every member's name, size and time: unchanged files, unchanged signature.
    pub fn signature(&self) -> i64 {
        let mut h = Xxh3::new();
        let mut members: Vec<&FileEntry> = std::iter::once(&self.original)
            .chain(&self.preview)
            .chain(&self.others)
            .chain(&self.sidecar)
            .collect();
        members.sort_by(|a, b| a.name.cmp(&b.name));
        for m in members {
            h.update(m.name.to_lowercase().as_bytes());
            h.update(&m.size.to_le_bytes());
            h.update(&m.mtime.to_le_bytes());
        }
        h.digest() as i64
    }
}

fn split(name: &str) -> (&str, String) {
    match name.rsplit_once('.') {
        Some((stem, ext)) if !stem.is_empty() => (stem, ext.to_lowercase()),
        _ => (name, String::new()),
    }
}

/// Groups listed files into photos. Sidecars attach by `name.ext.xmp` (preferred, matching the
/// original's extension) or `name.xmp`; sidecars without an image are ignored, as are files that
/// aren't images (videos are left out of v1, design.md §2.8(d)).
pub fn group(files: Vec<(String, FileEntry)>) -> Vec<Group> {
    /// A photo's files while grouping: folder and name as on disk, and (extension, file) pairs.
    type Members = (String, String, Vec<(String, FileEntry)>);
    // (dir key, stem key) -> members
    let mut images: HashMap<(String, String), Members> = HashMap::new();
    let mut sidecars: Vec<(String, FileEntry)> = Vec::new();
    for (dir, f) in files {
        if is_ignored_file(&f.name) {
            continue;
        }
        let (stem, ext) = split(&f.name);
        if ext == "xmp" {
            sidecars.push((dir, f));
        } else if IMAGE_EXTS.contains(&ext.as_str()) {
            let stem = stem.to_owned();
            let entry = images
                .entry((key(&dir), key(&stem)))
                .or_insert_with(|| (dir.clone(), stem.clone(), Vec::new()));
            entry.2.push((ext, f));
        }
    }

    let mut groups: HashMap<(String, String), Group> = images
        .into_iter()
        .map(|(k, (dir, stem, mut members))| {
            members.sort_by_key(|(ext, f)| (rank(ext), f.name.to_lowercase()));
            let (orig_ext, original) = members.remove(0);
            let preview_at = if is_jpeg(&orig_ext) {
                None
            } else {
                members.iter().position(|(ext, _)| is_jpeg(ext))
            };
            let preview = preview_at.map(|i| members.remove(i).1);
            let group = Group {
                rel_dir: dir,
                stem,
                original,
                preview,
                others: members.into_iter().map(|(_, f)| f).collect(),
                sidecar: None,
            };
            (k, group)
        })
        .collect();

    for (dir, f) in sidecars {
        let (target, _) = sidecar_target(&f.name);
        if let Some(g) = groups.get_mut(&(key(&dir), key(&target))) {
            let better = match &g.sidecar {
                None => true,
                Some(current) => sidecar_rank(g, &f.name) < sidecar_rank(g, &current.name),
            };
            if better {
                g.sidecar = Some(f);
            }
        }
    }
    let mut out: Vec<Group> = groups.into_values().collect();
    out.sort_by_key(|a| a.key());
    out
}

/// The photo name a sidecar belongs to, and the image extension it names, if any:
/// `IMG_1.CR2.xmp` → (`IMG_1`, `cr2`); `IMG_1.xmp` → (`IMG_1`, ``).
fn sidecar_target(name: &str) -> (String, String) {
    let base = split(name).0; // `name.ext` or `name`
    let (inner_stem, inner_ext) = split(base);
    if IMAGE_EXTS.contains(&inner_ext.as_str()) {
        (inner_stem.to_owned(), inner_ext)
    } else {
        (base.to_owned(), String::new())
    }
}

/// Lower is better: `name.<original ext>.xmp`, then another `name.<image ext>.xmp`, then the
/// plain `name.xmp`.
fn sidecar_rank(g: &Group, sidecar: &str) -> u8 {
    let (_, ext) = sidecar_target(sidecar);
    if ext.is_empty() {
        2
    } else if ext == split(&g.original.name).1 {
        0
    } else {
        1
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn f(name: &str) -> FileEntry {
        FileEntry {
            name: name.into(),
            size: 10,
            mtime: 1,
        }
    }

    fn files(dir: &str, names: &[&str]) -> Vec<(String, FileEntry)> {
        names.iter().map(|n| (dir.to_string(), f(n))).collect()
    }

    #[test]
    fn raw_and_jpeg_pair_is_one_photo() {
        let g = group(files(
            "trip",
            &["DSCF0001.RAF", "DSCF0001.JPG", "DSCF0002.jpg", "clip.MOV"],
        ));
        assert_eq!(g.len(), 2);
        let pair = &g[0];
        assert_eq!(pair.original.name, "DSCF0001.RAF");
        assert_eq!(pair.preview.as_ref().unwrap().name, "DSCF0001.JPG");
        assert_eq!(g[1].original.name, "DSCF0002.jpg");
        assert!(g[1].preview.is_none());
    }

    #[test]
    fn dng_wins_over_the_camera_raw_beside_it() {
        let g = group(files("", &["IMG_1.CR2", "IMG_1.dng", "IMG_1.jpg"]));
        assert_eq!(g.len(), 1);
        assert_eq!(g[0].original.name, "IMG_1.dng");
        assert_eq!(g[0].preview.as_ref().unwrap().name, "IMG_1.jpg");
        assert_eq!(g[0].others, vec![f("IMG_1.CR2")]);
    }

    #[test]
    fn edited_copies_stay_separate_and_names_match_case_insensitively() {
        let g = group(files(
            "",
            &["IMG_4.JPG", "IMG_4_edit.JPG", "Ärger.jpg", "ärger.CR2"],
        ));
        assert_eq!(g.len(), 3);
        let a = g.iter().find(|g| g.original.name == "ärger.CR2").unwrap();
        assert_eq!(a.preview.as_ref().unwrap().name, "Ärger.jpg");
    }

    #[test]
    fn sidecars_both_naming_styles_and_orphans() {
        let g = group(files(
            "",
            &["A.CR2", "A.xmp", "B.dng", "B.dng.xmp", "C.xmp"],
        ));
        assert_eq!(g.len(), 2);
        assert_eq!(g[0].sidecar.as_ref().unwrap().name, "A.xmp");
        assert_eq!(g[1].sidecar.as_ref().unwrap().name, "B.dng.xmp");
    }

    #[test]
    fn the_specific_sidecar_wins_over_the_plain_one() {
        let g = group(files("", &["D.CR2", "D.xmp", "D.CR2.xmp"]));
        assert_eq!(g[0].sidecar.as_ref().unwrap().name, "D.CR2.xmp");
    }

    #[test]
    fn signature_changes_with_any_member() {
        let a = group(files("", &["X.CR2", "X.xmp"])).remove(0);
        let mut changed = files("", &["X.CR2", "X.xmp"]);
        changed[1].1.mtime = 2;
        let b = group(changed).remove(0);
        assert_ne!(a.signature(), b.signature());
        assert_eq!(
            a.signature(),
            group(files("", &["X.xmp", "X.CR2"])).remove(0).signature()
        );
    }

    #[test]
    fn listing_skips_excluded_folders_and_records_what_it_listed() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("2023").join("Lightroom Catalog Backup")).unwrap();
        fs::create_dir_all(root.join(".dtrash")).unwrap();
        fs::write(root.join("2023").join("a.jpg"), b"x").unwrap();
        fs::write(
            root.join("2023")
                .join("Lightroom Catalog Backup")
                .join("b.jpg"),
            b"x",
        )
        .unwrap();
        fs::write(root.join(".dtrash").join("c.jpg"), b"x").unwrap();
        let l = list(root, &Scope::all(), &|| false);
        assert_eq!(l.files.len(), 1);
        assert_eq!(l.files[0].0, "2023");
        assert!(l.collected.contains("") && l.collected.contains("2023"));
        assert!(
            l.listed["2023"].contains("lightroom catalog backup"),
            "excluded, but seen"
        );
        assert_eq!(l.unreadable, 0);
    }

    #[test]
    fn listing_passes_through_left_out_folders() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        fs::create_dir_all(root.join("Phone").join("2021")).unwrap();
        fs::create_dir_all(root.join("Other")).unwrap();
        fs::write(root.join("top.jpg"), b"x").unwrap();
        fs::write(root.join("Phone").join("p.jpg"), b"x").unwrap();
        fs::write(root.join("Phone").join("2021").join("q.jpg"), b"x").unwrap();
        fs::write(root.join("Other").join("o.jpg"), b"x").unwrap();
        // Only Phone\2021 is picked.
        let mut scope = Scope::all();
        scope.set("", false);
        scope.set(r"Phone\2021", true);
        let l = list(root, &scope, &|| false);
        let names: Vec<&str> = l.files.iter().map(|(_, f)| f.name.as_str()).collect();
        assert_eq!(names, vec!["q.jpg"]);
        assert!(l.listed.contains_key("phone"), "passed through");
        assert!(
            !l.collected.contains("phone"),
            "but its own files aren't imported"
        );
        assert!(!l.listed.contains_key("other"), "not entered at all");
        // Nothing picked: nothing listed.
        let mut none = Scope::all();
        none.set("", false);
        assert!(list(root, &none, &|| false).listed.is_empty());
    }

    fn listing(dirs: &[(&str, &[&str])], scope: Scope) -> Listing {
        Listing {
            collected: dirs
                .iter()
                .map(|(d, _)| d.to_string())
                .filter(|d| scope.includes(d))
                .collect(),
            listed: dirs
                .iter()
                .map(|(d, subs)| (d.to_string(), subs.iter().map(|s| s.to_string()).collect()))
                .collect(),
            scope,
            ..Listing::default()
        }
    }

    #[test]
    fn which_vanished_photos_can_be_judged() {
        // The root was listed and contains `kept` and `locked`; `locked` couldn't be read.
        let l = listing(&[("", &["kept", "locked"]), ("kept", &[])], Scope::all());
        assert!(l.can_judge("kept"), "listed itself");
        assert!(
            l.can_judge("renamed"),
            "gone from a listed parent: renamed or deleted"
        );
        assert!(
            l.can_judge(r"renamed\deeper"),
            "a whole subtree that's gone"
        );
        assert!(
            !l.can_judge("locked"),
            "still there but unreadable: untouched"
        );
        assert!(
            !l.can_judge(r"locked\sub"),
            "below an unreadable folder: untouched"
        );
        // Only `2023` is included; the root was passed through to reach it.
        let mut scope = Scope::all();
        scope.set("", false);
        scope.set("2023", true);
        let picked = listing(
            &[
                ("", &["2023", "2022"]),
                ("2023", &["may"]),
                (r"2023\may", &[]),
            ],
            scope,
        );
        assert!(
            !picked.can_judge(""),
            "the root's own files are out of scope"
        );
        assert!(
            !picked.can_judge("2022"),
            "left out: out of scope, never judged"
        );
        assert!(!picked.can_judge("2019"), "gone, but out of scope anyway");
        assert!(
            picked.can_judge(r"2023\june"),
            "gone inside an included folder"
        );
    }
}

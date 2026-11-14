//! Listing a source and grouping its files into photos (design.md §2.1, §2.8(c), (e)).
//!
//! Everything here only reads directory listings. Sizes and times come from the directory
//! entries themselves, which on Windows arrive with the listing (no extra request per file).

use std::collections::{HashMap, HashSet};
use std::fs;
use std::path::Path;
use std::time::UNIX_EPOCH;

use xxhash_rust::xxh3::Xxh3;

use crate::library::key;

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
    /// Every folder whose listing succeeded, by key (relative path, lower case). Only photos in
    /// these folders can be marked missing.
    pub listed: HashSet<String>,
    /// Folders that couldn't be read (a NAS hiccup, permissions).
    pub unreadable: usize,
    /// Files by folder (relative path as on disk).
    pub files: Vec<(String, FileEntry)>,
}

/// Lists `root` below each of `folders` (relative; empty = the whole root), skipping excluded
/// folders. Stops early, with a partial listing, when `cancelled` returns true.
pub fn list(root: &Path, folders: &[String], cancelled: &dyn Fn() -> bool) -> Listing {
    let mut out = Listing::default();
    let mut todo: Vec<String> = if folders.is_empty() {
        vec![String::new()]
    } else {
        folders.to_vec()
    };
    while let Some(rel) = todo.pop() {
        if cancelled() {
            break;
        }
        if !out.listed.insert(key(&rel)) {
            continue; // already listed (overlapping picks)
        }
        let dir = if rel.is_empty() {
            root.to_path_buf()
        } else {
            root.join(&rel)
        };
        let Ok(entries) = fs::read_dir(&dir) else {
            out.listed.remove(&key(&rel));
            out.unreadable += 1;
            continue;
        };
        let mut complete = true;
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
                if !is_excluded_dir(&name) {
                    todo.push(if rel.is_empty() {
                        name
                    } else {
                        format!("{rel}\\{name}")
                    });
                }
            } else {
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
        if !complete {
            // A partly read folder doesn't count as listed: nothing in it becomes "missing".
            out.listed.remove(&key(&rel));
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
        let l = list(root, &[], &|| false);
        assert_eq!(l.files.len(), 1);
        assert_eq!(l.files[0].0, "2023");
        assert!(l.listed.contains("") && l.listed.contains("2023"));
        assert_eq!(l.unreadable, 0);
        let picked = list(root, &["2023".into(), "nope".into()], &|| false);
        assert_eq!(picked.files.len(), 1);
        assert_eq!(picked.unreadable, 1);
        assert!(!picked.listed.contains("")); // the root wasn't picked
    }
}

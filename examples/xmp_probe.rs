//! Dev probe (M2): MapCraft's XMP GPS reader against a plain text search, on real sidecars.
//! Usage: cargo run --release --example xmp_probe -- <folder>
//! Read-only; prints counts only.

use std::path::Path;

fn walk(dir: &Path, out: &mut Vec<std::path::PathBuf>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for e in entries.flatten() {
        let p = e.path();
        let name = e.file_name().to_string_lossy().to_lowercase();
        if p.is_dir() {
            if name != ".dtrash" && name != "lightroom catalog backup" && !name.ends_with(".lrdata")
            {
                walk(&p, out);
            }
        } else if name.ends_with(".xmp") {
            out.push(p);
        }
    }
}

fn main() {
    let root = std::env::args().nth(1).expect("folder");
    let mut files = Vec::new();
    walk(Path::new(&root), &mut files);
    let (mut text_has, mut parsed_gps, mut errors, mut missed) = (0, 0, 0, 0);
    for f in &files {
        let Ok(xml) = std::fs::read_to_string(f) else {
            errors += 1;
            continue;
        };
        let has = xml.contains("GPSLatitude");
        text_has += has as u32;
        match mapcraft::metadata::xmp::read_gps(&xml) {
            Ok(Some(_)) => parsed_gps += 1,
            Ok(None) => missed += has as u32,
            Err(_) => errors += 1,
        }
    }
    println!(
        "sidecars {} | mention GPSLatitude {text_has} | position read {parsed_gps} | \
         mention but no position {missed} | errors {errors}",
        files.len()
    );
}

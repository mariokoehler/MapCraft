//! Dev probe (M11): what removing positions would do to real sidecars, in memory only.
//! Usage: cargo run --release --example strip_probe -- <folder>
//! Read-only (nothing is written); prints counts only, never names or positions.

use std::collections::BTreeMap;
use std::path::Path;

fn main() {
    let root = std::env::args().nth(1).expect("folder");
    let mut counts: BTreeMap<String, u32> = BTreeMap::new();
    let mut add = |k: &str| *counts.entry(k.to_owned()).or_default() += 1;
    let entries = std::fs::read_dir(Path::new(&root)).expect("readable folder");
    for e in entries.flatten() {
        let name = e.file_name().to_string_lossy().to_lowercase();
        if !name.ends_with(".xmp") {
            continue;
        }
        let Ok(bytes) = std::fs::read(e.path()) else {
            add("unreadable file");
            continue;
        };
        let text = String::from_utf8_lossy(&bytes);
        let had = matches!(
            mapcraft::metadata::xmp::read_gps(text.trim_start_matches('\u{feff}')),
            Ok(Some(_))
        );
        add(if had {
            "with a position"
        } else {
            "without a position"
        });
        if !had {
            continue;
        }
        match mapcraft::geotag::sidecar::stripped(&bytes) {
            Ok(None) => add("  → would be deleted (nothing else in it)"),
            Ok(Some(_)) => add("  → would keep its other content"),
            Err(why) => add(&format!("  → refused: {why}")),
        }
    }
    for (k, n) in counts {
        println!("{n:>6}  {k}");
    }
}

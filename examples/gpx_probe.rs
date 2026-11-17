//! Reads track files (GPX or NMEA logs) as tagging does and prints aggregates only (points, times, length, pauses):
//! never names or coordinates. `cargo run --release --example gpx_probe -- <file.gpx>…`

use mapcraft::geotag::{DEFAULT_MAX_GAP, Track, read_track, time};

fn main() {
    let mut files = Vec::new();
    for path in std::env::args().skip(1) {
        let text = match std::fs::read(&path) {
            Ok(t) => t,
            Err(e) => {
                eprintln!("can't read a file: {e}");
                std::process::exit(1);
            }
        };
        match read_track(&text) {
            Ok(g) => files.push((g, String::new())),
            Err(e) => {
                eprintln!("not readable as a track: {e}");
                std::process::exit(1);
            }
        }
    }
    let t = Track::new(files);
    println!(
        "{} file(s), {} points, {} drawn lines, {} timed segments, {} points without a time dropped",
        t.files,
        t.points,
        t.lines.len(),
        t.segments.len(),
        t.dropped
    );
    println!("length {:.1} km", t.length_m / 1000.0);
    match t.span() {
        Some((a, b)) => {
            println!(
                "{} {} – {} UTC, {:.2} h",
                time::day(a),
                time::clock(a),
                time::clock(b),
                (b - a) / 3600.0
            );
            let pauses = t.pauses(DEFAULT_MAX_GAP);
            println!(
                "{} pause(s): {:?} min",
                pauses.len(),
                pauses
                    .iter()
                    .map(|p| ((p.to - p.from) / 60.0).round())
                    .collect::<Vec<_>>()
            );
            println!(
                "this PC's offset then: {:?}",
                time::local_offset_at(a).map(time::zone_text)
            );
        }
        None => println!("no times: nothing can be matched"),
    }
}

//! Dev probe (M2): MapCraft's own metadata reader against a whole-file `kamadak-exif` read.
//! Usage: cargo run --release --example exif_probe -- <list.txt>
//! Reads each listed file (read-only) and prints aggregates only: no names, paths or positions.

use std::collections::BTreeMap;
use std::fs::File;
use std::io::BufReader;
use std::path::Path;
use std::time::Instant;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let list = std::fs::read_to_string(&args[1]).expect("list");
    // ext -> [files, ours ok, ours gps, ref ok, ref gps, gps agree, taken_at agree, ours ms, ref ms]
    let mut stats: BTreeMap<String, [u64; 9]> = BTreeMap::new();
    let mut worst = 0.0f64;
    for path in list.lines().filter(|l| !l.trim().is_empty()) {
        let ext = path.rsplit('.').next().unwrap_or("").to_lowercase();
        let s = stats.entry(ext.clone()).or_default();
        s[0] += 1;
        let t = Instant::now();
        let ours = mapcraft::metadata::read_image(Path::new(path));
        s[7] += t.elapsed().as_millis() as u64;
        let t = Instant::now();
        // The reference: kamadak-exif on the whole file (RAF unsupported there).
        let reference = File::open(path).ok().and_then(|f| {
            exif::Reader::new()
                .read_from_container(&mut BufReader::new(f))
                .ok()
        });
        s[8] += t.elapsed().as_millis() as u64;
        let ref_gps = reference.as_ref().and_then(|e| {
            let deg = |tag| {
                e.get_field(tag, exif::In::PRIMARY)
                    .and_then(|f| match &f.value {
                        exif::Value::Rational(v) if v.len() == 3 => {
                            Some(v[0].to_f64() + v[1].to_f64() / 60.0 + v[2].to_f64() / 3600.0)
                        }
                        _ => None,
                    })
            };
            let r = |tag| {
                e.get_field(tag, exif::In::PRIMARY)
                    .map(|f| f.display_value().to_string())
            };
            let lat = deg(exif::Tag::GPSLatitude)?;
            let lon = deg(exif::Tag::GPSLongitude)?;
            let lat = if r(exif::Tag::GPSLatitudeRef).as_deref() == Some("S") {
                -lat
            } else {
                lat
            };
            let lon = if r(exif::Tag::GPSLongitudeRef).as_deref() == Some("W") {
                -lon
            } else {
                lon
            };
            mapcraft::metadata::Gps::new(lat, lon, None)
        });
        let ref_date = reference.as_ref().and_then(|e| {
            e.get_field(exif::Tag::DateTimeOriginal, exif::In::PRIMARY)
                .map(|f| f.display_value().to_string())
        });
        if let Ok(m) = &ours {
            s[1] += 1;
            s[2] += m.gps.is_some() as u64;
        }
        if reference.is_some() {
            s[3] += 1;
            s[4] += ref_gps.is_some() as u64;
        }
        let our_gps = ours.as_ref().ok().and_then(|m| m.gps);
        let agree = match (our_gps, ref_gps) {
            (Some(a), Some(b)) => {
                let d = (a.lat - b.lat).abs().max((a.lon - b.lon).abs());
                worst = worst.max(d);
                d < 1e-7
            }
            (None, None) => true,
            (Some(_), None) => reference.is_none(), // the reference can't read it (RAF)
            (None, Some(_)) => false,
        };
        s[5] += agree as u64;
        // kamadak shows dates as "2023-05-19 21:06:14"; ours as "2023-05-19T21:06:14".
        let our_date = ours.as_ref().ok().and_then(|m| m.taken_at.clone());
        let date_agree = match (&our_date, &ref_date) {
            (Some(a), Some(b)) => a.replace('T', " ") == *b,
            (None, None) => true,
            (Some(_), None) => reference.is_none(),
            (None, Some(_)) => false,
        };
        s[6] += date_agree as u64;
    }
    println!("type   files oursOK oursGPS refOK refGPS gpsAgree dateAgree ours-ms ref-ms");
    for (ext, s) in stats {
        let n = s[0].max(1);
        println!(
            "{ext:6} {:5} {:6} {:7} {:5} {:6} {:8} {:9} {:7} {:6}",
            s[0],
            s[1],
            s[2],
            s[3],
            s[4],
            s[5],
            s[6],
            s[7] / n,
            s[8] / n
        );
    }
    println!("largest position difference where both have one: {worst:e} degrees");
}

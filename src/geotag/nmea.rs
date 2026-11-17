//! NMEA 0183 logs (design.md §2.9(b)): the text a GPS logger writes, one `$…*hh` sentence per
//! line, as Sony's GPS-CS1 (`@Sonygps/ver1.0/wgs-84`) and many others do. Field layout from
//! https://gpsd.gitlab.io/gpsd/NMEA.html (checked 2026-11-17).
//!
//! Positions and times come from `RMC` (time, status, latitude, longitude, date); `GGA` adds the
//! altitude for the same time of day. Any talker (`GP`, `GN`, `GL`, …) is read. A log is one
//! segment: the logger's gaps become pauses or breaks in the matching (`Track`).

use std::collections::HashMap;

use super::gpx::{Gpx, Point};
use super::time;

/// One sentence's fields after the `$` (the first is the address, `GPRMC`), or `None` for a
/// line that isn't a sentence or whose checksum is wrong. A sentence without a checksum is read.
fn sentence(line: &str) -> Option<Vec<&str>> {
    let start = line.find('$')?;
    let s = line[start + 1..].trim_end();
    let body = match s.split_once('*') {
        Some((body, sum)) => {
            let want = u8::from_str_radix(sum.get(..2)?, 16).ok()?;
            if body.bytes().fold(0u8, |a, b| a ^ b) != want {
                return None;
            }
            body
        }
        None => s,
    };
    Some(body.split(',').collect())
}

/// `hhmmss[.ss]` → seconds of the day.
fn time_of_day(s: &str) -> Option<f64> {
    let d = |r: std::ops::Range<usize>| -> Option<u32> {
        let t = s.get(r)?;
        t.bytes()
            .all(|c| c.is_ascii_digit())
            .then(|| t.parse().ok())?
    };
    let (h, m, sec) = (d(0..2)?, d(2..4)?, d(4..6)?);
    if h > 23 || m > 59 || sec > 60 {
        return None;
    }
    let frac = match s.get(6..) {
        Some("") | None => 0.0,
        Some(r) => format!("0{r}").parse().ok()?,
    };
    Some(f64::from(h * 3600 + m * 60 + sec) + frac)
}

/// `ddmmyy` → days since 1970. The year has two digits: 80–99 are 1980–1999 (GPS started in
/// 1980), 00–79 are 2000–2079.
fn date(s: &str) -> Option<i64> {
    if s.len() != 6 || !s.bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let (d, m, y): (u32, u32, i64) = (
        s[0..2].parse().ok()?,
        s[2..4].parse().ok()?,
        s[4..6].parse().ok()?,
    );
    if !(1..=12).contains(&m) || !(1..=31).contains(&d) {
        return None;
    }
    let y = if y >= 80 { 1900 + y } else { 2000 + y };
    Some(time::days_from_civil(y, m, d))
}

/// `ddmm.mmmm` (or `dddmm.mmmm`) and its hemisphere → signed degrees. The degrees are the
/// digits before the minutes' two, wherever the decimal point is.
fn coordinate(v: &str, hemi: &str, max: f64) -> Option<f64> {
    let int_len = v.find('.').unwrap_or(v.len());
    if int_len < 3 || !v[..int_len].bytes().all(|c| c.is_ascii_digit()) {
        return None;
    }
    let deg: f64 = v[..int_len - 2].parse().ok()?;
    let min: f64 = v[int_len - 2..].parse().ok()?;
    if min >= 60.0 {
        return None;
    }
    let x = deg + min / 60.0;
    let x = match hemi {
        "N" | "E" => x,
        "S" | "W" => -x,
        _ => return None,
    };
    (x.abs() <= max).then_some(x)
}

/// The position in fields `i..i+4` (latitude, N/S, longitude, E/W).
fn position(f: &[&str], i: usize) -> Option<(f64, f64)> {
    Some((
        coordinate(f.get(i)?, f.get(i + 1)?, 90.0)?,
        coordinate(f.get(i + 2)?, f.get(i + 3)?, 180.0)?,
    ))
}

/// The key that pairs a `GGA` with the `RMC` of the same fix: its time of day in 10 ms.
fn key(tod: f64) -> i64 {
    (tod * 100.0).round() as i64
}

/// Reads an NMEA log. Fails only when the text holds no NMEA sentence at all.
pub fn parse(text: &str) -> Result<Gpx, String> {
    let mut sentences = 0usize;
    let mut rmc: Vec<(f64, Point)> = Vec::new(); // (time of day, point with its UTC time)
    let mut gga: Vec<Point> = Vec::new(); // in file order, without a time (GGA has no date)
    let mut ele: HashMap<i64, f64> = HashMap::new();
    for line in text.lines() {
        let Some(f) = sentence(line) else { continue };
        let kind = f[0].get(2..).filter(|_| f[0].len() == 5);
        sentences += usize::from(kind.is_some());
        match kind {
            Some("RMC") => {
                // 1 time, 2 status (A valid, V warning), 3–6 position, 9 date
                if f.get(2) != Some(&"A") {
                    continue;
                }
                let (Some(tod), Some((lat, lon)), Some(day)) = (
                    f.get(1).and_then(|s| time_of_day(s)),
                    position(&f, 3),
                    f.get(9).and_then(|s| date(s)),
                ) else {
                    continue;
                };
                let t = day as f64 * 86_400.0 + tod;
                rmc.push((
                    tod,
                    Point {
                        lat,
                        lon,
                        ele: None,
                        t: Some(t),
                    },
                ));
            }
            Some("GGA") => {
                // 1 time, 2–5 position, 6 quality (0 = no fix), 9 altitude (m)
                if matches!(f.get(6), None | Some(&"0") | Some(&"")) {
                    continue;
                }
                let Some((lat, lon)) = position(&f, 2) else {
                    continue;
                };
                let alt = f.get(9).and_then(|s| s.parse::<f64>().ok());
                if let (Some(tod), Some(a)) = (f.get(1).and_then(|s| time_of_day(s)), alt) {
                    ele.insert(key(tod), a);
                }
                gga.push(Point {
                    lat,
                    lon,
                    ele: alt,
                    t: None,
                });
            }
            _ => {}
        }
    }
    if sentences == 0 {
        return Err("neither a GPX file nor an NMEA log".to_owned());
    }
    let points: Vec<Point> = if rmc.is_empty() {
        // Only GGA: positions without a date can't be matched, but the track is still drawn.
        gga
    } else {
        rmc.into_iter()
            .map(|(tod, p)| Point {
                ele: ele.get(&key(tod)).copied(),
                ..p
            })
            .collect()
    };
    let mut g = Gpx::default();
    if !points.is_empty() {
        g.segments.push(points);
    }
    Ok(g)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A sentence with its checksum, as a logger writes it.
    fn line(body: &str) -> String {
        let sum = body.bytes().fold(0u8, |a, b| a ^ b);
        format!("${body}*{sum:02X}\r\n")
    }

    fn utc(s: &str) -> f64 {
        time::parse(s).unwrap().0
    }

    #[test]
    fn checksums() {
        assert!(sentence("$GPVTG,136.5,T,,M,000.0,N,000.0,K,A*0C").is_some());
        assert!(sentence("$GPVTG,136.5,T,,M,000.0,N,000.0,K,A*0D").is_none());
        assert!(sentence("$GPVTG,136.5,T").is_some(), "no checksum is read");
        assert!(sentence("@Sonygps/ver1.0/wgs-84").is_none());
    }

    #[test]
    fn a_sony_style_log() {
        // GGA before RMC (as Sony writes it), a warning fix, a bad checksum, GN talker, RMC
        // before GGA, the southern and western hemispheres, and midnight.
        let mut log = String::from("@Sonygps/ver1.0/wgs-84\r\n");
        log += &line("GPGGA,235950,4807.0380,N,01131.0000,E,1,05,01.5,00519.5,M,045.3,M,,");
        log += &line("GPGSA,A,3,02,13,20,23,25,,,,,,,,03.0,01.5,02.5");
        log += &line("GPRMC,235950,A,4807.0380,N,01131.0000,E,000.0,136.5,311208,,,A");
        log += &line("GPRMC,235955,V,4807.0400,N,01131.0010,E,000.0,136.5,311208,,,N");
        // A line changed after its checksum was computed.
        log += &line("GPRMC,235958,A,4807.0400,N,01131.0010,E,000.0,136.5,311208,,,A")
            .replace("235958", "235959");
        log += &line("GNRMC,000005.50,A,3352.1280,S,15112.5600,W,1.0,90.0,010109,,,A");
        log += &line("GNGGA,000005.50,3352.1280,S,15112.5600,W,1,07,1.0,-0012.0,M,,M,,");
        log += &line("GPGGA,000010,3352.1300,S,15112.5600,W,0,00,,,M,,M,,");
        let g = parse(&log).unwrap();
        assert_eq!(g.name, None);
        assert_eq!(g.segments.len(), 1);
        let p = &g.segments[0];
        assert_eq!(p.len(), 2, "the V fix and the bad checksum are skipped");
        assert!((p[0].lat - (48.0 + 7.038 / 60.0)).abs() < 1e-9);
        assert!((p[0].lon - (11.0 + 31.0 / 60.0)).abs() < 1e-9);
        assert_eq!(p[0].ele, Some(519.5), "GGA written before its RMC");
        assert_eq!(p[0].t, Some(utc("2008-12-31T23:59:50Z")));
        assert!((p[1].lat + (33.0 + 52.128 / 60.0)).abs() < 1e-9);
        assert!((p[1].lon + (151.0 + 12.56 / 60.0)).abs() < 1e-9);
        assert_eq!(p[1].ele, Some(-12.0), "GGA written after its RMC");
        assert_eq!(p[1].t, Some(utc("2009-01-01T00:00:05.5Z")));
    }

    #[test]
    fn only_gga_has_no_times() {
        let log = line("GPGGA,120000,4807.0380,N,01131.0000,E,1,05,1.5,519.5,M,,M,,")
            + &line("GPGGA,120015,4807.0480,N,01131.0100,E,1,05,1.5,520.0,M,,M,,");
        let g = parse(&log).unwrap();
        assert_eq!(g.points(), 2);
        assert!(g.segments[0].iter().all(|p| p.t.is_none()));
    }

    #[test]
    fn fields() {
        assert_eq!(date("010180"), Some(time::days_from_civil(1980, 1, 1)));
        assert_eq!(date("311279"), Some(time::days_from_civil(2079, 12, 31)));
        assert_eq!(date("321208"), None);
        assert_eq!(time_of_day("151311"), Some(54_791.0));
        assert_eq!(time_of_day("151311.25"), Some(54_791.25));
        assert_eq!(time_of_day("2513"), None);
        assert_eq!(coordinate("0000.0000", "N", 90.0), Some(0.0));
        assert_eq!(coordinate("9100.0000", "N", 90.0), None);
        assert_eq!(coordinate("4807.0380", "X", 90.0), None);
        assert_eq!(coordinate("12", "N", 90.0), None);
    }

    #[test]
    fn told_apart_by_content() {
        let gpx = "\u{FEFF}\n <gpx><trk><trkseg><trkpt lat=\"1\" lon=\"2\"/></trkseg></trk></gpx>";
        assert_eq!(
            super::super::read_track(gpx.as_bytes()).unwrap().points(),
            1
        );
        let log = line("GPRMC,120000,A,4807.0380,N,01131.0000,E,0,0,010109,,,A");
        assert_eq!(
            super::super::read_track(log.as_bytes()).unwrap().points(),
            1
        );
        let err = super::super::read_track(b"4807.0380,N,01131.0000,E").unwrap_err();
        assert!(!err.contains("4807"), "an error never quotes the file");
    }

    #[test]
    fn not_nmea() {
        assert!(parse("hello\nworld").is_err());
        assert!(parse("").is_err());
        let empty = parse(&line("GPRMC,120000,V,,,,,,,010109,,,N")).unwrap();
        assert_eq!(empty.points(), 0);
    }
}

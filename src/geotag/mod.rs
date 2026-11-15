//! Geotagging from GPX tracks (design.md §2.9, M9): read tracks, match photos by capture time,
//! and write the positions to XMP sidecars. This module is the part without UI: the track and
//! its matching (here), GPX (`gpx`), times (`time`) and the sidecars (`sidecar`).

pub mod gpx;
pub mod journal;
pub mod sidecar;
pub mod time;

use std::collections::HashSet;

pub use gpx::Point;

/// A photo is placed at a segment's end if its time is at most this far before or after it.
pub const END_SLACK: f64 = 120.0;
/// A gap longer than the max gap whose ends are this close is a pause, not a hole.
pub const PAUSE_RADIUS_M: f64 = 50.0;
/// The max gap by default, as LightCraft's `maxGap`.
pub const DEFAULT_MAX_GAP: f64 = 600.0;
/// The margins the panel offers, in hours, and the default.
pub const MARGINS: [u32; 4] = [1, 3, 6, 12];
pub const DEFAULT_MARGIN: u32 = 3;
/// The widest shift: the candidate pool covers it, so photos can slide in.
pub const MAX_SHIFT: i64 = 14 * 3600;

/// Distance in metres between two positions (haversine).
pub fn distance_m(a: (f64, f64), b: (f64, f64)) -> f64 {
    let r = 6_371_000.0;
    let (f1, f2) = (a.0.to_radians(), b.0.to_radians());
    let (df, dl) = ((b.0 - a.0).to_radians(), (b.1 - a.1).to_radians());
    let h = (df / 2.0).sin().powi(2) + f1.cos() * f2.cos() * (dl / 2.0).sin().powi(2);
    2.0 * r * h.sqrt().asin()
}

/// A pause: recording stopped (or the walker stood still) for longer than the max gap.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Pause {
    pub lat: f64,
    pub lon: f64,
    pub from: f64,
    pub to: f64,
}

/// One or more GPX files as one timeline (design.md §2.9(a), (b)).
#[derive(Debug, Clone, Default)]
pub struct Track {
    /// The first file's name (its `<name>`, else the file name), and how many files.
    pub name: String,
    pub files: usize,
    /// Every segment as drawn, times or not.
    pub lines: Vec<Vec<(f64, f64)>>,
    /// The timed segments, each sorted by time, sorted by start.
    pub segments: Vec<Vec<Point>>,
    /// Points dropped from a timed track because they had no time.
    pub dropped: usize,
    pub points: usize,
    pub length_m: f64,
}

impl Track {
    /// `files`: each GPX with the name to use when it has none of its own.
    pub fn new(files: Vec<(gpx::Gpx, String)>) -> Self {
        let mut t = Track {
            files: files.len(),
            ..Default::default()
        };
        for (g, fallback) in files {
            if t.name.is_empty() {
                t.name = g.name.clone().unwrap_or(fallback);
            }
            for seg in g.segments {
                t.points += seg.len();
                t.length_m += seg
                    .windows(2)
                    .map(|w| distance_m((w[0].lat, w[0].lon), (w[1].lat, w[1].lon)))
                    .sum::<f64>();
                t.lines.push(seg.iter().map(|p| (p.lat, p.lon)).collect());
                let mut timed: Vec<Point> = seg.iter().copied().filter(|p| p.t.is_some()).collect();
                t.dropped += seg.len() - timed.len();
                timed.sort_by(|a, b| a.t.unwrap_or(0.0).total_cmp(&b.t.unwrap_or(0.0)));
                if !timed.is_empty() {
                    t.segments.push(timed);
                }
            }
        }
        if t.segments.is_empty() {
            t.dropped = 0; // a route without times: nothing was dropped, nothing matches
        }
        t.segments
            .sort_by(|a, b| a[0].t.unwrap_or(0.0).total_cmp(&b[0].t.unwrap_or(0.0)));
        t
    }

    pub fn has_times(&self) -> bool {
        !self.segments.is_empty()
    }

    /// The first and last time, UTC.
    pub fn span(&self) -> Option<(f64, f64)> {
        let first = self.segments.first()?.first()?.t?;
        let last = self
            .segments
            .iter()
            .filter_map(|s| s.last()?.t)
            .fold(first, f64::max);
        Some((first, last))
    }

    /// The pauses: gaps longer than `max_gap` whose ends are within `PAUSE_RADIUS_M`.
    pub fn pauses(&self, max_gap: f64) -> Vec<Pause> {
        let mut out = Vec::new();
        for s in &self.segments {
            for w in s.windows(2) {
                let (a, b) = (w[0], w[1]);
                let dt = b.t.unwrap_or(0.0) - a.t.unwrap_or(0.0);
                if dt > max_gap && distance_m((a.lat, a.lon), (b.lat, b.lon)) <= PAUSE_RADIUS_M {
                    out.push(Pause {
                        lat: a.lat,
                        lon: a.lon,
                        from: a.t.unwrap_or(0.0),
                        to: b.t.unwrap_or(0.0),
                    });
                }
            }
        }
        out
    }

    /// Where the track was at UTC `t` (design.md §2.9(d)).
    pub fn locate(&self, t: f64, max_gap: f64) -> Place {
        let Some((first, last)) = self.span() else {
            return Place::Outside(Outside::NoTimes);
        };
        for s in &self.segments {
            let (a, b) = (s[0].t.unwrap_or(0.0), s[s.len() - 1].t.unwrap_or(0.0));
            if t < a - END_SLACK || t > b + END_SLACK {
                continue;
            }
            if t <= a {
                return Place::on(s[0], How::End);
            }
            if t >= b {
                return Place::on(s[s.len() - 1], How::End);
            }
            // The last point at or before t.
            let i = s.partition_point(|p| p.t.unwrap_or(0.0) <= t) - 1;
            let (p, q) = (s[i], s[i + 1]);
            let (tp, tq) = (p.t.unwrap_or(0.0), q.t.unwrap_or(0.0));
            let dt = tq - tp;
            if dt <= max_gap {
                let f = if dt > 0.0 { (t - tp) / dt } else { 0.0 };
                let ele = match (p.ele, q.ele) {
                    (Some(e), Some(g)) => Some(e + (g - e) * f),
                    (e, g) => e.or(g),
                };
                return Place::On {
                    lat: p.lat + (q.lat - p.lat) * f,
                    lon: p.lon + (q.lon - p.lon) * f,
                    ele,
                    how: How::Interpolated,
                };
            }
            if distance_m((p.lat, p.lon), (q.lat, q.lon)) <= PAUSE_RADIUS_M {
                return Place::on(p, How::Pause);
            }
            return Place::Outside(Outside::Gap(dt));
        }
        if t < first {
            Place::Outside(Outside::Before(first - t))
        } else if t > last {
            Place::Outside(Outside::After(t - last))
        } else {
            Place::Outside(Outside::Gap(0.0)) // between two segments
        }
    }

    /// The time at the track point nearest to a position (for "taken here", design.md §2.9(f)),
    /// interpolated along the nearest leg. `None` without times.
    pub fn time_near(&self, lat: f64, lon: f64) -> Option<(f64, (f64, f64))> {
        let mut best: Option<(f64, f64, (f64, f64))> = None; // (distance², time, spot)
        let k = lat.to_radians().cos();
        for s in &self.segments {
            for w in s.windows(2).chain(std::iter::once(&s[s.len() - 1..])) {
                let p = w[0];
                let q = *w.last().unwrap_or(&p);
                // Project onto the leg in a local flat frame.
                let (px, py) = ((p.lon - lon) * k, p.lat - lat);
                let (qx, qy) = ((q.lon - lon) * k, q.lat - lat);
                let (dx, dy) = (qx - px, qy - py);
                let len2 = dx * dx + dy * dy;
                let f = if len2 > 0.0 {
                    (-(px * dx + py * dy) / len2).clamp(0.0, 1.0)
                } else {
                    0.0
                };
                let (x, y) = (px + dx * f, py + dy * f);
                let d2 = x * x + y * y;
                if best.is_none_or(|b| d2 < b.0) {
                    let (tp, tq) = (p.t.unwrap_or(0.0), q.t.unwrap_or(0.0));
                    let spot = (p.lat + (q.lat - p.lat) * f, p.lon + (q.lon - p.lon) * f);
                    best = Some((d2, tp + (tq - tp) * f, spot));
                }
            }
        }
        best.map(|b| (b.1, b.2))
    }
}

/// How a photo got its place on the track.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum How {
    Interpolated,
    /// In a pause: the spot where it began.
    Pause,
    /// Just before the start or after the end of a segment: that end.
    End,
}

/// Why a photo isn't on the track.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Outside {
    /// Seconds before the first point.
    Before(f64),
    /// Seconds after the last point.
    After(f64),
    /// In a gap longer than the max gap (its length), or between two segments (0).
    Gap(f64),
    NoTimes,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Place {
    On {
        lat: f64,
        lon: f64,
        ele: Option<f64>,
        how: How,
    },
    Outside(Outside),
}

impl Place {
    fn on(p: Point, how: How) -> Self {
        Place::On {
            lat: p.lat,
            lon: p.lon,
            ele: p.ele,
            how,
        }
    }
}

/// A photo that may be tagged: what the library knows about it.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub id: i64,
    /// The capture time as the camera wrote it, as if UTC.
    pub clock: f64,
    /// Its own UTC offset (EXIF `OffsetTimeOriginal`), when it has one.
    pub offset: Option<i64>,
    /// Its current position (only when re-tagging photos that have one).
    pub current: Option<(f64, f64)>,
}

/// The settings of one tagging session (design.md §2.9(e), (g)).
#[derive(Debug, Clone, PartialEq)]
pub struct Shift {
    /// The photos' time zone, seconds east of UTC.
    pub zone: i64,
    /// The camera clock correction, seconds.
    pub correction: i64,
    /// Apply the zone to photos that have their own offset too.
    pub ignore_own: bool,
}

impl Shift {
    /// The shift for one photo: its own offset (plus the correction) unless told to ignore it.
    pub fn for_photo(&self, c: &Candidate) -> i64 {
        match c.offset {
            Some(o) if !self.ignore_own => o + self.correction,
            _ => self.zone + self.correction,
        }
    }

    /// The photo's time in UTC.
    pub fn utc(&self, c: &Candidate) -> f64 {
        c.clock - self.for_photo(c) as f64
    }

    pub fn total(&self) -> i64 {
        self.zone + self.correction
    }

    /// "This photo was taken where the track was at UTC `t`" (design.md §2.9(f)). A photo with
    /// its own offset sets only the correction; otherwise the zone stays if the correction then
    /// fits ±30 minutes, else the zone is the nearest whole hour.
    pub fn anchored(&self, c: &Candidate, t: f64) -> Shift {
        let total = (c.clock - t).round() as i64;
        let mut s = self.clone();
        match c.offset {
            Some(o) if !self.ignore_own => s.correction = total - o,
            _ => {
                if (total - self.zone).abs() > 1800 {
                    s.zone = ((total as f64 / 3600.0).round() as i64 * 3600)
                        .clamp(-12 * 3600, MAX_SHIFT);
                }
                s.correction = total - s.zone;
            }
        }
        s
    }
}

/// Where each candidate is at a shift: on the track, outside it within the margin, or further.
#[derive(Debug, Clone, Default)]
pub struct Matched {
    pub on: Vec<(i64, Place)>,
    pub outside: Vec<(i64, Outside)>,
    pub further: usize,
}

pub fn match_all(
    track: &Track,
    photos: &[Candidate],
    shift: &Shift,
    max_gap: f64,
    margin_h: u32,
) -> Matched {
    let mut m = Matched::default();
    let Some((first, last)) = track.span() else {
        return m;
    };
    let margin = f64::from(margin_h) * 3600.0;
    for c in photos {
        let t = shift.utc(c);
        match track.locate(t, max_gap) {
            p @ Place::On { .. } => m.on.push((c.id, p)),
            Place::Outside(o) => {
                if t < first - margin || t > last + margin {
                    m.further += 1;
                } else {
                    m.outside.push((c.id, o));
                }
            }
        }
    }
    m
}

/// The default zone (design.md §2.9(e)): the photos' own offset when those that have one agree,
/// else this PC's zone on the track's first day, else UTC.
pub fn default_zone(track: &Track, photos: &[Candidate]) -> i64 {
    let own: HashSet<i64> = photos.iter().filter_map(|c| c.offset).collect();
    if own.len() == 1 {
        return own.into_iter().next().unwrap_or(0);
    }
    track
        .span()
        .and_then(|(t, _)| time::local_offset_at(t))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated track: north from (10, 20) at 0.0001° per 10 s (about 11 m), from 08:00Z for
    /// 30 minutes, then a 60-minute pause in place, then 10 minutes on, then a 20-minute gap
    /// with a 1 km jump, then 10 minutes more.
    fn track() -> Track {
        let t0 = time::parse("2025-06-12T08:00:00Z").unwrap().0;
        let mut pts = Vec::new();
        let mut lat = 10.0;
        let mut t = t0;
        let step = |pts: &mut Vec<Point>, lat: &mut f64, t: &mut f64, n: usize| {
            for _ in 0..n {
                pts.push(Point {
                    lat: *lat,
                    lon: 20.0,
                    ele: Some(100.0),
                    t: Some(*t),
                });
                *lat += 0.0001;
                *t += 10.0;
            }
        };
        step(&mut pts, &mut lat, &mut t, 180); // 30 min
        lat -= 0.0001;
        t += 3600.0 - 10.0; // the pause: the next point an hour later, same spot
        step(&mut pts, &mut lat, &mut t, 60);
        lat += 0.01; // a 1 km jump
        t += 1200.0;
        step(&mut pts, &mut lat, &mut t, 60);
        Track::new(vec![(
            gpx::Gpx {
                name: None,
                segments: vec![pts],
            },
            "walk.gpx".into(),
        )])
    }

    fn at(s: &str) -> f64 {
        time::parse(s).unwrap().0
    }

    #[test]
    fn a_photo_at_10_local_with_utc_plus_2_matches_8_utc() {
        let tr = track();
        let photo = Candidate {
            id: 1,
            clock: at("2025-06-12T10:00:00"),
            offset: None,
            current: None,
        };
        let shift = Shift {
            zone: 7200,
            correction: 0,
            ignore_own: false,
        };
        assert_eq!(shift.utc(&photo), at("2025-06-12T08:00:00"));
        match tr.locate(shift.utc(&photo), DEFAULT_MAX_GAP) {
            Place::On { lat, lon, how, .. } => {
                assert_eq!((lat, lon, how), (10.0, 20.0, How::End));
            }
            p => panic!("{p:?}"),
        }
    }

    #[test]
    fn interpolation_pauses_ends_and_gaps() {
        let tr = track();
        let t0 = at("2025-06-12T08:00:00");
        match tr.locate(t0 + 15.0, DEFAULT_MAX_GAP) {
            Place::On { lat, how, ele, .. } => {
                assert!((lat - 10.00015).abs() < 1e-9);
                assert_eq!((how, ele), (How::Interpolated, Some(100.0)));
            }
            p => panic!("{p:?}"),
        }
        // In the pause: the spot where it began.
        match tr.locate(t0 + 1800.0 + 1000.0, DEFAULT_MAX_GAP) {
            Place::On { lat, how, .. } => {
                assert_eq!(how, How::Pause);
                assert!((lat - (10.0 + 179.0 * 0.0001)).abs() < 1e-9);
            }
            p => panic!("{p:?}"),
        }
        // In the 20-minute gap with a jump: outside.
        let (_, end) = tr.span().unwrap();
        let gap_mid = end - 600.0 - 600.0;
        assert!(matches!(
            tr.locate(gap_mid, DEFAULT_MAX_GAP),
            Place::Outside(Outside::Gap(_))
        ));
        // A wider max gap bridges it.
        assert!(matches!(
            tr.locate(gap_mid, 1800.0),
            Place::On {
                how: How::Interpolated,
                ..
            }
        ));
        // Just past the end: the end; further: after.
        assert!(matches!(
            tr.locate(end + 60.0, DEFAULT_MAX_GAP),
            Place::On { how: How::End, .. }
        ));
        assert_eq!(
            tr.locate(end + 600.0, DEFAULT_MAX_GAP),
            Place::Outside(Outside::After(600.0))
        );
        assert_eq!(
            tr.locate(t0 - 300.0, DEFAULT_MAX_GAP),
            Place::Outside(Outside::Before(300.0))
        );
        assert_eq!(tr.pauses(DEFAULT_MAX_GAP).len(), 1);
    }

    #[test]
    fn the_margin_counts_from_the_track_at_the_current_shift() {
        let tr = track();
        let photo = |id, s: &str| Candidate {
            id,
            clock: at(s),
            offset: None,
            current: None,
        };
        let photos = [
            photo(1, "2025-06-12T10:10:00"), // on the track
            photo(2, "2025-06-12T09:00:00"), // an hour before: outside, within 3 h
            photo(3, "2025-06-11T23:00:00"), // the evening before: further
        ];
        let shift = Shift {
            zone: 7200,
            correction: 0,
            ignore_own: false,
        };
        let m = match_all(&tr, &photos, &shift, DEFAULT_MAX_GAP, 3);
        assert_eq!(m.on.iter().map(|o| o.0).collect::<Vec<_>>(), [1]);
        assert_eq!(m.outside.len(), 1);
        assert_eq!(m.further, 1);
        let wide = match_all(&tr, &photos, &shift, DEFAULT_MAX_GAP, 12);
        assert_eq!((wide.outside.len(), wide.further), (2, 0));
    }

    #[test]
    fn own_offsets_and_the_anchor() {
        let shift = Shift {
            zone: 7200,
            correction: 0,
            ignore_own: false,
        };
        let own = Candidate {
            id: 1,
            clock: at("2025-06-12T11:00:00"),
            offset: Some(3 * 3600),
            current: None,
        };
        assert_eq!(shift.utc(&own), at("2025-06-12T08:00:00"));
        let ignoring = Shift {
            ignore_own: true,
            ..shift.clone()
        };
        assert_eq!(ignoring.utc(&own), at("2025-06-12T09:00:00"));
        // Anchored: the camera runs 3 min 40 s fast.
        let plain = Candidate {
            id: 2,
            clock: at("2025-06-12T10:03:40"),
            offset: None,
            current: None,
        };
        let a = shift.anchored(&plain, at("2025-06-12T08:00:00"));
        assert_eq!((a.zone, a.correction), (7200, 220));
        assert_eq!(time::shift_text(a.total()), "UTC+2:03:40");
        // Far from the zone: the nearest hour, the rest as the correction.
        let far = shift.anchored(&plain, at("2025-06-12T02:00:00"));
        assert_eq!((far.zone, far.correction), (8 * 3600, 220));
        // With its own offset: only the correction.
        let o = shift.anchored(&own, at("2025-06-12T07:59:00"));
        assert_eq!((o.zone, o.correction), (7200, 60));
    }

    #[test]
    fn the_time_near_a_spot() {
        let tr = track();
        let (t, spot) = tr.time_near(10.0005, 20.00001).unwrap();
        assert!((t - (at("2025-06-12T08:00:00") + 50.0)).abs() < 1.0);
        assert!((spot.0 - 10.0005).abs() < 1e-7);
    }

    #[test]
    fn several_files_one_timeline_and_routes() {
        let seg = |lat: f64, t: Option<f64>| {
            vec![Point {
                lat,
                lon: 0.0,
                ele: None,
                t,
            }]
        };
        let late = gpx::Gpx {
            name: Some("Day 2".into()),
            segments: vec![seg(2.0, Some(2000.0))],
        };
        let early = gpx::Gpx {
            name: None,
            segments: vec![seg(1.0, Some(1000.0))],
        };
        let tr = Track::new(vec![(late, "b.gpx".into()), (early, "a.gpx".into())]);
        assert_eq!((tr.name.as_str(), tr.files), ("Day 2", 2));
        assert_eq!(tr.span(), Some((1000.0, 2000.0)));
        let route = gpx::Gpx {
            name: None,
            segments: vec![seg(1.0, None), seg(2.0, None)],
        };
        let r = Track::new(vec![(route, "route.gpx".into())]);
        assert!(!r.has_times());
        assert_eq!((r.lines.len(), r.dropped, r.points), (2, 0, 2));
    }
}

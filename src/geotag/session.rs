//! One tagging session (design.md §2.9): the track, the candidate photos, the shift and the
//! ticks, and where each photo sits at the current settings. Pure state, so it's tested
//! without the UI; `ui::geotag_panel` draws it and the app wires it to the map.

use std::collections::HashSet;
use std::path::PathBuf;

use super::sidecar::Gps;
use super::{
    Candidate, DEFAULT_MARGIN, DEFAULT_MAX_GAP, MAX_SHIFT, Matched, Outside, Place, Shift, Track,
    default_zone, match_all, time,
};
use crate::library::{GeoPhoto, PhotoPoint};

/// How much wider than the track the candidates are read: the widest shift, the largest
/// margin and the clock correction.
const POOL: f64 = (MAX_SHIFT + 12 * 3600 + 3600) as f64;

pub struct Session {
    pub files: Vec<PathBuf>,
    pub track: Track,
    pub photos: Vec<GeoPhoto>,
    cands: Vec<Candidate>,
    pub shift: Shift,
    pub max_gap_min: u32,
    pub margin_h: u32,
    pub retag: bool,
    pub write_alt: bool,
    /// The unticked photos (design.md §2.9(h)).
    pub off: HashSet<i64>,
    pub matched: Matched,
    /// The photo clicked on the map, for "taken here".
    pub selected: Option<i64>,
    /// Waiting for a click on the track (design.md §2.9(f)).
    pub picking: bool,
    /// The photo that set the shift from its place, and the shift before (for Undo).
    pub anchored: Option<(i64, Shift)>,
    /// Photos without a position and without a capture time, library-wide.
    pub undated: i64,
    /// Something was changed since the session began (leaving asks first).
    pub changed: bool,
}

/// `YYYY-MM-DDTHH:MM:SS` of a clock reading.
fn naive(t: f64) -> String {
    let mut s = time::format_utc(t);
    s.pop(); // the Z
    s
}

impl Session {
    pub fn new(files: Vec<PathBuf>, track: Track, photos: Vec<GeoPhoto>, undated: i64) -> Self {
        let mut s = Session {
            files,
            track,
            photos: Vec::new(),
            cands: Vec::new(),
            shift: Shift {
                zone: 0,
                correction: 0,
                ignore_own: false,
            },
            max_gap_min: (DEFAULT_MAX_GAP / 60.0) as u32,
            margin_h: DEFAULT_MARGIN,
            retag: false,
            write_alt: true,
            off: HashSet::new(),
            matched: Matched::default(),
            selected: None,
            picking: false,
            anchored: None,
            undated,
            changed: false,
        };
        s.set_photos(photos);
        s.shift.zone = default_zone(&s.track, &s.cands);
        s.rematch();
        s
    }

    /// The capture times to read candidates for (`None` without times).
    pub fn pool(track: &Track) -> Option<(String, String)> {
        let (a, b) = track.span()?;
        Some((naive(a - POOL), naive(b + POOL)))
    }

    /// New candidates (the re-tag option changed, or thumbnails were made): ticks are kept.
    pub fn set_photos(&mut self, photos: Vec<GeoPhoto>) {
        self.cands = photos
            .iter()
            .filter_map(|p| {
                let (clock, _) = time::parse(&p.taken_at)?;
                Some(Candidate {
                    id: p.id,
                    clock,
                    offset: p.offset.as_deref().and_then(time::parse_offset).flatten(),
                    current: p.lat.zip(p.lon),
                })
            })
            .collect();
        self.photos = photos;
        self.rematch();
    }

    pub fn max_gap(&self) -> f64 {
        f64::from(self.max_gap_min) * 60.0
    }

    /// Matches every candidate at the current settings.
    pub fn rematch(&mut self) {
        self.matched = match_all(
            &self.track,
            &self.cands,
            &self.shift,
            self.max_gap(),
            self.margin_h,
        );
    }

    pub fn set_shift(&mut self, shift: Shift) {
        if shift != self.shift {
            self.shift = shift;
            self.changed = true;
            self.rematch();
        }
    }

    pub fn photo(&self, id: i64) -> Option<&GeoPhoto> {
        self.photos.iter().find(|p| p.id == id)
    }

    fn candidate(&self, id: i64) -> Option<&Candidate> {
        self.cands.iter().find(|c| c.id == id)
    }

    /// Some candidates carry their own time zone (the checkbox to ignore them shows).
    pub fn has_own_offsets(&self) -> bool {
        self.cands.iter().any(|c| c.offset.is_some())
    }

    /// The photos on the track, at their matched positions, for the map.
    pub fn points(&self) -> Vec<PhotoPoint> {
        let mut pts: Vec<PhotoPoint> = self
            .matched
            .on
            .iter()
            .filter_map(|(id, place)| {
                let Place::On { lat, lon, .. } = *place else {
                    return None;
                };
                let p = self.photo(*id)?;
                Some(PhotoPoint {
                    id: *id,
                    lat,
                    lon,
                    thumb: p.thumb,
                    taken: p
                        .taken_at
                        .chars()
                        .filter(char::is_ascii_digit)
                        .take(14)
                        .collect::<String>()
                        .parse()
                        .unwrap_or(0),
                })
            })
            .collect();
        pts.sort_by_key(|p| p.id);
        pts
    }

    pub fn on_track(&self) -> usize {
        self.matched.on.len()
    }

    pub fn ticked(&self) -> usize {
        self.matched
            .on
            .iter()
            .filter(|(id, _)| !self.off.contains(id))
            .count()
    }

    /// Flips these photos' ticks (the map passes the photos whose tick changes).
    pub fn toggle(&mut self, ids: &[i64]) {
        for id in ids {
            if !self.off.remove(id) {
                self.off.insert(*id);
            }
        }
        self.changed = true;
    }

    /// "The selected photo was taken here": the shift follows (design.md §2.9(f)).
    pub fn anchor_at(&mut self, lat: f64, lon: f64) -> bool {
        self.picking = false;
        let (Some(id), Some((t, _))) = (self.selected, self.track.time_near(lat, lon)) else {
            return false;
        };
        let Some(c) = self.candidate(id).cloned() else {
            return false;
        };
        let before = self.shift.clone();
        let mut new = self.shift.anchored(&c, t);
        new.correction = new.correction.clamp(-1800, 1800);
        self.set_shift(new);
        self.anchored = Some((id, before));
        true
    }

    pub fn undo_anchor(&mut self) {
        if let Some((_, before)) = self.anchored.take() {
            self.set_shift(before);
        }
    }

    /// What `outside` says about a photo: "46 min before the start".
    pub fn why(o: Outside) -> String {
        let span = |s: f64| {
            let m = (s / 60.0).round() as i64;
            if m >= 120 {
                format!("{} h {} min", m / 60, m % 60)
            } else {
                format!("{m} min")
            }
        };
        match o {
            Outside::Before(s) => format!("{} before the start", span(s)),
            Outside::After(s) => format!("{} after the end", span(s)),
            Outside::Gap(s) if s > 0.0 => format!("in a {} gap", span(s)),
            Outside::Gap(_) => "between two segments".to_owned(),
            Outside::NoTimes => "no times".to_owned(),
        }
    }

    /// The ticked photos on the track with the position to write.
    pub fn writes(&self) -> Vec<(GeoPhoto, Gps)> {
        self.matched
            .on
            .iter()
            .filter(|(id, _)| !self.off.contains(id))
            .filter_map(|(id, place)| {
                let Place::On { lat, lon, ele, .. } = *place else {
                    return None;
                };
                let c = self.candidate(*id)?;
                let p = self.photo(*id)?.clone();
                let gps = Gps {
                    lat,
                    lon,
                    alt: ele.filter(|_| self.write_alt),
                    time: self.shift.utc(c),
                };
                Some((p, gps))
            })
            .collect()
    }

    /// The photo's capture time as its camera wrote it, `HH:MM:SS`.
    pub fn clock_of(&self, id: i64) -> Option<String> {
        self.candidate(id).map(|c| time::clock(c.clock))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::geotag::{Point, gpx};
    use crate::library::Thumb;

    fn photo(id: i64, taken: &str, offset: Option<&str>) -> GeoPhoto {
        GeoPhoto {
            id,
            taken_at: taken.to_owned(),
            offset: offset.map(str::to_owned),
            lat: None,
            lon: None,
            alt: None,
            gps_source: None,
            thumb: Thumb::Pending,
            root: PathBuf::from("C:\\P"),
            unc: None,
            rel_dir: String::new(),
            stem: format!("IMG_{id}"),
            sidecar_name: None,
            original_name: format!("IMG_{id}.CR2"),
        }
    }

    /// Ten minutes north from (10, 20) from 08:00Z, a point a minute.
    fn track() -> Track {
        let t0 = time::parse("2025-06-12T08:00:00Z").unwrap().0;
        let pts = (0..=10)
            .map(|i| Point {
                lat: 10.0 + f64::from(i) * 0.001,
                lon: 20.0,
                ele: Some(500.0),
                t: Some(t0 + f64::from(i) * 60.0),
            })
            .collect();
        Track::new(vec![(
            gpx::Gpx {
                name: None,
                segments: vec![pts],
            },
            "t.gpx".into(),
        )])
    }

    #[test]
    fn the_session_places_ticks_shifts_and_writes() {
        // The photos' own zone (+02:00) is the default; one photo is outside.
        let photos = vec![
            photo(1, "2025-06-12T10:02:00", Some("+02:00")),
            photo(2, "2025-06-12T10:05:00", Some("+02:00")),
            photo(3, "2025-06-12T11:30:00", Some("+02:00")),
        ];
        let mut s = Session::new(Vec::new(), track(), photos, 4);
        assert_eq!(s.shift.zone, 7200);
        assert_eq!((s.on_track(), s.matched.outside.len()), (2, 1));
        assert_eq!(Session::why(s.matched.outside[0].1), "80 min after the end");
        let pts = s.points();
        assert!((pts[0].lat - 10.002).abs() < 1e-9);
        assert_eq!(pts[0].taken, 20250612100200);

        s.toggle(&[2]);
        assert_eq!(s.ticked(), 1);
        let w = s.writes();
        assert_eq!(w.len(), 1);
        assert_eq!(w[0].1.alt, Some(500.0));
        assert_eq!(time::format_utc(w[0].1.time), "2025-06-12T08:02:00Z");
        s.write_alt = false;
        assert_eq!(s.writes()[0].1.alt, None);

        // "Taken here": photo 1 at the spot of 08:04 sets a correction of -2 min.
        s.selected = Some(1);
        assert!(s.anchor_at(10.004, 20.0));
        assert_eq!(s.shift.correction, -120);
        assert!((s.points()[0].lat - 10.004).abs() < 1e-9);
        s.undo_anchor();
        assert_eq!(s.shift.correction, 0);
        assert!(s.changed);
    }

    #[test]
    fn the_pool_covers_every_shift() {
        let (from, to) = Session::pool(&track()).unwrap();
        assert_eq!(from, "2025-06-11T05:00:00");
        assert_eq!(to, "2025-06-13T11:10:00");
    }
}

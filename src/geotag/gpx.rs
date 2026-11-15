//! GPX 1.0 and 1.1 tracks (design.md §2.9(b), https://www.topografix.com/GPX/1/1/): the points
//! of each `trkseg` (and of each `rte`, which never has times) with position, elevation and
//! time. Extensions are skipped. Our own reader on `quick-xml`, like the XMP reader.

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use super::time;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Point {
    pub lat: f64,
    pub lon: f64,
    pub ele: Option<f64>,
    /// UTC seconds since 1970.
    pub t: Option<f64>,
}

/// One GPX file: its name and its segments, in file order.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Gpx {
    pub name: Option<String>,
    pub segments: Vec<Vec<Point>>,
}

impl Gpx {
    pub fn points(&self) -> usize {
        self.segments.iter().map(Vec::len).sum()
    }
}

/// Reads a GPX document.
pub fn parse(xml: &str) -> Result<Gpx, String> {
    let mut reader = Reader::from_str(xml);
    let mut gpx = Gpx::default();
    let mut point: Option<Point> = None;
    // The element whose text comes next: `ele`, `time` or `name`.
    let mut open: Option<&'static str> = None;
    let mut depth_ext = 0usize; // inside <extensions>: everything is skipped
    let mut saw_gpx = false;
    loop {
        let ev = reader.read_event().map_err(|e| e.to_string())?;
        match ev {
            Event::Start(ref e) | Event::Empty(ref e) => {
                let empty = matches!(ev, Event::Empty(_));
                let name = e.name();
                let local = name.local_name();
                let local = local.as_ref();
                if depth_ext > 0 {
                    if !empty {
                        depth_ext += 1;
                    }
                    continue;
                }
                match local {
                    "gpx" => saw_gpx = true,
                    "extensions" if !empty => depth_ext = 1,
                    "trkseg" | "rte" => gpx.segments.push(Vec::new()),
                    "trkpt" | "rtept" => {
                        let (mut lat, mut lon) = (None, None);
                        for a in e.attributes().flatten() {
                            let v = a
                                .normalized_value(XmlVersion::Implicit1_0)
                                .map_err(|e| e.to_string())?;
                            match a.key.local_name().as_ref() {
                                "lat" => lat = v.trim().parse::<f64>().ok(),
                                "lon" => lon = v.trim().parse::<f64>().ok(),
                                _ => {}
                            }
                        }
                        let p = match (lat, lon) {
                            (Some(lat), Some(lon))
                                if (-90.0..=90.0).contains(&lat)
                                    && (-180.0..=180.0).contains(&lon) =>
                            {
                                Some(Point {
                                    lat,
                                    lon,
                                    ele: None,
                                    t: None,
                                })
                            }
                            _ => None, // a point without a usable position is skipped
                        };
                        if empty {
                            push(&mut gpx, p);
                        } else {
                            point = p;
                        }
                    }
                    "ele" if point.is_some() => open = Some("ele"),
                    "time" if point.is_some() => open = Some("time"),
                    "name" if gpx.name.is_none() && point.is_none() => open = Some("name"),
                    _ => {}
                }
            }
            Event::Text(t) => {
                if let (Some(what), false) = (open, depth_ext > 0) {
                    let text = t.xml10_content();
                    let text = text.trim();
                    match (what, &mut point) {
                        ("ele", Some(p)) => p.ele = text.parse().ok(),
                        ("time", Some(p)) => {
                            // GPX times are UTC; one with an offset is converted, one with
                            // neither is taken as UTC (design.md §2.9(b)).
                            p.t = time::parse(text).map(|(t, z)| t - z.unwrap_or(0) as f64);
                        }
                        ("name", _) if !text.is_empty() => gpx.name = Some(text.to_owned()),
                        _ => {}
                    }
                }
            }
            Event::End(e) => {
                if depth_ext > 0 {
                    depth_ext -= 1;
                    continue;
                }
                open = None;
                let name = e.name();
                if matches!(name.local_name().as_ref(), "trkpt" | "rtept") {
                    let p = point.take();
                    push(&mut gpx, p);
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if !saw_gpx {
        return Err("not a GPX file".to_owned());
    }
    gpx.segments.retain(|s| !s.is_empty());
    Ok(gpx)
}

fn push(gpx: &mut Gpx, p: Option<Point>) {
    let Some(p) = p else { return };
    if gpx.segments.is_empty() {
        gpx.segments.push(Vec::new()); // a point outside any segment (malformed, but readable)
    }
    if let Some(s) = gpx.segments.last_mut() {
        s.push(p);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A generated track: two segments, the second with an offset time and an extension.
    const SAMPLE: &str = r#"<?xml version="1.0"?>
<gpx xmlns="http://www.topografix.com/GPX/1/1" creator="test" version="1.1">
  <metadata><name>Test walk</name></metadata>
  <trk><name>Track name</name>
    <trkseg>
      <trkpt lat="10.0" lon="20.0"><ele>100.5</ele><time>2025-06-12T08:00:00Z</time></trkpt>
      <trkpt lat="10.001" lon="20.001"><ele>101</ele><time>2025-06-12T08:00:10.250Z</time>
        <extensions><gpxtpx:TrackPointExtension xmlns:gpxtpx="x"><gpxtpx:hr>120</gpxtpx:hr><time>junk</time></gpxtpx:TrackPointExtension></extensions>
      </trkpt>
    </trkseg>
    <trkseg>
      <trkpt lat="10.01" lon="20.01"><time>2025-06-12T10:30:00+02:00</time></trkpt>
      <trkpt lat="999" lon="20.01"><time>2025-06-12T08:31:00Z</time></trkpt>
    </trkseg>
  </trk>
</gpx>"#;

    #[test]
    fn segments_points_and_times() {
        let g = parse(SAMPLE).unwrap();
        assert_eq!(g.name.as_deref(), Some("Test walk"));
        assert_eq!(g.segments.len(), 2);
        assert_eq!(g.points(), 3, "the point at latitude 999 is skipped");
        let a = g.segments[0][0];
        assert_eq!((a.lat, a.lon, a.ele), (10.0, 20.0, Some(100.5)));
        let t0 = time::parse("2025-06-12T08:00:00Z").unwrap().0;
        assert_eq!(a.t, Some(t0));
        let b = g.segments[0][1];
        assert!(
            (b.t.unwrap() - t0 - 10.25).abs() < 1e-6,
            "the extension's <time> is ignored"
        );
        assert_eq!(
            g.segments[1][0].t,
            Some(t0 + 1800.0),
            "+02:00 converted to UTC"
        );
        assert_eq!(g.segments[1][0].ele, None);
    }

    #[test]
    fn a_planned_route_has_no_times() {
        let g = parse(
            r#"<gpx version="1.1"><rte><name>Planned</name>
               <rtept lat="1" lon="2"/><rtept lat="1.1" lon="2.1"><ele>5</ele></rtept></rte>
               <trk><trkseg><trkpt lat="3" lon="4"><ele>7</ele></trkpt></trkseg></trk></gpx>"#,
        )
        .unwrap();
        assert_eq!(g.name.as_deref(), Some("Planned"));
        assert_eq!(g.segments.len(), 2);
        assert!(g.segments.iter().flatten().all(|p| p.t.is_none()));
    }

    #[test]
    fn not_gpx() {
        assert!(parse("<kml></kml>").is_err());
        assert!(parse("<gpx><trk>").is_err() || parse("<gpx><trk>").unwrap().points() == 0);
    }
}

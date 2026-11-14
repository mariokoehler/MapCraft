//! GPS from XMP sidecars (design.md §2.8(a)): Lightroom and others write positions there,
//! especially for camera RAWs, and the sidecar wins over the file's own EXIF.
//!
//! Positions appear as attributes (`exif:GPSLatitude="53,32.616N"`) or as elements
//! (`<exif:GPSLatitude>53,32.616N</exif:GPSLatitude>`); both occur in the user's library, always
//! as degrees plus decimal minutes (surveyed 2026-11-14). Degrees-minutes-seconds and plain
//! decimals are accepted too.

use quick_xml::events::Event;
use quick_xml::{Reader, XmlVersion};

use super::Gps;

/// Reads the GPS position from an XMP document. `Ok(None)`: a valid sidecar without a position.
pub fn read_gps(xml: &str) -> Result<Option<Gps>, String> {
    let mut reader = Reader::from_str(xml);
    let (mut lat, mut lon, mut alt, mut alt_ref) = (None, None, None, None);
    let mut open: Option<String> = None; // the GPS element whose text comes next
    loop {
        match reader.read_event().map_err(|e| e.to_string())? {
            Event::Start(e) | Event::Empty(e) => {
                for a in e.attributes().flatten() {
                    // `local_name` drops the namespace prefix: `exif:GPSLatitude` → `GPSLatitude`.
                    let key = a.key.local_name();
                    let value = a
                        .normalized_value(XmlVersion::Implicit1_0)
                        .map_err(|e| e.to_string())?;
                    store(
                        key.as_ref(),
                        &value,
                        &mut lat,
                        &mut lon,
                        &mut alt,
                        &mut alt_ref,
                    );
                }
                open = Some(e.name().local_name().as_ref().to_owned());
            }
            Event::Text(t) => {
                if let Some(key) = &open {
                    let value = t.xml10_content();
                    store(
                        key,
                        value.trim(),
                        &mut lat,
                        &mut lon,
                        &mut alt,
                        &mut alt_ref,
                    );
                }
            }
            Event::End(_) => open = None,
            Event::Eof => break,
            _ => {}
        }
    }
    let alt = alt.map(|a: f64| if alt_ref == Some(1) { -a } else { a });
    Ok(match (lat, lon) {
        (Some(lat), Some(lon)) => Gps::new(lat, lon, alt),
        _ => None,
    })
}

fn store(
    key: &str,
    value: &str,
    lat: &mut Option<f64>,
    lon: &mut Option<f64>,
    alt: &mut Option<f64>,
    alt_ref: &mut Option<u8>,
) {
    match key {
        "GPSLatitude" => *lat = coordinate(value, 'N', 'S'),
        "GPSLongitude" => *lon = coordinate(value, 'E', 'W'),
        "GPSAltitude" => *alt = rational(value),
        "GPSAltitudeRef" => *alt_ref = value.trim().parse().ok(),
        _ => {}
    }
}

/// `53,32.616N`, `53,32,36.96N` or `53.5436` (sign or hemisphere letter).
fn coordinate(value: &str, pos: char, neg: char) -> Option<f64> {
    let v = value.trim();
    let (body, sign) = match v.chars().last()? {
        c if c.eq_ignore_ascii_case(&pos) => (&v[..v.len() - 1], 1.0),
        c if c.eq_ignore_ascii_case(&neg) => (&v[..v.len() - 1], -1.0),
        _ => (v, 1.0),
    };
    let parts: Vec<f64> = body
        .split(',')
        .map(|p| p.trim().parse::<f64>())
        .collect::<Result<_, _>>()
        .ok()?;
    let deg = match parts[..] {
        [d] => d,
        [d, m] => d + m / 60.0,
        [d, m, s] => d + m / 60.0 + s / 3600.0,
        _ => return None,
    };
    Some(sign * deg)
}

/// `1234/10` or `123.4`.
fn rational(value: &str) -> Option<f64> {
    let v = value.trim();
    match v.split_once('/') {
        Some((n, d)) => {
            let (n, d): (f64, f64) = (n.trim().parse().ok()?, d.trim().parse().ok()?);
            (d != 0.0).then(|| n / d)
        }
        None => v.parse().ok(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const ATTR: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/"
          exif:GPSLatitude="33,52.2S" exif:GPSLongitude="151,12.6E"
          exif:GPSAltitude="584/10" exif:GPSAltitudeRef="0"/></rdf:RDF></x:xmpmeta>"#;

    const ELEM: &str = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="http://www.w3.org/1999/02/22-rdf-syntax-ns#">
        <rdf:Description xmlns:exif="http://ns.adobe.com/exif/1.0/">
          <exif:GPSLatitude>40,42.768N</exif:GPSLatitude>
          <exif:GPSLongitude>74,0.36W</exif:GPSLongitude>
        </rdf:Description></rdf:RDF></x:xmpmeta>"#;

    #[test]
    fn attribute_form_south_east_with_altitude() {
        let g = read_gps(ATTR).unwrap().unwrap();
        assert!((g.lat - -33.87).abs() < 1e-9);
        assert!((g.lon - 151.21).abs() < 1e-9);
        assert_eq!(g.alt, Some(58.4));
    }

    #[test]
    fn element_form_north_west() {
        let g = read_gps(ELEM).unwrap().unwrap();
        assert!((g.lat - 40.7128).abs() < 1e-9);
        assert!((g.lon - -74.006).abs() < 1e-9);
    }

    #[test]
    fn no_position_and_notations() {
        let none = r#"<x:xmpmeta xmlns:x="adobe:ns:meta/"><rdf:RDF xmlns:rdf="r"><rdf:Description xmlns:xmp="x" xmp:Rating="3"/></rdf:RDF></x:xmpmeta>"#;
        assert_eq!(read_gps(none).unwrap(), None);
        assert_eq!(
            coordinate("53,32,36N", 'N', 'S'),
            Some(53.0 + 32.0 / 60.0 + 36.0 / 3600.0)
        );
        assert_eq!(coordinate("-12.5", 'N', 'S'), Some(-12.5));
        assert_eq!(coordinate("garbage", 'N', 'S'), None);
        assert_eq!(rational("1/0"), None);
    }
}

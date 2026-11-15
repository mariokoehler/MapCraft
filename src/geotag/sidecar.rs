//! Writing GPS into XMP sidecars (design.md §2.9(i)): a new `name.xmp`, or the existing sidecar
//! with only its GPS properties changed. Everything else in an existing file stays byte for
//! byte: the edits are made on the text, at the positions the XML reader reports, never by
//! re-serialising the document. Each result is read back with `metadata::xmp::read_gps` before
//! it replaces anything.

use std::io::Write as _;
use std::path::{Path, PathBuf};

use quick_xml::Reader;
use quick_xml::events::Event;

use super::time;
use crate::metadata::xmp;

pub const EXIF_NS: &str = "http://ns.adobe.com/exif/1.0/";
const BOM: &[u8] = b"\xEF\xBB\xBF";

/// The properties MapCraft writes (and the only ones it changes), in writing order.
const PROPS: [&str; 6] = [
    "GPSVersionID",
    "GPSLatitude",
    "GPSLongitude",
    "GPSAltitudeRef",
    "GPSAltitude",
    "GPSTimeStamp",
];

/// A position to write.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gps {
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,
    /// The matched time, UTC.
    pub time: f64,
}

/// Degrees and decimal minutes with a hemisphere letter, as Lightroom writes them: `64,8.123456N`.
pub fn coordinate(v: f64, pos: char, neg: char) -> String {
    let h = if v < 0.0 { neg } else { pos };
    let a = v.abs();
    let mut d = a.floor();
    let mut m = ((a - d) * 60.0 * 1e6).round() / 1e6;
    if m >= 60.0 {
        d += 1.0;
        m = 0.0;
    }
    format!("{},{m:.6}{h}", d as i64)
}

/// The value of each property; `None` removes it (an altitude that isn't written).
fn values(g: &Gps) -> [(&'static str, Option<String>); 6] {
    let alt = g.alt.map(|a| {
        (
            if a < 0.0 { "1" } else { "0" }.to_owned(),
            format!("{}/100", (a.abs() * 100.0).round() as i64),
        )
    });
    [
        ("GPSVersionID", Some("2.2.0.0".to_owned())),
        ("GPSLatitude", Some(coordinate(g.lat, 'N', 'S'))),
        ("GPSLongitude", Some(coordinate(g.lon, 'E', 'W'))),
        ("GPSAltitudeRef", alt.as_ref().map(|a| a.0.clone())),
        ("GPSAltitude", alt.map(|a| a.1)),
        ("GPSTimeStamp", Some(time::format_utc(g.time))),
    ]
}

/// A new sidecar holding only the position.
pub fn create(g: &Gps) -> String {
    let mut attrs = String::new();
    for (k, v) in values(g) {
        if let Some(v) = v {
            attrs.push_str(&format!("\n    exif:{k}=\"{v}\""));
        }
    }
    format!(
        "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"MapCraft\">\n \
         <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\n  \
         <rdf:Description rdf:about=\"\"\n    xmlns:exif=\"{EXIF_NS}\"{attrs}/>\n \
         </rdf:RDF>\n</x:xmpmeta>\n"
    )
}

/// One attribute in a tag's text: its name, the span from the whitespace before it to the
/// closing quote, and the value's span (all relative to the tag's start).
struct Attr {
    name: String,
    lead: usize,
    value: (usize, usize),
    end: usize,
}

/// The attributes of a start tag's text (`<name a="1" b='2'>` or `.../>`).
fn attrs(tag: &str) -> Result<Vec<Attr>, String> {
    let b = tag.as_bytes();
    let mut i = 1;
    while i < b.len() && !b[i].is_ascii_whitespace() && b[i] != b'/' && b[i] != b'>' {
        i += 1;
    }
    let mut out = Vec::new();
    loop {
        let lead = i;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= b.len() || b[i] == b'/' || b[i] == b'>' {
            return Ok(out);
        }
        let n0 = i;
        while i < b.len() && b[i] != b'=' && !b[i].is_ascii_whitespace() {
            i += 1;
        }
        let name = tag[n0..i].to_owned();
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        if b.get(i) != Some(&b'=') {
            return Err(format!("unreadable attribute {name}"));
        }
        i += 1;
        while i < b.len() && b[i].is_ascii_whitespace() {
            i += 1;
        }
        let q = *b.get(i).ok_or("unreadable attribute")?;
        if q != b'"' && q != b'\'' {
            return Err(format!("unquoted attribute {name}"));
        }
        let v0 = i + 1;
        let v1 = v0 + tag[v0..].find(q as char).ok_or("unterminated attribute")?;
        i = v1 + 1;
        out.push(Attr {
            name,
            lead,
            value: (v0, v1),
            end: i,
        });
    }
}

/// A GPS element: its name, whole span, and text span (`None` when empty).
type ElemSpan = (String, (usize, usize), Option<(usize, usize)>);

/// Where a GPS property is in the document.
enum Found {
    /// An attribute: the span to remove it, and its value's span.
    Attr {
        whole: (usize, usize),
        value: (usize, usize),
    },
    /// An element: its whole span, and its text's span (`None` for `<p:X/>`).
    Elem {
        whole: (usize, usize),
        text: Option<(usize, usize)>,
        name: String,
    },
}

fn split(name: &str) -> (&str, &str) {
    name.split_once(':').unwrap_or(("", name))
}

/// The document with the GPS properties set to `g` (design.md §2.9(i)).
pub fn merge(doc: &str, g: &Gps) -> Result<String, String> {
    struct Tag {
        span: (usize, usize),
        name: String,
        attrs: Vec<Attr>,
    }
    let mut reader = Reader::from_str(doc);
    let mut tags: Vec<Tag> = Vec::new();
    // A GPS element being read: (its name, start, end of its start tag).
    let mut in_elem: Option<(String, usize, usize)> = None;
    let mut elems: Vec<ElemSpan> = Vec::new();
    loop {
        let before = reader.buffer_position() as usize;
        let ev = reader.read_event().map_err(|e| e.to_string())?;
        let after = reader.buffer_position() as usize;
        match ev {
            Event::Start(e) | Event::Empty(e) => {
                let empty = doc[before..after].ends_with("/>");
                let name = e.name().as_ref().to_owned();
                if in_elem.is_some() {
                    return Err("a GPS property holds a structure".to_owned());
                }
                let text = &doc[before..after];
                tags.push(Tag {
                    span: (before, after),
                    attrs: attrs(text)?,
                    name: name.clone(),
                });
                if PROPS.contains(&split(&name).1) {
                    if empty {
                        elems.push((name, (before, after), None));
                    } else {
                        in_elem = Some((name, before, after));
                    }
                }
            }
            Event::End(_) => {
                if let Some((name, start, content)) = in_elem.take() {
                    elems.push((name, (start, after), Some((content, before))));
                }
            }
            Event::Eof => break,
            _ => {}
        }
    }
    if in_elem.is_some() {
        return Err("unterminated element".to_owned());
    }

    // The prefixes bound to the EXIF namespace, and where.
    let mut prefixes: Vec<String> = Vec::new();
    let mut declared_on: Vec<usize> = Vec::new();
    for (i, t) in tags.iter().enumerate() {
        for a in &t.attrs {
            if let Some(p) = a.name.strip_prefix("xmlns:")
                && &doc[t.span.0 + a.value.0..t.span.0 + a.value.1] == EXIF_NS
            {
                prefixes.push(p.to_owned());
                declared_on.push(i);
            }
        }
    }
    let is_exif = |name: &str| {
        let (p, l) = split(name);
        prefixes.iter().any(|x| x == p) && PROPS.contains(&l)
    };
    let is_desc = |t: &Tag| split(&t.name).1 == "Description";

    let mut found: Vec<(&'static str, Found)> = Vec::new();
    for t in &tags {
        for a in &t.attrs {
            if is_exif(&a.name) {
                let local = split(&a.name).1;
                let prop = PROPS.iter().find(|p| **p == local).copied().unwrap_or("");
                found.push((
                    prop,
                    Found::Attr {
                        whole: (t.span.0 + a.lead, t.span.0 + a.end),
                        value: (t.span.0 + a.value.0, t.span.0 + a.value.1),
                    },
                ));
            }
        }
    }
    for (name, whole, text) in elems {
        if is_exif(&name) {
            let prop = PROPS
                .iter()
                .find(|p| **p == split(&name).1)
                .copied()
                .unwrap_or("");
            found.push((prop, Found::Elem { whole, text, name }));
        }
    }

    // Where new properties go: the Description that declares the prefix, else the first one.
    let target = tags
        .iter()
        .enumerate()
        .find(|(i, t)| is_desc(t) && declared_on.contains(i))
        .or_else(|| tags.iter().enumerate().find(|(_, t)| is_desc(t)))
        .map(|(i, _)| i)
        .ok_or("no rdf:Description")?;
    // A prefix declared on a tag that isn't a Description is on an ancestor (xmpmeta, RDF).
    let usable = declared_on
        .iter()
        .zip(&prefixes)
        .find(|(i, _)| **i == target || !is_desc(&tags[**i]))
        .map(|(_, p)| p.clone());

    let mut edits: Vec<(usize, usize, String)> = Vec::new();
    let mut inserts = String::new();
    let t = &tags[target];
    let sep = t
        .attrs
        .last()
        .map(|a| {
            doc[t.span.0 + a.lead..]
                .chars()
                .take_while(|c| c.is_whitespace())
                .collect::<String>()
        })
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| " ".to_owned());
    let prefix = match &usable {
        Some(p) => p.clone(),
        None => {
            inserts.push_str(&format!("{sep}xmlns:exif=\"{EXIF_NS}\""));
            "exif".to_owned()
        }
    };
    for (prop, value) in values(g) {
        let mut hits = found.iter().filter(|(p, _)| *p == prop);
        let first = hits.next();
        match (first, &value) {
            (Some((_, f)), Some(v)) => edits.push(match f {
                Found::Attr { value, .. } => (value.0, value.1, v.clone()),
                Found::Elem { text: Some(t), .. } => (t.0, t.1, v.clone()),
                Found::Elem {
                    whole,
                    text: None,
                    name,
                } => (whole.0, whole.1, format!("<{name}>{v}</{name}>")),
            }),
            (Some((_, f)), None) => edits.push(remove(f)),
            (None, Some(v)) => inserts.push_str(&format!("{sep}{prefix}:{prop}=\"{v}\"")),
            (None, None) => {}
        }
        // Duplicates of a property are removed.
        for (_, f) in hits {
            edits.push(remove(f));
        }
    }
    if !inserts.is_empty() {
        // After the last attribute (or the name), before any whitespace and the `>` or `/>`.
        let text = &doc[t.span.0..t.span.1];
        let close = if text.ends_with("/>") { 2 } else { 1 };
        let mut at = t.span.1 - close;
        while at > t.span.0 && doc.as_bytes()[at - 1].is_ascii_whitespace() {
            at -= 1;
        }
        edits.push((at, at, inserts));
    }
    edits.sort_by(|a, b| b.0.cmp(&a.0).then(b.1.cmp(&a.1)));
    let mut out = doc.to_owned();
    let mut last = usize::MAX;
    for (a, b, s) in edits {
        if b > last {
            return Err("overlapping GPS properties".to_owned());
        }
        out.replace_range(a..b, &s);
        last = a;
    }
    Ok(out)
}

fn remove(f: &Found) -> (usize, usize, String) {
    match f {
        Found::Attr { whole, .. } | Found::Elem { whole, .. } => (whole.0, whole.1, String::new()),
    }
}

/// The new content for a sidecar: created, or merged into `before` (which keeps its BOM).
/// Checked by reading the position back.
pub fn content(before: Option<&[u8]>, g: &Gps) -> Result<Vec<u8>, String> {
    let out = match before {
        None => create(g).into_bytes(),
        Some(b) => {
            let (bom, rest) = match b.strip_prefix(BOM) {
                Some(r) => (BOM, r),
                None => (&b[..0], b),
            };
            let text = std::str::from_utf8(rest).map_err(|_| "the sidecar isn't UTF-8")?;
            let merged = merge(text, g)?;
            [bom, merged.as_bytes()].concat()
        }
    };
    let text =
        std::str::from_utf8(out.strip_prefix(BOM).unwrap_or(&out)).map_err(|e| e.to_string())?;
    let back = xmp::read_gps(text)?.ok_or("the position doesn't read back")?;
    let close = |a: f64, b: f64| (a - b).abs() < 1e-6;
    if !close(back.lat, g.lat) || !close(back.lon, g.lon) {
        return Err("the position reads back differently".to_owned());
    }
    Ok(out)
}

/// The temporary file beside `path` that is renamed over it; the scan ignores it (it doesn't
/// end in `.xmp`).
fn temp_path(path: &Path) -> PathBuf {
    let name = path
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    path.with_file_name(format!(".{name}.mapcraft-tmp"))
}

/// A file's bytes, `None` if it doesn't exist.
pub fn read(path: &Path) -> Result<Option<Vec<u8>>, String> {
    match std::fs::read(path) {
        Ok(b) => Ok(Some(b)),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e.to_string()),
    }
}

/// Replaces `path` with `bytes` through a temporary file, if it still holds `expect` (another
/// app may have changed it since it was read). `expect: None`: it must not exist.
pub fn replace(path: &Path, expect: Option<&[u8]>, bytes: Option<&[u8]>) -> Result<(), String> {
    if read(path)?.as_deref() != expect {
        return Err("changed by another app since it was read".to_owned());
    }
    let Some(bytes) = bytes else {
        return std::fs::remove_file(path).map_err(|e| e.to_string());
    };
    let tmp = temp_path(path);
    let r = (|| {
        let mut f = std::fs::File::create(&tmp).map_err(|e| e.to_string())?;
        f.write_all(bytes).map_err(|e| e.to_string())?;
        f.sync_all().map_err(|e| e.to_string())?;
        drop(f);
        if read(path)?.as_deref() != expect {
            return Err("changed by another app since it was read".to_owned());
        }
        std::fs::rename(&tmp, path).map_err(|e| e.to_string())
    })();
    if r.is_err() {
        let _ = std::fs::remove_file(&tmp);
    }
    r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn g(alt: Option<f64>) -> Gps {
        Gps {
            lat: 64.135_338,
            lon: -21.895_21,
            alt,
            time: time::parse("2025-06-12T08:00:00Z").unwrap().0,
        }
    }

    #[test]
    fn coordinates_round_and_carry() {
        assert_eq!(coordinate(64.135_338, 'N', 'S'), "64,8.120280N");
        assert_eq!(coordinate(-21.895_21, 'E', 'W'), "21,53.712600W");
        assert_eq!(coordinate(-33.0, 'N', 'S'), "33,0.000000S");
        assert_eq!(coordinate(151.999_999_999_9, 'E', 'W'), "152,0.000000E");
        assert_eq!(coordinate(0.5, 'N', 'S'), "0,30.000000N");
    }

    #[test]
    fn a_new_sidecar_reads_back() {
        let s = create(&g(Some(-12.3)));
        let back = xmp::read_gps(&s).unwrap().unwrap();
        assert!((back.lat - 64.135_338).abs() < 1e-6 && (back.lon + 21.895_21).abs() < 1e-6);
        assert_eq!(back.alt, Some(-12.3));
        assert!(s.contains("exif:GPSTimeStamp=\"2025-06-12T08:00:00Z\""));
        assert!(content(None, &g(None)).is_ok());
    }

    /// Lightroom's style: attributes on the Description, CRLF line ends, other properties.
    const LR: &str = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\" x:xmptk=\"Adobe XMP Core 7.0\">\r\n <rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\">\r\n  <rdf:Description rdf:about=\"\"\r\n    xmlns:xmp=\"http://ns.adobe.com/xap/1.0/\"\r\n    xmlns:crs=\"http://ns.adobe.com/camera-raw-settings/1.0/\"\r\n   xmp:Rating=\"3\"\r\n   crs:Exposure2012=\"+0.35\">\r\n   <crs:ToneCurve><rdf:Seq><rdf:li>0, 0</rdf:li></rdf:Seq></crs:ToneCurve>\r\n  </rdf:Description>\r\n </rdf:RDF>\r\n</x:xmpmeta>\r\n";

    #[test]
    fn merging_adds_gps_and_keeps_every_other_byte() {
        let out = merge(LR, &g(None)).unwrap();
        let added = "\r\n   xmlns:exif=\"http://ns.adobe.com/exif/1.0/\"\r\n   exif:GPSVersionID=\"2.2.0.0\"\r\n   exif:GPSLatitude=\"64,8.120280N\"\r\n   exif:GPSLongitude=\"21,53.712600W\"\r\n   exif:GPSTimeStamp=\"2025-06-12T08:00:00Z\"";
        let at =
            LR.find("crs:Exposure2012=\"+0.35\"").unwrap() + "crs:Exposure2012=\"+0.35\"".len();
        let expected = format!("{}{added}{}", &LR[..at], &LR[at..]);
        assert_eq!(out, expected);
        // Merging again changes only the values, and the result reads back.
        let moved = Gps {
            lat: -33.5,
            lon: 151.25,
            alt: Some(20.0),
            ..g(None)
        };
        let again = merge(&out, &moved).unwrap();
        assert_eq!(again.matches("GPSLatitude").count(), 1);
        let back = xmp::read_gps(&again).unwrap().unwrap();
        assert_eq!((back.lat, back.lon, back.alt), (-33.5, 151.25, Some(20.0)));
        // Without an altitude, an old one is removed.
        let flat = merge(&again, &g(None)).unwrap();
        assert!(!flat.contains("GPSAltitude"));
        assert_eq!(flat, out);
    }

    #[test]
    fn elements_stay_elements_and_another_prefix_is_used() {
        let doc = "<x:xmpmeta xmlns:x=\"adobe:ns:meta/\"><rdf:RDF xmlns:rdf=\"http://www.w3.org/1999/02/22-rdf-syntax-ns#\"><rdf:Description rdf:about=\"\" xmlns:ex=\"http://ns.adobe.com/exif/1.0/\" xmlns:dc=\"http://purl.org/dc/elements/1.1/\">\n<ex:GPSLatitude>1,0.0N</ex:GPSLatitude>\n<ex:GPSLongitude>2,0.0E</ex:GPSLongitude>\n<dc:title>Kept</dc:title></rdf:Description></rdf:RDF></x:xmpmeta>";
        let out = merge(doc, &g(None)).unwrap();
        assert!(out.contains("<ex:GPSLatitude>64,8.120280N</ex:GPSLatitude>"));
        assert!(out.contains("<ex:GPSLongitude>21,53.712600W</ex:GPSLongitude>"));
        assert!(out.contains(" ex:GPSVersionID=\"2.2.0.0\""));
        assert!(out.contains("<dc:title>Kept</dc:title>"));
        assert!(!out.contains("xmlns:exif"));
        let back = xmp::read_gps(&out).unwrap().unwrap();
        assert!((back.lat - 64.135_338).abs() < 1e-6);
    }

    #[test]
    fn a_bom_is_kept_and_unreadable_sidecars_are_refused() {
        let with_bom = [BOM, LR.as_bytes()].concat();
        let out = content(Some(&with_bom), &g(None)).unwrap();
        assert!(out.starts_with(BOM));
        assert!(
            content(Some(b"\xff\xfe<x/>"), &g(None)).is_err(),
            "not UTF-8"
        );
        assert!(
            content(Some(b"<x:xmpmeta><rdf:RDF>"), &g(None)).is_err(),
            "broken"
        );
        assert!(
            content(Some(b"<x:xmpmeta/>"), &g(None)).is_err(),
            "no Description"
        );
    }

    #[test]
    fn replacing_checks_what_is_there() {
        let d = tempfile::tempdir().unwrap();
        let p = d.path().join("IMG_1.xmp");
        replace(&p, None, Some(b"one")).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"one");
        assert!(replace(&p, None, Some(b"two")).is_err(), "it exists now");
        assert!(
            replace(&p, Some(b"other"), Some(b"two")).is_err(),
            "changed meanwhile"
        );
        replace(&p, Some(b"one"), Some(b"two")).unwrap();
        assert_eq!(std::fs::read(&p).unwrap(), b"two");
        replace(&p, Some(b"two"), None).unwrap();
        assert!(!p.exists());
        assert_eq!(
            std::fs::read_dir(d.path()).unwrap().count(),
            0,
            "no temp file left"
        );
    }
}

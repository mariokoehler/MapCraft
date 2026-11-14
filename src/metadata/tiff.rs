//! A small, read-only TIFF/EXIF reader that fetches only what MapCraft needs.
//!
//! Why our own: `kamadak-exif` reads a whole TIFF-based file (DNG, CR2) into memory first, which
//! costs 150–350 ms per RAW over SMB, and fails on a partial read (measured 2026-11-14, design.md
//! §2.8). This reader takes the first 64 KB in one read and seeks only to the few directories and
//! values it needs: the main directory (IFD0), the Exif directory and the GPS directory.
//!
//! The same parser reads the EXIF block of a JPEG (APP1) and of a RAF's embedded JPEG, which are
//! TIFF structures too.

use std::io::{Read, Seek, SeekFrom};

use super::{Gps, Metadata};

/// The first read covers the header and, in practice, every directory we need.
const HEAD: usize = 64 * 1024;
/// Guard against corrupt files: never read more than this for one value.
const MAX_VALUE: usize = 64 * 1024;
/// Guard against loops and corrupt counts.
const MAX_ENTRIES: u16 = 1000;

/// Random access to the bytes of a file, with the first `HEAD` bytes cached.
pub struct Bytes<R> {
    inner: R,
    /// Where the TIFF structure starts within `inner` (0 for a TIFF file, the APP1 payload
    /// offset for a JPEG); all TIFF offsets are relative to it.
    base: u64,
    head: Vec<u8>,
}

impl<R: Read + Seek> Bytes<R> {
    pub fn new(mut inner: R, base: u64) -> std::io::Result<Self> {
        inner.seek(SeekFrom::Start(base))?;
        let mut head = Vec::with_capacity(HEAD);
        (&mut inner).take(HEAD as u64).read_to_end(&mut head)?;
        Ok(Self { inner, base, head })
    }

    /// `len` bytes at TIFF offset `off`.
    fn get(&mut self, off: u64, len: usize) -> Result<Vec<u8>, String> {
        if len > MAX_VALUE {
            return Err("value too large".into());
        }
        let end = off as usize + len;
        if end <= self.head.len() {
            return Ok(self.head[off as usize..end].to_vec());
        }
        let mut buf = vec![0; len];
        self.inner
            .seek(SeekFrom::Start(self.base + off))
            .and_then(|_| self.inner.read_exact(&mut buf))
            .map_err(|e| format!("read at {off}: {e}"))?;
        Ok(buf)
    }
}

#[derive(Clone, Copy)]
enum Order {
    Little,
    Big,
}

impl Order {
    fn u16(self, b: &[u8]) -> u16 {
        let a = [b[0], b[1]];
        match self {
            Self::Little => u16::from_le_bytes(a),
            Self::Big => u16::from_be_bytes(a),
        }
    }
    fn u32(self, b: &[u8]) -> u32 {
        let a = [b[0], b[1], b[2], b[3]];
        match self {
            Self::Little => u32::from_le_bytes(a),
            Self::Big => u32::from_be_bytes(a),
        }
    }
}

/// One directory entry: tag, type, count and either the inline value or its offset.
struct Entry {
    tag: u16,
    typ: u16,
    count: u32,
    raw: [u8; 4],
}

fn type_size(typ: u16) -> Option<usize> {
    match typ {
        1 | 2 | 6 | 7 => Some(1), // BYTE, ASCII, SBYTE, UNDEFINED
        3 | 8 => Some(2),         // SHORT, SSHORT
        4 | 9 | 11 => Some(4),    // LONG, SLONG, FLOAT
        5 | 10 | 12 => Some(8),   // RATIONAL, SRATIONAL, DOUBLE
        _ => None,
    }
}

struct Tiff<'a, R> {
    bytes: &'a mut Bytes<R>,
    order: Order,
}

impl<R: Read + Seek> Tiff<'_, R> {
    fn ifd(&mut self, off: u32) -> Result<Vec<Entry>, String> {
        let n = self.order.u16(&self.bytes.get(off as u64, 2)?);
        if n > MAX_ENTRIES {
            return Err("implausible directory".into());
        }
        let data = self.bytes.get(off as u64 + 2, n as usize * 12)?;
        Ok(data
            .as_chunks::<12>()
            .0
            .iter()
            .map(|e| Entry {
                tag: self.order.u16(&e[0..2]),
                typ: self.order.u16(&e[2..4]),
                count: self.order.u32(&e[4..8]),
                raw: [e[8], e[9], e[10], e[11]],
            })
            .collect())
    }

    fn value(&mut self, e: &Entry) -> Result<Vec<u8>, String> {
        let size = type_size(e.typ).ok_or("unknown type")? * e.count as usize;
        if size <= 4 {
            Ok(e.raw[..size].to_vec())
        } else {
            self.bytes.get(self.order.u32(&e.raw) as u64, size)
        }
    }

    fn ascii(&mut self, e: &Entry) -> Option<String> {
        if e.typ != 2 {
            return None;
        }
        let v = self.value(e).ok()?;
        let s = String::from_utf8_lossy(&v);
        let s = s.trim_end_matches('\0').trim();
        (!s.is_empty()).then(|| s.to_owned())
    }

    fn uint(&mut self, e: &Entry) -> Option<u32> {
        match e.typ {
            3 => Some(self.order.u16(&e.raw) as u32),
            4 => Some(self.order.u32(&e.raw)),
            _ => None,
        }
    }

    fn rationals(&mut self, e: &Entry) -> Option<Vec<f64>> {
        if e.typ != 5 && e.typ != 10 {
            return None;
        }
        let v = self.value(e).ok()?;
        Some(
            v.as_chunks::<8>()
                .0
                .iter()
                .map(|r| {
                    let (n, d) = (self.order.u32(&r[0..4]), self.order.u32(&r[4..8]));
                    let (n, d) = if e.typ == 10 {
                        (n as i32 as f64, d as i32 as f64)
                    } else {
                        (n as f64, d as f64)
                    };
                    if d == 0.0 { f64::NAN } else { n / d }
                })
                .collect(),
        )
    }

    fn find(entries: &[Entry], tag: u16) -> Option<&Entry> {
        entries.iter().find(|e| e.tag == tag)
    }
}

/// A JPEG embedded in a file, ready to be cut out: absolute position and length in the file,
/// and its pixel size (from the JPEG's own SOF marker).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Embedded {
    pub offset: u64,
    pub len: u64,
    pub width: u32,
    pub height: u32,
}

/// The JPEGs embedded in a TIFF structure (CR2, DNG, NEF, ARW…): every directory in the main
/// chain and its SubIFDs, where the image is one JPEG (`JPEGInterchangeFormat` and its length,
/// or a single strip). Lossless JPEGs are the RAW sensor data, not previews (design.md §2.2(b)
/// pitfalls), and are left out.
pub fn embedded<R: Read + Seek>(bytes: &mut Bytes<R>) -> Result<Vec<Embedded>, String> {
    let hdr = bytes.get(0, 8)?;
    let order = match &hdr[0..2] {
        b"II" => Order::Little,
        b"MM" => Order::Big,
        _ => return Err("not a TIFF structure".into()),
    };
    let mut t = Tiff { bytes, order };
    let mut out = Vec::new();
    let mut todo = vec![order.u32(&hdr[4..8])];
    let mut seen = std::collections::HashSet::new();
    while let Some(off) = todo.pop() {
        if off == 0 || !seen.insert(off) || seen.len() > 16 {
            continue;
        }
        let Ok(entries) = t.ifd(off) else { continue };
        // The next directory in the chain follows the entries.
        if let Ok(next) = t.bytes.get(off as u64 + 2 + entries.len() as u64 * 12, 4) {
            todo.push(t.order.u32(&next));
        }
        if let Some(sub) = Tiff::<R>::find(&entries, 0x14A) {
            match sub.count {
                1 => todo.extend(t.uint(sub)),
                n if n <= 32 => {
                    if let Ok(v) = t.value(sub) {
                        todo.extend(v.as_chunks::<4>().0.iter().map(|c| t.order.u32(c)));
                    }
                }
                _ => {}
            }
        }
        let pair = |a: u16, b: u16, t: &mut Tiff<'_, R>| {
            let start = Tiff::<R>::find(&entries, a).filter(|e| e.count == 1)?;
            let len = Tiff::<R>::find(&entries, b).filter(|e| e.count == 1)?;
            Some((t.uint(start)?, t.uint(len)?))
        };
        let Some((start, len)) = pair(0x201, 0x202, &mut t).or_else(|| pair(0x111, 0x117, &mut t))
        else {
            continue;
        };
        if let Some((w, h)) = jpeg_dimensions(t.bytes, start as u64) {
            out.push(Embedded {
                offset: t.bytes.base + start as u64,
                len: len as u64,
                width: w,
                height: h,
            });
        }
    }
    Ok(out)
}

/// The size of a baseline or progressive JPEG at TIFF offset `off`, from its SOF marker; `None`
/// for anything else, lossless JPEGs included.
pub fn jpeg_dimensions<R: Read + Seek>(bytes: &mut Bytes<R>, off: u64) -> Option<(u32, u32)> {
    // Walk the segments by their lengths (a few small reads), rather than scanning a fixed
    // buffer: a camera JPEG's EXIF segment alone can fill the first 64 KB (Fujifilm RAFs).
    if bytes.get(off, 2).ok()? != [0xFF, 0xD8] {
        return None;
    }
    let mut pos = off + 2;
    for _ in 0..64 {
        let hdr = bytes.get(pos, 4).ok()?;
        if hdr[0] != 0xFF {
            return None;
        }
        let m = hdr[1];
        if m == 0xFF {
            pos += 1; // fill byte
            continue;
        }
        let seg = u16::from_be_bytes([hdr[2], hdr[3]]) as u64;
        match m {
            0xC0..=0xC2 => {
                let sof = bytes.get(pos + 5, 4).ok()?;
                let h = u16::from_be_bytes([sof[0], sof[1]]) as u32;
                let w = u16::from_be_bytes([sof[2], sof[3]]) as u32;
                return Some((w, h));
            }
            // Lossless JPEG (RAW sensor data), other coding, or the scan before any SOF.
            0xC3 | 0xC5..=0xC7 | 0xC9..=0xCB | 0xCD..=0xCF | 0xDA | 0xD9 => return None,
            _ => pos += 2 + seg,
        }
    }
    None
}

/// Reads the metadata from a TIFF structure (a TIFF-based file, or an EXIF block).
pub fn read<R: Read + Seek>(bytes: &mut Bytes<R>) -> Result<Metadata, String> {
    let hdr = bytes.get(0, 8)?;
    let order = match &hdr[0..2] {
        b"II" => Order::Little,
        b"MM" => Order::Big,
        _ => return Err("not a TIFF structure".into()),
    };
    let mut t = Tiff { bytes, order };
    let ifd0 = t.ifd(order.u32(&hdr[4..8]))?;
    let mut m = Metadata::default();

    if let Some(e) = Tiff::<R>::find(&ifd0, 0x010F) {
        m.camera_make = t.ascii(e);
    }
    if let Some(e) = Tiff::<R>::find(&ifd0, 0x0110) {
        m.camera_model = t.ascii(e);
    }
    if let Some(e) = Tiff::<R>::find(&ifd0, 0x0112) {
        m.orientation = t.uint(e).map(|v| v as u16);
    }

    if let Some(exif_off) = Tiff::<R>::find(&ifd0, 0x8769).and_then(|e| t.uint(e)) {
        // A broken Exif directory loses the date and lens, not the whole photo.
        if let Ok(exif) = t.ifd(exif_off) {
            if let Some(e) = Tiff::<R>::find(&exif, 0x9003) {
                m.taken_at = t.ascii(e).and_then(|s| exif_datetime(&s));
            }
            if let Some(e) = Tiff::<R>::find(&exif, 0x9011) {
                m.taken_at_offset = t.ascii(e);
            }
            if let Some(e) = Tiff::<R>::find(&exif, 0xA434) {
                m.lens = t.ascii(e);
            }
            let w = Tiff::<R>::find(&exif, 0xA002).and_then(|e| t.uint(e));
            let h = Tiff::<R>::find(&exif, 0xA003).and_then(|e| t.uint(e));
            if let (Some(w), Some(h)) = (w, h) {
                m.width = Some(w);
                m.height = Some(h);
            }
        }
    }

    if let Some(gps_off) = Tiff::<R>::find(&ifd0, 0x8825).and_then(|e| t.uint(e))
        && let Ok(gps) = t.ifd(gps_off)
    {
        m.gps = read_gps(&mut t, &gps);
    }
    Ok(m)
}

fn read_gps<R: Read + Seek>(t: &mut Tiff<'_, R>, gps: &[Entry]) -> Option<Gps> {
    let ref_char = |t: &mut Tiff<'_, R>, tag| {
        Tiff::<R>::find(gps, tag)
            .and_then(|e| t.value(e).ok())
            .and_then(|v| v.first().copied())
    };
    let dms = |t: &mut Tiff<'_, R>, tag| {
        let v = t.rationals(Tiff::<R>::find(gps, tag)?)?;
        match v[..] {
            [d, m, s] => Some(d + m / 60.0 + s / 3600.0),
            [d, m] => Some(d + m / 60.0),
            [d] => Some(d),
            _ => None,
        }
    };
    let lat = dms(t, 2)?;
    let lon = dms(t, 4)?;
    let lat = if ref_char(t, 1) == Some(b'S') {
        -lat
    } else {
        lat
    };
    let lon = if ref_char(t, 3) == Some(b'W') {
        -lon
    } else {
        lon
    };
    let alt = Tiff::<R>::find(gps, 6)
        .and_then(|e| t.rationals(e))
        .and_then(|v| v.first().copied())
        .filter(|a| a.is_finite())
        .map(|a| if ref_char(t, 5) == Some(1) { -a } else { a });
    Gps::new(lat, lon, alt)
}

/// `2023:05:19 21:06:14` to `2023-05-19T21:06:14`.
pub fn exif_datetime(s: &str) -> Option<String> {
    let b = s.as_bytes();
    if b.len() < 19
        || !b[..19].iter().enumerate().all(|(i, c)| match i {
            4 | 7 => *c == b':' || *c == b'-',
            10 => *c == b' ' || *c == b'T',
            13 | 16 => *c == b':',
            _ => c.is_ascii_digit(),
        })
    {
        return None;
    }
    if &s[0..4] == "0000" {
        return None; // "0000:00:00 00:00:00" means "unknown"
    }
    Some(format!(
        "{}-{}-{}T{}",
        &s[0..4],
        &s[5..7],
        &s[8..10],
        &s[11..19]
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_datetimes() {
        assert_eq!(
            exif_datetime("2023:05:19 21:06:14").as_deref(),
            Some("2023-05-19T21:06:14")
        );
        assert_eq!(exif_datetime("0000:00:00 00:00:00"), None);
        assert_eq!(exif_datetime("    :  :     :  :  "), None);
        assert_eq!(exif_datetime(""), None);
    }
}

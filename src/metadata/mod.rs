//! Reading a photo's metadata: position, capture time, camera, lens, size (design.md §2.1(b)).
//!
//! Everything here only *reads* files (design.md §1.1). `read_image` dispatches on the file's
//! first bytes, not its extension:
//! - TIFF-based files (TIFF, DNG, CR2) and JPEGs go through our own `tiff` reader;
//! - a RAF is read through the JPEG embedded in it;
//! - PNG, HEIF and WebP go through `kamadak-exif`.

pub mod tiff;
pub mod xmp;

use std::fs::File;
use std::io::{BufReader, Read, Seek, SeekFrom};
use std::path::Path;

/// A position in decimal degrees (south and west negative), altitude in metres.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Gps {
    pub lat: f64,
    pub lon: f64,
    pub alt: Option<f64>,
}

impl Gps {
    /// `None` for values that can't be a real position. An empty GPS block is common: the tags
    /// exist but hold zeros, which would put the photo at 0°, 0° in the Gulf of Guinea.
    pub fn new(lat: f64, lon: f64, alt: Option<f64>) -> Option<Self> {
        let valid = lat.is_finite()
            && lon.is_finite()
            && lat.abs() <= 90.0
            && lon.abs() <= 180.0
            && !(lat == 0.0 && lon == 0.0);
        valid.then_some(Self { lat, lon, alt })
    }
}

/// What MapCraft records from a photo's metadata. Every field is optional: files differ wildly.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Metadata {
    pub gps: Option<Gps>,
    /// Local capture time, ISO 8601 without zone: `2023-05-19T21:06:14`.
    pub taken_at: Option<String>,
    /// The zone offset, when the camera recorded one: `+02:00`.
    pub taken_at_offset: Option<String>,
    pub camera_make: Option<String>,
    pub camera_model: Option<String>,
    pub lens: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub orientation: Option<u16>,
}

/// Why a file's metadata couldn't be read. The difference matters for a rescan: an I/O error
/// (the NAS dropped, the file is locked) says nothing about the photo, so its record must stay
/// as it is and the file is tried again next time; a format error is a fact about the file.
#[derive(Debug, Clone, PartialEq)]
pub enum ReadError {
    Io(String),
    Format(String),
}

impl std::fmt::Display for ReadError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "can't read the file: {e}"),
            Self::Format(e) => write!(f, "unreadable metadata: {e}"),
        }
    }
}

/// An I/O error, except that running out of bytes means a truncated file: a format problem.
fn io(e: std::io::Error) -> ReadError {
    if e.kind() == std::io::ErrorKind::UnexpectedEof {
        ReadError::Format("the file is truncated".into())
    } else {
        ReadError::Io(e.to_string())
    }
}

/// Reads the metadata embedded in an image file.
pub fn read_image(path: &Path) -> Result<Metadata, ReadError> {
    read_image_from(File::open(path).map_err(io)?)
}

/// Like `read_image`, on a file that's already open (the import opens each original once).
pub fn read_image_from(mut file: File) -> Result<Metadata, ReadError> {
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    let mut magic = [0u8; 16];
    let n = file.read(&mut magic).map_err(io)?;
    let magic = &magic[..n];
    if magic.starts_with(b"II*\0") || magic.starts_with(b"MM\0*") {
        return tiff::read(&mut tiff::Bytes::new(file, 0).map_err(io)?).map_err(ReadError::Format);
    }
    if magic.starts_with(&[0xFF, 0xD8]) {
        return read_jpeg(file, 0);
    }
    if magic.starts_with(b"FUJIFILMCCD-RAW") {
        // RAF: a big-endian pointer at byte 84 to the embedded JPEG, which carries the EXIF.
        let mut ptr = [0u8; 4];
        file.seek(SeekFrom::Start(84))
            .and_then(|_| file.read_exact(&mut ptr))
            .map_err(io)?;
        return read_jpeg(file, u32::from_be_bytes(ptr) as u64);
    }
    file.seek(SeekFrom::Start(0)).map_err(io)?;
    match exif::Reader::new().read_from_container(&mut BufReader::new(file)) {
        Ok(e) => Ok(from_kamadak(&e)),
        // No EXIF at all is normal for PNGs and screenshots: nothing to tell, not an error.
        Err(exif::Error::NotFound(_)) => Ok(Metadata::default()),
        Err(exif::Error::Io(e)) => Err(io(e)),
        Err(e) => Err(ReadError::Format(e.to_string())),
    }
}

/// A JPEG starting at `start`: walk its segments to the EXIF block (APP1 `Exif\0\0`).
fn read_jpeg(mut file: File, start: u64) -> Result<Metadata, ReadError> {
    let mut pos = start + 2;
    for _ in 0..64 {
        // The marker first: the end-of-image marker has no length after it.
        let mut marker = [0u8; 2];
        file.seek(SeekFrom::Start(pos))
            .and_then(|_| file.read_exact(&mut marker))
            .map_err(io)?;
        if marker[0] != 0xFF || marker[1] == 0xDA || marker[1] == 0xD9 {
            break; // not a segment, or the image data starts: no EXIF before it
        }
        let marker = marker[1];
        let mut len = [0u8; 2];
        file.read_exact(&mut len).map_err(io)?;
        let len = u16::from_be_bytes(len) as u64;
        if marker == 0xE1 {
            let mut id = [0u8; 6];
            file.read_exact(&mut id).map_err(io)?;
            if &id == b"Exif\0\0" {
                let base = pos + 4 + 6;
                return tiff::read(&mut tiff::Bytes::new(file, base).map_err(io)?)
                    .map_err(ReadError::Format);
            }
        }
        pos += 2 + len;
    }
    Ok(Metadata::default()) // a JPEG without EXIF is fine: it just has nothing to tell
}

fn from_kamadak(e: &exif::Exif) -> Metadata {
    use exif::{In, Tag, Value};
    let text = |tag| {
        e.get_field(tag, In::PRIMARY).and_then(|f| match &f.value {
            Value::Ascii(v) => v
                .first()
                .map(|s| String::from_utf8_lossy(s).trim().to_owned())
                .filter(|s| !s.is_empty()),
            _ => None,
        })
    };
    let uint = |tag| {
        e.get_field(tag, In::PRIMARY)
            .and_then(|f| f.value.get_uint(0))
    };
    let deg = |tag| {
        e.get_field(tag, In::PRIMARY).and_then(|f| match &f.value {
            Value::Rational(v) if !v.is_empty() => Some(
                v.iter()
                    .zip([1.0, 60.0, 3600.0])
                    .map(|(r, d)| r.to_f64() / d)
                    .sum::<f64>(),
            ),
            _ => None,
        })
    };
    let lat = deg(Tag::GPSLatitude).map(|v| {
        if text(Tag::GPSLatitudeRef).as_deref() == Some("S") {
            -v
        } else {
            v
        }
    });
    let lon = deg(Tag::GPSLongitude).map(|v| {
        if text(Tag::GPSLongitudeRef).as_deref() == Some("W") {
            -v
        } else {
            v
        }
    });
    Metadata {
        gps: lat.zip(lon).and_then(|(a, o)| Gps::new(a, o, None)),
        taken_at: text(Tag::DateTimeOriginal).and_then(|s| tiff::exif_datetime(&s)),
        taken_at_offset: text(Tag::OffsetTimeOriginal),
        camera_make: text(Tag::Make),
        camera_model: text(Tag::Model),
        lens: text(Tag::LensModel),
        width: uint(Tag::PixelXDimension),
        height: uint(Tag::PixelYDimension),
        orientation: uint(Tag::Orientation).map(|v| v as u16),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exif_after_a_jfif_segment_is_found() {
        // Most cameras and phones write APP0 (JFIF) before APP1 (EXIF); the generated demo
        // JPEGs put EXIF first, so this layout needs its own test.
        let with_exif = crate::demo::jpeg_with_gps(-12.5, 130.8, "2024:01:02 03:04:05");
        let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE0, 0x00, 0x10];
        jpeg.extend(b"JFIF\0\x01\x01\0\0\x01\0\x01\0\0");
        jpeg.extend(&with_exif[2..]); // APP1 onwards, without its own SOI
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("jfif.jpg");
        std::fs::write(&path, &jpeg).unwrap();
        let m = read_image(&path).unwrap();
        let g = m.gps.expect("position after APP0");
        assert!((g.lat - -12.5).abs() < 1e-6 && (g.lon - 130.8).abs() < 1e-6);
        // A JPEG with only start and end markers is fine and has nothing to tell.
        std::fs::write(&path, [0xFF, 0xD8, 0xFF, 0xD9]).unwrap();
        assert_eq!(read_image(&path).unwrap(), Metadata::default());
    }

    #[test]
    fn empty_and_impossible_positions_are_none() {
        assert!(Gps::new(0.0, 0.0, None).is_none());
        assert!(Gps::new(91.0, 10.0, None).is_none());
        assert!(Gps::new(f64::NAN, 10.0, None).is_none());
        assert!(Gps::new(-33.9, 151.2, None).is_some()); // south and east
        assert!(Gps::new(40.7, -74.0, None).is_some()); // north and west
    }
}

//! Generated test photos: minimal JPEGs that carry only EXIF (a position and a capture date).
//! Used by the tests and by `mapcraft --make-demo <folder> [count]`, so trials of the GUI never
//! need real photos (CLAUDE.md: test images are our own or generated).

use std::fs;
use std::io;
use std::path::Path;

/// A minimal JPEG (SOI, APP1 EXIF, EOI) with a GPS position and a capture date
/// (`YYYY:MM:DD HH:MM:SS`). Enough for the metadata readers; not a viewable image.
pub fn jpeg_with_gps(lat: f64, lon: f64, date: &str) -> Vec<u8> {
    let tiff = tiff_with_gps(lat, lon, date);
    let mut jpeg = vec![0xFF, 0xD8, 0xFF, 0xE1];
    jpeg.extend(((tiff.len() + 8) as u16).to_be_bytes());
    jpeg.extend(b"Exif\0\0");
    jpeg.extend(tiff);
    jpeg.extend([0xFF, 0xD9]);
    jpeg
}

/// A little-endian TIFF structure: IFD0 → Exif IFD (DateTimeOriginal) and GPS IFD.
pub fn tiff_with_gps(lat: f64, lon: f64, date: &str) -> Vec<u8> {
    fn entry(out: &mut Vec<u8>, tag: u16, typ: u16, count: u32, value: u32) {
        out.extend(tag.to_le_bytes());
        out.extend(typ.to_le_bytes());
        out.extend(count.to_le_bytes());
        out.extend(value.to_le_bytes());
    }
    fn dms(v: f64) -> [(u32, u32); 3] {
        let v = v.abs();
        let d = v.floor();
        let m = ((v - d) * 60.0).floor();
        let s = ((v - d) * 60.0 - m) * 60.0;
        [
            (d as u32, 1),
            (m as u32, 1),
            ((s * 1000.0).round() as u32, 1000),
        ]
    }
    // Layout: header 8 | IFD0 (2 entries) @8 | Exif IFD (1) @38 | GPS IFD (4) @56 | data @110
    let (ifd0, exif, gps, data) = (8u32, 38u32, 56u32, 110u32);
    let mut t = b"II*\0".to_vec();
    t.extend(ifd0.to_le_bytes());
    t.extend(2u16.to_le_bytes());
    entry(&mut t, 0x8769, 4, 1, exif);
    entry(&mut t, 0x8825, 4, 1, gps);
    t.extend(0u32.to_le_bytes());
    t.extend(1u16.to_le_bytes());
    entry(&mut t, 0x9003, 2, 20, data); // DateTimeOriginal, 19 chars + NUL
    t.extend(0u32.to_le_bytes());
    t.extend(4u16.to_le_bytes());
    let ns = if lat < 0.0 { b'S' } else { b'N' };
    let ew = if lon < 0.0 { b'W' } else { b'E' };
    entry(&mut t, 1, 2, 2, u32::from_le_bytes([ns, 0, 0, 0]));
    entry(&mut t, 2, 5, 3, data + 20);
    entry(&mut t, 3, 2, 2, u32::from_le_bytes([ew, 0, 0, 0]));
    entry(&mut t, 4, 5, 3, data + 44);
    t.extend(0u32.to_le_bytes());
    debug_assert_eq!(t.len() as u32, data);
    t.extend(date.as_bytes());
    t.push(0);
    for (n, d) in dms(lat).into_iter().chain(dms(lon)) {
        t.extend(n.to_le_bytes());
        t.extend(d.to_le_bytes());
    }
    t
}

/// Places for the demo folders: (folder name, lat, lon). Invented trips, real cities.
const TRIPS: [(&str, f64, f64); 8] = [
    ("2019-05-04_Lisbon", 38.7223, -9.1393),
    ("2020-08-15_Iceland", 64.1466, -21.9426),
    ("2021-03-20_Rome", 41.9028, 12.4964),
    ("2022-07-02_Norway", 59.9139, 10.7522),
    ("2023-05-19_Hamburg", 53.5511, 9.9937),
    ("2023-10-01_Vienna", 48.2082, 16.3738),
    ("2024-06-12_Edinburgh", 55.9533, -3.1883),
    ("2025-09-09_Barcelona", 41.3874, 2.1686),
];

/// Writes `count` demo photos into `folder`, spread over the trips' folders; about one in eight
/// has no position. A deterministic scatter, so runs are comparable.
pub fn make_demo(folder: &Path, count: usize) -> io::Result<()> {
    let mut seed: u64 = 0x5eed;
    let mut next = move || {
        seed ^= seed << 13;
        seed ^= seed >> 7;
        seed ^= seed << 17;
        (seed % 10_000) as f64 / 10_000.0
    };
    for i in 0..count {
        let (trip, lat, lon) = TRIPS[i % TRIPS.len()];
        let dir = folder.join(trip);
        fs::create_dir_all(&dir)?;
        let date = format!(
            "{}:{:02}:{:02} 12:{:02}:00",
            &trip[..4],
            &trip[5..7],
            &trip[8..10],
            i % 60
        );
        let bytes = if i % 8 == 7 {
            vec![0xFF, 0xD8, 0xFF, 0xD9] // a JPEG without EXIF: no position
        } else {
            jpeg_with_gps(
                lat + (next() - 0.5) * 0.2,
                lon + (next() - 0.5) * 0.3,
                &date,
            )
        };
        fs::write(dir.join(format!("IMG_{i:05}.jpg")), bytes)?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_photos_are_read_back_with_their_positions() {
        let dir = tempfile::tempdir().unwrap();
        make_demo(dir.path(), 16).unwrap();
        let first = dir.path().join("2019-05-04_Lisbon").join("IMG_00000.jpg");
        let m = crate::metadata::read_image(&first).unwrap();
        let g = m.gps.unwrap();
        assert!((g.lat - 38.72).abs() < 0.2 && (g.lon - -9.14).abs() < 0.2);
        assert_eq!(m.taken_at.as_deref(), Some("2019-05-04T12:00:00"));
        let no_gps = dir
            .path()
            .join("2025-09-09_Barcelona")
            .join("IMG_00007.jpg");
        assert!(crate::metadata::read_image(&no_gps).unwrap().gps.is_none());
    }
}

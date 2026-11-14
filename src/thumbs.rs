//! Thumbnails and previews (design.md §2.2): a 256 px thumbnail for the map and a 1024 px preview
//! for the detail panel, as JPEG files in the data folder, never next to the originals.
//!
//! Sources, fastest first (§2.2(b)):
//! 1. a RAW's embedded JPEG preview: the smallest one that is large enough;
//! 2. the JPEG beside a RAW (§2.8(c));
//! 3. the original itself: JPEGs decode at reduced scale (a JPEG can be decoded at 1/2, 1/4 or
//!    1/8 size almost for free), PNG, TIFF and WebP decode in full, within size limits checked
//!    *before* decoding, because running out of memory can't be caught.
//!
//! Orientation comes from the file that supplied the pixels. Everything here only reads the
//! originals (design.md §1.1).

use std::fs::File;
use std::io::{Cursor, Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};

use image::imageops::{self, FilterType};
use image::{DynamicImage, RgbImage};

use crate::metadata::{self, ReadError};

/// The map's thumbnails: long edge in px.
pub const THUMB: u32 = 256;
/// The detail panel's previews: long edge in px.
pub const PREVIEW: u32 = 1024;
/// JPEG quality of both (design.md §2.2(a), user-confirmed).
const QUALITY: u8 = 80;
/// A file larger than this isn't decoded in full (a 2.5 GB TIFF would need gigabytes).
const MAX_FILE: u64 = 400 * 1024 * 1024;
/// Nor an image with more pixels than this.
const MAX_PIXELS: u64 = 120_000_000;
/// The JPEG decoder's memory cap.
const MAX_DECODE_BYTES: usize = 512 * 1024 * 1024;

/// File types decoded in-process when there's no embedded preview.
const DECODABLE: [&str; 6] = ["jpg", "jpeg", "png", "tif", "tiff", "webp"];

#[derive(Debug, Clone, PartialEq)]
pub enum ThumbError {
    /// A file couldn't be opened or read (NAS hiccup, locked): try again later.
    Io(String),
    /// No source MapCraft can use (unsupported format, too large, damaged): a placeholder.
    Unusable(String),
}

impl std::fmt::Display for ThumbError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Io(e) => write!(f, "can't read the file: {e}"),
            Self::Unusable(e) => write!(f, "no usable image: {e}"),
        }
    }
}

fn io(e: std::io::Error) -> ThumbError {
    ThumbError::Io(e.to_string())
}

fn read_err(e: ReadError) -> ThumbError {
    match e {
        ReadError::Io(e) => ThumbError::Io(e),
        ReadError::Format(e) => ThumbError::Unusable(e),
    }
}

/// Which source a thumbnail came from (for tests and measurements).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Source {
    Embedded,
    PairedJpeg,
    Original,
}

/// A photo's files, as the library records them.
pub struct PhotoFiles<'a> {
    pub dir: &'a Path,
    pub original: &'a str,
    pub preview: Option<&'a str>,
    /// The original's EXIF orientation (it applies to its embedded previews too).
    pub orientation: Option<u16>,
}

/// Renders the photo at `long_edge` px as a JPEG.
pub fn render(files: &PhotoFiles<'_>, long_edge: u32) -> Result<(Vec<u8>, Source), ThumbError> {
    let (img, source) = pixels(files, long_edge)?;
    Ok((encode(&fit(img, long_edge)), source))
}

/// The best source's pixels, already turned upright, at least `long_edge` px where possible.
pub fn pixels(files: &PhotoFiles<'_>, long_edge: u32) -> Result<(RgbImage, Source), ThumbError> {
    let original = files.dir.join(files.original);
    let ext = extension(files.original);

    // 1. An embedded preview (RAWs).
    let mut file = File::open(&original).map_err(io)?;
    let previews = metadata::embedded_previews(&mut file).map_err(read_err)?;
    if let Some(p) = choose(&previews, long_edge) {
        let mut bytes = vec![0; p.len as usize];
        file.seek(SeekFrom::Start(p.offset)).map_err(io)?;
        file.read_exact(&mut bytes).map_err(io)?;
        let img = decode_jpeg(&bytes, long_edge)?;
        return Ok((orient(img, files.orientation), Source::Embedded));
    }
    drop(file);

    // 2. The JPEG beside a RAW.
    if let Some(name) = files.preview {
        let path = files.dir.join(name);
        let orientation = metadata::read_image(&path).map_err(read_err)?.orientation;
        let bytes = read_all(&path)?;
        let img = decode_jpeg(&bytes, long_edge)?;
        return Ok((orient(img, orientation), Source::PairedJpeg));
    }

    // 3. The original itself.
    if !DECODABLE.contains(&ext.as_str()) {
        return Err(ThumbError::Unusable(format!(
            "{ext} files need LightCraft or PhotoCraft"
        )));
    }
    let size = std::fs::metadata(&original).map_err(io)?.len();
    if size > MAX_FILE {
        return Err(ThumbError::Unusable(format!(
            "too large to decode ({} MB)",
            size >> 20
        )));
    }
    let bytes = read_all(&original)?;
    let img = if ext == "jpg" || ext == "jpeg" {
        decode_jpeg(&bytes, long_edge)?
    } else {
        decode_other(&bytes, long_edge)?
    };
    Ok((orient(img, files.orientation), Source::Original))
}

fn extension(name: &str) -> String {
    name.rsplit_once('.')
        .map(|(_, e)| e.to_lowercase())
        .unwrap_or_default()
}

fn read_all(path: &Path) -> Result<Vec<u8>, ThumbError> {
    std::fs::read(path).map_err(io)
}

/// The smallest embedded preview whose long edge is at least `long_edge`, else the largest one
/// that still makes a usable thumbnail (at least 256 px).
fn choose(
    previews: &[metadata::tiff::Embedded],
    long_edge: u32,
) -> Option<metadata::tiff::Embedded> {
    let edge = |p: &metadata::tiff::Embedded| p.width.max(p.height);
    previews
        .iter()
        .filter(|p| edge(p) >= long_edge)
        .min_by_key(|p| edge(p))
        .or_else(|| {
            previews
                .iter()
                .filter(|p| edge(p) >= THUMB)
                .max_by_key(|p| edge(p))
        })
        .copied()
}

/// Decodes a JPEG at the smallest scale (1/8 … 1) that keeps its long edge at least `long_edge`.
pub fn decode_jpeg(bytes: &[u8], long_edge: u32) -> Result<RgbImage, ThumbError> {
    let bad = |e: jpeg_decoder::Error| ThumbError::Unusable(format!("JPEG: {e}"));
    let mut d = jpeg_decoder::Decoder::new(Cursor::new(bytes));
    d.set_max_decoding_buffer_size(MAX_DECODE_BYTES);
    d.read_info().map_err(bad)?;
    let info = d
        .info()
        .ok_or_else(|| ThumbError::Unusable("JPEG without a header".into()))?;
    if info.width as u64 * info.height as u64 > MAX_PIXELS * 4 {
        return Err(ThumbError::Unusable("JPEG too large".into()));
    }
    // `scale` picks the smallest scale whose size is at least the requested one.
    let long = info.width.max(info.height) as f32;
    let factor = (long_edge as f32 / long).min(1.0);
    let req = |v: u16| ((v as f32 * factor).ceil() as u16).max(1);
    let (w, h) = d.scale(req(info.width), req(info.height)).map_err(bad)?;
    let data = d.decode().map_err(bad)?;
    let (w, h) = (w as u32, h as u32);
    let rgb: Vec<u8> = match info.pixel_format {
        jpeg_decoder::PixelFormat::RGB24 => data,
        jpeg_decoder::PixelFormat::L8 => data.iter().flat_map(|&v| [v, v, v]).collect(),
        jpeg_decoder::PixelFormat::L16 => data
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|c| {
                let v = c[0]; // big-endian: the high byte
                [v, v, v]
            })
            .collect(),
        jpeg_decoder::PixelFormat::CMYK32 => data
            .as_chunks::<4>()
            .0
            .iter()
            .flat_map(|c| {
                // Adobe CMYK JPEGs are stored inverted; a plain conversion is enough for a
                // thumbnail.
                let k = c[3] as u16;
                [c[0], c[1], c[2]].map(|v| ((v as u16 * k) / 255) as u8)
            })
            .collect(),
    };
    RgbImage::from_raw(w, h, rgb).ok_or_else(|| ThumbError::Unusable("JPEG size mismatch".into()))
}

/// PNG, TIFF, WebP: checked against the limits before decoding, then converted to sRGB from
/// the colour space their embedded ICC profile names, and shrunk to `long_edge` in linear light.
///
/// Lightroom exports TIFFs in ProPhoto RGB (16-bit, gamma 1.8, or 32-bit float linear for HDR
/// merges) and Adobe RGB; shown as if they were sRGB they come out far too dark or dull (measured
/// on the user's library, 2026-11-14: a linear ProPhoto TIFF averaged 36/255, its CR2 100/255).
fn decode_other(bytes: &[u8], long_edge: u32) -> Result<RgbImage, ThumbError> {
    use image::ImageDecoder;
    let bad = |e: image::ImageError| ThumbError::Unusable(e.to_string());
    let reader = || -> Result<image::ImageReader<Cursor<&[u8]>>, ThumbError> {
        let mut r = image::ImageReader::new(Cursor::new(bytes))
            .with_guessed_format()
            .map_err(|e| ThumbError::Unusable(e.to_string()))?;
        let mut limits = image::Limits::default();
        limits.max_alloc = Some(MAX_DECODE_BYTES as u64 * 2);
        r.limits(limits);
        Ok(r)
    };
    let mut decoder = reader()?.into_decoder().map_err(bad)?;
    let (w, h) = decoder.dimensions();
    if w as u64 * h as u64 > MAX_PIXELS {
        return Err(ThumbError::Unusable(format!("too many pixels ({w} × {h})")));
    }
    let space = decoder
        .icc_profile()
        .ok()
        .flatten()
        .map_or(Space::Srgb, |icc| Space::from_icc(&icc));
    let img = DynamicImage::from_decoder(decoder).map_err(bad)?;
    Ok(to_srgb(img.to_rgb32f(), space, long_edge))
}

/// The colour spaces MapCraft converts from; anything else is taken as sRGB.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Space {
    Srgb,
    AdobeRgb,
    ProPhoto,
    ProPhotoLinear,
}

impl Space {
    /// From an ICC profile's description (`desc` in v2 profiles, `mluc` in v4).
    pub fn from_icc(icc: &[u8]) -> Self {
        let desc = icc_description(icc).unwrap_or_default().to_lowercase();
        if desc.contains("prophoto") || desc.contains("romm") {
            if desc.contains("linear") {
                Self::ProPhotoLinear
            } else {
                Self::ProPhoto
            }
        } else if desc.contains("adobe rgb") {
            Self::AdobeRgb
        } else {
            Self::Srgb
        }
    }

    /// Encoded value → linear light.
    fn linearize(self, v: f32) -> f32 {
        match self {
            Self::Srgb => {
                if v <= 0.04045 {
                    v / 12.92
                } else {
                    ((v + 0.055) / 1.055).powf(2.4)
                }
            }
            Self::AdobeRgb => v.max(0.0).powf(563.0 / 256.0),
            Self::ProPhoto => {
                if v < 16.0 / 512.0 {
                    v / 16.0
                } else {
                    v.powf(1.8)
                }
            }
            Self::ProPhotoLinear => v,
        }
    }

    /// Linear RGB in this space → linear sRGB (D65). ProPhoto is D50-based; the matrix includes
    /// the Bradford adaptation.
    fn to_srgb_matrix(self) -> [[f32; 3]; 3] {
        match self {
            Self::Srgb => [[1.0, 0.0, 0.0], [0.0, 1.0, 0.0], [0.0, 0.0, 1.0]],
            Self::AdobeRgb => [
                [1.398_355_7, -0.398_355_7, 0.0],
                [0.0, 1.0, 0.0],
                [0.0, -0.042_928_9, 1.042_928_9],
            ],
            Self::ProPhoto | Self::ProPhotoLinear => [
                [2.034_192_6, -0.727_419_8, -0.306_772_8],
                [-0.228_878, 1.231_744, -0.002_866],
                [-0.008_555_8, -0.153_264_5, 1.161_820_3],
            ],
        }
    }
}

fn icc_description(icc: &[u8]) -> Option<String> {
    let u32_at = |i: usize| {
        icc.get(i..i + 4)
            .map(|b| u32::from_be_bytes([b[0], b[1], b[2], b[3]]))
    };
    let count = u32_at(128)? as usize;
    for k in 0..count.min(64) {
        let at = 132 + 12 * k;
        if icc.get(at..at + 4)? != b"desc" {
            continue;
        }
        let (off, len) = (u32_at(at + 4)? as usize, u32_at(at + 8)? as usize);
        let tag = icc.get(off..off + len)?;
        return match tag.get(..4)? {
            b"desc" => {
                let n = u32::from_be_bytes(tag.get(8..12)?.try_into().ok()?) as usize;
                Some(
                    String::from_utf8_lossy(tag.get(12..12 + n)?)
                        .trim_end_matches(' ')
                        .to_owned(),
                )
            }
            b"mluc" => {
                let (len, start) = (
                    u32::from_be_bytes(tag.get(20..24)?.try_into().ok()?) as usize,
                    u32::from_be_bytes(tag.get(24..28)?.try_into().ok()?) as usize,
                );
                let units: Vec<u16> = tag
                    .get(start..start + len)?
                    .as_chunks::<2>()
                    .0
                    .iter()
                    .map(|c| u16::from_be_bytes([c[0], c[1]]))
                    .collect();
                Some(String::from_utf16_lossy(&units))
            }
            _ => None,
        };
    }
    None
}

/// Linearizes, shrinks to `long_edge` (in linear light), converts to sRGB and encodes to 8 bits.
fn to_srgb(img: image::Rgb32FImage, space: Space, long_edge: u32) -> RgbImage {
    let mut lin = img;
    for p in lin.pixels_mut() {
        for c in p.0.iter_mut() {
            *c = space.linearize(*c);
        }
    }
    let (w, h) = lin.dimensions();
    let s = (long_edge as f32 / w.max(h) as f32).min(1.0);
    let small = if s < 1.0 {
        let (nw, nh) = (
            ((w as f32 * s).round() as u32).max(1),
            ((h as f32 * s).round() as u32).max(1),
        );
        imageops::resize(&lin, nw, nh, FilterType::Triangle)
    } else {
        lin
    };
    let m = space.to_srgb_matrix();
    let encode = |v: f32| {
        let v = v.clamp(0.0, 1.0);
        let e = if v <= 0.003_130_8 {
            v * 12.92
        } else {
            1.055 * v.powf(1.0 / 2.4) - 0.055
        };
        (e * 255.0 + 0.5) as u8
    };
    RgbImage::from_fn(small.width(), small.height(), |x, y| {
        let [r, g, b] = small.get_pixel(x, y).0;
        let out = [0, 1, 2].map(|i| encode(m[i][0] * r + m[i][1] * g + m[i][2] * b));
        image::Rgb(out)
    })
}

/// Applies an EXIF orientation (1–8), so the image is stored upright (§2.2(a)).
pub fn orient(img: RgbImage, orientation: Option<u16>) -> RgbImage {
    match orientation.unwrap_or(1) {
        2 => imageops::flip_horizontal(&img),
        3 => imageops::rotate180(&img),
        4 => imageops::flip_vertical(&img),
        5 => imageops::flip_horizontal(&imageops::rotate90(&img)),
        6 => imageops::rotate90(&img),
        7 => imageops::flip_horizontal(&imageops::rotate270(&img)),
        8 => imageops::rotate270(&img),
        _ => img,
    }
}

/// Scales down so the long edge is `long_edge` (never up).
pub fn fit(img: RgbImage, long_edge: u32) -> RgbImage {
    let (w, h) = img.dimensions();
    let long = w.max(h);
    if long <= long_edge {
        return img;
    }
    let s = long_edge as f32 / long as f32;
    let (nw, nh) = (
        ((w as f32 * s).round() as u32).max(1),
        ((h as f32 * s).round() as u32).max(1),
    );
    imageops::resize(&img, nw, nh, FilterType::Triangle)
}

pub fn encode(img: &RgbImage) -> Vec<u8> {
    let mut out = Vec::new();
    let mut enc = image::codecs::jpeg::JpegEncoder::new_with_quality(&mut out, QUALITY);
    enc.encode_image(img)
        .expect("encoding an RGB image to memory can't fail");
    out
}

/// Where a photo's thumbnail (`THUMB`) or preview (`PREVIEW`) lives in the data folder,
/// sharded so no folder holds more than a few thousand files (§3.3):
/// `thumbs/3a/07/1234_256.jpg`.
pub fn path(data: &Path, photo_id: i64, long_edge: u32) -> PathBuf {
    let h = (photo_id as u64).wrapping_mul(0x9E37_79B9_7F4A_7C15);
    let kind = if long_edge == THUMB {
        "thumbs"
    } else {
        "previews"
    };
    data.join(kind)
        .join(format!("{:02x}", (h >> 56) as u8))
        .join(format!("{:02x}", (h >> 48) as u8))
        .join(format!("{photo_id}_{long_edge}.jpg"))
}

/// Deletes the thumbnail and preview files of photos that no longer exist: they are the user's
/// images and must go with their records. Returns how many files were removed.
pub fn sweep(data: &Path, live: &std::collections::HashSet<i64>) -> usize {
    let mut removed = 0;
    for kind in ["thumbs", "previews"] {
        let Ok(level1) = std::fs::read_dir(data.join(kind)) else {
            continue;
        };
        for a in level1.flatten() {
            let Ok(level2) = std::fs::read_dir(a.path()) else {
                continue;
            };
            for b in level2.flatten() {
                let Ok(files) = std::fs::read_dir(b.path()) else {
                    continue;
                };
                for f in files.flatten() {
                    let name = f.file_name().to_string_lossy().into_owned();
                    let id = name.split('_').next().and_then(|i| i.parse::<i64>().ok());
                    let stale = name.ends_with(".tmp") || id.is_some_and(|i| !live.contains(&i));
                    if stale && std::fs::remove_file(f.path()).is_ok() {
                        removed += 1;
                    }
                }
            }
        }
    }
    removed
}

/// Writes a rendered image to its place, via a temporary file and a rename, so a cancelled job
/// never leaves half a file behind.
pub fn store(target: &Path, jpeg: &[u8]) -> std::io::Result<()> {
    if let Some(dir) = target.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let tmp = target.with_extension("jpg.tmp");
    std::fs::write(&tmp, jpeg)?;
    std::fs::rename(&tmp, target)
}

/// Needs a full decode (TIFF, PNG, WebP originals without a JPEG beside them): such decodes run
/// one at a time, since one can take a gigabyte of memory.
pub fn is_heavy(original: &str, preview: Option<&str>) -> bool {
    let ext = extension(original);
    preview.is_none() && matches!(ext.as_str(), "tif" | "tiff" | "png" | "webp")
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A 40 × 20 image, red on the left half and blue on the right: a wrong rotation shows.
    fn asymmetric() -> RgbImage {
        RgbImage::from_fn(40, 20, |x, _| {
            if x < 20 {
                image::Rgb([255, 0, 0])
            } else {
                image::Rgb([0, 0, 255])
            }
        })
    }

    #[test]
    fn orientation_8_turns_the_image_upright() {
        // Orientation 8: the camera was turned 90° clockwise; the image must turn back
        // counter-clockwise, so the left (red) half ends up at the bottom.
        let o = orient(asymmetric(), Some(8));
        assert_eq!(o.dimensions(), (20, 40), "portrait");
        assert_eq!(o.get_pixel(10, 35).0, [255, 0, 0], "red at the bottom");
        let six = orient(asymmetric(), Some(6));
        assert_eq!(
            six.get_pixel(10, 5).0,
            [255, 0, 0],
            "orientation 6: red at the top"
        );
    }

    #[test]
    fn jpegs_decode_at_reduced_scale_and_fit() {
        let big = RgbImage::from_fn(2048, 1024, |x, y| {
            image::Rgb([(x % 256) as u8, (y % 256) as u8, 0])
        });
        let jpeg = encode(&big);
        let small = decode_jpeg(&jpeg, 256).unwrap();
        assert_eq!(
            small.dimensions(),
            (256, 128),
            "1/8 scale is exactly enough"
        );
        let mid = decode_jpeg(&jpeg, 300).unwrap();
        assert_eq!(
            mid.dimensions(),
            (512, 256),
            "1/4 scale: the next one that is large enough"
        );
        assert_eq!(fit(mid, 256).dimensions(), (256, 128));
    }

    #[test]
    fn rendering_a_jpeg_original_and_refusing_unknown_formats() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.jpg"), encode(&asymmetric())).unwrap();
        std::fs::write(dir.path().join("b.psd"), b"8BPS").unwrap();
        let files = PhotoFiles {
            dir: dir.path(),
            original: "a.jpg",
            preview: None,
            orientation: None,
        };
        let (jpeg, source) = render(&files, THUMB).unwrap();
        assert_eq!(source, Source::Original);
        assert_eq!(
            decode_jpeg(&jpeg, 1024).unwrap().dimensions(),
            (40, 20),
            "never scaled up"
        );
        let psd = PhotoFiles {
            original: "b.psd",
            ..files
        };
        assert!(matches!(render(&psd, THUMB), Err(ThumbError::Unusable(_))));
        let gone = PhotoFiles {
            original: "missing.jpg",
            ..psd
        };
        assert!(matches!(render(&gone, THUMB), Err(ThumbError::Io(_))));
    }

    /// A minimal ICC profile with a v2 `desc` tag.
    fn icc(desc: &str) -> Vec<u8> {
        let mut icc = vec![0u8; 128];
        icc.extend(1u32.to_be_bytes()); // one tag
        icc.extend(b"desc");
        icc.extend(144u32.to_be_bytes());
        let mut tag = b"desc    ".to_vec();
        tag.extend((desc.len() as u32 + 1).to_be_bytes());
        tag.extend(desc.as_bytes());
        tag.push(0);
        icc.extend((tag.len() as u32).to_be_bytes());
        icc.extend(tag);
        icc
    }

    #[test]
    fn colour_spaces_are_recognised_and_grey_stays_grey() {
        assert_eq!(
            Space::from_icc(&icc("ProPhoto RGB (Linear RGB Profile)")),
            Space::ProPhotoLinear
        );
        assert_eq!(Space::from_icc(&icc("ProPhoto RGB")), Space::ProPhoto);
        assert_eq!(Space::from_icc(&icc("Adobe RGB (1998)")), Space::AdobeRgb);
        assert_eq!(Space::from_icc(&icc("sRGB IEC61966-2.1")), Space::Srgb);
        assert_eq!(Space::from_icc(b"garbage"), Space::Srgb);
        // 18 % grey in linear light is sRGB 118 in every space, and stays neutral.
        let grey = |space: Space, encoded: f32| {
            let img = image::Rgb32FImage::from_pixel(4, 4, image::Rgb([encoded; 3]));
            to_srgb(img, space, 256).get_pixel(0, 0).0
        };
        for (space, v) in [
            (Space::ProPhotoLinear, 0.18),
            (Space::ProPhoto, 0.18f32.powf(1.0 / 1.8)),
            (Space::AdobeRgb, 0.18f32.powf(256.0 / 563.0)),
        ] {
            let [r, g, b] = grey(space, v);
            assert!(
                (r as i32 - 118).abs() <= 1 && r == g && g == b,
                "{space:?}: {r} {g} {b}"
            );
        }
    }

    #[test]
    fn sweeping_removes_only_the_files_of_deleted_photos() {
        let dir = tempfile::tempdir().unwrap();
        for (id, size) in [(1, THUMB), (2, THUMB), (2, PREVIEW)] {
            store(&path(dir.path(), id, size), b"x").unwrap();
        }
        std::fs::write(
            path(dir.path(), 1, THUMB).with_extension("jpg.tmp"),
            b"half",
        )
        .unwrap();
        let live = std::collections::HashSet::from([1]);
        assert_eq!(
            sweep(dir.path(), &live),
            3,
            "photo 2's two files and the leftover .tmp"
        );
        assert!(path(dir.path(), 1, THUMB).exists());
        assert!(!path(dir.path(), 2, PREVIEW).exists());
    }

    #[test]
    fn thumbnail_paths_are_sharded_and_distinct() {
        let d = Path::new("D");
        assert_ne!(path(d, 1, THUMB), path(d, 2, THUMB));
        assert_ne!(path(d, 1, THUMB), path(d, 1, PREVIEW));
        assert!(path(d, 1234, THUMB).ends_with("1234_256.jpg"));
        assert_eq!(path(d, 7, THUMB).components().count(), 5);
    }
}

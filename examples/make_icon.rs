//! Makes the app icons from the master image (design.md M7):
//! `cargo run --release --example make_icon`
//!
//! Reads `assets/brand/mapcraft-icon-1024.png` (the user's icon, square, transparent corners)
//! and writes, beside it:
//! - `mapcraft.ico`: every size Windows asks for (Explorer, taskbar, Alt+Tab, title bar);
//!   `build.rs` puts it into the exe. Sizes below 256 are classic 32-bit bitmaps, 256 is PNG:
//!   that's the layout every Windows API reads (some misread small PNG entries as noise);
//! - `mapcraft-icon-256.png`: the window's own icon, which the app sets at start.
//!
//! Run it again whenever the master changes, and commit the results.

use std::io::Cursor;
use std::path::Path;

use image::imageops::FilterType;
use image::{ImageFormat, RgbaImage};

const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn main() {
    let dir = Path::new("assets/brand");
    let master = image::open(dir.join("mapcraft-icon-1024.png"))
        .expect("assets/brand/mapcraft-icon-1024.png")
        .to_rgba8();
    assert_eq!(master.width(), master.height(), "the master must be square");

    let pngs: Vec<(u32, Vec<u8>)> = SIZES
        .iter()
        .map(|&s| {
            let img = image::imageops::resize(&master, s, s, FilterType::Lanczos3);
            (s, if s >= 256 { png(&img) } else { dib(&img) })
        })
        .collect();

    // ICO: a 6-byte header, a 16-byte entry per image, then the images.
    let mut ico = Vec::new();
    ico.extend_from_slice(&[0, 0, 1, 0]);
    ico.extend_from_slice(&(pngs.len() as u16).to_le_bytes());
    let mut offset = 6 + 16 * pngs.len() as u32;
    for (size, data) in &pngs {
        let side = if *size >= 256 { 0 } else { *size as u8 }; // 0 means 256
        ico.extend_from_slice(&[side, side, 0, 0]);
        ico.extend_from_slice(&1u16.to_le_bytes()); // planes
        ico.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
        ico.extend_from_slice(&(data.len() as u32).to_le_bytes());
        ico.extend_from_slice(&offset.to_le_bytes());
        offset += data.len() as u32;
    }
    for (_, data) in &pngs {
        ico.extend_from_slice(data);
    }
    std::fs::write(dir.join("mapcraft.ico"), &ico).expect("write mapcraft.ico");
    let big = &pngs.last().expect("sizes").1; // 256 px, PNG
    std::fs::write(dir.join("mapcraft-icon-256.png"), big).expect("write the 256 px PNG");
    println!(
        "wrote mapcraft.ico ({} sizes, {} KB) and mapcraft-icon-256.png",
        SIZES.len(),
        ico.len() / 1024
    );
}

fn png(img: &RgbaImage) -> Vec<u8> {
    let mut out = Cursor::new(Vec::new());
    img.write_to(&mut out, ImageFormat::Png)
        .expect("encode PNG");
    out.into_inner()
}

/// An icon entry as a classic bitmap: a BITMAPINFOHEADER whose height counts the image and its
/// mask, the pixels bottom-up as BGRA, then a 1-bit AND mask (all zero: the alpha decides).
fn dib(img: &RgbaImage) -> Vec<u8> {
    let (w, h) = img.dimensions();
    let mut out = Vec::new();
    for v in [40u32, w, 2 * h] {
        out.extend_from_slice(&v.to_le_bytes());
    }
    out.extend_from_slice(&1u16.to_le_bytes()); // planes
    out.extend_from_slice(&32u16.to_le_bytes()); // bits per pixel
    out.extend_from_slice(&[0; 24]); // no compression, sizes and palette left to the reader
    for y in (0..h).rev() {
        for x in 0..w {
            let [r, g, b, a] = img.get_pixel(x, y).0;
            out.extend_from_slice(&[b, g, r, a]);
        }
    }
    let mask_row = w.div_ceil(32) * 4; // 1 bit per pixel, rows padded to 4 bytes
    out.resize(out.len() + (mask_row * h) as usize, 0);
    out
}

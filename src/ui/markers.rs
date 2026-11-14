//! Photo markers on the map (design.md §2.3, M3): a 64 px thumbnail in a white frame with a
//! pointer and a pin at the photo's position (design system, "Photo marker").
//!
//! Until M4 groups photos, one photo per *cell* of a fixed world grid shows as a thumbnail and
//! the others in that cell stay dots (design.md §2.3, unconfirmed). The grid is anchored to the
//! world at the whole-number zoom, not to the screen, so panning never swaps which photo shows.
//!
//! The thumbnails are read and decoded on a background thread (`Textures`); the UI thread only
//! uploads finished images, for the markers it draws, and keeps a bounded number of them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Color32, ColorImage, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, Ui};

use crate::library::{PhotoPoint, Thumb};
use crate::theme::Palette;
use crate::thumbs::{self, THUMB};
use crate::ui::icons::Icon;

/// The world grid's cell edge in screen pixels at the whole-number zoom: a 68 px marker plus
/// room to see the map between neighbours.
pub const CELL: f64 = 80.0;

/// The most marker textures kept on the GPU (a 256 px thumbnail is about 0.2 MB). A screen holds
/// about 200 cells, so this covers a screen and some panning back and forth.
const TEXTURE_CAP: usize = 400;

/// The position of `lat`/`lon` in the world's pixels at `zoom` (Web Mercator, 256 px tiles), the
/// same space the map's tiles are cut from.
pub fn world_px(lat: f64, lon: f64, zoom: i32) -> (f64, f64) {
    let size = 256.0 * 2f64.powi(zoom);
    let x = (lon + 180.0) / 360.0 * size;
    let lat = lat.clamp(-85.051_128_78, 85.051_128_78).to_radians();
    let y = (1.0 - (lat.tan() + 1.0 / lat.cos()).ln() / std::f64::consts::PI) / 2.0 * size;
    (x, y)
}

/// Which photos show as markers at `zoom`: per grid cell, the photo with the lowest id among
/// those with a thumbnail, else the lowest id among the placeholders. Returns indices into
/// `points`; photos still waiting for their thumbnail never take a cell (they stay dots).
pub fn pick(points: &[PhotoPoint], zoom: i32) -> Vec<usize> {
    let mut best: HashMap<(i64, i64), usize> = HashMap::new();
    let rank = |p: &PhotoPoint| (p.thumb != Thumb::Ok, p.id);
    for (i, p) in points.iter().enumerate() {
        if p.thumb == Thumb::Pending {
            continue;
        }
        let (x, y) = world_px(p.lat, p.lon, zoom);
        let cell = ((x / CELL).floor() as i64, (y / CELL).floor() as i64);
        best.entry(cell)
            .and_modify(|b| {
                if rank(p) < rank(&points[*b]) {
                    *b = i;
                }
            })
            .or_insert(i);
    }
    let mut out: Vec<usize> = best.into_values().collect();
    out.sort_unstable();
    out
}

/// A marker texture, as `Textures::get` finds it.
pub enum Look<'a> {
    Loading,
    Ready(&'a TextureHandle),
    /// The file couldn't be read: the marker shows the placeholder.
    Failed,
}

enum Entry {
    Loading,
    Ready { tex: TextureHandle, used: u64 },
    Failed,
}

/// The marker thumbnails: requested by id, loaded off the UI thread, evicted least recently used.
pub struct Textures {
    requests: Sender<i64>,
    results: Receiver<(i64, Option<ColorImage>)>,
    entries: HashMap<i64, Entry>,
    frame: u64,
}

impl Textures {
    pub fn new(data: PathBuf, ctx: egui::Context) -> Self {
        let (requests, rx) = mpsc::channel::<i64>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("mapcraft-markers".into())
            .spawn(move || {
                // Ends when the app drops its `Textures` (the request channel closes).
                for id in rx {
                    let image = load(&thumbs::path(&data, id, THUMB));
                    if tx.send((id, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn the marker loader");
        Self {
            requests,
            results,
            entries: HashMap::new(),
            frame: 0,
        }
    }

    /// Uploads the images loaded since last frame and evicts beyond the cap. Call once a frame.
    pub fn begin_frame(&mut self, ctx: &egui::Context) {
        self.frame += 1;
        while let Ok((id, image)) = self.results.try_recv() {
            // Only if still wanted: `forget` may have dropped it while it loaded.
            if let Some(e) = self.entries.get_mut(&id) {
                *e = match image {
                    Some(img) => Entry::Ready {
                        tex: ctx.load_texture(format!("marker-{id}"), img, TextureOptions::LINEAR),
                        used: self.frame,
                    },
                    None => Entry::Failed,
                };
            }
        }
        let ready = self
            .entries
            .values()
            .filter(|e| matches!(e, Entry::Ready { .. }))
            .count();
        if ready > TEXTURE_CAP {
            let mut by_age: Vec<(u64, i64)> = self
                .entries
                .iter()
                .filter_map(|(id, e)| match e {
                    Entry::Ready { used, .. } => Some((*used, *id)),
                    _ => None,
                })
                .collect();
            by_age.sort_unstable();
            for (_, id) in by_age.into_iter().take(ready - TEXTURE_CAP) {
                self.entries.remove(&id); // dropping the handle frees the texture
            }
        }
    }

    /// The texture for photo `id`, requesting it the first time.
    pub fn get(&mut self, id: i64) -> Look<'_> {
        let frame = self.frame;
        let e = self.entries.entry(id).or_insert_with(|| {
            let _ = self.requests.send(id);
            Entry::Loading
        });
        match e {
            Entry::Loading => Look::Loading,
            Entry::Failed => Look::Failed,
            Entry::Ready { tex, used } => {
                *used = frame;
                Look::Ready(tex)
            }
        }
    }

    /// Drops the textures of photos whose thumbnail is no longer made (changed, or gone), so a
    /// re-made thumbnail is loaded afresh.
    pub fn retain(&mut self, keep: impl Fn(i64) -> bool) {
        self.entries.retain(|id, _| keep(*id));
    }
}

/// Reads and decodes a thumbnail file; `None` if it's missing or damaged.
fn load(path: &std::path::Path) -> Option<ColorImage> {
    let bytes = std::fs::read(path).ok()?;
    let img = image::load_from_memory_with_format(&bytes, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

/// What a marker shows in its frame.
pub enum Face<'a> {
    Image(&'a TextureHandle),
    /// Still loading, or no usable source: the placeholder ground with an image icon.
    Placeholder,
}

/// The marker's frame rect for a photo at screen position `pos`: the pointer's tip, 6 px below
/// the frame, sits on the position. Hovered markers grow 2 px per side and lift 2 px.
pub fn frame_rect(p: &Palette, pos: Pos2, hovered: bool) -> Rect {
    let outer = p.size_marker + 2.0 * p.stroke_marker;
    let r = Rect::from_min_size(
        egui::pos2(pos.x - outer / 2.0, pos.y - 6.0 - outer),
        egui::vec2(outer, outer),
    );
    if hovered {
        r.translate(egui::vec2(0.0, -2.0)).expand(2.0)
    } else {
        r
    }
}

/// Paints one marker (design system "Photo marker": frame, edge, shadow, pointer, pin).
pub fn paint(ui: &Ui, p: &Palette, pos: Pos2, face: Face<'_>, hovered: bool, selected: bool) {
    let painter = ui.painter();
    let rect = frame_rect(p, pos, hovered);
    let radius = p.radius_sm;
    let (frame, stroke) = if selected {
        (p.map_marker_frame_selected, p.stroke_marker_selected)
    } else {
        (p.map_marker_frame, p.stroke_marker)
    };
    let shadow = if hovered {
        p.elevation_marker_raised
    } else {
        p.elevation_marker
    };
    painter.add(shadow.as_shape(rect, radius));

    // The pointer: an edge-coloured copy 1 px lower first (the drop shadow), then the frame's.
    let bottom = rect.bottom();
    let tri = |dy: f32, c: Color32| {
        Shape::convex_polygon(
            vec![
                egui::pos2(pos.x - 5.0, bottom - 1.0 + dy),
                egui::pos2(pos.x + 5.0, bottom - 1.0 + dy),
                egui::pos2(pos.x, pos.y + dy),
            ],
            c,
            Stroke::NONE,
        )
    };
    painter.add(tri(1.0, p.map_marker_edge));
    painter.rect_filled(
        rect.expand(p.stroke_marker_edge),
        radius + 1,
        p.map_marker_edge,
    );
    painter.rect_filled(rect, radius, frame);
    painter.add(tri(0.0, frame));

    let inner = rect.shrink(stroke);
    match face {
        Face::Image(tex) => {
            // Fill the square: crop the long side, centred (like CSS object-fit: cover).
            let [w, h] = tex.size().map(|v| v as f32);
            let uv = if w > h {
                let m = (1.0 - h / w) / 2.0;
                Rect::from_min_max(egui::pos2(m, 0.0), egui::pos2(1.0 - m, 1.0))
            } else {
                let m = (1.0 - w / h) / 2.0;
                Rect::from_min_max(egui::pos2(0.0, m), egui::pos2(1.0, 1.0 - m))
            };
            painter.image(tex.id(), inner, uv, Color32::WHITE);
        }
        Face::Placeholder => {
            painter.rect_filled(inner, 1, p.map_marker_placeholder);
            let icon = Rect::from_center_size(inner.center(), egui::vec2(24.0, 24.0));
            Icon::Image
                .image(24.0, p.map_marker_placeholder_fg)
                .paint_at(ui, icon);
        }
    }

    // The pin: the exact position, a pin-coloured dot in a 1 px frame ring.
    painter.circle_filled(pos, 3.5, p.map_marker_frame);
    painter.circle_filled(pos, 2.5, p.map_pin);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn pt(id: i64, lat: f64, lon: f64, thumb: Thumb) -> PhotoPoint {
        PhotoPoint {
            id,
            lat,
            lon,
            thumb,
        }
    }

    #[test]
    fn world_pixels_match_the_tile_grid() {
        // Zoom 0 is one 256 px tile: the origin at the top left, Greenwich and the equator
        // in the middle.
        let (x, y) = world_px(0.0, 0.0, 0);
        assert!((x - 128.0).abs() < 1e-9 && (y - 128.0).abs() < 1e-9);
        let (x, y) = world_px(85.051_128_78, -180.0, 3);
        assert!(x.abs() < 1e-6 && y.abs() < 1e-6);
        // Each zoom step doubles the pixels.
        let (a, _) = world_px(48.2, 16.37, 10);
        let (b, _) = world_px(48.2, 16.37, 11);
        assert!((b - 2.0 * a).abs() < 1e-6);
    }

    #[test]
    fn one_photo_per_cell_shows_and_the_choice_is_stable() {
        let points = vec![
            pt(5, 64.1466, -21.9426, Thumb::Ok),
            pt(3, 64.1467, -21.9427, Thumb::Failed), // same cell, a placeholder
            pt(9, 64.1468, -21.9425, Thumb::Ok),     // same cell, higher id
            pt(1, 64.1469, -21.9424, Thumb::Pending), // waiting: never a marker
            pt(7, 38.7223, -9.1393, Thumb::Failed),  // alone in its cell
        ];
        let shown: Vec<i64> = pick(&points, 12).iter().map(|&i| points[i].id).collect();
        assert_eq!(
            shown,
            vec![5, 7],
            "thumbnails beat placeholders, then the lowest id"
        );
        // The order of the points doesn't matter.
        let mut rev = points.clone();
        rev.reverse();
        let shown: Vec<i64> = pick(&rev, 12).iter().map(|&i| rev[i].id).collect();
        assert_eq!(shown.len(), 2);
        assert!(shown.contains(&5) && shown.contains(&7));
    }

    #[test]
    fn the_pointer_tip_sits_on_the_position() {
        let tokens = crate::theme::Tokens::embedded(crate::theme::ThemeKind::Dark).unwrap();
        let p = Palette::new(&tokens);
        let pos = egui::pos2(300.0, 200.0);
        let r = frame_rect(&p, pos, false);
        assert_eq!(r.width(), 68.0);
        assert_eq!(r.center().x, pos.x);
        assert_eq!(r.bottom(), pos.y - 6.0);
        let h = frame_rect(&p, pos, true);
        assert_eq!((h.width(), h.bottom()), (72.0, pos.y - 6.0));
    }
}

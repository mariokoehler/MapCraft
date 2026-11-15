//! Photo markers on the map (design.md §2.3): a 64 px thumbnail in a white frame with a
//! pointer and a pin at the photo's position, and for a group the same with stacked frames
//! behind and a count badge (design system, "Photo marker" and "Group marker"). Which photos and
//! groups show is decided by `cluster`.
//!
//! The thumbnails are read and decoded on a background thread (`Textures`); the UI thread only
//! uploads finished images, for the markers it draws, and keeps a bounded number of them.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{Color32, ColorImage, Pos2, Rect, Shape, Stroke, TextureHandle, TextureOptions, Ui};

use crate::import::scope::Check;
use crate::theme::Palette;
use crate::thumbs::{self, THUMB};
use crate::ui::icons::Icon;

/// The most marker textures kept on the GPU (a 256 px thumbnail is about 0.2 MB). A screen holds
/// about 200 cells, so this covers a screen and some panning back and forth.
const TEXTURE_CAP: usize = 400;

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
            // Never what the last frame drew: with more markers in view than the cap, evicting
            // those would reload and evict them again every frame. The cap gives way instead.
            let mut by_age: Vec<(u64, i64)> = self
                .entries
                .iter()
                .filter_map(|(id, e)| match e {
                    Entry::Ready { used, .. } if *used + 1 < self.frame => Some((*used, *id)),
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

/// A group's count as its badge shows it: `214`, `1.2k`, `12k`, `1.2M` (rounded down, so it
/// never claims more photos than there are).
pub fn count_text(n: u32) -> String {
    match n {
        0..1_000 => n.to_string(),
        1_000..10_000 => format!("{}.{}k", n / 1_000, n / 100 % 10),
        10_000..1_000_000 => format!("{}k", n / 1_000),
        _ => format!("{}.{}M", n / 1_000_000, n / 100_000 % 10),
    }
}

/// Paints one marker (design system "Photo marker": frame, edge, shadow, pointer, pin). With
/// `count` > 1 it's a group marker: two frames stacked behind it and a count badge.
pub fn paint(
    ui: &Ui,
    p: &Palette,
    pos: Pos2,
    face: Face<'_>,
    count: u32,
    hovered: bool,
    selected: bool,
) {
    let painter = ui.painter();
    let rect = frame_rect(p, pos, hovered);
    let radius = p.radius_sm;
    if count > 1 {
        // The stack: frames offset up and right, the farther one first.
        for off in [6.0, 3.0] {
            let r = rect.translate(egui::vec2(off, -off));
            painter.rect_filled(
                r.expand(p.stroke_marker_edge),
                radius + 1,
                p.map_marker_edge,
            );
            painter.rect_filled(r, radius, p.map_marker_frame);
        }
    }
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

    if count > 1 {
        // The badge: top right, 9 px above the frame and 12 px past it; a frame ring and an
        // edge ring around it.
        let text = painter.layout_no_wrap(
            count_text(count),
            egui::FontId::new(11.0, egui::FontFamily::Name(crate::theme::SEMIBOLD.into())),
            p.map_badge_text,
        );
        let h = p.size_badge;
        let w = (text.size().x + 10.0).max(h);
        let badge = Rect::from_min_size(
            egui::pos2(rect.right() + 12.0 - w, rect.top() - 9.0),
            egui::vec2(w, h),
        );
        let round = (h / 2.0) as u8;
        painter.rect_filled(badge.expand(2.5), round + 3, p.map_marker_edge);
        painter.rect_filled(badge.expand(1.5), round + 2, p.map_marker_frame);
        painter.rect_filled(badge, round, p.map_badge_bg);
        painter.galley(badge.center() - text.size() / 2.0, text, p.map_badge_text);
    }
}

/// The tick on a marker or a strip thumbnail in tagging mode (design.md §2.9(h), canvas
/// `MapCraft — Geotagging`): a 20 px round button, accent with a check when on, a dash when
/// mixed, white when off. Returns its click; `name` is its accessible name.
pub fn tick(
    ui: &Ui,
    p: &Palette,
    center: Pos2,
    state: Check,
    name: &str,
    id: egui::Id,
) -> egui::Response {
    let rect = Rect::from_center_size(center, egui::vec2(20.0, 20.0));
    let r = ui.interact(rect, id, egui::Sense::click());
    r.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, true, state == Check::On, name)
    });
    let painter = ui.painter();
    let hover = r.hovered();
    painter.circle_filled(center, 11.0, p.map_marker_edge);
    let fill = match state {
        Check::Off => p.map_marker_frame,
        _ if hover => p.accent_hover,
        _ => p.accent,
    };
    painter.circle(
        center,
        9.5,
        fill,
        egui::Stroke::new(1.5, p.map_marker_frame),
    );
    if state == Check::Off {
        painter.circle_stroke(center, 9.0, egui::Stroke::new(1.0, p.border_normal));
    }
    let mark = match state {
        Check::On => Some(Icon::Check),
        Check::Mixed => Some(Icon::Minus),
        Check::Off => None,
    };
    if let Some(icon) = mark {
        icon.image(12.0, p.text_on_accent)
            .paint_at(ui, Rect::from_center_size(center, egui::vec2(12.0, 12.0)));
    }
    r
}

/// Washes out a marker whose photos are all unticked: what won't be tagged reads as faded.
pub fn fade(ui: &Ui, p: &Palette, pos: Pos2) {
    let rect = frame_rect(p, pos, false);
    ui.painter()
        .rect_filled(rect, p.radius_sm, p.map_marker_frame.gamma_multiply(0.55));
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn badge_counts_round_down() {
        assert_eq!(count_text(2), "2");
        assert_eq!(count_text(999), "999");
        assert_eq!(count_text(1_250), "1.2k");
        assert_eq!(count_text(9_999), "9.9k");
        assert_eq!(count_text(12_800), "12k");
        assert_eq!(count_text(1_250_000), "1.2M");
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

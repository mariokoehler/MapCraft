//! The map and everything drawn over it (design system "Map layout"): the photos as thumbnail
//! markers and position dots (design.md §2.3, `markers`), controls top-right, scale bar bottom-left, the provider's attribution
//! flush bottom-right of the *visible* map, and the empty-library card while there are no photos.

use std::collections::{HashMap, HashSet};
use std::path::Path;

use egui::{
    Align2, Area, Button, CornerRadius, Frame, Id, Margin, Order, Rect, RichText, Sense, Stroke,
    Ui, text::LayoutJob,
};
use walkers::{
    HttpTiles, Map, MapMemory, MercatorProjection, Plugin, Position, Projection, Projector, lat_lon,
};

use crate::import::Bounds;
use crate::library::{PhotoPoint, Thumb};
use crate::provider::Provider;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::markers::{self, Face, Look, Textures};
use crate::ui::widgets;

/// Where the map opens while the library is empty: central Europe.
const HOME: (f64, f64, f64) = (50.0, 10.0, 4.0); // lat, lon, zoom

/// What the map asks of the app this frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MapAction {
    AddFolder,
    /// A marker was clicked (`Some`), or the map's background (`None`).
    Select(Option<i64>),
}

pub struct MapView {
    provider: Provider,
    /// `None` in tests: the map then draws no tiles and makes no network requests.
    tiles: Option<HttpTiles>,
    memory: MapMemory,
    home: Position,
    points: Vec<PhotoPoint>,
    /// The photos shown as markers at a whole-number zoom, and per point whether it's one of
    /// them (`markers::pick`); recomputed when the zoom level or the points change.
    picked: Option<(i32, Vec<usize>, Vec<bool>)>,
    textures: Textures,
    /// File names of photos with a marker, for their accessible names; filled by the app.
    names: HashMap<i64, String>,
    wanted_names: Vec<i64>,
    selected: Option<i64>,
    /// The marker under the pointer this frame, with its frame rect (for the hover card).
    hovered: Option<(i64, Rect)>,
    /// The map's size last frame, for fitting a zoom to an area.
    last_rect: Rect,
    /// Keyboard shortcuts are live this frame.
    keys: bool,
}

impl MapView {
    /// `tile_cache: None` draws no tiles (tests and headless runs).
    /// `data`: the data folder, where the thumbnails are.
    pub fn new(
        provider: Provider,
        tile_cache: Option<&Path>,
        data: &Path,
        ctx: egui::Context,
    ) -> Self {
        let mut memory = MapMemory::default();
        // A zoom within the tile range never fails; ignore the error rather than unwrap.
        let _ = memory.set_zoom(HOME.2);
        Self {
            provider,
            tiles: tile_cache.map(|c| provider.tiles(c, ctx.clone())),
            memory,
            home: lat_lon(HOME.0, HOME.1),
            points: Vec::new(),
            picked: None,
            textures: Textures::new(data.to_path_buf(), ctx),
            names: HashMap::new(),
            wanted_names: Vec::new(),
            selected: None,
            hovered: None,
            last_rect: Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(1280.0, 760.0)),
            keys: true,
        }
    }

    pub fn set_points(&mut self, points: Vec<PhotoPoint>) {
        // Textures of photos whose thumbnail isn't made any more (changed, removed) are dropped,
        // so a re-made one loads afresh.
        let made: HashSet<i64> = points
            .iter()
            .filter(|p| p.thumb == Thumb::Ok)
            .map(|p| p.id)
            .collect();
        self.textures.retain(|id| made.contains(&id));
        if self
            .selected
            .is_some_and(|s| !points.iter().any(|p| p.id == s))
        {
            self.selected = None;
        }
        self.points = points;
        self.picked = None;
    }

    pub fn select(&mut self, id: Option<i64>) {
        self.selected = id;
    }

    pub fn selected(&self) -> Option<i64> {
        self.selected
    }

    /// The thumbnail texture of photo `id`, if it's loaded (requesting it otherwise).
    pub fn thumbnail(&mut self, id: i64) -> Option<&egui::TextureHandle> {
        match self.textures.get(id) {
            Look::Ready(t) => Some(t),
            _ => None,
        }
    }

    /// The marker under the pointer, with its frame rect.
    pub fn hovered(&self) -> Option<(i64, Rect)> {
        self.hovered
    }

    /// Photos with a marker whose file name isn't known yet (the app looks them up).
    pub fn wanted_names(&mut self) -> Vec<i64> {
        std::mem::take(&mut self.wanted_names)
    }

    pub fn add_names(&mut self, names: impl IntoIterator<Item = (i64, String)>) {
        self.names.extend(names);
    }

    /// Centres and zooms the map so `b` fills most of it.
    pub fn zoom_to(&mut self, b: Bounds) {
        let center = lat_lon((b.min_lat + b.max_lat) / 2.0, (b.min_lon + b.max_lon) / 2.0);
        self.memory.center_at(center);
        let _ = self.memory.set_zoom(fit_zoom(b, self.last_rect.size()));
    }

    /// The area of every photo on the map, if there are any.
    pub fn all_bounds(&self) -> Option<Bounds> {
        let mut it = self.points.iter();
        let first = it.next()?;
        Some(it.fold(Bounds::around(first.lat, first.lon), |b, p| {
            b.include(p.lat, p.lon)
        }))
    }

    /// Draws the map. `empty`: the library has no photos, so the empty-library card shows.
    /// `keys`: the map's keyboard shortcuts are live (not while a dialog is open).
    pub fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        empty: bool,
        keys: bool,
    ) -> (Rect, Option<MapAction>) {
        self.keys = keys;
        let zoom = self.memory.zoom().floor() as i32;
        if self.picked.as_ref().is_none_or(|(z, _, _)| *z != zoom) {
            let picked = markers::pick(&self.points, zoom);
            let mut is_picked = vec![false; self.points.len()];
            for &i in &picked {
                is_picked[i] = true;
            }
            self.picked = Some((zoom, picked, is_picked));
        }
        let (_, picked, is_picked) = self.picked.as_ref().expect("just set");
        self.textures.begin_frame(ui.ctx());
        let mut out = PhotosOut::default();
        let photos = Photos {
            points: &self.points,
            picked,
            is_picked,
            textures: &mut self.textures,
            names: &self.names,
            wanted: &mut self.wanted_names,
            palette: p,
            selected: self.selected,
            out: &mut out,
        };
        let tiles = self.tiles.as_mut().map(|t| t as &mut dyn walkers::Tiles);
        let map = Map::new(tiles, &mut self.memory, self.home)
            .zoom_with_ctrl(false) // the wheel zooms (design.md §2.3)
            .double_click_to_zoom(true)
            .with_plugin(photos);
        let rect = ui.add(map).rect;
        self.hovered = out.hovered;
        let mut action = match (out.clicked, out.background_clicked) {
            (Some(id), _) => Some(MapAction::Select(Some(id))),
            (None, true) if self.selected.is_some() => Some(MapAction::Select(None)),
            _ => None,
        };
        self.last_rect = rect;
        self.zoom_keys(ui);

        self.controls(ui, p, rect);
        self.scale_bar(ui, p, rect);
        self.attribution(ui, p, rect);
        if empty && let Some(a) = empty_state(ui, p, rect) {
            action = Some(a);
        }
        (rect, action)
    }

    /// `+`/`-` zoom (design.md §2.3; walkers has no keyboard handling). `=` counts as `+`, since
    /// that's the unshifted key on many layouts. Ignored while a text field has the keyboard.
    fn zoom_keys(&mut self, ui: &Ui) {
        if !self.keys || ui.ctx().egui_wants_keyboard_input() {
            return;
        }
        let (zoom_in, zoom_out) = ui.input(|i| {
            (
                i.key_pressed(egui::Key::Plus) || i.key_pressed(egui::Key::Equals),
                i.key_pressed(egui::Key::Minus),
            )
        });
        if zoom_in {
            let _ = self.memory.zoom_in();
        }
        if zoom_out {
            let _ = self.memory.zoom_out();
        }
    }

    /// Zoom in/out and "zoom to all photos" (once there are photos); "map style" has nothing to
    /// choose from yet.
    fn controls(&mut self, ui: &Ui, p: &Palette, map: Rect) {
        let all = self.all_bounds();
        let frame = Frame::NONE
            .fill(p.bg_panel)
            .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
            .corner_radius(p.radius_md)
            .shadow(p.elevation_panel)
            .inner_margin(Margin::same(2));
        Area::new(Id::unique("map-controls"))
            .order(Order::Foreground)
            .pivot(Align2::RIGHT_TOP)
            .fixed_pos(map.right_top() + egui::vec2(-p.space_3, p.space_3))
            .show(ui.ctx(), |ui| {
                frame.show(ui, |ui| {
                    ui.spacing_mut().item_spacing = egui::vec2(2.0, 2.0);
                    if control(ui, p, Icon::Plus, "Zoom in (+)", true).clicked() {
                        let _ = self.memory.zoom_in();
                    }
                    if control(ui, p, Icon::Minus, "Zoom out (−)", true).clicked() {
                        let _ = self.memory.zoom_out();
                    }
                    ui.separator();
                    let hint = if all.is_some() {
                        "Zoom to all photos (F)"
                    } else {
                        "Zoom to all photos: no photos on the map yet"
                    };
                    let fit = control(ui, p, Icon::Scan, hint, all.is_some()).clicked()
                        || (all.is_some()
                            && self.keys
                            && !ui.ctx().egui_wants_keyboard_input()
                            && ui.input(|i| i.key_pressed(egui::Key::F)));
                    if fit && let Some(b) = all {
                        self.zoom_to(b);
                    }
                    control(ui, p, Icon::Layers, "Map style: not available yet", false);
                });
            });
    }

    /// A bar of a round length (1, 2 or 5 × 10ⁿ m) at most 100 px long, for the map's centre.
    fn scale_bar(&self, ui: &Ui, p: &Palette, map: Rect) {
        let center = self.memory.detached().unwrap_or(self.home);
        let px_per_m = MercatorProjection.scale_pixel_per_meter(center, self.memory.zoom());
        let Some((meters, width)) = nice_scale(px_per_m, 100.0) else {
            return;
        };
        let label = if meters >= 1000.0 {
            format!("{} km", meters / 1000.0)
        } else {
            format!("{meters} m")
        };
        let painter = ui.painter_at(map);
        let font = egui::FontId::proportional(11.0);
        let galley = painter.layout_no_wrap(label, font, p.map_scrim_light_text);
        let pad = egui::vec2(6.0, 2.0);
        let size =
            egui::vec2(galley.size().x + p.space_2 + width as f32, galley.size().y) + pad * 2.0;
        let bg = Rect::from_min_size(
            map.left_bottom() + egui::vec2(p.space_3, -p.space_3 - size.y),
            size,
        );
        painter.rect_filled(bg, p.radius_sm, p.map_scrim_light);
        painter.galley(bg.min + pad, galley.clone(), p.map_scrim_light_text);
        let x0 = bg.min.x + pad.x + galley.size().x + p.space_2;
        let x1 = x0 + width as f32;
        let y = bg.center().y + 3.0;
        let stroke = Stroke::new(1.5, p.map_scrim_light_text);
        painter.line_segment([egui::pos2(x0, y), egui::pos2(x1, y)], stroke);
        for x in [x0, x1] {
            painter.line_segment([egui::pos2(x, y - 4.0), egui::pos2(x, y)], stroke);
        }
    }

    /// The provider's credit, always visible, flush bottom-right; "OpenStreetMap" links to the
    /// copyright page (required by every provider researched, design.md §2.3).
    fn attribution(&self, ui: &mut Ui, p: &Palette, map: Rect) {
        let attribution = self.provider.attribution();
        let font = egui::FontId::proportional(11.0);
        let mut job = LayoutJob::default();
        let plain = egui::TextFormat::simple(font.clone(), p.map_scrim_light_text);
        let mut link = plain.clone();
        link.underline = Stroke::new(1.0, p.map_scrim_light_text);
        job.append(attribution.before, 0.0, plain.clone());
        job.append(attribution.link, 0.0, link);
        job.append(attribution.after, 0.0, plain);
        let galley = ui.painter().layout_job(job);
        let pad = egui::vec2(6.0, 1.0);
        let rect = Rect::from_min_size(
            map.right_bottom() - galley.size() - pad * 2.0,
            galley.size() + pad * 2.0,
        );
        let painter = ui.painter_at(map);
        let corner = CornerRadius {
            nw: p.radius_sm,
            ne: 0,
            sw: 0,
            se: 0,
        };
        painter.rect_filled(rect, corner, p.map_scrim_light);
        painter.galley(rect.min + pad, galley, p.map_scrim_light_text);
        let response = ui.interact(rect, Id::unique("map-attribution"), Sense::click());
        if response.clicked() {
            ui.ctx().open_url(egui::OpenUrl::new_tab(attribution.url));
        }
        response
            .on_hover_cursor(egui::CursorIcon::PointingHand)
            .on_hover_text(attribution.url);
    }
}

/// One 28 px map-control button.
fn control(ui: &mut Ui, p: &Palette, icon: Icon, hint: &str, enabled: bool) -> egui::Response {
    // egui fades disabled text but not a tinted image, so pick the disabled ink here.
    let ink = if enabled {
        p.text_normal
    } else {
        p.text_disabled
    };
    let button = Button::image(icon.image(18.0, ink))
        .frame_when_inactive(false)
        .min_size(egui::vec2(p.size_map_control, p.size_map_control));
    let response = ui.add_enabled(enabled, button);
    crate::ui::widgets::named(&response, hint, enabled);
    if enabled {
        response.on_hover_text(hint)
    } else {
        response.on_disabled_hover_text(hint)
    }
}

/// The card shown while there are no photos.
fn empty_state(ui: &Ui, p: &Palette, map: Rect) -> Option<MapAction> {
    let mut action = None;
    Area::new(Id::unique("empty-state"))
        .order(Order::Middle)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(map.center())
        .show(ui.ctx(), |ui| {
            Frame::NONE
                .fill(p.bg_window)
                .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                .corner_radius(p.radius_md)
                .shadow(p.elevation_panel)
                .inner_margin(Margin::same((p.space_4 * 1.5) as i8))
                .show(ui, |ui| {
                    ui.set_width(300.0);
                    ui.vertical_centered(|ui| {
                        let glyph = egui::vec2(40.0, 40.0);
                        let (rect, _) = ui.allocate_exact_size(glyph, Sense::hover());
                        ui.painter()
                            .circle_filled(rect.center(), 20.0, p.widget_active_weak);
                        Icon::FolderPlus
                            .image(p.size_icon_md, p.text_strong)
                            .paint_at(
                                ui,
                                Rect::from_center_size(rect.center(), egui::vec2(20.0, 20.0)),
                            );
                        ui.add_space(p.space_3);
                        ui.label(
                            RichText::new("No photos on the map yet")
                                .heading()
                                .color(p.text_strong),
                        );
                        ui.add_space(p.space_1);
                        ui.label(
                            RichText::new(
                                "Add a folder of photos from a local drive or a network share. \
                                 Photos with a GPS position appear where they were taken.",
                            )
                            .color(p.text_weak),
                        );
                        ui.add_space(p.space_4);
                        let add = Button::image_and_text(
                            Icon::FolderPlus.image(p.size_icon_sm, p.text_on_accent),
                            RichText::new("Add folder…").color(p.text_on_accent),
                        )
                        .fill(p.accent);
                        if ui.add(add).clicked() {
                            action = Some(MapAction::AddFolder);
                        }
                    });
                });
        });
    action
}

/// The photos on the map (design.md §2.3): markers for the picked ones, dots for the rest.
/// A dot is a pin-coloured dot in a white ring with a dark edge, so it reads on pale land and on
/// saturated tiles alike (design system, map rules).
struct Photos<'a> {
    points: &'a [PhotoPoint],
    picked: &'a [usize],
    is_picked: &'a [bool],
    textures: &'a mut Textures,
    names: &'a HashMap<i64, String>,
    wanted: &'a mut Vec<i64>,
    palette: &'a Palette,
    selected: Option<i64>,
    out: &'a mut PhotosOut,
}

#[derive(Default)]
struct PhotosOut {
    hovered: Option<(i64, Rect)>,
    clicked: Option<i64>,
    background_clicked: bool,
}

/// Dots closer than this many pixels are drawn once: a dot is 12 px wide, so closer neighbours
/// can't be told apart anyway, and drawing each would only cost time (until M4 groups them).
/// Measured with 100,000 photos over Europe (2026-11-14): see design.md §2.3.
const DOT_CELL: f32 = 5.0;

impl Plugin for Photos<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        response: &egui::Response,
        projector: &Projector,
        _: &MapMemory,
    ) {
        let p = self.palette;
        let clip = ui.clip_rect();
        let screen = |pt: &PhotoPoint| {
            let v = projector.project(lat_lon(pt.lat, pt.lon));
            egui::pos2(v.x, v.y)
        };

        // The markers in view, southern ones in front (they're nearer the viewer's eye line).
        let mut shown: Vec<(&PhotoPoint, egui::Pos2)> = self
            .picked
            .iter()
            .map(|&i| (&self.points[i], screen(&self.points[i])))
            .filter(|(_, pos)| clip.intersects(markers::frame_rect(p, *pos, true).expand(4.0)))
            .collect();
        shown.sort_by(|a, b| a.1.y.total_cmp(&b.1.y));

        // The dots, under the markers. A dot right at a marker's pin is left out: the marker
        // stands for that spot (M4's count badge will say how many), and the pile of dots
        // would only blot out the pin.
        {
            const NEAR: f32 = 10.0;
            let mut pins: HashMap<(i32, i32), Vec<egui::Pos2>> = HashMap::new();
            for (_, pos) in &shown {
                let bin = ((pos.x / NEAR).floor() as i32, (pos.y / NEAR).floor() as i32);
                pins.entry(bin).or_default().push(*pos);
            }
            let under_a_pin = |pos: egui::Pos2| {
                let (bx, by) = ((pos.x / NEAR).floor() as i32, (pos.y / NEAR).floor() as i32);
                (-1..=1).any(|dx| {
                    (-1..=1).any(|dy| {
                        pins.get(&(bx + dx, by + dy))
                            .is_some_and(|v| v.iter().any(|q| q.distance(pos) < NEAR))
                    })
                })
            };
            let painter = ui.painter();
            let dot_clip = clip.expand(8.0);
            let mut seen = HashSet::new();
            for (i, pt) in self.points.iter().enumerate() {
                if self.is_picked[i] {
                    continue;
                }
                let pos = screen(pt);
                if !dot_clip.contains(pos) {
                    continue;
                }
                let cell = ((pos.x / DOT_CELL) as i32, (pos.y / DOT_CELL) as i32);
                if !seen.insert(cell) || under_a_pin(pos) {
                    continue;
                }
                // Two shapes per dot: the dark edge, then the pin colour with its white ring.
                painter.circle_filled(pos, 6.0, p.map_marker_edge);
                painter.circle(pos, 4.0, p.map_pin, Stroke::new(2.0, p.map_marker_frame));
            }
        }

        // Interaction first (egui hit-tests against last frame's rects, so the order of
        // registration is the stacking order), then painting with the hover known.
        let mut hovered = None;
        for (pt, pos) in &shown {
            let rect = markers::frame_rect(p, *pos, false);
            let hit = rect.union(Rect::from_center_size(*pos, egui::vec2(10.0, 12.0)));
            let r = ui.interact(hit, Id::unique(("photo-marker", pt.id)), Sense::click());
            let name = match self.names.get(&pt.id) {
                Some(n) => format!("Photo {n}"),
                None => {
                    self.wanted.push(pt.id);
                    "Photo".to_owned()
                }
            };
            widgets::named(&r, &name, true);
            if r.hovered() {
                hovered = Some(pt.id);
            }
            if r.clicked() {
                self.out.clicked = Some(pt.id);
            }
        }
        if response.clicked() {
            self.out.background_clicked = true;
        }

        let mut last = Vec::new();
        for (pt, pos) in shown {
            let (hover, sel) = (hovered == Some(pt.id), self.selected == Some(pt.id));
            if hover || sel {
                last.push((pt, pos, hover, sel)); // drawn on top
                continue;
            }
            let face = face(self.textures, pt);
            markers::paint(ui, p, pos, face, false, false);
        }
        last.sort_by_key(|(_, _, hover, _)| *hover); // the hovered one topmost
        for (pt, pos, hover, sel) in last {
            if hover {
                self.out.hovered = Some((pt.id, markers::frame_rect(p, pos, true)));
            }
            let face = face(self.textures, pt);
            markers::paint(ui, p, pos, face, hover, sel);
        }
    }
}

/// A marker's face: its thumbnail once loaded, else the placeholder.
fn face<'t>(textures: &'t mut Textures, pt: &PhotoPoint) -> Face<'t> {
    if pt.thumb != Thumb::Ok {
        return Face::Placeholder;
    }
    match textures.get(pt.id) {
        Look::Ready(t) => Face::Image(t),
        Look::Loading | Look::Failed => Face::Placeholder,
    }
}

/// The zoom that fits `b` into a map of `size` px, with a margin; clamped to street level at
/// most, so a single photo doesn't zoom in to the last tile.
pub fn fit_zoom(b: Bounds, size: egui::Vec2) -> f64 {
    let a = MercatorProjection.project(lat_lon(b.max_lat, b.min_lon), 0.0);
    let c = MercatorProjection.project(lat_lon(b.min_lat, b.max_lon), 0.0);
    let (w0, h0) = ((c.x() - a.x()).abs(), (c.y() - a.y()).abs());
    let usable = (size * 0.8).max(egui::vec2(1.0, 1.0));
    let zx = if w0 > 0.0 {
        (usable.x as f64 / w0).log2()
    } else {
        f64::INFINITY
    };
    let zy = if h0 > 0.0 {
        (usable.y as f64 / h0).log2()
    } else {
        f64::INFINITY
    };
    zx.min(zy).clamp(2.0, 16.0)
}

/// The longest 1/2/5 × 10ⁿ metre length that fits in `max_px`, and its width in px.
fn nice_scale(px_per_m: f64, max_px: f64) -> Option<(f64, f64)> {
    if !(px_per_m.is_finite() && px_per_m > 0.0) {
        return None;
    }
    let max_m = max_px / px_per_m;
    let magnitude = 10f64.powf(max_m.log10().floor());
    let meters = [5.0, 2.0, 1.0]
        .into_iter()
        .map(|f| f * magnitude)
        .find(|m| *m <= max_m)?;
    Some((meters, meters * px_per_m))
}

#[cfg(test)]
mod tests {
    use super::nice_scale;

    #[test]
    fn fitting_a_zoom_to_an_area() {
        use crate::import::Bounds;
        let size = egui::vec2(1280.0, 760.0);
        let europe = Bounds {
            min_lat: 36.0,
            max_lat: 60.0,
            min_lon: -10.0,
            max_lon: 30.0,
        };
        let city = Bounds {
            min_lat: 53.50,
            max_lat: 53.60,
            min_lon: 9.90,
            max_lon: 10.10,
        };
        let (ze, zc) = (super::fit_zoom(europe, size), super::fit_zoom(city, size));
        assert!((3.0..6.0).contains(&ze), "a continent: {ze}");
        assert!((10.0..14.0).contains(&zc), "a city: {zc}");
        assert_eq!(
            super::fit_zoom(Bounds::around(53.55, 9.99), size),
            16.0,
            "one photo: street level"
        );
    }

    #[test]
    fn scale_picks_a_round_length_that_fits() {
        // 0.2 px per metre: 100 px is 500 m.
        let (m, w) = nice_scale(0.2, 100.0).unwrap();
        assert_eq!(m, 500.0);
        assert!((w - 100.0).abs() < 1e-9);
        // 0.07 px per metre: 100 px is 1,428 m, so 1 km fits.
        assert_eq!(nice_scale(0.07, 100.0).unwrap().0, 1000.0);
        assert!(nice_scale(0.0, 100.0).is_none());
    }
}

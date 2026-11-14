//! The map and everything drawn over it (design system "Map layout"): the photos as thumbnail
//! markers, groups and position dots (design.md §2.3, §2.4; `markers`, `cluster`), the strip of
//! a group at one spot, controls top-right, scale bar bottom-left, the provider's attribution
//! flush bottom-right of the *visible* map, and the empty-library card while there are no photos.
//!
//! The groups are built on a background thread (`Builder`) whenever the photos change; the map
//! draws from the newest finished build, which owns the snapshot of photos it was made from.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::Arc;
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{
    Align2, Area, Button, CornerRadius, Frame, Id, Margin, Order, Rect, RichText, Sense, Stroke,
    Ui, text::LayoutJob,
};
use walkers::{
    HttpTiles, Map, MapMemory, MercatorProjection, Plugin, Position, Projection, Projector, lat_lon,
};

use crate::cluster::{self, Clusters, MAX_LEVEL};
use crate::import::Bounds;
use crate::library::{PhotoPoint, Thumb};
use crate::provider::Provider;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::markers::{self, Face, Look, Textures};
use crate::ui::strip::{Strip, StripAction};
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
    /// The newest finished grouping, and the thread that makes them.
    clusters: Option<Arc<Clusters>>,
    builder: Builder,
    /// The photos of a group at one spot, while its strip is open.
    strip: Option<Strip>,
    /// Dev flag `--open strip`: open the first one once the groups are built.
    open_strip: bool,
    /// Photos whose thumbnail is made: only those textures are loaded (a changed photo's old
    /// file would show otherwise).
    made: HashSet<i64>,
    /// The node containing the selected photo, for (selected, level, build), so it's marked
    /// even when another photo represents its group.
    selected_node: Option<(i64, usize, usize, usize)>,
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
            clusters: None,
            builder: Builder::new(ctx.clone()),
            strip: None,
            open_strip: false,
            made: HashSet::new(),
            selected_node: None,
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
        self.made = made;
        if self
            .selected
            .is_some_and(|s| !points.iter().any(|p| p.id == s))
        {
            self.selected = None;
        }
        if let Some(strip) = &mut self.strip {
            let alive: HashSet<i64> = points.iter().map(|p| p.id).collect();
            strip.retain(|id| alive.contains(&id));
            if strip.is_empty() {
                self.strip = None;
            }
        }
        self.builder.request(points.clone());
        self.points = points;
    }

    #[cfg(test)]
    pub fn set_zoom(&mut self, zoom: f64) {
        let _ = self.memory.set_zoom(zoom);
    }

    /// The group or photo marker that holds the selected photo, at the current zoom (tests).
    #[cfg(test)]
    pub fn selected_group(&mut self) -> Option<(usize, usize)> {
        self.selected_node()
    }

    /// The (level, index) of the node at the current zoom that holds the selected photo.
    fn selected_node(&mut self) -> Option<(usize, usize)> {
        let (id, c) = (self.selected?, self.clusters.clone()?);
        let (z, nodes) = c.level(self.memory.zoom());
        let build = Arc::as_ptr(&c) as usize;
        if let Some((sid, sz, sbuild, i)) = self.selected_node
            && (sid, sz, sbuild) == (id, z, build)
        {
            return Some((z, i));
        }
        let pts = c.points();
        let p = pts.binary_search_by_key(&id, |p| p.id).ok()?;
        let (lat, lon) = (pts[p].lat, pts[p].lon);
        // Few nodes cover the photo's position; confirm with their members.
        let i = nodes.iter().enumerate().position(|(i, n)| {
            let b = n.bounds();
            (b.min_lat..=b.max_lat).contains(&lat)
                && (b.min_lon..=b.max_lon).contains(&lon)
                && (n.count == 1 && n.rep as usize == p || c.members(z, i).contains(&(p as u32)))
        })?;
        self.selected_node = Some((id, z, build, i));
        Some((z, i))
    }

    /// Opens the strip of the first group at one spot at the current zoom, once grouped.
    pub fn open_first_strip(&mut self) {
        self.open_strip = true;
    }

    /// Closes the strip; true if one was open (Esc closes it before clearing the selection).
    pub fn close_strip(&mut self) -> bool {
        self.strip.take().is_some()
    }

    /// The groups are built for the current photos (tests wait for this).
    pub fn grouped(&self) -> bool {
        self.clusters
            .as_ref()
            .is_some_and(|c| c.points().len() == self.points.len())
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
        if let Some(c) = self.builder.latest() {
            self.clusters = Some(c);
        }
        if self.open_strip
            && let Some(c) = self.clusters.clone()
        {
            self.open_strip = false;
            let (z, nodes) = c.level(self.memory.zoom());
            if let Some(i) =
                (0..nodes.len()).find(|&i| nodes[i].count > 1 && c.split_level(z, i).is_none())
            {
                self.open_group(z, i);
            }
        }
        self.textures.begin_frame(ui.ctx());
        let selected_node = self.selected_node();
        let mut out = PhotosOut::default();
        let photos = Photos {
            clusters: self.clusters.as_deref(),
            points: &self.points,
            textures: &mut self.textures,
            names: &self.names,
            wanted: &mut self.wanted_names,
            palette: p,
            selected: self.selected,
            selected_node,
            strip: self.strip.as_ref(),
            out: &mut out,
        };
        let tiles = self.tiles.as_mut().map(|t| t as &mut dyn walkers::Tiles);
        let map = Map::new(tiles, &mut self.memory, self.home)
            .zoom_with_ctrl(false) // the wheel zooms (design.md §2.3)
            .double_click_to_zoom(true)
            .with_plugin(photos);
        let rect = ui.add(map).rect;
        self.hovered = out.hovered;
        self.last_rect = rect;
        // The strip belongs to one zoom level's group.
        if self
            .strip
            .as_ref()
            .is_some_and(|s| Some(s.level) != out.level)
        {
            self.strip = None;
        }
        let mut action = match out.clicked {
            Some(Clicked::Photo(id)) => Some(MapAction::Select(Some(id))),
            Some(Clicked::Group(z, i)) => {
                self.open_group(z, i);
                None
            }
            None if out.background_clicked => {
                self.strip = None;
                self.selected.is_some().then_some(MapAction::Select(None))
            }
            None => None,
        };
        if let (Some(strip), Some(tip)) = (&mut self.strip, out.strip_tip) {
            let a = strip.show(
                ui.ctx(),
                p,
                tip,
                rect,
                &mut self.textures,
                &self.made,
                &self.names,
                &mut self.wanted_names,
                self.selected,
            );
            match a {
                Some(StripAction::Close) => self.strip = None,
                Some(StripAction::Select(id)) => action = Some(MapAction::Select(Some(id))),
                None => {}
            }
        }
        self.zoom_keys(ui);

        self.controls(ui, p, rect);
        self.scale_bar(ui, p, rect);
        self.attribution(ui, p, rect);
        if empty && let Some(a) = empty_state(ui, p, rect) {
            action = Some(a);
        }
        (rect, action)
    }

    /// A click on group `i` of level `z`: zoom in until it falls apart, or, if it never does
    /// (photos at one spot), open its strip (design.md §2.3).
    fn open_group(&mut self, z: usize, i: usize) {
        let Some(c) = self.clusters.clone() else {
            return;
        };
        let (_, nodes) = c.level(z as f64);
        let node = nodes[i];
        match c.split_level(z, i) {
            Some(split) => {
                // Fit its extent, but never less than the level where it splits: the fit keeps
                // a margin and stops at street level, which could leave it one marker.
                let fit = fit_zoom(node.bounds(), self.last_rect.size());
                let target = fit.max(split as f64).min(MAX_LEVEL as f64);
                let b = node.bounds();
                self.memory.center_at(lat_lon(
                    (b.min_lat + b.max_lat) / 2.0,
                    (b.min_lon + b.max_lon) / 2.0,
                ));
                let _ = self.memory.set_zoom(target);
            }
            None => {
                let pts = c.points();
                let photos = c
                    .members(z, i)
                    .into_iter()
                    .map(|m| (pts[m as usize].id, pts[m as usize].taken))
                    .collect();
                self.strip = Some(Strip::new(photos, node.lat_lon(), z));
            }
        }
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

/// The photos on the map (design.md §2.3, §2.4): per node of the current zoom level a marker
/// (a photo, or a group with its count), or a dot for a single photo whose thumbnail isn't made
/// yet. Before the first grouping is built, every photo is a dot. A dot is a pin-coloured dot in
/// a white ring with a dark edge, so it reads on pale land and on saturated tiles alike.
struct Photos<'a> {
    clusters: Option<&'a Clusters>,
    points: &'a [PhotoPoint],
    textures: &'a mut Textures,
    names: &'a HashMap<i64, String>,
    wanted: &'a mut Vec<i64>,
    palette: &'a Palette,
    selected: Option<i64>,
    selected_node: Option<(usize, usize)>,
    strip: Option<&'a Strip>,
    out: &'a mut PhotosOut,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Clicked {
    Photo(i64),
    /// A group: its level and index.
    Group(usize, usize),
}

#[derive(Default)]
struct PhotosOut {
    hovered: Option<(i64, Rect)>,
    clicked: Option<Clicked>,
    background_clicked: bool,
    /// The level drawn this frame.
    level: Option<usize>,
    /// Where the open strip points: just above its group's marker.
    strip_tip: Option<egui::Pos2>,
}

/// Dots closer than this many pixels are drawn once: a dot is 12 px wide, so closer neighbours
/// can't be told apart anyway (only before the first grouping, when every photo is a dot).
const DOT_CELL: f32 = 5.0;

/// One marker to draw this frame.
struct Shown {
    level_index: usize,
    pos: egui::Pos2,
    count: u32,
    /// The photo it shows (the group's representative).
    rep: PhotoPoint,
    /// Screen radius of the area a group covers (for the hover halo).
    reach: f32,
    /// Stable across rebuilds while the group is the same (its centre), for egui's hit ids.
    key: (u64, u64),
}

impl Plugin for Photos<'_> {
    fn run(
        self: Box<Self>,
        ui: &mut Ui,
        response: &egui::Response,
        projector: &Projector,
        memory: &MapMemory,
    ) {
        let p = self.palette;
        let clip = ui.clip_rect();
        let screen = |lat: f64, lon: f64| {
            let v = projector.project(lat_lon(lat, lon));
            egui::pos2(v.x, v.y)
        };
        let dot = |ui: &Ui, pos: egui::Pos2| {
            let painter = ui.painter();
            // Two shapes per dot: the dark edge, then the pin colour with its white ring.
            painter.circle_filled(pos, 6.0, p.map_marker_edge);
            painter.circle(pos, 4.0, p.map_pin, Stroke::new(2.0, p.map_marker_frame));
        };
        if response.clicked() {
            self.out.background_clicked = true;
        }

        let Some(c) = self.clusters else {
            let dot_clip = clip.expand(8.0);
            let mut seen = HashSet::new();
            for pt in self.points {
                let pos = screen(pt.lat, pt.lon);
                let cell = ((pos.x / DOT_CELL) as i32, (pos.y / DOT_CELL) as i32);
                if dot_clip.contains(pos) && seen.insert(cell) {
                    dot(ui, pos);
                }
            }
            return;
        };

        let (level, nodes) = c.level(memory.zoom());
        self.out.level = Some(level);
        let pts = c.points();
        // Cull in world units first (cheap): the visible area plus a marker's size around it.
        let world = |pos: egui::Pos2| {
            let g = projector.unproject(egui::vec2(pos.x, pos.y));
            cluster::mercator(g.y(), g.x())
        };
        let view = clip.expand(p.size_marker + 24.0);
        let (x0, y0) = world(view.left_top());
        let (x1, y1) = world(view.right_bottom());
        // Zoomed far out the view can be wider than the world; then nothing is culled here.
        let cull = x0 < x1 && y0 < y1;
        let mut shown = Vec::new();
        for (i, n) in nodes.iter().enumerate() {
            if cull && (n.x < x0 || n.x > x1 || n.y < y0 || n.y > y1) {
                continue;
            }
            let (lat, lon) = n.lat_lon();
            let pos = screen(lat, lon);
            if !clip.intersects(markers::frame_rect(p, pos, true).expand(16.0)) {
                continue;
            }
            let rep = pts[n.rep as usize];
            if n.count == 1 && rep.thumb == Thumb::Pending {
                dot(ui, pos);
                continue;
            }
            let reach = if n.count > 1 {
                let b = n.bounds();
                [
                    (b.min_lat, b.min_lon),
                    (b.min_lat, b.max_lon),
                    (b.max_lat, b.min_lon),
                    (b.max_lat, b.max_lon),
                ]
                .iter()
                .map(|&(a, o)| screen(a, o).distance(pos))
                .fold(0.0, f32::max)
            } else {
                0.0
            };
            shown.push(Shown {
                level_index: i,
                pos,
                count: n.count,
                rep,
                reach,
                key: (n.x.to_bits(), n.y.to_bits()),
            });
        }
        // Southern markers in front (they're nearer the viewer's eye line).
        shown.sort_by(|a, b| a.pos.y.total_cmp(&b.pos.y));

        // Interaction first (egui hit-tests against last frame's rects, so the order of
        // registration is the stacking order), then painting with the hover known.
        let mut hovered = None;
        for m in &shown {
            let rect = markers::frame_rect(p, m.pos, false);
            let hit = rect.union(Rect::from_center_size(m.pos, egui::vec2(10.0, 12.0)));
            let id = if m.count > 1 {
                Id::unique(("photo-group", level, m.key))
            } else {
                Id::unique(("photo-marker", m.rep.id))
            };
            let r = ui.interact(hit, id, Sense::click());
            let name = if m.count > 1 {
                format!("Group of {} photos", m.count)
            } else {
                match self.names.get(&m.rep.id) {
                    Some(n) => format!("Photo {n}"),
                    None => {
                        self.wanted.push(m.rep.id);
                        "Photo".to_owned()
                    }
                }
            };
            widgets::named(&r, &name, true);
            if r.hovered() {
                hovered = Some(m.level_index);
            }
            if r.clicked() {
                self.out.clicked = Some(if m.count > 1 {
                    Clicked::Group(level, m.level_index)
                } else {
                    Clicked::Photo(m.rep.id)
                });
            }
        }

        let is_selected = |m: &Shown| {
            self.selected == Some(m.rep.id)
                || self.selected_node == Some((level, m.level_index))
                || self.strip.is_some_and(|s| s.contains(m.rep.id))
        };
        let mut last = Vec::new();
        for m in &shown {
            let (hover, sel) = (hovered == Some(m.level_index), is_selected(m));
            if sel && self.strip.is_some() {
                self.out.strip_tip = Some(egui::pos2(
                    m.pos.x,
                    markers::frame_rect(p, m.pos, false).top() - 16.0,
                ));
            }
            if hover || sel {
                last.push((m, hover, sel)); // drawn on top
                continue;
            }
            let face = face(self.textures, &m.rep);
            markers::paint(ui, p, m.pos, face, m.count, false, false);
        }
        last.sort_by_key(|(_, hover, _)| *hover); // the hovered one topmost
        for (m, hover, sel) in last {
            if hover && m.count > 1 {
                // The halo: the area the group covers.
                ui.painter()
                    .circle_filled(m.pos, m.reach.clamp(24.0, 600.0), p.map_cluster_halo);
            }
            if hover && m.count == 1 {
                self.out.hovered = Some((m.rep.id, markers::frame_rect(p, m.pos, true)));
            }
            let face = face(self.textures, &m.rep);
            markers::paint(ui, p, m.pos, face, m.count, hover, sel);
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

/// Builds the groups off the UI thread. Requests are coalesced (only the newest snapshot is
/// built); when only thumbnail states or dates changed, the previous groups are kept and just
/// their representatives chosen again.
struct Builder {
    requests: Sender<Vec<PhotoPoint>>,
    results: Receiver<Arc<Clusters>>,
}

impl Builder {
    fn new(ctx: egui::Context) -> Self {
        let (requests, rx) = mpsc::channel::<Vec<PhotoPoint>>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("mapcraft-groups".into())
            .spawn(move || {
                let mut last: Option<Arc<Clusters>> = None;
                while let Ok(mut points) = rx.recv() {
                    while let Ok(newer) = rx.try_recv() {
                        points = newer;
                    }
                    let c = Arc::new(match &last {
                        Some(l) if l.same_positions(&points) => l.with_points(points),
                        _ => {
                            let t = std::time::Instant::now();
                            let n = points.len();
                            let c = Clusters::build(points);
                            tracing::debug!("grouped {n} photos in {:?}", t.elapsed());
                            c
                        }
                    });
                    last = Some(c.clone());
                    if tx.send(c).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn the group builder");
        Self { requests, results }
    }

    fn request(&self, points: Vec<PhotoPoint>) {
        let _ = self.requests.send(points);
    }

    /// The newest finished build since last asked, if any.
    fn latest(&self) -> Option<Arc<Clusters>> {
        self.results.try_iter().last()
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

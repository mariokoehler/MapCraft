//! The map and everything drawn over it (design system "Map layout"): controls top-right, scale
//! bar bottom-left, the provider's attribution flush bottom-right of the *visible* map, and in
//! M1 the empty-library card in the middle.

use std::path::Path;

use egui::{
    Align2, Area, Button, CornerRadius, Frame, Id, Margin, Order, Rect, RichText, Sense, Stroke,
    Ui, text::LayoutJob,
};
use walkers::{HttpTiles, Map, MapMemory, MercatorProjection, Position, Projection, lat_lon};

use crate::provider::Provider;
use crate::theme::Palette;
use crate::ui::icons::Icon;

/// Where the map opens while the library is empty: central Europe.
const HOME: (f64, f64, f64) = (50.0, 10.0, 4.0); // lat, lon, zoom

pub struct MapView {
    provider: Provider,
    tiles: HttpTiles,
    memory: MapMemory,
    home: Position,
}

impl MapView {
    pub fn new(provider: Provider, tile_cache: &Path, ctx: egui::Context) -> Self {
        let mut memory = MapMemory::default();
        // A zoom within the tile range never fails; ignore the error rather than unwrap.
        let _ = memory.set_zoom(HOME.2);
        Self {
            provider,
            tiles: provider.tiles(tile_cache, ctx),
            memory,
            home: lat_lon(HOME.0, HOME.1),
        }
    }

    pub fn show(&mut self, ui: &mut Ui, p: &Palette) {
        let map = Map::new(Some(&mut self.tiles), &mut self.memory, self.home)
            .zoom_with_ctrl(false) // the wheel zooms (design.md §2.3)
            .double_click_to_zoom(true);
        let rect = ui.add(map).rect;
        self.zoom_keys(ui);

        self.controls(ui, p, rect);
        self.scale_bar(ui, p, rect);
        self.attribution(ui, p, rect);
        empty_state(ui, p, rect);
    }

    /// `+`/`-` zoom (design.md §2.3; walkers has no keyboard handling). `=` counts as `+`, since
    /// that's the unshifted key on many layouts. Ignored while a text field has the keyboard.
    fn zoom_keys(&mut self, ui: &Ui) {
        if ui.ctx().egui_wants_keyboard_input() {
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

    /// Zoom in/out work; "zoom to all photos" and "map style" have nothing to act on yet.
    fn controls(&mut self, ui: &Ui, p: &Palette, map: Rect) {
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
                    control(
                        ui,
                        p,
                        Icon::Scan,
                        "Zoom to all photos: no photos yet",
                        false,
                    );
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
    if enabled {
        response.on_hover_text(hint)
    } else {
        response.on_disabled_hover_text(hint)
    }
}

/// The card shown while there are no photos. "Add folder…" arrives with M2.
fn empty_state(ui: &Ui, p: &Palette, map: Rect) {
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
                        ui.add_enabled(false, add)
                            .on_disabled_hover_text("Importing photos isn't available yet");
                    });
                });
        });
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

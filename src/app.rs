//! The main window: top bar above, the map filling the rest (design system "Map layout").

use std::path::Path;

use egui::{Frame, Panel, Stroke};

use crate::provider::Provider;
use crate::theme::{self, Palette, ThemeKind, Tokens};
use crate::ui::{map_view::MapView, top_bar};

pub struct MapCraftApp {
    palette: Palette,
    map: MapView,
}

impl MapCraftApp {
    pub fn new(cc: &eframe::CreationContext<'_>, tile_cache: &Path) -> Self {
        // The tokens are compiled in and covered by tests, so a failure here is a build defect.
        let tokens = Tokens::embedded(ThemeKind::Dark).expect("embedded design tokens");
        theme::apply(&cc.egui_ctx, &tokens, ThemeKind::Dark);
        Self {
            palette: Palette::new(&tokens),
            map: MapView::new(Provider::default(), tile_cache, cc.egui_ctx.clone()),
        }
    }
}

impl eframe::App for MapCraftApp {
    fn ui(&mut self, ui: &mut egui::Ui, _frame: &mut eframe::Frame) {
        let p = &self.palette;
        let bar = Panel::top("top-bar")
            .exact_size(p.size_topbar)
            .frame(Frame::NONE.fill(p.bg_panel))
            .show(ui, |ui| top_bar::show(ui, p));
        // The hairline under the top bar.
        let r = bar.response.rect;
        ui.painter().hline(
            r.x_range(),
            r.bottom() - p.stroke_hairline / 2.0,
            Stroke::new(p.stroke_hairline, p.border_subtle),
        );

        egui::CentralPanel::default()
            .frame(Frame::NONE.fill(p.map_ref_land))
            .show(ui, |ui| self.map.show(ui, p));
    }
}

//! The top bar (36 px): app mark, search, actions. In M1 search and the actions have no
//! behaviour yet, so they are drawn disabled (search arrives in M5, "Add folder…" in M2).

use egui::{Align, Button, Image, ImageSource, Layout, RichText, Sense, Stroke, StrokeKind, Ui};

use crate::theme::Palette;
use crate::ui::icons::Icon;

const MARK: &[u8] = include_bytes!("../../assets/brand/mapcraft-mark-32.svg");

pub fn show(ui: &mut Ui, p: &Palette) {
    ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
        ui.add_space(p.space_2);
        ui.add(
            Image::new(ImageSource::Bytes {
                uri: "bytes://brand/mapcraft-mark-32.svg".into(),
                bytes: MARK.into(),
            })
            .fit_to_exact_size(egui::vec2(p.size_icon_md, p.size_icon_md)),
        )
        .on_hover_text("MapCraft");
        ui.add_space(p.space_1);
        search_field(ui, p);

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(p.space_1);
            ghost_button(ui, p, Icon::Settings, "Settings (coming soon)");
            ghost_button(ui, p, Icon::FolderPlus, "Add folder… (coming with M2)");
        });
    });
}

/// The search field, disabled until there's something to search (design system: "Disabled (no
/// photos yet)").
fn search_field(ui: &mut Ui, p: &Palette) {
    let size = egui::vec2(320.0, ui.spacing().interact_size.y);
    let (rect, response) = ui.allocate_exact_size(size, Sense::hover());
    let painter = ui.painter();
    painter.rect(
        rect,
        p.radius_sm,
        p.bg_extreme,
        Stroke::new(p.stroke_hairline, p.border_subtle),
        StrokeKind::Inside,
    );
    let icon_rect = egui::Rect::from_center_size(
        egui::pos2(
            rect.left() + p.space_2 + p.size_icon_sm / 2.0,
            rect.center().y,
        ),
        egui::vec2(p.size_icon_sm, p.size_icon_sm),
    );
    Icon::Search
        .image(p.size_icon_sm, p.text_disabled)
        .paint_at(ui, icon_rect);
    painter.text(
        egui::pos2(icon_rect.right() + p.space_1, rect.center().y),
        egui::Align2::LEFT_CENTER,
        "Search places, files, cameras",
        egui::FontId::proportional(13.0),
        p.text_disabled,
    );
    response.on_hover_text("Search arrives with M5, once photos are imported");
}

fn ghost_button(ui: &mut Ui, p: &Palette, icon: Icon, hint: &str) {
    let size = ui.spacing().interact_size.y;
    // Disabled until they have behaviour; egui doesn't fade a tinted image, so tint it here.
    let button = Button::image(icon.image(p.size_icon_sm, p.text_disabled))
        .frame_when_inactive(false)
        .min_size(egui::vec2(size, size));
    ui.add_enabled(false, button)
        .on_disabled_hover_text(RichText::new(hint));
}

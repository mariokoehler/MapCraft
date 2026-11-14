//! The top bar (36 px): app mark, search, counts, import progress, actions (canvas
//! `MapCraft — Import`). Search and settings have no behaviour yet, so they are drawn disabled
//! (search arrives in M5).

use egui::{Align, Image, ImageSource, Layout, RichText, Sense, Stroke, StrokeKind, Ui};

use crate::library::Counts;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, thousands};

const MARK: &[u8] = include_bytes!("../../assets/brand/mapcraft-mark-32.svg");

/// A running import as the top bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    /// `None` while the folders are still being listed.
    pub progress: Option<(usize, usize)>,
    pub cancelling: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopBarAction {
    AddFolder,
    Sources,
    CancelImport,
}

pub fn show(
    ui: &mut Ui,
    p: &Palette,
    counts: Counts,
    running: Option<Running>,
) -> Option<TopBarAction> {
    let mut action = None;
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
            widgets::ghost(ui, p, Icon::Settings, "Settings: not available yet", false);
            let busy = running.is_some();
            if widgets::ghost(ui, p, Icon::Folders, "Sources", true).clicked() {
                action = Some(TopBarAction::Sources);
            }
            let add_hint = if busy {
                "Add folder…: wait for the import to finish"
            } else {
                "Add folder…"
            };
            if widgets::ghost(ui, p, Icon::FolderPlus, add_hint, !busy).clicked() {
                action = Some(TopBarAction::AddFolder);
            }
            if let Some(r) = running
                && progress_chip(ui, p, r)
            {
                action = Some(TopBarAction::CancelImport);
            }
            if counts.photos > 0 {
                ui.label(
                    RichText::new(counts_text(counts))
                        .small()
                        .color(p.text_weak),
                );
            }
        });
    });
    action
}

/// "4,881 photos · 1,019 without a position" (design system: say it with the number).
pub fn counts_text(c: Counts) -> String {
    let present = (c.photos - c.missing).max(0) as usize;
    let mut s = format!("{} photos", thousands(present));
    let without = c.without_position().max(0) as usize;
    if without > 0 {
        s.push_str(&format!(" · {} without a position", thousands(without)));
    }
    if c.missing > 0 {
        s.push_str(&format!(" · {} missing", thousands(c.missing as usize)));
    }
    s
}

/// The progress chip ("Importing 1,204 / 2,053") with its cancel button; true when cancelled.
fn progress_chip(ui: &mut Ui, p: &Palette, r: Running) -> bool {
    let mut cancel = false;
    egui::Frame::NONE
        .fill(p.bg_extreme)
        .stroke(Stroke::new(1.0, p.border_subtle))
        .corner_radius(255)
        .inner_margin(egui::Margin::symmetric(8, 2))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 6.0;
            // Right to left: cancel, bar, text, spinner.
            cancel = widgets::ghost(
                ui,
                p,
                Icon::X,
                "Cancel import (it resumes next time)",
                !r.cancelling,
            )
            .clicked();
            let (text, fraction) = match (r.cancelling, r.progress) {
                (true, _) => ("Stopping…".to_owned(), None),
                (false, None) => ("Listing folders…".to_owned(), None),
                (false, Some((done, total))) => (
                    format!("Importing {} / {}", thousands(done), thousands(total)),
                    Some(if total == 0 {
                        1.0
                    } else {
                        done as f32 / total as f32
                    }),
                ),
            };
            if let Some(f) = fraction {
                let (rect, _) = ui.allocate_exact_size(egui::vec2(60.0, 4.0), Sense::hover());
                ui.painter().rect_filled(rect, 2, p.border_subtle);
                let mut done = rect;
                done.set_width(rect.width() * f.clamp(0.0, 1.0));
                ui.painter().rect_filled(done, 2, p.accent);
            }
            ui.label(RichText::new(text).small().color(p.text_normal));
            ui.add(egui::Spinner::new().size(12.0).color(p.text_weak));
        });
    cancel
}

/// The search field, disabled until M5 (design system: "Disabled (no photos yet)").
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
    response.on_hover_text("Search: not available yet");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn counts_leave_out_missing_photos_and_name_them() {
        let c = Counts {
            photos: 5000,
            with_position: 3862,
            missing: 119,
        };
        assert_eq!(
            counts_text(c),
            "4,881 photos · 1,019 without a position · 119 missing"
        );
        let all_placed = Counts {
            photos: 10,
            with_position: 10,
            missing: 0,
        };
        assert_eq!(counts_text(all_placed), "10 photos");
    }
}

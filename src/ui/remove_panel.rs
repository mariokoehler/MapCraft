//! The panel of removal mode (design.md §2.9(n), canvas `MapCraft — Remove position`,
//! artboards 2 and 5): the last removal's Undo, which photos, the counts, and Remove positions.
//! It only reads the session and says what was asked; the app changes the session.

use egui::{Rect, RichText, Sense, Stroke, Ui};

use crate::geotag::removal::{Removal, Scope};
use crate::theme::{Palette, SEMIBOLD};
use crate::ui::geotag_panel::{section, weak};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Tone, thousands};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum RemoveAction {
    Leave,
    Scope(Scope),
    TickAll,
    UntickAll,
    Remove,
    CancelWrite,
    /// Undo the newest removal (its journal), after its toast is gone.
    UndoLast,
}

/// `writing`: the removal's progress while it runs; `last`: what the newest removal journal
/// holds ("12 photos, 2026-11-18 14:02 UTC").
pub fn show(
    ui: &mut Ui,
    p: &Palette,
    r: &Removal,
    writing: Option<(usize, usize)>,
    last: Option<&str>,
) -> Option<RemoveAction> {
    let mut action = None;
    // Header.
    let (header, _) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), p.size_topbar),
        Sense::hover(),
    );
    ui.painter().hline(
        header.x_range(),
        header.bottom() - p.stroke_hairline / 2.0,
        Stroke::new(p.stroke_hairline, p.border_subtle),
    );
    let inner = Rect::from_min_max(
        header.min + egui::vec2(p.space_3, 0.0),
        header.max - egui::vec2(p.panel_margin, 0.0),
    );
    ui.scope_builder(egui::UiBuilder::new().max_rect(inner), |ui| {
        ui.with_layout(egui::Layout::left_to_right(egui::Align::Center), |ui| {
            ui.label(
                RichText::new("Remove positions")
                    .family(egui::FontFamily::Name(SEMIBOLD.into()))
                    .size(15.0)
                    .color(p.text_strong),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost(
                    ui,
                    p,
                    Icon::X,
                    "Leave removal mode (Esc)",
                    writing.is_none(),
                )
                .clicked()
                {
                    action = Some(RemoveAction::Leave);
                }
            });
        });
    });

    // The footer first, at the bottom; the sections scroll above it.
    let footer_h = 44.0;
    let body = ui.available_rect_before_wrap();
    let footer = Rect::from_min_max(egui::pos2(body.left(), body.bottom() - footer_h), body.max);
    ui.painter().hline(
        footer.x_range(),
        footer.top(),
        Stroke::new(p.stroke_hairline, p.border_subtle),
    );
    ui.scope_builder(
        egui::UiBuilder::new().max_rect(footer.shrink2(egui::vec2(p.space_3, p.space_2))),
        |ui| {
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if let Some((done, total)) = writing {
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(RemoveAction::CancelWrite);
                    }
                    ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
                    ui.label(
                        RichText::new(format!(
                            "Removing {} of {}…",
                            thousands(done.min(total)),
                            thousands(total)
                        ))
                        .color(p.text_normal),
                    );
                } else {
                    let n = r.ticked();
                    let text = match n {
                        0 => "Remove positions…".to_owned(),
                        1 => "Remove position from 1 photo…".to_owned(),
                        n => format!("Remove positions from {} photos…", thousands(n)),
                    };
                    if widgets::danger(ui, p, &text, n > 0).clicked() {
                        action = Some(RemoveAction::Remove);
                    }
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(RemoveAction::Leave);
                    }
                }
            });
        },
    );

    let rest = Rect::from_min_max(body.min, egui::pos2(body.right(), footer.top()));
    ui.scope_builder(egui::UiBuilder::new().max_rect(rest), |ui| {
        ui.add_enabled_ui(writing.is_none(), |ui| {
            egui::ScrollArea::vertical()
                .auto_shrink([false, false])
                .show(ui, |ui| {
                    ui.spacing_mut().item_spacing.y = 0.0;
                    if let Some(last) = last {
                        section(ui, p, "Last removed", |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(last).color(p.text_normal));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::secondary(ui, p, None, "Undo it", true)
                                            .clicked()
                                        {
                                            action = Some(RemoveAction::UndoLast);
                                        }
                                    },
                                );
                            });
                            weak(
                                ui,
                                p,
                                "Puts the sidecars back as they were, if no other app changed \
                                 them since.",
                            );
                        });
                    }
                    scope_section(ui, p, r, &mut action);
                    photos_section(ui, p, r, &mut action);
                    section(ui, p, "What happens", |ui| {
                        weak(
                            ui,
                            p,
                            "Only the GPS properties leave each sidecar; everything else in it \
                             stays as it is. A sidecar left with nothing else is deleted. Undo \
                             puts them back.",
                        );
                    });
                });
        });
    });
    action
}

fn scope_section(ui: &mut Ui, p: &Palette, r: &Removal, action: &mut Option<RemoveAction>) {
    section(ui, p, "Which photos", |ui| {
        ui.horizontal(|ui| {
            ui.label("Show");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for (scope, label) in [
                    (Scope::Map, "On the map now"),
                    (Scope::Folder, "This folder"),
                ] {
                    if ui.selectable_label(r.scope == scope, label).clicked() && r.scope != scope {
                        *action = Some(RemoveAction::Scope(scope));
                    }
                }
            });
        });
        let place = match r.scope {
            Scope::Folder => r.folder.as_str(),
            Scope::Map => "Every photo the filters let through",
        };
        ui.add(egui::Label::new(RichText::new(place).monospace().small()).truncate());
        weak(
            ui,
            p,
            "Tick the photos whose position is wrong. A group’s tick sets all its photos; at \
             one spot, the strip shows each with its own tick.",
        );
    });
}

fn photos_section(ui: &mut Ui, p: &Palette, r: &Removal, action: &mut Option<RemoveAction>) {
    section(ui, p, "Photos", |ui| {
        ui.label(
            RichText::new(format!(
                "{} ticked of {} with a position from a sidecar",
                thousands(r.ticked()),
                thousands(r.removable())
            ))
            .color(p.text_strong),
        );
        ui.horizontal(|ui| {
            if widgets::secondary(ui, p, None, "Tick all", r.ticked() < r.removable()).clicked() {
                *action = Some(RemoveAction::TickAll);
            }
            if widgets::secondary(ui, p, None, "Untick all", r.ticked() > 0).clicked() {
                *action = Some(RemoveAction::UntickAll);
            }
        });
        let n = r.in_file();
        if n > 0 {
            ui.add_space(p.space_1);
            widgets::banner(
                ui,
                p,
                Tone::Info,
                &format!(
                    "{} more {} position inside the photo file. MapCraft doesn’t change \
                     originals, so {} shown faded, without a tick.",
                    thousands(n),
                    if n == 1 { "has its" } else { "have their" },
                    if n == 1 { "it’s" } else { "they’re" },
                ),
            );
        }
    });
}

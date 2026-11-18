//! The top bar (36 px): app mark, search, counts, import progress, actions (canvas
//! `MapCraft — Import`), the Filters button (`ui::filters`) and Settings (search: `ui::search`).

use egui::{Align, Image, ImageSource, Layout, RichText, Sense, Stroke, Ui};

use crate::library::Counts;
use crate::theme::Palette;
use crate::ui::filters::FilterPanel;
use crate::ui::icons::Icon;
use crate::ui::search::SearchBox;
use crate::ui::widgets::{self, thousands};

const MARK: &[u8] = include_bytes!("../../assets/brand/mapcraft-mark-32.svg");

/// A running background task as the top bar shows it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Running {
    pub task: Task,
    /// `None` while the folders are still being listed.
    pub progress: Option<(usize, usize)>,
    pub cancelling: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Task {
    Import,
    Thumbnails,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TopBarAction {
    AddFolder,
    Sources,
    /// Cancel the running task.
    Cancel,
    /// The Filters button opened or closed its panel.
    Filters,
    Settings,
    /// Tag photos from a GPS track (design.md §2.9).
    Geotag,
}

/// Tagging mode as the top bar sees it: on (everything else waits), or why it can't start.
/// `removing`: removal mode is on instead (design.md §2.9(n)); the bar waits as for tagging.
#[derive(Debug, Clone, Copy, Default)]
pub struct Tagging {
    pub on: bool,
    pub removing: bool,
    pub blocked: Option<&'static str>,
}

/// The filters as the top bar shows them: the panel's button, how many are set, and how many
/// photos they leave on the map (`None` while none is set).
pub struct BarFilters<'a> {
    pub panel: &'a mut FilterPanel,
    pub active: usize,
    pub shown: Option<usize>,
}

pub fn show(
    ui: &mut Ui,
    p: &Palette,
    counts: Counts,
    running: Option<Running>,
    search: &mut SearchBox,
    filters: BarFilters<'_>,
    tagging: Tagging,
) -> Option<TopBarAction> {
    let free = !tagging.on && !tagging.removing; // in either mode the rest of the bar waits
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
        ui.add_enabled_ui(free, |ui| {
            search.field(ui, p);
            if counts.with_position > 0 && filters.panel.button(ui, p, filters.active) {
                action = Some(TopBarAction::Filters);
            }
        });

        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.add_space(p.space_1);
            if widgets::ghost(ui, p, Icon::Settings, "Settings", free).clicked() {
                action = Some(TopBarAction::Settings);
            }
            // Folders can be added while thumbnails are made: the import waits for them to stop.
            let busy = running.is_some_and(|r| r.task == Task::Import);
            if widgets::ghost(ui, p, Icon::Folders, "Sources", free).clicked() {
                action = Some(TopBarAction::Sources);
            }
            let add_hint = if busy {
                "Add folder…: wait for the import to finish"
            } else {
                "Add folder…"
            };
            if widgets::ghost(ui, p, Icon::FolderPlus, add_hint, !busy && free).clicked() {
                action = Some(TopBarAction::AddFolder);
            }
            if counts.photos > 0 {
                let hint = match (tagging.on, tagging.blocked) {
                    (true, _) => "Tag photos from a GPS track…: tagging is open",
                    _ if tagging.removing => {
                        "Tag photos from a GPS track…: removing positions is open"
                    }
                    (false, Some(why)) => why,
                    (false, None) => "Tag photos from a GPS track…",
                };
                let ok = free && tagging.blocked.is_none();
                if widgets::ghost(ui, p, Icon::Route, hint, ok).clicked() {
                    action = Some(TopBarAction::Geotag);
                }
            }
            if let Some(r) = running
                && progress_chip(ui, p, r)
            {
                action = Some(TopBarAction::Cancel);
            }
            if counts.photos > 0 {
                ui.label(
                    RichText::new(counts_text(counts, filters.shown))
                        .small()
                        .color(p.text_weak),
                );
            }
        });
    });
    action
}

/// "4,881 photos · 1,019 without a position" (design system: say it with the number). With
/// filters set, "2,015 of 3,862 photos shown · …": of those on the map, the ones shown.
pub fn counts_text(c: Counts, shown: Option<usize>) -> String {
    let present = (c.photos - c.missing).max(0) as usize;
    let mut s = match shown {
        Some(n) => format!(
            "{} of {} photos shown",
            thousands(n),
            thousands(c.with_position.max(0) as usize)
        ),
        None => format!("{} photos", thousands(present)),
    };
    let without = c.without_position().max(0) as usize;
    if without > 0 {
        s.push_str(&format!(" · {} without a position", thousands(without)));
    }
    if c.missing > 0 {
        s.push_str(&format!(" · {} missing", thousands(c.missing as usize)));
    }
    s
}

/// The progress chip ("Importing 1,204 / 2,053", "Making thumbnails 80 / 3,800") with its
/// cancel button; true when cancelled.
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
            let (hint, listing, doing) = match r.task {
                Task::Import => (
                    "Cancel import (it resumes next time)",
                    "Listing folders…",
                    "Importing",
                ),
                Task::Thumbnails => (
                    "Stop making thumbnails (they continue next time)",
                    "Preparing thumbnails…",
                    "Making thumbnails",
                ),
            };
            cancel = widgets::ghost(ui, p, Icon::X, hint, !r.cancelling).clicked();
            let (text, fraction) = match (r.cancelling, r.progress) {
                (true, _) => ("Stopping…".to_owned(), None),
                (false, None) => (listing.to_owned(), None),
                (false, Some((done, total))) => (
                    format!("{doing} {} / {}", thousands(done), thousands(total)),
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
            counts_text(c, None),
            "4,881 photos · 1,019 without a position · 119 missing"
        );
        let all_placed = Counts {
            photos: 10,
            with_position: 10,
            missing: 0,
        };
        assert_eq!(counts_text(all_placed, None), "10 photos");
        assert_eq!(
            counts_text(c, Some(2015)),
            "2,015 of 3,862 photos shown · 1,019 without a position · 119 missing"
        );
    }
}

//! The strip: the photos of a group that can't split any further (taken at one spot), shown
//! above its marker (design.md §2.3; canvas `MapCraft — Main window`, artboard "Selected").
//!
//! It's keyed by photo ids, not by a group: groups are rebuilt as photos come and go, the ids
//! stay. Only the visible page's thumbnails are requested.

use egui::{Align2, Area, Color32, Frame, Margin, Order, Pos2, Rect, RichText, Sense, Shape, Ui};

use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::markers::{Look, Textures};
use crate::ui::widgets;

/// Thumbnails per page (the mockup's row).
pub const PAGE: usize = 8;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StripAction {
    Close,
    Select(i64),
}

pub struct Strip {
    /// The photos, oldest first, with their capture times (`yyyymmddhhmmss`, 0 if undated).
    photos: Vec<(i64, i64)>,
    /// Where it points: the group's position.
    pub anchor: (f64, f64),
    /// The zoom level it was opened at; it closes when the level changes.
    pub level: usize,
    page: usize,
}

impl Strip {
    pub fn new(mut photos: Vec<(i64, i64)>, anchor: (f64, f64), level: usize) -> Self {
        photos.sort_by_key(|&(id, taken)| (taken == 0, taken, id));
        Self {
            photos,
            anchor,
            level,
            page: 0,
        }
    }

    pub fn contains(&self, id: i64) -> bool {
        self.photos.iter().any(|&(p, _)| p == id)
    }

    pub fn len(&self) -> usize {
        self.photos.len()
    }

    pub fn is_empty(&self) -> bool {
        self.photos.is_empty()
    }

    /// Keeps only photos still in `alive` (a rescan removed some).
    pub fn retain(&mut self, alive: impl Fn(i64) -> bool) {
        self.photos.retain(|&(id, _)| alive(id));
        self.page = self.page.min(self.photos.len().saturating_sub(1) / PAGE);
    }

    /// Draws it with its pointer's tip at `tip` (just above the group's marker).
    #[allow(clippy::too_many_arguments)]
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        tip: Pos2,
        map: Rect,
        textures: &mut Textures,
        names: &std::collections::HashMap<i64, String>,
        wanted: &mut Vec<i64>,
        selected: Option<i64>,
    ) -> Option<StripAction> {
        let mut action = None;
        let pages = self.photos.len().div_ceil(PAGE).max(1);
        let ink = p.map_scrim_dark_text;
        let shown = Area::new(egui::Id::unique("group-strip"))
            .order(Order::Foreground)
            .pivot(Align2::CENTER_BOTTOM)
            .fixed_pos(tip - egui::vec2(0.0, 6.0))
            .constrain_to(map)
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.map_scrim_dark)
                    .corner_radius(p.radius_md)
                    .shadow(p.elevation_panel)
                    .inner_margin(Margin::same(p.space_2 as i8))
                    .show(ui, |ui| {
                        // As wide as the row: the header's right-aligned part would stretch it.
                        let k = self.photos.len().min(PAGE) as f32;
                        let row = 2.0 * 20.0 + k * p.size_strip_thumb + (k + 1.0) * 4.0;
                        ui.set_max_width(row.max(240.0));
                        ui.spacing_mut().item_spacing = egui::vec2(p.space_2, 6.0);
                        // Header: count, date, close.
                        ui.horizontal(|ui| {
                            ui.label(
                                RichText::new(format!(
                                    "{} photos at this spot",
                                    widgets::thousands(self.photos.len())
                                ))
                                .small()
                                .color(ink),
                            );
                            ui.with_layout(
                                egui::Layout::right_to_left(egui::Align::Center),
                                |ui| {
                                    if widgets::ghost_on(ui, p, Icon::X, "Close strip", ink)
                                        .clicked()
                                    {
                                        action = Some(StripAction::Close);
                                    }
                                    if let Some(d) = self.dates() {
                                        ui.label(
                                            RichText::new(d).small().color(ink.gamma_multiply(0.8)),
                                        );
                                    }
                                },
                            );
                        });
                        // The row: prev, a page of thumbnails, next.
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            if nav(ui, p, Icon::ChevronLeft, "Previous photos", self.page > 0) {
                                self.page -= 1;
                            }
                            let start = self.page * PAGE;
                            let end = (start + PAGE).min(self.photos.len());
                            for &(id, _) in &self.photos[start..end] {
                                if thumb(ui, p, id, textures, names, wanted, selected == Some(id)) {
                                    action = Some(StripAction::Select(id));
                                }
                            }
                            if nav(
                                ui,
                                p,
                                Icon::ChevronRight,
                                "Next photos",
                                self.page + 1 < pages,
                            ) {
                                self.page += 1;
                            }
                        });
                    })
            });
        // The pointer under the box.
        let r = shown.response.rect;
        let x = tip.x.clamp(r.left() + 12.0, r.right() - 12.0);
        ctx.layer_painter(shown.response.layer_id)
            .add(Shape::convex_polygon(
                vec![
                    egui::pos2(x - 6.0, r.bottom()),
                    egui::pos2(x + 6.0, r.bottom()),
                    egui::pos2(x, r.bottom() + 6.0),
                ],
                p.map_scrim_dark,
                egui::Stroke::NONE,
            ));
        action
    }

    /// `19 May 2023`, or `19 May – 3 Aug 2023` across days; `None` if none is dated.
    fn dates(&self) -> Option<String> {
        let dated: Vec<i64> = self
            .photos
            .iter()
            .map(|&(_, t)| t)
            .filter(|&t| t > 0)
            .collect();
        let (first, last) = (*dated.iter().min()?, *dated.iter().max()?);
        Some(day_range(first, last))
    }
}

/// `19 May 2023` or a range of days, from `yyyymmddhhmmss` keys.
pub fn day_range(first: i64, last: i64) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts = |k: i64| {
        let day = k / 1_000_000;
        (
            day / 10_000,
            (day / 100 % 100).clamp(1, 12) as usize,
            day % 100,
        )
    };
    let (y1, m1, d1) = parts(first);
    let (y2, m2, d2) = parts(last);
    let (n1, n2) = (MONTHS[m1 - 1], MONTHS[m2 - 1]);
    if (y1, m1, d1) == (y2, m2, d2) {
        format!("{d1} {n1} {y1}")
    } else if y1 == y2 {
        format!("{d1} {n1} – {d2} {n2} {y2}")
    } else {
        format!("{d1} {n1} {y1} – {d2} {n2} {y2}")
    }
}

/// A page arrow; true when clicked.
fn nav(ui: &mut Ui, p: &Palette, icon: Icon, hint: &str, enabled: bool) -> bool {
    let size = egui::vec2(20.0, p.size_strip_thumb);
    let sense = if enabled {
        Sense::click()
    } else {
        Sense::hover()
    };
    let (rect, r) = ui.allocate_exact_size(size, sense);
    widgets::named(&r, hint, enabled);
    let mut ink = p.map_scrim_dark_text;
    if !enabled {
        ink = ink.gamma_multiply(0.35);
    } else if r.hovered() {
        ui.painter()
            .rect_filled(rect, p.radius_sm, Color32::from_white_alpha(20));
    }
    let icon_rect = Rect::from_center_size(rect.center(), egui::vec2(16.0, 16.0));
    icon.image(16.0, ink).paint_at(ui, icon_rect);
    enabled && r.clicked()
}

/// One thumbnail of the row; true when clicked.
fn thumb(
    ui: &mut Ui,
    p: &Palette,
    id: i64,
    textures: &mut Textures,
    names: &std::collections::HashMap<i64, String>,
    wanted: &mut Vec<i64>,
    selected: bool,
) -> bool {
    let s = p.size_strip_thumb;
    let (rect, r) = ui.allocate_exact_size(egui::vec2(s, s), Sense::click());
    let name = match names.get(&id) {
        Some(n) => format!("Photo {n}"),
        None => {
            wanted.push(id);
            "Photo".to_owned()
        }
    };
    widgets::named(&r, &name, true);
    let painter = ui.painter();
    // Selected: the selection outline with a frame ring outside it; hovered: a frame ring.
    if selected {
        painter.rect_filled(rect.expand(3.0), 4, p.map_marker_frame);
        painter.rect_filled(rect.expand(p.stroke_focus), 3, p.map_selection_outline);
    } else if r.hovered() {
        painter.rect_filled(rect.expand(p.stroke_focus), 3, p.map_marker_frame);
    }
    match textures.get(id) {
        Look::Ready(t) => {
            let [w, h] = t.size().map(|v| v as f32);
            let uv = if w > h {
                let m = (1.0 - h / w) / 2.0;
                Rect::from_min_max(egui::pos2(m, 0.0), egui::pos2(1.0 - m, 1.0))
            } else {
                let m = (1.0 - w / h) / 2.0;
                Rect::from_min_max(egui::pos2(0.0, m), egui::pos2(1.0, 1.0 - m))
            };
            painter.image(t.id(), rect, uv, Color32::WHITE);
        }
        _ => {
            painter.rect_filled(rect, 2, p.map_marker_placeholder);
            let icon = Rect::from_center_size(rect.center(), egui::vec2(18.0, 18.0));
            Icon::Image
                .image(18.0, p.map_marker_placeholder_fg)
                .paint_at(ui, icon);
        }
    }
    r.clicked()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn day_ranges() {
        assert_eq!(day_range(20230519120000, 20230519235959), "19 May 2023");
        assert_eq!(
            day_range(20230519120000, 20230803000000),
            "19 May – 3 Aug 2023"
        );
        assert_eq!(
            day_range(20221231120000, 20230101000000),
            "31 Dec 2022 – 1 Jan 2023"
        );
    }

    #[test]
    fn photos_sort_oldest_first_with_undated_last_and_pages_shrink() {
        let mut s = Strip::new(
            vec![(3, 0), (1, 20230601000000), (2, 20230501000000)],
            (0.0, 0.0),
            19,
        );
        assert_eq!(
            s.photos.iter().map(|p| p.0).collect::<Vec<_>>(),
            vec![2, 1, 3]
        );
        assert_eq!(s.dates().as_deref(), Some("1 May – 1 Jun 2023"));
        s.page = 0;
        s.retain(|id| id != 1);
        assert_eq!(s.len(), 2);
        assert!(s.contains(3) && !s.contains(1));
    }
}

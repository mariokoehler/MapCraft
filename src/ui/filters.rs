//! The filters' UI (canvas `MapCraft — Filters`): the Filters button in the top bar, its panel
//! (date histogram and range, sources, cameras), the chips over the map, and the card shown when
//! nothing matches. What a filter hides is decided in `crate::filter`.

use egui::{
    Align, Align2, Area, Button, Frame, Id, Layout, Margin, Order, Rect, RichText, Sense, Stroke,
    Ui, Vec2,
};

use crate::filter::{self, Facets, Filter, Month};
use crate::import::scope::Check;
use crate::library::Source;
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, thousands};

/// Cameras listed before "Show all" (design.md §2.3, unconfirmed).
const CAMERAS_SHOWN: usize = 6;
/// Years with their own button before "Earlier…".
const YEARS_SHOWN: usize = 4;
/// At most this many bars in the histogram; longer spans put several months in one bar.
const MAX_BARS: usize = 96;
const WIDTH: f32 = 380.0;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Knob {
    From,
    To,
}

pub struct FilterPanel {
    pub open: bool,
    button: Rect,
    all_cameras: bool,
    earlier: bool,
    /// The range while a knob is dragged; applied on release (a re-query per frame is too much
    /// for a big library).
    dragging: Option<(Knob, Month, Month)>,
    /// The pointer's offset from the dragged handle's centre when grabbed.
    grab: f32,
}

impl Default for FilterPanel {
    fn default() -> Self {
        Self {
            open: false,
            button: Rect::NOTHING,
            all_cameras: false,
            earlier: false,
            dragging: None,
            grab: 0.0,
        }
    }
}

/// What the chips and the no-match card ask for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FiltersAction {
    /// The filter changed.
    Changed,
    /// Open the panel ("Change filters…").
    OpenPanel,
}

impl FilterPanel {
    /// The Filters button, with a badge counting the filters set. Toggles the panel.
    pub fn button(&mut self, ui: &mut Ui, p: &Palette, active: usize) -> bool {
        let text = RichText::new("Filters").color(if active > 0 || self.open {
            p.text_strong
        } else {
            p.text_normal
        });
        let mut b = Button::image_and_text(Icon::Filter.image(p.size_icon_sm, p.text_normal), text)
            .selected(self.open);
        if active > 0 {
            b = b.stroke(Stroke::new(1.0, p.accent));
        }
        let r = ui.add(b);
        if active > 0 {
            // The badge, painted over the button's right end (no extra widget: ids stay put).
            let t = active.to_string();
            let galley =
                ui.painter()
                    .layout_no_wrap(t, egui::FontId::proportional(11.0), p.text_on_accent);
            let size = Vec2::new(galley.size().x.max(8.0) + 8.0, 16.0);
            let rect =
                Rect::from_center_size(r.rect.right_center() + egui::vec2(-2.0, -10.0), size);
            ui.painter().rect_filled(rect, 255, p.accent);
            ui.painter().galley(
                rect.center() - galley.size() / 2.0,
                galley,
                p.text_on_accent,
            );
        }
        let name = if active > 0 {
            format!("Filters ({active} set)")
        } else {
            "Filters".to_owned()
        };
        r.widget_info(|| egui::WidgetInfo::selected(egui::Role::Button, true, self.open, &name));
        self.button = r.rect;
        let clicked = r.clicked();
        if clicked {
            self.open = !self.open;
        }
        clicked
    }

    /// The panel under the button, while open. True when the filter changed.
    #[allow(clippy::too_many_arguments)]
    pub fn panel(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        f: &mut Filter,
        facets: &Facets,
        sources: &[Source],
        shown: usize,
        on_map: usize,
    ) -> bool {
        if !self.open {
            self.dragging = None;
            return false;
        }
        if ctx.input_mut(|i| i.consume_key(egui::Modifiers::NONE, egui::Key::Escape)) {
            self.open = false;
            return false;
        }
        let before = f.clone();
        let shown_area = Area::new(Id::unique("filter-panel"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(self.button.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.bg_window)
                    .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                    .corner_radius(p.radius_md)
                    .shadow(p.elevation_popup)
                    .show(ui, |ui| {
                        ui.set_width(WIDTH);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        section(ui, p, true, |ui| self.dates(ui, p, f, facets));
                        if sources.len() > 1 {
                            section(ui, p, false, |ui| sources_list(ui, p, f, facets, sources));
                        }
                        if !facets.cameras.is_empty() {
                            section(ui, p, false, |ui| self.cameras(ui, p, f, facets));
                        }
                        section(ui, p, false, |ui| {
                            ui.horizontal(|ui| {
                                ui.label(
                                    RichText::new(format!(
                                        "{} of {} photos on the map",
                                        thousands(shown),
                                        thousands(on_map)
                                    ))
                                    .small()
                                    .color(p.text_weak),
                                );
                                ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                    if widgets::secondary(ui, p, None, "Clear all", f.is_active())
                                        .clicked()
                                    {
                                        *f = Filter::default();
                                    }
                                });
                            });
                        });
                    });
            });
        // A press anywhere else closes it (the button toggles it itself).
        let outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|pos| {
                    !shown_area.response.rect.contains(pos) && !self.button.contains(pos)
                })
        });
        if outside && self.dragging.is_none() {
            self.open = false;
        }
        *f != before
    }

    fn dates(&mut self, ui: &mut Ui, p: &Palette, f: &mut Filter, facets: &Facets) {
        let Some((first, last)) = facets.span() else {
            caption(ui, p, "Date");
            ui.label(RichText::new("None of the photos on the map has a date.").color(p.text_weak));
            return;
        };
        let n = filter::month_index(first, last) + 1;
        let (from, to) = match self.dragging {
            Some((_, a, b)) => (a, b),
            None => f.dates.unwrap_or((first, last)),
        };
        ui.horizontal(|ui| {
            caption(ui, p, "Date");
            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                let t = if self.dragging.is_none() && f.dates.is_none() {
                    "Any time".to_owned()
                } else {
                    filter::range_text(from, to)
                };
                ui.label(RichText::new(t).color(p.text_strong));
            });
        });
        ui.add_space(p.space_2);
        let (a, b) = (
            filter::month_index(first, from).clamp(0, n - 1),
            filter::month_index(first, to).clamp(0, n - 1),
        );
        histogram(ui, p, facets, first, n, a, b);
        ui.add_space(p.space_2);
        if let Some((from, to)) = self.slider(ui, p, first, n, a, b) {
            f.dates = ((from, to) != (first, last)).then_some((from, to));
        }
        years_axis(ui, p, first, n);
        ui.add_space(p.space_2);
        // Quick years: Any time, the newest few, Earlier… (all of them).
        let years = facets.years();
        ui.horizontal_wrapped(|ui| {
            ui.spacing_mut().item_spacing = egui::vec2(4.0, 4.0);
            if small(ui, p, "Any time", f.dates.is_none()).clicked() {
                f.dates = None;
            }
            let count = if self.earlier {
                years.len()
            } else {
                YEARS_SHOWN
            };
            for &y in years.iter().take(count) {
                let range = (y * 100 + 1, y * 100 + 12);
                if small(ui, p, &y.to_string(), f.dates == Some(range)).clicked() {
                    f.dates = Some(range);
                }
            }
            if years.len() > YEARS_SHOWN
                && !self.earlier
                && small(ui, p, "Earlier…", false).clicked()
            {
                self.earlier = true;
            }
        });
        if f.dates.is_some() && facets.undated > 0 {
            ui.add_space(p.space_1);
            ui.label(
                RichText::new(format!(
                    "{} photos without a date are hidden",
                    thousands(facets.undated)
                ))
                .small()
                .color(p.text_weak),
            );
        }
    }

    /// The two-handle range over `n` months from `first`, showing months `a..=b`. Returns the new
    /// range when a drag ends.
    fn slider(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        first: Month,
        n: i64,
        a: i64,
        b: i64,
    ) -> Option<(Month, Month)> {
        let (rect, _) =
            ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), Sense::hover());
        let rail = Rect::from_center_size(rect.center(), egui::vec2(rect.width() - 16.0, 4.0));
        let x_of = |i: i64| rail.left() + rail.width() * i as f32 / n as f32;
        let (xa, xb) = (x_of(a), x_of(b + 1));
        let painter = ui.painter();
        painter.rect_filled(rail, 2, p.border_subtle);
        painter.rect_filled(Rect::from_x_y_ranges(xa..=xb, rail.y_range()), 2, p.accent);
        let mut out = None;
        // Close handles split the space between them, so both can still be grabbed.
        let mid = (xa + xb) / 2.0;
        for (knob, x) in [(Knob::From, xa), (Knob::To, xb)] {
            let r = Rect::from_center_size(egui::pos2(x, rect.center().y), Vec2::splat(16.0));
            let hit = match knob {
                Knob::From => Rect::from_x_y_ranges(r.left()..=r.right().min(mid), r.y_range()),
                Knob::To => Rect::from_x_y_ranges(r.left().max(mid)..=r.right(), r.y_range()),
            };
            let id = ui.scope_id().with(("knob", knob == Knob::From));
            let resp = ui.interact(hit, id, Sense::drag());
            let (label, month) = match knob {
                Knob::From => ("Date from", filter::month_at(first, a)),
                Knob::To => ("Date to", filter::month_at(first, b)),
            };
            resp.widget_info(|| egui::WidgetInfo::slider(true, month as f64, label));
            let hot = resp.hovered() || resp.dragged();
            ui.painter().circle(
                r.center(),
                7.0,
                p.bg_window,
                Stroke::new(if hot { 2.0 } else { 1.5 }, p.accent),
            );
            if resp.drag_started()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                // Where on the handle it was grabbed: a press without moving changes nothing.
                self.grab = pos.x - x;
            }
            if resp.dragged()
                && let Some(pos) = resp.interact_pointer_pos()
            {
                let t = (pos.x - self.grab - rail.left()) / rail.width() * n as f32;
                let (na, nb) = filter::drag_range(knob == Knob::From, t, a, b, n);
                self.dragging = Some((
                    knob,
                    filter::month_at(first, na),
                    filter::month_at(first, nb),
                ));
            }
            if resp.drag_stopped()
                && let Some((_, from, to)) = self.dragging.take()
            {
                out = Some((from, to));
            }
        }
        out
    }

    fn cameras(&mut self, ui: &mut Ui, p: &Palette, f: &mut Filter, facets: &Facets) {
        caption(ui, p, "Cameras");
        let count = if self.all_cameras {
            facets.cameras.len()
        } else {
            CAMERAS_SHOWN
        };
        let rows = |ui: &mut Ui, f: &mut Filter| {
            for c in facets.cameras.iter().take(count) {
                let on = f.camera_shown(c);
                if check_row(ui, p, &c.label, c.photos, on) {
                    f.set_camera(c, !on);
                }
            }
        };
        if self.all_cameras {
            egui::ScrollArea::vertical()
                .max_height(240.0)
                .show(ui, |ui| rows(ui, f));
        } else {
            rows(ui, f);
        }
        if facets.cameras.len() > CAMERAS_SHOWN && !self.all_cameras {
            ui.add_space(2.0);
            let t = format!("Show all {} cameras", facets.cameras.len());
            if ui
                .add(
                    Button::new(RichText::new(t).small().color(p.text_normal))
                        .frame_when_inactive(false),
                )
                .clicked()
            {
                self.all_cameras = true;
            }
        }
    }
}

fn sources_list(ui: &mut Ui, p: &Palette, f: &mut Filter, facets: &Facets, sources: &[Source]) {
    caption(ui, p, "Sources");
    for s in sources {
        let on = !f.hidden_sources.contains(&s.id);
        let n = facets.sources.get(&s.id).copied().unwrap_or(0);
        if check_row(ui, p, &s.label, n, on) {
            if on {
                f.hidden_sources.insert(s.id);
            } else {
                f.hidden_sources.remove(&s.id);
            }
        }
    }
}

/// A section of the panel, with a hairline above all but the first.
fn section(ui: &mut Ui, p: &Palette, first: bool, body: impl FnOnce(&mut Ui)) {
    if !first {
        let (r, _) = ui.allocate_exact_size(egui::vec2(WIDTH, p.stroke_hairline), Sense::hover());
        ui.painter().rect_filled(r, 0.0, p.border_subtle);
    }
    let top = if first { p.space_3 } else { p.space_2 };
    Frame::NONE
        .inner_margin(Margin {
            left: p.space_3 as i8,
            right: p.space_3 as i8,
            top: top as i8,
            bottom: p.space_2 as i8,
        })
        .show(ui, |ui| {
            ui.set_width(WIDTH - 2.0 * p.space_3);
            body(ui);
        });
}

fn caption(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).small().color(p.text_weak));
}

/// A small button, highlighted when `on`.
fn small(ui: &mut Ui, p: &Palette, text: &str, on: bool) -> egui::Response {
    let ink = if on { p.text_strong } else { p.text_normal };
    ui.add(Button::new(RichText::new(text).small().color(ink)).selected(on))
}

/// A checklist row: box, label, count. True when clicked (box or label).
fn check_row(ui: &mut Ui, p: &Palette, label: &str, photos: usize, on: bool) -> bool {
    let mut clicked = false;
    ui.horizontal(|ui| {
        ui.set_min_height(22.0);
        ui.spacing_mut().item_spacing.x = p.space_2;
        let state = if on { Check::On } else { Check::Off };
        clicked |= widgets::tri_check(ui, p, state, true, label).clicked();
        clicked |= ui
            .add(egui::Label::new(RichText::new(label).color(p.text_normal)).sense(Sense::click()))
            .clicked();
        ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
            ui.label(RichText::new(thousands(photos)).small().color(p.text_weak));
        });
    });
    clicked
}

/// Photos per month (or per few months on long spans) over `n` months from `first`; bars in
/// `a..=b` are lit.
fn histogram(ui: &mut Ui, p: &Palette, facets: &Facets, first: Month, n: i64, a: i64, b: i64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 56.0), Sense::hover());
    let per = (n as usize).div_ceil(MAX_BARS).max(1) as i64;
    let bars = (n + per - 1) / per;
    let mut counts = vec![0usize; bars as usize];
    for &(m, c) in &facets.months {
        let i = filter::month_index(first, m);
        if (0..n).contains(&i) {
            counts[(i / per) as usize] += c;
        }
    }
    let max = counts.iter().copied().max().unwrap_or(0).max(1) as f32;
    let rail = rect.shrink2(egui::vec2(8.0, 0.0));
    let w = rail.width() / bars as f32;
    for (i, &c) in counts.iter().enumerate() {
        if c == 0 {
            continue;
        }
        let h = (rect.height() * c as f32 / max).max(2.0);
        let x = rail.left() + w * i as f32;
        let lit = (i as i64 * per..(i as i64 + 1) * per).any(|m| (a..=b).contains(&m));
        let bar = Rect::from_min_max(
            egui::pos2(x + 0.5, rect.bottom() - h),
            egui::pos2(x + w - 0.5, rect.bottom()),
        );
        ui.painter()
            .rect_filled(bar, 1, if lit { p.accent } else { p.border_normal });
    }
}

/// Year labels under the slider, at each January, as many as fit.
fn years_axis(ui: &mut Ui, p: &Palette, first: Month, n: i64) {
    let (rect, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 16.0), Sense::hover());
    let rail = rect.shrink2(egui::vec2(8.0, 0.0));
    let font = egui::FontId::proportional(11.0);
    let (y0, y1) = (first / 100, filter::month_at(first, n - 1) / 100);
    // Each year at its January when that's in the span (a span within one year: that year),
    // at least 36 px apart.
    let mut last_x = f32::NEG_INFINITY;
    for y in y0..=y1 {
        let i = filter::month_index(first, y * 100 + 1);
        if i < 0 && y0 != y1 {
            continue;
        }
        let x = rail.left() + rail.width() * i.max(0) as f32 / n as f32;
        let x = x.clamp(rail.left() + 12.0, rail.right() - 12.0);
        if x - last_x < 36.0 {
            continue;
        }
        ui.painter().text(
            egui::pos2(x, rect.center().y),
            Align2::CENTER_CENTER,
            y.to_string(),
            font.clone(),
            p.text_weak,
        );
        last_x = x;
    }
}

/// The active filters as chips over the map's top left, each removable, plus Clear all.
pub fn chips(
    ctx: &egui::Context,
    p: &Palette,
    map: Rect,
    f: &mut Filter,
    facets: &Facets,
    sources: &[Source],
) -> Option<FiltersAction> {
    if !f.is_active() {
        return None;
    }
    let before = f.clone();
    let mut texts: Vec<(&str, String)> = Vec::new();
    if let Some((a, b)) = f.dates {
        texts.push(("date", filter::range_text(a, b)));
    }
    if !f.hidden_sources.is_empty() {
        let (shown, hidden): (Vec<&Source>, Vec<&Source>) = sources
            .iter()
            .partition(|s| !f.hidden_sources.contains(&s.id));
        let shown: Vec<&str> = shown.iter().map(|s| s.label.as_str()).collect();
        let hidden: Vec<&str> = hidden.iter().map(|s| s.label.as_str()).collect();
        texts.push(("source", filter::list_text("sources", &shown, &hidden)));
    }
    if !f.hidden_cameras.is_empty() {
        let (shown, hidden): (Vec<_>, Vec<_>) =
            facets.cameras.iter().partition(|c| f.camera_shown(c));
        let shown: Vec<&str> = shown
            .iter()
            .map(|c: &&filter::Camera| c.label.as_str())
            .collect();
        let hidden: Vec<&str> = hidden
            .iter()
            .map(|c: &&filter::Camera| c.label.as_str())
            .collect();
        texts.push(("camera", filter::list_text("cameras", &shown, &hidden)));
    }
    Area::new(Id::unique("filter-chips"))
        .order(Order::Middle)
        .pivot(Align2::LEFT_TOP)
        .fixed_pos(map.left_top() + egui::vec2(p.space_3, p.space_3))
        .show(ctx, |ui| {
            ui.set_max_width((map.width() - 160.0).max(200.0));
            ui.horizontal_wrapped(|ui| {
                ui.spacing_mut().item_spacing = egui::vec2(6.0, 6.0);
                for (kind, text) in &texts {
                    let remove = Frame::NONE
                        .fill(p.bg_window)
                        .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                        .corner_radius(255)
                        .shadow(p.elevation_panel)
                        .inner_margin(Margin {
                            left: 10,
                            right: 2,
                            top: 0,
                            bottom: 0,
                        })
                        .show(ui, |ui| {
                            ui.spacing_mut().item_spacing.x = 4.0;
                            ui.set_height(24.0);
                            ui.label(RichText::new(text).small().color(p.text_strong));
                            widgets::ghost(
                                ui,
                                p,
                                Icon::X,
                                &format!("Remove the {kind} filter"),
                                true,
                            )
                            .clicked()
                        })
                        .inner;
                    if remove {
                        match *kind {
                            "date" => f.dates = None,
                            "source" => f.hidden_sources.clear(),
                            _ => f.hidden_cameras.clear(),
                        }
                    }
                }
                let clear = Frame::NONE
                    .fill(p.bg_window)
                    .corner_radius(p.radius_sm)
                    .shadow(p.elevation_panel)
                    .show(ui, |ui| {
                        widgets::secondary(ui, p, None, "Clear all", true).clicked()
                    })
                    .inner;
                if clear {
                    *f = Filter::default();
                }
            });
        });
    (*f != before).then_some(FiltersAction::Changed)
}

/// The card shown when the filters hide every photo on the map.
pub fn no_match(
    ctx: &egui::Context,
    p: &Palette,
    map: Rect,
    f: &mut Filter,
    on_map: usize,
) -> Option<FiltersAction> {
    let mut action = None;
    Area::new(Id::unique("filter-no-match"))
        .order(Order::Middle)
        .pivot(Align2::CENTER_CENTER)
        .fixed_pos(egui::pos2(map.center().x, map.top() + map.height() * 0.42))
        .show(ctx, |ui| {
            Frame::NONE
                .fill(p.bg_window)
                .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                .corner_radius(p.radius_lg)
                .shadow(p.elevation_popup)
                .inner_margin(Margin::same(p.space_4 as i8))
                .show(ui, |ui| {
                    ui.set_width(360.0 - 2.0 * p.space_4);
                    ui.spacing_mut().item_spacing.y = p.space_2;
                    ui.label(
                        RichText::new("No photo matches the filters")
                            .size(15.0)
                            .strong()
                            .color(p.text_strong),
                    );
                    ui.label(
                        RichText::new(format!(
                            "None of your {} photos on the map match all of them.",
                            thousands(on_map)
                        ))
                        .color(p.text_weak),
                    );
                    ui.horizontal(|ui| {
                        if widgets::primary(ui, p, None, "Clear all filters", true).clicked() {
                            *f = Filter::default();
                            action = Some(FiltersAction::Changed);
                        }
                        if widgets::secondary(ui, p, None, "Change filters…", true).clicked() {
                            action = Some(FiltersAction::OpenPanel);
                        }
                    });
                });
        });
    action
}

//! The search field and its results (design.md §2.5; canvas `MapCraft — Main window`, artboard
//! "Search").
//!
//! The dropdown stays open by its own flag, not by the field's focus: a click on a row takes the
//! focus away before the row sees the click. ↑, ↓, Enter and Esc are taken before the text field
//! sees them. Enter only acts on results for the text as it is now.

use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver};

use egui::{
    Align2, Area, Frame, Key, Margin, Modifiers, Order, Rect, RichText, Sense, Stroke, Ui,
    text::{LayoutJob, TextFormat},
};

use crate::import::Bounds;
use crate::places;
use crate::search::{PhotoHit, Results, Searcher};
use crate::theme::{Palette, SEMIBOLD};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, thousands};

/// What a chosen result asks of the app.
#[derive(Debug, Clone, PartialEq)]
pub enum SearchAction {
    /// Centre the map on a place, at a zoom for its size.
    GoTo { lat: f64, lon: f64, zoom: f64 },
    /// Fit the map to these photos (a folder, a period, a camera).
    ZoomTo(Bounds),
    /// Centre on a photo and select it.
    Photo { id: i64, lat: f64, lon: f64 },
}

/// The zoom for a place of `population` people: a city fills the map, a village shows streets.
pub fn place_zoom(population: i64) -> f64 {
    match population {
        1_000_000.. => 11.0,
        100_000.. => 12.0,
        10_000.. => 13.0,
        _ => 14.0,
    }
}

/// The place-name download, while it runs or after it failed.
enum Download {
    Running {
        progress: Receiver<(u64, u64)>,
        done: Receiver<Result<usize, String>>,
        at: (u64, u64),
    },
    Failed(String),
}

#[derive(Clone)]
enum Do {
    Act(SearchAction),
    Download,
    Nothing,
}

/// One row of the dropdown, owned (built from the results each frame).
#[derive(Clone)]
struct Row {
    section: &'static str,
    icon: Icon,
    label: String,
    mono: bool,
    sub: String,
    tooltip: Option<String>,
    act: Do,
}

pub struct SearchBox {
    text: String,
    open: bool,
    results: Option<Results>,
    cursor: usize,
    enter: bool,
    searcher: Searcher,
    field: Rect,
    data: PathBuf,
    /// Downloading may run (off in tests: no network).
    network: bool,
    download: Option<Download>,
    /// Download requests, when `network` is off (for the tests).
    pub download_asked: usize,
    ctx: egui::Context,
}

impl SearchBox {
    pub fn new(db: PathBuf, data: &Path, network: bool, ctx: egui::Context) -> Self {
        Self {
            text: String::new(),
            open: false,
            results: None,
            cursor: 0,
            enter: false,
            searcher: Searcher::new(db, places::index_path(data), ctx.clone()),
            field: Rect::NOTHING,
            data: data.to_path_buf(),
            network,
            download: None,
            download_asked: 0,
            ctx,
        }
    }

    pub fn is_open(&self) -> bool {
        self.open
    }

    /// The results for the text as typed now (older answers are ignored).
    fn current(&self) -> Option<&Results> {
        self.results.as_ref().filter(|r| r.text == self.text)
    }

    /// The field, in the top bar.
    pub fn field(&mut self, ui: &mut Ui, p: &Palette) {
        if let Some(r) = self.searcher.latest() {
            self.results = Some(r);
        }
        self.poll_download();
        let id = egui::Id::unique("search-field");
        // Keys for the dropdown, taken before the text field sees them.
        let focused = ui.memory(|m| m.has_focus(id));
        let (mut up, mut down, mut esc) = (false, false, false);
        if focused && self.open {
            ui.input_mut(|i| {
                up = i.consume_key(Modifiers::NONE, Key::ArrowUp);
                down = i.consume_key(Modifiers::NONE, Key::ArrowDown);
                self.enter |= i.consume_key(Modifiers::NONE, Key::Enter);
                esc = i.consume_key(Modifiers::NONE, Key::Escape);
            });
        }
        if ui.input_mut(|i| i.consume_key(Modifiers::COMMAND, Key::K)) {
            ui.memory_mut(|m| m.request_focus(id));
        }

        let stroke = if focused { p.accent } else { p.border_normal };
        let shown = Frame::NONE
            .fill(p.bg_extreme)
            .stroke(Stroke::new(1.0, stroke))
            .corner_radius(p.radius_sm)
            .inner_margin(Margin::symmetric(p.space_2 as i8, 0))
            .show(ui, |ui| {
                ui.set_width(320.0 - 2.0 * p.space_2);
                ui.set_height(p.interact_height - 2.0);
                ui.horizontal_centered(|ui| {
                    ui.spacing_mut().item_spacing.x = p.space_1;
                    ui.add(Icon::Search.image(p.size_icon_sm, p.text_weak));
                    let clear = !self.text.is_empty();
                    let edit = egui::TextEdit::singleline(&mut self.text)
                        .id(id)
                        .frame(Frame::NONE)
                        .hint_text(
                            RichText::new("Search places, files, cameras").color(p.text_weak),
                        )
                        .desired_width(ui.available_width() - if clear { 24.0 } else { 0.0 });
                    let r = ui.add(edit);
                    // Its accessible name (screen readers, the UI tests): the hint isn't one.
                    ui.ctx()
                        .accesskit_node_builder(r.id, |n| n.set_label("Search"));
                    if r.changed() {
                        self.open = self.text.trim().chars().count() >= 2;
                        self.cursor = 0;
                        self.searcher.ask(&self.text);
                    }
                    if r.gained_focus() && self.text.trim().chars().count() >= 2 {
                        self.open = true;
                    }
                    if clear && widgets::ghost(ui, p, Icon::X, "Clear search (Esc)", true).clicked()
                    {
                        self.text.clear();
                        self.open = false;
                        ui.memory_mut(|m| m.request_focus(id));
                    }
                });
            });
        self.field = shown.response.rect;
        if esc {
            self.open = false;
        }
        let rows = self
            .rows()
            .iter()
            .filter(|r| !matches!(r.act, Do::Nothing))
            .count();
        if rows > 0 {
            if down {
                self.cursor = (self.cursor + 1) % rows;
            }
            if up {
                self.cursor = (self.cursor + rows - 1) % rows;
            }
        }
    }

    /// The rows for the current results.
    fn rows(&self) -> Vec<Row> {
        let mut rows = Vec::new();
        let Some(r) = self.current() else {
            return rows;
        };
        // Places: the results, or the download row while there's no index.
        if r.no_places {
            let (label, sub, act) = match &self.download {
                Some(Download::Running {
                    at: (done, total), ..
                }) => (
                    "Downloading place names…".to_owned(),
                    if *total > 0 {
                        format!("{} %", done * 100 / total)
                    } else {
                        format!("{} MB", done >> 20)
                    },
                    Do::Nothing,
                ),
                Some(Download::Failed(_)) => (
                    "Couldn't download place names: try again".to_owned(),
                    String::new(),
                    Do::Download,
                ),
                None => (
                    "Download place names (14 MB, from geonames.org)".to_owned(),
                    String::new(),
                    Do::Download,
                ),
            };
            let tooltip = match &self.download {
                Some(Download::Failed(e)) => Some(e.clone()),
                _ => Some(
                    "Place search works offline from the GeoNames list of cities and towns. \
                     It's downloaded once; nothing about you or your photos is sent."
                        .to_owned(),
                ),
            };
            rows.push(Row {
                section: "Places",
                icon: Icon::Download,
                label,
                mono: false,
                sub,
                tooltip,
                act,
            });
        }
        for h in &r.places {
            rows.push(Row {
                section: "Places",
                icon: Icon::MapPin,
                label: h.place.label(h.with_region),
                mono: false,
                sub: thousands(h.photos),
                tooltip: None,
                act: Do::Act(SearchAction::GoTo {
                    lat: h.place.lat,
                    lon: h.place.lon,
                    zoom: place_zoom(h.place.population),
                }),
            });
        }
        for h in &r.photos {
            let zoom = |s: &crate::library::FoundSet| {
                s.bounds
                    .map_or(Do::Nothing, |b| Do::Act(SearchAction::ZoomTo(b)))
            };
            rows.push(match h {
                PhotoHit::Folder(s) => {
                    let last = s.label.rsplit(['\\', '/']).next().unwrap_or(&s.label);
                    Row {
                        section: "Photos",
                        icon: Icon::Folder,
                        label: format!("…\\{last}"),
                        mono: true,
                        sub: thousands(s.photos),
                        tooltip: Some(s.label.replace('/', "\\")),
                        act: zoom(s),
                    }
                }
                PhotoHit::Period(s) => Row {
                    section: "Photos",
                    icon: Icon::Calendar,
                    label: format!("Photos from {}", period_text(&s.label)),
                    mono: false,
                    sub: thousands(s.photos),
                    tooltip: None,
                    act: zoom(s),
                },
                PhotoHit::Camera(s) => Row {
                    section: "Photos",
                    icon: Icon::Camera,
                    label: s.label.clone(),
                    mono: false,
                    sub: thousands(s.photos),
                    tooltip: None,
                    act: zoom(s),
                },
                PhotoHit::File(f) => Row {
                    section: "Photos",
                    icon: Icon::Image,
                    label: f.name.clone(),
                    mono: true,
                    sub: f
                        .taken_at
                        .as_deref()
                        .and_then(|t| crate::ui::details::date_text(t, false))
                        .and_then(|t| t.split(',').next().map(str::to_owned))
                        .unwrap_or_default(),
                    tooltip: None,
                    act: Do::Act(SearchAction::Photo {
                        id: f.id,
                        lat: f.lat,
                        lon: f.lon,
                    }),
                },
            });
        }
        rows
    }

    /// The dropdown under the field, while open. Returns the chosen result's action.
    pub fn dropdown(&mut self, ctx: &egui::Context, p: &Palette) -> Option<SearchAction> {
        let enter = std::mem::take(&mut self.enter);
        if !self.open {
            return None;
        }
        let rows = self.rows();
        let waiting = self.current().is_none();
        let mut chosen: Option<Do> = None;
        let mut selectable = 0usize;
        let width = 360.0;
        let shown = Area::new(egui::Id::unique("search-dropdown"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_TOP)
            .fixed_pos(self.field.left_bottom() + egui::vec2(0.0, 4.0))
            .show(ctx, |ui| {
                Frame::NONE
                    .fill(p.bg_window)
                    .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                    .corner_radius(p.radius_md)
                    .shadow(p.elevation_popup)
                    .inner_margin(Margin::same(4))
                    .show(ui, |ui| {
                        ui.set_width(width - 8.0);
                        ui.spacing_mut().item_spacing.y = 0.0;
                        let mut section = "";
                        if rows.is_empty() {
                            let t = if waiting {
                                "Searching…"
                            } else {
                                "Nothing found"
                            };
                            ui.add_space(6.0);
                            ui.label(RichText::new(t).small().color(p.text_weak));
                            ui.add_space(6.0);
                        }
                        for row in &rows {
                            if row.section != section {
                                section = row.section;
                                header(ui, p, section);
                            }
                            let index = if matches!(row.act, Do::Nothing) {
                                None
                            } else {
                                selectable += 1;
                                Some(selectable - 1)
                            };
                            let at_cursor = index == Some(self.cursor);
                            if draw_row(ui, p, row, &self.text, at_cursor) || (enter && at_cursor) {
                                chosen = Some(row.act.clone());
                            }
                        }
                        footer(ui, p, rows.iter().any(|r| r.section == "Places"));
                    });
            });
        // A press anywhere else closes it.
        let outside = ctx.input(|i| {
            i.pointer.any_pressed()
                && i.pointer.interact_pos().is_some_and(|pos| {
                    !shown.response.rect.contains(pos) && !self.field.contains(pos)
                })
        });
        if outside {
            self.open = false;
        }
        match chosen? {
            Do::Act(a) => {
                self.open = false;
                ctx.memory_mut(|m| m.surrender_focus(egui::Id::unique("search-field")));
                Some(a)
            }
            Do::Download => {
                self.start_download();
                None
            }
            Do::Nothing => None,
        }
    }

    #[cfg(test)]
    pub fn reopen_for_tests(&mut self) {
        self.searcher.reopen_places();
    }

    /// Sets the text as if typed, the field focused and the dropdown open (tests, and the
    /// dev flag `--open search=<text>`).
    pub fn type_in(&mut self, text: &str) {
        self.text = text.to_owned();
        self.open = true;
        self.cursor = 0;
        self.searcher.ask(text);
        self.ctx
            .memory_mut(|m| m.request_focus(egui::Id::unique("search-field")));
    }

    /// Closes the dropdown (Esc from the app, a click on the map).
    pub fn close(&mut self) {
        self.open = false;
    }

    fn start_download(&mut self) {
        if !self.network {
            self.download_asked += 1;
            return;
        }
        if matches!(self.download, Some(Download::Running { .. })) {
            return;
        }
        let (ptx, progress) = mpsc::channel();
        let (dtx, done) = mpsc::channel();
        let (data, ctx) = (self.data.clone(), self.ctx.clone());
        std::thread::spawn(move || {
            let report = |d: u64, t: u64| {
                let _ = ptx.send((d, t));
                ctx.request_repaint();
            };
            let r = places::download(&data, &report);
            let _ = dtx.send(r);
            ctx.request_repaint();
        });
        tracing::info!("downloading place names from geonames.org");
        self.download = Some(Download::Running {
            progress,
            done,
            at: (0, 0),
        });
    }

    fn poll_download(&mut self) {
        let Some(Download::Running { progress, done, at }) = &mut self.download else {
            return;
        };
        if let Some(p) = progress.try_iter().last() {
            *at = p;
        }
        match done.try_recv() {
            Ok(Ok(n)) => {
                tracing::info!("place index built: {n} places");
                self.download = None;
                self.searcher.reopen_places();
                self.searcher.ask(&self.text);
            }
            Ok(Err(e)) => {
                tracing::warn!("place names download failed: {e}");
                self.download = Some(Download::Failed(e));
            }
            Err(_) => {}
        }
    }
}

/// `2019` → "2019", `2019-08` → "Aug 2019", `2019-08-14` → "14 Aug 2019".
pub fn period_text(period: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Vec<&str> = period.split('-').collect();
    let month = |m: &str| {
        m.parse::<usize>()
            .ok()
            .and_then(|m| MONTHS.get(m.wrapping_sub(1)))
    };
    match parts.as_slice() {
        [y, m] => month(m).map_or(period.to_owned(), |m| format!("{m} {y}")),
        [y, m, d] => month(m).map_or(period.to_owned(), |m| {
            format!("{} {m} {y}", d.trim_start_matches('0'))
        }),
        _ => period.to_owned(),
    }
}

fn header(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add_space(6.0);
    ui.horizontal(|ui| {
        ui.add_space(p.space_2);
        ui.label(RichText::new(text).small().color(p.text_weak));
    });
    ui.add_space(2.0);
}

/// One row; true when clicked. The typed text is drawn strong where it appears in the label.
fn draw_row(ui: &mut Ui, p: &Palette, row: &Row, typed: &str, at_cursor: bool) -> bool {
    let enabled = !matches!(row.act, Do::Nothing);
    let (rect, r) = ui.allocate_exact_size(
        egui::vec2(ui.available_width(), p.interact_height),
        if enabled {
            Sense::click()
        } else {
            Sense::hover()
        },
    );
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &row.label));
    let painter = ui.painter();
    if at_cursor {
        painter.rect_filled(rect, p.radius_sm, p.accent_subtle);
    } else if r.hovered() && enabled {
        painter.rect_filled(rect, p.radius_sm, p.widget_hovered_weak);
    }
    let lead = if at_cursor { p.accent } else { p.text_weak };
    let icon = Rect::from_center_size(
        egui::pos2(rect.left() + p.space_2 + 7.0, rect.center().y),
        egui::vec2(14.0, 14.0),
    );
    row.icon.image(14.0, lead).paint_at(ui, icon);
    let ink = if at_cursor {
        p.text_strong
    } else {
        p.text_normal
    };
    let font = if row.mono {
        egui::FontId::monospace(12.0)
    } else {
        egui::FontId::proportional(13.0)
    };
    // The sub text (count or date), right-aligned.
    let sub = painter.layout_no_wrap(
        row.sub.clone(),
        egui::FontId::proportional(11.0),
        p.text_weak,
    );
    let sub_x = rect.right() - p.space_2 - sub.size().x;
    painter.galley(
        egui::pos2(sub_x, rect.center().y - sub.size().y / 2.0),
        sub,
        p.text_weak,
    );
    // The label, with the typed text in it strong.
    let mut job = LayoutJob::default();
    let plain = TextFormat::simple(font.clone(), ink);
    let strong = TextFormat::simple(
        if row.mono {
            font.clone()
        } else {
            egui::FontId::new(13.0, egui::FontFamily::Name(SEMIBOLD.into()))
        },
        p.text_strong,
    );
    match find_folded(&row.label, typed.trim()) {
        Some((a, b)) => {
            job.append(&row.label[..a], 0.0, plain.clone());
            job.append(&row.label[a..b], 0.0, strong);
            job.append(&row.label[b..], 0.0, plain);
        }
        None => job.append(&row.label, 0.0, plain),
    }
    job.wrap.max_width = (sub_x - icon.right() - 2.0 * p.space_2).max(40.0);
    job.wrap.max_rows = 1;
    job.wrap.break_anywhere = true;
    let galley = ui.fonts_mut(|f| f.layout_job(job));
    let pos = egui::pos2(
        icon.right() + p.space_2,
        rect.center().y - galley.size().y / 2.0,
    );
    ui.painter().galley(pos, galley, ink);
    let r = match &row.tooltip {
        Some(t) => r.on_hover_text(t),
        None => r,
    };
    enabled && r.clicked()
}

/// Where `needle` appears in `hay`, ignoring case, as byte offsets into `hay`.
fn find_folded(hay: &str, needle: &str) -> Option<(usize, usize)> {
    if needle.is_empty() {
        return None;
    }
    let n: Vec<char> = needle.chars().flat_map(char::to_lowercase).collect();
    let chars: Vec<(usize, char)> = hay.char_indices().collect();
    (0..chars.len()).find_map(|i| {
        let mut j = i;
        let mut k = 0;
        while k < n.len() && j < chars.len() {
            let lower: Vec<char> = chars[j].1.to_lowercase().collect();
            if n[k..].starts_with(&lower) {
                k += lower.len();
                j += 1;
            } else {
                return None;
            }
        }
        (k == n.len()).then(|| {
            let end = chars.get(j).map_or(hay.len(), |c| c.0);
            (chars[i].0, end)
        })
    })
}

fn footer(ui: &mut Ui, p: &Palette, places: bool) {
    ui.add_space(4.0);
    let (line, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        line.x_range(),
        line.center().y,
        Stroke::new(p.stroke_hairline, p.border_subtle),
    );
    ui.add_space(4.0);
    ui.horizontal(|ui| {
        ui.add_space(p.space_2);
        ui.spacing_mut().item_spacing.x = p.space_3;
        for (keys, what) in [("↑↓", "move"), ("Enter", "go to"), ("Esc", "close")] {
            ui.horizontal(|ui| {
                ui.spacing_mut().item_spacing.x = 4.0;
                Frame::NONE
                    .fill(p.bg_faint)
                    .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                    .corner_radius(p.radius_sm)
                    .inner_margin(Margin::symmetric(4, 0))
                    .show(ui, |ui| {
                        ui.label(
                            RichText::new(keys)
                                .monospace()
                                .size(11.0)
                                .color(p.text_weak),
                        );
                    });
                ui.label(RichText::new(what).small().color(p.text_weak));
            });
        }
        if places {
            // GeoNames' licence (CC BY 4.0) asks for credit where its data shows.
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                ui.add_space(p.space_2);
                ui.hyperlink_to(
                    RichText::new("Places: GeoNames").small().color(p.text_weak),
                    "https://www.geonames.org/",
                )
                .on_hover_text("Place names © GeoNames, CC BY 4.0");
            });
        }
    });
    ui.add_space(2.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_are_found_ignoring_case_and_periods_read_well() {
        assert_eq!(find_folded("Hamburg, Germany", "hamb"), Some((0, 4)));
        assert_eq!(find_folded("…\\2023-05-19_Hamburg", "HAMB"), Some((15, 19)));
        assert_eq!(find_folded("Vík í Mýrdal", "ík"), Some((1, 4)));
        assert_eq!(find_folded("Vienna", "wien"), None);
        assert_eq!(period_text("2019"), "2019");
        assert_eq!(period_text("2019-08"), "Aug 2019");
        assert_eq!(period_text("2019-08-04"), "4 Aug 2019");
        assert_eq!(place_zoom(1_800_000), 11.0);
        assert_eq!(place_zoom(750), 14.0);
    }
}

//! The hover card over a marker and the detail panel of the selected photo (canvas
//! `MapCraft — Main window`, artboards "Main" and "Selected").
//!
//! Both are built from the library's record only: the UI thread never touches the photo's
//! folder (it may be on a slow or sleeping NAS). The panel's 1024 px preview is read, or made on
//! demand, by `Previews` on a background thread.

use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Receiver, Sender};

use egui::{
    Align2, Area, ColorImage, Frame, Margin, Order, Rect, RichText, Sense, Stroke, TextureHandle,
    TextureOptions, Ui,
};

use crate::library::PhotoDetail;
use crate::theme::{Palette, SEMIBOLD};
use crate::thumbs::{self, PREVIEW, PhotoFiles};
use crate::ui::icons::Icon;
use crate::ui::widgets;

// ---- Text ------------------------------------------------------------------------------------

const MONTHS: [&str; 12] = [
    "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
];

/// `2023-05-19T21:06:14` → `19 May 2023, 21:06:14` (or `21:06` without seconds). The camera's
/// local time, as recorded: no zone conversion.
pub fn date_text(taken_at: &str, seconds: bool) -> Option<String> {
    let (date, time) = taken_at.split_once('T')?;
    let mut d = date.split('-');
    let (y, m, day) = (d.next()?, d.next()?, d.next()?);
    let month = MONTHS.get(m.parse::<usize>().ok()?.checked_sub(1)?)?;
    let day: u32 = day.parse().ok()?;
    let time = if seconds {
        time
    } else {
        time.get(..5).unwrap_or(time)
    };
    Some(format!("{day} {month} {y}, {time}"))
}

/// The camera's name: the model alone when it already starts with the make (`Canon EOS R5`
/// from `Canon` + `Canon EOS R5`), else both.
pub fn camera_text(make: Option<&str>, model: Option<&str>) -> Option<String> {
    match (make.map(str::trim), model.map(str::trim)) {
        (Some(make), Some(model)) => {
            let first = make.split_whitespace().next().unwrap_or(make);
            if model.to_lowercase().starts_with(&first.to_lowercase()) {
                Some(model.to_owned())
            } else {
                Some(format!("{make} {model}"))
            }
        }
        (None, Some(m)) | (Some(m), None) => Some(m.to_owned()),
        (None, None) => None,
    }
}

/// `24.1 MB · 6240 × 4160` (MB as Explorer counts them, 1024 × 1024 bytes).
pub fn size_text(bytes: i64, width: Option<u32>, height: Option<u32>) -> String {
    let mb = bytes as f64 / (1024.0 * 1024.0);
    let size = if mb >= 1.0 {
        format!("{mb:.1} MB")
    } else {
        format!("{:.0} KB", (bytes as f64 / 1024.0).max(1.0))
    };
    match (width, height) {
        (Some(w), Some(h)) => format!("{size} · {w} × {h}"),
        _ => size,
    }
}

/// `53.5436° N, 9.9893° E`: four decimals, about 10 m.
pub fn position_text(lat: f64, lon: f64) -> String {
    let ns = if lat < 0.0 { 'S' } else { 'N' };
    let ew = if lon < 0.0 { 'W' } else { 'E' };
    format!("{:.4}° {ns}, {:.4}° {ew}", lat.abs(), lon.abs())
}

/// Where the position came from.
pub fn gps_source_text(source: &str) -> &'static str {
    match source {
        "xmp" => "XMP sidecar",
        "exif" => "The photo's EXIF",
        _ => "Unknown",
    }
}

/// The photo's folder split for middle truncation: everything but the last folder (which may be
/// cut short), and the last folder (always shown).
pub fn folder_parts(root: &Path, rel_dir: &str) -> (String, String) {
    let root = root
        .to_string_lossy()
        .trim_end_matches(['\\', '/'])
        .to_owned();
    if rel_dir.is_empty() {
        return (root, String::new());
    }
    let rel = rel_dir.replace('/', "\\");
    match rel.rsplit_once('\\') {
        Some((head, last)) => (format!("{root}\\{head}"), format!("\\{last}")),
        None => (root, format!("\\{rel}")),
    }
}

/// The original's full path, for Show in Explorer and Open (with the source's root as given).
pub fn original_path(root: &Path, d: &PhotoDetail) -> PathBuf {
    let dir = if d.rel_dir.is_empty() {
        root.to_path_buf()
    } else {
        root.join(&d.rel_dir)
    };
    dir.join(&d.original_name)
}

// ---- Hover card ------------------------------------------------------------------------------

/// The hover card (232 px) beside a marker's frame: name, capture time, camera.
pub fn hover_card(ctx: &egui::Context, p: &Palette, d: &PhotoDetail, marker: Rect, map: Rect) {
    const WIDTH: f32 = 232.0;
    // To the marker's right, its bottom 12 px below the frame's top; to the left near the edge.
    let right = marker.right() + 12.0 + WIDTH < map.right() - 8.0;
    let (pivot, x) = if right {
        (Align2::LEFT_BOTTOM, marker.right() + 12.0)
    } else {
        (Align2::RIGHT_BOTTOM, marker.left() - 12.0)
    };
    Area::new(egui::Id::unique("hover-card"))
        .order(Order::Tooltip)
        .interactable(false)
        .pivot(pivot)
        .fixed_pos(egui::pos2(x, marker.top() + 12.0))
        .constrain_to(map)
        .show(ctx, |ui| {
            Frame::NONE
                .fill(p.bg_window)
                .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                .corner_radius(p.radius_md)
                .shadow(p.elevation_popup)
                .inner_margin(Margin::symmetric(p.space_3 as i8, p.space_2 as i8))
                .show(ui, |ui| {
                    ui.set_width(WIDTH - 2.0 * p.space_3);
                    ui.spacing_mut().item_spacing.y = 0.0;
                    ui.add(
                        egui::Label::new(RichText::new(&d.original_name).color(p.text_strong))
                            .truncate(),
                    );
                    let line = |ui: &mut Ui, icon: Icon, text: &str| {
                        ui.horizontal(|ui| {
                            ui.spacing_mut().item_spacing.x = 6.0;
                            ui.add(icon.image(12.0, p.text_weak));
                            ui.add(
                                egui::Label::new(RichText::new(text).small().color(p.text_weak))
                                    .truncate(),
                            );
                        });
                    };
                    if let Some(t) = d.taken_at.as_deref().and_then(|t| date_text(t, false)) {
                        line(ui, Icon::Calendar, &t);
                    }
                    let camera = camera_text(d.camera_make.as_deref(), d.camera_model.as_deref());
                    if let Some(c) = camera {
                        line(ui, Icon::Camera, &c);
                    }
                });
        });
}

// ---- Detail panel ----------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PanelAction {
    Close,
    ShowInExplorer,
    Open,
}

/// What the panel shows in its preview box.
pub enum PreviewFace<'a> {
    Image(&'a TextureHandle),
    /// Being read or made: the marker's thumbnail meanwhile, if it's loaded.
    Loading(Option<&'a TextureHandle>),
    /// No usable source (the marker's placeholder).
    None,
}

/// The panel's collapsed sections, kept while the selection changes.
#[derive(Default)]
pub struct DetailPanel {
    collapsed: HashSet<&'static str>,
}

impl DetailPanel {
    pub fn show(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        d: &PhotoDetail,
        preview: PreviewFace<'_>,
    ) -> Option<PanelAction> {
        let mut action = None;
        // Header: title and close.
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
                    RichText::new("Photo details")
                        .family(egui::FontFamily::Name(SEMIBOLD.into()))
                        .size(15.0)
                        .color(p.text_strong),
                );
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if widgets::ghost(ui, p, Icon::X, "Close panel (Ctrl+I)", true).clicked() {
                        action = Some(PanelAction::Close);
                    }
                });
            });
        });

        egui::ScrollArea::vertical()
            .auto_shrink([false, false])
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing.y = 0.0;
                self.preview(ui, p, preview);
                // Actions.
                Frame::NONE
                    .inner_margin(Margin {
                        left: p.space_3 as i8,
                        right: p.space_3 as i8,
                        top: 0,
                        bottom: p.space_3 as i8,
                    })
                    .show(ui, |ui| {
                        ui.horizontal_wrapped(|ui| {
                            ui.spacing_mut().item_spacing = egui::vec2(p.space_2, p.space_2);
                            let b = |ui: &mut Ui, icon, text: &str, enabled| {
                                widgets::secondary(ui, p, Some(icon), text, enabled)
                            };
                            if b(ui, Icon::FolderOpen, "Show in Explorer", true).clicked() {
                                action = Some(PanelAction::ShowInExplorer);
                            }
                            if b(ui, Icon::ExternalLink, "Open", true)
                                .on_hover_text("Open with the app Windows uses for this file")
                                .clicked()
                            {
                                action = Some(PanelAction::Open);
                            }
                            b(ui, Icon::Brush, "PhotoCraft", false)
                                .on_disabled_hover_text("Edit in PhotoCraft: not available yet");
                            b(ui, Icon::Sliders, "LightCraft", false)
                                .on_disabled_hover_text("Develop in LightCraft: not available yet");
                        });
                    });
                self.sections(ui, p, d);
            });
        action
    }

    /// The preview box: 3:2, the image fitted inside it.
    fn preview(&self, ui: &mut Ui, p: &Palette, face: PreviewFace<'_>) {
        let m = p.space_3;
        let width = ui.available_width() - 2.0 * m;
        let (outer, _) = ui.allocate_exact_size(
            egui::vec2(ui.available_width(), width * 2.0 / 3.0 + 2.0 * m),
            Sense::hover(),
        );
        let rect = outer.shrink(m);
        let painter = ui.painter();
        painter.rect_filled(rect, p.radius_sm, p.bg_extreme);
        let tex = match face {
            PreviewFace::Image(t) | PreviewFace::Loading(Some(t)) => Some(t),
            _ => None,
        };
        if let Some(t) = tex {
            let size = t.size_vec2();
            let scale = (rect.width() / size.x).min(rect.height() / size.y);
            let fitted = Rect::from_center_size(rect.center(), size * scale);
            egui::Image::new((t.id(), fitted.size()))
                .corner_radius(p.radius_sm)
                .paint_at(ui, fitted);
        } else {
            painter.rect_filled(rect, p.radius_sm, p.map_marker_placeholder);
            let icon = Rect::from_center_size(rect.center(), egui::vec2(32.0, 32.0));
            Icon::Image
                .image(32.0, p.map_marker_placeholder_fg)
                .paint_at(ui, icon);
        }
        if matches!(face, PreviewFace::Loading(_)) {
            let at = Rect::from_min_size(
                rect.right_bottom() - egui::vec2(22.0, 22.0),
                egui::vec2(14.0, 14.0),
            );
            ui.put(at, egui::Spinner::new().size(14.0).color(p.text_weak));
        }
    }

    fn sections(&mut self, ui: &mut Ui, p: &Palette, d: &PhotoDetail) {
        let mut file = Vec::new();
        file.push(("Name", Value::Strong(d.original_name.clone())));
        let (head, tail) = folder_parts(&d.root, &d.rel_dir);
        file.push(("Folder", Value::Folder(head, tail)));
        file.push((
            "Size",
            Value::Plain(size_text(d.file_size, d.width, d.height)),
        ));
        file.push(("Source", Value::Plain(d.source_label.clone())));

        let mut capture = Vec::new();
        if let Some(t) = d.taken_at.as_deref().and_then(|t| date_text(t, true)) {
            let t = match &d.taken_at_offset {
                Some(o) => format!("{t} ({o})"),
                None => t,
            };
            capture.push(("Date", Value::Plain(t)));
        }
        if let Some(c) = camera_text(d.camera_make.as_deref(), d.camera_model.as_deref()) {
            capture.push(("Camera", Value::Plain(c)));
        }
        if let Some(l) = &d.lens {
            capture.push(("Lens", Value::Plain(l.clone())));
        }

        let mut location = Vec::new();
        if let (Some(lat), Some(lon)) = (d.lat, d.lon) {
            location.push(("Position", Value::Mono(position_text(lat, lon))));
        }
        if let Some(a) = d.alt {
            location.push(("Altitude", Value::Plain(format!("{a:.0} m"))));
        }
        if let Some(s) = &d.gps_source {
            location.push(("From", Value::Plain(gps_source_text(s).to_owned())));
        }

        let all = [("File", file), ("Capture", capture), ("Location", location)];
        let n = all.len();
        for (i, (title, rows)) in all.into_iter().enumerate() {
            if rows.is_empty() {
                continue;
            }
            self.section(ui, p, title, &rows, i + 1 < n);
        }
    }

    fn section(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        title: &'static str,
        rows: &[(&str, Value)],
        rule: bool,
    ) {
        let open = !self.collapsed.contains(title);
        let r = Frame::NONE
            .inner_margin(Margin {
                left: p.space_3 as i8,
                right: p.space_3 as i8,
                top: p.space_2 as i8,
                bottom: if open {
                    p.space_3 as i8
                } else {
                    p.space_2 as i8
                },
            })
            .show(ui, |ui| {
                let (h, resp) = ui.allocate_exact_size(
                    egui::vec2(ui.available_width(), p.interact_height),
                    Sense::click(),
                );
                resp.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, true, title));
                let ink = if resp.hovered() {
                    p.text_strong
                } else {
                    p.text_weak
                };
                let chevron = if open {
                    Icon::ChevronDown
                } else {
                    Icon::ChevronRight
                };
                let icon = Rect::from_min_size(
                    egui::pos2(h.left(), h.center().y - 6.0),
                    egui::vec2(12.0, 12.0),
                );
                chevron.image(12.0, ink).paint_at(ui, icon);
                ui.painter().text(
                    egui::pos2(icon.right() + p.space_1, h.center().y),
                    Align2::LEFT_CENTER,
                    title,
                    egui::FontId::proportional(11.0),
                    ink,
                );
                if resp.clicked() {
                    if open {
                        self.collapsed.insert(title);
                    } else {
                        self.collapsed.remove(title);
                    }
                }
                if open {
                    let value_width = (ui.available_width() - 76.0 - p.space_3).max(48.0);
                    egui::Grid::new(("detail-section", title))
                        .num_columns(2)
                        .min_col_width(76.0)
                        .max_col_width(value_width)
                        .spacing(egui::vec2(p.space_3, 3.0))
                        .show(ui, |ui| {
                            for (key, value) in rows {
                                ui.label(RichText::new(*key).small().color(p.text_weak));
                                value.show(ui, p, value_width);
                                ui.end_row();
                            }
                        });
                }
            });
        if rule {
            let b = r.response.rect;
            ui.painter().hline(
                b.x_range(),
                b.bottom() - p.stroke_hairline / 2.0,
                Stroke::new(p.stroke_hairline, p.border_subtle),
            );
        }
    }
}

/// A value in the panel's key–value lists.
enum Value {
    Plain(String),
    Strong(String),
    Mono(String),
    /// A folder, cut short in the middle: the head may be truncated, the last folder never.
    Folder(String, String),
}

impl Value {
    fn show(&self, ui: &mut Ui, p: &Palette, width: f32) {
        let truncated = |ui: &mut Ui, text: RichText, w: f32| {
            ui.scope(|ui| {
                ui.set_max_width(w);
                ui.add(egui::Label::new(text).truncate())
            })
            .inner
        };
        match self {
            Value::Plain(t) => {
                truncated(ui, RichText::new(t).color(p.text_normal), width);
            }
            Value::Strong(t) => {
                truncated(ui, RichText::new(t).color(p.text_strong), width);
            }
            Value::Mono(t) => {
                truncated(ui, RichText::new(t).monospace().color(p.text_normal), width);
            }
            Value::Folder(head, tail) => {
                let font = egui::FontId::monospace(12.0);
                let tail_w = ui
                    .painter()
                    .layout_no_wrap(tail.clone(), font, p.text_normal)
                    .size()
                    .x;
                let r = ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = 0.0;
                    truncated(
                        ui,
                        RichText::new(head).monospace().color(p.text_normal),
                        (width - tail_w).max(24.0),
                    );
                    ui.label(RichText::new(tail).monospace().color(p.text_normal));
                });
                r.response.on_hover_text(format!("{head}{tail}"));
            }
        }
    }
}

// ---- Preview loader --------------------------------------------------------------------------

enum PreviewState {
    Loading,
    Ready(TextureHandle),
    Failed,
}

/// The selected photo's 1024 px preview: read from the data folder, or made from the photo's
/// files on a background thread (design.md §2.2(b)) and kept for next time.
pub struct Previews {
    requests: Sender<PhotoDetail>,
    results: Receiver<(i64, Option<ColorImage>)>,
    current: Option<(i64, PreviewState)>,
}

impl Previews {
    pub fn new(data: PathBuf, ctx: egui::Context) -> Self {
        let (requests, rx) = mpsc::channel::<PhotoDetail>();
        let (tx, results) = mpsc::channel();
        std::thread::Builder::new()
            .name("mapcraft-preview".into())
            .spawn(move || {
                while let Ok(mut d) = rx.recv() {
                    // Only the newest request matters: the user may click through markers.
                    while let Ok(newer) = rx.try_recv() {
                        d = newer;
                    }
                    let image = make_preview(&data, &d);
                    if tx.send((d.id, image)).is_err() {
                        break;
                    }
                    ctx.request_repaint();
                }
            })
            .expect("spawn the preview loader");
        Self {
            requests,
            results,
            current: None,
        }
    }

    /// The preview for `d`, requesting it when the selection changed.
    pub fn get(
        &mut self,
        ctx: &egui::Context,
        d: &PhotoDetail,
    ) -> Option<Result<&TextureHandle, ()>> {
        while let Ok((id, image)) = self.results.try_recv() {
            if let Some((cur, state)) = &mut self.current
                && *cur == id
            {
                *state = match image {
                    Some(img) => PreviewState::Ready(ctx.load_texture(
                        format!("preview-{id}"),
                        img,
                        TextureOptions::LINEAR,
                    )),
                    None => PreviewState::Failed,
                };
            }
        }
        if self.current.as_ref().is_none_or(|(id, _)| *id != d.id) {
            let state = if d.thumb_made {
                let _ = self.requests.send(d.clone());
                PreviewState::Loading
            } else {
                PreviewState::Failed // no usable source: the placeholder, without trying
            };
            self.current = Some((d.id, state));
        }
        match &self.current {
            Some((_, PreviewState::Ready(t))) => Some(Ok(t)),
            Some((_, PreviewState::Failed)) => Some(Err(())),
            _ => None,
        }
    }

    /// Forgets the preview (the photo changed): the next `get` reads or makes it again.
    pub fn forget(&mut self, id: i64) {
        if self.current.as_ref().is_some_and(|(cur, _)| *cur == id) {
            self.current = None;
        }
    }
}

/// Reads the kept preview, or makes it from the photo's files and keeps it.
fn make_preview(data: &Path, d: &PhotoDetail) -> Option<ColorImage> {
    let path = thumbs::path(data, d.id, PREVIEW);
    let jpeg = match std::fs::read(&path) {
        Ok(b) => b,
        Err(_) => {
            let root = crate::import::reachable_root(&d.root, d.unc.as_deref())?;
            let dir = if d.rel_dir.is_empty() {
                root
            } else {
                root.join(&d.rel_dir)
            };
            let files = PhotoFiles {
                dir: &dir,
                original: &d.original_name,
                preview: d.preview_name.as_deref(),
                orientation: d.orientation,
            };
            let (jpeg, _) = std::panic::catch_unwind(|| thumbs::render(&files, PREVIEW))
                .ok()?
                .ok()?;
            let _ = thumbs::store(&path, &jpeg);
            jpeg
        }
    };
    let img = image::load_from_memory_with_format(&jpeg, image::ImageFormat::Jpeg).ok()?;
    let rgba = img.to_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    Some(ColorImage::from_rgba_unmultiplied(size, rgba.as_raw()))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn dates_read_like_the_mockup() {
        assert_eq!(
            date_text("2023-05-19T21:06:14", true).as_deref(),
            Some("19 May 2023, 21:06:14")
        );
        assert_eq!(
            date_text("2023-08-03T19:42:05", false).as_deref(),
            Some("3 Aug 2023, 19:42")
        );
        assert_eq!(date_text("2023-13-01T00:00:00", true), None);
        assert_eq!(date_text("garbage", true), None);
    }

    #[test]
    fn camera_names_drop_a_repeated_make() {
        assert_eq!(
            camera_text(Some("Canon"), Some("Canon EOS 5D Mark IV")).as_deref(),
            Some("Canon EOS 5D Mark IV")
        );
        assert_eq!(
            camera_text(Some("FUJIFILM"), Some("X-T3")).as_deref(),
            Some("FUJIFILM X-T3")
        );
        assert_eq!(
            camera_text(Some("NIKON CORPORATION"), Some("NIKON D850")).as_deref(),
            Some("NIKON D850")
        );
        assert_eq!(camera_text(None, Some("X-T3")).as_deref(), Some("X-T3"));
        assert_eq!(camera_text(None, None), None);
    }

    #[test]
    fn sizes_positions_and_folders() {
        assert_eq!(
            size_text(25_270_000, Some(6240), Some(4160)),
            "24.1 MB · 6240 × 4160"
        );
        assert_eq!(size_text(2_000, None, None), "2 KB");
        assert_eq!(position_text(53.54361, 9.98932), "53.5436° N, 9.9893° E");
        assert_eq!(position_text(-33.8688, -70.6), "33.8688° S, 70.6000° W");
        let root = Path::new(r"Z:\Photos\");
        assert_eq!(
            folder_parts(root, r"2023-05-19_Hamburg\Speicherstadt"),
            (
                r"Z:\Photos\2023-05-19_Hamburg".to_owned(),
                r"\Speicherstadt".to_owned()
            )
        );
        assert_eq!(
            folder_parts(root, "Trip"),
            (r"Z:\Photos".to_owned(), r"\Trip".to_owned())
        );
        assert_eq!(
            folder_parts(root, ""),
            (r"Z:\Photos".to_owned(), String::new())
        );
    }
}

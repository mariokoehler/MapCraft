//! The panel of tagging mode (design.md §2.9, canvas `MapCraft — Geotagging`, artboard 1):
//! the track, the time shift, matching, the photos, and Write positions. It only reads the
//! session and says what was asked; the app changes the session.

use egui::{Frame, Margin, Rect, RichText, Sense, Stroke, Ui};

use crate::geotag::session::Session;
use crate::geotag::{MARGINS, time};
use crate::theme::{Palette, SEMIBOLD};
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Tone, thousands};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum GeoAction {
    Leave,
    AddFile,
    Zone(i64),
    Correction(i64),
    IgnoreOwn(bool),
    StartPick,
    CancelPick,
    UndoAnchor,
    MaxGap(u32),
    Margin(u32),
    Retag(bool),
    WriteAlt(bool),
    Write,
    CancelWrite,
    /// Undo the newest write (its journal), after its toast is gone.
    UndoLast,
}

/// How many "outside the track" photos are listed by name.
const OUTSIDE_ROWS: usize = 6;

pub(crate) fn section(ui: &mut Ui, p: &Palette, title: &str, body: impl FnOnce(&mut Ui)) {
    Frame::NONE
        .inner_margin(Margin {
            left: p.space_3 as i8,
            right: p.space_3 as i8,
            top: p.space_2 as i8,
            bottom: p.space_3 as i8,
        })
        .show(ui, |ui| {
            ui.set_width(ui.available_width());
            ui.spacing_mut().item_spacing.y = p.space_1;
            widgets::label(ui, p, title);
            body(ui);
        });
    let (r, _) = ui.allocate_exact_size(egui::vec2(ui.available_width(), 1.0), Sense::hover());
    ui.painter().hline(
        r.x_range(),
        r.center().y,
        Stroke::new(p.stroke_hairline, p.border_subtle),
    );
}

pub(crate) fn weak(ui: &mut Ui, p: &Palette, text: &str) {
    ui.add(egui::Label::new(RichText::new(text).small().color(p.text_weak)).wrap());
}

fn kv(ui: &mut Ui, p: &Palette, k: &str, v: &str) {
    ui.horizontal(|ui| {
        ui.add_sized(
            [76.0, 18.0],
            egui::Label::new(RichText::new(k).small().color(p.text_weak)),
        );
        ui.label(RichText::new(v).color(p.text_normal));
    });
}

/// `writing`: the write's progress while it runs.
pub fn show(
    ui: &mut Ui,
    p: &Palette,
    s: &Session,
    writing: Option<(usize, usize)>,
    last: Option<&str>,
) -> Option<GeoAction> {
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
                RichText::new("Tag from a GPS track")
                    .family(egui::FontFamily::Name(SEMIBOLD.into()))
                    .size(15.0)
                    .color(p.text_strong),
            );
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                if widgets::ghost(ui, p, Icon::X, "Leave tagging (Esc)", writing.is_none())
                    .clicked()
                {
                    action = Some(GeoAction::Leave);
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
                        action = Some(GeoAction::CancelWrite);
                    }
                    ui.add(egui::Spinner::new().size(14.0).color(p.text_weak));
                    ui.label(
                        RichText::new(format!(
                            "Writing {} of {}…",
                            thousands(done.min(total)),
                            thousands(total)
                        ))
                        .color(p.text_normal),
                    );
                } else {
                    let n = s.ticked();
                    let text = if n == 1 {
                        "Write position to 1 photo…".to_owned()
                    } else {
                        format!("Write positions to {} photos…", thousands(n))
                    };
                    let ok = n > 0 && s.track.has_times() && !s.picking;
                    if widgets::primary(ui, p, None, &text, ok).clicked() {
                        action = Some(GeoAction::Write);
                    }
                    if widgets::secondary(ui, p, None, "Cancel", true).clicked() {
                        action = Some(GeoAction::Leave);
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
                        section(ui, p, "Last written", |ui| {
                            ui.horizontal(|ui| {
                                ui.label(RichText::new(last).color(p.text_normal));
                                ui.with_layout(
                                    egui::Layout::right_to_left(egui::Align::Center),
                                    |ui| {
                                        if widgets::secondary(ui, p, None, "Undo it", true)
                                            .clicked()
                                        {
                                            action = Some(GeoAction::UndoLast);
                                        }
                                    },
                                );
                            });
                        });
                    }
                    track_section(ui, p, s, &mut action);
                    if s.track.has_times() {
                        shift_section(ui, p, s, &mut action);
                        matching_section(ui, p, s, &mut action);
                        photos_section(ui, p, s);
                    }
                });
        });
    });
    action
}

fn track_section(ui: &mut Ui, p: &Palette, s: &Session, action: &mut Option<GeoAction>) {
    section(ui, p, "Track", |ui| {
        let t = &s.track;
        ui.add(egui::Label::new(RichText::new(&t.name).color(p.text_strong)).truncate());
        if t.files > 1 {
            weak(ui, p, &format!("{} track files as one timeline", t.files));
        }
        let shift = s.shift.total() as f64;
        match t.span() {
            Some((a, b)) => {
                kv(
                    ui,
                    p,
                    "Recorded",
                    &format!(
                        "{}, {} – {}",
                        time::day(a + shift),
                        &time::clock(a + shift)[..5],
                        &time::clock(b + shift)[..5],
                    ),
                );
                let mins = ((b - a) / 60.0).round() as i64;
                kv(
                    ui,
                    p,
                    "Length",
                    &format!(
                        "{:.1} km · {} h {} min",
                        t.length_m / 1000.0,
                        mins / 60,
                        mins % 60
                    ),
                );
            }
            None => kv(ui, p, "Length", &format!("{:.1} km", t.length_m / 1000.0)),
        }
        let pauses = t.pauses(s.max_gap()).len();
        let mut pts = format!(
            "{} · {} segment{}",
            thousands(t.points),
            t.lines.len(),
            if t.lines.len() == 1 { "" } else { "s" }
        );
        if pauses > 0 {
            pts.push_str(&format!(
                " · {pauses} pause{}",
                if pauses == 1 { "" } else { "s" }
            ));
        }
        kv(ui, p, "Points", &pts);
        if t.dropped > 0 {
            weak(
                ui,
                p,
                &format!(
                    "{} points without a time were left out.",
                    thousands(t.dropped)
                ),
            );
        }
        if !t.has_times() {
            ui.add_space(p.space_1);
            widgets::banner(
                ui,
                p,
                Tone::Warning,
                "This track has no times, so photos can’t be matched to it. Tracks recorded by \
                 a GPS device or a phone app have them; routes planned on a website usually \
                 don’t.",
            );
        }
        ui.add_space(p.space_1);
        if widgets::secondary(ui, p, Some(Icon::Plus), "Add another track file…", true).clicked()
        {
            *action = Some(GeoAction::AddFile);
        }
    });
}

fn shift_section(ui: &mut Ui, p: &Palette, s: &Session, action: &mut Option<GeoAction>) {
    section(ui, p, "Time shift: what the photos’ clocks read", |ui| {
        ui.label(
            RichText::new(time::shift_text(s.shift.total()))
                .family(egui::FontFamily::Name(SEMIBOLD.into()))
                .size(20.0)
                .color(p.text_strong),
        );
        if let Some((id, _)) = s.anchored {
            ui.horizontal(|ui| {
                let name = s.photo(id).map_or("a photo", |p| p.original_name.as_str());
                weak(ui, p, &format!("Set from {name}’s place."));
                if ui.small_button("Undo").clicked() {
                    *action = Some(GeoAction::UndoAnchor);
                }
            });
        }
        ui.add_space(p.space_1);
        widgets::label(ui, p, "Time zone");
        ui.horizontal(|ui| {
            if widgets::secondary(ui, p, None, "−1 h", s.shift.zone > -12 * 3600).clicked() {
                *action = Some(GeoAction::Zone(s.shift.zone - 3600));
            }
            let mut minutes = s.shift.zone / 60;
            let r = ui.add(
                egui::DragValue::new(&mut minutes)
                    .range(-12 * 60..=14 * 60)
                    .speed(15.0)
                    .custom_formatter(|m, _| time::zone_text(m as i64 * 60))
                    .custom_parser(|t| {
                        let t = t.trim().trim_start_matches("UTC");
                        time::parse_offset(t).flatten().map(|s| (s / 60) as f64)
                    }),
            );
            widgets::named(&r, "Time zone", true);
            if r.changed() {
                *action = Some(GeoAction::Zone(minutes * 60));
            }
            if widgets::secondary(ui, p, None, "+1 h", s.shift.zone < 14 * 3600).clicked() {
                *action = Some(GeoAction::Zone(s.shift.zone + 3600));
            }
        });
        ui.add_space(p.space_1);
        ui.horizontal(|ui| {
            widgets::label(ui, p, "Camera clock correction");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let c = s.shift.correction;
                let text = if c == 0 {
                    "none".to_owned()
                } else {
                    let a = c.abs();
                    let sign = if c < 0 { '−' } else { '+' };
                    match (a / 60, a % 60) {
                        (0, sec) => format!("{sign}{sec} s"),
                        (m, 0) => format!("{sign}{m} min"),
                        (m, sec) => format!("{sign}{m} min {sec} s"),
                    }
                };
                ui.label(RichText::new(text).color(p.text_strong));
            });
        });
        let mut c = s.shift.correction;
        ui.spacing_mut().slider_width = ui.available_width();
        let r = ui.add(
            egui::Slider::new(&mut c, -1800..=1800)
                .step_by(5.0)
                .show_value(false),
        );
        widgets::named(&r, "Camera clock correction", true);
        if r.changed() {
            *action = Some(GeoAction::Correction(c));
        }
        ui.horizontal(|ui| {
            weak(ui, p, "−30 min");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                weak(ui, p, "+30 min");
            });
        });
        ui.add_space(p.space_1);
        if s.picking {
            ui.horizontal(|ui| {
                weak(ui, p, "Click the spot on the track where it was taken.");
                if ui.small_button("Cancel").clicked() {
                    *action = Some(GeoAction::CancelPick);
                }
            });
        } else {
            let r = widgets::secondary(
                ui,
                p,
                Some(Icon::Crosshair),
                "Set from a photo’s place…",
                s.selected.is_some(),
            )
            .on_disabled_hover_text("Select a photo on the map first");
            if r.clicked() {
                *action = Some(GeoAction::StartPick);
            }
        }
        if s.has_own_offsets() {
            let mut ignore = s.shift.ignore_own;
            if ui
                .checkbox(&mut ignore, "Ignore the photos’ own time zones")
                .changed()
            {
                *action = Some(GeoAction::IgnoreOwn(ignore));
            }
        }
    });
}

fn matching_section(ui: &mut Ui, p: &Palette, s: &Session, action: &mut Option<GeoAction>) {
    section(ui, p, "Matching", |ui| {
        ui.horizontal(|ui| {
            ui.label("Bridge gaps up to");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                let mut m = s.max_gap_min;
                let r = ui.add(egui::DragValue::new(&mut m).range(1..=240).suffix(" min"));
                widgets::named(&r, "Bridge gaps up to", true);
                if r.changed() {
                    *action = Some(GeoAction::MaxGap(m));
                }
            });
        });
        ui.horizontal(|ui| {
            ui.label("Look this far around it");
            ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                for h in MARGINS.iter().rev() {
                    if ui
                        .selectable_label(s.margin_h == *h, format!("{h} h"))
                        .clicked()
                    {
                        *action = Some(GeoAction::Margin(*h));
                    }
                }
            });
        });
        let mut retag = s.retag;
        if ui
            .checkbox(&mut retag, "Also re-tag photos that have a position")
            .changed()
        {
            *action = Some(GeoAction::Retag(retag));
        }
        let mut alt = s.write_alt;
        if ui.checkbox(&mut alt, "Write the altitude too").changed() {
            *action = Some(GeoAction::WriteAlt(alt));
        }
        if s.track.nmea {
            weak(
                ui,
                p,
                "Off by default for NMEA logs: old GPS loggers’ altitude is often poor.",
            );
        }
    });
}

fn photos_section(ui: &mut Ui, p: &Palette, s: &Session) {
    section(ui, p, "Photos", |ui| {
        ui.label(
            RichText::new(format!(
                "{} on the track, {} ticked",
                thousands(s.on_track()),
                thousands(s.ticked())
            ))
            .color(p.text_strong),
        );
        let out = &s.matched.outside;
        if out.is_empty() {
            weak(ui, p, "None outside the track.");
        } else {
            weak(
                ui,
                p,
                &format!("{} outside the track at this shift:", thousands(out.len())),
            );
            for (id, why) in out.iter().take(OUTSIDE_ROWS) {
                let name = s.photo(*id).map_or("", |p| p.original_name.as_str());
                let clock = s.clock_of(*id).unwrap_or_default();
                ui.horizontal(|ui| {
                    ui.add(
                        egui::Label::new(
                            RichText::new(format!("{name} · {}", &clock[..clock.len().min(5)]))
                                .small(),
                        )
                        .truncate(),
                    );
                    ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                        weak(ui, p, &Session::why(*why));
                    });
                });
            }
            if out.len() > OUTSIDE_ROWS {
                weak(
                    ui,
                    p,
                    &format!("and {} more", thousands(out.len() - OUTSIDE_ROWS)),
                );
            }
        }
        let further = s.matched.further;
        if further > 0 {
            weak(
                ui,
                p,
                &format!(
                    "{} more photo{} further than {} h from the track.",
                    thousands(further),
                    if further == 1 { " is" } else { "s are" },
                    s.margin_h
                ),
            );
        }
        if s.undated > 0 {
            weak(
                ui,
                p,
                &format!(
                    "{} photo{} without a position {} no capture time and can’t be matched.",
                    thousands(s.undated as usize),
                    if s.undated == 1 { "" } else { "s" },
                    if s.undated == 1 { "has" } else { "have" },
                ),
            );
        }
    });
}

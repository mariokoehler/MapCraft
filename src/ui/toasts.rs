//! Toasts (canvas `MapCraft — Import`, artboard 3): what an import did, with the number
//! (design system principle 8: "Say what happened, with the number").

use std::time::Duration;

use egui::{Align2, Area, Frame, Id, Margin, Order, Rect, RichText, Stroke};

use crate::import::{Bounds, Summary};
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Tone, thousands};

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ToastAction {
    ZoomTo(Bounds),
    /// Show the photo in LightCraft again.
    RetryLightCraft,
    OpenSettings,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Toast {
    pub tone: Tone,
    pub title: String,
    pub text: String,
    pub action: Option<(String, ToastAction)>,
}

#[derive(Default)]
pub struct Toasts {
    list: Vec<Toast>,
}

impl Toasts {
    pub fn replace(&mut self, toasts: Vec<Toast>) {
        self.list = toasts;
    }

    pub fn remove_titled(&mut self, title: &str) {
        self.list.retain(|t| t.title != title);
    }

    pub fn titles(&self) -> Vec<&str> {
        self.list.iter().map(|t| t.title.as_str()).collect()
    }

    /// Draws the toasts bottom-left of the map, above the scale bar.
    pub fn show(&mut self, ctx: &egui::Context, p: &Palette, map: Rect) -> Option<ToastAction> {
        let mut action = None;
        let mut dismiss = None;
        if self.list.is_empty() {
            return None;
        }
        Area::new(Id::unique("toasts"))
            .order(Order::Foreground)
            .pivot(Align2::LEFT_BOTTOM)
            .fixed_pos(map.left_bottom() + egui::vec2(p.space_3, -44.0))
            .show(ctx, |ui| {
                ui.spacing_mut().item_spacing.y = p.space_2;
                for (i, t) in self.list.iter().enumerate() {
                    let (fg, _) = t.tone.colors(p);
                    Frame::NONE
                        .fill(p.bg_window)
                        .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
                        .corner_radius(p.radius_md)
                        .shadow(p.elevation_popup)
                        .inner_margin(Margin::same(10))
                        .show(ui, |ui| {
                            ui.set_max_width(380.0);
                            ui.horizontal_top(|ui| {
                                ui.add(t.tone.icon().image(p.size_icon_md, fg));
                                ui.vertical(|ui| {
                                    ui.label(RichText::new(&t.title).strong().color(p.text_strong));
                                    ui.label(RichText::new(&t.text).color(p.text_normal));
                                    if let Some((label, a)) = &t.action
                                        && widgets::secondary(ui, p, None, label, true).clicked()
                                    {
                                        action = Some(*a);
                                    }
                                });
                                if widgets::ghost(ui, p, Icon::X, "Dismiss", true).clicked() {
                                    dismiss = Some(i);
                                }
                            });
                        });
                }
            });
        if let Some(i) = dismiss {
            self.list.remove(i);
        }
        action
    }
}

/// "45 s", "1 min 12 s".
pub fn duration(d: Duration) -> String {
    let s = d.as_secs();
    if s < 60 {
        format!("{s} s")
    } else {
        format!("{} min {} s", s / 60, s % 60)
    }
}

/// The toasts for a finished import or rescan.
pub fn for_summary(s: &Summary, first: bool, took: Duration) -> Vec<Toast> {
    let written = s.new + s.changed + s.moved + s.restored;
    let zoom = |label: &str| {
        s.bounds
            .filter(|_| s.new + s.moved + s.restored > 0 || first)
            .map(|b| (label.to_owned(), ToastAction::ZoomTo(b)))
    };
    let mut out = Vec::new();
    if s.cancelled {
        out.push(Toast {
            tone: Tone::Warning,
            title: if first {
                "Import paused"
            } else {
                "Rescan paused"
            }
            .into(),
            text: format!(
                "{} photos read so far. The rest is read when you rescan; nothing is read twice.",
                thousands(written)
            ),
            action: zoom("Zoom to new photos"),
        });
    } else if first {
        out.push(Toast {
            tone: Tone::Success,
            title: "Import finished".into(),
            text: format!(
                "{} photos · {} on the map · {}",
                thousands(written),
                thousands(written.saturating_sub(s.without_position)),
                duration(took)
            ),
            action: zoom("Zoom to new photos"),
        });
    } else {
        let mut parts = vec![format!("{} new", thousands(s.new))];
        for (n, what) in [
            (s.changed, "changed"),
            (s.moved, "moved"),
            (s.restored, "back"),
            (s.missing, "missing"),
        ] {
            if n > 0 {
                parts.push(format!("{} {what}", thousands(n)));
            }
        }
        parts.push(format!("{} unchanged", thousands(s.unchanged)));
        out.push(Toast {
            tone: Tone::Success,
            title: "Rescan finished".into(),
            text: format!("{} · {}", parts.join(" · "), duration(took)),
            action: zoom("Zoom to new photos"),
        });
    }

    let mut notes = Vec::new();
    if s.skipped > 0 {
        notes.push(format!(
            "{} couldn't be read and will be tried again next time.",
            files(s.skipped)
        ));
    }
    if s.failed > 0 {
        notes.push(format!("{} have unreadable metadata.", files(s.failed)));
    }
    if s.unreadable_folders > 0 {
        notes.push(format!(
            "{} couldn't be listed.",
            plural(s.unreadable_folders, "folder", "folders")
        ));
    }
    if s.without_position > 0 {
        out.push(Toast {
            tone: Tone::Warning,
            title: format!(
                "{} have no position",
                plural(s.without_position, "photo", "photos")
            ),
            text: std::iter::once("They're in the library but not on the map.".to_owned())
                .chain(notes)
                .collect::<Vec<_>>()
                .join(" "),
            action: None,
        });
    } else if !notes.is_empty() {
        out.push(Toast {
            tone: Tone::Warning,
            title: "Some files were skipped".into(),
            text: notes.join(" "),
            action: None,
        });
    }
    out
}

/// The toast for an import that failed outright (the source went offline, the library broke).
pub fn for_failure(message: &str) -> Toast {
    Toast {
        tone: Tone::Error,
        title: "Import failed".into(),
        text: format!("{message}. Nothing was lost; try again when the folder is reachable."),
        action: None,
    }
}

/// A toast that isn't about an import: an app that couldn't start, LightCraft's answers.
pub fn note(tone: Tone, title: &str, text: &str, action: Option<(&str, ToastAction)>) -> Toast {
    Toast {
        tone,
        title: title.to_owned(),
        text: text.to_owned(),
        action: action.map(|(l, a)| (l.to_owned(), a)),
    }
}

fn files(n: usize) -> String {
    plural(n, "file", "files")
}

fn plural(n: usize, one: &str, many: &str) -> String {
    format!("{} {}", thousands(n), if n == 1 { one } else { many })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn summary() -> Summary {
        Summary {
            photos: 2053,
            new: 2053,
            without_position: 407,
            bounds: Some(Bounds::around(1.0, 2.0)),
            ..Summary::default()
        }
    }

    #[test]
    fn a_first_import_says_what_reached_the_map() {
        let t = for_summary(&summary(), true, Duration::from_secs(72));
        assert_eq!(t[0].title, "Import finished");
        assert_eq!(t[0].text, "2,053 photos · 1,646 on the map · 1 min 12 s");
        assert!(t[0].action.is_some());
        assert_eq!(t[1].title, "407 photos have no position");
    }

    #[test]
    fn a_rescan_lists_only_what_changed() {
        let s = Summary {
            photos: 600,
            new: 3,
            moved: 2,
            missing: 1,
            unchanged: 594,
            skipped: 1,
            ..Summary::default()
        };
        let t = for_summary(&s, false, Duration::from_secs(4));
        assert_eq!(t[0].title, "Rescan finished");
        assert_eq!(
            t[0].text,
            "3 new · 2 moved · 1 missing · 594 unchanged · 4 s"
        );
        assert_eq!(t[1].title, "Some files were skipped");
        assert!(t[1].text.starts_with("1 file couldn't be read"));
    }

    #[test]
    fn a_cancelled_import_promises_to_resume() {
        let s = Summary {
            new: 120,
            cancelled: true,
            ..Summary::default()
        };
        let t = for_summary(&s, true, Duration::from_secs(10));
        assert_eq!(t[0].title, "Import paused");
        assert!(t[0].text.contains("120 photos read so far"));
    }
}

//! Small widgets shared by the dialogs, drawn from the design system's tokens: the tri-state
//! checkbox, the button kinds, status chips and the dialog frame.

use egui::{
    Align, Button, Color32, CornerRadius, Frame, Id, Layout, Margin, Modal, Response, RichText,
    Sense, Stroke, StrokeKind, Ui, Vec2,
};

use crate::import::scope::Check;
use crate::theme::Palette;
use crate::ui::icons::Icon;

/// The status tones of chips, banners and toasts.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tone {
    Success,
    Warning,
    Error,
    Offline,
}

impl Tone {
    pub fn colors(self, p: &Palette) -> (Color32, Color32) {
        match self {
            Self::Success => (p.status_success, p.status_success_subtle),
            Self::Warning => (p.status_warning, p.status_warning_subtle),
            Self::Error => (p.status_error, p.status_error_subtle),
            Self::Offline => (p.status_offline, p.status_offline_subtle),
        }
    }

    pub fn icon(self) -> Icon {
        match self {
            Self::Success => Icon::CircleCheck,
            Self::Warning => Icon::TriangleAlert,
            Self::Error => Icon::CircleAlert,
            Self::Offline => Icon::WifiOff,
        }
    }
}

/// The design system's 14 px checkbox with on, off and mixed states. Returns the click.
/// `name` is its accessible name (what a screen reader, and the UI tests, call it).
pub fn tri_check(ui: &mut Ui, p: &Palette, state: Check, enabled: bool, name: &str) -> Response {
    let (rect, response) = ui.allocate_exact_size(Vec2::splat(14.0), Sense::click());
    response.widget_info(|| {
        egui::WidgetInfo::selected(egui::Role::CheckBox, enabled, state == Check::On, name)
    });
    let response = if enabled {
        response
    } else {
        response.on_hover_cursor(egui::CursorIcon::Default)
    };
    let painter = ui.painter();
    let filled = state != Check::Off;
    let (fill, stroke) = match (enabled, filled) {
        (false, _) => (p.bg_faint, p.border_subtle),
        (true, true) => (
            if response.hovered() {
                p.accent_hover
            } else {
                p.accent
            },
            p.accent,
        ),
        (true, false) => (
            p.bg_extreme,
            if response.hovered() {
                p.border_strong
            } else {
                p.border_normal
            },
        ),
    };
    painter.rect(
        rect,
        p.radius_sm,
        fill,
        Stroke::new(1.0, stroke),
        StrokeKind::Inside,
    );
    let mark = match state {
        Check::On => Some(Icon::Check),
        Check::Mixed => Some(Icon::Minus),
        Check::Off => None,
    };
    if let (Some(icon), true) = (mark, enabled) {
        icon.image(12.0, p.text_on_accent).paint_at(
            ui,
            egui::Rect::from_center_size(rect.center(), Vec2::splat(12.0)),
        );
    }
    if enabled {
        response
    } else {
        response.on_hover_text("Never imported")
    }
}

fn content(icon: Option<Icon>, text: &str, color: Color32, p: &Palette) -> Button<'static> {
    let label = RichText::new(text.to_owned()).color(color);
    match icon {
        Some(i) => Button::image_and_text(i.image(p.size_icon_sm, color), label),
        None => Button::new(label),
    }
}

/// The primary button: accent fill, `text-on-accent`.
pub fn primary(
    ui: &mut Ui,
    p: &Palette,
    icon: Option<Icon>,
    text: &str,
    enabled: bool,
) -> Response {
    let ink = if enabled {
        p.text_on_accent
    } else {
        p.text_disabled
    };
    let fill = if enabled { p.accent } else { p.bg_faint };
    ui.add_enabled(enabled, content(icon, text, ink, p).fill(fill))
}

/// A secondary button.
pub fn secondary(
    ui: &mut Ui,
    p: &Palette,
    icon: Option<Icon>,
    text: &str,
    enabled: bool,
) -> Response {
    let ink = if enabled {
        p.text_normal
    } else {
        p.text_disabled
    };
    ui.add_enabled(enabled, content(icon, text, ink, p))
}

/// A danger button: outlined in `status-error`.
pub fn danger(ui: &mut Ui, p: &Palette, text: &str, enabled: bool) -> Response {
    let ink = if enabled {
        p.status_error
    } else {
        p.text_disabled
    };
    ui.add_enabled(
        enabled,
        content(None, text, ink, p)
            .stroke(Stroke::new(1.0, ink))
            .fill(Color32::TRANSPARENT),
    )
}

/// An icon-only ghost button with a tooltip.
pub fn ghost(ui: &mut Ui, p: &Palette, icon: Icon, hint: &str, enabled: bool) -> Response {
    let size = ui.spacing().interact_size.y;
    let ink = if enabled {
        p.text_normal
    } else {
        p.text_disabled
    };
    let r = ui.add_enabled(
        enabled,
        Button::image(icon.image(p.size_icon_sm, ink))
            .frame_when_inactive(false)
            .min_size(Vec2::splat(size)),
    );
    named(&r, hint, enabled);
    if enabled {
        r.on_hover_text(hint)
    } else {
        r.on_disabled_hover_text(hint)
    }
}

/// Gives an icon-only button its accessible name (a screen reader's label, like `aria-label`;
/// design system: "aria-label on icon-only buttons"). The text before a colon is the name:
/// "Zoom to all photos: no photos yet" → "Zoom to all photos".
pub fn named(r: &Response, hint: &str, enabled: bool) {
    let name = hint.split(':').next().unwrap_or(hint).trim().to_owned();
    r.widget_info(|| egui::WidgetInfo::labeled(egui::Role::Button, enabled, &name));
}

/// A status chip: icon and word, on the tone's subtle ground.
pub fn chip(ui: &mut Ui, p: &Palette, tone: Tone, text: &str) {
    let (fg, bg) = tone.colors(p);
    Frame::NONE
        .fill(bg)
        .corner_radius(CornerRadius::same(255))
        .inner_margin(Margin::symmetric(6, 1))
        .show(ui, |ui| {
            ui.spacing_mut().item_spacing.x = 4.0;
            ui.horizontal(|ui| {
                ui.add(tone.icon().image(12.0, fg));
                ui.label(RichText::new(text).small().color(fg));
            });
        });
}

/// A modal dialog in the design system's frame: title bar, body, right-aligned button row.
/// The map behind isn't dimmed (flat, no scrim) but takes no input. Returns `true` when the
/// user asked to close it: Esc or the close button. A click outside does nothing, so stray
/// clicks on the map can't discard what's been picked in a dialog.
#[allow(clippy::too_many_arguments)] // a layout helper: each part is one argument
pub fn dialog(
    ctx: &egui::Context,
    p: &Palette,
    id: &str,
    title: &str,
    width: f32,
    closable: bool,
    body: impl FnOnce(&mut Ui),
    footer: impl FnOnce(&mut Ui),
) -> bool {
    let frame = Frame::NONE
        .fill(p.bg_window)
        .stroke(Stroke::new(p.stroke_hairline, p.border_subtle))
        .corner_radius(p.radius_lg)
        .shadow(p.elevation_modal);
    let mut close_clicked = false;
    let response = Modal::new(Id::unique(id))
        .frame(frame)
        .backdrop_color(Color32::TRANSPARENT)
        .show(ctx, |ui| {
            ui.set_width(width);
            ui.spacing_mut().item_spacing.y = p.space_2;
            Frame::NONE
                .inner_margin(Margin::symmetric(12, 4))
                .show(ui, |ui| {
                    ui.horizontal(|ui| {
                        ui.label(RichText::new(title).heading().color(p.text_strong));
                        if closable {
                            ui.with_layout(Layout::right_to_left(Align::Center), |ui| {
                                close_clicked =
                                    ghost(ui, p, Icon::X, "Close (Esc)", true).clicked();
                            });
                        }
                    });
                });
            separator(ui, p);
            Frame::NONE.inner_margin(Margin::same(12)).show(ui, |ui| {
                ui.set_max_width(width - 24.0); // content never widens the dialog
                body(ui)
            });
            separator(ui, p);
            Frame::NONE
                .inner_margin(Margin::symmetric(12, 8))
                .show(ui, |ui| {
                    ui.with_layout(Layout::right_to_left(Align::Center), footer);
                });
        });
    let _ = response;
    let esc = ctx.input(|i| i.key_pressed(egui::Key::Escape));
    close_clicked || (closable && esc)
}

fn separator(ui: &mut Ui, p: &Palette) {
    let w = ui.available_width();
    let (rect, _) = ui.allocate_exact_size(egui::vec2(w, p.stroke_hairline), Sense::hover());
    ui.painter().rect_filled(rect, 0.0, p.border_subtle);
}

/// A small weak caption ("Folder", "Folders to import").
pub fn label(ui: &mut Ui, p: &Palette, text: &str) {
    ui.label(RichText::new(text).small().color(p.text_weak));
}

/// Thousands separators: 12800 → "12,800" (design system: numbers).
pub fn thousands(n: usize) -> String {
    let s = n.to_string();
    let mut out = String::new();
    for (i, c) in s.chars().enumerate() {
        if i > 0 && (s.len() - i).is_multiple_of(3) {
            out.push(',');
        }
        out.push(c);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::thousands;

    #[test]
    fn numbers_get_thousands_separators() {
        assert_eq!(thousands(0), "0");
        assert_eq!(thousands(999), "999");
        assert_eq!(thousands(12800), "12,800");
        assert_eq!(thousands(1234567), "1,234,567");
    }
}

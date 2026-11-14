//! The egui theme, transcribed from the design system (design.md §3.9).
//!
//! `apply` sets egui's global `Style` (fonts, text sizes, spacing, widget colours) from the
//! tokens; `Palette` holds the colours and sizes MapCraft's own drawing code uses (the map
//! overlays, the top bar). Nothing outside this module hard-codes a colour or a size.

pub mod tokens;

use std::sync::Arc;

use egui::{
    Color32, CornerRadius, FontData, FontDefinitions, FontFamily, FontId, Margin, Shadow, Stroke,
    TextStyle, Visuals, style::WidgetVisuals,
};

pub use tokens::{ThemeKind, Tokens};

/// The family name for IBM Plex Sans SemiBold. egui has no weight axis, so the bold face is its
/// own family (the design system README says so).
pub const SEMIBOLD: &str = "sans-semibold";
/// The text style for empty states (22 px SemiBold).
pub const DISPLAY: &str = "Display";

/// Colours and sizes for MapCraft's own painting. Built once from the tokens; every field is
/// read here, so a renamed token fails the `palette_builds_in_both_themes` test.
#[derive(Debug, Clone)]
pub struct Palette {
    pub bg_panel: Color32,
    pub bg_window: Color32,
    pub text_strong: Color32,
    pub text_normal: Color32,
    pub text_weak: Color32,
    pub text_disabled: Color32,
    pub text_on_accent: Color32,
    pub accent: Color32,
    pub border_subtle: Color32,
    pub widget_active_weak: Color32,
    pub bg_extreme: Color32,
    pub map_ref_land: Color32,
    pub map_scrim_light: Color32,
    pub map_scrim_light_text: Color32,
    pub elevation_panel: Shadow,
    pub radius_sm: u8,
    pub radius_md: u8,
    pub space_1: f32,
    pub space_2: f32,
    pub space_3: f32,
    pub space_4: f32,
    pub size_topbar: f32,
    pub size_map_control: f32,
    pub size_icon_sm: f32,
    pub size_icon_md: f32,
    pub stroke_hairline: f32,
}

impl Palette {
    pub fn new(t: &Tokens) -> Self {
        Self {
            bg_panel: t.color("bg-panel"),
            bg_window: t.color("bg-window"),
            text_strong: t.color("text-strong"),
            text_normal: t.color("text-normal"),
            text_weak: t.color("text-weak"),
            text_disabled: t.color("text-disabled"),
            text_on_accent: t.color("text-on-accent"),
            accent: t.color("accent"),
            border_subtle: t.color("border-subtle"),
            widget_active_weak: t.color("widget-active-weak"),
            bg_extreme: t.color("bg-extreme"),
            map_ref_land: t.color("map-ref-land"),
            map_scrim_light: t.color("map-scrim-light"),
            map_scrim_light_text: t.color("map-scrim-light-text"),
            elevation_panel: t.shadow("elevation-panel"),
            radius_sm: t.px("radius-sm") as u8,
            radius_md: t.px("radius-md") as u8,
            space_1: t.px("space-1"),
            space_2: t.px("space-2"),
            space_3: t.px("space-3"),
            space_4: t.px("space-4"),
            size_topbar: t.px("size-topbar"),
            size_map_control: t.px("size-map-control"),
            size_icon_sm: t.px("size-icon-sm"),
            size_icon_md: t.px("size-icon-md"),
            stroke_hairline: t.px("stroke-hairline"),
        }
    }
}

/// Installs fonts and the style for `t` on the egui context.
pub fn apply(ctx: &egui::Context, t: &Tokens, kind: ThemeKind) {
    ctx.set_fonts(fonts());
    let visuals = visuals(t, kind);
    ctx.all_styles_mut(|style| {
        style.visuals = visuals.clone();
        style.text_styles = text_styles();
        style.animation_time = 0.08; // 80 ms: hover, press, tooltip fade

        let s = &mut style.spacing;
        s.item_spacing = egui::vec2(t.px("item-spacing-x"), t.px("item-spacing-y"));
        s.button_padding = egui::vec2(t.px("button-padding-x"), t.px("button-padding-y"));
        s.indent = t.px("indent");
        s.interact_size.y = t.px("interact-height");
        s.window_margin = Margin::same(t.px("window-margin") as i8);
        s.menu_margin = Margin::same(t.px("menu-margin") as i8);
        s.icon_width = t.px("size-checkbox");
        s.scroll.bar_width = t.px("size-scrollbar");
    });
}

/// IBM Plex Sans and Mono (SIL OFL, `assets/fonts/`), embedded in the exe.
fn fonts() -> FontDefinitions {
    let mut fonts = FontDefinitions::default();
    let mut add = |name: &str, bytes: &'static [u8]| {
        fonts
            .font_data
            .insert(name.to_owned(), Arc::new(FontData::from_static(bytes)));
    };
    add(
        "plex-sans",
        include_bytes!("../../assets/fonts/IBMPlexSans-Regular.ttf"),
    );
    add(
        "plex-sans-semibold",
        include_bytes!("../../assets/fonts/IBMPlexSans-SemiBold.ttf"),
    );
    add(
        "plex-mono",
        include_bytes!("../../assets/fonts/IBMPlexMono-Regular.ttf"),
    );

    // Plex first; egui's built-in fonts stay behind it as fallbacks (symbols, emoji).
    let families = &mut fonts.families;
    families
        .entry(FontFamily::Proportional)
        .or_default()
        .insert(0, "plex-sans".to_owned());
    families
        .entry(FontFamily::Monospace)
        .or_default()
        .insert(0, "plex-mono".to_owned());
    let mut semibold = vec!["plex-sans-semibold".to_owned()];
    semibold.extend(families[&FontFamily::Proportional].iter().cloned());
    families.insert(FontFamily::Name(SEMIBOLD.into()), semibold);
    fonts
}

/// The type scale: Small 11, Body 13, Button 13, Heading 15 SemiBold, Monospace 12, Display 22.
fn text_styles() -> std::collections::BTreeMap<TextStyle, FontId> {
    let semibold = FontFamily::Name(SEMIBOLD.into());
    [
        (
            TextStyle::Small,
            FontId::new(11.0, FontFamily::Proportional),
        ),
        (TextStyle::Body, FontId::new(13.0, FontFamily::Proportional)),
        (
            TextStyle::Button,
            FontId::new(13.0, FontFamily::Proportional),
        ),
        (TextStyle::Heading, FontId::new(15.0, semibold.clone())),
        (
            TextStyle::Monospace,
            FontId::new(12.0, FontFamily::Monospace),
        ),
        (TextStyle::Name(DISPLAY.into()), FontId::new(22.0, semibold)),
    ]
    .into()
}

fn visuals(t: &Tokens, kind: ThemeKind) -> Visuals {
    let mut v = match kind {
        ThemeKind::Dark => Visuals::dark(),
        ThemeKind::Light => Visuals::light(),
    };
    v.window_fill = t.color("bg-window");
    v.panel_fill = t.color("bg-panel");
    v.extreme_bg_color = t.color("bg-extreme");
    v.faint_bg_color = t.color("bg-faint");
    v.code_bg_color = t.color("bg-code");
    v.override_text_color = None;
    v.weak_text_color = Some(t.color("text-weak"));
    v.hyperlink_color = t.color("link");
    v.selection.bg_fill = t.color("accent-subtle");
    v.selection.stroke = Stroke::new(1.0, t.color("accent"));
    v.window_stroke = Stroke::new(t.px("stroke-hairline"), t.color("border-subtle"));
    v.window_corner_radius = CornerRadius::same(t.px("radius-lg") as u8);
    v.menu_corner_radius = CornerRadius::same(t.px("radius-md") as u8);
    v.window_shadow = t.shadow("elevation-modal");
    v.popup_shadow = t.shadow("elevation-popup");
    v.warn_fg_color = t.color("status-warning");
    v.error_fg_color = t.color("status-error");

    let state = |name: &str| WidgetVisuals {
        bg_fill: t.color(&format!("widget-{name}-bg")),
        weak_bg_fill: t.color(&format!("widget-{name}-weak")),
        bg_stroke: Stroke::new(
            t.px(&format!("widget-{name}-stroke-width")),
            t.color(&format!("widget-{name}-stroke")),
        ),
        corner_radius: CornerRadius::same(t.px("radius-sm") as u8),
        fg_stroke: Stroke::new(1.0, t.color(&format!("widget-{name}-fg"))),
        expansion: t.px(&format!("expansion-{name}")),
    };
    v.widgets.noninteractive = state("noninteractive");
    v.widgets.inactive = state("inactive");
    v.widgets.hovered = state("hovered");
    v.widgets.active = state("active");
    v.widgets.open = state("open");
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn palette_and_visuals_build_in_both_themes() {
        for kind in [ThemeKind::Dark, ThemeKind::Light] {
            let t = Tokens::embedded(kind).unwrap();
            let p = Palette::new(&t);
            let v = visuals(&t, kind);
            assert_eq!(v.panel_fill, p.bg_panel);
        }
    }

    #[test]
    fn fonts_and_text_styles_are_consistent() {
        let fonts = fonts();
        for style in text_styles().values() {
            assert!(
                fonts.families.contains_key(&style.family),
                "{:?}",
                style.family
            );
        }
        assert_eq!(
            fonts.font_data.len() - FontDefinitions::default().font_data.len(),
            3
        );
    }
}

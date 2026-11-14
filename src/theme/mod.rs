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
    // Added for M2's dialogs, toasts and dots.
    pub bg_faint: Color32,
    pub accent_hover: Color32,
    pub accent_subtle: Color32,
    pub border_normal: Color32,
    pub border_strong: Color32,
    pub widget_hovered_weak: Color32,
    pub status_success: Color32,
    pub status_success_subtle: Color32,
    pub status_warning: Color32,
    pub status_warning_subtle: Color32,
    pub status_error: Color32,
    pub status_error_subtle: Color32,
    pub status_offline: Color32,
    pub status_offline_subtle: Color32,
    pub map_pin: Color32,
    pub map_marker_frame: Color32,
    pub map_marker_edge: Color32,
    pub elevation_popup: Shadow,
    pub elevation_modal: Shadow,
    pub radius_lg: u8,
    pub indent: f32,
    // Added for M3's thumbnail markers, hover card and detail panel.
    pub map_marker_frame_selected: Color32,
    pub map_marker_placeholder: Color32,
    pub map_marker_placeholder_fg: Color32,
    pub map_marker_missing: Color32,
    pub map_marker_missing_fg: Color32,
    pub map_badge_bg: Color32,
    pub map_badge_text: Color32,
    pub elevation_marker: Shadow,
    pub elevation_marker_raised: Shadow,
    pub stroke_marker: f32,
    pub stroke_marker_selected: f32,
    pub stroke_marker_edge: f32,
    pub size_marker: f32,
    pub size_badge: f32,
    pub size_panel: f32,
    pub panel_margin: f32,
    pub interact_height: f32,
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
            bg_faint: t.color("bg-faint"),
            accent_hover: t.color("accent-hover"),
            accent_subtle: t.color("accent-subtle"),
            border_normal: t.color("border-normal"),
            border_strong: t.color("border-strong"),
            widget_hovered_weak: t.color("widget-hovered-weak"),
            status_success: t.color("status-success"),
            status_success_subtle: t.color("status-success-subtle"),
            status_warning: t.color("status-warning"),
            status_warning_subtle: t.color("status-warning-subtle"),
            status_error: t.color("status-error"),
            status_error_subtle: t.color("status-error-subtle"),
            status_offline: t.color("status-offline"),
            status_offline_subtle: t.color("status-offline-subtle"),
            map_pin: t.color("map-pin"),
            map_marker_frame: t.color("map-marker-frame"),
            map_marker_edge: t.color("map-marker-edge"),
            elevation_popup: t.shadow("elevation-popup"),
            elevation_modal: t.shadow("elevation-modal"),
            radius_lg: t.px("radius-lg") as u8,
            indent: t.px("indent"),
            map_marker_frame_selected: t.color("map-marker-frame-selected"),
            map_marker_placeholder: t.color("map-marker-placeholder"),
            map_marker_placeholder_fg: t.color("map-marker-placeholder-fg"),
            map_marker_missing: t.color("map-marker-missing"),
            map_marker_missing_fg: t.color("map-marker-missing-fg"),
            map_badge_bg: t.color("map-badge-bg"),
            map_badge_text: t.color("map-badge-text"),
            elevation_marker: t.shadow("elevation-marker"),
            elevation_marker_raised: t.shadow("elevation-marker-raised"),
            stroke_marker: t.px("stroke-marker"),
            stroke_marker_selected: t.px("stroke-marker-selected"),
            stroke_marker_edge: t.px("stroke-marker-edge"),
            size_marker: t.px("size-marker"),
            size_badge: t.px("size-badge"),
            size_panel: t.px("size-panel"),
            panel_margin: t.px("panel-margin"),
            interact_height: t.px("interact-height"),
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

    /// `text_styles` is written out by hand; this keeps it equal to the design system's scale.
    #[test]
    fn type_scale_matches_the_design_tokens() {
        let json: serde_json::Value =
            serde_json::from_str(include_str!("../../assets/design/tokens.json")).unwrap();
        let styles = text_styles();
        let mut checked = 0;
        for group in json["type"]["groups"].as_array().unwrap() {
            for s in group["styles"].as_array().unwrap() {
                let name = s["name"].as_str().unwrap();
                let style = match name {
                    "Small" => TextStyle::Small,
                    "Body" => TextStyle::Body,
                    "Button" => TextStyle::Button,
                    "Heading" => TextStyle::Heading,
                    "Monospace" => TextStyle::Monospace,
                    "Display" => TextStyle::Name(DISPLAY.into()),
                    other => panic!("the design system has a new text style `{other}`"),
                };
                let font = &styles[&style];
                let size: f32 = s["fontSize"]
                    .as_str()
                    .unwrap()
                    .trim_end_matches("px")
                    .parse()
                    .unwrap();
                assert_eq!(font.size, size, "{name} size");
                let semibold = s["fontWeight"].as_u64() == Some(600);
                let is_semibold = font.family == FontFamily::Name(SEMIBOLD.into());
                assert_eq!(is_semibold, semibold, "{name} weight");
                checked += 1;
            }
        }
        assert_eq!(checked, styles.len());
    }

    /// `apply` reads the spacing and widget tokens; a renamed token panics here, not at startup.
    #[test]
    fn apply_reads_every_style_token() {
        let ctx = egui::Context::default();
        for kind in [ThemeKind::Dark, ThemeKind::Light] {
            apply(&ctx, &Tokens::embedded(kind).unwrap(), kind);
        }
    }
}

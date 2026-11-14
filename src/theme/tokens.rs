//! Reads the design system's `tokens.json` (copied to `assets/design/`, design.md §3.9).
//!
//! The file is embedded in the exe at compile time (`include_str!`, like a classpath resource
//! that can't go missing) and parsed once at startup. Only the shapes the design system uses are
//! accepted; anything else is an error, so a design change that this parser doesn't understand
//! fails loudly instead of drawing the wrong colour.

use std::collections::HashMap;

use egui::{Color32, Shadow};
use serde_json::Value;

/// The design system's `tokens.json`. Copied from the `MapCraft — Design system` canvas.
const TOKENS_JSON: &str = include_str!("../../assets/design/tokens.json");

/// The token families that hold lengths in px.
const LENGTH_FAMILIES: [&str; 5] = ["spacing", "radius", "stroke", "expansion", "size"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ThemeKind {
    Dark,
    #[allow(dead_code)] // built in the tests; a setting for it comes later
    Light,
}

impl ThemeKind {
    fn key(self) -> &'static str {
        match self {
            Self::Dark => "dark",
            Self::Light => "light",
        }
    }
}

/// Every token of one theme, by name.
#[derive(Debug, Clone)]
pub struct Tokens {
    colors: HashMap<String, Color32>,
    lengths: HashMap<String, f32>,
    shadows: HashMap<String, Shadow>,
}

impl Tokens {
    /// The embedded design-system tokens for `kind`.
    pub fn embedded(kind: ThemeKind) -> Result<Self, String> {
        Self::parse(TOKENS_JSON, kind)
    }

    pub fn parse(json: &str, kind: ThemeKind) -> Result<Self, String> {
        let root: Value = serde_json::from_str(json).map_err(|e| format!("tokens.json: {e}"))?;
        let mut tokens = Self {
            colors: HashMap::new(),
            lengths: HashMap::new(),
            shadows: HashMap::new(),
        };
        for (name, value) in family(&root, "color")? {
            let text = for_theme(value, kind).ok_or_else(|| format!("{name}: no value"))?;
            tokens.colors.insert(
                name.clone(),
                parse_color(text).map_err(|e| format!("{name}: {e}"))?,
            );
        }
        for fam in LENGTH_FAMILIES {
            for (name, value) in family(&root, fam)? {
                let text = for_theme(value, kind).ok_or_else(|| format!("{name}: no value"))?;
                tokens.lengths.insert(
                    name.clone(),
                    parse_px(text).map_err(|e| format!("{name}: {e}"))?,
                );
            }
        }
        for (name, value) in family(&root, "shadow")? {
            let text = for_theme(value, kind).ok_or_else(|| format!("{name}: no value"))?;
            tokens.shadows.insert(
                name.clone(),
                parse_shadow(text).map_err(|e| format!("{name}: {e}"))?,
            );
        }
        Ok(tokens)
    }

    /// A colour token. Panics on an unknown name: names are fixed at compile time and every one
    /// the app reads is covered by the tests (`Palette` and the egui style are built in a test).
    pub fn color(&self, name: &str) -> Color32 {
        *self
            .colors
            .get(name)
            .unwrap_or_else(|| panic!("unknown colour token `{name}`"))
    }

    pub fn px(&self, name: &str) -> f32 {
        *self
            .lengths
            .get(name)
            .unwrap_or_else(|| panic!("unknown length token `{name}`"))
    }

    pub fn shadow(&self, name: &str) -> Shadow {
        *self
            .shadows
            .get(name)
            .unwrap_or_else(|| panic!("unknown shadow token `{name}`"))
    }
}

/// The `(name, value)` pairs of one token family (`{"tokens": [{"name", "value"}, …]}`).
fn family<'a>(root: &'a Value, fam: &str) -> Result<Vec<(String, &'a Value)>, String> {
    let list = root[fam]["tokens"]
        .as_array()
        .ok_or_else(|| format!("tokens.json: family `{fam}` has no token list"))?;
    list.iter()
        .map(|t| {
            let name = t["name"]
                .as_str()
                .ok_or_else(|| format!("{fam}: a token without a name"))?;
            Ok((name.to_owned(), &t["value"]))
        })
        .collect()
}

/// A value is either one string for every theme or `{"dark": …, "light": …}`. A theme missing
/// from the object inherits the first theme's (dark) value, as the design system specifies.
fn for_theme(value: &Value, kind: ThemeKind) -> Option<&str> {
    match value {
        Value::String(s) => Some(s),
        Value::Object(map) => map
            .get(kind.key())
            .or_else(|| map.get(ThemeKind::Dark.key()))
            .and_then(Value::as_str),
        _ => None,
    }
}

/// `#rrggbb` or `#rrggbbaa`.
fn parse_color(text: &str) -> Result<Color32, String> {
    let hex = text
        .strip_prefix('#')
        .ok_or_else(|| format!("`{text}` isn't a hex colour"))?;
    let byte = |i: usize| {
        u8::from_str_radix(hex.get(i..i + 2).unwrap_or("x"), 16)
            .map_err(|_| format!("`{text}` isn't a hex colour"))
    };
    match hex.len() {
        6 => Ok(Color32::from_rgb(byte(0)?, byte(2)?, byte(4)?)),
        8 => Ok(Color32::from_rgba_unmultiplied(
            byte(0)?,
            byte(2)?,
            byte(4)?,
            byte(6)?,
        )),
        _ => Err(format!("`{text}` isn't #rrggbb or #rrggbbaa")),
    }
}

/// `12px`, or a bare `0`.
fn parse_px(text: &str) -> Result<f32, String> {
    let number = text.strip_suffix("px").unwrap_or(text);
    if number != text || number == "0" {
        number
            .parse()
            .map_err(|_| format!("`{text}` isn't a px length"))
    } else {
        Err(format!("`{text}` isn't a px length"))
    }
}

/// `x y blur spread #colour`, e.g. `0 4px 12px 0 #0000008c` (CSS box-shadow order).
fn parse_shadow(text: &str) -> Result<Shadow, String> {
    let parts: Vec<&str> = text.split_whitespace().collect();
    let [x, y, blur, spread, color] = parts[..] else {
        return Err(format!("`{text}` isn't `x y blur spread #colour`"));
    };
    // egui stores shadow sizes as small integers (i8/u8); the design system's are all small.
    let int = |s: &str| parse_px(s).map(|v| v.round() as i32);
    Ok(Shadow {
        offset: [int(x)? as i8, int(y)? as i8],
        blur: int(blur)? as u8,
        spread: int(spread)? as u8,
        color: parse_color(color)?,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn embedded_tokens_parse_in_both_themes() {
        let dark = Tokens::embedded(ThemeKind::Dark).unwrap();
        let light = Tokens::embedded(ThemeKind::Light).unwrap();
        assert_ne!(dark.color("bg-panel"), light.color("bg-panel"));
        // Map tokens are the same in both themes (the map is always light).
        assert_eq!(
            dark.color("map-scrim-light"),
            light.color("map-scrim-light")
        );
        assert_eq!(dark.px("size-topbar"), 36.0);
    }

    #[test]
    fn value_shapes() {
        assert_eq!(
            parse_color("#ff8000").unwrap(),
            Color32::from_rgb(255, 128, 0)
        );
        assert_eq!(
            parse_color("#0000008c").unwrap(),
            Color32::from_rgba_unmultiplied(0, 0, 0, 0x8c)
        );
        assert!(parse_color("red").is_err());
        assert!(parse_color("#fff").is_err());
        assert_eq!(parse_px("12px").unwrap(), 12.0);
        assert_eq!(parse_px("0").unwrap(), 0.0);
        assert!(parse_px("1rem").is_err());
        let s = parse_shadow("0 4px 12px 0 #0000008c").unwrap();
        assert_eq!((s.offset, s.blur, s.spread), ([0, 4], 12, 0));
        assert!(parse_shadow("0 4px 12px #000").is_err());
    }

    #[test]
    fn an_unknown_shape_fails_loudly() {
        let json = r#"{"color":{"tokens":[{"name":"x","value":"rgb(1,2,3)"}]},
            "spacing":{"tokens":[]},"radius":{"tokens":[]},"stroke":{"tokens":[]},
            "expansion":{"tokens":[]},"size":{"tokens":[]},"shadow":{"tokens":[]}}"#;
        assert!(Tokens::parse(json, ThemeKind::Dark).is_err());
    }
}

//! Lucide icons (ISC licence, `assets/icons/`, from lucide-static 1.65.0), embedded in the exe.
//!
//! The files draw with `currentColor`, which an SVG renderer paints black, and black can't be
//! tinted to another colour. So each icon is turned white once, then tinted per widget state
//! with `Image::tint` (the design system's rule: single-ink icons, tinted with the fg colour).

use std::sync::{Arc, LazyLock};

use egui::{Color32, Image, ImageSource, load::Bytes};

/// Declares the `Icon` enum, its list and its files in one place. A `macro_rules!` macro
/// generates code at compile time (a little like an annotation processor in Java).
macro_rules! icons {
    ($($variant:ident => $file:literal),* $(,)?) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq)]
        pub enum Icon { $($variant),* }

        const ALL: &[Icon] = &[$(Icon::$variant),*];

        impl Icon {
            fn file(self) -> (&'static str, &'static str) {
                match self {
                    $(Self::$variant => ($file, include_str!(concat!("../../assets/icons/", $file, ".svg")))),*
                }
            }
        }
    };
}

icons! {
    Search => "search",
    FolderPlus => "folder-plus",
    Folder => "folder",
    Folders => "folders",
    Settings => "settings",
    Plus => "plus",
    Minus => "minus",
    Scan => "scan",
    Layers => "layers",
    ChevronRight => "chevron-right",
    ChevronLeft => "chevron-left",
    ChevronDown => "chevron-down",
    Check => "check",
    X => "x",
    Refresh => "refresh-cw",
    CircleCheck => "circle-check",
    CircleAlert => "circle-alert",
    TriangleAlert => "triangle-alert",
    Wifi => "wifi",
    WifiOff => "wifi-off",
    Image => "image",
    ImageOff => "image-off",
    Calendar => "calendar",
    Camera => "camera",
    FolderOpen => "folder-open",
    ExternalLink => "external-link",
    Brush => "brush",
    Sliders => "sliders-horizontal",
}

impl Icon {
    /// The icon at `size` px, tinted `tint`.
    pub fn image(self, size: f32, tint: Color32) -> Image<'static> {
        let (name, _) = self.file();
        let bytes = WHITE[ALL.iter().position(|i| *i == self).expect("listed in ALL")].clone();
        Image::new(ImageSource::Bytes {
            uri: format!("bytes://icon/{name}.svg").into(),
            bytes: Bytes::Shared(bytes),
        })
        .fit_to_exact_size(egui::vec2(size, size))
        .tint(tint)
    }
}

/// Every icon's SVG with its ink turned white, made on first use (like a lazily initialised
/// `static final` in Java).
static WHITE: LazyLock<Vec<Arc<[u8]>>> = LazyLock::new(|| {
    ALL.iter()
        .map(|icon| {
            let svg = icon.file().1.replace("currentColor", "#ffffff");
            Arc::from(svg.into_bytes().into_boxed_slice())
        })
        .collect()
});

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_icon_is_single_ink_and_whitened() {
        for &icon in ALL {
            let (name, svg) = icon.file();
            assert!(
                svg.contains("currentColor"),
                "{name} should draw with currentColor"
            );
            let white = &WHITE[ALL.iter().position(|i| *i == icon).unwrap()];
            let text = std::str::from_utf8(white).unwrap();
            assert!(
                !text.contains("currentColor") && text.contains("#ffffff"),
                "{name}"
            );
        }
    }
}

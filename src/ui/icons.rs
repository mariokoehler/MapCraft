//! Lucide icons (ISC licence, `assets/icons/`, from lucide-static 1.65.0), embedded in the exe.
//!
//! The files draw with `currentColor`, which an SVG renderer paints black, and black can't be
//! tinted to another colour. So each icon is turned white once, then tinted per widget state
//! with `Image::tint` (the design system's rule: single-ink icons, tinted with the fg colour).

use std::sync::{Arc, LazyLock};

use egui::{Color32, Image, ImageSource, load::Bytes};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Icon {
    Search,
    FolderPlus,
    Settings,
    Plus,
    Minus,
    Scan,
    Layers,
}

const ALL: [Icon; 7] = [
    Icon::Search,
    Icon::FolderPlus,
    Icon::Settings,
    Icon::Plus,
    Icon::Minus,
    Icon::Scan,
    Icon::Layers,
];

impl Icon {
    fn file(self) -> (&'static str, &'static str) {
        match self {
            Self::Search => ("search", include_str!("../../assets/icons/search.svg")),
            Self::FolderPlus => (
                "folder-plus",
                include_str!("../../assets/icons/folder-plus.svg"),
            ),
            Self::Settings => ("settings", include_str!("../../assets/icons/settings.svg")),
            Self::Plus => ("plus", include_str!("../../assets/icons/plus.svg")),
            Self::Minus => ("minus", include_str!("../../assets/icons/minus.svg")),
            Self::Scan => ("scan", include_str!("../../assets/icons/scan.svg")),
            Self::Layers => ("layers", include_str!("../../assets/icons/layers.svg")),
        }
    }

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
        for icon in ALL {
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

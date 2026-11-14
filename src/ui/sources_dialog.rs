//! The sources list and the remove confirmation (canvas `MapCraft — Import`, artboards 4–5).
//!
//! Whether each source is online is checked on a background thread when the dialog opens: an
//! unreachable NAS can take many seconds to answer (design.md §3.1).

use std::collections::HashMap;
use std::sync::mpsc::{self, Receiver};

use egui::{Align, Grid, Layout, RichText, Sense, Ui};

use crate::library::{Counts, Source};
use crate::theme::Palette;
use crate::ui::icons::Icon;
use crate::ui::widgets::{self, Tone, thousands};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourcesAction {
    AddFolder,
    Rescan(i64),
    ChangeFolders(i64),
    Remove(i64),
    Close,
}

pub struct SourcesDialog {
    selected: Option<i64>,
    online: HashMap<i64, bool>,
    rx: Receiver<(i64, bool)>,
    confirm_remove: Option<i64>,
}

impl SourcesDialog {
    pub fn open(sources: &[Source], ctx: &egui::Context) -> Self {
        let (tx, rx) = mpsc::channel();
        let roots: Vec<_> = sources
            .iter()
            .map(|s| (s.id, s.root_path.clone(), s.unc_path.clone()))
            .collect();
        let c = ctx.clone();
        std::thread::spawn(move || {
            for (id, root, unc) in roots {
                let ok = crate::import::reachable_root(&root, unc.as_deref()).is_some();
                if tx.send((id, ok)).is_err() {
                    break;
                }
                c.request_repaint();
            }
        });
        Self {
            selected: sources.first().map(|s| s.id),
            online: HashMap::new(),
            rx,
            confirm_remove: None,
        }
    }

    /// `busy`: an import is running, so nothing that starts another one is offered.
    pub fn show(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        sources: &[Source],
        counts: &HashMap<i64, Counts>,
        busy: bool,
    ) -> Option<SourcesAction> {
        for (id, ok) in self.rx.try_iter() {
            self.online.insert(id, ok);
        }
        if let Some(id) = self.confirm_remove {
            return self.confirm(ctx, p, sources, counts, id);
        }
        let mut action = None;
        let mut remove = false;
        let selected = self
            .selected
            .filter(|id| sources.iter().any(|s| s.id == *id));
        let close = widgets::dialog(
            ctx,
            p,
            "sources-dialog",
            "Sources",
            760.0,
            true,
            |ui| self.table(ui, p, sources, counts),
            |ui| {
                let can = selected.is_some() && !busy;
                if widgets::primary(ui, p, Some(Icon::Refresh), "Rescan", can).clicked() {
                    action = selected.map(SourcesAction::Rescan);
                }
                if widgets::secondary(ui, p, None, "Change folders…", can).clicked() {
                    action = selected.map(SourcesAction::ChangeFolders);
                }
                remove = widgets::danger(ui, p, "Remove…", can).clicked();
                ui.with_layout(Layout::left_to_right(Align::Center), |ui| {
                    if widgets::secondary(ui, p, Some(Icon::FolderPlus), "Add folder…", !busy)
                        .clicked()
                    {
                        action = Some(SourcesAction::AddFolder);
                    }
                    if busy {
                        ui.label(
                            RichText::new("An import is running.")
                                .small()
                                .color(p.text_weak),
                        );
                    }
                });
            },
        );
        if remove {
            self.confirm_remove = selected;
        }
        if close && action.is_none() {
            action = Some(SourcesAction::Close);
        }
        action
    }

    fn table(
        &mut self,
        ui: &mut Ui,
        p: &Palette,
        sources: &[Source],
        counts: &HashMap<i64, Counts>,
    ) {
        if sources.is_empty() {
            ui.label(
                RichText::new("No sources yet. Add a folder of photos to start.")
                    .color(p.text_weak),
            );
            return;
        }
        Grid::new("sources-table")
            .num_columns(6)
            .striped(true)
            .spacing([16.0, 6.0])
            .min_col_width(60.0)
            .show(ui, |ui| {
                for h in [
                    "Source",
                    "Photos",
                    "On the map",
                    "Missing",
                    "Last scan",
                    "Status",
                ] {
                    ui.label(RichText::new(h).small().color(p.text_weak));
                }
                ui.end_row();
                for s in sources {
                    let c = counts.get(&s.id).copied().unwrap_or_default();
                    let selected = self.selected == Some(s.id);
                    let cell = ui.vertical(|ui| {
                        let name = RichText::new(&s.label).color(if selected {
                            p.accent
                        } else {
                            p.text_strong
                        });
                        ui.label(name);
                        ui.label(
                            RichText::new(s.root_path.to_string_lossy())
                                .monospace()
                                .small()
                                .color(p.text_weak),
                        );
                    });
                    let row = egui::Id::unique(("source-row", s.id));
                    if ui
                        .interact(cell.response.rect, row, Sense::click())
                        .clicked()
                    {
                        self.selected = Some(s.id);
                    }
                    for n in [c.photos, c.with_position, c.missing] {
                        ui.label(thousands(n as usize));
                    }
                    ui.label(s.last_scan_at.as_deref().map_or("never".to_owned(), day));
                    match self.online.get(&s.id) {
                        Some(true) => widgets::chip(ui, p, Tone::Success, "Online"),
                        Some(false) => widgets::chip(ui, p, Tone::Offline, "Offline"),
                        None => {
                            ui.label(RichText::new("checking…").small().color(p.text_weak));
                        }
                    }
                    ui.end_row();
                }
            });
        ui.add_space(p.space_2);
        ui.label(
            RichText::new(
                "Rescan finds new, changed, moved and missing photos. Only new and changed ones \
                 are read, so a rescan of unchanged folders takes seconds. Missing photos leave \
                 the map; their records stay until you remove them.",
            )
            .small()
            .color(p.text_weak),
        );
    }

    fn confirm(
        &mut self,
        ctx: &egui::Context,
        p: &Palette,
        sources: &[Source],
        counts: &HashMap<i64, Counts>,
        id: i64,
    ) -> Option<SourcesAction> {
        let Some(s) = sources.iter().find(|s| s.id == id) else {
            self.confirm_remove = None;
            return None;
        };
        let photos = counts.get(&id).map_or(0, |c| c.photos) as usize;
        let mut action = None;
        let mut keep = false;
        let close = widgets::dialog(
            ctx,
            p,
            "remove-source",
            &format!("Remove “{}”?", s.label),
            420.0,
            false,
            |ui| {
                ui.horizontal_top(|ui| {
                    ui.add(Icon::TriangleAlert.image(p.size_icon_md, p.status_warning));
                    ui.vertical(|ui| {
                        ui.label(
                            RichText::new(format!(
                                "Its {} photos leave MapCraft's library and the map.",
                                thousands(photos)
                            ))
                            .color(p.text_strong),
                        );
                        ui.label(
                            RichText::new(format!(
                                "Your files in {} are not touched. You can add the folder again later.",
                                s.root_path.display()
                            ))
                            .color(p.text_weak),
                        );
                    });
                });
            },
            |ui| {
                if widgets::danger(ui, p, "Remove source", true).clicked() {
                    action = Some(SourcesAction::Remove(id));
                }
                let k = widgets::secondary(ui, p, None, "Keep source", true);
                if !ui.memory(|m| m.focused().is_some()) {
                    k.request_focus(); // the safe button has the default focus
                }
                keep = k.clicked();
            },
        );
        if keep || close || action.is_some() {
            self.confirm_remove = None;
        }
        action
    }
}

/// `2026-11-14T18:42:00Z` → `14 Nov 2026` (design system: dates in lists).
pub fn day(iso: &str) -> String {
    const MONTHS: [&str; 12] = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let parts: Option<(u32, usize, u32)> = (|| {
        Some((
            iso.get(0..4)?.parse().ok()?,
            iso.get(5..7)?.parse::<usize>().ok()?,
            iso.get(8..10)?.parse().ok()?,
        ))
    })();
    match parts {
        Some((y, m, d)) if (1..=12).contains(&m) => format!("{d} {} {y}", MONTHS[m - 1]),
        _ => iso.to_owned(),
    }
}

#[cfg(test)]
mod tests {
    use super::day;

    #[test]
    fn dates_read_like_the_design_system() {
        assert_eq!(day("2026-11-14T18:42:00Z"), "14 Nov 2026");
        assert_eq!(day("2026-01-05T00:00:00Z"), "5 Jan 2026");
        assert_eq!(day("garbage"), "garbage");
    }
}

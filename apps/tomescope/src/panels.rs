//! Auxiliary reader panels: table of contents, bookmarks, annotations
//! list, and a figure/image gallery for quickly jumping between
//! illustrations or diagrams in a textbook.

use crate::content::{BookContent, Locus, TocEntry};
use eframe::egui::{self, RichText, ScrollArea};
use uikit_egui::PaletteColors;

/// Draw a hierarchical table of contents. Returns `Some(locus)` if an
/// entry was clicked, so the caller can navigate the reader there.
pub fn draw_toc(
    ui: &mut egui::Ui,
    toc: &[TocEntry],
    current_block: usize,
    colors: &PaletteColors,
) -> Option<Locus> {
    let mut jump = None;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if toc.is_empty() {
            ui.colored_label(colors.text_weak, "No table of contents available.");
            return;
        }
        for entry in toc {
            let indent = entry.depth as f32 * 14.0;
            ui.horizontal(|ui| {
                ui.add_space(indent);
                let is_current = entry.block <= current_block
                    && toc
                        .iter()
                        .filter(|e| e.block <= current_block)
                        .last()
                        .map(|e| e.block == entry.block)
                        .unwrap_or(false);
                let text = if is_current {
                    RichText::new(&entry.title).color(colors.accent).strong()
                } else {
                    RichText::new(&entry.title).color(colors.text)
                };
                if ui.add(egui::Label::new(text).sense(egui::Sense::click())).clicked() {
                    jump = Some(Locus {
                        block: entry.block,
                        ch: 0,
                    });
                }
            });
        }
    });
    jump
}

/// A saved reading position with a user-facing label, distinct from
/// highlights/annotations (those live in `study.rs`).
#[derive(Clone, Debug, serde::Serialize, serde::Deserialize)]
pub struct Bookmark {
    pub locus: Locus,
    pub label: String,
    pub created_unix: i64,
}

pub fn draw_bookmarks(
    ui: &mut egui::Ui,
    bookmarks: &[Bookmark],
    colors: &PaletteColors,
) -> BookmarkAction {
    let mut action = BookmarkAction::None;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if bookmarks.is_empty() {
            ui.colored_label(colors.text_weak, "No bookmarks yet. Tap the ribbon icon while reading to add one.");
            return;
        }
        for (i, bm) in bookmarks.iter().enumerate() {
            ui.horizontal(|ui| {
                let label = if bm.label.is_empty() {
                    format!("Block {}", bm.locus.block)
                } else {
                    bm.label.clone()
                };
                if ui
                    .add(egui::Label::new(RichText::new(label).color(colors.text)).sense(egui::Sense::click()))
                    .clicked()
                {
                    action = BookmarkAction::Jump(bm.locus);
                }
                ui.with_layout(egui::Layout::right_to_left(egui::Align::Center), |ui| {
                    if ui.small_button("✕").clicked() {
                        action = BookmarkAction::Remove(i);
                    }
                });
            });
            ui.colored_label(colors.text_weak, crate::util::relative_time(bm.created_unix));
            ui.separator();
        }
    });
    action
}

pub enum BookmarkAction {
    None,
    Jump(Locus),
    Remove(usize),
}

/// Draw the figure/image gallery: a grid of thumbnails for every image in
/// the book, letting the reader jump straight to a diagram or plate.
pub fn draw_figure_gallery(
    ui: &mut egui::Ui,
    content: &BookContent,
    thumb: impl Fn(&mut egui::Ui, usize, egui::Vec2),
    colors: &PaletteColors,
) -> Option<usize> {
    let mut jump = None;
    if content.images.is_empty() {
        ui.colored_label(colors.text_weak, "This book has no images.");
        return None;
    }
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        let thumb_size = egui::vec2(120.0, 160.0);
        egui::Grid::new("figure_gallery_grid")
            .spacing(egui::vec2(10.0, 10.0))
            .show(ui, |ui| {
                let mut col = 0;
                let cols = (ui.available_width() / (thumb_size.x + 10.0)).floor().max(1.0) as usize;
                for (idx, img) in content.images.iter().enumerate() {
                    ui.vertical(|ui| {
                        thumb(ui, idx, thumb_size);
                        let name = if img.name.is_empty() {
                            format!("Figure {idx}")
                        } else {
                            img.name.clone()
                        };
                        ui.colored_label(colors.text_weak, crate::util::truncate_chars(&name, 18));
                        if ui.button("Go to").clicked() {
                            jump = Some(idx);
                        }
                    });
                    col += 1;
                    if col >= cols {
                        col = 0;
                        ui.end_row();
                    }
                }
            });
    });
    jump
}

/// Draw the footnotes-and-endnotes index panel, distinct from the inline
/// popover in `reader.rs` — this is a scrollable full list for reference.
pub fn draw_footnotes_index(ui: &mut egui::Ui, content: &BookContent, colors: &PaletteColors) -> Option<usize> {
    let mut jump = None;
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        if content.footnotes.is_empty() {
            ui.colored_label(colors.text_weak, "No footnotes in this book.");
            return;
        }
        for (i, fnote) in content.footnotes.iter().enumerate() {
            ui.horizontal(|ui| {
                ui.colored_label(colors.accent, format!("[{}]", fnote.label));
                let text = crate::content::join_spans(&fnote.spans);
                if ui
                    .add(egui::Label::new(RichText::new(text).color(colors.text)).sense(egui::Sense::click()))
                    .clicked()
                {
                    jump = Some(i);
                }
            });
            ui.add_space(4.0);
        }
    });
    jump
}

/// Draw the book "info" panel: metadata card shown from the reader menu.
pub fn draw_book_info(
    ui: &mut egui::Ui,
    title: &str,
    authors: &[String],
    description: &str,
    publisher: &Option<String>,
    year: &Option<i32>,
    isbn: &Option<String>,
    colors: &PaletteColors,
) {
    ScrollArea::vertical().auto_shrink([false, false]).show(ui, |ui| {
        ui.heading(RichText::new(title).color(colors.text));
        if !authors.is_empty() {
            ui.colored_label(colors.text_weak, authors.join(", "));
        }
        ui.add_space(8.0);
        if let Some(p) = publisher {
            ui.label(format!("Publisher: {p}"));
        }
        if let Some(y) = year {
            ui.label(format!("Year: {y}"));
        }
        if let Some(i) = isbn {
            ui.label(format!("ISBN: {i}"));
        }
        if !description.is_empty() {
            ui.add_space(10.0);
            ui.separator();
            ui.add_space(6.0);
            ui.label(description);
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::content::TocEntry;

    #[test]
    fn toc_entries_preserve_order() {
        let toc = vec![
            TocEntry { title: "Ch 1".into(), depth: 0, block: 0 },
            TocEntry { title: "1.1".into(), depth: 1, block: 3 },
            TocEntry { title: "Ch 2".into(), depth: 0, block: 10 },
        ];
        assert_eq!(toc[0].depth, 0);
        assert_eq!(toc[1].depth, 1);
        assert!(toc[2].block > toc[0].block);
    }
}

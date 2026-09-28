//! grid — the library as days of thumbnails.
//!
//! Three across on a phone, as many 160-pixel cells as fit on a desktop,
//! days newest first with a date heading and a count. Cells are square and
//! the thumbnail is CROPPED to fill them (centre crop) rather than fitted:
//! a grid of letterboxed rectangles reads as a contact sheet of gaps, and
//! the viewer is one tap away for the whole picture.
//!
//! Only cells inside the viewport are painted or uploaded, which is what
//! keeps a 2,000-photo library's texture use at "what is on screen"
//! rather than "everything ever scrolled past".

use crate::library::{DayGroup, Library};
use crate::textures::Textures;

/// The gap between cells, in points.
pub const GAP: f32 = 4.0;
/// Desktop cells try for this size; the count is whatever fits.
pub const DESKTOP_CELL: f32 = 160.0;
/// A phone always gets this many, whatever its width.
pub const PHONE_COLS: usize = 3;

/// How many cells across.
pub fn columns(width: f32, compact: bool) -> usize {
    if compact { PHONE_COLS } else { ((width + GAP) / (DESKTOP_CELL + GAP)).floor().max(PHONE_COLS as f32) as usize }
}

/// The side of a square cell for a column count.
pub fn cell_side(width: f32, cols: usize) -> f32 {
    ((width - GAP * (cols as f32 - 1.0)) / cols as f32).max(24.0)
}

/// The rows a day needs.
pub fn rows(n: usize, cols: usize) -> usize { n.div_ceil(cols.max(1)) }

/// Centre-crop UV for a `w × h` image in a square cell.
pub fn cover_uv(w: usize, h: usize) -> egui::Rect {
    if w == 0 || h == 0 { return egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)); }
    if w > h {
        let f = h as f32 / w as f32;
        let x0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x0 + f, 1.0))
    } else {
        let f = w as f32 / h as f32;
        let y0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, y0), egui::pos2(1.0, y0 + f))
    }
}

/// What a tap on the grid asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridAction {
    /// Open this photo (an index into the library).
    Open(usize),
}

/// Draw the days. `days` is what `Library::days` returned for the current
/// search; the caller keeps it between frames rather than regrouping on
/// every one.
pub fn grid_ui(ui: &mut egui::Ui, lib: &mut Library, days: &[DayGroup], textures: &mut Textures,
               compact: bool, c: &uikit_egui::PaletteColors) -> Option<GridAction> {
    let mut out = None;
    if days.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.colored_label(c.text_weak, if lib.is_empty() { "No photos yet." } else { "Nothing matches." });
        });
        return None;
    }
    let width = ui.available_width();
    let cols = columns(width, compact);
    let side = cell_side(width, cols);
    egui::ScrollArea::vertical().id_salt("photo-grid").auto_shrink([false, false]).show(ui, |ui| {
        for day in days {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.colored_label(c.text, egui::RichText::new(&day.label).strong());
                ui.colored_label(c.text_weak, egui::RichText::new(format!("{} photo{}", day.photos.len(),
                    if day.photos.len() == 1 { "" } else { "s" })).small());
            });
            ui.add_space(2.0);
            for row in day.photos.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for &i in row {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
                        if ui.is_rect_visible(rect) {
                            let tex = lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p));
                            match tex {
                                Some(t) => {
                                    let [w, h] = t.size();
                                    ui.painter().image(t.id(), rect, cover_uv(w, h), egui::Color32::WHITE);
                                }
                                None => {
                                    ui.painter().rect_filled(rect, 3.0, c.bg_raised);
                                    if let Some(p) = lib.photos.get(i) {
                                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                            p.name().chars().next().unwrap_or('·').to_ascii_uppercase(),
                                            egui::FontId::proportional(side * 0.3), c.text_weak);
                                    }
                                }
                            }
                            if resp.hovered() {
                                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, c.accent), egui::StrokeKind::Inside);
                            }
                        }
                        if resp.clicked() { out = Some(GridAction::Open(i)); }
                        if let Some(p) = lib.photos.get(i) {
                            resp.on_hover_text(format!("{}\n{}×{}", p.name(), p.w, p.h));
                        }
                    }
                });
                ui.add_space(GAP);
            }
        }
        ui.add_space(12.0);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_gets_three_columns_whatever_its_width() {
        assert_eq!(columns(320.0, true), 3);
        assert_eq!(columns(480.0, true), 3);
    }

    #[test]
    fn a_desktop_gets_as_many_cells_as_fit_never_fewer_than_three() {
        assert_eq!(columns(1000.0, false), 6);
        assert_eq!(columns(400.0, false), 3, "narrow desktop windows do not go below the phone count");
        let side = cell_side(1000.0, 6);
        assert!((side * 6.0 + GAP * 5.0 - 1000.0).abs() < 0.01, "cells and gaps fill the width exactly");
    }

    #[test]
    fn rows_round_up() {
        assert_eq!(rows(0, 3), 0);
        assert_eq!(rows(3, 3), 1);
        assert_eq!(rows(4, 3), 2);
    }

    #[test]
    fn the_cover_crop_keeps_the_middle_of_a_wide_or_tall_picture() {
        let wide = cover_uv(400, 200);
        assert!((wide.min.x - 0.25).abs() < 1e-6 && (wide.max.x - 0.75).abs() < 1e-6);
        assert_eq!(wide.min.y, 0.0);
        let tall = cover_uv(200, 400);
        assert!((tall.min.y - 0.25).abs() < 1e-6 && (tall.max.y - 0.75).abs() < 1e-6);
        let square = cover_uv(100, 100);
        assert_eq!(square.min, egui::pos2(0.0, 0.0));
        assert_eq!(square.max, egui::pos2(1.0, 1.0));
    }
}
//! grid — the library as days of thumbnails.
//!
//! Three across on a phone, as many 160-pixel cells as fit on a desktop,
//! days newest first with a date heading and a count. Cells are square and
//! the thumbnail is CROPPED to fill them (centre crop) rather than fitted:
//! a grid of letterboxed rectangles reads as a contact sheet of gaps, and
//! the viewer is one tap away for the whole picture.
//!
//! Only cells inside the viewport are painted or uploaded, which is what
//! keeps a 2,000-photo library's texture use at "what is on screen"
//! rather than "everything ever scrolled past".

use crate::library::{DayGroup, Library};
use crate::textures::Textures;

/// The gap between cells, in points.
pub const GAP: f32 = 4.0;
/// Desktop cells try for this size; the count is whatever fits.
pub const DESKTOP_CELL: f32 = 160.0;
/// A phone always gets this many, whatever its width.
pub const PHONE_COLS: usize = 3;

/// How many cells across.
pub fn columns(width: f32, compact: bool) -> usize {
    if compact { PHONE_COLS } else { ((width + GAP) / (DESKTOP_CELL + GAP)).floor().max(PHONE_COLS as f32) as usize }
}

/// The side of a square cell for a column count.
pub fn cell_side(width: f32, cols: usize) -> f32 {
    ((width - GAP * (cols as f32 - 1.0)) / cols as f32).max(24.0)
}

/// The rows a day needs.
pub fn rows(n: usize, cols: usize) -> usize { n.div_ceil(cols.max(1)) }

/// Centre-crop UV for a `w × h` image in a square cell.
pub fn cover_uv(w: usize, h: usize) -> egui::Rect {
    if w == 0 || h == 0 { return egui::Rect::from_min_max(egui::pos2(0.0, 0.0), egui::pos2(1.0, 1.0)); }
    if w > h {
        let f = h as f32 / w as f32;
        let x0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(x0, 0.0), egui::pos2(x0 + f, 1.0))
    } else {
        let f = w as f32 / h as f32;
        let y0 = (1.0 - f) / 2.0;
        egui::Rect::from_min_max(egui::pos2(0.0, y0), egui::pos2(1.0, y0 + f))
    }
}

/// What a tap on the grid asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GridAction {
    /// Open this photo (an index into the library).
    Open(usize),
}

/// Draw the days. `days` is what `Library::days` returned for the current
/// search; the caller keeps it between frames rather than regrouping on
/// every one.
pub fn grid_ui(ui: &mut egui::Ui, lib: &mut Library, days: &[DayGroup], textures: &mut Textures,
               compact: bool, c: &uikit_egui::PaletteColors) -> Option<GridAction> {
    let mut out = None;
    if days.is_empty() {
        ui.add_space(24.0);
        ui.vertical_centered(|ui| {
            ui.colored_label(c.text_weak, if lib.is_empty() { "No photos yet." } else { "Nothing matches." });
        });
        return None;
    }
    let width = ui.available_width();
    let cols = columns(width, compact);
    let side = cell_side(width, cols);
    egui::ScrollArea::vertical().id_salt("photo-grid").auto_shrink([false, false]).show(ui, |ui| {
        for day in days {
            ui.add_space(6.0);
            ui.horizontal(|ui| {
                ui.colored_label(c.text, egui::RichText::new(&day.label).strong());
                ui.colored_label(c.text_weak, egui::RichText::new(format!("{} photo{}", day.photos.len(),
                    if day.photos.len() == 1 { "" } else { "s" })).small());
            });
            ui.add_space(2.0);
            for row in day.photos.chunks(cols) {
                ui.horizontal(|ui| {
                    ui.spacing_mut().item_spacing.x = GAP;
                    for &i in row {
                        let (rect, resp) = ui.allocate_exact_size(egui::vec2(side, side), egui::Sense::click());
                        if ui.is_rect_visible(rect) {
                            let tex = lib.photos.get_mut(i).and_then(|p| textures.get(ui.ctx(), p));
                            match tex {
                                Some(t) => {
                                    let [w, h] = t.size();
                                    ui.painter().image(t.id(), rect, cover_uv(w, h), egui::Color32::WHITE);
                                }
                                None => {
                                    ui.painter().rect_filled(rect, 3.0, c.bg_raised);
                                    if let Some(p) = lib.photos.get(i) {
                                        ui.painter().text(rect.center(), egui::Align2::CENTER_CENTER,
                                            p.name().chars().next().unwrap_or('·').to_ascii_uppercase(),
                                            egui::FontId::proportional(side * 0.3), c.text_weak);
                                    }
                                }
                            }
                            if resp.hovered() {
                                ui.painter().rect_stroke(rect, 3.0, egui::Stroke::new(2.0, c.accent), egui::StrokeKind::Inside);
                            }
                        }
                        if resp.clicked() { out = Some(GridAction::Open(i)); }
                        if let Some(p) = lib.photos.get(i) {
                            resp.on_hover_text(format!("{}\n{}×{}", p.name(), p.w, p.h));
                        }
                    }
                });
                ui.add_space(GAP);
            }
        }
        ui.add_space(12.0);
    });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_phone_gets_three_columns_whatever_its_width() {
        assert_eq!(columns(320.0, true), 3);
        assert_eq!(columns(480.0, true), 3);
    }

    #[test]
    fn a_desktop_gets_as_many_cells_as_fit_never_fewer_than_three() {
        assert_eq!(columns(1000.0, false), 6);
        assert_eq!(columns(400.0, false), 3, "narrow desktop windows do not go below the phone count");
        let side = cell_side(1000.0, 6);
        assert!((side * 6.0 + GAP * 5.0 - 1000.0).abs() < 0.01, "cells and gaps fill the width exactly");
    }

    #[test]
    fn rows_round_up() {
        assert_eq!(rows(0, 3), 0);
        assert_eq!(rows(3, 3), 1);
        assert_eq!(rows(4, 3), 2);
    }

    #[test]
    fn the_cover_crop_keeps_the_middle_of_a_wide_or_tall_picture() {
        let wide = cover_uv(400, 200);
        assert!((wide.min.x - 0.25).abs() < 1e-6 && (wide.max.x - 0.75).abs() < 1e-6);
        assert_eq!(wide.min.y, 0.0);
        let tall = cover_uv(200, 400);
        assert!((tall.min.y - 0.25).abs() < 1e-6 && (tall.max.y - 0.75).abs() < 1e-6);
        let square = cover_uv(100, 100);
        assert_eq!(square.min, egui::pos2(0.0, 0.0));
        assert_eq!(square.max, egui::pos2(1.0, 1.0));
    }
}

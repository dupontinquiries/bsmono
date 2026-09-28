//! The layout switcher: a narrow strip of icons beside the keys, collapsible to a sliver.
//!
//! It replaces `uikit_egui::vertical_tabs` here, which printed each name on a touch-height
//! tab. With nine boards that was a 64pt-wide column TALLER than the keyboard (nine 38-44pt
//! tabs against four rows), so the switcher, not the keys, set the height of the board's
//! row -- one source of the dead space under the keys -- and the names ran off both edges.
//!
//! Now: [`RAIL_W`] wide, one [`CELL`]-tall icon per board (from `layout_icons`), scrolling
//! when there are more boards than fit beside the keys, never taller than the keys. A
//! chevron at the top collapses it to [`RAIL_COLLAPSED_W`], giving the keys the full width;
//! the collapsed sliver is itself the expand button, the whole height of the board.

use crate::layout_icons::{self, Kind};
use egui::{Align2, Color32, CornerRadius, FontId, Rect, Sense, Ui, Vec2};

/// Expanded width, in points. Wide enough for a 20pt icon with a margin a thumb can hit.
/// HALF of what it was (40): the keys get the width back. Switching is a
/// vertical swipe on the rail now (demo_app `rail_gestures`), so the icons
/// no longer need to be comfortable tap targets on their own.
pub const RAIL_W: f32 = 20.0;
/// Collapsed width: a sliver that still reads as a handle and is still tappable.
pub const RAIL_COLLAPSED_W: f32 = 10.0;
/// One board's cell height.
pub const CELL: f32 = 36.0;
/// The collapse chevron's row.
pub const CHEVRON_H: f32 = 24.0;
/// Icon size inside a cell.
pub const ICON: f32 = 13.0;

/// One board in the rail.
#[derive(Debug, Clone)]
pub struct Entry {
    /// Full name: the tooltip, and the source of the Generic icon's initial.
    pub name: String,
    pub kind: Kind,
    /// Loaded from a layout file rather than built in: marked with a corner dot, which is
    /// what tells "QWERTY" from "English (QWERTY) from en_us_qwerty.json" at a glance.
    pub from_file: bool,
}

impl Entry {
    pub fn new(name: &str, from_file: bool) -> Entry {
        Entry { name: name.to_string(), kind: Kind::for_layout(name), from_file }
    }
}

/// The rail's width for a collapse state -- what the caller reserves beside the keys.
pub fn width(collapsed: bool) -> f32 {
    if collapsed { RAIL_COLLAPSED_W } else { RAIL_W }
}

/// Draw the rail into `ui` (which the caller has sized to [`width`] x the board height).
/// Returns the index the user tapped, if any. `collapsed` is toggled in place.
pub fn show(
    ui: &mut Ui,
    entries: &[Entry],
    selected: usize,
    collapsed: &mut bool,
    colors: &uikit_egui::PaletteColors,
) -> Option<usize> {
    let height = ui.available_height();
    if *collapsed {
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(RAIL_COLLAPSED_W, height), Sense::click());
        let fill = if resp.hovered() { colors.bg_hover } else { colors.bg_raised };
        ui.painter().rect_filled(rect.shrink(1.0), CornerRadius::same(4), fill);
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(RAIL_COLLAPSED_W - 2.0));
        layout_icons::paint(ui.painter(), layout_icons::chevron(true), icon_rect, colors.text.gamma_multiply(0.7));
        if resp.on_hover_text("show keyboards").clicked() {
            *collapsed = false;
        }
        return None;
    }

    let mut picked = None;
    ui.vertical(|ui| {
        ui.spacing_mut().item_spacing = Vec2::ZERO;
        let (rect, resp) = ui.allocate_exact_size(Vec2::new(RAIL_W, CHEVRON_H), Sense::click());
        if resp.hovered() {
            ui.painter().rect_filled(rect.shrink(2.0), CornerRadius::same(4), colors.bg_hover);
        }
        let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(CHEVRON_H - 8.0));
        layout_icons::paint(ui.painter(), layout_icons::chevron(false), icon_rect, colors.text.gamma_multiply(0.7));
        if resp.on_hover_text("hide the switcher (full-width keys)").clicked() {
            *collapsed = true;
        }
        egui::ScrollArea::vertical()
            .id_salt("rusty_board_layout_rail")
            .max_height((height - CHEVRON_H).max(CELL))
            .auto_shrink([false, true])
            .scroll_bar_visibility(egui::scroll_area::ScrollBarVisibility::AlwaysHidden)
            // A vertical drag on the rail SWITCHES boards now; letting the
            // list scroll under the same finger would fight it.
            .drag_to_scroll(false)
            .show(ui, |ui| {
                ui.spacing_mut().item_spacing = Vec2::ZERO;
                for (i, e) in entries.iter().enumerate() {
                    if cell(ui, e, i == selected, colors).clicked() {
                        picked = Some(i);
                    }
                }
            });
    });
    picked
}

/// One board's cell: icon, selection, file mark, tooltip.
fn cell(ui: &mut Ui, e: &Entry, selected: bool, colors: &uikit_egui::PaletteColors) -> egui::Response {
    let (rect, resp) = ui.allocate_exact_size(Vec2::new(RAIL_W, CELL), Sense::click());
    let p = ui.painter();
    let tile = rect.shrink2(Vec2::new(1.5, 2.0));
    if selected {
        p.rect_filled(tile, CornerRadius::same(6), uikit_egui::glass_tint(colors.accent, 72));
    } else if resp.hovered() {
        p.rect_filled(tile, CornerRadius::same(6), colors.bg_hover);
    }
    let ink = if selected { colors.text } else { colors.text.gamma_multiply(0.75) };
    let icon_rect = Rect::from_center_size(rect.center(), Vec2::splat(ICON));
    layout_icons::paint(p, layout_icons::icon(e.kind), icon_rect, ink);
    if e.kind == Kind::Generic {
        let initial: String = e.name.chars().find(|c| c.is_alphanumeric()).into_iter().collect();
        p.text(icon_rect.center(), Align2::CENTER_CENTER, initial, FontId::proportional(11.0), ink);
    }
    if e.from_file {
        p.circle_filled(tile.right_bottom() - Vec2::new(4.0, 4.0), 2.5, dot(colors));
    }
    resp.on_hover_text(&e.name)
}

fn dot(colors: &uikit_egui::PaletteColors) -> Color32 {
    colors.accent
}

#[cfg(test)]
mod tests {
    use super::*;

    fn draw(entries: &[Entry], collapsed: bool, h: f32) -> (egui::FullOutput, Rect) {
        let ctx = egui::Context::default();
        let mut c = collapsed;
        let mut used = Rect::NOTHING;
        let input = egui::RawInput {
            screen_rect: Some(Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 400.0))),
            ..Default::default()
        };
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                ui.allocate_ui(Vec2::new(width(c), h), |ui| {
                    let colors = uikit_egui::palette_colors("system");
                    let _ = show(ui, entries, 0, &mut c, &colors);
                    used = ui.min_rect();
                });
            });
        });
        (out, used)
    }

    fn nine() -> Vec<Entry> {
        ["en_us_qwerty", "symbols", "cardinal", "cardinal8", "Stenotype"]
            .iter()
            .map(|n| Entry::new(n, false))
            .chain(
                ["English (QWERTY)", "English (Dvorak — Ortholinear)", "English (Cardinal Grid)", "Stenotype"]
                    .iter()
                    .map(|n| Entry::new(n, true)),
            )
            .collect()
    }

    /// THE HEIGHT BUG. Nine boards must not make the rail taller than the keys beside it --
    /// that is what let the switcher, not the keyboard, set the board's height.
    #[test]
    fn nine_boards_fit_beside_a_four_row_board() {
        let board_h = 4.0 * (crate::KEY_H + crate::KEY_GAP);
        let (_, used) = draw(&nine(), false, board_h);
        assert!(used.height() <= board_h + 0.5, "rail {} tall beside a {board_h} board", used.height());
        assert!((used.width() - RAIL_W).abs() < 0.5, "rail width {}", used.width());
        assert!(RAIL_W < 64.0, "narrower than the 64pt tab column it replaces");
    }

    /// Icons, not names: no board name is printed (the names are tooltips). The Generic
    /// icon prints one initial and nothing longer.
    #[test]
    fn the_rail_prints_no_names() {
        let mut entries = nine();
        entries.push(Entry::new("Klingon", true));
        let (out, _) = draw(&entries, false, 600.0);
        let texts = uikit_egui::layout_probe::texts(&out.shapes);
        for t in &texts {
            assert!(t.text.chars().count() <= 1, "a name was printed: {:?}", t.text);
        }
        assert!(texts.iter().any(|t| t.text == "K"), "the generic board shows its initial");
    }

    /// Collapsed, the rail is a sliver the full height of the board.
    #[test]
    fn collapsed_is_a_full_height_sliver() {
        let (_, used) = draw(&nine(), true, 200.0);
        assert!((used.width() - RAIL_COLLAPSED_W).abs() < 0.5);
        assert!((used.height() - 200.0).abs() < 0.5, "the expand handle should span the board");
    }
}

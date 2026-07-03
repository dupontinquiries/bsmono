//! rusty-board-egui — the on-screen keyboard, ported to egui on uikit-egui + adaptive.
//!
//! The GTK4 UI (`src/ui/*`) becomes this: a keyboard rendered with egui, sized by the
//! [`Adaptive`] layer (touch targets grow on the FP5, tighten on desktop) and themed from
//! snapclone-theme. rusty-board's `layouts`, `gestures`, and `keyboard/engine.rs` are
//! already GTK-free, so on-device this front-end consumes the real `layouts::Layout` and
//! feeds taps/swipes into the engine. Here it carries a small standalone [`Layout`] so the
//! crate builds and can be checked against egui 0.29 on its own.

use egui::{Align2, FontId, Rect, Rounding, Sense, Stroke, Ui, Vec2};
use uikit_egui::{apply_palette, Adaptive, PaletteColors};

/// A single key: what it shows and its relative width within the row.
#[derive(Debug, Clone)]
pub struct Key {
    pub label: String,
    pub weight: f32,
    /// The string emitted on tap (defaults to the label).
    pub emit: String,
}

impl Key {
    pub fn new(label: &str) -> Key {
        Key { label: label.to_string(), weight: 1.0, emit: label.to_string() }
    }
    pub fn wide(label: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: label.to_string() }
    }
    pub fn emitting(label: &str, emit: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: emit.to_string() }
    }
}

/// A keyboard row and its relative height.
#[derive(Debug, Clone)]
pub struct Row {
    pub keys: Vec<Key>,
    pub weight: f32,
}

/// A minimal keyboard layout (a faithful subset of rusty-board's `layouts::Layout`).
#[derive(Debug, Clone)]
pub struct Layout {
    pub name: String,
    pub rows: Vec<Row>,
}

impl Layout {
    /// A compact QWERTY, mirroring rusty-board's `default_qwerty` shape.
    pub fn qwerty() -> Layout {
        fn row(keys: &[&str]) -> Row {
            Row { keys: keys.iter().map(|k| Key::new(k)).collect(), weight: 1.0 }
        }
        Layout {
            name: "qwerty".into(),
            rows: vec![
                row(&["q", "w", "e", "r", "t", "y", "u", "i", "o", "p"]),
                row(&["a", "s", "d", "f", "g", "h", "j", "k", "l"]),
                Row {
                    keys: vec![
                        Key::wide("⇧", 1.5),
                        Key::new("z"),
                        Key::new("x"),
                        Key::new("c"),
                        Key::new("v"),
                        Key::new("b"),
                        Key::new("n"),
                        Key::new("m"),
                        Key::emitting("⌫", "\u{8}", 1.5),
                    ],
                    weight: 1.0,
                },
                Row {
                    keys: vec![
                        Key::emitting("space", " ", 6.0),
                        Key::emitting("⏎", "\n", 2.0),
                    ],
                    weight: 1.0,
                },
            ],
        }
    }
}

/// Apply the shared palette to the egui context (call once at startup).
pub fn apply_theme(ctx: &egui::Context) {
    let palette = snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]);
    apply_palette(ctx, palette);
}

/// Render the keyboard into `ui`, sized by `adaptive`, and return the `emit` string of a
/// tapped key (if any). The whole surface adapts: key height is the adaptive touch target
/// (comfortably large on a phone), spacing is the adaptive gutter, and the board fills the
/// available width with per-key widths proportional to their weight.
pub fn keyboard_ui(ui: &mut Ui, layout: &Layout, adaptive: &Adaptive) -> Option<String> {
    let colors = PaletteColors::from_palette(snapclone_theme::palette_for(snapclone_theme::PaletteId::all()[0]));
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target; // 44pt touch / 28pt pointer, from the adaptive layer
    let total_w = ui.available_width();

    let mut emitted = None;
    for row in &layout.rows {
        let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
        let usable = total_w - gutter * (row.keys.len() as f32 + 1.0);
        ui.horizontal(|ui| {
            ui.add_space(gutter);
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                if paint_key(ui, key, Vec2::new(w, key_h), gutter, &colors) {
                    emitted = Some(key.emit.clone());
                }
            }
        });
        ui.add_space(gutter);
    }
    emitted
}

/// Paint one key as a carved rounded tile with a centered label; returns true on tap.
fn paint_key(ui: &mut Ui, key: &Key, size: Vec2, gutter: f32, c: &PaletteColors) -> bool {
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click());
    let painter = ui.painter();
    let fill = if resp.is_pointer_button_down_on() {
        c.accent
    } else if resp.hovered() {
        c.bg_hover
    } else {
        c.bg_raised
    };
    let rounding = Rounding::same(size.y * 0.18);
    painter.rect_filled(rect, rounding, fill);
    painter.rect_stroke(rect, rounding, Stroke::new(1.0, c.bg_base));
    let text_color = if resp.is_pointer_button_down_on() { c.bg_base } else { c.text };
    painter.text(
        rect.center(),
        Align2::CENTER_CENTER,
        &key.label,
        FontId::proportional((size.y * 0.42).clamp(11.0, 26.0)),
        text_color,
    );
    let _ = gutter;
    resp.clicked()
}

/// Convenience for tests / headless: which key (emit) a tap at `pos` would hit, given a
/// laid-out board of `board` rect. Pure geometry, no egui context needed — lets the layout
/// math be unit-tested without a live UI.
pub fn hit_test(layout: &Layout, board: Rect, adaptive: &Adaptive, pos: egui::Pos2) -> Option<String> {
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target;
    let mut y = board.top();
    for row in &layout.rows {
        let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
        let usable = board.width() - gutter * (row.keys.len() as f32 + 1.0);
        let mut x = board.left() + gutter;
        let row_rect = Rect::from_min_size(egui::pos2(board.left(), y), Vec2::new(board.width(), key_h));
        if row_rect.contains(pos) {
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                let krect = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, key_h));
                if krect.contains(pos) {
                    return Some(key.emit.clone());
                }
                x += w;
            }
        }
        y += key_h + gutter;
    }
    None
}

/// Accessibility: a stable id for the key at (row, col), so voice focus survives re-layout.
pub fn key_id(row: usize, col: usize) -> u64 {
    ((row as u64) << 8) | (col as u64)
}

/// The emit string for a key id (inverse of [`key_id`]).
pub fn emit_for_id(layout: &Layout, id: u64) -> Option<String> {
    let r = (id >> 8) as usize;
    let c = (id & 0xff) as usize;
    layout.rows.get(r).and_then(|row| row.keys.get(c)).map(|k| k.emit.clone())
}

/// Register every key with an [`A11yScene`] so voice can navigate the keyboard. Call each
/// frame (before/with rendering). Keys become focusable buttons labelled by their glyph.
pub fn register_a11y(scene: &mut uikit_egui::A11yScene, layout: &Layout) {
    scene.begin_frame();
    for (r, row) in layout.rows.iter().enumerate() {
        for (c, key) in row.keys.iter().enumerate() {
            scene.add(key_id(r, c), uikit_egui::Semantics::button(&key.label), None);
        }
    }
}

/// Apply a voice accessibility action to the keyboard. On "activate" it returns the `emit`
/// string of the focused key (the app types it); navigation/describe return `None` here (the
/// scene speaks). This makes the ported keyboard fully voice-navigable via voiced.
pub fn voice_key(scene: &mut uikit_egui::A11yScene, layout: &Layout, action: uikit_egui::A11yAction) -> Option<String> {
    match scene.apply(action) {
        uikit_egui::A11yOutcome::Activate(id) => emit_for_id(layout, id),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use uikit_egui::DeviceClass;

    #[test]
    fn qwerty_shape() {
        let l = Layout::qwerty();
        assert_eq!(l.rows.len(), 4);
        assert_eq!(l.rows[0].keys.len(), 10);
        // backspace emits the control char, not its glyph
        let bs = l.rows[2].keys.iter().find(|k| k.label == "⌫").unwrap();
        assert_eq!(bs.emit, "\u{8}");
    }

    #[test]
    fn phone_keys_are_taller_than_desktop() {
        let phone = Adaptive { class: DeviceClass::Phone, touch: true, min_touch_target: 44.0, ..Adaptive::default() };
        let desktop = Adaptive { class: DeviceClass::Desktop, ..Adaptive::default() };
        assert!(phone.min_touch_target > desktop.min_touch_target, "touch targets grow on phone");
    }

    #[test]
    fn hit_test_maps_position_to_key() {
        let l = Layout::qwerty();
        let a = Adaptive { min_touch_target: 40.0, gutter: 4.0, ..Adaptive::default() };
        let board = Rect::from_min_size(egui::pos2(0.0, 0.0), Vec2::new(400.0, 200.0));
        // top-left of the first row should land on 'q'
        let hit = hit_test(&l, board, &a, egui::pos2(6.0, 5.0));
        assert_eq!(hit.as_deref(), Some("q"));
    }

    #[test]
    fn keyboard_is_voice_navigable() {
        use uikit_egui::{A11yAction, A11yScene};
        let l = Layout::qwerty();
        let mut scene = A11yScene::new();
        register_a11y(&mut scene, &l);
        // "next" focuses the first key (q), "next" again → w
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Next), None); // focus q (nav speaks, no emit)
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Next), None); // focus w
        // "activate" types the focused key
        assert_eq!(voice_key(&mut scene, &l, A11yAction::Activate).as_deref(), Some("w"));
        // id round-trips to emit; backspace's control char comes through
        let bs_id = key_id(2, 8);
        assert_eq!(emit_for_id(&l, bs_id).as_deref(), Some("\u{8}"));
    }
}

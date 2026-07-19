//! Programmatic keyboard builder.
//!
//! [`LayoutBuilder`] is a fluent API for assembling a [`Layout`] by mixing
//! key *kinds* freely within and across rows:
//!
//! - **regular** character keys ([`LayoutBuilder::char_key`])
//! - **cardinal** gesture cells with per-direction outputs
//!   ([`LayoutBuilder::cardinal_key`] + [`CardinalCell`])
//! - **steno** cells ([`LayoutBuilder::steno_key`])
//! - **special** keys like space/backspace/shift ([`LayoutBuilder::special_key`])
//!
//! The builder is the data-model core behind the in-settings keyboard
//! builder UI: the UI manipulates a builder (or the [`Layout`] it
//! produces), then persists via [`crate::io::save_user_layout`].
//!
//! A "mixed" layout is just a [`Layout`] whose rows contain different key
//! kinds; nothing special is needed to combine them. The layout's
//! [`LayoutStyle`] decides the dispatch mode for the whole surface — set
//! it to `CardinalGrid` if the layout is primarily cardinal/steno cells,
//! or `Standard` for a normal tap/swipe-word keyboard with the odd
//! cardinal cell.

use super::{Layout, LayoutStyle, Row, TextDirection};
use crate::key::{Key, KeyAction, KeyGestures};

/// Per-direction outputs for a cardinal cell. All fields optional; unset
/// directions produce no gesture. Build with [`CardinalCell::new`] then
/// the `with_*` setters, or construct the struct directly.
#[derive(Debug, Clone, Default)]
pub struct CardinalCell {
    /// The tap label (also the character the tap emits).
    pub label: String,
    pub n: Option<String>,
    pub ne: Option<String>,
    pub e: Option<String>,
    pub se: Option<String>,
    pub s: Option<String>,
    pub sw: Option<String>,
    pub w: Option<String>,
    pub nw: Option<String>,
    pub cw: Option<String>,
    pub ccw: Option<String>,
}

impl CardinalCell {
    pub fn new(label: &str) -> Self {
        Self {
            label: label.to_string(),
            ..Default::default()
        }
    }
    pub fn n(mut self, v: &str) -> Self { self.n = Some(v.into()); self }
    pub fn ne(mut self, v: &str) -> Self { self.ne = Some(v.into()); self }
    pub fn e(mut self, v: &str) -> Self { self.e = Some(v.into()); self }
    pub fn se(mut self, v: &str) -> Self { self.se = Some(v.into()); self }
    pub fn s(mut self, v: &str) -> Self { self.s = Some(v.into()); self }
    pub fn sw(mut self, v: &str) -> Self { self.sw = Some(v.into()); self }
    pub fn w(mut self, v: &str) -> Self { self.w = Some(v.into()); self }
    pub fn nw(mut self, v: &str) -> Self { self.nw = Some(v.into()); self }
    pub fn cw(mut self, v: &str) -> Self { self.cw = Some(v.into()); self }
    pub fn ccw(mut self, v: &str) -> Self { self.ccw = Some(v.into()); self }

    fn into_gestures(self) -> KeyGestures {
        KeyGestures {
            n: self.n, ne: self.ne, e: self.e, se: self.se,
            s: self.s, sw: self.sw, w: self.w, nw: self.nw,
            cw: self.cw, ccw: self.ccw,
        }
    }
}

/// Fluent builder for a [`Layout`]. Add rows with [`row`](Self::row),
/// then push keys into the current row with the `*_key` methods.
pub struct LayoutBuilder {
    name: String,
    display_name: String,
    language: String,
    direction: TextDirection,
    grid_columns: f32,
    ortholinear: bool,
    style: LayoutStyle,
    rows: Vec<Row>,
}

impl LayoutBuilder {
    /// Start a new builder. `name` is the internal id (slugged for the
    /// filename); `display_name` is what the user sees.
    pub fn new(name: &str, display_name: &str) -> Self {
        Self {
            name: name.to_string(),
            display_name: display_name.to_string(),
            language: "en".to_string(),
            direction: TextDirection::Ltr,
            grid_columns: 10.0,
            ortholinear: true,
            style: LayoutStyle::Standard,
            rows: Vec::new(),
        }
    }

    pub fn language(mut self, lang: &str) -> Self {
        self.language = lang.to_string();
        self
    }

    pub fn direction(mut self, dir: TextDirection) -> Self {
        self.direction = dir;
        self
    }

    pub fn grid_columns(mut self, cols: f32) -> Self {
        self.grid_columns = cols;
        self
    }

    pub fn ortholinear(mut self, on: bool) -> Self {
        self.ortholinear = on;
        self
    }

    /// Set the dispatch style for the whole layout. Use `CardinalGrid`
    /// for layouts that are primarily cardinal/steno cells; `Standard`
    /// for a normal tap/swipe-word keyboard.
    pub fn style(mut self, style: LayoutStyle) -> Self {
        self.style = style;
        self
    }

    /// Begin a new (empty) row. Subsequent `*_key` calls push into it.
    pub fn row(mut self) -> Self {
        self.rows.push(Row {
            keys: Vec::new(),
            weight: 1.0,
            offset: 0.0,
        });
        self
    }

    fn push_key(&mut self, key: Key) {
        if self.rows.is_empty() {
            // Auto-start a row so callers can't push into nothing.
            self.rows.push(Row {
                keys: Vec::new(),
                weight: 1.0,
                offset: 0.0,
            });
        }
        self.rows.last_mut().unwrap().keys.push(key);
    }

    /// Add a regular character key (taps emit the character; swipe-type
    /// targets it).
    pub fn char_key(mut self, label: &str) -> Self {
        self.push_key(Key::character(label));
        self
    }

    /// Add a regular character key with an explicit width (in units).
    pub fn char_key_w(mut self, label: &str, width: f32) -> Self {
        self.push_key(Key::character(label).with_width(width));
        self
    }

    /// Add a cardinal-gesture cell. Tap emits the cell label; the
    /// configured directions emit their outputs via swipe gestures.
    pub fn cardinal_key(mut self, cell: CardinalCell) -> Self {
        let label = cell.label.clone();
        let gestures = cell.into_gestures();
        let key = Key::character(&label).with_gestures(gestures);
        self.push_key(key);
        self
    }

    /// Add a cardinal cell with an explicit width.
    pub fn cardinal_key_w(mut self, cell: CardinalCell, width: f32) -> Self {
        let label = cell.label.clone();
        let gestures = cell.into_gestures();
        let key = Key::character(&label)
            .with_gestures(gestures)
            .with_width(width);
        self.push_key(key);
        self
    }

    /// Add a steno cell. `button_id` is one of "L1","L2","R1","R2".
    pub fn steno_key(mut self, label: &str, button_id: &str) -> Self {
        let key = Key::special(label, KeyAction::Steno(button_id.to_string()));
        self.push_key(key);
        self
    }

    /// Add a special key (space, backspace, shift, return, etc.) by
    /// action, with a width.
    pub fn special_key(mut self, label: &str, action: KeyAction, width: f32) -> Self {
        self.push_key(Key::special(label, action).with_width(width));
        self
    }

    /// Set the current row's horizontal offset (for staggered,
    /// non-ortholinear layouts).
    pub fn row_offset(mut self, offset: f32) -> Self {
        if let Some(r) = self.rows.last_mut() {
            r.offset = offset;
        }
        self
    }

    /// Finish building. The result is NOT validated here; callers that
    /// persist should go through [`crate::io::save_user_layout`]
    /// which validates first.
    pub fn build(self) -> Layout {
        Layout {
            name: self.name,
            display_name: self.display_name,
            language: self.language,
            direction: self.direction,
            rows: self.rows,
            symbols_layout: None,
            ortholinear: self.ortholinear,
            grid_columns: self.grid_columns,
            style: self.style,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::io;

    #[test]
    fn builds_simple_regular_row() {
        let layout = LayoutBuilder::new("test_simple", "Test Simple")
            .row()
            .char_key("a")
            .char_key("b")
            .char_key("c")
            .build();
        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0].keys.len(), 3);
        assert_eq!(layout.rows[0].keys[0].label, "a");
        // Character keys are swipe targets.
        assert!(layout.rows[0].keys[0].swipe_target);
    }

    #[test]
    fn builds_cardinal_cell_with_directions() {
        let cell = CardinalCell::new("a").n("1").e("@").s("?").w("&");
        let layout = LayoutBuilder::new("test_card", "Test Cardinal")
            .style(LayoutStyle::CardinalGrid)
            .row()
            .cardinal_key(cell)
            .build();
        let key = &layout.rows[0].keys[0];
        assert_eq!(key.label, "a");
        let g = key.gestures.as_ref().expect("has gestures");
        assert_eq!(g.n.as_deref(), Some("1"));
        assert_eq!(g.e.as_deref(), Some("@"));
        assert_eq!(g.s.as_deref(), Some("?"));
        assert_eq!(g.w.as_deref(), Some("&"));
        assert_eq!(g.ne, None);
    }

    #[test]
    fn builds_mixed_layout() {
        // A row mixing a regular key, a cardinal cell, and a steno cell.
        let layout = LayoutBuilder::new("test_mixed", "Test Mixed")
            .row()
            .char_key("q")
            .cardinal_key(CardinalCell::new("e").n("3"))
            .steno_key("L1", "L1")
            .special_key("⌫", KeyAction::Backspace, 1.5)
            .build();
        let keys = &layout.rows[0].keys;
        assert_eq!(keys.len(), 4);
        assert_eq!(keys[0].action, KeyAction::Character);
        assert!(keys[1].gestures.is_some());
        assert!(matches!(keys[2].action, KeyAction::Steno(_)));
        assert!(matches!(keys[3].action, KeyAction::Backspace));
        assert_eq!(keys[3].width, 1.5);
    }

    #[test]
    fn auto_starts_row_if_none() {
        // Pushing a key without calling .row() first shouldn't panic.
        let layout = LayoutBuilder::new("t", "T").char_key("x").build();
        assert_eq!(layout.rows.len(), 1);
        assert_eq!(layout.rows[0].keys[0].label, "x");
    }

    #[test]
    fn widths_and_grid_columns_respected() {
        let layout = LayoutBuilder::new("t", "T")
            .grid_columns(8.0)
            .row()
            .char_key_w("a", 2.0)
            .char_key_w("b", 6.0)
            .build();
        assert_eq!(layout.grid_columns, 8.0);
        let total: f32 = layout.rows[0].keys.iter().map(|k| k.width).sum();
        assert_eq!(total, 8.0);
    }

    #[test]
    fn built_layout_validates_and_roundtrips() {
        let layout = LayoutBuilder::new("rt_test", "Roundtrip Test")
            .style(LayoutStyle::CardinalGrid)
            .row()
            .cardinal_key(CardinalCell::new("a").n("1").s("2"))
            .cardinal_key(CardinalCell::new("b").e("3"))
            .row()
            .char_key("c")
            .special_key("space", KeyAction::Space, 1.0)
            .build();
        // Validates.
        io::validate(&layout).expect("valid");
        // Round-trips through JSON.
        let json = io::export_to_string(&layout).expect("export");
        let back = io::import_from_string(&json).expect("import");
        assert_eq!(back.name, "rt_test");
        assert_eq!(back.style, LayoutStyle::CardinalGrid);
        assert_eq!(back.rows.len(), 2);
        assert_eq!(
            back.rows[0].keys[0].gestures.as_ref().unwrap().n.as_deref(),
            Some("1")
        );
    }
}

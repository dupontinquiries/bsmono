//! layout_edit — the custom-layout editor, as a model.
//!
//! Everything the companion app's layout screen does, with no UI: pick a key,
//! change what it says and does, resize it, add and remove keys and rows,
//! start from a built-in board, validate, save. The screen is a thin view over
//! this, so every edit is testable and a second front end (a CLI, the
//! in-keyboard panel) could drive the same operations.
//!
//! Works on `board_layouts::Layout`, the same JSON type the keyboard loads
//! from `board_layouts::io::user_layouts_dir()`, so a saved layout is exactly
//! what the keyboard reads: there is no second format to drift.

use board_layouts::key::{Key, KeyAction, KeyType};
use board_layouts::{Layout, Row};

/// What a key does, as a flat choice list for a picker. `needs_value` says
/// whether the choice takes a string (the text to type, the keysym name).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionChoice {
    Character,
    Text,
    Keysym,
    Backspace,
    Return,
    Space,
    Tab,
    Shift,
    SymbolsToggle,
    Voice,
    HideKeyboard,
    Settings,
    Undo,
    Spacer,
}

impl ActionChoice {
    pub const ALL: [ActionChoice; 14] = [
        ActionChoice::Character,
        ActionChoice::Text,
        ActionChoice::Keysym,
        ActionChoice::Backspace,
        ActionChoice::Return,
        ActionChoice::Space,
        ActionChoice::Tab,
        ActionChoice::Shift,
        ActionChoice::SymbolsToggle,
        ActionChoice::Voice,
        ActionChoice::HideKeyboard,
        ActionChoice::Settings,
        ActionChoice::Undo,
        ActionChoice::Spacer,
    ];

    pub fn label(self) -> &'static str {
        match self {
            ActionChoice::Character => "Type its label",
            ActionChoice::Text => "Type text…",
            ActionChoice::Keysym => "Press a key (keysym)…",
            ActionChoice::Backspace => "Backspace",
            ActionChoice::Return => "Return",
            ActionChoice::Space => "Space",
            ActionChoice::Tab => "Tab",
            ActionChoice::Shift => "Shift",
            ActionChoice::SymbolsToggle => "Symbols ↔ letters",
            ActionChoice::Voice => "Voice typing",
            ActionChoice::HideKeyboard => "Hide keyboard",
            ActionChoice::Settings => "Keyboard settings",
            ActionChoice::Undo => "Undo",
            ActionChoice::Spacer => "Spacer (does nothing)",
        }
    }

    pub fn needs_value(self) -> bool {
        matches!(self, ActionChoice::Text | ActionChoice::Keysym)
    }

    pub fn of(a: &KeyAction) -> ActionChoice {
        match a {
            KeyAction::Character => ActionChoice::Character,
            KeyAction::Literal(_) => ActionChoice::Text,
            KeyAction::Keysym(_) => ActionChoice::Keysym,
            KeyAction::Backspace => ActionChoice::Backspace,
            KeyAction::Return => ActionChoice::Return,
            KeyAction::Space => ActionChoice::Space,
            KeyAction::Tab => ActionChoice::Tab,
            KeyAction::Shift => ActionChoice::Shift,
            KeyAction::SymbolsToggle => ActionChoice::SymbolsToggle,
            KeyAction::Voice => ActionChoice::Voice,
            KeyAction::HideKeyboard => ActionChoice::HideKeyboard,
            KeyAction::Settings => ActionChoice::Settings,
            KeyAction::Undo => ActionChoice::Undo,
            KeyAction::NoOp => ActionChoice::Spacer,
            // Actions this picker does not offer (steno buttons, code modes,
            // …) read as "type its label"; `Editor::set_action` is never
            // called for them unless the user picks something else.
            _ => ActionChoice::Character,
        }
    }

    pub fn to_action(self, value: &str) -> KeyAction {
        match self {
            ActionChoice::Character => KeyAction::Character,
            ActionChoice::Text => KeyAction::Literal(value.to_string()),
            ActionChoice::Keysym => KeyAction::Keysym(value.to_string()),
            ActionChoice::Backspace => KeyAction::Backspace,
            ActionChoice::Return => KeyAction::Return,
            ActionChoice::Space => KeyAction::Space,
            ActionChoice::Tab => KeyAction::Tab,
            ActionChoice::Shift => KeyAction::Shift,
            ActionChoice::SymbolsToggle => KeyAction::SymbolsToggle,
            ActionChoice::Voice => KeyAction::Voice,
            ActionChoice::HideKeyboard => KeyAction::HideKeyboard,
            ActionChoice::Settings => KeyAction::Settings,
            ActionChoice::Undo => KeyAction::Undo,
            ActionChoice::Spacer => KeyAction::NoOp,
        }
    }

    /// The colour class a key doing this should have (drives how the keyboard paints it).
    fn key_type(self) -> KeyType {
        match self {
            ActionChoice::Character | ActionChoice::Text => KeyType::Normal,
            ActionChoice::Shift | ActionChoice::SymbolsToggle => KeyType::Modifier,
            ActionChoice::Backspace | ActionChoice::Return | ActionChoice::Tab | ActionChoice::Keysym | ActionChoice::Undo => KeyType::Action,
            ActionChoice::Space | ActionChoice::Spacer => KeyType::Normal,
            ActionChoice::Voice | ActionChoice::HideKeyboard | ActionChoice::Settings => KeyType::Special,
        }
    }
}

/// Widths a key can take, in unit keys. Stepped, not free: a keyboard built
/// from quarter units lines up; one built from 1.137 does not.
pub const WIDTH_STEP: f32 = 0.25;
pub const WIDTH_MIN: f32 = 0.25;
pub const WIDTH_MAX: f32 = 10.0;

/// A plain key with every default the layout format gives it.
pub fn new_key(label: &str) -> Key {
    serde_json::from_value(serde_json::json!({ "label": label })).expect("a label-only key always deserializes")
}

/// A new, empty-ish layout to start from: one row of five keys.
pub fn blank(display_name: &str) -> Layout {
    let rows = vec![Row { keys: ["a", "b", "c", "d", "e"].iter().map(|l| new_key(l)).collect(), weight: 1.0, offset: 0.0 }];
    let mut l: Layout = serde_json::from_value(serde_json::json!({
        "name": "x", "display_name": "x", "language": "en", "direction": "ltr", "rows": []
    }))
    .expect("a minimal layout always deserializes");
    l.rows = rows;
    l.grid_columns = 5.0;
    set_names(&mut l, display_name);
    l
}

fn set_names(l: &mut Layout, display_name: &str) {
    let display = if display_name.trim().is_empty() { "My layout" } else { display_name.trim() };
    l.display_name = display.to_string();
    l.name = format!("user-{}", board_layouts::io::slugify(display));
}

#[derive(Debug, Clone)]
pub struct Editor {
    pub layout: Layout,
    /// (row, key) of the selected key.
    pub sel: Option<(usize, usize)>,
    pub dirty: bool,
}

impl Editor {
    pub fn new(layout: Layout) -> Editor {
        Editor { layout, sel: None, dirty: false }
    }

    /// Start from a built-in board: a copy, renamed so saving it can never
    /// overwrite the original (built-ins are compiled in, but a user file of
    /// the same name would shadow it in the keyboard's list).
    pub fn from_template(template: &Layout, display_name: &str) -> Editor {
        let mut l = template.clone();
        set_names(&mut l, display_name);
        Editor { layout: l, sel: None, dirty: true }
    }

    pub fn rename(&mut self, display_name: &str) {
        set_names(&mut self.layout, display_name);
        self.dirty = true;
    }

    pub fn selected(&self) -> Option<&Key> {
        let (r, k) = self.sel?;
        self.layout.rows.get(r)?.keys.get(k)
    }

    fn selected_mut(&mut self) -> Option<&mut Key> {
        let (r, k) = self.sel?;
        self.dirty = true;
        self.layout.rows.get_mut(r)?.keys.get_mut(k)
    }

    pub fn select(&mut self, row: usize, key: usize) {
        let ok = self.layout.rows.get(row).is_some_and(|r| key < r.keys.len());
        self.sel = ok.then_some((row, key));
    }

    pub fn set_label(&mut self, label: &str) {
        if let Some(k) = self.selected_mut() {
            k.label = label.to_string();
        }
    }

    /// Empty means "the label, upper-cased" (the format's default).
    pub fn set_shifted(&mut self, shifted: &str) {
        if let Some(k) = self.selected_mut() {
            k.shifted = (!shifted.trim().is_empty()).then(|| shifted.to_string());
        }
    }

    /// Long-press variants as typed: space-separated ("é è ê ë").
    pub fn set_extended(&mut self, text: &str) {
        if let Some(k) = self.selected_mut() {
            k.extended = text.split_whitespace().map(str::to_string).collect();
        }
    }

    pub fn extended_text(&self) -> String {
        self.selected().map(|k| k.extended.join(" ")).unwrap_or_default()
    }

    pub fn set_width(&mut self, w: f32) {
        if let Some(k) = self.selected_mut() {
            let stepped = (w / WIDTH_STEP).round() * WIDTH_STEP;
            k.width = stepped.clamp(WIDTH_MIN, WIDTH_MAX);
        }
    }

    pub fn set_action(&mut self, choice: ActionChoice, value: &str) {
        if let Some(k) = self.selected_mut() {
            k.action = choice.to_action(value);
            k.key_type = choice.key_type();
            // Only a key that types a letter should be swiped through; a
            // swipe path crossing Backspace must not "type" it.
            k.swipe_target = matches!(choice, ActionChoice::Character);
        }
    }

    /// The string a Text/Keysym action carries (for the value box).
    pub fn action_value(&self) -> String {
        match self.selected().map(|k| &k.action) {
            Some(KeyAction::Literal(s)) | Some(KeyAction::Keysym(s)) => s.clone(),
            _ => String::new(),
        }
    }

    /// A new key right after the selection (or at the end of row 0), selected.
    pub fn insert_key_after(&mut self) {
        let (r, k) = self.sel.unwrap_or((0, self.layout.rows.first().map_or(0, |r| r.keys.len().saturating_sub(1))));
        if self.layout.rows.is_empty() {
            self.layout.rows.push(Row { keys: vec![], weight: 1.0, offset: 0.0 });
        }
        let r = r.min(self.layout.rows.len() - 1);
        let row = &mut self.layout.rows[r];
        let at = (k + 1).min(row.keys.len());
        row.keys.insert(at, new_key("?"));
        self.sel = Some((r, at));
        self.dirty = true;
    }

    /// Delete the selected key; the selection moves to its neighbour. A row
    /// left empty is removed too (an empty row is a gap nobody can tap).
    pub fn delete_key(&mut self) {
        let Some((r, k)) = self.sel else { return };
        let Some(row) = self.layout.rows.get_mut(r) else { return };
        if k >= row.keys.len() {
            return;
        }
        row.keys.remove(k);
        self.dirty = true;
        if row.keys.is_empty() {
            self.layout.rows.remove(r);
            self.sel = None;
            return;
        }
        self.sel = Some((r, k.min(row.keys.len() - 1)));
    }

    /// Move the selected key left (-1) or right (+1) within its row.
    pub fn move_key(&mut self, dir: i32) {
        let Some((r, k)) = self.sel else { return };
        let Some(row) = self.layout.rows.get_mut(r) else { return };
        let to = k as i32 + dir;
        if to < 0 || to as usize >= row.keys.len() {
            return;
        }
        row.keys.swap(k, to as usize);
        self.sel = Some((r, to as usize));
        self.dirty = true;
    }

    /// A new row below the selected one (or at the bottom), holding one key.
    pub fn insert_row_below(&mut self) {
        let at = self.sel.map_or(self.layout.rows.len(), |(r, _)| r + 1).min(self.layout.rows.len());
        self.layout.rows.insert(at, Row { keys: vec![new_key("?")], weight: 1.0, offset: 0.0 });
        self.sel = Some((at, 0));
        self.dirty = true;
    }

    pub fn delete_row(&mut self) {
        let Some((r, _)) = self.sel else { return };
        if r < self.layout.rows.len() && self.layout.rows.len() > 1 {
            self.layout.rows.remove(r);
            self.sel = None;
            self.dirty = true;
        }
    }

    /// The row's stagger, in unit keys (ignored on ortholinear boards).
    pub fn set_row_offset(&mut self, offset: f32) {
        if let Some((r, _)) = self.sel {
            if let Some(row) = self.layout.rows.get_mut(r) {
                row.offset = ((offset / WIDTH_STEP).round() * WIDTH_STEP).clamp(0.0, 3.0);
                self.dirty = true;
            }
        }
    }

    pub fn set_ortholinear(&mut self, on: bool) {
        self.layout.ortholinear = on;
        self.dirty = true;
    }

    /// Fit `grid_columns` to the widest row, so an ortholinear board spans the
    /// width and validation's "row too wide" check measures the real board.
    pub fn fit_columns(&mut self) {
        let widest = self.layout.rows.iter().map(|r| r.keys.iter().map(|k| k.width).sum::<f32>() + r.offset).fold(0.0, f32::max);
        if widest > 0.0 {
            self.layout.grid_columns = widest;
        }
    }

    /// Structural check (board_layouts' own), before saving.
    pub fn validate(&self) -> Result<(), String> {
        board_layouts::io::validate(&self.layout).map_err(|e| e.to_string())
    }

    pub fn to_json(&self) -> Result<String, String> {
        board_layouts::io::export_to_string(&self.layout).map_err(|e| e.to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn qwerty() -> Layout {
        board_layouts::io::import_from_string(crate::real_layout::QWERTY_JSON).unwrap()
    }

    #[test]
    fn a_template_is_a_renamed_copy_that_cannot_overwrite_the_original() {
        let e = Editor::from_template(&qwerty(), "Thumb QWERTY");
        assert_eq!(e.layout.display_name, "Thumb QWERTY");
        assert!(e.layout.name.starts_with("user-") && e.layout.name != qwerty().name);
        assert_eq!(e.layout.rows.len(), qwerty().rows.len());
        assert!(e.validate().is_ok());
    }

    #[test]
    fn editing_a_key_changes_exactly_that_key() {
        let mut e = Editor::from_template(&qwerty(), "x");
        e.select(0, 0);
        let before_next = e.layout.rows[0].keys[1].label.clone();
        e.set_label("ä");
        e.set_shifted("Ä");
        e.set_extended("à á  â");
        e.set_width(1.13);
        assert_eq!(e.selected().unwrap().label, "ä");
        assert_eq!(e.selected().unwrap().shifted.as_deref(), Some("Ä"));
        assert_eq!(e.selected().unwrap().extended, vec!["à", "á", "â"]);
        assert_eq!(e.selected().unwrap().width, 1.25, "stepped to quarters");
        assert_eq!(e.layout.rows[0].keys[1].label, before_next);
        e.set_shifted("  ");
        assert_eq!(e.selected().unwrap().shifted, None, "blank means the default");
    }

    #[test]
    fn actions_with_text_round_trip_through_json() {
        let mut e = Editor::new(blank("Macros"));
        e.select(0, 0);
        e.set_action(ActionChoice::Text, "kessler@example.com");
        e.select(0, 1);
        e.set_action(ActionChoice::Keysym, "Left");
        e.select(0, 2);
        e.set_action(ActionChoice::Backspace, "");
        assert!(!e.selected().unwrap().swipe_target, "a swipe never types Backspace");
        let json = e.to_json().unwrap();
        let back = board_layouts::io::import_from_string(&json).unwrap();
        assert_eq!(back.rows[0].keys[0].action, KeyAction::Literal("kessler@example.com".into()));
        assert_eq!(back.rows[0].keys[1].action, KeyAction::Keysym("Left".into()));
        assert_eq!(ActionChoice::of(&back.rows[0].keys[2].action), ActionChoice::Backspace);
    }

    #[test]
    fn keys_and_rows_can_be_added_moved_and_removed() {
        let mut e = Editor::new(blank("b"));
        e.select(0, 1);
        e.insert_key_after();
        assert_eq!(e.sel, Some((0, 2)));
        assert_eq!(e.layout.rows[0].keys.len(), 6);
        e.set_label("Z");
        e.move_key(1);
        assert_eq!(e.layout.rows[0].keys[3].label, "Z");
        assert_eq!(e.sel, Some((0, 3)));
        e.move_key(99);
        e.insert_row_below();
        assert_eq!(e.layout.rows.len(), 2);
        e.delete_key();
        assert_eq!(e.layout.rows.len(), 1, "an emptied row goes too");
        e.select(0, 0);
        e.delete_row();
        assert_eq!(e.layout.rows.len(), 1, "the last row stays: a layout needs one");
    }

    #[test]
    fn fitting_columns_makes_a_wide_row_valid() {
        let mut e = Editor::new(blank("w"));
        e.select(0, 0);
        for _ in 0..8 {
            e.insert_key_after();
        }
        e.set_width(4.0);
        e.fit_columns();
        assert!(e.validate().is_ok(), "{:?}", e.validate());
        assert_eq!(e.layout.grid_columns, 16.0, "12 one-unit keys and one of 4");
    }

    #[test]
    fn every_action_choice_maps_back_to_itself() {
        for c in ActionChoice::ALL {
            assert_eq!(ActionChoice::of(&c.to_action("v")), c, "{c:?}");
        }
    }
}

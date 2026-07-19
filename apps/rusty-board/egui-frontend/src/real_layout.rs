//! real_layout — the GTK app's layout layer, consumed natively via the shared `board-layouts`
//! crate. The first version of this module MIRRORED the JSON schema with its own serde structs;
//! that duplicate is gone — both frontends now read ONE model (`board_layouts::{Layout, Row}`,
//! `board_layouts::key::{Key, KeyAction}`), so a schema change lands in exactly one place and
//! this module's tests parse the same embedded resource files the GTK app ships.
//!
//! What stays here is the egui-side composition: the shift cycle (off → one-shot → caps, as the
//! GTK `keyboard_view` carried it), the symbols toggle via each layout's `symbols_layout` name,
//! typed [`Press`]es for the host, and [`to_render`] projecting layout + shift onto the existing
//! render [`crate::Layout`] — `keyboard_ui`, the glass painter, and the hold-variants popup all
//! work unchanged. Action keys cross the render layer as `\u{0}`-prefixed reserved tokens
//! ([`classify`] maps them back); the typed [`KeyAction`] is the source, so new action kinds are
//! a compile-time concern, not a string drift.

use board_layouts::key::{Key as KeyDef, KeyAction};
use board_layouts::Layout as LayoutFile;

// ── the shipped layouts, embedded (no runtime path needed) ───────────────────

pub const QWERTY_JSON: &str = include_str!("../../resources/layouts/en_us_qwerty.json");
pub const DVORAK_ORTHO_JSON: &str = include_str!("../../resources/layouts/en_us_dvorak_ortho.json");
pub const SYMBOLS_JSON: &str = include_str!("../../resources/layouts/symbols.json");

/// Parse one layout file — `board_layouts::io::import_from_string`, stringly-errored for the
/// frontend's plumbing.
pub fn parse(json: &str) -> Result<LayoutFile, String> {
    board_layouts::io::import_from_string(json).map_err(|e| e.to_string())
}

/// The three shipped layouts, parsed (and validated by the shared `io::validate`) from the
/// embedded bytes. Panics only if the repo's own resource files are broken — which the tests
/// catch first.
pub fn builtin() -> Vec<LayoutFile> {
    [QWERTY_JSON, DVORAK_ORTHO_JSON, SYMBOLS_JSON]
        .iter()
        .map(|j| parse(j).expect("shipped layout parses"))
        .collect()
}

// ── shift state (the GTK keyboard_view cycle) ────────────────────────────────

/// Off → tap shift → `Once` (one-shot: the next text key consumes it) → tap shift again → `Caps`
/// (locked) → tap again → Off.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Shift {
    #[default]
    Off,
    Once,
    Caps,
}

impl Shift {
    pub fn active(self) -> bool {
        self != Shift::Off
    }
    /// The shift key was tapped.
    pub fn tap(self) -> Shift {
        match self {
            Shift::Off => Shift::Once,
            Shift::Once => Shift::Caps,
            Shift::Caps => Shift::Off,
        }
    }
    /// A text key was emitted: one-shot consumes, caps holds.
    pub fn after_text(self) -> Shift {
        match self {
            Shift::Once => Shift::Off,
            s => s,
        }
    }
}

// ── typed presses (action keys ride the render layer as reserved tokens) ─────

/// What a key press means to the host. `Text` covers plain keys, shifted keys, hold-variants,
/// space (as `" "`), tab (as `"\t"`), and `Literal` keys — hosts treat them all as text,
/// matching the GTK path.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Press {
    Text(String),
    Backspace,
    Return,
    Shift,
    SymbolsToggle,
    Voice,
}

/// Reserved emit-token prefix for action keys crossing the render layer. `\u{0}` can't occur in
/// layout text, so tokens never collide with real emissions.
const ACT: &str = "\u{0}act:";

/// The render-layer token for a typed action. `Character` never gets a token (it's a text key);
/// `Literal`/`Keysym` carry their payload as plain text (keysym injection is the wayland side's
/// job — in this frontend it degrades to the name, documented).
fn action_emit(action: &KeyAction, text: &str) -> String {
    match action {
        KeyAction::Character => text.to_string(),
        KeyAction::Literal(s) => s.clone(),
        KeyAction::Keysym(name) => name.clone(),
        KeyAction::Backspace => format!("{ACT}backspace"),
        KeyAction::Return => format!("{ACT}return"),
        KeyAction::Space => format!("{ACT}space"),
        KeyAction::Tab => format!("{ACT}tab"),
        KeyAction::Shift => format!("{ACT}shift"),
        KeyAction::SymbolsToggle => format!("{ACT}symbols_toggle"),
        KeyAction::Voice => format!("{ACT}voice"),
        // pages/system actions this demo host doesn't implement: cross as their kind so
        // `classify` degrades them to visible-but-inert text (never a silent drop)
        other => format!("{ACT}{other:?}"),
    }
}

/// Turn a `keyboard_ui` emission back into a typed [`Press`].
pub fn classify(emit: &str) -> Press {
    match emit.strip_prefix(ACT) {
        Some("shift") => Press::Shift,
        Some("backspace") => Press::Backspace,
        Some("return") => Press::Return,
        Some("space") => Press::Text(" ".to_string()),
        Some("tab") => Press::Text("\t".to_string()),
        Some("symbols_toggle") => Press::SymbolsToggle,
        Some("voice") => Press::Voice,
        Some(other) => Press::Text(other.to_string()),
        None => Press::Text(emit.to_string()),
    }
}

// ── projection onto the existing render layer ────────────────────────────────

/// The label/emitted text for a key under a shift state — the shared model's documented default:
/// `shifted` falls back to `label.to_uppercase()` when missing.
fn key_text(k: &KeyDef, shift: Shift) -> String {
    if shift.active() {
        k.shifted.clone().unwrap_or_else(|| k.label.to_uppercase())
    } else {
        k.label.clone()
    }
}

/// Project a layout + shift state onto the render [`crate::Layout`] `keyboard_ui` draws: text
/// keys relabel/emit per shift (the GTK view relabels under shift); `extended` (or
/// `extended_shifted` while shifted) becomes the hold-variants popup; action keys carry their
/// reserved token and show their file label.
pub fn to_render(file: &LayoutFile, shift: Shift) -> crate::Layout {
    let rows = file
        .rows
        .iter()
        .map(|r| {
            let keys = r
                .keys
                .iter()
                .map(|k| {
                    let is_text = matches!(k.action, KeyAction::Character);
                    let text = key_text(k, shift);
                    let (label, emit) = if is_text {
                        (text.clone(), text)
                    } else {
                        (k.label.clone(), action_emit(&k.action, &k.label))
                    };
                    let mut key = crate::Key::emitting(&label, &emit, k.width);
                    key.variants = if shift.active() && !k.extended_shifted.is_empty() {
                        k.extended_shifted.clone()
                    } else {
                        k.extended.clone()
                    };
                    key
                })
                .collect();
            crate::Row { keys, weight: r.weight }
        })
        .collect();
    crate::Layout { name: file.display_name.clone(), rows }
}

// ── the stateful board over real layout files ────────────────────────────────

/// The real-layout keyboard state: which file is active, the shift cycle, and the symbols
/// toggle (switches to the active layout's `symbols_layout` by name; toggling again returns to
/// the layout that entered symbols — the GTK behavior).
pub struct RealBoard {
    pub files: Vec<LayoutFile>,
    pub active: usize,
    pub shift: Shift,
    before_symbols: Option<usize>,
}

impl RealBoard {
    /// A board over the shipped layouts, starting on the first (QWERTY).
    pub fn new() -> RealBoard {
        RealBoard { files: builtin(), active: 0, shift: Shift::Off, before_symbols: None }
    }

    pub fn with_files(files: Vec<LayoutFile>) -> RealBoard {
        RealBoard { files, active: 0, shift: Shift::Off, before_symbols: None }
    }

    pub fn current(&self) -> &LayoutFile {
        &self.files[self.active]
    }

    /// The render layout for the current file + shift state (hand to `keyboard_ui`).
    pub fn render(&self) -> crate::Layout {
        to_render(self.current(), self.shift)
    }

    fn index_of(&self, name: &str) -> Option<usize> {
        self.files.iter().position(|f| f.name == name)
    }

    /// Feed a `keyboard_ui` emission through the state machine. Returns the press the HOST still
    /// has to act on (`Text`/`Backspace`/`Return`/`Voice`); `Shift` and `SymbolsToggle` are
    /// handled here (state mutated) and return `None`.
    pub fn press(&mut self, emit: &str) -> Option<Press> {
        match classify(emit) {
            Press::Shift => {
                self.shift = self.shift.tap();
                None
            }
            Press::SymbolsToggle => {
                if let Some(back) = self.before_symbols.take() {
                    self.active = back; // toggling back out of symbols
                } else if let Some(sym) = self.current().symbols_layout.clone().and_then(|n| self.index_of(&n)) {
                    self.before_symbols = Some(self.active);
                    self.active = sym;
                }
                None
            }
            Press::Text(t) => {
                self.shift = self.shift.after_text();
                Some(Press::Text(t))
            }
            other => Some(other),
        }
    }
}

impl Default for RealBoard {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn all_three_shipped_layout_files_parse_through_the_shared_model() {
        let files = builtin();
        assert_eq!(files.len(), 3);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["en_us_qwerty", "en_us_dvorak_ortho", "symbols"]);
        // qwerty declares the symbols layout by name, and that name resolves
        assert_eq!(files[0].symbols_layout.as_deref(), Some("symbols"));
        assert!(files.iter().any(|f| f.name == "symbols"));
        assert!(files[1].ortholinear);
        // schema spot-pins against the real bytes, now through board_layouts types
        let q = &files[0].rows[0].keys[0];
        assert_eq!((q.label.as_str(), q.shifted.as_deref()), ("q", Some("Q")));
        assert!(q.extended.contains(&"1".to_string()));
        let shift_key = files[0].rows[2].keys.first().unwrap();
        assert_eq!(shift_key.action, KeyAction::Shift, "typed action, no string sniffing");
        assert_eq!(shift_key.width, 1.5);
    }

    #[test]
    fn shift_cycles_off_once_caps_and_one_shot_consumes() {
        let mut s = Shift::Off;
        s = s.tap();
        assert_eq!(s, Shift::Once);
        assert_eq!(s.after_text(), Shift::Off, "one-shot consumed by the first text key");
        s = Shift::Once.tap();
        assert_eq!(s, Shift::Caps);
        assert_eq!(s.after_text(), Shift::Caps, "caps holds across text");
        assert_eq!(s.tap(), Shift::Off);
    }

    #[test]
    fn to_render_relabels_under_shift_with_the_uppercase_default() {
        let files = builtin();
        let low = to_render(&files[0], Shift::Off);
        let up = to_render(&files[0], Shift::Caps);
        assert_eq!(low.rows[0].keys[0].emit, "q");
        assert_eq!(up.rows[0].keys[0].emit, "Q");
        assert_eq!(up.rows[0].keys[0].label, "Q", "the view relabels under shift, like GTK");
        assert!(low.rows[0].keys[2].variants.contains(&"é".to_string()));
        let shift_render = &low.rows[2].keys[0];
        assert_eq!(classify(&shift_render.emit), Press::Shift);
        assert_eq!(shift_render.weight, 1.5, "file width flows into render weight");
        // the shared model's documented default: missing `shifted` uppercases the label
        let mut k = files[0].rows[0].keys[0].clone();
        k.shifted = None;
        assert_eq!(key_text(&k, Shift::Caps), "Q");
    }

    #[test]
    fn classify_round_trips_every_action_and_passes_text_through() {
        for (a, press) in [
            (KeyAction::Shift, Press::Shift),
            (KeyAction::Backspace, Press::Backspace),
            (KeyAction::Return, Press::Return),
            (KeyAction::SymbolsToggle, Press::SymbolsToggle),
            (KeyAction::Voice, Press::Voice),
        ] {
            assert_eq!(classify(&action_emit(&a, "x")), press);
        }
        assert_eq!(classify(&action_emit(&KeyAction::Space, "x")), Press::Text(" ".to_string()));
        assert_eq!(classify(&action_emit(&KeyAction::Tab, "x")), Press::Text("\t".to_string()));
        // Literal/Keysym carry payloads as text; plain text passes through
        assert_eq!(classify(&action_emit(&KeyAction::Literal(":-)".into()), "x")), Press::Text(":-)".to_string()));
        assert_eq!(classify("é"), Press::Text("é".to_string()));
    }

    #[test]
    fn board_presses_drive_shift_and_symbols_state() {
        let mut b = RealBoard::new();
        assert_eq!(b.current().name, "en_us_qwerty");
        let shift_tok = action_emit(&KeyAction::Shift, "");
        assert_eq!(b.press(&shift_tok), None);
        assert_eq!(b.shift, Shift::Once);
        let r = b.render();
        assert_eq!(r.rows[0].keys[0].emit, "Q");
        assert_eq!(b.press("Q"), Some(Press::Text("Q".to_string())));
        assert_eq!(b.shift, Shift::Off, "one-shot consumed");
        let sym_tok = action_emit(&KeyAction::SymbolsToggle, "");
        assert_eq!(b.press(&sym_tok), None);
        assert_eq!(b.current().name, "symbols");
        assert_eq!(b.press(&sym_tok), None);
        assert_eq!(b.current().name, "en_us_qwerty");
        assert_eq!(b.press(&action_emit(&KeyAction::Backspace, "")), Some(Press::Backspace));
        assert_eq!(b.press(&action_emit(&KeyAction::Space, "")), Some(Press::Text(" ".to_string())));
    }
}

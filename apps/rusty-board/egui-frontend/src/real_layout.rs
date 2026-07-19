//! real_layout — the GTK app's layout *files*, consumed natively. The standalone `Layout` in
//! `lib.rs` was a stopgap ("a faithful subset … so the crate builds on its own"); this module is
//! the real thing: a serde model of `resources/layouts/*.json` (the exact schema
//! `src/layouts/mod.rs` reads — field names, defaults, and the `action.kind` strings), the three
//! shipped layouts embedded at compile time, and the keyboard STATE the GTK `keyboard_view`
//! carried: shift (off → one-shot → caps-lock cycle), the symbols toggle (via each layout's
//! `symbols_layout` name, returning to the previous layout on toggle-back), and typed key
//! presses so the host maps actions instead of sniffing strings.
//!
//! Composition, not duplication: [`to_render`] projects a parsed file + shift state onto the
//! existing render [`crate::Layout`] — `keyboard_ui`/`paint_key`/the hold-variants popup all work
//! unchanged. Action keys ride through the render layer as reserved emit tokens ([`classify`]
//! turns them back into [`Press`]); `extended` becomes the hold-variants popup; `shifted` is the
//! emitted text under shift. Everything here is pure and tested against the REAL resource files
//! (parsed from the embedded bytes), so schema drift in the GTK app fails these tests loudly.

use serde::Deserialize;

// ── the file schema (mirrors src/layouts/mod.rs; serde defaults match) ───────

#[derive(Debug, Clone, Deserialize)]
pub struct LayoutFile {
    pub name: String,
    pub display_name: String,
    #[serde(default)]
    pub language: String,
    #[serde(default)]
    pub direction: String,
    /// The layout switched to by the `symbols_toggle` action (a layout `name`).
    #[serde(default)]
    pub symbols_layout: Option<String>,
    #[serde(default)]
    pub ortholinear: bool,
    pub rows: Vec<RowDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct RowDef {
    #[serde(default = "one")]
    pub weight: f32,
    pub keys: Vec<KeyDef>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct KeyDef {
    pub label: String,
    /// Emitted text under shift; text keys without it emit `label` in every shift state.
    #[serde(default)]
    pub shifted: Option<String>,
    /// Long-press alternates — the hold-variants popup.
    #[serde(default)]
    pub extended: Vec<String>,
    /// Action keys (shift / backspace / return / space / symbols_toggle / voice).
    #[serde(default)]
    pub action: Option<ActionDef>,
    #[serde(default)]
    pub key_type: Option<String>,
    /// Relative width in row units (the GTK `width`; the render layer calls it `weight`).
    #[serde(default = "one")]
    pub width: f32,
    #[serde(default)]
    pub swipe_target: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
pub struct ActionDef {
    pub kind: String,
}

fn one() -> f32 {
    1.0
}

// ── the shipped layouts, embedded (no runtime path needed) ───────────────────

pub const QWERTY_JSON: &str = include_str!("../../resources/layouts/en_us_qwerty.json");
pub const DVORAK_ORTHO_JSON: &str = include_str!("../../resources/layouts/en_us_dvorak_ortho.json");
pub const SYMBOLS_JSON: &str = include_str!("../../resources/layouts/symbols.json");

/// Parse one layout file.
pub fn parse(json: &str) -> Result<LayoutFile, String> {
    serde_json::from_str(json).map_err(|e| e.to_string())
}

/// The three shipped layouts, parsed from the embedded bytes. Panics only if the repo's own
/// resource files are broken — which the tests catch first.
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
/// and space (as `" "` — hosts treat it as text, matching the GTK path).
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

fn action_token(kind: &str) -> String {
    format!("{ACT}{kind}")
}

/// Turn a `keyboard_ui` emission back into a typed [`Press`].
pub fn classify(emit: &str) -> Press {
    match emit.strip_prefix(ACT) {
        Some("shift") => Press::Shift,
        Some("backspace") => Press::Backspace,
        Some("return") => Press::Return,
        Some("space") => Press::Text(" ".to_string()),
        Some("symbols_toggle") => Press::SymbolsToggle,
        Some("voice") => Press::Voice,
        Some(other) => Press::Text(other.to_string()), // unknown action: degrade to its kind text
        None => Press::Text(emit.to_string()),
    }
}

// ── projection onto the existing render layer ────────────────────────────────

/// Project a parsed layout + shift state onto the render [`crate::Layout`] `keyboard_ui` draws:
/// text keys emit `shifted` when shift is active (label otherwise) and show it as their label
/// (the GTK view relabels under shift); `extended` becomes the hold-variants popup; action keys
/// carry their reserved token and show their file label (⇧ highlighted by state is the host's
/// paint concern, out of scope here).
pub fn to_render(file: &LayoutFile, shift: Shift) -> crate::Layout {
    let rows = file
        .rows
        .iter()
        .map(|r| {
            let keys = r
                .keys
                .iter()
                .map(|k| {
                    if let Some(a) = &k.action {
                        let mut key = crate::Key::emitting(&k.label, &action_token(&a.kind), k.width);
                        key.variants = k.extended.clone();
                        key
                    } else {
                        let text = if shift.active() {
                            k.shifted.clone().unwrap_or_else(|| k.label.clone())
                        } else {
                            k.label.clone()
                        };
                        let mut key = crate::Key::emitting(&text, &text, k.width);
                        key.variants = k.extended.clone();
                        key
                    }
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
    fn all_three_shipped_layout_files_parse() {
        let files = builtin();
        assert_eq!(files.len(), 3);
        let names: Vec<&str> = files.iter().map(|f| f.name.as_str()).collect();
        assert_eq!(names, ["en_us_qwerty", "en_us_dvorak_ortho", "symbols"]);
        // qwerty declares the symbols layout by name, and that name resolves
        assert_eq!(files[0].symbols_layout.as_deref(), Some("symbols"));
        assert!(files.iter().any(|f| f.name == "symbols"));
        // the dvorak variant is ortholinear per its file
        assert!(files[1].ortholinear);
        // schema spot-pins against the real bytes: q/Q with extended, the wide shift action key
        let q = &files[0].rows[0].keys[0];
        assert_eq!((q.label.as_str(), q.shifted.as_deref()), ("q", Some("Q")));
        assert!(q.extended.contains(&"1".to_string()));
        let shift_key = files[0].rows[2].keys.first().unwrap();
        assert_eq!(shift_key.action.as_ref().map(|a| a.kind.as_str()), Some("shift"));
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
    fn to_render_relabels_under_shift_and_carries_extended_as_variants() {
        let files = builtin();
        let low = to_render(&files[0], Shift::Off);
        let up = to_render(&files[0], Shift::Caps);
        assert_eq!(low.rows[0].keys[0].emit, "q");
        assert_eq!(up.rows[0].keys[0].emit, "Q");
        assert_eq!(up.rows[0].keys[0].label, "Q", "the view relabels under shift, like GTK");
        // extended → hold-variants popup
        assert!(low.rows[0].keys[2].variants.contains(&"é".to_string()));
        // action keys carry reserved tokens the render layer can't collide with
        let shift_render = &low.rows[2].keys[0];
        assert_eq!(classify(&shift_render.emit), Press::Shift);
        assert_eq!(shift_render.weight, 1.5, "file width flows into render weight");
    }

    #[test]
    fn classify_round_trips_every_action_and_passes_text_through() {
        for (kind, press) in [
            ("shift", Press::Shift),
            ("backspace", Press::Backspace),
            ("return", Press::Return),
            ("symbols_toggle", Press::SymbolsToggle),
            ("voice", Press::Voice),
        ] {
            assert_eq!(classify(&action_token(kind)), press);
        }
        assert_eq!(classify(&action_token("space")), Press::Text(" ".to_string()));
        assert_eq!(classify("é"), Press::Text("é".to_string()));
    }

    #[test]
    fn board_presses_drive_shift_and_symbols_state() {
        let mut b = RealBoard::new();
        assert_eq!(b.current().name, "en_us_qwerty");
        // shift tap is absorbed by the board; the next text key comes out shifted and consumes it
        assert_eq!(b.press(&action_token("shift")), None);
        assert_eq!(b.shift, Shift::Once);
        let r = b.render();
        assert_eq!(r.rows[0].keys[0].emit, "Q");
        assert_eq!(b.press("Q"), Some(Press::Text("Q".to_string())));
        assert_eq!(b.shift, Shift::Off, "one-shot consumed");
        // symbols toggle switches by the layout's declared name, and toggles back
        assert_eq!(b.press(&action_token("symbols_toggle")), None);
        assert_eq!(b.current().name, "symbols");
        assert_eq!(b.press(&action_token("symbols_toggle")), None);
        assert_eq!(b.current().name, "en_us_qwerty");
        // actions the host owns pass through
        assert_eq!(b.press(&action_token("backspace")), Some(Press::Backspace));
        assert_eq!(b.press(&action_token("return")), Some(Press::Return));
        assert_eq!(b.press(&action_token("space")), Some(Press::Text(" ".to_string())));
    }
}

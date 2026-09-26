//! rusty-board-egui — the on-screen keyboard, ported to egui on uikit-egui + adaptive.
//!
//! The GTK4 UI (`src/ui/*`) becomes this: a keyboard rendered with egui, sized by the
//! [`Adaptive`] layer (touch targets grow on the FP5, tighten on desktop) and themed from
//! snapclone-theme. rusty-board's `layouts`, `gestures`, and `keyboard/engine.rs` are
//! already GTK-free, so on-device this front-end consumes the real `layouts::Layout` and
//! feeds taps/swipes into the engine. Here it carries a small standalone [`Layout`] so the
//! crate builds and can be checked against egui 0.29 on its own.

use egui::{Align2, FontId, Rect, CornerRadius, Sense, Stroke, Ui, Vec2};
use uikit_egui::{apply_palette, Adaptive, PaletteColors};

/// Which controls live on the board, which on the options layer, and which
/// belong in a separate settings window. The placement rule is "does this need
/// the keyboard on screen while you use it", which is what stops the top bar
/// growing back.
pub mod chrome;
/// The settings model driven by name rather than by thumb: one command grammar
/// behind the `rusty-board-settings` CLI and the full-page configurator, so
/// neither can drift into its own idea of what the options are.
pub mod settings_cli;
pub mod cardinal_gesture;
pub mod ngram;
pub mod swipe_rerank;
pub mod settings_model;
pub mod animations;
pub mod tap_hold;
pub mod keyboard_service;
/// Where a keypress actually goes: the system, or a preview box.
pub mod sink;
pub mod focus_policy;
pub mod adaptive;
pub mod nudge;
pub mod real_layout;
pub mod dictation;
pub mod mic_source;
pub mod tray;
/// One-handed compaction: anchor the board to a corner and narrow it.
pub mod halfboard;
/// The reach polygon and its per-row compaction (`single-hand-mode.md` §1).
pub mod thumbarc;
/// Measuring the operator's thumb reach: anchor, sweep, confirm.
pub mod thumbcal;
/// The two gestures on the top edge: the digit up-arc and the minimize fling.
pub mod topedge;
/// Swipe out of the letters and back to type a space (`single-hand-mode.md` §3).
pub mod springboard;
/// The layout switcher's icons (an SVG subset drawn as strokes) and the rail that shows them.
pub mod layout_icons;
pub mod layout_rail;
/// Full / Tray / Dismissed and every way between them (`docs/single-hand-mode.md`).
pub mod visible_mode;
pub mod microterm;
pub mod voice;
pub mod steno;
#[cfg(feature = "wayland")]
pub mod wayland;
pub mod settings;
/// Session/placement policy: which host obligations the shell can meet, KWin socket capture.
pub mod platform;
/// The shells: what owns the surface and runs `Feel::frame` in it (layer-shell under `layer`).
#[cfg(feature = "app")]
pub mod shell;
// config/mod.rs refers to `crate::util`, so the port re-exports it at the root.
pub use settings::util;
// …and the shared voice tree refers to `crate::config::VoiceConfig`, so the
// port mounts config at the root too — the half of the job the util
// re-export above prepared for and nobody finished, which is why the voice
// feature never compiled in this crate (c11 field report). Shared files may
// use `crate::`-paths ONLY for modules both crates expose at the same root;
// anything under voice/ itself uses `super::` (voice::tests pins this).
#[cfg(feature = "voice")]
#[path = "../../src/config/mod.rs"]
pub mod config;
/// Capture + streaming partials, driving the mounted engine from the frame loop.
#[cfg(feature = "voice")]
pub mod voice_host;
pub mod swipe;
#[cfg(feature = "app")]
pub mod demo_app;

/// A single key: what it shows and its relative width within the row.
#[derive(Debug, Clone)]
pub struct Key {
    pub label: String,
    pub weight: f32,
    /// The string emitted on tap (defaults to the label).
    pub emit: String,
    /// Long-press alternates (accents, symbols); empty = no popup. Surfaced via the shared
    /// `uikit_egui::slide_menu` gesture grid on a hold.
    pub variants: Vec<String>,
    /// Cardinal-swipe variants (MessagEase-style): tap emits `emit`, a swipe in a compass direction
    /// emits that direction's string. Empty = a plain key. Surfaced via `slide_menu::show_cardinal`.
    pub cardinal: Vec<(uikit_egui::slide_menu::Dir8, String)>,
    /// Clockwise-rotation output — GTK's `KeyGestures.cw`, almost always a digit (each cardinal
    /// key's rotation types the digit at its own numpad position: top-left rotates to "1", top-
    /// right to "3", and so on). `None` when the key has no rotation output.
    pub rotate_cw: Option<String>,
    /// Counter-clockwise-rotation output — GTK's `KeyGestures.ccw`. On most cardinal keys this is
    /// the tap letter's own capital (the ONE letter a hairpin cannot reach, since hairpin
    /// capitalises whichever swipe DIRECTION it points toward and the tap letter has none); `a`'s
    /// ccw is `,` instead, because a sentence-starting comma is common enough on the single most
    /// frequent key to be worth the exception.
    pub rotate_ccw: Option<String>,
    /// `Some(button_id)` ("L1"/"L2"/"R1"/"R2") when this key routes gestures
    /// into the stenotype engine instead of committing `emit`/`cardinal`
    /// text directly. See `steno.rs`'s module doc for the dispatch this
    /// enables and why cardinal's normal id==label string can't carry it
    /// (steno phonemes collide across buttons/directions — `"n"` is both
    /// L1-north and R2-north — so identity has to survive as the typed
    /// button id, not be recovered from the committed text afterward).
    pub steno_button: Option<String>,
    /// What KIND of key this is — the GTK original's `KeyType`, which drives its `.key-normal`
    /// / `.key-modifier` / `.key-action` / `.key-special` CSS class and therefore its colour.
    ///
    /// The egui port had no role at all: `paint_key_face` guessed with
    /// `key.label.chars().count() > 1`, so "space", "enter", "shift" and "?123" were all one
    /// look. On the GTK board Enter is accent-filled with white text — it is the key that
    /// COMMITS, and it is meant to be findable without reading it — and a locked shift is a
    /// solid accent cap you can see from across the room. Neither existed here.
    pub role: KeyRole,
    /// A modifier's LATCH, which GTK carries as the extra CSS classes `.shift-active` and
    /// `.shift-locked` on the shift key.
    ///
    /// The port had no equivalent, so a one-shot shift and a caps lock looked exactly like
    /// an idle shift: the only way to know which of three states the board was in was to
    /// type a letter and see what came out. On the GTK board a locked shift is a solid
    /// accent cap you can read from across the room.
    pub latch: Latch,
}

/// A modifier's latch state — `.shift-active` / `.shift-locked` in the stylesheet.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum Latch {
    #[default]
    Off,
    /// Armed for one press. Tinted accent, accent text, accent border.
    Once,
    /// Held until turned off. Solid accent, white text — the loudest state on the board,
    /// because it is the one that silently changes what every other key does.
    Locked,
    /// ENGAGED AND RUNNING — the mic key while dictation is recording
    /// (`.key-special.voice-active` in the GTK stylesheet, which pulses).
    ///
    /// A separate state from `Locked` because it pulses, and it pulses because a live
    /// microphone must be visible on the surface you are looking at. The screen already had
    /// a "● dictating" label, in a toolbar the layer-shell build does not draw at all: on a
    /// phone the keyboard IS the surface, so the only indicator that the mic was open was
    /// off-screen.
    Live,
}

/// A key's role, mirroring `board_layouts::KeyType` and the four CSS classes the GTK
/// stylesheet paints from. Kept as its own enum rather than re-using `KeyType` directly so
/// the egui board can carry the two shift STATES, which in GTK are extra classes
/// (`.shift-active`, `.shift-locked`) on a modifier rather than types of their own.
#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug, Default)]
pub enum KeyRole {
    /// A letter or symbol. `.key-normal`.
    #[default]
    Normal,
    /// Shift, ctrl, the symbols toggle. `.key-modifier` — a touch lighter than the surface,
    /// bolder text, and it recedes rather than competing with the letters.
    Modifier,
    /// Enter, and anything else that commits. `.key-action` — accent fill, white text.
    Action,
    /// Backspace, voice, emoji. `.key-special` — faintest of all, still legible.
    Special,
}

impl KeyRole {
    /// From the shared layout crate's type, so a layout file's `key_type` reaches the paint
    /// code instead of being re-derived from the label.
    pub fn from_key_type(t: board_layouts::key::KeyType) -> KeyRole {
        match t {
            board_layouts::key::KeyType::Normal => KeyRole::Normal,
            board_layouts::key::KeyType::Modifier => KeyRole::Modifier,
            board_layouts::key::KeyType::Action => KeyRole::Action,
            board_layouts::key::KeyType::Special => KeyRole::Special,
        }
    }

    /// From a layout file's `key_type` string. An unknown value is Normal rather than an
    /// error: a newer layout naming a role this build does not have should render as a
    /// plain key, not fail to load.
    pub fn from_json(s: &str) -> KeyRole {
        match s.trim().to_ascii_lowercase().as_str() {
            "modifier" => KeyRole::Modifier,
            "action" => KeyRole::Action,
            "special" => KeyRole::Special,
            _ => KeyRole::Normal,
        }
    }

    /// The role a bare label implies, for the demo layouts that are built in code and have no
    /// `key_type` to read. Names, not lengths: "space" is a normal key that happens to be
    /// wide, and guessing by length made it a modifier.
    pub fn from_label(label: &str) -> KeyRole {
        let l = label.trim().to_ascii_lowercase();
        match l.as_str() {
            "enter" | "return" | "↵" | "⏎" | "go" | "send" | "search" => KeyRole::Action,
            "shift" | "⇧" | "caps" | "ctrl" | "alt" | "meta" | "fn" | "?123" | "abc" | "=\\<" => KeyRole::Modifier,
            "backspace" | "⌫" | "del" | "delete" | "tab" | "esc" | "voice" | "🎤" | "emoji" | "☺" => KeyRole::Special,
            _ => KeyRole::Normal,
        }
    }
}

impl Key {
    pub fn new(label: &str) -> Key {
        Key { label: label.to_string(), weight: 1.0, emit: label.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None , role: KeyRole::from_label(label) , latch: Latch::Off }
    }
    pub fn wide(label: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: label.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None , role: KeyRole::from_label(label) , latch: Latch::Off }
    }
    pub fn emitting(label: &str, emit: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: emit.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None , role: KeyRole::from_label(label) , latch: Latch::Off }
    }
    /// Set a modifier's latch — `.shift-active` / `.shift-locked`.
    pub fn with_latch(mut self, latch: Latch) -> Key {
        self.latch = latch;
        self
    }

    /// Set the key's role explicitly — for a layout that knows better than the label does.
    pub fn with_role(mut self, role: KeyRole) -> Key {
        self.role = role;
        self
    }

    /// Attach long-press variant characters (each emits itself).
    pub fn with_variants(mut self, vs: &[&str]) -> Key {
        self.variants = vs.iter().map(|s| s.to_string()).collect();
        self
    }
    /// Attach cardinal-swipe variants: `(direction, emitted string)` pairs. Tap still emits `emit`.
    pub fn with_cardinal(mut self, dirs: &[(uikit_egui::slide_menu::Dir8, &str)]) -> Key {
        self.cardinal = dirs.iter().map(|(d, s)| (*d, s.to_string())).collect();
        self
    }
    /// Attach rotation outputs: GTK's `KeyGestures.cw`/`.ccw`. `None` leaves that direction
    /// unbound, same as an absent field in GTK's layout data.
    pub fn with_rotation(mut self, cw: Option<&str>, ccw: Option<&str>) -> Key {
        self.rotate_cw = cw.map(str::to_string);
        self.rotate_ccw = ccw.map(str::to_string);
        self
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
    /// Draw each cardinal key's direction glyphs at rest. True for boards whose compasses are
    /// the POINT (MessagEase-style grids, the stenotype: the author placed every direction);
    /// false for qwerty, whose compasses are accents derived from `extended` — four tiny
    /// glyphs on forty-five keys is clutter. Measured (examples/frame_bench, qwerty): 4% of
    /// the frame under glass, 11% under the default palette — the clutter is the reason, the
    /// frame time is a bonus. The per-key cost that remains is egui's own widget overhead.
    pub hints: bool,
}

/// Schema for rusty-board's REAL layout files (`resources/layouts/*.json`) — the GTK app's actual
/// source of truth, not a hand-copied approximation of it. This is the concrete fix for the
/// "layout matching" problem: rusty-board-egui used to hardcode its own `Layout::qwerty()` with
/// numbers someone typed by hand and never re-checked against the real file — which had already
/// drifted (the real bottom row is `?123(1.5) | 🎤(1.0) | space(5.0) | ⏎(1.5)`; the hardcoded
/// version was missing `?123`/`🎤` entirely and had space at 6.0, ⏎ at 2.0 — silently wrong
/// proportions AND missing keys, not just a rounding difference). Loading the real file at compile
/// time (`include_str!`) means the two front-ends can never drift again on THIS layout, short of
/// someone deliberately hand-editing one side — which is a much smaller, much more visible thing
/// to review than the two files quietly diverging with no test ever comparing them.
#[derive(serde::Deserialize)]
struct JsonLayout {
    #[serde(default)]
    name: String,
    rows: Vec<JsonRow>,
}
#[derive(serde::Deserialize)]
struct JsonRow {
    #[serde(default = "one")]
    weight: f32,
    keys: Vec<JsonKey>,
}
#[derive(serde::Deserialize)]
struct JsonKey {
    label: String,
    #[serde(default)]
    extended: Vec<String>,
    #[serde(default = "one")]
    width: f32,
    #[serde(default)]
    action: Option<JsonAction>,
    /// `"modifier"` / `"action"` / `"special"` — the SAME field the GTK app reads to pick a
    /// key's CSS class. The port parsed everything else in this schema and dropped this one,
    /// so every key looked the same and Enter was not the accent key it is on the real
    /// board.
    #[serde(default)]
    key_type: Option<String>,
}
#[derive(serde::Deserialize)]
struct JsonAction {
    kind: String,
}
fn one() -> f32 { 1.0 }

/// The literal control string a key's `action.kind` emits (backspace/return/space are the only
/// ones with an unambiguous single control character; anything else — voice, symbols_toggle, or
/// no action at all — falls through to emitting the key's own label, matching `Key::new`'s
/// existing default).
/// What a JSON layout's `action.kind` emits.
///
/// The three text-producing actions emit their character; EVERY OTHER ACTION emits the
/// `\0act:` token `real_layout` already uses, which `classify` decodes back into a `Press`.
///
/// This used to return `None` for anything but those three, and `layout_from_json` then fell
/// back to the key's LABEL — so on the bundled qwerty the mic key emitted the string "🎤"
/// and typed an emoji into your document, `?123` typed "?123", and shift typed "⇧". The
/// three that happened to work were the three listed here. The real-layout path
/// (`real_layout::to_render`) has always produced the tokens; only this projection dropped
/// them, and it is the one behind `Layout::qwerty()`, which is the board that comes up
/// first.
fn emit_for_action(kind: &str) -> Option<String> {
    Some(match kind {
        "backspace" => "\u{8}".to_string(),
        "return" => "\n".to_string(),
        "space" => " ".to_string(),
        // Everything else crosses as a token rather than as its own label. An action this
        // build does not implement still arrives at `classify` as a named action and is
        // ignored there — visible and inert, never typed as text.
        other => format!("{}{other}", real_layout::ACT),
    })
}

/// Parse a rusty-board layout JSON string (the real `resources/layouts/*.json` schema) into this
/// crate's `Layout`. Pure — no filesystem access; callers `include_str!` the real file so the
/// binary always carries an exact, byte-identical copy of whatever the GTK app ships.
fn layout_from_json(text: &str) -> Result<Layout, serde_json::Error> {
    let parsed: JsonLayout = serde_json::from_str(text)?;
    let rows: Vec<Row> = parsed.rows.into_iter().map(|r| Row {
        weight: r.weight,
        keys: r.keys.into_iter().map(|k| {
            let emit = k
                .action
                .as_ref()
                .and_then(|a| emit_for_action(&a.kind))
                .unwrap_or_else(|| k.label.clone());
            let role = k.key_type.as_deref().map(KeyRole::from_json).unwrap_or_else(|| KeyRole::from_label(&k.label));
            let mut key = Key { role, latch: Latch::Off, label: k.label, weight: k.width, emit, variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None };
            if !k.extended.is_empty() { key.variants = k.extended; }
            key
        }).collect(),
    }).collect();
    // The second layer (board_layouts::second_layer), by the same table the real
    // board uses: each letter key's positional symbol first in its long-press list.
    let mut rows = rows;
    let labels: Vec<Vec<&str>> = rows.iter().map(|r: &Row| r.keys.iter().map(|k| k.label.as_str()).collect()).collect();
    let plan = board_layouts::second_layer::plan(&labels);
    for (row, syms) in rows.iter_mut().zip(plan) {
        for (key, sym) in row.keys.iter_mut().zip(syms) {
            if let Some(sym) = sym {
                key.variants = board_layouts::second_layer::merged(&key.variants, sym);
            }
        }
    }
    Ok(Layout { hints: false, name: if parsed.name.is_empty() { "qwerty".into() } else { parsed.name }, rows })
}

impl Layout {
    /// A compact QWERTY, loaded from rusty-board's REAL layout file — see `layout_from_json`'s
    /// doc comment for why this replaced a hand-copied approximation.
    pub fn qwerty() -> Layout {
        layout_from_json(include_str!("../../resources/layouts/en_us_qwerty.json"))
            .expect("bundled en_us_qwerty.json must parse — this is a compile-time asset, not user input")
    }

    /// The four-button **stenotype** board, projected from `board_layouts`' own definition.
    ///
    /// `steno.rs`'s module docs list "define a Stenotype layout for `to_render` to produce —
    /// there is currently no render-layer key that carries a steno button id" as the first
    /// missing piece, and `apply_mode("stenotype")` had nothing to switch to. Both halves
    /// existed: the layout is `board_layouts::Layout::default_stenotype()` (already used by
    /// `RealBoard`), and `to_render` already emits the steno tokens `paint_key` decodes.
    /// Only the demo `Board` did not carry it.
    ///
    /// Projected rather than rebuilt in code, so the gesture labels and the phoneme table
    /// they mirror cannot drift — `default_stenotype`'s own comment says to keep them in
    /// sync, and two definitions is how that stops happening.
    pub fn stenotype() -> Layout {
        real_layout::to_render(&board_layouts::Layout::default_stenotype(), real_layout::Shift::Off)
    }

    /// An experimental MessagEase-style **cardinal** keyboard: a 3×3 of high-frequency letters where a
    /// tap types the center and a swipe in a compass direction types that direction's variant. This is
    /// the flagship user of the shared `slide_menu::show_cardinal` gesture. Reached like any other
    /// layout (swipe the board sideways to cycle to it).
    pub fn cardinal() -> Layout {
        use uikit_egui::slide_menu::Dir8::{E, N, S, W};
        fn k(center: &str, dirs: &[(uikit_egui::slide_menu::Dir8, &str)]) -> Key {
            Key::new(center).with_cardinal(dirs)
        }
        let row = |keys: Vec<Key>| Row { keys, weight: 1.0 };
        Layout {
            hints: true,
            name: "cardinal".into(),
            rows: vec![
                row(vec![
                    k("e", &[(N, "E"), (W, "b"), (S, "v"), (E, "g")]),
                    k("t", &[(N, "T"), (W, "c"), (S, "x"), (E, "k")]),
                    k("a", &[(N, "A"), (W, "d"), (S, "z"), (E, "q")]),
                ]),
                row(vec![
                    k("o", &[(N, "O"), (W, "f"), (S, "j")]),
                    k("i", &[(N, "I"), (W, "p"), (S, "y")]),
                    k("n", &[(N, "N"), (W, "m"), (S, "w")]),
                ]),
                row(vec![
                    k("s", &[(N, "S"), (W, "l"), (E, "u")]),
                    k("h", &[(N, "H"), (S, ".")]),
                    k("r", &[(N, "R"), (E, ",")]),
                ]),
                Row {
                    keys: vec![
                        Key::emitting("⌫", "\u{8}", 1.5),
                        // Dictation reachable from the cardinal board. The GTK build only ever
                        // put the mic on the qwerty row, so the two best input modes could not
                        // be used together; the token is the same `\u{0}act:voice` the real
                        // layout files' Voice action crosses the render layer as.
                        Key::emitting("🎤", &real_layout::voice_emit(), 1.0),
                        Key::emitting("space", " ", 3.0),
                        // BACKSPACE, which this layout did not have. A cardinal board types
                        // a wrong letter more often than a qwerty one does — that is what a
                        // swipe-per-glyph board is — so it is the layout that needs the
                        // correction key most, and it was the one without it.
                        Key::emitting("⌫", "\u{8}", 1.0),
                        Key::emitting("⏎", "\n", 1.5),
                    ],
                    weight: 1.0,
                },
            ],
        }
    }

    /// An **8-way** cardinal keyboard, MessagEase-style and frequency-optimized: the nine most common
    /// English letters sit on the key *centers* (tap), so the vast majority of keystrokes are plain
    /// taps; the remaining letters ride the cardinal swipes and the digits ride the diagonals, so the
    /// whole alphabet plus numbers fit in nine keys. Swiping north is always the uppercase. Exercises
    /// the widget's full `Dir8`.
    pub fn cardinal8() -> Layout {
        use uikit_egui::slide_menu::Dir8::{E, N, NE, S, SE, SW, W};
        fn k(center: &str, dirs: &[(uikit_egui::slide_menu::Dir8, &str)]) -> Key {
            Key::new(center).with_cardinal(dirs)
        }
        let row = |keys: Vec<Key>| Row { keys, weight: 1.0 };
        Layout {
            hints: true,
            name: "cardinal8".into(),
            rows: vec![
                // centers e t a o i n s h r — the nine highest-frequency letters (tap = no swipe)
                row(vec![
                    k("e", &[(N, "E"), (E, "b"), (S, "v"), (NE, "1"), (SE, "2"), (SW, "3")]),
                    k("t", &[(N, "T"), (E, "c"), (S, "w"), (NE, "4"), (SE, "5"), (SW, "6")]),
                    k("a", &[(N, "A"), (E, "d"), (S, "x"), (NE, "7"), (SE, "8"), (SW, "9")]),
                ]),
                row(vec![
                    k("o", &[(N, "O"), (E, "f"), (S, "y"), (W, "k"), (SE, "0")]),
                    k("i", &[(N, "I"), (E, "g"), (S, "z"), (W, "p")]),
                    k("n", &[(N, "N"), (E, "l"), (S, "m"), (W, "u")]),
                ]),
                row(vec![
                    k("s", &[(N, "S"), (E, "j"), (S, "q"), (W, "."), (SE, ",")]),
                    k("h", &[(N, "H"), (E, "'"), (S, "!"), (W, "?")]),
                    k("r", &[(N, "R"), (E, ":"), (S, "@"), (W, "/")]),
                ]),
                Row {
                    keys: vec![
                        Key::emitting("🎤", &real_layout::voice_emit(), 1.0),
                        Key::emitting("space", " ", 2.0),
                        Key::emitting("⌫", "\u{8}", 1.0),
                        Key::emitting("⏎", "\n", 1.0),
                    ],
                    weight: 1.0,
                },
            ],
        }
    }

    /// A symbols/numbers layout, reached by swiping the keyboard sideways.
    pub fn symbols() -> Layout {
        fn row(keys: &[&str]) -> Row {
            Row { keys: keys.iter().map(|k| Key::new(k)).collect(), weight: 1.0 }
        }
        Layout {
            hints: false,
            name: "symbols".into(),
            rows: vec![
                row(&["1", "2", "3", "4", "5", "6", "7", "8", "9", "0"]),
                row(&["!", "@", "#", "$", "%", "&", "*", "(", ")"]),
                Row {
                    keys: vec![
                        Key::wide("=\\<", 1.5),
                        Key::new("-"),
                        Key::new("_"),
                        Key::new("+"),
                        Key::new("/"),
                        Key::new(":"),
                        Key::new(";"),
                        Key::new("?"),
                        Key::emitting("⌫", "\u{8}", 1.5),
                    ],
                    weight: 1.0,
                },
                Row {
                    keys: vec![
                        // THE MIC, on this board too. Dictation is a property of the
                        // keyboard, not of one layout — a board you have to swipe away
                        // from in order to talk is a board that makes you stop and think
                        // about layouts mid-sentence.
                        Key::emitting("🎤", &real_layout::voice_emit(), 1.0),
                        Key::emitting("space", " ", 5.0),
                        Key::emitting("⏎", "\n", 2.0),
                    ],
                    weight: 1.0,
                },
            ],
        }
    }
}

/// A keyboard with multiple layouts you swipe between (qwerty ↔ symbols), using the shared
/// gesture layer — mirroring rusty-board's swipe layout-switching.
pub struct Board {
    layouts: Vec<Layout>,
    idx: usize,
    gesture: uikit_egui::GestureLayer,
    palette: String,
    /// The microphone is open. Drawn on the mic key, because on a phone the keyboard is
    /// the whole surface and the screen's "● dictating" label is in a toolbar the
    /// layer-shell build never draws.
    voice_active: bool,
    /// Off / one-shot / locked, exactly as the real board tracks it.
    ///
    /// The code-built boards had no shift state at all. Before the last commit the shift
    /// key typed the string "⇧"; after it, `Press::Shift` reached the app and fell through
    /// a `_ => {}`. Better, and still a key you press that does nothing — which on a
    /// keyboard is its own kind of broken.
    shift: real_layout::Shift,
    /// Where the symbols toggle came from, so `?123` and `ABC` are the same key going both
    /// ways rather than a one-way trip.
    before_symbols: Option<usize>,
}

impl Board {
    /// Default board: qwerty + symbols.
    pub fn new() -> Board {
        Board { layouts: vec![Layout::qwerty(), Layout::symbols(), Layout::cardinal(), Layout::cardinal8(), Layout::stenotype()], idx: 0, gesture: uikit_egui::GestureLayer::horizontal_page(320.0), voice_active: false,
            shift: real_layout::Shift::Off, before_symbols: None, palette: "system".to_string() }
    }
    pub fn current(&self) -> &Layout {
        &self.layouts[self.idx]
    }

    /// Select the board the SETTINGS ask for.
    ///
    /// `layout.experimental_mode` has offered Off / Cardinal / Stenotype / Words since the
    /// panel was written, and choosing one did nothing: the value went into a `HashMap` and
    /// no code read it back. A setting that persists and changes nothing is the failure
    /// `settings::ready`'s own comment warns about, and this is the second field in a row to
    /// have it.
    ///
    /// Returns whether the board actually moved, so a caller can avoid resetting shift or
    /// re-announcing the layout when nothing changed.
    pub fn apply_mode(&mut self, mode: &str) -> bool {
        let want = match mode.trim() {
            "cardinal" => "cardinal",
            "stenotype" => "stenotype",
            "words" => "cardinal8",
            // "off", and anything a newer build might write that this one does not know:
            // the letter board, which is the safe answer. An unknown mode must not leave the
            // keyboard on whatever was last selected.
            _ => "en_us_qwerty",
        };
        // Case-insensitive: `to_render` takes the rendered layout's name from the file's
        // DISPLAY name ("Stenotype"), while the settings value and the code-built boards use
        // the lower-case one. Matching exactly meant `apply_mode("stenotype")` could not
        // find the board it had just been given.
        let Some(i) = self.layouts.iter().position(|l| l.name.eq_ignore_ascii_case(want)) else {
            // The named board is not in this build. Leave the current one alone rather than
            // falling to index 0 — silently switching the board out from under someone is
            // worse than ignoring a setting this build cannot honour.
            return false;
        };
        if i == self.idx {
            return false;
        }
        self.idx = i;
        // A layout swap ends a one-shot shift: it was armed for a letter on the board you
        // just left.
        self.shift = real_layout::Shift::Off;
        self.before_symbols = None;
        true
    }


    /// Switch layout by a swipe sign (wraps, since there are only a couple). Pure index arithmetic —
    /// used by tests and by whatever drives layout switching (the sidebar's tap, or a future prev/next).
    pub fn swipe(&mut self, sign: i32) {
        let n = self.layouts.len() as i32;
        self.idx = (((self.idx as i32 + sign) % n + n) % n) as usize;
    }
    /// Jump directly to a layout by index (clamped) — what the vertical-tabs sidebar calls on a tap.
    pub fn set_layout(&mut self, idx: usize) {
        self.idx = idx.min(self.layouts.len().saturating_sub(1));
    }
    /// Layout names, in order, for the sidebar's tab labels.
    pub fn layout_names(&self) -> Vec<&str> {
        self.layouts.iter().map(|l| l.name.as_str()).collect()
    }
    pub fn layout_index(&self) -> usize { self.idx }

    /// The shift state, for the caller that needs to draw it.
    pub fn shift(&self) -> real_layout::Shift { self.shift }

    /// Tell the board the microphone is open, so the mic key can show it.
    pub fn set_voice_active(&mut self, on: bool) {
        self.voice_active = on;
    }

    pub fn voice_active(&self) -> bool {
        self.voice_active
    }

    /// A shift press: Off → Once → Caps → Off, the same cycle as the real board. One
    /// implementation of that cycle, in `real_layout::Shift`, so the two boards cannot
    /// disagree about what a second tap means.
    pub fn tap_shift(&mut self) {
        self.shift = self.shift.tap();
    }

    /// After text is emitted, a one-shot shift is spent. Caps is not.
    pub fn after_text(&mut self) {
        self.shift = self.shift.after_text();
    }

    /// `?123` / `ABC`. Returns to the layout it came from, so the key is the same one going
    /// both ways — a toggle that only goes one way is a trap on a phone, where the way back
    /// is a swipe nobody has been told about.
    pub fn toggle_symbols(&mut self) {
        let symbols = self.layouts.iter().position(|l| l.name == "symbols");
        match (self.before_symbols.take(), symbols) {
            (Some(back), _) => self.set_layout(back),
            (None, Some(sym)) if sym != self.idx => {
                self.before_symbols = Some(self.idx);
                self.set_layout(sym);
            }
            _ => {}
        }
    }

    /// The current layout with the shift state applied: letters uppercased, and the shift
    /// key itself carrying the latch so the cap shows which of the three states is on.
    ///
    /// Built on demand rather than stored, because the alternative is two copies of every
    /// layout that have to be kept in step — and the shifted one is only needed while a
    /// frame is being drawn.
    pub fn rendered(&self) -> std::borrow::Cow<'_, Layout> {
        // The spacebar's arrow swipes are attached at RENDER time, to whatever board is
        // showing — "in any layout" is the doc's wording, and five boards with five copies
        // of the same four gestures is how one of them ends up missing an arrow. Which also
        // means `rendered` can no longer take the borrowed shortcut.
        let mut l = self.current().clone();
        real_layout::attach_space_arrows(&mut l);
        if self.shift == real_layout::Shift::Off && !self.voice_active {
            return std::borrow::Cow::Owned(l);
        }
        for row in &mut l.rows {
            for k in &mut row.keys {
                if k.role == KeyRole::Modifier && crate::real_layout::classify(&k.emit) == real_layout::Press::Shift {
                    k.latch = match self.shift {
                        real_layout::Shift::Once => Latch::Once,
                        real_layout::Shift::Caps => Latch::Locked,
                        real_layout::Shift::Off => Latch::Off,
                    };
                } else if self.voice_active
                    && crate::real_layout::classify(&k.emit) == real_layout::Press::Voice
                {
                    k.latch = Latch::Live;
                } else if self.shift != real_layout::Shift::Off
                    && k.role == KeyRole::Normal
                    && k.emit.chars().all(|c| c.is_lowercase())
                    && !k.emit.is_empty()
                {
                    // Only a plain letter is upshifted. A digit, a symbol or an action token
                    // has no upper case, and uppercasing an action token would corrupt it.
                    k.label = k.label.to_uppercase();
                    k.emit = k.emit.to_uppercase();
                }
            }
        }
        std::borrow::Cow::Owned(l)
    }
    /// Read a committed swipe from the gesture layer (call each frame with the board rect).
    ///
    /// NOT used by the live app any more (see `vertical_tabs` in feel.rs): a whole-board swipe surface
    /// reads the same raw pointer trajectory that per-key cardinal swipes read, with nothing arbitrating
    /// between them — a swipe on a key's compass could also register as a page change, and since a
    /// cardinal key's gesture state is keyed by its on-screen rect, a layout change MID-swipe orphans
    /// that state (the id no longer matches anything drawn next frame). That's the intermittent
    /// "swipe doesn't register" failure. Kept (not deleted) in case a narrow dedicated bezel strip
    /// wants swipe-to-page later — just don't feed it the same surface a key renders in.
    pub fn poll_swipe(&mut self, ui: &egui::Ui, surface: Rect) {
        let frame = self.gesture.update(ui, surface, true);
        if let Some(dir) = frame.page_jump {
            self.swipe(dir.sign());
        }
    }

    /// Adopt cross-app preferences: theme + text/button size on the context, and the swipe
    /// feel (incl. detent animation) on the keyboard's layout-switch gesture. Keeps the
    /// keyboard consistent with the rest of the fleet when settings change live.
    pub fn apply_settings(&mut self, ctx: &egui::Context, prefs: &settings_kit::Prefs) {
        uikit_egui::apply_prefs(ctx, prefs);
        *self.gesture.config_mut() = prefs.gesture_config(320.0);
        self.palette = prefs.palette.clone();
    }

    /// The user's selected palette id (empty ⇒ default), for the render fn to colour keys with.
    pub fn palette(&self) -> &str { &self.palette }
}

impl Default for Board {
    fn default() -> Self {
        Board::new()
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
/// The appearance settings the board actually draws with.
///
/// `AppearanceConfig` has carried `key_radius`, `opacity` and `show_magnifier` since the GTK
/// build, `settings.rs` round-trips them, and the renderer read NONE of them: the radius was
/// `size.y * 0.18`, the board was opaque, and the magnifier I added last commit ignored its
/// own switch. A setting that persists and changes nothing is worse than one that is missing,
/// because the operator concludes the feature is broken rather than absent.
///
/// Separate from `AppearanceConfig` so this crate's renderer does not depend on the config
/// crate's shape, and so `Default` is the look the board had before any of this — which is
/// what keeps every existing call site drawing exactly what it drew.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Style {
    /// Key corner radius in points. `None` = the old proportional rule, which is the right
    /// default: a fixed radius looks wrong when the same board draws at phone and desktop
    /// key sizes.
    pub key_radius: Option<f32>,
    /// Board opacity, 0.05..=1.0.
    pub opacity: f32,
    /// Show the magnifier bubble on press.
    pub magnifier: bool,
    /// One-handed compaction: which side the board is anchored to.
    pub half: crate::halfboard::Side,
    /// Use the REACH POLYGON rather than one uniform inset: rows near the anchor keep
    /// nearly the full width, rows far from it give more up. `None` falls back to
    /// `halfboard`'s uniform inset, which is what every existing caller gets.
    pub arc: bool,
    /// A MEASURED reach, from `thumbcal`, replacing the guess `default_for` makes from the
    /// board's width. `None` keeps the width-based default — a hand that has never been
    /// measured is better served by an estimate than by nothing.
    pub calibrated_reach: Option<f32>,
}

impl Default for Style {
    fn default() -> Self {
        Style { calibrated_reach: None, key_radius: None, opacity: 1.0, magnifier: true, half: crate::halfboard::Side::Off, arc: true }
    }
}

impl Style {
    /// From the persisted appearance block.
    ///
    /// Clamped, because these arrive from a TOML file a person can edit: a radius of 400
    /// makes every key a circle with no glyph in it, and an opacity of 0 is a keyboard you
    /// cannot see but still type on — which is indistinguishable from the app having
    /// crashed.
    pub fn from_appearance(key_radius: f64, opacity: f64, magnifier: bool) -> Style {
        Style {
            key_radius: Some((key_radius as f32).clamp(0.0, 24.0)),
            opacity: (opacity as f32).clamp(0.05, 1.0),
            magnifier,
            // Not persisted: which hand is holding the phone is a right-now fact, not a
            // preference. A keyboard that comes up compacted to the left because that is
            // how you held it yesterday is a keyboard you have to un-compact.
            half: crate::halfboard::Side::Off,
            arc: true,
            calibrated_reach: None,
        }
    }

    /// Fold the settings panel's values onto a base style.
    ///
    /// One place, so a new setting cannot be half-applied. The panel used to special-case
    /// `layout.experimental_mode` inline at the button, and a second special case beside it
    /// is how the two drift — one applied on change, the other only at startup.
    ///
    /// Unknown keys and unknown values are ignored rather than reset to a default: a config
    /// written by a newer build must not silently undo a setting this build does understand.
    pub fn with_settings(mut self, values: &std::collections::HashMap<String, String>) -> Style {
        if let Some(h) = values.get("layout.hand") {
            self.half = match h.as_str() {
                "left" => crate::halfboard::Side::Left,
                "right" => crate::halfboard::Side::Right,
                "off" => crate::halfboard::Side::Off,
                _ => self.half,
            };
        }
        self
    }

    /// The radius for a key of this height.
    pub fn radius_for(&self, key_h: f32) -> u8 {
        match self.key_radius {
            Some(r) => r.clamp(0.0, key_h * 0.5).round() as u8,
            None => (key_h * 0.18) as u8,
        }
    }
}

pub fn keyboard_ui(ui: &mut Ui, layout: &Layout, adaptive: &Adaptive, palette: &str) -> Option<String> {
    keyboard_ui_styled(ui, layout, adaptive, palette, Style::default())
}

/// [`keyboard_ui`], drawn with explicit [`Style`].
pub fn keyboard_ui_styled(ui: &mut Ui, layout: &Layout, adaptive: &Adaptive, palette: &str, style: Style) -> Option<String> {
    let colors = uikit_egui::palette_colors(palette);
    let glass = palette == "system"; // the shared system-utilities look (gitmind/rusty-board): glass keys
    let gutter = adaptive.gutter;
    // ONE-HANDED COMPACTION. The keys are laid into a narrower rect anchored to one side;
    // everything else about the layout is unchanged, so every key keeps its relative place
    // and the far edge comes within a thumb's reach. `Side::Off` returns the full rect, so
    // this is a no-op for every caller that has not asked for it. See `halfboard`.
    let full_rect = ui.max_rect();
    // WHERE EVERY KEY GOES, decided once by a pure function that `hit_test` and the swipe
    // recognizer's hitboxes also use -- see `key_rects`.
    let rects = key_rects(layout, full_rect, adaptive, &style);
    let board_bottom = rects
        .iter()
        .rev()
        .find_map(|r| r.first())
        .map(|r| r.bottom() + gutter)
        .unwrap_or(full_rect.top());

    if glass {
        // Glass root surface: high-alpha tinted fill + a single top hairline highlight, matching the
        // ported rusty-board-GTK `.glass-keyboard` recipe (no gradient/blur — see uikit_egui::glass).
        // `.keyboard-view { border-radius: 16px 16px 0 0 }` — the board's top corners are
        // rounded and its bottom ones are not, because it is anchored to the screen edge.
        // The port drew a plain rectangle, which is the one visual difference you notice
        // before any key: the GTK board reads as a panel that has slid up, this read as a
        // screen that had been cut in half.
        let board_rect = ui.max_rect();
        // `--rb-keyboard-opacity`: the board's own fill, not the whole surface. Fading the
        // KEYS as well would make a translucent keyboard unreadable rather than see-through
        // — the setting is about letting the app behind show past the board, which is what
        // the GTK rule does (`alpha(@view_bg_color, var(--rb-keyboard-opacity))`).
        let base = uikit_egui::surface_fill(colors.bg_base);
        let base = egui::Color32::from_rgba_unmultiplied(
            base.r(),
            base.g(),
            base.b(),
            (base.a() as f32 * style.opacity).round().clamp(0.0, 255.0) as u8,
        );
        ui.painter().rect_filled(
            board_rect,
            egui::CornerRadius { nw: BOARD_RADIUS, ne: BOARD_RADIUS, sw: 0, se: 0 },
            base,
        );
        uikit_egui::paint_top_hairline(ui, board_rect);
    }

    let mut emitted = None;
    for (row_i, row) in layout.rows.iter().enumerate() {
        for (col, key) in row.keys.iter().enumerate() {
            let rect = rects[row_i][col];
            if let Some(e) = paint_key(ui, key, rect, (row_i, col), row_i == 0, &colors, glass, layout.hints, style) {
                emitted = Some(e);
            }
        }
    }
    // The board occupies exactly its rows. Nothing below the last row is claimed, so a
    // caller that stacks something under the keys starts where the keys end.
    ui.advance_cursor_after_rect(Rect::from_min_max(full_rect.min, egui::pos2(full_rect.max.x, board_bottom)));
    emitted
}

/// The widest the keys get, in points. `MAX_KEYBOARD_WIDTH` in the GTK keyboard
/// (`src/ui/keyboard_view.rs`): wider screens centre a 720pt board instead of stretching
/// every key into a flat slab. The egui port stretched to the window, which on a desktop
/// window made the board read as a different keyboard from the GTK one.
pub const MAX_BOARD_W: f32 = 720.0;

/// GTK's default key height (`keyboard_view.rs`: `key_h = 46`), used for every input
/// modality. See [`board_adaptive`].
pub const KEY_H: f32 = 46.0;

/// GTK's gap between keys (`Box::spacing(4)`), used between rows as well.
pub const KEY_GAP: f32 = 4.0;

/// The adaptive profile the KEYBOARD is laid out with: the detected device, with the key
/// height and gap pinned to GTK's.
///
/// Pinned because the detected target depends on input modality -- 28pt for a pointer,
/// 44pt for touch -- and a keyboard whose key height changes with how you touch it is two
/// keyboards. That was the "rows move on tap" report: on a tablet-width screen the rows
/// grew when a finger landed. `Adaptive::detect` now latches touch, but even latched the
/// FIRST tap would reflow the board, and on a desktop the 28pt keys were the other half of
/// "the layout feels strange" next to GTK's 46.
pub fn board_adaptive(ctx: &egui::Context) -> Adaptive {
    let mut a = Adaptive::detect(ctx);
    a.min_touch_target = KEY_H;
    a.gutter = KEY_GAP;
    a
}

/// Every key's rect, `[row][col]`, for a board drawn into `full` -- THE geometry.
///
/// One function for the painter ([`keyboard_ui_styled`]), the tap hit-test ([`hit_test`]) and
/// the swipe recognizer's hitboxes (`swipe::SwipeTracker::hitboxes`). Before this the painter
/// laid keys out with egui's flow layout and the other two re-derived the walk by hand, and
/// they had drifted: the hand walks advanced `x += w` with no gap between keys, and did not
/// know about the parent's `item_spacing.y` between rows, so the swipe decoder's idea of
/// where a key was slid a gutter further off per key and ~3pt per row. Placing keys at
/// computed rects makes the three agree by construction.
///
/// Rules: the board is centred at no more than [`MAX_BOARD_W`]; a one-handed style narrows
/// each row (per row, when the thumb arc is on); rows are `key_h` tall and `key_h + gutter`
/// apart from the top; each row has a gutter at both ends and between keys, and the keys
/// share the rest by weight.
pub fn key_rects(layout: &Layout, full: Rect, adaptive: &Adaptive, style: &Style) -> Vec<Vec<Rect>> {
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target;
    let full = board_area(full);
    // ONE-HANDED COMPACTION. The keys are laid into a narrower rect anchored to one side;
    // every key keeps its relative place and the far edge comes within a thumb's reach.
    // `Side::Off` returns the full rect. See `halfboard`.
    let keys_rect = style.half.board_rect(full);
    // THE REACH POLYGON, per row. `halfboard` insets every row equally, which gives away
    // the shape -- the bottom row loses width it did not need to. The doc's algorithm is
    // per-ROW ("only the row's outer bounds shift; muscle memory is preserved").
    let arc = if style.arc {
        style
            .half
            .is_on()
            .then(|| crate::thumbarc::Corner::from_side(style.half))
            .flatten()
            .and_then(|c| match style.calibrated_reach {
                // MEASURED BEATS ESTIMATED.
                Some(r) => Some(crate::thumbarc::Arc::default_shape(c, r)),
                None => crate::thumbarc::Arc::default_for(full.width(), c),
            })
    } else {
        None
    };
    let rows_n = layout.rows.len();
    layout
        .rows
        .iter()
        .enumerate()
        .map(|(row_i, row)| {
            let (left, row_w) = match &arc {
                Some(a) => {
                    let (lo, hi) = a.row_bounds(row_i, rows_n);
                    (full.left() + full.width() * lo, full.width() * (hi - lo))
                }
                None => (keys_rect.left(), keys_rect.width()),
            };
            let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
            let usable = (row_w - gutter * (row.keys.len() as f32 + 1.0)).max(0.0);
            let y = full.top() + row_i as f32 * (key_h + gutter);
            let mut x = left + gutter;
            row.keys
                .iter()
                .map(|key| {
                    let w = (usable * (key.weight / total_weight)).max(8.0);
                    let r = Rect::from_min_size(egui::pos2(x, y), Vec2::new(w, key_h));
                    x += w + gutter;
                    r
                })
                .collect()
        })
        .collect()
}

/// The part of `full` the keys are laid out in: all of it, or a [`MAX_BOARD_W`] column
/// centred in it. Anything that places things AROUND the keys (the space zones) uses this
/// rather than the raw rect, or on a wide window it would put them under keys.
pub fn board_area(full: Rect) -> Rect {
    if full.width() > MAX_BOARD_W {
        Rect::from_center_size(full.center(), Vec2::new(MAX_BOARD_W, full.height()))
    } else {
        full
    }
}

/// The height [`keyboard_ui_styled`] occupies for `layout`: every row plus a gutter under
/// each. What a shell sizes its surface to, so no strip of dead space sits under the keys.
pub fn board_height(layout: &Layout, adaptive: &Adaptive) -> f32 {
    layout.rows.len() as f32 * (adaptive.min_touch_target + adaptive.gutter)
}

/// Same as `keyboard_ui`, but also routes the tapped key's emit code through `registry` under the
/// key's own id (the same `SignalRegistry<()>` shape rusty-board's GTK original uses via
/// `signal_kit::Signal<Key>` on `KeyWidget` — see key_widget.rs). Additive: `keyboard_ui` itself
/// is untouched (existing callers reading its `Option<String>` return keep working unchanged);
/// this is for a caller that wants shared app logic (something both the GTK and egui front-ends
/// can subscribe to identically) to also see key presses, without threading the return value
/// through by hand at every call site.
/// ## Not called yet, and that is not an oversight
///
/// `wiring_check` lists this as a renderer nothing calls. It is an ALTERNATIVE
/// to `keyboard_ui`, not a missing surface: `demo_app` calls the plain one and
/// gets the pressed key back directly, which is all a single front-end needs.
///
/// The signal shape exists because the GTK side really uses it —
/// `src/ui/key_widget.rs` holds a `signal_kit::Signal<Key>`, with its own
/// comment noting the two binaries follow "the same `Signal<Key>` pattern even
/// though they're separate binaries, not a live shared bus". This function is
/// what lets an egui caller subscribe identically when there IS shared app
/// logic to subscribe.
///
/// Wiring it today would mean inventing a subscriber: nothing in this app owns
/// a `SignalRegistry`, and a registry with one publisher and no listener is
/// indirection, not architecture. Left for the caller that needs it.
pub fn keyboard_ui_with_signal(
    ui: &mut Ui,
    layout: &Layout,
    adaptive: &Adaptive,
    palette: &str,
    registry: &signal_kit::SignalRegistry<()>,
) -> Option<String> {
    let emitted = keyboard_ui(ui, layout, adaptive, palette);
    if let Some(id) = &emitted {
        registry.emit(id, ());
    }
    emitted
}

/// Paint one key as a carved rounded tile with a centered label; returns true on tap.
///
/// When `glass` is set (the System palette — gitmind/rusty-board's shared "system utilities" look), keys
/// use the ported rusty-board-GTK glass recipe (`uikit_egui::glass`): high-alpha tinted fill, hairline
/// border, instant colour-flip on press — no gradient/sheen, no blur. Otherwise keys use the standard
/// carved-tile motion glide.
/// Reserved token prefix for a steno key's tap/swipe VALUE (as opposed to
/// its displayed phoneme label). `\u{0}` can't occur in real layout text —
/// same reservation trick `real_layout::classify`'s `ACT` prefix uses, kept
/// separate (`"steno:"` vs `"act:"`) so the two decoders never collide.
const STENO_TOKEN_PREFIX: &str = "\u{0}steno:";

/// Encode a steno key's resolved gesture as a value string carrying the
/// button id and gesture code, instead of the ambiguous phoneme text.
///
/// The GTK reference never needs this: it dispatches from the typed
/// `KeyAction::Steno` + the classified swipe direction directly, in the
/// same function call, so there's no round trip through committed text at
/// all. This crate's render layer commits by returning a plain `String`
/// from `paint_key` that a later `classify()` call re-interprets — so
/// identity has to survive IN that string. It can't be the phoneme itself:
/// `default_stenotype()` puts `"n"` on both L1's north swipe and R2's
/// north swipe (see `crates/board-layouts/src/lib.rs`), so two different
/// (button, direction) pairs produce the same letter and recovering one
/// from the other is not possible after the fact.
pub(crate) fn steno_token(button_id: &str, gesture_code: &str) -> String {
    format!("{STENO_TOKEN_PREFIX}{button_id}:{gesture_code}")
}

/// Decode a `steno_token` back into `(button_id, gesture_code)`. Exposed so
/// `real_layout::classify` (a different module, deliberately not sharing
/// its own `ACT` reserved-prefix constant with this one) can recognize the
/// same tokens `paint_key` produces.
pub fn decode_steno_token(value: &str) -> Option<(&str, &str)> {
    let rest = value.strip_prefix(STENO_TOKEN_PREFIX)?;
    rest.split_once(':')
}

/// This crate's stable direction code for a steno swipe. Only N/E/S/W are
/// reachable — `default_stenotype()` never sets a diagonal — but the match
/// is total so a future diagonal doesn't silently vanish into `_`.
pub(crate) fn dir_code(d: uikit_egui::slide_menu::Dir8) -> &'static str {
    use uikit_egui::slide_menu::Dir8;
    match d {
        Dir8::N => "N",
        Dir8::E => "E",
        Dir8::S => "S",
        Dir8::W => "W",
        Dir8::NE => "NE",
        Dir8::SE => "SE",
        Dir8::SW => "SW",
        Dir8::NW => "NW",
    }
}

/// Bridge the classifier's `Direction` to the widget's `Dir8`.
///
/// Two enums for one idea, in two crates that should not depend on each other:
/// `cardinal_gesture` is rusty-board's ported classifier and `Dir8` belongs to
/// the shared widget kit. A `From` impl would need one to know about the other.
fn dir8_from(d: cardinal_gesture::Direction) -> uikit_egui::slide_menu::Dir8 {
    use cardinal_gesture::Direction as D;
    use uikit_egui::slide_menu::Dir8 as U;
    match d {
        D::N => U::N, D::NE => U::NE, D::E => U::E, D::SE => U::SE,
        D::S => U::S, D::SW => U::SW, D::W => U::W, D::NW => U::NW,
    }
}

/// The id a cardinal key's press state lives under.
///
/// Factored out because BOTH halves of the key need it and they must agree:
/// `paint_key_face` peeks at the press to draw the morph, and `dispatch_key`
/// drives the same press to resolve it. Two id expressions is one rename away
/// from a cap that animates nothing while the gesture works fine, which is a
/// bug with no error message.
fn cardinal_id(ui: &Ui, key: &Key, rect: Rect) -> egui::Id {
    ui.id().with(("keycard", &key.label, rect.min.x as i32, rect.min.y as i32))
}

/// The tap/swipe boundary, derived from the key so a phone key and a desktop
/// key both flick at a quarter of their height. Shared by the widget, the
/// classifier and the morph — see `dispatch_key`'s note on the interlock.
fn key_deadzone(size: Vec2) -> f32 {
    (size.y * 0.25).clamp(9.0, 16.0)
}

#[allow(clippy::too_many_arguments)]
fn paint_key(ui: &mut Ui, key: &Key, rect: Rect, at: (usize, usize), top_row: bool, c: &PaletteColors, glass: bool, hints: bool, style: Style) -> Option<String> {
    // CLICK AND DRAG. This was `Sense::click()`, which is why cardinal swipes
    // did nothing: a key that only senses clicks never sees a drag, so the
    // direction could not be measured however good the classifier was.
    //
    // AT A COMPUTED RECT, under an id from the key's (row, col). Allocated by egui's flow
    // layout, a key's place depended on everything the Ui had laid out before it; placed,
    // it is exactly where `key_rects` -- and therefore `hit_test` and the swipe hitboxes --
    // say it is.
    let size = rect.size();
    let resp = ui.interact(rect, ui.id().with(("key", at.0, at.1)), Sense::click_and_drag());
    // PAINT, then DISPATCH — two steps, and the first has no return in it.
    //
    // The glass recipe used to be a control-flow branch that painted the key
    // and then `return resp.clicked()`-ed — so under the "system" palette
    // (which is `Board::new()`'s default) the cardinal, hold-variant and
    // steno paths below were unreachable, and every cardinal key was a plain
    // click. That was the state bug behind "swipes don't register on the
    // experimental keyboards": the classifier, the widget and the layouts
    // were all fine, and none of them was being called. A palette chooses
    // how a key LOOKS; it never gets to decide what a gesture MEANS.
    paint_key_face(ui, key, rect, size, top_row, c, glass, hints, &resp, style);
    dispatch_key(ui, key, rect, size, &resp)
}


/// Does this key deserve a magnifier when pressed?
///
/// Character keys only. On a phone your thumb covers the key you are pressing, and the
/// magnifier is the only way to see what went in — but magnifying "shift" or "?123" tells
/// you nothing you did not already know from the key you deliberately aimed at, and a
/// bubble over the space bar is a bubble over half the board.
pub fn wants_magnifier(key: &Key) -> bool {
    if key.role != KeyRole::Normal || key.latch != Latch::Off {
        return false;
    }
    let g = key.label.trim();
    // One visible glyph. A wide key with a word on it ("space") is not a character.
    !g.is_empty() && g.chars().count() == 1 && !g.chars().all(char::is_whitespace)
}

/// Where the magnifier sits: centred over the key, floated above it, kept inside `bounds`.
///
/// Clamped horizontally because the outermost keys are exactly the ones whose bubble would
/// otherwise hang off the screen edge — and on a phone the board IS the screen, so there is
/// nothing beside it to hang over. Clamped upward too: on the top row the bubble would
/// float off the board, so it is allowed to overlap the key it belongs to rather than
/// disappear.
pub fn magnifier_rect(key: Rect, bounds: Rect, ppp: f32) -> Rect {
    let w = (key.width() * 1.6).max(MAGNIFIER_MIN_W);
    let h = key.height() * 1.25;
    let gap = 6.0;
    let mut r = Rect::from_center_size(
        egui::pos2(key.center().x, key.top() - gap - h * 0.5),
        egui::vec2(w, h),
    );
    if r.left() < bounds.left() {
        r = r.translate(egui::vec2(bounds.left() - r.left(), 0.0));
    }
    if r.right() > bounds.right() {
        r = r.translate(egui::vec2(bounds.right() - r.right(), 0.0));
    }
    if r.top() < bounds.top() {
        r = r.translate(egui::vec2(0.0, bounds.top() - r.top()));
    }
    uikit_egui::crisp::snap_rect(r, ppp)
}

/// `.key-magnifier { min-width }` — a bubble narrower than this reads as a tooltip.
pub const MAGNIFIER_MIN_W: f32 = 48.0;
/// `.key-magnifier { font-size: 28px }`.
pub const MAGNIFIER_PT: f32 = 28.0;

/// The mic key's alpha while recording, from the frame clock.
///
/// A full sine between [`LIVE_MIN`] and 255 once per [`LIVE_PERIOD`]. It never reaches zero
/// — a pulse that fades out entirely reads as a key that has turned OFF at the bottom of
/// every cycle, which is the opposite of what it is saying.
pub fn live_pulse(time: f64) -> u8 {
    let phase = (time / LIVE_PERIOD) * std::f64::consts::TAU;
    let t = (phase.sin() * 0.5 + 0.5) as f32; // 0..1
    (LIVE_MIN as f32 + t * (255.0 - LIVE_MIN as f32)).round().clamp(0.0, 255.0) as u8
}

/// Seconds per pulse. Slow enough to read as breathing rather than blinking.
pub const LIVE_PERIOD: f64 = 1.6;
/// The dimmest the mic key gets. Never invisible.
pub const LIVE_MIN: u8 = 90;

/// `.keyboard-view { border-radius: 16px 16px 0 0 }`.
pub const BOARD_RADIUS: u8 = 16;

/// `.key-button:active { transform: scale(0.94) }` from the GTK stylesheet.
pub const PRESS_SCALE: f32 = 0.94;

/// Shrink a rect about its own centre, so a pressed key moves without nudging its
/// neighbours (a layout that reflows under a thumb is worse than one that does not move).
fn shrink_about_centre(rect: Rect, k: f32) -> Rect {
    Rect::from_center_size(rect.center(), rect.size() * k)
}

/// The glyph on the cap: the resting label, or the morph mid-swap.
///
/// One helper rather than the same three lines in each palette branch, because
/// a palette chooses how a key LOOKS and never gets to decide what a gesture
/// means — the same rule that made the cardinal path unreachable under the
/// glass recipe once already.
fn paint_cap_label(
    painter: &egui::Painter,
    rect: Rect,
    label: &str,
    morph: Option<(uikit_egui::slide_menu::Dir8, &str, f32)>,
    font: FontId,
    color: egui::Color32,
) {
    match morph {
        Some((dir, incoming, progress)) => uikit_egui::slide_menu::paint_cardinal_morph(
            painter, rect, label, incoming, dir, progress, font, color,
        ),
        None => {
            painter.text(rect.center(), Align2::CENTER_CENTER, label, font, color);
        }
    }
}

/// The visual half of a key: glass or carved-tile, pressed/hovered/at-rest. No gesture logic.
#[allow(clippy::too_many_arguments)]
fn paint_key_face(ui: &mut Ui, key: &Key, rect: Rect, size: Vec2, top_row: bool, c: &PaletteColors, glass: bool, hints: bool, resp: &egui::Response, style: Style) {
    // What the in-flight swipe (if any) is currently arming. Peeked, not
    // driven: `dispatch_key` still owns the press state, and it runs after
    // this. On the first frame of a press there is nothing to peek yet and the
    // cap paints at rest, which is one frame and not perceptible.
    let preview = if key.cardinal.is_empty() {
        None
    } else {
        uikit_egui::slide_menu::peek_cardinal(ui.ctx(), cardinal_id(ui, key, rect), key_deadzone(size))
    };
    // Only an ARMED direction that this key actually carries morphs the cap.
    // A swipe toward a direction with no variant leaves the resting glyph
    // alone rather than fading it out to nothing.
    let morph: Option<(uikit_egui::slide_menu::Dir8, &str, f32)> = preview.and_then(|p| {
        let d = p.active?;
        let s = key.cardinal.iter().find(|(k, _)| *k == d).map(|(_, s)| s.as_str())?;
        Some((d, s, p.progress))
    });
    // What the incoming glyph SHOWS: a value can be an action token (the spacebar's
    // arrows), which is for the dispatcher and must never be painted as text.
    let morph_label = morph.map(|(d, s, p)| (d, real_layout::display_label(s), p));
    let morph = morph_label.as_ref().map(|(d, s, p)| (*d, s.as_ref(), *p));
    let painter = ui.painter();
    let pressed = resp.is_pointer_button_down_on();
    // `--rb-key-radius`, when the operator has set one. The proportional default stays
    // the default: a fixed radius looks wrong when one board draws at both phone and
    // desktop key sizes.
    let rounding = CornerRadius::same(style.radius_for(size.y));
    // ON THE PIXEL GRID. A cap font of `key_height * 0.42` is 18.48pt on a 44pt key, and at
    // a 1.25 or 1.5 desktop scale that is 23.1 or 27.72 DEVICE pixels — a glyph with no
    // whole-pixel stem in it, which reads as soft and slightly tall. It also made egui
    // re-rasterise the whole alphabet whenever a key changed height by a fraction, since
    // its glyph atlas is keyed on the exact size. See `uikit_egui::crisp`.
    let ppp = ui.ctx().pixels_per_point();
    // `.key-label { font-size: 16px }` on GTK's 46px key: 0.35 of the height. This was
    // 0.42 (19.3pt on a 46pt key), which is most of why the port's caps looked heavier
    // than GTK's side by side.
    let font = FontId::proportional(uikit_egui::crisp::snap_font_pt(
        (size.y * 0.35).clamp(11.0, 26.0),
        ppp,
    ));
    // And the cap itself: a 1px border at x = 40.3 covers 70% of one column and 30% of the
    // next — two grey columns instead of one line, on every key at once.
    let rect = uikit_egui::crisp::snap_rect(rect, ppp);
    // THE PRESS SCALES on the carved cap, as GTK's `.key-button:active { transform:
    // scale(0.94) }` does -- about its own centre so neighbours do not shift. NOT under
    // glass: GTK's glass rules (`keyboard.css`, the glass-mode block) set no transform, the
    // cap only flips colour, and glass is this port's default palette. And never on a
    // cardinal key, in either palette: its cap is something you READ while dragging (the
    // label slides, see `paint_cardinal_morph`), and a cap that shrinks under the finger
    // and grows back on release was the "cardinal keys change size on tap and on flick"
    // report.
    let scales = pressed && !glass && key.cardinal.is_empty();
    let rect = if scales { shrink_about_centre(rect, PRESS_SCALE) } else { rect };
    if glass {
        // THE ROLE, from the layout file, not from the label's length. `.key-action` is
        // accent-filled with white text (Enter, the key that commits — findable without
        // reading it), `.key-modifier` is a touch lighter than the surface, `.key-special`
        // is fainter still. Guessing `label.chars().count() > 1` made "space", "enter",
        // "shift" and "?123" one look, and lost the accent cap entirely.
        let (fill, stroke) = if pressed {
            (uikit_egui::glass_tint(c.accent, uikit_egui::GlassAlphas::PRESSED_ACCENT), uikit_egui::key_border(c.border))
        } else if key.latch == Latch::Locked {
            // .key-modifier.shift-locked: solid accent. The loudest cap on the board.
            (c.accent, uikit_egui::key_border(c.accent))
        } else if key.latch == Latch::Live {
            // .key-special.voice-active: accent, PULSING. A static highlight is easy to
            // stop seeing; a live microphone should keep asking to be noticed.
            let a = live_pulse(ui.input(|i| i.time));
            ui.ctx().request_repaint();
            (
                egui::Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), a),
                uikit_egui::key_border(c.accent),
            )
        } else if key.latch == Latch::Once {
            // .key-modifier.shift-active: accent at 0.25 with an accent border.
            (
                egui::Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 64),
                Stroke::new(1.0_f32, egui::Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), 128)),
            )
        } else if key.role == KeyRole::Action {
            // .key-action: accent at 0.85, going solid on hover.
            let a = if resp.hovered() { 255 } else { 217 };
            (
                egui::Color32::from_rgba_unmultiplied(c.accent.r(), c.accent.g(), c.accent.b(), a),
                uikit_egui::key_border(c.accent),
            )
        } else if key.role == KeyRole::Modifier || key.role == KeyRole::Special {
            (if resp.hovered() { uikit_egui::key_fill_modifier_hover(c.bg_card) } else { uikit_egui::key_fill_modifier(c.bg_card) }, uikit_egui::key_border_soft(c.border))
        } else {
            (if resp.hovered() { uikit_egui::key_fill_hover(c.bg_card) } else { uikit_egui::key_fill(c.bg_card) }, uikit_egui::key_border(c.border))
        };
        painter.rect(rect, rounding, fill, stroke, egui::StrokeKind::Inside);
        // The top row, by index: this compared `rect.top()` to the gutter, which is only the
        // top row when the board is drawn at y = 0 -- under a panel or a status strip it was
        // no row at all.
        if top_row {
            uikit_egui::paint_top_hairline(ui, rect);
        }
        // .key-action's label is white at rest too — that is what makes it read as the
        // commit key rather than as another grey cap.
        let text_color = if pressed
            || key.role == KeyRole::Action
            || key.latch == Latch::Locked
            || key.latch == Latch::Live
        {
            egui::Color32::WHITE
        } else if key.latch == Latch::Once {
            c.accent
        } else if key.role == KeyRole::Special {
            c.text.gamma_multiply(0.85)
        } else {
            c.text
        };
        paint_cap_label(painter, rect, &key.label, morph, font.clone(), text_color);
        // THE MAGNIFIER. On a phone the thumb covers the key it is pressing, so this bubble
        // is the only way to see what went in — `.key-magnifier` in the GTK stylesheet, and
        // absent from this port entirely. Drawn on a FOREGROUND layer: at board scale it
        // overlaps the row above, and a magnifier a neighbouring key can paint over is a
        // magnifier that vanishes exactly when the board is busiest.
        if pressed && style.magnifier && wants_magnifier(key) {
            let bounds = ui.clip_rect();
            let bub = magnifier_rect(rect, bounds, ppp);
            let top = ui.ctx().layer_painter(egui::LayerId::new(
                egui::Order::Foreground,
                ui.id().with("magnifier"),
            ));
            // `box-shadow: 0 4px 12px alpha(black, 0.25)` — painted directly rather than
            // through egui's popup shadow, which belongs to a Window and this is not one.
            let shadow = egui::epaint::Shadow {
                offset: [0, 4],
                blur: 12,
                spread: 0,
                color: egui::Color32::from_black_alpha(64),
            };
            top.add(shadow.as_shape(bub, CornerRadius::same(12)));
            top.rect(
                bub,
                CornerRadius::same(12),
                c.bg_card,
                Stroke::new(1.0, uikit_egui::key_border(c.border).color),
                egui::StrokeKind::Inside,
            );
            top.text(
                bub.center(),
                egui::Align2::CENTER_CENTER,
                &key.label,
                FontId::proportional(uikit_egui::crisp::snap_font_pt(MAGNIFIER_PT, ppp)),
                c.text,
            );
        }
    } else {
        // Rest→hover→press glides (120ms) instead of snapping — the QML key ColorAnimation feel.
        let fill = uikit_egui::motion::interactive_fill(ui.ctx(), resp, c.bg_raised, c.bg_hover, c.accent);
        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(rect, rounding, Stroke::new(1.0_f32, c.bg_base), egui::StrokeKind::Inside);
        let text_color = if pressed { c.bg_base } else { c.text };
        paint_cap_label(painter, rect, &key.label, morph, font.clone(), text_color);
    }
    // THE SECOND LAYER, visible: a key's first long-press character small in its top-right
    // corner (Gboard's hint), so the layer is found by looking rather than by holding every
    // key. Not where compass glyphs are drawn at rest -- the two would collide in the corner.
    if !key.variants.is_empty() && !(hints && !key.cardinal.is_empty()) {
        // 10pt floor: nothing on a cap is smaller (every_cap_label_is_rasterised_on_the_pixel_grid).
        // Snapped to whole device pixels, and the floor applied AFTER snapping: 10pt at 1.25x is
        // 12.5px, which snaps down to 12px = 9.6pt -- so round up to the next whole pixel.
        // GTK's hint labels are 11px (`theme.rs`): 0.24 of a 46pt key.
        let pt = uikit_egui::crisp::snap_font_pt((size.y * 0.24).clamp(10.0, 12.0), ppp);
        let pt = if pt < 10.0 { (10.0 * ppp).ceil() / ppp } else { pt };
        let hint = FontId::proportional(pt);
        painter.text(
            rect.right_top() + egui::vec2(-size.y * 0.12, size.y * 0.08),
            egui::Align2::RIGHT_TOP,
            &key.variants[0],
            hint,
            c.text.gamma_multiply(0.5),
        );
    }
    // The compass hints, drawn small around the tap label so a cardinal key advertises its
    // swipes at rest (FlickBoard/MessagEase draw every direction's glyph in its corner). Only the
    // four cardinals ride the edges; diagonals sit in their corners.
    if hints && !key.cardinal.is_empty() {
        let hint = FontId::proportional(uikit_egui::crisp::snap_font_pt(
            (size.y * 0.24).clamp(8.0, 13.0),
            ppp,
        ));
        let hint_color = c.text.gamma_multiply(0.55);
        let inset = size.y * 0.16;
        for (d, s) in &key.cardinal {
            // The armed direction's hint is suppressed as its glyph travels to
            // the centre: leaving it printed shows the same letter twice on a
            // cap this small, which reads as a rendering fault rather than as
            // motion. The others dim, so the one in play is the only thing at
            // full weight.
            let (skip, dim) = match morph {
                Some((md, _, p)) if md == *d => (p > 0.35, 1.0),
                Some((_, _, p)) => (false, 1.0 - 0.55 * p),
                None => (false, 1.0),
            };
            if skip {
                continue;
            }
            let off = d.offset();
            let anchor = rect.center() + Vec2::new(off.x * (rect.width() * 0.5 - inset), off.y * (rect.height() * 0.5 - inset));
            painter.text(anchor, Align2::CENTER_CENTER, real_layout::display_label(s), hint.clone(), hint_color.gamma_multiply(dim));
        }
    }
}

/// The gesture half of a key: which string this press/drag/hold resolves to, if any. Runs for
/// every palette. Cardinal keys go through the compass widget (direction) plus the ported
/// classifier (hairpin = capitalise, rotation = digit); plain keys through hold-variants or a
/// click.
fn dispatch_key(ui: &mut Ui, key: &Key, rect: Rect, size: Vec2, resp: &egui::Response) -> Option<String> {
    // Cardinal-swipe key (MessagEase-style): tap emits `emit`, a swipe in a compass direction emits
    // that direction's variant. `show_cardinal` resolves tap-vs-swipe from the PRESS POINT.
    if !key.cardinal.is_empty() {
        let id = cardinal_id(ui, key, rect);
        // ONE tap/swipe boundary for the widget and the classifier. The classifier's module doc
        // calls this an interlock: if the two disagree, a movement is a tap to one and a swipe to
        // the other, and the key fires twice or not at all. Derived from the key size so a phone
        // key and a desktop key both flick at a quarter of their height (FlickBoard's default
        // swipe threshold is 8dp — short flicks are the point of this layout).
        let deadzone = key_deadzone(size);
        // ENGAGED = something can come out of this key this frame: it is pressed, dragging,
        // releasing, or its compass is still open from an earlier frame. A key that is none
        // of those (thirty-nine of the forty, on any frame) returns here, before the option
        // strings below are built and before the widget is consulted. This was the
        // per-frame cost of a cardinal board: nine small allocations per key per frame for
        // keys nobody was touching.
        let engaged = resp.is_pointer_button_down_on()
            || resp.dragged()
            || resp.drag_stopped()
            || resp.clicked()
            || uikit_egui::slide_menu::SlideMenu::is_cardinal_open(ui, id);
        if !engaged {
            return None;
        }
        let thresholds = cardinal_gesture::CardinalThresholds::default()
            .scaled(deadzone as f64 / cardinal_gesture::CardinalThresholds::default().tap_max_excursion_px);
        // Steno keys (L1/L2/R1/R2): the tap/direction resolution is identical to a normal cardinal
        // key, but the VALUE each option carries must be an unambiguous (button, gesture) token
        // rather than the phoneme text — see `steno_token`'s doc comment. The displayed LABEL
        // still shows the phoneme; only the returned value differs.
        let tap_id = match &key.steno_button {
            Some(btn) => steno_token(btn, "tap"),
            None => key.emit.clone(),
        };
        let tap = uikit_egui::slide_menu::SlideOption::new(tap_id, key.label.clone());
        let dirs: Vec<(uikit_egui::slide_menu::Dir8, uikit_egui::slide_menu::SlideOption)> = key
            .cardinal
            .iter()
            .map(|(d, s)| {
                let opt_id = match &key.steno_button {
                    Some(btn) => steno_token(btn, dir_code(*d)),
                    None => s.clone(),
                };
                (*d, uikit_egui::slide_menu::SlideOption::new(opt_id, s.clone()))
            })
            .collect();
        // HAIRPIN / ROTATION TRACKING, alongside show_cardinal rather than replacing it.
        // `show_cardinal` owns direction; it has no concept of out-and-back (capitalisation) or
        // rotation (digits). The path is sampled here and classified on release, and a claim
        // from the classifier OVERRIDES the widget's answer for that release. Steno keys skip
        // this: GTK's dispatcher does the same ("Hairpin/rotation/diagonals: not used by steno"),
        // and the hairpin arm's `to_uppercase` would mangle a steno token.
        let mut claimed: Option<String> = None;
        if key.steno_button.is_none() {
            let path_id = id.with("hairpin");
            if resp.drag_started() {
                if let Some(p) = ui.ctx().input(|i| i.pointer.press_origin()).or_else(|| resp.interact_pointer_pos()) {
                    let t = (ui.input(|i| i.time) * 1000.0) as u32;
                    ui.ctx().data_mut(|d| d.insert_temp(path_id, vec![(p.x, p.y, t)]));
                }
            }
            if resp.dragged() {
                if let Some(p) = resp.interact_pointer_pos() {
                    let t = (ui.input(|i| i.time) * 1000.0) as u32;
                    ui.ctx().data_mut(|d| {
                        let mut v: Vec<(f32, f32, u32)> = d.get_temp(path_id).unwrap_or_default();
                        if v.len() < 256 { v.push((p.x, p.y, t)); }
                        d.insert_temp(path_id, v);
                    });
                }
            }
            if resp.drag_stopped() {
                let pts: Vec<(f32, f32, u32)> = ui.ctx().data_mut(|d| d.get_temp(path_id).unwrap_or_default());
                ui.ctx().data_mut(|d| d.remove::<Vec<(f32, f32, u32)>>(path_id));
                if pts.len() >= 3 {
                    let path = cardinal_gesture::path_from_samples(&pts);
                    match cardinal_gesture::classify(&path, &thresholds) {
                        cardinal_gesture::CardinalGesture::Hairpin(d) => {
                            // The hairpin's DIRECTION selects which variant to capitalise.
                            let want = dir8_from(d);
                            let target = key.cardinal.iter().find(|(k, _)| *k == want)
                                .map(|(_, s)| s.clone())
                                .unwrap_or_else(|| key.emit.clone());
                            claimed = Some(target.to_uppercase());
                        }
                        cardinal_gesture::CardinalGesture::Rotation(r) => {
                            claimed = match r {
                                cardinal_gesture::Rotation::Cw => key.rotate_cw.clone(),
                                cardinal_gesture::Rotation::Ccw => key.rotate_ccw.clone(),
                            };
                        }
                        _ => {}
                    }
                }
            }
        }
        let mut menu = uikit_egui::slide_menu::SlideMenu::new(id).item_size((size.y * 0.9).clamp(36.0, 52.0));
        menu.deadzone = deadzone;
        // NO FLOATING COMPASS. The keycap draws the gesture itself (see
        // `paint_key_face`), and the popup was doing three unhelpful things at
        // once: covering the neighbouring keys, printing the eight options a
        // second time when the cap already prints them at rest, and rendering
        // the tap glyph in its most prominent cell — directly under the finger
        // hiding it. `show_cardinal` still owns resolution; only its chrome is
        // off. voiceterm's action compass keeps the popup, because a terminal
        // grid has no cap to substitute into.
        menu.cardinal_compass = false;
        // ALWAYS shown, even on a frame the classifier claimed: this is what closes the widget's
        // press state. Returning early before this call left the compass open with a stale origin.
        let outcome = menu.show_cardinal(ui, id, resp, &tap, &dirs);
        if let Some(c) = claimed {
            uikit_egui::slide_menu::SlideMenu::cancel_cardinal(ui, id);
            return Some(c);
        }
        if let uikit_egui::slide_menu::SlideOutcome::Selected(v, _) = outcome {
            return Some(v);
        }
        return None;
    }
    // Long-press → variant grid via the shared gesture widget; a quick tap still types the key.
    if !key.variants.is_empty() {
        let id = ui.id().with(("keyvar", &key.label, rect.min.x as i32, rect.min.y as i32));
        let opts: Vec<uikit_egui::slide_menu::SlideOption> = key
            .variants
            .iter()
            .map(|v| uikit_egui::slide_menu::SlideOption::new(v.clone(), v.clone()))
            .collect();
        let menu = uikit_egui::slide_menu::SlideMenu::new(id).columns(key.variants.len().min(5)).hold(0.3);
        match menu.show_stored(ui, id, resp, &opts) {
            uikit_egui::slide_menu::SlideOutcome::Selected(v, _) => return Some(v),
            uikit_egui::slide_menu::SlideOutcome::Cancelled => return None, // opened then released without a pick
            uikit_egui::slide_menu::SlideOutcome::Idle => {}
        }
    }
    if resp.clicked() {
        Some(key.emit.clone())
    } else {
        None
    }
}

/// Convenience for tests / headless: which key (emit) a tap at `pos` would hit, given a
/// laid-out board of `board` rect. Pure geometry, no egui context needed — lets the layout
/// math be unit-tested without a live UI.
pub fn hit_test(layout: &Layout, board: Rect, adaptive: &Adaptive, pos: egui::Pos2) -> Option<String> {
    let rects = key_rects(layout, board, adaptive, &Style::default());
    layout.rows.iter().zip(&rects).find_map(|(row, rs)| {
        row.keys.iter().zip(rs).find(|(_, r)| r.contains(pos)).map(|(k, _)| k.emit.clone())
    })
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
    fn steno_token_round_trips() {
        let tok = steno_token("L1", "N");
        assert_eq!(decode_steno_token(&tok), Some(("L1", "N")));
    }

    #[test]
    fn steno_token_is_not_confused_with_plain_text() {
        // A layout with no steno keys should never accidentally produce
        // something `decode_steno_token` matches — the reserved `\u{0}`
        // prefix can't occur in real layout text.
        assert_eq!(decode_steno_token("n"), None);
        assert_eq!(decode_steno_token("N"), None);
    }

    #[test]
    fn dir_code_is_stable_for_the_four_directions_steno_actually_uses() {
        use uikit_egui::slide_menu::Dir8;
        assert_eq!(dir_code(Dir8::N), "N");
        assert_eq!(dir_code(Dir8::E), "E");
        assert_eq!(dir_code(Dir8::S), "S");
        assert_eq!(dir_code(Dir8::W), "W");
    }

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
    fn qwerty_bottom_row_matches_the_real_layout_exactly_not_a_hand_copied_guess() {
        // regression test for the actual "layout matching" bug: this crate used to hardcode its
        // own approximate copy of the bottom row (space=6.0, ⏎=2.0, missing ?123/🎤 entirely) that
        // had already drifted from resources/layouts/en_us_qwerty.json, the GTK app's real source
        // of truth. Now that qwerty() loads the real file, this locks the two back together —
        // if this ever fails, either the real JSON changed (update this test) or the loader
        // regressed (fix the loader), but the two can't silently drift apart unnoticed again.
        let l = Layout::qwerty();
        let bottom = &l.rows[3].keys;
        assert_eq!(bottom.len(), 6, "the real bottom row is ?123, 🎤, space, comma, period, ⏎ — not just space+enter");
        let labels: Vec<&str> = bottom.iter().map(|k| k.label.as_str()).collect();
        assert_eq!(labels, vec!["?123", "🎤", " ", ",", ".", "⏎"]);
        let weights: Vec<f32> = bottom.iter().map(|k| k.weight).collect();
        assert_eq!(weights, vec![1.5, 1.0, 5.0, 1.0, 1.0, 1.5], "these are the real file's widths, not the old hand-typed 6.0/2.0");
        let space = bottom.iter().find(|k| k.label == " ").unwrap();
        assert_eq!(space.emit, " ");
        let enter = bottom.iter().find(|k| k.label == "⏎").unwrap();
        assert_eq!(enter.emit, "\n");
    }

    #[test]
    fn qwerty_long_press_variants_come_from_the_real_extended_field() {
        // the old hardcoded layout had its own separate accented-letter table (`variants_for`)
        // that had to be kept in sync by hand; now `variants` comes directly from the real
        // file's `extended` array, so there's nothing to keep in sync — it just IS the source.
        let l = Layout::qwerty();
        let e = l.rows[0].keys.iter().find(|k| k.label == "e").unwrap();
        assert!(e.variants.contains(&"é".to_string()), "expected é among e's real extended entries: {:?}", e.variants);
    }

    #[test]
    fn the_standard_layouts_carry_a_second_layer_on_every_letter() {
        // Demo board (Layout::qwerty) and the real board (real_layout::builtin) load the same
        // files through different paths; both get the positional layer.
        let q = Layout::qwerty();
        let key = |l: &Layout, lab: &str| l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == lab).cloned().unwrap();
        assert_eq!(key(&q, "s").variants.first().map(String::as_str), Some("#"));
        assert_eq!(key(&q, "m").variants.first().map(String::as_str), Some("?"));
        assert!(key(&q, "e").variants.contains(&"é".to_string()), "accents kept");
        let files = crate::real_layout::builtin();
        let dv = files.iter().find(|f| f.name.contains("dvorak")).expect("dvorak ships");
        let p = dv.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == "p").unwrap();
        assert_eq!(p.extended.first().map(String::as_str), Some("4"), "p is in QWERTY's r column");
        let sym = files.iter().find(|f| f.name.contains("symbol")).expect("symbols ships");
        let untouched = crate::real_layout::parse(crate::real_layout::SYMBOLS_JSON).unwrap();
        assert_eq!(sym.rows.len(), untouched.rows.len());
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
    fn cardinal_layout_exposes_swipe_variants() {
        let l = Layout::cardinal();
        let with_card = l.rows.iter().flat_map(|r| &r.keys).filter(|k| !k.cardinal.is_empty()).count();
        assert!(with_card >= 9, "the 3x3 letters should carry cardinal variants, got {with_card}");
        // 'e' taps to "e" and swipes north to "E"
        let e = l.rows.iter().flat_map(|r| &r.keys).find(|k| k.emit == "e").unwrap();
        assert!(e.cardinal.iter().any(|(d, s)| matches!(d, uikit_egui::slide_menu::Dir8::N) && s == "E"));
    }

    #[test]
    fn cardinal8_covers_the_alphabet_with_frequent_letters_on_taps() {
        let l = Layout::cardinal8();
        let letter_keys: Vec<&Key> = l.rows.iter().flat_map(|r| &r.keys).filter(|k| !k.cardinal.is_empty()).collect();
        // the nine centers are the high-frequency letters, typed on a plain tap
        let centers: std::collections::HashSet<&str> = letter_keys.iter().map(|k| k.emit.as_str()).collect();
        for c in ["e", "t", "a", "o", "i", "n", "s", "h", "r"] {
            assert!(centers.contains(c), "high-frequency '{c}' should be a tap center");
        }
        // every letter a–z is reachable, as a center or a swipe variant
        let mut reachable: std::collections::HashSet<char> = centers.iter().flat_map(|s| s.chars()).collect();
        for k in &letter_keys {
            for (_, s) in &k.cardinal {
                reachable.extend(s.to_lowercase().chars());
            }
        }
        for ch in 'a'..='z' {
            assert!(reachable.contains(&ch), "letter '{ch}' should be reachable on cardinal8");
        }
    }

    #[test]
    fn swipe_switches_layout() {
        let mut b = Board::new();
        assert_eq!(b.current().name, "en_us_qwerty");
        b.swipe(1);
        assert_eq!(b.current().name, "symbols");
        b.swipe(1);
        assert_eq!(b.current().name, "cardinal"); // experimental cardinal keyboards are in the rotation
        b.swipe(1);
        assert_eq!(b.current().name, "cardinal8");
        b.swipe(1);
        assert_eq!(b.current().name, "Stenotype", "the steno board joined the rotation");
        b.swipe(1); // wraps back
        assert_eq!(b.current().name, "en_us_qwerty");
        b.swipe(-1); // wraps backward to the last layout
        assert_eq!(b.current().name, "Stenotype");
    }

    #[test]
    fn apply_settings_runs_and_keeps_layout() {
        let ctx = egui::Context::default();
        let mut b = Board::new();
        let mut prefs = settings_kit::Prefs::default();
        prefs.gesture.animation = settings_kit::AnimationStyle::Snappy;
        prefs.touch.text_scale = 1.4;
        // adopting cross-app settings doesn't disturb the current layout
        b.apply_settings(&ctx, &prefs);
        assert_eq!(b.current().name, "en_us_qwerty");
        // text scale reached the shared egui context
        let body = ctx.style().text_styles.get(&egui::TextStyle::Body).map(|f| f.size).unwrap_or(0.0);
        assert!(body > 0.0);
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

// ── voice mode (dictation) — mic-kit fallback chain + voice-kit pipeline ──────
// The keyboard's voice mode: read the mic through a graceful FallbackChain, accumulate in the
// voice pipeline, and on release transcribe + filter (at the user's VoiceFilter level) into
// text to type. The mic is a step-up — if it's silent/stale the status shows it and no text is
// forced. Same pipeline voiceterm's voice pane and logscope voice commands will use.

/// Keyboard dictation state.
pub struct VoiceMode {
    pipeline: voice_kit::pipeline::VoicePipeline,
    active: bool,
    status: mic_kit::InputStatus,
    scratch: Vec<f32>,
}

impl VoiceMode {
    pub fn new() -> VoiceMode {
        VoiceMode {
            pipeline: voice_kit::pipeline::VoicePipeline::new(1600), // ~0.1s @16k before a flush is worthwhile
            active: false,
            status: mic_kit::InputStatus::Silent,
            scratch: vec![0.0; 320],
        }
    }

    pub fn is_active(&self) -> bool {
        self.active
    }

    pub fn status(&self) -> mic_kit::InputStatus {
        self.status
    }

    /// Start/stop dictation. Returns the new active state.
    pub fn toggle(&mut self) -> bool {
        self.active = !self.active;
        if !self.active {
            self.pipeline.cancel();
        }
        self.active
    }

    /// Sync the filter strength from cross-app settings (VoiceFilter level).
    pub fn apply_settings(&mut self, prefs: &settings_kit::Prefs) {
        let vf = prefs.a11y.voice_filter;
        self.pipeline.set_filter(vf.strips_artifacts(), vf.filters_hallucinations(), vf.requires_speech_like());
    }

    /// Per-frame while active: pull a frame from the mic chain and accumulate it. Records the
    /// mic status (Live/Fallback/Silent) so the UI can show it. Cheap; never blocks.
    pub fn pump(&mut self, mic: &mut dyn mic_kit::AudioInput) {
        if !self.active {
            return;
        }
        let n = self.scratch.len();
        let mut frame = std::mem::take(&mut self.scratch);
        frame.resize(n, 0.0);
        self.status = mic.read(&mut frame);
        if self.status != mic_kit::InputStatus::Silent {
            self.pipeline.push(&frame);
        }
        self.scratch = frame;
    }

    /// End the utterance: transcribe + filter into text to type. Also deactivates.
    pub fn finish(&mut self, transcriber: &mut dyn voice_kit::pipeline::Transcriber) -> Option<String> {
        self.active = false;
        self.pipeline.flush(transcriber)
    }
}

impl Default for VoiceMode {
    fn default() -> Self {
        VoiceMode::new()
    }
}

#[cfg(test)]
mod voice_tests {
    use super::*;

    struct Stub(&'static str);
    impl voice_kit::pipeline::Transcriber for Stub {
        fn transcribe(&mut self, _audio: &[f32]) -> Option<String> {
            Some(self.0.to_string())
        }
    }

    #[test]
    fn toggle_and_filter_from_settings() {
        let mut v = VoiceMode::new();
        assert!(!v.is_active());
        assert!(v.toggle());
        let mut prefs = settings_kit::Prefs::default();
        prefs.a11y.voice_filter = settings_kit::VoiceFilter::Strict;
        v.apply_settings(&prefs); // Strict → require_speech; just shouldn't panic
        assert!(v.is_active());
    }

    #[test]
    fn pump_live_mic_accumulates_then_finish_types_text() {
        let mut v = VoiceMode::new();
        v.toggle(); // active
        // a live mic feeding audio
        let mut mic = mic_kit::BufferedInput::new(2048, 4);
        mic.feed(&vec![0.2f32; 1800]);
        v.pump(&mut mic);
        assert_eq!(v.status(), mic_kit::InputStatus::Live);
        let text = v.finish(&mut Stub("hello world  "));
        assert_eq!(text.as_deref(), Some("hello world"));
        assert!(!v.is_active(), "finish deactivates");
    }

    #[test]
    fn pump_silent_mic_reports_silent_and_no_forced_text() {
        let mut v = VoiceMode::new();
        v.toggle();
        // a mic that never got fed → silent/stale
        let mut mic = mic_kit::BufferedInput::new(64, 1);
        for _ in 0..3 {
            v.pump(&mut mic);
        }
        assert_eq!(v.status(), mic_kit::InputStatus::Silent);
        // nothing accumulated → finish yields no text (graceful, not an error)
        assert!(v.finish(&mut Stub("phantom")).is_none());
    }
}

/// Headless end-to-end: synthetic pointer events through a real `egui::Context`, into
/// `keyboard_ui`, out as the committed string. This is the test that was missing while the
/// cardinal path was dead — every unit under it passed (classifier, widget maths, layout tables)
/// and no test ever pressed a key and looked at what came out.
#[cfg(test)]
mod swipe_delivery_tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    const KEY_H: f32 = 44.0;
    const GUTTER: f32 = 4.0;
    const W: f32 = 400.0;

    struct Rig {
        ctx: egui::Context,
        layout: Layout,
        adaptive: Adaptive,
        palette: &'static str,
        t: f64,
    }

    impl Rig {
        fn new(layout: Layout, palette: &'static str) -> Rig {
            let rig = Rig {
                ctx: egui::Context::default(),
                layout,
                adaptive: Adaptive { min_touch_target: KEY_H, gutter: GUTTER, ..Adaptive::default() },
                palette,
                t: 0.0,
            };
            rig
        }

        /// One frame with `events`; returns what `keyboard_ui` emitted.
        fn frame(&mut self, events: Vec<Event>) -> Option<String> {
            self.t += 0.03;
            let raw = RawInput {
                events,
                time: Some(self.t),
                screen_rect: Some(Rect::from_min_size(Pos2::ZERO, Vec2::new(W, 300.0))),
                ..Default::default()
            };
            let mut out = None;
            let (layout, adaptive, palette) = (&self.layout, &self.adaptive, self.palette);
            let _ = self.ctx.run(raw, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    // No inter-key spacing, so key geometry is exactly `key_rect`'s arithmetic.
                    ui.spacing_mut().item_spacing = Vec2::ZERO;
                    out = keyboard_ui(ui, layout, adaptive, palette);
                });
            });
            out
        }

        /// Where key (row, col) is on screen under the test panel -- from `key_rects`, the
        /// function the painter places keys with.
        fn key_rect(&self, row: usize, col: usize) -> Rect {
            let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(W, 300.0));
            key_rects(&self.layout, screen, &self.adaptive, &Style::default())[row][col]
        }

        /// Press at `from`, drag through `via`, release at `to`. Returns everything emitted
        /// across the gesture's frames (a correct gesture emits exactly once).
        fn gesture(&mut self, from: Pos2, via: &[Pos2], to: Pos2) -> Vec<String> {
            let mut emitted = Vec::new();
            // warm-up: egui hit-tests against the previous frame's widget rects
            self.frame(vec![]);
            self.frame(vec![]);
            let press = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
            emitted.extend(self.frame(vec![Event::PointerMoved(from), press(from, true)]));
            for &p in via {
                emitted.extend(self.frame(vec![Event::PointerMoved(p)]));
            }
            emitted.extend(self.frame(vec![Event::PointerMoved(to)]));
            emitted.extend(self.frame(vec![press(to, false)]));
            emitted.extend(self.frame(vec![]));
            emitted.extend(self.frame(vec![]));
            emitted
        }

        fn tap(&mut self, at: Pos2) -> Vec<String> {
            self.gesture(at, &[], at)
        }
    }

    /// The cardinal8 layout's top-left key: tap `e`, N `E`, E `b`, S `v`, NE `1`, SE `2`, SW `3`.
    fn e_key(rig: &Rig) -> Rect {
        let r = rig.key_rect(0, 0);
        assert_eq!(rig.layout.rows[0].keys[0].emit, "e");
        r
    }

    #[test]
    fn a_tap_in_the_middle_of_a_cardinal_key_types_its_centre() {
        for palette in ["system", "default"] {
            let mut rig = Rig::new(Layout::cardinal8(), palette);
            let r = e_key(&rig);
            assert_eq!(rig.tap(r.center()), vec!["e".to_string()], "palette {palette}");
        }
    }

    #[test]
    fn a_tap_near_the_edge_of_a_cardinal_key_is_still_a_tap() {
        // THE REGRESSION. Measured from the key centre, a press 18px above centre was already
        // past the deadzone, so a plain tap near the top edge typed the NORTH variant (`E`).
        // Measured from the press point it has not moved at all.
        for palette in ["system", "default"] {
            let mut rig = Rig::new(Layout::cardinal8(), palette);
            let r = e_key(&rig);
            let near_top = Pos2::new(r.center().x, r.top() + 4.0);
            assert_eq!(rig.tap(near_top), vec!["e".to_string()], "palette {palette}");
            let near_left = Pos2::new(r.left() + 5.0, r.center().y);
            assert_eq!(rig.tap(near_left), vec!["e".to_string()], "palette {palette}");
        }
    }

    #[test]
    fn cardinal_swipes_from_the_centre_hit_the_directional_keys() {
        let mut rig = Rig::new(Layout::cardinal8(), "system");
        let r = e_key(&rig);
        let c = r.center();
        let d = 40.0;
        let cases = [
            (Vec2::new(0.0, -d), "E"),  // N
            (Vec2::new(d, 0.0), "b"),   // E
            (Vec2::new(0.0, d), "v"),   // S
            (Vec2::new(d, -d), "1"),    // NE
            (Vec2::new(d, d), "2"),     // SE
            (Vec2::new(-d, d), "3"),    // SW
        ];
        for (delta, want) in cases {
            let mid = c + delta * 0.5;
            let end = c + delta;
            assert_eq!(rig.gesture(c, &[mid], end), vec![want.to_string()], "swipe {delta:?}");
        }
    }

    #[test]
    fn cardinal_swipes_are_relative_to_the_press_point_not_the_key() {
        // Press on the LEFT third of the key and swipe right: relative to the centre this nets
        // out near zero (a "tap"); relative to the press point it is unambiguously East.
        for palette in ["system", "default"] {
            let mut rig = Rig::new(Layout::cardinal8(), palette);
            let r = e_key(&rig);
            let from = Pos2::new(r.left() + 8.0, r.center().y);
            let to = Pos2::new(r.left() + 8.0 + 36.0, r.center().y);
            assert_eq!(rig.gesture(from, &[Pos2::new(from.x + 18.0, from.y)], to), vec!["b".to_string()], "palette {palette}");
            // and from the TOP edge, swiping down is South, even though the finger ends up
            // below the key's centre by less than it started above it
            let from = Pos2::new(r.center().x, r.top() + 4.0);
            let to = Pos2::new(r.center().x, r.top() + 4.0 + 36.0);
            assert_eq!(rig.gesture(from, &[Pos2::new(from.x, from.y + 18.0)], to), vec!["v".to_string()], "palette {palette}");
        }
    }

    #[test]
    fn an_overshoot_that_settles_back_is_still_the_swipe() {
        // Lift-off drift: out 40px east, back to 6px before release. The release point is
        // inside the deadzone; the farthest reach is not.
        let mut rig = Rig::new(Layout::cardinal8(), "system");
        let r = e_key(&rig);
        let c = r.center();
        let out = rig.gesture(c, &[c + Vec2::new(40.0, 0.0)], c + Vec2::new(6.0, 0.0));
        assert_eq!(out, vec!["b".to_string()]);
    }

    #[test]
    fn a_hairpin_capitalises_the_direction_it_points_at() {
        // Out to the east and back within the hairpin window: the east variant, capitalised.
        // Runs THROUGH the widget on the same press — the classifier's claim must win and the
        // compass state must be closed afterwards, or the next tap is measured from a stale origin.
        let mut rig = Rig::new(Layout::cardinal8(), "system");
        let r = e_key(&rig);
        let c = r.center();
        let out = rig.gesture(c, &[c + Vec2::new(30.0, 0.0), c + Vec2::new(60.0, 0.0), c + Vec2::new(30.0, 0.0)], c + Vec2::new(2.0, 0.0));
        assert_eq!(out, vec!["B".to_string()]);
        // the follow-up tap is a clean tap
        assert_eq!(rig.tap(c), vec!["e".to_string()]);
    }

    #[test]
    fn the_gtk_cardinal_grid_flicks_and_rotates_through_the_egui_path() {
        // THE REFERENCE. The GTK build's own cardinal layout (board_layouts'
        // `default_cardinal_grid`, rendered by `real_layout::to_render` as the sidebar tab
        // does), driven through the same press-drag-release rig: `a` east is `e`, `a` north is
        // `u`, and a clockwise circle on `a` is its digit `1` — the eleven-gesture key working
        // end to end, not just its classifier.
        let file = board_layouts::Layout::default_cardinal_grid();
        let mut rig = Rig::new(crate::real_layout::to_render(&file, crate::real_layout::Shift::Off), "system");
        let (row, col) = rig.layout.rows.iter().enumerate()
            .find_map(|(r, row)| row.keys.iter().position(|k| k.emit == "a").map(|c| (r, c)))
            .expect("the `a` cell");
        let rect = rig.key_rect(row, col);
        let c = rect.center();
        assert_eq!(rig.tap(c), vec!["a".to_string()]);
        assert_eq!(rig.gesture(c, &[c + Vec2::new(20.0, 0.0)], c + Vec2::new(40.0, 0.0)), vec!["e".to_string()]);
        assert_eq!(rig.gesture(c, &[c + Vec2::new(0.0, -20.0)], c + Vec2::new(0.0, -40.0)), vec!["u".to_string()]);
        // clockwise on screen (y down): E → S → W → N and back — a full turn, ending near the start
        let r = 30.0;
        let circle: Vec<Pos2> = (1..24).map(|i| {
            let a = i as f32 / 24.0 * std::f32::consts::TAU;
            c + Vec2::new(r * a.cos(), r * a.sin()) - Vec2::new(r, 0.0)
        }).collect();
        let start = c;
        let end = c + Vec2::new(0.5, 0.0);
        let out = rig.gesture(start, &circle, end);
        assert_eq!(out, vec!["1".to_string()], "a clockwise turn on `a` types its digit");
    }

    #[test]
    fn a_plain_key_still_types_on_tap_under_the_glass_palette() {
        // The complement: restructuring paint_key must not have taken the click path away
        // from ordinary keys, which is the whole qwerty board.
        let mut rig = Rig::new(Layout::qwerty(), "system");
        let q = rig.key_rect(0, 0);
        assert_eq!(rig.tap(q.center()), vec!["q".to_string()]);
    }
}

#[cfg(test)]
mod gtk_parity_tests {
    use super::*;

    /// The real layout files carry `key_type`, and the port dropped it — so Enter was not
    /// the accent key it is on the GTK board, and shift, space and "?123" were all the same
    /// grey cap as each other.
    #[test]
    fn the_real_layout_carries_its_key_roles_through() {
        let text = include_str!("../../resources/layouts/en_us_qwerty.json");
        let layout = layout_from_json(text).expect("the shipped layout parses");
        let find = |label: &str| {
            layout
                .rows
                .iter()
                .flat_map(|r| &r.keys)
                .find(|k| k.label == label)
                .unwrap_or_else(|| panic!("no {label:?} key in the shipped layout"))
        };
        assert_eq!(find("⏎").role, KeyRole::Action, "Enter is the commit key");
        assert_eq!(find("⇧").role, KeyRole::Modifier);
        assert_eq!(find("?123").role, KeyRole::Modifier);
        assert_eq!(find("⌫").role, KeyRole::Special);
        assert_eq!(find("🎤").role, KeyRole::Special);
        assert_eq!(find("a").role, KeyRole::Normal, "a letter is a plain key");
        // and the roles are not all the same, which a `key_type` that failed to parse would
        // silently produce
        let roles: std::collections::HashSet<KeyRole> =
            layout.rows.iter().flat_map(|r| &r.keys).map(|k| k.role).collect();
        assert!(roles.len() >= 4, "only {} distinct roles — key_type is not being read", roles.len());
    }

    /// The board the phone actually runs is `real_layout::to_render`, a different projection
    /// from `layout_from_json` — and it dropped the same field, plus the shift state.
    #[test]
    fn the_real_board_carries_roles_and_shows_which_shift_is_on() {
        use crate::real_layout::{to_render, Shift};
        let file = crate::real_layout::parse(include_str!("../../resources/layouts/en_us_qwerty.json"))
            .expect("the shipped layout parses");
        let find = |l: &crate::Layout, label: &str| {
            l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == label).cloned()
        };

        let off = to_render(&file, Shift::Off);
        assert_eq!(find(&off, "⏎").unwrap().role, KeyRole::Action, "Enter commits, and looks like it");
        assert_eq!(find(&off, "⌫").unwrap().role, KeyRole::Special);
        let shift_off = find(&off, "⇧").unwrap();
        assert_eq!(shift_off.role, KeyRole::Modifier);
        assert_eq!(shift_off.latch, Latch::Off);

        // ONE-SHOT and LOCKED are different caps, because they are different states and
        // the only way to tell them apart before this was to type a letter and look.
        let once = to_render(&file, Shift::Once);
        assert_eq!(find(&once, "⇧").unwrap().latch, Latch::Once);
        let caps = to_render(&file, Shift::Caps);
        assert_eq!(find(&caps, "⇧").unwrap().latch, Latch::Locked);

        // the latch is on the SHIFT key only — a letter does not light up because caps is on
        let a = find(&caps, "A").or_else(|| find(&caps, "a")).expect("a letter key");
        assert_eq!(a.latch, Latch::Off);
        assert_eq!(a.role, KeyRole::Normal);
    }

    /// A layout built in code has no `key_type`, so the label decides — by NAME, not by
    /// length. Length is what made "space" a modifier.
    #[test]
    fn a_label_implies_its_role_by_name_not_by_length() {
        assert_eq!(KeyRole::from_label("enter"), KeyRole::Action);
        assert_eq!(KeyRole::from_label("Return"), KeyRole::Action);
        assert_eq!(KeyRole::from_label("shift"), KeyRole::Modifier);
        assert_eq!(KeyRole::from_label("?123"), KeyRole::Modifier);
        assert_eq!(KeyRole::from_label("backspace"), KeyRole::Special);
        assert_eq!(KeyRole::from_label("space"), KeyRole::Normal, "wide is not the same as special");
        assert_eq!(KeyRole::from_label("q"), KeyRole::Normal);
        // an unknown key_type from a newer layout renders as a plain key rather than failing
        assert_eq!(KeyRole::from_json("navigation"), KeyRole::Normal);
        assert_eq!(KeyRole::from_json("ACTION"), KeyRole::Action, "case does not matter");
    }

    /// The four keys the GTK board has and this one must: mic, space, backspace, enter —
    /// on EVERY layout, and doing what they say rather than typing their own label.
    ///
    /// The bundled qwerty's mic emitted the string "🎤" and typed an emoji into your
    /// document, because `emit_for_action` returned `None` for every action but the three
    /// text-producing ones and the projection then fell back to the key's LABEL. Shift typed
    /// "⇧" and `?123` typed "?123" the same way. Meanwhile `cardinal` had no backspace at
    /// all and `symbols` had no mic.
    #[test]
    fn every_board_has_the_four_keys_and_they_do_what_they_say() {
        use crate::real_layout::{classify, Press};
        let boards = [
            ("qwerty", Layout::qwerty()),
            ("symbols", Layout::symbols()),
            ("cardinal", Layout::cardinal()),
            ("cardinal8", Layout::cardinal8()),
        ];
        for (name, l) in boards {
            let presses: Vec<Press> =
                l.rows.iter().flat_map(|r| &r.keys).map(|k| classify(&k.emit)).collect();
            let has = |p: &Press| presses.iter().any(|q| q == p);
            assert!(has(&Press::Voice), "{name} has no working mic key");
            assert!(has(&Press::Backspace) || has(&Press::Text("\u{8}".into())), "{name} has no backspace");
            assert!(has(&Press::Return) || has(&Press::Text("\n".into())), "{name} has no enter");
            assert!(has(&Press::Text(" ".into())), "{name} has no space");
            // and NOTHING types a label that should have been an action
            for k in l.rows.iter().flat_map(|r| &r.keys) {
                if let Press::Text(t) = classify(&k.emit) {
                    assert!(
                        !matches!(t.as_str(), "🎤" | "⇧" | "?123" | "⌫" | "⏎"),
                        "{name}: the {t:?} key types its own label"
                    );
                }
            }
        }
    }

    /// Specifically the projection that was wrong: a JSON layout's non-text actions must
    /// cross as tokens, not as labels. `real_layout::to_render` always did this; this one
    /// did not, and it is the one behind `Layout::qwerty()` — the board that comes up first.
    #[test]
    fn a_json_layouts_actions_cross_as_tokens() {
        use crate::real_layout::{classify, Press};
        let l = Layout::qwerty();
        let by_label = |lbl: &str| {
            l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == lbl).unwrap_or_else(|| panic!("no {lbl}"))
        };
        assert_eq!(classify(&by_label("🎤").emit), Press::Voice);
        assert_eq!(classify(&by_label("⇧").emit), Press::Shift);
        assert_eq!(classify(&by_label("?123").emit), Press::SymbolsToggle);
        // the three that always worked still do, as plain characters
        assert_eq!(classify(&by_label("⌫").emit), Press::Text("\u{8}".into()));
        assert_eq!(classify(&by_label("⏎").emit), Press::Text("\n".into()));
        assert_eq!(classify(&by_label(" ").emit), Press::Text(" ".into()));
    }

    /// Shift and `?123` on the code-built boards. Before the last commit they typed their
    /// own labels ("⇧", "?123"); after it they arrived as tokens and fell through a
    /// `_ => {}`. A key you press that does nothing is its own kind of broken.
    #[test]
    fn shift_cycles_and_a_one_shot_is_spent_by_the_letter_it_shifted() {
        use crate::real_layout::Shift;
        let mut b = Board::new();
        assert_eq!(b.shift(), Shift::Off);
        let letter = |l: &Layout| {
            l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label.eq_ignore_ascii_case("q")).cloned()
        };
        assert_eq!(letter(&b.rendered()).unwrap().emit, "q");

        // Off → Once: the letters upshift and the shift cap shows the latch.
        b.tap_shift();
        assert_eq!(b.shift(), Shift::Once);
        let r = b.rendered();
        assert_eq!(letter(&r).unwrap().emit, "Q");
        let shift_key = r.rows.iter().flat_map(|x| &x.keys)
            .find(|k| crate::real_layout::classify(&k.emit) == crate::real_layout::Press::Shift)
            .expect("a shift key");
        assert_eq!(shift_key.latch, Latch::Once);
        drop(r);

        // A letter spends a one-shot.
        b.after_text();
        assert_eq!(b.shift(), Shift::Off);
        assert_eq!(letter(&b.rendered()).unwrap().emit, "q");

        // Once → Caps: NOT spent by text, which is the whole difference between them.
        b.tap_shift();
        b.tap_shift();
        assert_eq!(b.shift(), Shift::Caps);
        b.after_text();
        assert_eq!(b.shift(), Shift::Caps, "caps was spent like a one-shot");
        assert_eq!(letter(&b.rendered()).unwrap().emit, "Q");
        let r = b.rendered();
        let shift_key = r.rows.iter().flat_map(|x| &x.keys)
            .find(|k| crate::real_layout::classify(&k.emit) == crate::real_layout::Press::Shift).unwrap();
        assert_eq!(shift_key.latch, Latch::Locked);
        drop(r);

        // Caps → Off closes the cycle.
        b.tap_shift();
        assert_eq!(b.shift(), Shift::Off);
    }

    /// Shift must not corrupt what it does not understand. An action token is bytes, not a
    /// word, and uppercasing it would break the key.
    #[test]
    fn shift_upshifts_letters_and_leaves_everything_else_alone() {
        let mut b = Board::new();
        b.tap_shift();
        let r = b.rendered();
        for k in r.rows.iter().flat_map(|x| &x.keys) {
            match crate::real_layout::classify(&k.emit) {
                crate::real_layout::Press::Text(t) => {
                    // A letter is upshifted; a space, a digit and punctuation are not.
                    if t.chars().all(|c| c.is_alphabetic()) && !t.is_empty() {
                        assert!(t.chars().all(|c| !c.is_lowercase()), "{t:?} was left lower-case");
                    }
                }
                // Every action still decodes as itself — nothing was uppercased into junk.
                p => assert!(
                    matches!(p, crate::real_layout::Press::Shift
                                | crate::real_layout::Press::Voice
                                | crate::real_layout::Press::SymbolsToggle
                                | crate::real_layout::Press::Backspace
                                | crate::real_layout::Press::Return
                                | crate::real_layout::Press::Steno(..)),
                    "{p:?} came out of a shifted layout"
                ),
            }
        }
    }

    /// A toggle that only goes one way is a trap on a phone, where the way back is a swipe
    /// nobody has been told about.
    #[test]
    fn the_symbols_toggle_returns_to_where_it_came_from() {
        let mut b = Board::new();
        let start = b.layout_index();
        // The bundled file's own name. `Layout::qwerty()` parses
        // `resources/layouts/en_us_qwerty.json`, whose `name` field is `en_us_qwerty` — the
        // constructor is called `qwerty`, the LAYOUT is not, and asserting the constructor's
        // name here was asserting something no data says.
        assert_eq!(b.current().name, "en_us_qwerty");
        b.toggle_symbols();
        assert_eq!(b.current().name, "symbols");
        b.toggle_symbols();
        assert_eq!(b.layout_index(), start, "?123 was a one-way trip");

        // From a different board it returns THERE, not to qwerty.
        let cardinal = b.layout_names().iter().position(|n| *n == "cardinal").expect("cardinal exists");
        b.set_layout(cardinal);
        b.toggle_symbols();
        assert_eq!(b.current().name, "symbols");
        b.toggle_symbols();
        assert_eq!(b.layout_index(), cardinal);
    }

    /// "Cardinal swipe on the spacebar in any layout → Arrow key" is in the doc's Full-state
    /// table. The TRAY had it from the day `tray.rs` was ported; the full board did not, so
    /// the same gesture on the same key meant different things depending on whether the
    /// keyboard was minimized.
    #[test]
    fn every_board_gives_its_spacebar_the_arrow_swipes() {
        use crate::real_layout::{classify, Arrow, Press};
        use uikit_egui::slide_menu::Dir8;
        let mut b = Board::new();
        for mode in ["off", "cardinal", "words", "stenotype"] {
            b.apply_mode(mode);
            let l = b.rendered();
            let space = l
                .rows
                .iter()
                .flat_map(|r| &r.keys)
                .find(|k| classify(&k.emit) == Press::Text(" ".to_string()))
                .unwrap_or_else(|| panic!("{mode}: no spacebar"));
            let by = |d: Dir8| space.cardinal.iter().find(|(k, _)| *k == d).map(|(_, s)| s.clone());
            for (d, want) in [
                (Dir8::W, Arrow::Left),
                (Dir8::E, Arrow::Right),
                (Dir8::N, Arrow::Up),
                (Dir8::S, Arrow::Down),
            ] {
                let got = by(d).unwrap_or_else(|| panic!("{mode}: no {d:?} swipe on space"));
                assert_eq!(classify(&got), Press::Arrow(want), "{mode} {d:?}");
            }
        }
    }

    /// A layout that deliberately put something else on its space swipes means it, and
    /// silently overwriting that would be the attacher deciding it knows better than the
    /// layout file.
    #[test]
    fn a_spacebar_that_already_has_gestures_is_left_alone() {
        use crate::real_layout::{attach_space_arrows, classify, Press};
        use uikit_egui::slide_menu::Dir8;
        let mut l = Layout::qwerty();
        for row in &mut l.rows {
            for k in &mut row.keys {
                if classify(&k.emit) == Press::Text(" ".to_string()) {
                    k.cardinal = vec![(Dir8::N, "custom".to_string())];
                }
            }
        }
        attach_space_arrows(&mut l);
        let space = l.rows.iter().flat_map(|r| &r.keys)
            .find(|k| classify(&k.emit) == Press::Text(" ".to_string())).unwrap();
        assert_eq!(space.cardinal.len(), 1, "the attacher overwrote a layout's own gestures");
        assert_eq!(space.cardinal[0].1, "custom");
    }

    /// The arrows must reach the sink under the names it expects, and be their own Press —
    /// an arrow that classified as text would type the word "left".
    #[test]
    fn an_arrow_token_is_a_press_not_a_word() {
        use crate::real_layout::{classify, Arrow, Press};
        for a in [Arrow::Left, Arrow::Right, Arrow::Up, Arrow::Down] {
            assert_eq!(classify(&a.token()), Press::Arrow(a));
            assert!(!a.token().contains(a.keysym()), "the token must not be the keysym itself");
        }
        assert_eq!(Arrow::Left.keysym(), "Left");
        assert_eq!(Arrow::Down.keysym(), "Down");
    }

    /// The stenotype board's four keys must carry a STENO BUTTON ID. That is what makes
    /// `paint_key` emit a steno token on press instead of typing a letter — the token is
    /// built at press time from the button and the gesture, so a resting key does not carry
    /// one. (My first version of this test looked for tokens in `emit` and found none, which
    /// is correct and proves nothing.)
    #[test]
    fn the_steno_board_carries_button_ids_and_no_other_board_does() {
        let steno = Layout::stenotype();
        let ids: Vec<&str> = steno
            .rows
            .iter()
            .flat_map(|r| &r.keys)
            .filter_map(|k| k.steno_button.as_deref())
            .collect();
        assert_eq!(ids.len(), 4, "{ids:?}");
        for b in ["L1", "L2", "R1", "R2"] {
            assert!(ids.contains(&b), "no {b} key: {ids:?}");
        }

        // Each of the four carries its swipe alternates, which is where the phonemes are.
        let with_swipes = steno
            .rows
            .iter()
            .flat_map(|r| &r.keys)
            .filter(|k| k.steno_button.is_some() && !k.cardinal.is_empty())
            .count();
        assert_eq!(with_swipes, 4, "a steno button with no gestures is one phoneme, not five");

        // The utility row is ordinary keys, so the board is escapable — a steno board with
        // no backspace and no way back to letters is a trap.
        let labels: Vec<&str> = steno.rows.iter().flat_map(|r| &r.keys).map(|k| k.label.as_str()).collect();
        assert!(labels.contains(&"⌫"), "{labels:?}");
        assert!(labels.contains(&"?123"), "{labels:?}");

        // And no OTHER board carries one, or a letter key would feed the engine.
        for l in [Layout::qwerty(), Layout::symbols(), Layout::cardinal(), Layout::cardinal8()] {
            assert!(
                l.rows.iter().flat_map(|r| &r.keys).all(|k| k.steno_button.is_none()),
                "{} has a steno button on it",
                l.name
            );
        }
    }

    /// `layout.experimental_mode` has offered Off / Cardinal / Stenotype / Words since the
    /// settings panel was written, and choosing one did nothing: the value went into a map
    /// and no code read it back.
    #[test]
    fn the_experimental_mode_setting_moves_the_board() {
        let mut b = Board::new();
        let start = b.current().name.clone();
        assert_eq!(start, "en_us_qwerty");

        assert!(b.apply_mode("cardinal"));
        assert_eq!(b.current().name, "cardinal");
        assert!(b.apply_mode("stenotype"));
        // "Stenotype" with a capital: `to_render` names a rendered layout from the file's
        // DISPLAY name, which is why `apply_mode` matches case-insensitively.
        assert_eq!(b.current().name, "Stenotype", "the steno board is reachable now");
        assert!(b.apply_mode("words"));
        assert_eq!(b.current().name, "cardinal8");
        assert!(b.apply_mode("off"));
        assert_eq!(b.current().name, "en_us_qwerty");

        // Idempotent: re-applying the mode you are in is not a change, so a caller does not
        // reset shift or re-announce a layout that did not move.
        assert!(!b.apply_mode("off"));
    }

    /// A mode this build cannot honour must not silently switch the board to something
    /// else. Stenotype has no layout here yet.
    #[test]
    fn a_mode_with_no_layout_leaves_the_board_alone() {
        let mut b = Board::new();
        // Every mode this build names now HAS a layout — stenotype was the last one
        // missing, and adding it made the "known mode, absent layout" branch unreachable
        // from here. The guard stays (a build could ship without a board), but there is no
        // honest way to exercise it without a constructor that exists only for a test, so
        // this covers what is reachable: an unknown value.
        b.apply_mode("cardinal");
        for m in ["off", "cardinal", "stenotype", "words"] {
            b.apply_mode(m);
            assert!(
                b.layout_names().iter().any(|n| n.eq_ignore_ascii_case(match m {
                    "cardinal" => "cardinal",
                    "stenotype" => "stenotype",
                    "words" => "cardinal8",
                    _ => "en_us_qwerty",
                })),
                "the settings offer {m} and this build cannot draw it"
            );
        }
        b.apply_mode("cardinal");

        // An unknown value from a newer build falls to the LETTER board rather than staying
        // wherever it happened to be — an unreadable setting must not leave the keyboard in
        // an arbitrary state.
        assert!(b.apply_mode("some-future-mode"));
        assert_eq!(b.current().name, "en_us_qwerty");
    }

    /// A layout swap ends a one-shot shift: it was armed for a letter on the board you just
    /// left.
    #[test]
    fn switching_boards_disarms_a_one_shot_shift() {
        let mut b = Board::new();
        b.tap_shift();
        assert_eq!(b.shift(), real_layout::Shift::Once);
        b.apply_mode("cardinal");
        assert_eq!(b.shift(), real_layout::Shift::Off, "the shift followed you to another board");
    }

    /// GTK's own `rebuild_view` hardcodes `DominantSide::Right` under a comment saying "a
    /// future setting will let the user choose". This is that setting, and it has to reach
    /// the renderer — the previous three appearance fields did not, which is the whole
    /// reason this test exists.
    #[test]
    fn the_one_handed_setting_reaches_the_style() {
        use crate::halfboard::Side;
        let mut v = std::collections::HashMap::new();
        let base = Style::default();
        assert_eq!(base.half, Side::Off, "a keyboard must not start having picked a hand");

        v.insert("layout.hand".to_string(), "left".to_string());
        assert_eq!(base.with_settings(&v).half, Side::Left);
        v.insert("layout.hand".to_string(), "right".to_string());
        assert_eq!(base.with_settings(&v).half, Side::Right);
        v.insert("layout.hand".to_string(), "off".to_string());
        assert_eq!(base.with_settings(&v).half, Side::Off);

        // The panel offers exactly these three, so a value it can produce is always one the
        // renderer understands.
        let s = crate::settings_model::SETTINGS.iter().find(|s| s.key == "layout.hand").expect("offered");
        for value in s.values {
            v.insert("layout.hand".to_string(), (*value).to_string());
            let _ = base.with_settings(&v);
        }
        assert_eq!(s.values.len(), 3);
    }

    /// A config written by a newer build must not silently undo a setting this build does
    /// understand.
    #[test]
    fn unknown_settings_and_values_are_ignored_not_reset() {
        use crate::halfboard::Side;
        let mut v = std::collections::HashMap::new();
        let base = Style { half: Side::Left, ..Style::default() };

        // A value from the future: keep what we had rather than falling to Off.
        v.insert("layout.hand".to_string(), "arc-bottom-left".to_string());
        assert_eq!(base.with_settings(&v).half, Side::Left, "an unknown value reset the setting");

        // A key from the future: no effect, and the rest still applies.
        v.clear();
        v.insert("layout.thumb_polygon".to_string(), "…".to_string());
        v.insert("layout.hand".to_string(), "right".to_string());
        assert_eq!(base.with_settings(&v).half, Side::Right);

        // An empty map changes nothing at all.
        assert_eq!(base.with_settings(&Default::default()).half, Side::Left);
    }

    /// `AppearanceConfig` has carried these since the GTK build and `settings::ready`
    /// reported the section live, while the renderer read none of them. A setting that
    /// persists and changes nothing is worse than one that is missing: the operator
    /// concludes the feature is broken rather than absent.
    #[test]
    fn the_appearance_settings_reach_the_renderer() {
        // The default is what the board looked like before any of this, so every existing
        // call site draws exactly what it drew.
        let d = Style::default();
        assert_eq!(d.key_radius, None);
        assert_eq!(d.opacity, 1.0);
        assert!(d.magnifier);
        assert_eq!(d.radius_for(44.0), (44.0f32 * 0.18) as u8, "the default is the old rule");

        // A set radius is honoured, and stays proportional-free.
        let s = Style::from_appearance(4.0, 0.8, false);
        assert_eq!(s.radius_for(44.0), 4);
        assert_eq!(s.radius_for(28.0), 4, "a fixed radius is fixed at both key sizes");
        assert!(!s.magnifier);
        assert!((s.opacity - 0.8).abs() < 1e-6);
    }

    /// These arrive from a TOML file a person can edit.
    #[test]
    fn absurd_settings_are_clamped_rather_than_obeyed() {
        // A radius of 400 makes every key a circle with no glyph in it — and half the key
        // height is the most a rounded rectangle can take before it stops being one.
        let huge = Style::from_appearance(400.0, 1.0, true);
        assert_eq!(huge.radius_for(44.0), 22, "a radius past half the height");
        assert!(huge.key_radius.unwrap() <= 24.0);

        // An opacity of 0 is a keyboard you cannot see but still type on, which is
        // indistinguishable from the app having crashed.
        let gone = Style::from_appearance(8.0, 0.0, true);
        assert!(gone.opacity >= 0.05, "an invisible keyboard was allowed");
        let over = Style::from_appearance(8.0, 4.0, true);
        assert_eq!(over.opacity, 1.0);
        // negatives too, since a hand-edited file can hold them
        assert!(Style::from_appearance(-5.0, -1.0, true).opacity >= 0.05);
        assert_eq!(Style::from_appearance(-5.0, 1.0, true).radius_for(44.0), 0);
    }

    /// On a phone your thumb covers the key it is pressing, so the magnifier is the only
    /// way to see what went in. The GTK board has `.key-magnifier`; this port had nothing.
    #[test]
    fn only_character_keys_get_a_magnifier() {
        let l = Layout::qwerty();
        let by = |lbl: &str| l.rows.iter().flat_map(|r| &r.keys).find(|k| k.label == lbl)
            .unwrap_or_else(|| panic!("no {lbl} key"));
        assert!(wants_magnifier(by("a")));
        assert!(wants_magnifier(by("q")));
        // Magnifying a modifier tells you nothing you did not know from the key you aimed
        // at, and a bubble over the space bar is a bubble over half the board.
        assert!(!wants_magnifier(by("⇧")));
        assert!(!wants_magnifier(by("?123")));
        assert!(!wants_magnifier(by("⏎")), "the commit key is not a character");
        assert!(!wants_magnifier(by(" ")), "space is not a glyph worth showing");
        // A latched key is showing its state; a bubble on top of that hides the state.
        let mut shifted = by("a").clone();
        shifted.latch = Latch::Once;
        assert!(!wants_magnifier(&shifted));
        // A word on a wide key is not a character either.
        assert!(!wants_magnifier(&Key::wide("space", 5.0)));
    }

    /// The outermost keys are exactly the ones whose bubble would hang off the edge — and on
    /// a phone the board IS the screen, so there is nothing beside it to hang over.
    #[test]
    fn the_magnifier_stays_on_the_board() {
        let bounds = Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 200.0));
        let mid = Rect::from_min_size(egui::pos2(160.0, 100.0), egui::vec2(32.0, 44.0));
        let m = magnifier_rect(mid, bounds, 1.0);
        assert!(bounds.contains_rect(m), "{m:?} escaped {bounds:?}");
        assert!(m.bottom() <= mid.top(), "the bubble covered the key it belongs to");
        assert!((m.center().x - mid.center().x).abs() < 0.6, "not centred over its key");

        // Leftmost and rightmost keys are pushed in rather than clipped.
        let left = Rect::from_min_size(egui::pos2(0.0, 100.0), egui::vec2(32.0, 44.0));
        let ml = magnifier_rect(left, bounds, 1.0);
        assert!(ml.left() >= bounds.left() - 0.01, "{ml:?}");
        let right = Rect::from_min_size(egui::pos2(328.0, 100.0), egui::vec2(32.0, 44.0));
        let mr = magnifier_rect(right, bounds, 1.0);
        assert!(mr.right() <= bounds.right() + 0.01, "{mr:?}");

        // TOP ROW: the bubble would float off the board, so it is allowed to overlap the
        // key rather than disappear. Visible and slightly in the way beats invisible.
        let top = Rect::from_min_size(egui::pos2(160.0, 0.0), egui::vec2(32.0, 44.0));
        let mt = magnifier_rect(top, bounds, 1.0);
        assert!(mt.top() >= bounds.top() - 0.01, "the bubble floated off the top: {mt:?}");
        assert!(bounds.contains_rect(mt));

        // Wide enough to read at 28pt even over a narrow key.
        assert!(m.width() >= MAGNIFIER_MIN_W);
    }

    /// A LIVE MICROPHONE MUST BE VISIBLE ON THE SURFACE YOU ARE LOOKING AT. The screen had
    /// a "● dictating" label in a toolbar the layer-shell build never draws — on a phone the
    /// keyboard is the whole surface, so the only indicator the mic was open was off-screen.
    #[test]
    fn the_mic_key_shows_that_the_microphone_is_open() {
        use crate::real_layout::{classify, Press};
        let mut b = Board::new();
        let mic_latch = |b: &Board| {
            b.rendered()
                .rows
                .iter()
                .flat_map(|r| &r.keys)
                .find(|k| classify(&k.emit) == Press::Voice)
                .map(|k| k.latch)
        };
        assert_eq!(mic_latch(&b), Some(Latch::Off));
        b.set_voice_active(true);
        assert_eq!(mic_latch(&b), Some(Latch::Live), "the mic key does not show a live microphone");
        b.set_voice_active(false);
        assert_eq!(mic_latch(&b), Some(Latch::Off), "it kept pulsing after the mic closed");

        // Only the mic. A board where every key looked live would say nothing.
        b.set_voice_active(true);
        let others: Vec<Latch> = b
            .rendered()
            .rows
            .iter()
            .flat_map(|r| &r.keys)
            .filter(|k| classify(&k.emit) != Press::Voice)
            .map(|k| k.latch)
            .collect();
        assert!(others.iter().all(|l| *l != Latch::Live), "another key latched Live");
    }

    /// Voice and shift are independent: recording while caps is locked must show both, and
    /// `rendered`'s early return has to account for either being set.
    #[test]
    fn voice_and_shift_do_not_cancel_each_other() {
        use crate::real_layout::{classify, Press, Shift};
        let mut b = Board::new();
        b.set_voice_active(true);
        b.tap_shift();
        assert_eq!(b.shift(), Shift::Once);
        let r = b.rendered();
        let find = |p: Press| r.rows.iter().flat_map(|x| &x.keys).find(|k| classify(&k.emit) == p).map(|k| k.latch);
        assert_eq!(find(Press::Voice), Some(Latch::Live));
        assert_eq!(find(Press::Shift), Some(Latch::Once));
        // and the letters upshifted, which is the shift half still working
        assert!(r.rows.iter().flat_map(|x| &x.keys).any(|k| k.label == "A"));
    }

    /// The pulse breathes rather than blinks, and never fades to nothing — a key that
    /// reaches zero reads as OFF at the bottom of every cycle, which is the opposite of
    /// what it is saying.
    #[test]
    fn the_live_pulse_never_goes_dark() {
        let mut min = 255u8;
        let mut max = 0u8;
        // A whole period, finely sampled.
        for i in 0..400 {
            let a = live_pulse(i as f64 * LIVE_PERIOD / 200.0);
            min = min.min(a);
            max = max.max(a);
        }
        assert!(min >= LIVE_MIN, "the pulse dipped to {min}, below the floor");
        assert!(max >= 250, "the pulse never reached full: {max}");
        assert!(LIVE_PERIOD > 1.0, "faster than this is a blink, not a breath");
        // It is periodic: the same phase a period later is the same value.
        for t in [0.0f64, 0.3, 0.9] {
            assert_eq!(live_pulse(t), live_pulse(t + LIVE_PERIOD));
        }
    }

    /// The reported desktop fault: tall, blurry caps. Every glyph the board paints must be
    /// at a size that is a whole number of DEVICE pixels, at whatever scale the desktop is
    /// running — otherwise the stems land between columns and the text softens.
    #[test]
    fn every_cap_label_is_rasterised_on_the_pixel_grid() {
        let board = Board::new();
        for ppp in [1.0f32, 1.25, 1.5, 2.0] {
            let ctx = egui::Context::default();
            ctx.set_pixels_per_point(ppp);
            let mut input = egui::RawInput::default();
            input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(731.0, 263.0)));
            let out = ctx.run(input, |ctx| {
                egui::CentralPanel::default().show(ctx, |ui| {
                    let _ = keyboard_ui(ui, board.current(), &Adaptive::detect(ctx), "system");
                });
            });
            let sizes: Vec<f32> = uikit_egui::layout_probe::texts(&out.shapes).iter().map(|t| t.font_pt).collect();
            assert!(!sizes.is_empty(), "no labels painted at {ppp}x");
            for s in &sizes {
                let px = s * ppp;
                assert!(
                    (px - px.round()).abs() < 0.02,
                    "a cap at {ppp}x is {s}pt = {px} device pixels — a fractional glyph is the blur",
                );
            }
            // and a deliberately awkward width does not change that
            assert!(sizes.iter().all(|s| *s >= 10.0), "a cap shrank below legibility: {sizes:?}");
        }
    }

    /// The ribbon is a strip of words with one hairline under it, not a second row of keys.
    /// The port drew default egui Buttons: each candidate came with its own box and border.
    #[test]
    fn the_suggestion_ribbon_is_pills_and_one_hairline() {
        use board_gestures::WordCandidate;
        use crate::swipe::{candidate_bar, BAR_HEIGHT};
        let cands: Vec<WordCandidate> = ["the", "then", "there"]
            .iter()
            .map(|w| WordCandidate { word: (*w).to_string(), score: 1.0 })
            .collect();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 200.0)));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let a = Adaptive { min_touch_target: 44.0, gutter: 4.0, ..Adaptive::default() };
                let _ = candidate_bar(ui, &cands, &a);
            });
        });
        // No pill paints a border or a fill at rest: a resting ribbon is words on the
        // surface. (Hover and press tint, but nothing is hovered in a headless frame.)
        let filled: Vec<&egui::epaint::RectShape> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) => Some(r),
                _ => None,
            })
            .filter(|r| r.fill.a() > 0 || r.stroke.width > 0.0)
            // The panel's own background is a rect too, and it is the full frame; a PILL is
            // word-sized. Anything smaller than a third of the width inside the ribbon is
            // one, and there should be none of them at rest.
            .filter(|r| r.rect.width() < 120.0 && r.rect.height() < 60.0)
            .collect();
        assert!(filled.is_empty(), "a resting pill painted a box: {:?}", filled.iter().map(|r| r.rect).collect::<Vec<_>>());
        // and the three words are there, at the stylesheet's size
        let texts = uikit_egui::layout_probe::texts(&out.shapes);
        assert_eq!(texts.len(), 3, "{:?}", texts.iter().map(|t| &t.text).collect::<Vec<_>>());
        assert!(texts.iter().all(|t| (t.font_pt - crate::swipe::PILL_FONT_PT).abs() < 0.6), "{:?}", texts[0].font_pt);
        // the hairline: one horizontal line at the ribbon's bottom
        let lines = out.shapes.iter().filter(|s| matches!(s.shape, egui::Shape::LineSegment { .. })).count();
        assert_eq!(lines, 1, "expected exactly one hairline under the ribbon, saw {lines}");
        assert!(BAR_HEIGHT >= 40.0);
    }

    /// The press moves the cap (`transform: scale(0.94)`) about its own centre, so a pressed
    /// key does not nudge its neighbours.
    #[test]
    fn a_pressed_cap_shrinks_about_its_centre() {
        let r = Rect::from_min_size(egui::pos2(10.0, 20.0), egui::vec2(40.0, 44.0));
        let p = shrink_about_centre(r, PRESS_SCALE);
        assert_eq!(p.center(), r.center(), "the cap moved sideways under the thumb");
        assert!(p.width() < r.width() && p.height() < r.height());
        assert!((p.width() / r.width() - PRESS_SCALE).abs() < 1e-5);
        assert!(r.contains_rect(p));
    }

    /// The board is a panel that slid up from the bottom edge: rounded on top, square below.
    #[test]
    fn the_board_is_rounded_on_top_only() {
        assert_eq!(BOARD_RADIUS, 16, "the GTK stylesheet says 16px 16px 0 0");
        let board = Board::new();
        let ctx = egui::Context::default();
        let mut input = egui::RawInput::default();
        input.screen_rect = Some(Rect::from_min_size(egui::Pos2::ZERO, egui::vec2(360.0, 260.0)));
        let out = ctx.run(input, |ctx| {
            egui::CentralPanel::default().show(ctx, |ui| {
                let _ = keyboard_ui(ui, board.current(), &Adaptive::detect(ctx), "system");
            });
        });
        // the board's own fill is the first rect, and it carries the asymmetric radius
        let radii: Vec<egui::CornerRadius> = out
            .shapes
            .iter()
            .filter_map(|s| match &s.shape {
                egui::Shape::Rect(r) => Some(r.corner_radius),
                _ => None,
            })
            .collect();
        assert!(
            radii.iter().any(|r| r.nw == BOARD_RADIUS && r.sw == 0),
            "no rect with a rounded top and a square bottom was painted"
        );
    }

/// The key geometry, without egui: `key_rects` is what the painter, `hit_test` and the
/// swipe hitboxes all read, so its arithmetic is pinned here directly.
///
/// Replaces `row_spending`, which modelled the egui flow layout's budget (`gutter * (n+1)`
/// reserved, egui's `item_spacing.x` actually spent). Keys are placed at computed rects
/// now, so there is no second quantity for the budget to disagree with.
#[cfg(test)]
mod key_geometry {
    use super::*;

    fn ad(key_h: f32, gutter: f32) -> Adaptive {
        Adaptive { min_touch_target: key_h, gutter, ..Adaptive::default() }
    }

    /// Every row spends exactly its width: a gutter at each end and between every pair.
    #[test]
    fn every_row_spends_exactly_its_width_with_a_gutter_everywhere() {
        let full = Rect::from_min_size(egui::pos2(10.0, 20.0), Vec2::new(600.0, 300.0));
        for layout in [Layout::qwerty(), Layout::cardinal8(), Layout::symbols()] {
            for g in [4.0f32, 6.0, 8.0] {
                let rects = key_rects(&layout, full, &ad(46.0, g), &Style::default());
                assert_eq!(rects.len(), layout.rows.len());
                for rs in &rects {
                    assert!(!rs.is_empty());
                    assert!((rs[0].left() - (full.left() + g)).abs() < 1e-3, "left gutter");
                    assert!((full.right() - rs.last().unwrap().right() - g).abs() < 1e-3, "right gutter: {rs:?}");
                    for w in rs.windows(2) {
                        assert!((w[1].left() - w[0].right() - g).abs() < 1e-3, "gap between keys");
                    }
                }
            }
        }
    }

    /// Rows are `key_h` tall and `key_h + gutter` apart, from the top of the board.
    #[test]
    fn rows_are_key_high_and_a_gutter_apart() {
        let full = Rect::from_min_size(egui::pos2(0.0, 7.0), Vec2::new(500.0, 400.0));
        let a = ad(46.0, 4.0);
        let rects = key_rects(&Layout::qwerty(), full, &a, &Style::default());
        for (i, rs) in rects.iter().enumerate() {
            for r in rs {
                assert!((r.top() - (7.0 + i as f32 * 50.0)).abs() < 1e-3);
                assert!((r.height() - 46.0).abs() < 1e-3);
            }
        }
        let last = rects.last().unwrap()[0];
        assert!((board_height(&Layout::qwerty(), &a) - (last.bottom() + 4.0 - 7.0)).abs() < 1e-3,
            "board_height must be exactly what the rows occupy");
    }

    /// Wider than GTK's MAX_KEYBOARD_WIDTH, the board is centred at that width; narrower,
    /// it uses all of it.
    #[test]
    fn a_wide_window_centres_a_720pt_board() {
        let a = ad(46.0, 4.0);
        let wide = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(1200.0, 300.0));
        let rs = &key_rects(&Layout::qwerty(), wide, &a, &Style::default())[0];
        let (l, r) = (rs[0].left() - 4.0, rs.last().unwrap().right() + 4.0);
        assert!(((r - l) - MAX_BOARD_W).abs() < 1e-3, "board width {}", r - l);
        assert!(((l + r) * 0.5 - 600.0).abs() < 1e-3, "not centred: {l}..{r}");
        let narrow = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(400.0, 300.0));
        let rs = &key_rects(&Layout::qwerty(), narrow, &a, &Style::default())[0];
        assert!((rs.last().unwrap().right() - 396.0).abs() < 1e-3);
    }

    /// The tap hit-test answers from the same rects: every key's centre hits that key, and
    /// a point in the gap between two keys hits neither.
    #[test]
    fn hit_test_answers_from_the_painted_rects() {
        let a = ad(46.0, 6.0);
        let full = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(480.0, 300.0));
        let layout = Layout::qwerty();
        let rects = key_rects(&layout, full, &a, &Style::default());
        let mut n = 0;
        for (row, rs) in layout.rows.iter().zip(&rects) {
            for (k, r) in row.keys.iter().zip(rs) {
                assert_eq!(hit_test(&layout, full, &a, r.center()).as_deref(), Some(k.emit.as_str()));
                n += 1;
            }
            let gap = egui::pos2((rs[0].right() + rs[1].left()) * 0.5, rs[0].center().y);
            assert_eq!(hit_test(&layout, full, &a, gap), None, "the gap is not a key");
        }
        assert!(n > 25, "the scan found {n} keys");
    }

    /// A one-handed, arced board keeps every row inside the (centred) board.
    #[test]
    fn an_arced_row_stays_inside_the_board() {
        let a = ad(46.0, 6.0);
        let full = Rect::from_min_size(egui::Pos2::ZERO, Vec2::new(700.0, 300.0));
        let style = Style { half: crate::halfboard::Side::Left, arc: true, ..Style::default() };
        let rects = key_rects(&Layout::qwerty(), full, &a, &style);
        for rs in &rects {
            assert!(rs[0].left() >= full.left() - 1e-3 && rs.last().unwrap().right() <= full.right() + 1e-3);
        }
    }
}
}


/// What a pressed cap looks like: its SIZE, across a tap and a drag.
#[cfg(test)]
mod press_face_tests {
    use super::*;
    use egui::{Event, Modifiers, PointerButton, Pos2, RawInput};

    /// The painted cap rect under `probe`, each frame of press -> drag -> release.
    fn cap_sizes(layout: &Layout, palette: &str, probe: Pos2, drag_to: Pos2) -> Vec<Vec2> {
        let ctx = egui::Context::default();
        let adaptive = Adaptive { min_touch_target: KEY_H, gutter: KEY_GAP, ..Adaptive::default() };
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let key = key_rects(layout, screen, &adaptive, &Style::default())
            .into_iter()
            .flatten()
            .find(|r| r.contains(probe))
            .expect("a key under the probe");
        let mut t = 0.0;
        let mut run = |events: Vec<Event>| {
            t += 0.03;
            let raw = RawInput { events, time: Some(t), screen_rect: Some(screen), ..Default::default() };
            let out = ctx.run(raw, |ctx| {
                egui::CentralPanel::default().frame(egui::Frame::NONE).show(ctx, |ui| {
                    let _ = keyboard_ui(ui, layout, &adaptive, palette);
                });
            });
            // The cap: the rect shape centred on the key, about its size.
            out.shapes
                .iter()
                .filter_map(|c| match &c.shape {
                    egui::Shape::Rect(r) => Some(r.rect),
                    _ => None,
                })
                .filter(|r| (r.center() - key.center()).length() < 2.0 && r.width() > key.width() * 0.8 && r.width() <= key.width() + 1.0)
                .map(|r| r.size())
                .next()
                .expect("the cap was painted")
        };
        let btn = |pos, pressed| Event::PointerButton { pos, button: PointerButton::Primary, pressed, modifiers: Modifiers::default() };
        let mut sizes = vec![run(vec![]), run(vec![])];
        sizes.push(run(vec![Event::PointerMoved(probe), btn(probe, true)]));
        sizes.push(run(vec![]));
        let mid = probe + (drag_to - probe) * 0.5;
        sizes.push(run(vec![Event::PointerMoved(mid)]));
        sizes.push(run(vec![Event::PointerMoved(drag_to)]));
        sizes.push(run(vec![btn(drag_to, false)]));
        sizes.push(run(vec![]));
        sizes
    }

    /// "The swipe cardinal keys change size on tap and on flick/drag": a cardinal cap is the
    /// same size at rest, pressed, and mid-flick, in both palettes.
    #[test]
    fn a_cardinal_cap_holds_its_size_through_a_tap_and_a_flick() {
        let layout = Layout::cardinal8();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let adaptive = Adaptive { min_touch_target: KEY_H, gutter: KEY_GAP, ..Adaptive::default() };
        let k = key_rects(&layout, screen, &adaptive, &Style::default())[1][1];
        for palette in ["system", "dark"] {
            let s = cap_sizes(&layout, palette, k.center(), k.center() + Vec2::new(0.0, 30.0));
            assert!(s.iter().all(|v| (*v - s[0]).length() < 0.01), "{palette}: the cap changed size: {s:?}");
        }
    }

    /// Plain keys: glass flips colour only (GTK's glass rules set no transform); the carved
    /// palette keeps GTK's `scale(0.94)`. Pins both halves, so dropping the scale everywhere
    /// is caught as surely as keeping it on glass.
    #[test]
    fn a_plain_cap_scales_only_on_the_carved_palette() {
        let layout = Layout::qwerty();
        let screen = Rect::from_min_size(Pos2::ZERO, Vec2::new(400.0, 300.0));
        let adaptive = Adaptive { min_touch_target: KEY_H, gutter: KEY_GAP, ..Adaptive::default() };
        let k = key_rects(&layout, screen, &adaptive, &Style::default())[1][2];
        let glass = cap_sizes(&layout, "system", k.center(), k.center());
        assert!(glass.iter().all(|v| (*v - glass[0]).length() < 0.01), "glass cap changed size: {glass:?}");
        let carved = cap_sizes(&layout, "dark", k.center(), k.center());
        let pressed = carved[3];
        assert!((pressed.x / carved[0].x - PRESS_SCALE).abs() < 0.02, "carved cap did not press-scale: {carved:?}");
    }
}

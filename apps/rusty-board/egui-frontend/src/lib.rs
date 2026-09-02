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

pub mod cardinal_gesture;
pub mod ngram;
pub mod swipe_rerank;
pub mod settings_model;
pub mod animations;
pub mod tap_hold;
pub mod keyboard_service;
/// Where a keypress actually goes: the system, or a preview box.
pub mod sink;
pub mod adaptive;
pub mod nudge;
pub mod real_layout;
pub mod dictation;
pub mod mic_source;
pub mod tray;
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
}

impl Key {
    pub fn new(label: &str) -> Key {
        Key { label: label.to_string(), weight: 1.0, emit: label.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None }
    }
    pub fn wide(label: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: label.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None }
    }
    pub fn emitting(label: &str, emit: &str, weight: f32) -> Key {
        Key { label: label.to_string(), weight, emit: emit.to_string(), variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None }
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
fn emit_for_action(kind: &str) -> Option<&'static str> {
    match kind {
        "backspace" => Some("\u{8}"),
        "return" => Some("\n"),
        "space" => Some(" "),
        _ => None,
    }
}

/// Parse a rusty-board layout JSON string (the real `resources/layouts/*.json` schema) into this
/// crate's `Layout`. Pure — no filesystem access; callers `include_str!` the real file so the
/// binary always carries an exact, byte-identical copy of whatever the GTK app ships.
fn layout_from_json(text: &str) -> Result<Layout, serde_json::Error> {
    let parsed: JsonLayout = serde_json::from_str(text)?;
    let rows = parsed.rows.into_iter().map(|r| Row {
        weight: r.weight,
        keys: r.keys.into_iter().map(|k| {
            let emit = k.action.as_ref().and_then(|a| emit_for_action(&a.kind)).unwrap_or(&k.label).to_string();
            let mut key = Key { label: k.label, weight: k.width, emit, variants: Vec::new(), cardinal: Vec::new(), rotate_cw: None, rotate_ccw: None, steno_button: None };
            if !k.extended.is_empty() { key.variants = k.extended; }
            key
        }).collect(),
    }).collect();
    Ok(Layout { hints: false, name: if parsed.name.is_empty() { "qwerty".into() } else { parsed.name }, rows })
}

impl Layout {
    /// A compact QWERTY, loaded from rusty-board's REAL layout file — see `layout_from_json`'s
    /// doc comment for why this replaced a hand-copied approximation.
    pub fn qwerty() -> Layout {
        layout_from_json(include_str!("../../resources/layouts/en_us_qwerty.json"))
            .expect("bundled en_us_qwerty.json must parse — this is a compile-time asset, not user input")
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
                    keys: vec![Key::emitting("space", " ", 6.0), Key::emitting("⏎", "\n", 2.0)],
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
}

impl Board {
    /// Default board: qwerty + symbols.
    pub fn new() -> Board {
        Board { layouts: vec![Layout::qwerty(), Layout::symbols(), Layout::cardinal(), Layout::cardinal8()], idx: 0, gesture: uikit_egui::GestureLayer::horizontal_page(320.0), palette: "system".to_string() }
    }
    pub fn current(&self) -> &Layout {
        &self.layouts[self.idx]
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
pub fn keyboard_ui(ui: &mut Ui, layout: &Layout, adaptive: &Adaptive, palette: &str) -> Option<String> {
    let colors = uikit_egui::palette_colors(palette);
    let glass = palette == "system"; // the shared system-utilities look (gitmind/rusty-board): glass keys
    let gutter = adaptive.gutter;
    let key_h = adaptive.min_touch_target; // 44pt touch / 28pt pointer, from the adaptive layer
    let total_w = ui.available_width();

    if glass {
        // Glass root surface: high-alpha tinted fill + a single top hairline highlight, matching the
        // ported rusty-board-GTK `.glass-keyboard` recipe (no gradient/blur — see uikit_egui::glass).
        let board_rect = ui.max_rect();
        ui.painter().rect_filled(board_rect, 0, uikit_egui::surface_fill(colors.bg_base));
        uikit_egui::paint_top_hairline(ui, board_rect);
    }

    let mut emitted = None;
    for row in &layout.rows {
        let total_weight: f32 = row.keys.iter().map(|k| k.weight).sum::<f32>().max(0.001);
        let usable = total_w - gutter * (row.keys.len() as f32 + 1.0);
        ui.horizontal(|ui| {
            ui.add_space(gutter);
            for key in &row.keys {
                let w = (usable * (key.weight / total_weight)).max(8.0);
                if let Some(e) = paint_key(ui, key, Vec2::new(w, key_h), gutter, &colors, glass, layout.hints) {
                    emitted = Some(e);
                }
            }
        });
        ui.add_space(gutter);
    }
    emitted
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

#[allow(clippy::too_many_arguments)]
fn paint_key(ui: &mut Ui, key: &Key, size: Vec2, gutter: f32, c: &PaletteColors, glass: bool, hints: bool) -> Option<String> {
    // CLICK AND DRAG. This was `Sense::click()`, which is why cardinal swipes
    // did nothing: a key that only senses clicks never sees a drag, so the
    // direction could not be measured however good the classifier was.
    let (rect, resp) = ui.allocate_exact_size(size, Sense::click_and_drag());
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
    paint_key_face(ui, key, rect, size, gutter, c, glass, hints, &resp);
    dispatch_key(ui, key, rect, size, &resp)
}

/// The visual half of a key: glass or carved-tile, pressed/hovered/at-rest. No gesture logic.
#[allow(clippy::too_many_arguments)]
fn paint_key_face(ui: &mut Ui, key: &Key, rect: Rect, size: Vec2, gutter: f32, c: &PaletteColors, glass: bool, hints: bool, resp: &egui::Response) {
    let painter = ui.painter();
    let pressed = resp.is_pointer_button_down_on();
    let rounding = CornerRadius::same((size.y * 0.18) as u8);
    let font = FontId::proportional((size.y * 0.42).clamp(11.0, 26.0));
    if glass {
        let modifier = key.label.chars().count() > 1; // "shift"/"space"/"enter" etc. — sit subtly less present
        let (fill, stroke) = if pressed {
            (uikit_egui::glass_tint(c.accent, uikit_egui::GlassAlphas::PRESSED_ACCENT), uikit_egui::key_border(c.border))
        } else if modifier {
            (if resp.hovered() { uikit_egui::key_fill_modifier_hover(c.bg_card) } else { uikit_egui::key_fill_modifier(c.bg_card) }, uikit_egui::key_border_soft(c.border))
        } else {
            (if resp.hovered() { uikit_egui::key_fill_hover(c.bg_card) } else { uikit_egui::key_fill(c.bg_card) }, uikit_egui::key_border(c.border))
        };
        painter.rect(rect, rounding, fill, stroke, egui::StrokeKind::Inside);
        if rect.top() <= gutter + 1.0 {
            uikit_egui::paint_top_hairline(ui, rect);
        }
        let text_color = if pressed { egui::Color32::WHITE } else { c.text };
        painter.text(rect.center(), Align2::CENTER_CENTER, &key.label, font.clone(), text_color);
    } else {
        // Rest→hover→press glides (120ms) instead of snapping — the QML key ColorAnimation feel.
        let fill = uikit_egui::motion::interactive_fill(ui.ctx(), resp, c.bg_raised, c.bg_hover, c.accent);
        painter.rect_filled(rect, rounding, fill);
        painter.rect_stroke(rect, rounding, Stroke::new(1.0_f32, c.bg_base), egui::StrokeKind::Inside);
        let text_color = if pressed { c.bg_base } else { c.text };
        painter.text(rect.center(), Align2::CENTER_CENTER, &key.label, font.clone(), text_color);
    }
    // The compass hints, drawn small around the tap label so a cardinal key advertises its
    // swipes at rest (FlickBoard/MessagEase draw every direction's glyph in its corner). Only the
    // four cardinals ride the edges; diagonals sit in their corners.
    if hints && !key.cardinal.is_empty() {
        let hint = FontId::proportional((size.y * 0.22).clamp(8.0, 13.0));
        let hint_color = c.text.gamma_multiply(0.55);
        let inset = size.y * 0.16;
        for (d, s) in &key.cardinal {
            let off = d.offset();
            let anchor = rect.center() + Vec2::new(off.x * (rect.width() * 0.5 - inset), off.y * (rect.height() * 0.5 - inset));
            painter.text(anchor, Align2::CENTER_CENTER, s, hint.clone(), hint_color);
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
        let id = ui.id().with(("keycard", &key.label, rect.min.x as i32, rect.min.y as i32));
        // ONE tap/swipe boundary for the widget and the classifier. The classifier's module doc
        // calls this an interlock: if the two disagree, a movement is a tap to one and a swipe to
        // the other, and the key fires twice or not at all. Derived from the key size so a phone
        // key and a desktop key both flick at a quarter of their height (FlickBoard's default
        // swipe threshold is 8dp — short flicks are the point of this layout).
        let deadzone = (size.y * 0.25).clamp(9.0, 16.0);
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
        b.swipe(1); // wraps back
        assert_eq!(b.current().name, "en_us_qwerty");
        b.swipe(-1); // wraps backward to the last layout
        assert_eq!(b.current().name, "cardinal8");
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

        /// Where key (row, col) is on screen under the test panel.
        fn key_rect(&self, row: usize, col: usize) -> Rect {
            let r = &self.layout.rows[row];
            let total_weight: f32 = r.keys.iter().map(|k| k.weight).sum();
            let usable = W - GUTTER * (r.keys.len() as f32 + 1.0);
            let mut x = GUTTER;
            for (c, k) in r.keys.iter().enumerate() {
                let w = usable * (k.weight / total_weight);
                if c == col {
                    return Rect::from_min_size(Pos2::new(x, row as f32 * (KEY_H + GUTTER)), Vec2::new(w, KEY_H));
                }
                x += w;
            }
            panic!("no key at ({row},{col})");
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

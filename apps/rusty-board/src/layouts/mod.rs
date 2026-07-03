//! Keyboard layouts: loaded from JSON files in `resources/layouts/`.

pub mod builder;
pub mod io;

use crate::keyboard::{Key, KeyAction, KeyGestures};
use anyhow::{Context, Result};
use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Layout {
    pub name: String,
    pub display_name: String,
    pub language: String,
    pub direction: TextDirection,
    pub rows: Vec<Row>,
    /// The layout used when in symbols mode (often shared across languages).
    #[serde(default)]
    pub symbols_layout: Option<String>,
    /// True for ortholinear layouts (rows aligned in a strict grid, no
    /// row-to-row offset). When false, rows can have their own `offset`
    /// values and the layout renders with traditional row-stagger.
    #[serde(default)]
    pub ortholinear: bool,
    /// Number of "unit columns" the layout spans. Ortholinear layouts use
    /// this to pin every row to the same width regardless of content.
    /// Defaults to 10.
    #[serde(default = "default_grid_columns")]
    pub grid_columns: f32,
    /// Input style. `Standard` is the conventional tap/swipe-word
    /// keyboard. `CardinalGrid` is a MessagEase-style 3×3 with cardinal
    /// swipes / hairpins / rotation gestures per key.
    #[serde(default)]
    pub style: LayoutStyle,
}

/// How the user interacts with this layout — standard keys vs the
/// experimental cardinal-grid input style.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum LayoutStyle {
    /// Standard tap + swipe-word keyboard. Long-press opens popovers.
    Standard,
    /// Cardinal-grid: each key supports tap, 4-or-8-way cardinal swipe,
    /// hairpin (out-and-back) for capitalization, and CW/CCW rotation
    /// gestures for digits/symbols. Best with a 3×3 grid of dense keys.
    CardinalGrid,
    /// Stenotype: two-button-per-thumb pair-stroke keyboard. Each
    /// button has 5 gestures (tap + 4 cardinals = 5 phonemes), two
    /// buttons per thumb = 10 outcomes per thumb, paired = 100
    /// pair-strokes per stroke. Pairs commit as phonemes or look up
    /// in a brief table for common English words. Parallelogram-
    /// shaped buttons visually lean toward keyboard center (\\//).
    /// Reached via swipe-up on the ABC/?#= mode-toggle key.
    Stenotype,
}

impl Default for LayoutStyle {
    fn default() -> Self {
        LayoutStyle::Standard
    }
}

/// How the swipe/gesture dispatcher should treat a layout. Derived
/// once from `LayoutStyle` (see `LayoutStyle::dispatch_mode`) so the
/// gesture-handling code in `keyboard_view.rs` doesn't re-derive a
/// "is this cardinal-ish?" boolean at multiple sites — that
/// duplication is exactly what let the cardinal/steno gating drift
/// out of sync and produced the canvas-open + close-keyboard bugs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DispatchMode {
    /// QWERTY/Dvorak: full swipe-typing pipeline, spacebar-arrow
    /// gestures, minimize-fling, top-row digit swipes, canvas-open.
    Standard,
    /// Cardinal grid: per-cell directional dispatch only. None of the
    /// Standard freeform-swipe machinery runs.
    Cardinal,
    /// Stenotype: per-button steno-gesture dispatch. Like Cardinal in
    /// that the Standard machinery is suppressed, but the gestures
    /// route into the steno engine rather than the cardinal letter
    /// tables.
    Stenotype,
}

impl DispatchMode {
    /// True for modes where the Standard freeform-swipe machinery
    /// (swipe-typing recognizer, spacebar-arrow, minimize-fling,
    /// top-row digit, canvas-open) should be SUPPRESSED. Both
    /// Cardinal and Stenotype suppress it; only Standard runs it.
    ///
    /// This is the replacement for the old `is_cardinal` boolean —
    /// keeping a single named predicate so call sites read clearly
    /// ("if dispatch_mode.suppresses_freeform_swipe()") instead of a
    /// bare matches! that someone could forget to extend when a new
    /// mode is added.
    pub fn suppresses_freeform_swipe(self) -> bool {
        !matches!(self, DispatchMode::Standard)
    }

    /// True for the two experimental per-key directional modes that
    /// route through `dispatch_cardinal_swipe`.
    pub fn uses_cardinal_dispatch(self) -> bool {
        matches!(self, DispatchMode::Cardinal | DispatchMode::Stenotype)
    }
}

impl LayoutStyle {
    /// Classify this layout style into a `DispatchMode`. Single
    /// source of truth for the gesture dispatcher.
    pub fn dispatch_mode(self) -> DispatchMode {
        match self {
            LayoutStyle::Standard => DispatchMode::Standard,
            LayoutStyle::CardinalGrid => DispatchMode::Cardinal,
            LayoutStyle::Stenotype => DispatchMode::Stenotype,
        }
    }
}

fn default_grid_columns() -> f32 {
    10.0
}

#[derive(Debug, Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum TextDirection {
    Ltr,
    Rtl,
}

impl Default for TextDirection {
    fn default() -> Self {
        TextDirection::Ltr
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row {
    pub keys: Vec<Key>,
    /// Vertical "row weight" — usually all 1.0; tweak to make a row taller.
    #[serde(default = "default_row_weight")]
    pub weight: f32,
    /// Horizontal offset in unit-key widths. 0.0 = flush left (ortholinear);
    /// 0.25/0.5 = traditional QWERTY-style row stagger. Ignored when the
    /// parent layout's `ortholinear` flag is true.
    #[serde(default)]
    pub offset: f32,
}

fn default_row_weight() -> f32 {
    1.0
}

impl Layout {
    /// Is this layout a transient (overlay) page rather than the
    /// user's persistent letter layout?
    ///
    /// # Layer model
    ///
    /// The keyboard navigates between two kinds of layouts:
    ///
    ///   - **Primary** (qwerty / dvorak / cardinal / stenotype):
    ///     the user's persistent letter layout, chosen in Settings.
    ///     Only one is active at a time; switching is a settings
    ///     change.
    ///
    ///   - **Transient** (symbols / numbers / emoji / arrows):
    ///     overlay pages reached temporarily from the primary by
    ///     tapping a mode-toggle key (`?123`) or by a focus event
    ///     (arrows tray). The keyboard "remembers" the primary
    ///     while on a transient and returns there on `ABC` tap.
    ///
    /// # Two navigation axes
    ///
    /// Buttons that switch between layers come in two flavors:
    ///
    ///   - **Vertical** (primary ↔ transient):
    ///     - `?123` button on primary → first transient (Symbols).
    ///     - `ABC` button on transient → primary (always). This is
    ///       the "single tap home" guarantee — no matter which
    ///       transient page the user is on, `ABC` returns to their
    ///       letters.
    ///
    ///   - **Horizontal** (cycle within transients):
    ///     - `?#=` button on a transient → next transient page in
    ///       the cycle (Symbols ↔ Numbers today; future pages would
    ///       extend the cycle).
    ///
    /// # Adding a new transient layer (3rd, 4th, 5th)
    ///
    ///   1. Write a `Layout::default_xxx()` constructor returning
    ///      the new layout with `name: "xxx"` and the standard
    ///      `?#=` (cycle) + `ABC` (return-to-primary) buttons in
    ///      its bottom row.
    ///   2. Add `"xxx"` to the `is_transient_page` matcher below
    ///      so the StateChanged handler classifies it correctly.
    ///   3. If the new layer corresponds to a new engine state,
    ///      add the `KeypadState::Xxx` variant + the mapping in
    ///      `corresponding_keypad_state`.
    ///   4. Extend the engine's `KeyAction::Numbers` cycle to walk
    ///      into the new state.
    ///
    /// The `ABC` button's behavior is unconditional and doesn't
    /// require updates — it always restores `last_letter_layout`
    /// which is whatever primary the user was on before entering
    /// the transient chain.
    ///
    /// This is the single source of truth — the keyboard_view's
    /// StateChanged handler, the focus reconciler, and any future
    /// layer-cycle logic should ALL ask the Layout itself rather than
    /// re-deriving from name string matches at multiple sites.
    pub fn is_transient_page(&self) -> bool {
        matches!(
            self.name.as_str(),
            "symbols" | "numbers" | "emoji" | "arrows"
        )
    }

    /// The `KeypadState` the engine should be in while this layout
    /// is displayed. Used by view-layer paths that swap layouts
    /// without going through the engine (focus-driven tray, the
    /// ShowFullKeyboard handler) so engine state stays in sync with
    /// what's visually showing.
    ///
    /// Mismatch between engine state and visible layout causes
    /// real bugs — e.g., if engine thinks Symbols but view is on
    /// letters, the next `?123` tap toggles Symbols→Normal (because
    /// engine says we're already on Symbols) and nothing changes
    /// visually. The user sees a dead button.
    pub fn corresponding_keypad_state(&self) -> crate::keyboard::KeypadState {
        use crate::keyboard::KeypadState;
        match self.name.as_str() {
            "symbols" => KeypadState::Symbols,
            "numbers" => KeypadState::Numbers,
            "emoji" => KeypadState::Emoji,
            // Arrows is a view-only overlay; the engine doesn't need
            // a dedicated state for it. Normal is correct — the
            // arrows keys emit keysym presses directly, not text.
            _ => KeypadState::Normal,
        }
    }

    pub fn load_from_file(path: &Path) -> Result<Self> {
        let s = std::fs::read_to_string(path)
            .with_context(|| format!("reading layout {}", path.display()))?;
        let layout: Self = serde_json::from_str(&s)
            .with_context(|| format!("parsing layout {}", path.display()))?;
        Ok(layout)
    }

    /// Build a sensible default QWERTY layout in code, so the keyboard works
    /// even if the resources directory is missing.
    pub fn default_qwerty() -> Self {
        Self {
            name: "en_us_qwerty".into(),
            display_name: "English (QWERTY)".into(),
            language: "en".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row {
                    keys: vec![
                        char_with_long_press("q", &["1"]),
                        char_with_long_press("w", &["2"]),
                        char_with_long_press("e", &["3", "é", "è", "ê", "ë"]),
                        char_with_long_press("r", &["4"]),
                        char_with_long_press("t", &["5"]),
                        char_with_long_press("y", &["6"]),
                        char_with_long_press("u", &["7", "ü", "ú", "ù"]),
                        char_with_long_press("i", &["8", "í", "ï", "î"]),
                        char_with_long_press("o", &["9", "ó", "ò", "ô", "ö"]),
                        char_with_long_press("p", &["0"]),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        char_with_long_press("a", &["à", "á", "â", "ä", "å"]),
                        Key::character("s"),
                        Key::character("d"),
                        Key::character("f"),
                        Key::character("g"),
                        Key::character("h"),
                        Key::character("j"),
                        Key::character("k"),
                        Key::character("l"),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("⇧", KeyAction::Shift),
                        Key::character("z"),
                        Key::character("x"),
                        Key::character("c"),
                        Key::character("v"),
                        Key::character("b"),
                        Key::character("n"),
                        Key::character("m"),
                        Key::special("⌫", KeyAction::Backspace).with_width(1.5),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("?123", KeyAction::SymbolsToggle),
                        Key::special("🎤", KeyAction::Voice),
                        Key::modifier("✨", KeyAction::Llm),
                        Key::special(" ", KeyAction::Space).with_width(2.0),
                        Key::character(",")
                            .with_extended(vec!["!".into(), "?".into(), ";".into()]),
                        Key::character(".")
                            .with_extended(vec!["?".into(), "!".into(), "…".into()]),
                        Key::modifier("⚙", KeyAction::Settings).with_width(1.0),
                        Key::special("⏎", KeyAction::Return).with_width(1.5),
                    ],
                    weight: 1.0, offset: 0.0,
                },
            ],
            symbols_layout: Some("symbols".into()),
            ortholinear: false,
            grid_columns: 10.0,
            style: LayoutStyle::Standard,
        }
    }

    /// The standard symbols page.
    pub fn default_symbols() -> Self {
        Self {
            name: "symbols".into(),
            display_name: "Symbols".into(),
            language: "*".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row {
                    keys: vec![
                        Key::character("1"),
                        Key::character("2"),
                        Key::character("3"),
                        Key::character("4"),
                        Key::character("5"),
                        Key::character("6"),
                        Key::character("7"),
                        Key::character("8"),
                        Key::character("9"),
                        Key::character("0"),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::character("@"),
                        Key::character("#"),
                        Key::character("$"),
                        Key::character("_"),
                        Key::character("&"),
                        Key::character("-"),
                        Key::character("+"),
                        Key::character("("),
                        Key::character(")"),
                        Key::character("/"),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("?#=", KeyAction::Numbers).with_width(1.5),
                        Key::character("*"),
                        Key::character("\""),
                        Key::character("'"),
                        Key::character(":"),
                        Key::character(";"),
                        Key::character("!"),
                        Key::character("?"),
                        Key::special("⌫", KeyAction::Backspace).with_width(1.5),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("ABC", KeyAction::SymbolsToggle).with_width(1.5),
                        Key::special("🎤", KeyAction::Voice).with_width(1.0),
                        Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                        Key::special(" ", KeyAction::Space).with_width(4.0),
                        Key::character(",").with_width(0.5),
                        Key::character(".").with_width(0.5),
                        Key::special("⏎", KeyAction::Return).with_width(1.5),
                    ],
                    weight: 1.0, offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: false,
            grid_columns: 10.0,
            style: LayoutStyle::Standard,
        }
    }

    /// Ortholinear variant of the symbols page. Same content as
    /// `default_symbols` but every key is exactly 1 unit wide,
    /// matching the geometric feel of ortho letter layouts
    /// (stenotype, cardinal-grid). Used when the user toggles
    /// symbols from an ortho layout — without this, they'd land
    /// on the staggered Row 3 / Row 4 keys of `default_symbols`
    /// (which use widths 1.5/1.0/0.5/4.0) and the visual
    /// continuity is broken.
    ///
    /// Row 3 makes `?#=` and `⌫` 1.0 wide (was 1.5 each in the
    /// standard); the freed slot holds `=` which is useful for
    /// the typical ortho user's typing patterns (programmers /
    /// steno).
    ///
    /// Row 4 splits the wide spacebar into 4 separate 1.0-wide
    /// Space keys. Each one taps to a single space; this is
    /// equivalent to one wide spacebar UX-wise but renders
    /// uniformly.
    pub fn default_symbols_ortho() -> Self {
        Self {
            name: "symbols".into(),
            display_name: "Symbols (ortho)".into(),
            language: "*".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row {
                    keys: vec![
                        Key::character("1"),
                        Key::character("2"),
                        Key::character("3"),
                        Key::character("4"),
                        Key::character("5"),
                        Key::character("6"),
                        Key::character("7"),
                        Key::character("8"),
                        Key::character("9"),
                        Key::character("0"),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::character("@"),
                        Key::character("#"),
                        Key::character("$"),
                        Key::character("_"),
                        Key::character("&"),
                        Key::character("-"),
                        Key::character("+"),
                        Key::character("("),
                        Key::character(")"),
                        Key::character("/"),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("?#=", KeyAction::Numbers).with_width(1.0),
                        Key::character("*"),
                        Key::character("\""),
                        Key::character("'"),
                        Key::character(":"),
                        Key::character(";"),
                        Key::character("!"),
                        Key::character("?"),
                        Key::character("="),
                        Key::special("⌫", KeyAction::Backspace),
                    ],
                    weight: 1.0, offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::modifier("ABC", KeyAction::SymbolsToggle).with_width(1.0),
                        Key::special("🎤", KeyAction::Voice),
                        Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                        // 4 separate Space keys — uniform cells,
                        // same UX as one wide Space (each tap = one
                        // space character). The "spacebar feel"
                        // disappears, but ortho layouts already
                        // don't have that as a defining feature.
                        Key::special(" ", KeyAction::Space),
                        Key::special(" ", KeyAction::Space),
                        Key::special(" ", KeyAction::Space),
                        Key::special(" ", KeyAction::Space),
                        Key::character(","),
                        Key::character("."),
                        Key::special("⏎", KeyAction::Return),
                    ],
                    weight: 1.0, offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: true,
            grid_columns: 10.0,
            style: LayoutStyle::Standard,
        }
    }

    /// A numeric keypad — phone-style 3x4 grid with `+ - * / =` and a
    /// decimal. Useful for password fields and PIN entry. Total grid is
    /// 4 columns wide.
    /// Compact two-row tray shown when `behavior.focus_show_tray`
    /// is on AND a textbox just got focus. Has arrow keys
    /// (←/↑/↓/→), backspace, return, and a "Show keyboard" key
    /// (`KeyAction::ShowFullKeyboard`) that swaps the layout back
    /// to the full one. Useful when the user wants to navigate the
    /// focused text field without dismissing the keyboard entirely.
    ///
    /// 4 columns × 2 rows = 8 keys total. Each key is a single
    /// The arrows tray — a compact navigation surface shown when
    /// `focus_show_tray=true` and a text field receives focus.
    ///
    /// 5-col × 2-row uniform grid. The central key is a JOYSTICK
    /// (cardinal-on-keysym): tap = Down arrow, swipe N/E/W = Up/
    /// Right/Left. Separate one-cell ←/↑/→ keys flank it for
    /// precise single-character moves (tap), while the joystick
    /// lets a quick swipe move multiple cells without lifting
    /// (combined with the 200ms/35ms key-repeat override on
    /// arrow keysyms, navigation is fast).
    ///
    /// Row 1: ⌫ ↑ ⏎ ABC ✕
    /// Row 2: ← [JOY] → Space Space
    pub fn default_arrows() -> Self {
        // Joystick key: KeyAction::Keysym("Down") + gestures map.
        // The behavior() method returns Cardinal for Keysym+gestures
        // (see Key::behavior). dispatch_cardinal_swipe detects the
        // Keysym source and emits NamedKey with the gesture value
        // instead of the StenoCommit path that would type literal
        // text. Result: swipe N → Up arrow keysym fires.
        let joystick = Key::special("↓", KeyAction::Keysym("Down".into()))
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("Up".into()),
                e: Some("Right".into()),
                s: Some("Down".into()),
                w: Some("Left".into()),
                ..Default::default()
            });
        Self {
            name: "arrows".into(),
            display_name: "Arrows".into(),
            language: "*".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row {
                    keys: vec![
                        Key::special("⌫", KeyAction::Backspace),
                        Key::special("↑", KeyAction::Keysym("Up".into())),
                        Key::special("⏎", KeyAction::Return),
                        Key::modifier("ABC", KeyAction::ShowFullKeyboard).with_width(1.0),
                        Key::modifier("✕", KeyAction::HideKeyboard).with_width(1.0),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                Row {
                    keys: vec![
                        Key::special("←", KeyAction::Keysym("Left".into())),
                        joystick,
                        Key::special("→", KeyAction::Keysym("Right".into())),
                        Key::special(" ", KeyAction::Space),
                        Key::special(" ", KeyAction::Space),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: true,
            grid_columns: 5.0,
            style: LayoutStyle::Standard,
        }
    }

    /// Second symbols page — the "more" punctuation/math page,
    /// reached by tapping `?#=` from the first symbols page (and
    /// vice versa). Contains programmer-oriented punctuation that
    /// doesn't fit on the primary symbols page (=, ~, <, >, brackets,
    /// braces, backtick, backslash, pipe, caret, percent, currencies).
    ///
    /// Same 10-column geometry as `default_symbols` so the layout
    /// doesn't visually jump when toggling between the two pages.
    /// Bottom-row utility keys mirror the symbols page (ABC,
    /// 🎤, ✨, space, comma, period, return).
    /// Stenotype layout: 4 parallelogram buttons (L1 L2 | R1 R2),
    /// one row, sized to fill the keyboard area. Each button has
    /// 5 gestures (tap + N/E/S/W) mapped to a phoneme. Pairs of
    /// gestures across the two thumbs commit as briefs or phoneme
    /// pairs via the steno engine.
    ///
    /// The button's `KeyGestures` carry the per-cardinal phoneme
    /// labels (so the renderer can show them on the button face),
    /// but they're SEMANTIC labels only — the actual commit logic
    /// goes through the steno engine, not the standard cardinal-
    /// grid dispatcher.
    ///
    /// `KeyAction::Steno("L1")` etc. tells the keyboard view to
    /// route gestures on this key into the steno engine.
    pub fn default_stenotype() -> Self {
        // Each button's gesture labels match the phoneme map in
        // src/steno/mod.rs::build_phoneme_table. Keep these in
        // sync; the rendered labels are user-facing affordances,
        // the table is the actual commit lookup.
        let l1 = Key::special("t", KeyAction::Steno("L1".into()))
            .with_width(2.5)
            .with_gestures(KeyGestures {
                n: Some("n".into()),
                e: Some("s".into()),
                s: Some("l".into()),
                w: Some("r".into()),
                ..Default::default()
            });
        let l2 = Key::special("d", KeyAction::Steno("L2".into()))
            .with_width(2.5)
            .with_gestures(KeyGestures {
                n: Some("k".into()),
                e: Some("m".into()),
                s: Some("p".into()),
                w: Some("b".into()),
                ..Default::default()
            });
        let r1 = Key::special("ə", KeyAction::Steno("R1".into()))
            .with_width(2.5)
            .with_gestures(KeyGestures {
                n: Some("a".into()),
                e: Some("e".into()),
                s: Some("o".into()),
                w: Some("i".into()),
                ..Default::default()
            });
        let r2 = Key::special("u", KeyAction::Steno("R2".into()))
            .with_width(2.5)
            .with_gestures(KeyGestures {
                n: Some("n".into()),
                e: Some("t".into()),
                s: Some("d".into()),
                w: Some("ŋ".into()),
                ..Default::default()
            });
        // Utility row: gives steno a reliable escape (Settings + ?123)
        // plus the essential editing keys. Without an explicit
        // Settings key here, rebuild_view would auto-inject a ⚙ into
        // the 4-button steno row and shrink one of the parallelogram
        // buttons to make room — mangling the \\// layout. Defining
        // the row here keeps the steno buttons pristine.
        //
        // ?123 (SymbolsToggle) opens the transient symbols page just
        // like in qwerty/cardinal — tapping ABC there returns to
        // stenotype (the user's primary). Permanent exit from
        // stenotype is via Settings → Keyboard mode, not through this
        // button. The four steno buttons sum to 10 units (row 0); this
        // utility row also sums to 10 so the grid stays aligned.
        //
        // Label is "?123" not "ABC": stenotype IS letters (it's the
        // user's primary), so labeling the symbols-entry button "ABC"
        // would be misleading — "ABC" means "return to letters" and
        // is reserved for that semantic on the transient pages. This
        // matches the qwerty / cardinal convention.
        let util_row = Row {
            keys: vec![
                Key::modifier("⚙", KeyAction::Settings).with_width(1.5),
                Key::modifier("?123", KeyAction::SymbolsToggle).with_width(2.0),
                Key::special("space", KeyAction::Space).with_width(3.0),
                Key::modifier("⌫", KeyAction::Backspace).with_width(2.0),
                Key::modifier("⏎", KeyAction::Return).with_width(1.5),
            ],
            weight: 1.0,
            offset: 0.0,
        };
        Self {
            name: "stenotype".into(),
            display_name: "Stenotype".into(),
            language: "*".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row {
                    keys: vec![l1, l2, r1, r2],
                    weight: 1.0,
                    offset: 0.0,
                },
                util_row,
            ],
            symbols_layout: None,
            ortholinear: true,
            grid_columns: 10.0,
            style: LayoutStyle::Stenotype,
        }
    }

    pub fn default_numbers() -> Self {
        Self {
            name: "numbers".into(),
            display_name: "More Symbols".into(),
            language: "*".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                // Row 0: math operators and common shell punctuation.
                Row {
                    keys: vec![
                        Key::character("~"),
                        Key::character("`"),
                        Key::character("|"),
                        Key::character("•"),
                        Key::character("√"),
                        Key::character("π"),
                        Key::character("÷"),
                        Key::character("×"),
                        Key::character("§"),
                        Key::character("∆"),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 1: currencies, caret, equality, braces.
                Row {
                    keys: vec![
                        Key::character("£"),
                        Key::character("¢"),
                        Key::character("€"),
                        Key::character("¥"),
                        Key::character("^"),
                        Key::character("°"),
                        Key::character("="),
                        Key::character("{"),
                        Key::character("}"),
                        Key::character("\\"),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 2: page-back toggle on left, comparison ops,
                // brackets, backspace on right (mirrors symbols
                // page's row 2 with the same flanking width pair).
                //
                // `?#=` is the conventional "more symbols" page
                // toggle marker on phone keyboards (iOS uses it,
                // Android Gboard uses similar). Tapping it goes
                // back to the primary symbols page via Numbers
                // → Symbols state transition.
                Row {
                    keys: vec![
                        Key::modifier("?#=", KeyAction::Numbers).with_width(1.5),
                        Key::character("%"),
                        Key::character("©"),
                        Key::character("®"),
                        Key::character("<"),
                        Key::character(">"),
                        Key::character("["),
                        Key::character("]"),
                        Key::special("⌫", KeyAction::Backspace).with_width(1.5),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 3: utility row identical to the symbols page
                // so muscle memory carries over.
                Row {
                    keys: vec![
                        Key::modifier("ABC", KeyAction::SymbolsToggle).with_width(1.5),
                        Key::special("🎤", KeyAction::Voice).with_width(1.0),
                        Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                        Key::special(" ", KeyAction::Space).with_width(4.0),
                        Key::character(",").with_width(0.5),
                        Key::character(".").with_width(0.5),
                        Key::special("⏎", KeyAction::Return).with_width(1.5),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: false,
            grid_columns: 10.0,
            style: LayoutStyle::Standard,
        }
    }

    /// Ortholinear variant of the second symbols page (the `?#=`
    /// numbers/math page). Same content as `default_numbers` but
    /// Row 3's utility row is split into 10 uniform cells so the
    /// page renders without staggering on ortho letter layouts.
    /// See `default_symbols_ortho` for the same treatment of the
    /// first symbols page.
    pub fn default_numbers_ortho() -> Self {
        let mut base = Self::default_numbers();
        base.display_name = "More Symbols (ortho)".into();
        base.ortholinear = true;
        // Replace the staggered Row 3 with a uniform one.
        if let Some(row3) = base.rows.last_mut() {
            row3.keys = vec![
                Key::modifier("ABC", KeyAction::SymbolsToggle).with_width(1.0),
                Key::special("🎤", KeyAction::Voice),
                Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                Key::special(" ", KeyAction::Space),
                Key::special(" ", KeyAction::Space),
                Key::special(" ", KeyAction::Space),
                Key::special(" ", KeyAction::Space),
                Key::character(","),
                Key::character("."),
                Key::special("⏎", KeyAction::Return),
            ];
        }
        base
    }

    /// Cardinal-grid layout — MessagEase-inspired 3×3 of dense keys.
    ///
    /// Each key supports:
    ///   * **Tap** → primary letter
    ///   * **Cardinal swipe** (N/E/S/W/NE/NW/SE/SW) → adjacent letter
    ///   * **Hairpin** (out-and-back in any direction) → capitalize
    ///   * **Rotate CW/CCW** → digit / punctuation
    ///
    /// Letter placement is roughly frequency-optimized for English:
    /// the most-used letters live on the tap positions of each key,
    /// less-frequent ones fan out as cardinal swipes. All gestures
    /// (tap, N/E/S/W cardinal, hairpin, CW/CCW rotation) dispatch
    /// through `gestures::cardinal::dispatch` to produce the right
    /// character.
    ///
    /// Layout (taps shown):
    /// ```text
    ///   ┌───┬───┬───┐
    ///   │ a │ n │ i │
    ///   ├───┼───┼───┤
    ///   │ t │ ⎵ │ s │
    ///   ├───┼───┼───┤
    ///   │ c │ h │ r │
    ///   └───┴───┴───┘
    /// ```
    /// The 9 tap letters cover ~64% of English text by frequency
    /// (a, t, h, s, r, i, n, c — plus space). Remaining letters are
    /// distributed across cardinal swipes.
    pub fn default_cardinal_grid() -> Self {
        // Each tap key plus its 4 cardinal letters. Frequency-based
        // grouping: vowels cluster around `a`, common consonants
        // distribute through the grid.
        let a = Key::character("a")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("u".into()),
                e: Some("e".into()),
                s: Some("i".into()),
                w: Some("o".into()),
                cw: Some("1".into()),
                ccw: Some(",".into()),
                ..Default::default()
            });
        let n = Key::character("n")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("y".into()),
                e: Some("d".into()),
                w: Some("b".into()),
                cw: Some("2".into()),
                ccw: Some("N".into()),
                ..Default::default()
            });
        let i = Key::character("i")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("k".into()),
                s: Some("p".into()),
                w: Some("v".into()),
                cw: Some("3".into()),
                ccw: Some("I".into()),
                ..Default::default()
            });
        let t = Key::character("t")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("g".into()),
                e: Some("f".into()),
                s: Some("z".into()),
                cw: Some("4".into()),
                ccw: Some("T".into()),
                ..Default::default()
            });
        let space = Key::special(" ", KeyAction::Space)
            .with_width(1.0);
        let s = Key::character("s")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("j".into()),
                s: Some("x".into()),
                w: Some("l".into()),
                cw: Some("6".into()),
                ccw: Some("S".into()),
                ..Default::default()
            });
        let c = Key::character("c")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("w".into()),
                e: Some("m".into()),
                cw: Some("7".into()),
                ccw: Some("C".into()),
                ..Default::default()
            });
        let h = Key::character("h")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                e: Some("'".into()),
                w: Some(".".into()),
                n: Some("q".into()),
                cw: Some("8".into()),
                ccw: Some("H".into()),
                ..Default::default()
            });
        let r = Key::character("r")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("?".into()),
                w: Some("!".into()),
                cw: Some("9".into()),
                ccw: Some("R".into()),
                ..Default::default()
            });

        Self {
            name: "cardinal_grid_en".into(),
            display_name: "English (Cardinal Grid)".into(),
            language: "en".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                Row { keys: vec![a, n, i],            weight: 1.0, offset: 0.0 },
                Row { keys: vec![t, space, s],        weight: 1.0, offset: 0.0 },
                Row { keys: vec![c, h, r],            weight: 1.0, offset: 0.0 },
                // Condensed utility row 1 — was three separate rows.
                // Each column-of-3 now packs two half-width keys
                // stacked side-by-side (using width=0.5). Total per
                // column still = 1.0 unit so column alignment stays
                // intact, but vertical space halves.
                //
                // Left column: ?123 | ↶ (symbols toggle, undo)
                // Center column: ✏ NoOp spacer (keeps swipe path clean)
                // Right column: 🎤 | ✨ (voice, LLM)
                Row {
                    keys: vec![
                        Key::modifier("?123", KeyAction::SymbolsToggle).with_width(0.5),
                        Key::special("↶", KeyAction::Undo).with_width(0.5),
                        Key::special("", KeyAction::NoOp).with_width(1.0),
                        Key::special("🎤", KeyAction::Voice).with_width(0.5),
                        Key::modifier("✨", KeyAction::Llm).with_width(0.5),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Bottom row: backspace | return | settings.
                Row {
                    keys: vec![
                        Key::special("⌫", KeyAction::Backspace).with_width(1.0),
                        Key::special("⏎", KeyAction::Return).with_width(1.0),
                        Key::modifier("⚙", KeyAction::Settings).with_width(1.0),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: true,
            grid_columns: 3.0,
            style: LayoutStyle::CardinalGrid,
        }
    }

    /// 5-column cardinal grid with word/template columns. The first 3
    /// columns are the same letter cells as `default_cardinal_grid`;
    /// the rightmost 2 columns are 6 cells (2 per letter row) holding
    /// "clusters" — multi-character emissions that vary by the
    /// process-wide `code_mode`. Tap commits the cluster's primary;
    /// N/E/S/W swipes commit variants.
    ///
    /// CodeMode::Prose: prose word clusters (articles, pronoun "I",
    ///   to-be verbs, connectors, prepositions, common verbs). Each
    ///   emission ends with a trailing space so chaining doesn't
    ///   require manual space taps.
    ///
    /// CodeMode::{C, Rust, Python}: code-template clusters specific
    ///   to the language — includes/types/control flow/I/O/etc.
    ///   Idiom-appropriate whitespace per emission (trailing space
    ///   where the next thing is an identifier, open-paren where
    ///   it's an argument, newline where it's a statement boundary).
    ///
    /// The bottom row's CodeLangCycle button shows the current mode's
    /// short label and cycles Prose → C → Rust → Python → Prose on
    /// tap. The cycle handler is in the UI event listener (see
    /// `keyboard_view`'s KeyAction match).
    pub fn default_cardinal_grid_words() -> Self {
        use crate::keyboard::code_mode::{current, CodeMode};

        // Letter cells — same set as default_cardinal_grid so users
        // who already know that layout's a/n/i/t/s/c/h/r positions
        // don't have to relearn. Only the rightmost columns differ.
        let a = Key::character("a")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("u".into()), e: Some("e".into()),
                s: Some("i".into()), w: Some("o".into()),
                cw: Some("1".into()), ccw: Some(",".into()),
                ..Default::default()
            });
        let n = Key::character("n")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("y".into()), e: Some("d".into()),
                w: Some("b".into()),
                cw: Some("2".into()), ccw: Some("N".into()),
                ..Default::default()
            });
        let i = Key::character("i")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("k".into()), s: Some("p".into()),
                w: Some("v".into()),
                cw: Some("3".into()), ccw: Some("I".into()),
                ..Default::default()
            });
        let t = Key::character("t")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("g".into()), e: Some("f".into()),
                s: Some("z".into()),
                cw: Some("4".into()), ccw: Some("T".into()),
                ..Default::default()
            });
        let space = Key::special(" ", KeyAction::Space).with_width(1.0);
        let s = Key::character("s")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("j".into()), s: Some("x".into()),
                w: Some("l".into()),
                cw: Some("6".into()), ccw: Some("S".into()),
                ..Default::default()
            });
        let c = Key::character("c")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("w".into()), e: Some("m".into()),
                cw: Some("7".into()), ccw: Some("C".into()),
                ..Default::default()
            });
        let h = Key::character("h")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                e: Some("'".into()), w: Some(".".into()),
                n: Some("q".into()),
                cw: Some("8".into()), ccw: Some("H".into()),
                ..Default::default()
            });
        let r = Key::character("r")
            .with_width(1.0)
            .with_gestures(KeyGestures {
                n: Some("?".into()), w: Some("!".into()),
                cw: Some("9".into()), ccw: Some("R".into()),
                ..Default::default()
            });

        // 6 word/template cells from the current code_mode. Order:
        // (row1-col4, row1-col5, row2-col4, row2-col5, row3-col4, row3-col5).
        let mode = current();
        let (w1, w2, w3, w4, w5, w6) = match mode {
            CodeMode::Prose => prose_clusters(),
            CodeMode::C => c_clusters(),
            CodeMode::Rust => rust_clusters(),
            CodeMode::Python => python_clusters(),
        };

        Self {
            name: "cardinal_grid_words".into(),
            display_name: format!("Cardinal Grid + Words ({})", mode.short_label()),
            language: "en".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                // Letter row 1: a, n, i + 2 word cells
                Row { keys: vec![a, n, i, w1, w2], weight: 1.0, offset: 0.0 },
                // Letter row 2: t, space, s + 2 word cells
                Row { keys: vec![t, space, s, w3, w4], weight: 1.0, offset: 0.0 },
                // Letter row 3: c, h, r + 2 word cells
                Row { keys: vec![c, h, r, w5, w6], weight: 1.0, offset: 0.0 },
                // Utility row: ?123, undo, NoOp spacer, voice, llm.
                // Same layout-purpose keys as default_cardinal_grid's
                // utility row but expanded from 5 half-units (total
                // 3.0) to 5 full units (total 5.0) for the wider grid.
                Row {
                    keys: vec![
                        Key::modifier("?123", KeyAction::SymbolsToggle).with_width(1.0),
                        Key::special("↶", KeyAction::Undo).with_width(1.0),
                        Key::special("", KeyAction::NoOp).with_width(1.0),
                        Key::special("🎤", KeyAction::Voice).with_width(1.0),
                        Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Action row: backspace, return, settings, code-lang
                // cycle, punctuation cardinal cell. The lang button's
                // label reflects the current mode and changes on cycle.
                Row {
                    keys: vec![
                        Key::special("⌫", KeyAction::Backspace).with_width(1.0),
                        Key::special("⏎", KeyAction::Return).with_width(1.0),
                        Key::modifier("⚙", KeyAction::Settings).with_width(1.0),
                        Key::modifier(mode.short_label(), KeyAction::CodeLangCycle)
                            .with_width(1.0),
                        // Punctuation cardinal cell — tap=".", swipes
                        // give the common terminal/connector punctuation.
                        // Width 1.0 to keep row sum at 5.0.
                        Key::character(".")
                            .with_width(1.0)
                            .with_gestures(KeyGestures {
                                n: Some("?".into()),
                                e: Some("!".into()),
                                s: Some(",".into()),
                                w: Some(";".into()),
                                cw: Some(":".into()),
                                ccw: Some(" - ".into()),
                                ..Default::default()
                            }),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: None,
            ortholinear: true,
            grid_columns: 5.0,
            style: LayoutStyle::CardinalGrid,
        }
    }

    /// Dvorak in a strict ortholinear grid: every row exactly 10 columns
    /// wide, no row stagger, every letter key the same size.
    ///
    /// Standard Dvorak has 26 letters + `' , . / ; -`; we fit them on three
    /// 10-column rows using `'`, `,`, `.` in the top row (already part of
    /// Dvorak's home positions there) and tucking `q j k x b m w v z` plus
    /// space-row punctuation across the lower rows.
    pub fn default_dvorak_ortholinear() -> Self {
        Self {
            name: "en_us_dvorak_ortho".into(),
            display_name: "English (Dvorak — Ortholinear)".into(),
            language: "en".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                // Row 1: standard Dvorak top row, 10 keys.
                Row {
                    keys: vec![
                        char_with_long_press("'", &["\""]),
                        char_with_long_press(",", &["<"]),
                        char_with_long_press(".", &[">"]),
                        Key::character("p"),
                        Key::character("y"),
                        Key::character("f"),
                        Key::character("g"),
                        Key::character("c"),
                        Key::character("r"),
                        Key::character("l"),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 2: standard Dvorak home row, 10 keys.
                Row {
                    keys: vec![
                        char_with_long_press("a", &["à", "á", "â", "ä"]),
                        char_with_long_press("o", &["ó", "ò", "ô", "ö"]),
                        char_with_long_press("e", &["é", "è", "ê", "ë"]),
                        char_with_long_press("u", &["ü", "ú", "ù"]),
                        char_with_long_press("i", &["í", "ï", "î"]),
                        Key::character("d"),
                        Key::character("h"),
                        Key::character("t"),
                        Key::character("n"),
                        char_with_long_press("s", &["ß"]),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 3: shift(1) + 8 letters + backspace(1) = 10 grid units.
                Row {
                    keys: vec![
                        Key::modifier("⇧", KeyAction::Shift).with_width(1.0),
                        Key::character("q"),
                        Key::character("j"),
                        Key::character("k"),
                        Key::character("x"),
                        Key::character("b"),
                        Key::character("m"),
                        Key::character("w"),
                        Key::character("v"),
                        Key::special("⌫", KeyAction::Backspace).with_width(1.0),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 4: ?123 + voice + llm + ⌫-buf + space(2.34) +
                //         ⏎-buf + z + / + return(2) = 10 units.
                //
                // The two thin (0.33-wide) "arcade buffer" keys
                // flanking the spacebar sit at thumb-natural positions.
                // Left thumb falls on ⌫, right thumb on ⏎ — the two
                // highest-frequency editing actions. The existing
                // Backspace (row 3 right end) and Return (row 4 far
                // right) stay put so muscle memory still works; the
                // buffers are PURELY additive reach.
                //
                // Spacebar shrunk from 3.0 → 2.34 to make room for
                // the buffers without busting the grid_columns=10
                // invariant. Still the widest key on the layout —
                // the existing ⏎ at the right is 2.0, so spacebar
                // at 2.34 stays meaningfully big.
                //
                // The `arcade-buffer` CSS class on the buffer keys
                // gives them a distinctive narrow / accented look
                // so they're findable despite the small hit area.
                Row {
                    keys: vec![
                        Key::modifier("?123", KeyAction::SymbolsToggle).with_width(1.0),
                        Key::special("🎤", KeyAction::Voice).with_width(1.0),
                        Key::modifier("✨", KeyAction::Llm).with_width(1.0),
                        Key::special("⌫", KeyAction::Backspace)
                            .with_width(0.33)
                            .with_css_class("arcade-buffer"),
                        Key::special(" ", KeyAction::Space).with_width(2.34),
                        Key::special("⏎", KeyAction::Return)
                            .with_width(0.33)
                            .with_css_class("arcade-buffer"),
                        Key::character("z"),
                        char_with_long_press("/", &["?", "\\"]),
                        Key::special("⏎", KeyAction::Return).with_width(2.0),
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: Some("symbols".into()),
            ortholinear: true,
            grid_columns: 10.0,
            style: LayoutStyle::Standard,
        }
    }

    /// Convert a layout's geometry between staggered and ortholinear.
    /// "Staggered" means the rows are offset by `[0.0, 0.25, 0.5, 0.0]`
    /// (matching physical-QWERTY key positions). "Ortholinear" means
    /// every row's `offset` is 0 — keys form a strict grid.
    ///
    /// This is a non-destructive helper: it returns a new Layout with
    /// the geometry flipped, leaving the key contents unchanged. Used
    /// to derive `qwerty_ortho` and `dvorak_staggered` from the canonical
    /// staggered-QWERTY and ortho-Dvorak definitions, so we only need
    /// one source of truth per letter system.
    pub fn with_geometry(mut self, ortholinear: bool) -> Self {
        self.ortholinear = ortholinear;

        // Standard staggered offsets for a 4-row keyboard. Match the
        // physical-QWERTY stagger: row 0 flush, row 1 offset by 0.25
        // (so a/s/d/f align between q-w/w-e/e-r/r-t), row 2 offset by
        // 0.5 from row 0, bottom row flush.
        let staggered_offsets = [0.0, 0.25, 0.5, 0.0];

        for (i, row) in self.rows.iter_mut().enumerate() {
            if ortholinear {
                row.offset = 0.0;
            } else {
                row.offset = staggered_offsets.get(i).copied().unwrap_or(0.0);
            }
        }

        // Update the display-name suffix so debug logs make sense.
        // (Kept simple — Settings UI doesn't display this name.)
        let suffix = if ortholinear { "Ortho" } else { "Staggered" };
        if !self.display_name.contains(suffix) {
            // Strip any previous geometry suffix.
            let base = self
                .display_name
                .split(" — ")
                .next()
                .unwrap_or(&self.display_name)
                .to_string();
            self.display_name = format!("{} — {}", base, suffix);
        }
        self
    }

    /// QWERTY in ortholinear geometry — derived from `default_qwerty`
    /// with row offsets zeroed.
    pub fn default_qwerty_ortho() -> Self {
        Self::default_qwerty().with_geometry(true)
    }

    /// Dvorak in staggered geometry — derived from
    /// `default_dvorak_ortholinear` with stagger offsets restored.
    pub fn default_dvorak_staggered() -> Self {
        Self::default_dvorak_ortholinear().with_geometry(false)
    }

    /// Dvorak-flavored cardinal grid — "Candidate B" layout.
    ///
    /// Designed by iterating on a Dvorak typist's actual ergonomic
    /// feedback. Three principles drove the placement:
    ///
    ///   1. **Tap → swipe bigrams within a single cell.** The top-mid
    ///      cell holds `e t l r h`, so `the`, `er`, `he`, `re`, `th`
    ///      all become consecutive in-cell motions (tap-then-swipe).
    ///      `e` at center, `t` north, `h` south, `r` east, `l` west.
    ///
    ///   2. **Left-right balance for two-thumb typing.** Vowels live
    ///      mostly on the left grid column, consonants on the right.
    ///      Common bigrams that cross vowel-consonant boundaries
    ///      naturally alternate sides, giving a rocking motion.
    ///
    ///   3. **Counter-swipe satisfaction at word boundaries.** Words
    ///      like `tortuga` produce a sequence of rightward swipes that
    ///      ends with a leftward motion to the final vowel — like
    ///      twitching the opposite muscle group after exercising one.
    ///
    /// Placements:
    ///
    /// ```text
    ///   ┌─────────────┬────────────┬─────────────┐
    ///   │      '      │      t     │      y      │
    ///   │   q  a  o   │   l  e  r  │   x  i  u   │
    ///   │      ,      │      h     │      .      │
    ///   ├─────────────┼────────────┼─────────────┤
    ///   │      f      │     ⎵      │      c      │
    ///   │   z  s  d   │  +arrows   │   w  n  g   │
    ///   │      b      │            │      m      │
    ///   ├─────────────┼────────────┼─────────────┤
    ///   │      j      │            │             │
    ///   │      k  p   │   ?123     │   (blank)   │
    ///   │      ;      │            │             │
    ///   ├─────────────┼────────────┼─────────────┤
    ///   │      ⚙      │      ⏎     │      ⌫      │
    ///   └─────────────┴────────────┴─────────────┘
    /// ```
    ///
    /// All 26 letters plus four common punctuation marks (`' , . ;`)
    /// fit using only cardinal swipes — no diagonals required. The
    /// utility row sits below; settings on the non-dominant side,
    /// backspace on the dominant side for one-handed reach.
    pub fn default_cardinal_grid_dvorak() -> Self {
        // Default to right-handed utility-row layout (settings on
        // left, backspace on right). Use
        // `default_cardinal_grid_dvorak_for_anchor` to flip for a
        // left-handed user.
        Self::default_cardinal_grid_dvorak_for_anchor(
            crate::config::ThumbAnchor::BottomRight,
        )
    }

    /// Cardinal Dvorak with utility-row placement chosen based on
    /// dominant-thumb anchor. The dominant side gets backspace (the
    /// frequently-used key); the non-dominant side gets settings (less
    /// frequent). Return stays in the middle.
    ///
    /// For right-handed (anchor=BottomRight): ⚙ left, ⏎ middle, ⌫ right.
    /// For left-handed (anchor=BottomLeft):  ⌫ left, ⏎ middle, ⚙ right.
    pub fn default_cardinal_grid_dvorak_for_anchor(
        anchor: crate::config::ThumbAnchor,
    ) -> Self {
        // ---- Row 0: vowels on outer cells, e-r-t-h-l hot cluster in middle.
        //
        // Top-left cell (a-centered):
        //   N: ' (apostrophe — common after consonants for contractions)
        //   E: o (the vowel-vowel `ao` bigram, also common as `o`-leading)
        //   S: , (comma — common after words)
        //   W: q (rare; gets the outer/edge slot)
        //   CW: 1 (digit; CW-rotation per cell maps to a digit in
        //          row-major order across the 3x3 grid:
        //          a→1, e→2, i→3, s→4, ' '→[space stays as space],
        //          n→6, k→7, [bot-mid utility row, no digit],
        //          🎤→[voice cell, no digit])
        //   CCW: A (capitalize the cell's center letter — alternative
        //          to the hairpin gesture)
        let cell_a = Key::character("a").with_width(1.0).with_gestures(KeyGestures {
            n: Some("'".into()),
            e: Some("o".into()),
            s: Some(",".into()),
            w: Some("q".into()),
            cw: Some("1".into()),
            ccw: Some("A".into()),
            ..Default::default()
        });

        // Top-middle cell (e-centered) — the bigram hot zone. Every direction
        // here paired with the center produces a top-20 English bigram:
        //   N=t, E=r, S=h, W=l
        //   t↔e: `te`/`et` (common)
        //   e↔r: `er`/`re` (common)
        //   e↔h: `he`/`eh` (very common as part of `the`)
        //   l↔e: `le`/`el` (common)
        //   Also intra-cell: t↔h is `th`/`ht`, h↔r is part of `chr`/...,
        //   and the whole cell handles `the` as N→S→C taps.
        let cell_e = Key::character("e").with_width(1.0).with_gestures(KeyGestures {
            n: Some("t".into()),
            e: Some("r".into()),
            s: Some("h".into()),
            w: Some("l".into()),
            cw: Some("2".into()),
            ccw: Some("E".into()),
            ..Default::default()
        });

        // Top-right cell (i-centered):
        //   N: y (rare-but-present consonant, top-row-ish in Dvorak)
        //   E: u (vowel-vowel bigram `iu`, plus `u` is mid-frequency)
        //   S: . (period — common after words; symmetry with `,` on the left)
        //   W: x (rare; placed here to balance `q` on the opposite corner)
        let cell_i = Key::character("i").with_width(1.0).with_gestures(KeyGestures {
            n: Some("y".into()),
            e: Some("u".into()),
            s: Some(".".into()),
            w: Some("x".into()),
            cw: Some("3".into()),
            ccw: Some("I".into()),
            ..Default::default()
        });

        // ---- Row 1: mid-frequency consonants + space anchor.
        //
        // Mid-left cell (s-centered):
        //   N: f, E: d, S: b, W: z
        //   s↔d: `sd` rare but `ds` slightly less rare; mostly placed by
        //   row-position correspondence to Dvorak's home row (d is to the
        //   right of s on real Dvorak, so it gets E here).
        //   f↔s: `fs`/`sf` rare; f gets N because it's the top-row
        //   neighbor on real Dvorak (`f` lives directly above `u`/`d`).
        let cell_s = Key::character("s").with_width(1.0).with_gestures(KeyGestures {
            n: Some("f".into()),
            e: Some("d".into()),
            s: Some("b".into()),
            w: Some("z".into()),
            cw: Some("4".into()),
            ccw: Some("S".into()),
            ..Default::default()
        });

        // Middle cell is the springboard. No swipes here — those are
        // claimed by the global spacebar-arrow handler that runs higher
        // up in the gesture-dispatch chain.
        let space = Key::special(" ", KeyAction::Space).with_width(1.0);

        // Mid-right cell (n-centered):
        //   N: c, E: g, S: m, W: w
        //   n↔g: `ng` is a top-30 English bigram (-ing suffix).
        //   n↔m: `nm`/`mn` rare but m fits here positionally (m is
        //   bottom-row-right in Dvorak, below n's column).
        //   c is top-row-right in Dvorak, hence N.
        let cell_n = Key::character("n").with_width(1.0).with_gestures(KeyGestures {
            n: Some("c".into()),
            e: Some("g".into()),
            s: Some("m".into()),
            w: Some("w".into()),
            cw: Some("6".into()),
            ccw: Some("N".into()),
            ..Default::default()
        });

        // ---- Row 2: utility row (bot-left holds rare letters; bot-mid is
        // ⌫/⏎/⚙; bot-right is empty for future expansion or a layout key).
        //
        // Bot-left cell (k-centered) — rare-letter pocket:
        //   N: j, C: k, E: p, S: ;, W: v
        //   `k` is the most-used of `j k p`, gets the center.
        //   `;` (semicolon) goes south because it's a "below normal text"
        //   character in writing flow.
        //   `v` lives on the W slot per user request (left side of the
        //   grid, in the rare-letter pocket — fits the "v is uncommon
        //   in dvorak typing" semantics).
        let cell_k = Key::character("k").with_width(1.0).with_gestures(KeyGestures {
            n: Some("j".into()),
            e: Some("p".into()),
            s: Some(";".into()),
            w: Some("v".into()),
            cw: Some("7".into()),
            ccw: Some("K".into()),
            ..Default::default()
        });

        // Bot-right cell — Voice key. Cardinal grid is 3×3 dense
        // letter cells plus a utility row; without putting Voice
        // here, cardinal users would have no way to invoke voice
        // input. This slot was previously a no-op placeholder
        // reserved for future expansion.
        let cell_voice = Key::special("🎤", KeyAction::Voice).with_width(1.0);

        Self {
            name: "cardinal_grid_dvorak".into(),
            display_name: "Dvorak (Cardinal Grid)".into(),
            language: "en".into(),
            direction: TextDirection::Ltr,
            rows: vec![
                // Row 0: top vowels + e-hot-cell.
                Row {
                    keys: vec![cell_a, cell_e, cell_i],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 1: mid consonants + space anchor.
                Row {
                    keys: vec![cell_s, space, cell_n],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Row 2: bot-left rare-letter pocket + bot-mid symbol-page
                // toggle + bot-right reserved for future expansion.
                //
                // The bot-mid cell triggers the symbol page (numbers
                // and punctuation). Without it, cardinal grid users
                // would have no way to type digits since the cardinal
                // hot cells are saturated with letters.
                Row {
                    keys: vec![
                        cell_k,
                        Key::modifier("?123", KeyAction::SymbolsToggle).with_width(1.0),
                        cell_voice,
                    ],
                    weight: 1.0,
                    offset: 0.0,
                },
                // Condensed utility row. Was two rows (undo/llm,
                // then backspace/return/settings); merged into one
                // by using half-width pairs:
                //   left column = undo | backspace
                //   center column = return
                //   right column = llm | settings
                // Dominant-side flip preserved.
                Row {
                    keys: {
                        let undo = Key::special("↶", KeyAction::Undo).with_width(0.5);
                        let backspace = Key::special("⌫", KeyAction::Backspace).with_width(0.5);
                        let return_key = Key::special("⏎", KeyAction::Return).with_width(1.0);
                        let llm = Key::modifier("✨", KeyAction::Llm).with_width(0.5);
                        let settings = Key::modifier("⚙", KeyAction::Settings).with_width(0.5);
                        match anchor {
                            crate::config::ThumbAnchor::BottomRight => {
                                // Dominant right: backspace on the
                                // right (closest to thumb).
                                vec![settings, llm, return_key, backspace, undo]
                            }
                            crate::config::ThumbAnchor::BottomLeft => {
                                vec![undo, backspace, return_key, llm, settings]
                            }
                        }
                    },
                    weight: 1.0,
                    offset: 0.0,
                },
            ],
            symbols_layout: Some("symbols".into()),
            ortholinear: true,
            grid_columns: 3.0,
            style: LayoutStyle::CardinalGrid,
        }
    }
}

fn char_with_long_press(label: &str, extended: &[&str]) -> Key {
    Key::character(label).with_extended(extended.iter().map(|s| s.to_string()).collect())
}

/// Build a word/template cell: the cell's label is the "head" of the
/// cluster (without trailing whitespace), and tapping emits the head
/// followed by the configured trailing whitespace (typically " " for
/// prose words, but " ", "(", or "" for code templates).
fn word_cell(
    label: &str,
    emit: &str,
    gestures: KeyGestures,
) -> Key {
    Key::special(label, KeyAction::Literal(emit.into()))
        .with_width(1.0)
        .with_gestures(gestures)
}

/// PROSE clusters — six common English structural building blocks.
/// Each emission ends with a space so the user types the next word
/// immediately (no manual space tap).
///
/// Layout assignment:
///   row1: [the/articles] [I/pronoun-I]
///   row2: [is/to-be]     [and/connectors]
///   row3: [to/prep]      [have/aux+common verbs]
fn prose_clusters() -> (Key, Key, Key, Key, Key, Key) {
    // Cell 1: Articles. "the" is the most common English word
    // (~7% of all text); making it tap-emit pays off fast.
    let articles = word_cell("the", "the ", KeyGestures {
        n: Some("a ".into()),
        e: Some("an ".into()),
        s: Some("this ".into()),
        w: Some("that ".into()),
        cw: Some("these ".into()),
        ccw: Some("those ".into()),
        ..Default::default()
    });
    // Cell 2: First-person pronoun cluster. Capitalized I because
    // English convention; user can't accidentally type lowercase "i".
    let pronoun_i = word_cell("I", "I ", KeyGestures {
        n: Some("I'm ".into()),
        e: Some("I've ".into()),
        s: Some("I'll ".into()),
        w: Some("I'd ".into()),
        cw: Some("me ".into()),
        ccw: Some("my ".into()),
        ..Default::default()
    });
    // Cell 3: To-be conjugations. "is" most common form.
    let to_be = word_cell("is", "is ", KeyGestures {
        n: Some("are ".into()),
        e: Some("was ".into()),
        s: Some("were ".into()),
        w: Some("be ".into()),
        cw: Some("been ".into()),
        ccw: Some("being ".into()),
        ..Default::default()
    });
    // Cell 4: Connectors / conjunctions. Sentence flow.
    let connectors = word_cell("and", "and ", KeyGestures {
        n: Some("but ".into()),
        e: Some("or ".into()),
        s: Some("so ".into()),
        w: Some("because ".into()),
        cw: Some("if ".into()),
        ccw: Some("when ".into()),
        ..Default::default()
    });
    // Cell 5: Prepositions. Six of the most common in English.
    let prepositions = word_cell("to", "to ", KeyGestures {
        n: Some("of ".into()),
        e: Some("in ".into()),
        s: Some("for ".into()),
        w: Some("with ".into()),
        cw: Some("on ".into()),
        ccw: Some("from ".into()),
        ..Default::default()
    });
    // Cell 6: Auxiliary + common verbs. "have/has/had" + "do/did" +
    // "will" — most frequent helpers.
    let verbs = word_cell("have", "have ", KeyGestures {
        n: Some("has ".into()),
        e: Some("had ".into()),
        s: Some("do ".into()),
        w: Some("did ".into()),
        cw: Some("does ".into()),
        ccw: Some("will ".into()),
        ..Default::default()
    });
    (articles, pronoun_i, to_be, connectors, prepositions, verbs)
}

/// C TEMPLATES — six clusters covering the most common C
/// statement openings. Idiomatic whitespace: trailing space for
/// keyword-then-identifier, open-paren for function-or-control-then-
/// expression, newline-inside-block for trailing block bodies.
///
/// Layout assignment:
///   row1: [#include] [int/types]
///   row2: [if/flow]  [printf/io]
///   row3: [*/mem]    [struct/decl]
fn c_clusters() -> (Key, Key, Key, Key, Key, Key) {
    let includes = word_cell("#inc", "#include <", KeyGestures {
        n: Some("#include <stdio.h>\n".into()),
        e: Some("#include <stdlib.h>\n".into()),
        s: Some("#include <string.h>\n".into()),
        w: Some("#define ".into()),
        cw: Some("#include \"".into()),
        ccw: Some("#ifndef ".into()),
        ..Default::default()
    });
    let types = word_cell("int", "int ", KeyGestures {
        n: Some("void ".into()),
        e: Some("char ".into()),
        s: Some("float ".into()),
        w: Some("double ".into()),
        cw: Some("long ".into()),
        ccw: Some("size_t ".into()),
        ..Default::default()
    });
    let control = word_cell("if", "if (", KeyGestures {
        n: Some("else if (".into()),
        e: Some("else {\n    ".into()),
        s: Some("while (".into()),
        w: Some("for (".into()),
        cw: Some("switch (".into()),
        ccw: Some("return ".into()),
        ..Default::default()
    });
    let io = word_cell("prt", "printf(\"", KeyGestures {
        n: Some("fprintf(stderr, \"".into()),
        e: Some("\\n".into()),
        s: Some("scanf(\"".into()),
        w: Some("puts(".into()),
        cw: Some("putchar(".into()),
        ccw: Some("getchar()".into()),
        ..Default::default()
    });
    let mem = word_cell("*ptr", "*", KeyGestures {
        n: Some("&".into()),
        e: Some("malloc(".into()),
        s: Some("free(".into()),
        w: Some("sizeof(".into()),
        cw: Some("NULL".into()),
        ccw: Some("->".into()),
        ..Default::default()
    });
    // Note: the tap for "decl" emits "struct " but the cw rotation
    // emits a whole `int main` skeleton — useful for starting a new
    // file in one gesture.
    let decl = word_cell("struct", "struct ", KeyGestures {
        n: Some("typedef ".into()),
        e: Some("static ".into()),
        s: Some("const ".into()),
        w: Some("extern ".into()),
        cw: Some("int main(void) {\n    \n    return 0;\n}".into()),
        ccw: Some("/*  */".into()),
        ..Default::default()
    });
    (includes, types, control, io, mem, decl)
}

/// RUST TEMPLATES — six clusters for Rust's most common syntax
/// openings. Heavier on attributes/derives and the Result/Option
/// idioms (`?`, `.unwrap()`, `Some`, `Ok`) that come up constantly.
///
/// Layout assignment:
///   row1: [fn/decl]  [let/bind]
///   row2: [if/flow]  [?/Result]
///   row3: [->/types] [use/import]
fn rust_clusters() -> (Key, Key, Key, Key, Key, Key) {
    let fn_decl = word_cell("fn", "fn ", KeyGestures {
        n: Some("pub fn ".into()),
        e: Some("pub ".into()),
        s: Some("async fn ".into()),
        w: Some("impl ".into()),
        cw: Some("trait ".into()),
        ccw: Some("where ".into()),
        ..Default::default()
    });
    let bindings = word_cell("let", "let ", KeyGestures {
        n: Some("let mut ".into()),
        e: Some("const ".into()),
        s: Some("static ".into()),
        w: Some("self".into()),
        cw: Some("&self".into()),
        ccw: Some("&mut self".into()),
        ..Default::default()
    });
    let control = word_cell("if", "if ", KeyGestures {
        n: Some("if let ".into()),
        e: Some("else {\n    ".into()),
        s: Some("while ".into()),
        w: Some("for ".into()),
        cw: Some("match ".into()),
        ccw: Some("loop {\n    ".into()),
        ..Default::default()
    });
    let result = word_cell("?", "?", KeyGestures {
        n: Some(".unwrap()".into()),
        e: Some(".expect(\"".into()),
        s: Some("Some(".into()),
        w: Some("Ok(".into()),
        cw: Some("None".into()),
        ccw: Some("Err(".into()),
        ..Default::default()
    });
    let types = word_cell("->", "-> ", KeyGestures {
        n: Some("<T>".into()),
        e: Some("&".into()),
        s: Some("&mut ".into()),
        w: Some("Vec<".into()),
        cw: Some("Box<".into()),
        ccw: Some("Result<".into()),
        ..Default::default()
    });
    let attrs_use = word_cell("use", "use ", KeyGestures {
        n: Some("#[derive(".into()),
        e: Some("#[cfg(".into()),
        s: Some("mod ".into()),
        w: Some("pub use ".into()),
        cw: Some("extern crate ".into()),
        ccw: Some("#[test]\n".into()),
        ..Default::default()
    });
    (fn_decl, bindings, control, result, types, attrs_use)
}

/// PYTHON TEMPLATES — six clusters covering def/class, control
/// flow, self/super, return/yield, imports, and print/format. The
/// indentation in block-openers (`else:\n    `) drops the user
/// inside the block, on the indent level Python expects.
///
/// Layout assignment:
///   row1: [def/decl]   [self/method]
///   row2: [if/flow]    [return/exit]
///   row3: [import]     [print/io]
fn python_clusters() -> (Key, Key, Key, Key, Key, Key) {
    let decl = word_cell("def", "def ", KeyGestures {
        n: Some("class ".into()),
        e: Some("async def ".into()),
        s: Some("lambda ".into()),
        w: Some("@".into()),
        cw: Some("@property\n".into()),
        ccw: Some("@staticmethod\n".into()),
        ..Default::default()
    });
    let self_method = word_cell("self.", "self.", KeyGestures {
        n: Some("self".into()),
        e: Some("def __init__(self".into()),
        s: Some("super().".into()),
        w: Some("cls".into()),
        cw: Some("*args".into()),
        ccw: Some("**kwargs".into()),
        ..Default::default()
    });
    let control = word_cell("if", "if ", KeyGestures {
        n: Some("elif ".into()),
        e: Some("else:\n    ".into()),
        s: Some("while ".into()),
        w: Some("for ".into()),
        cw: Some("try:\n    ".into()),
        ccw: Some("except ".into()),
        ..Default::default()
    });
    let exit = word_cell("ret", "return ", KeyGestures {
        n: Some("yield ".into()),
        e: Some("raise ".into()),
        s: Some("pass".into()),
        w: Some("None".into()),
        cw: Some("continue".into()),
        ccw: Some("break".into()),
        ..Default::default()
    });
    let imports = word_cell("imp", "import ", KeyGestures {
        n: Some("from ".into()),
        e: Some(" as ".into()),
        s: Some("from typing import ".into()),
        w: Some("from collections import ".into()),
        cw: Some("from __future__ import ".into()),
        ccw: Some("import sys\n".into()),
        ..Default::default()
    });
    let io = word_cell("prt", "print(", KeyGestures {
        n: Some("print(f\"".into()),
        e: Some("f\"".into()),
        s: Some(".format(".into()),
        w: Some("repr(".into()),
        cw: Some("input(".into()),
        ccw: Some("len(".into()),
        ..Default::default()
    });
    (decl, self_method, control, exit, imports, io)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_qwerty_has_four_rows() {
        let l = Layout::default_qwerty();
        assert_eq!(l.rows.len(), 4);
        assert_eq!(l.name, "en_us_qwerty");
    }

    #[test]
    fn qwerty_top_row_has_q_through_p() {
        let l = Layout::default_qwerty();
        let labels: Vec<&str> = l.rows[0].keys.iter().map(|k| k.label.as_str()).collect();
        assert_eq!(labels, vec!["q","w","e","r","t","y","u","i","o","p"]);
    }

    #[test]
    fn space_is_wider_than_letters() {
        // The space bar must be wider than a letter (which is 1.0
        // unit wide), but the current qwerty bottom row crowds in
        // ?123, mic, llm, space, comma, period, settings, and
        // return — leaving space at only 2.0. This is borderline
        // narrow for thumb-typing; the symbols page sets space=4.0
        // which feels noticeably better. TODO: consider trimming
        // the qwerty bottom row (e.g., move settings to long-press
        // on ?123) so space can widen to ≥4.0.
        let l = Layout::default_qwerty();
        let bottom = &l.rows[3];
        let space = bottom
            .keys
            .iter()
            .find(|k| matches!(k.action, KeyAction::Space))
            .unwrap();
        assert!(
            space.width > 1.0,
            "space ({}) must be wider than a letter (1.0)",
            space.width
        );
    }

    #[test]
    fn shift_is_modifier_type() {
        let l = Layout::default_qwerty();
        let shift = l.rows[2].keys.iter().find(|k| matches!(k.action, KeyAction::Shift)).unwrap();
        assert_eq!(shift.key_type, crate::keyboard::KeyType::Modifier);
    }

    #[test]
    fn dvorak_ortho_is_marked_ortholinear() {
        let l = Layout::default_dvorak_ortholinear();
        assert!(l.ortholinear);
        assert_eq!(l.grid_columns, 10.0);
    }

    #[test]
    fn dvorak_ortho_every_row_fills_grid() {
        let l = Layout::default_dvorak_ortholinear();
        // Every row's total width must equal grid_columns exactly. This is
        // what makes the layout truly ortholinear — a perfect rectangle.
        for (i, row) in l.rows.iter().enumerate() {
            let total: f32 = row.keys.iter().map(|k| k.width).sum();
            assert!(
                (total - l.grid_columns).abs() < 0.001,
                "row {} sums to {} units, expected {}",
                i,
                total,
                l.grid_columns
            );
            assert_eq!(
                row.offset, 0.0,
                "row {} has offset {}, expected 0 for ortholinear",
                i, row.offset
            );
        }
    }

    #[test]
    fn dvorak_ortho_contains_all_dvorak_letters() {
        let l = Layout::default_dvorak_ortholinear();
        let mut letters: std::collections::HashSet<char> = std::collections::HashSet::new();
        for row in &l.rows {
            for k in &row.keys {
                if matches!(k.action, KeyAction::Character) {
                    if let Some(c) = k.label.chars().next() {
                        if c.is_ascii_alphabetic() {
                            letters.insert(c);
                        }
                    }
                }
            }
        }
        for c in 'a'..='z' {
            assert!(letters.contains(&c), "missing letter '{}' in Dvorak ortho", c);
        }
    }

    #[test]
    fn dvorak_ortho_top_row_matches_canonical_dvorak() {
        let l = Layout::default_dvorak_ortholinear();
        let row1: Vec<&str> = l.rows[0].keys.iter().map(|k| k.label.as_str()).collect();
        // Canonical Dvorak top row: ' , . p y f g c r l
        assert_eq!(row1, vec!["'", ",", ".", "p", "y", "f", "g", "c", "r", "l"]);
    }

    #[test]
    fn dvorak_ortho_home_row_matches_canonical_dvorak() {
        let l = Layout::default_dvorak_ortholinear();
        let row2: Vec<&str> = l.rows[1].keys.iter().map(|k| k.label.as_str()).collect();
        // Canonical Dvorak home row: a o e u i d h t n s
        assert_eq!(row2, vec!["a", "o", "e", "u", "i", "d", "h", "t", "n", "s"]);
    }

    #[test]
    fn json_roundtrip() {
        let l = Layout::default_qwerty();
        let s = serde_json::to_string(&l).unwrap();
        let l2: Layout = serde_json::from_str(&s).unwrap();
        assert_eq!(l.name, l2.name);
        assert_eq!(l.rows.len(), l2.rows.len());
    }

    #[test]
    fn numbers_layout_is_more_symbols_page() {
        // The "numbers" layout (named historically) is actually the
        // secondary symbols page — the iOS-style "?#=" page reached
        // by tapping ?#= from the primary symbols. It contains
        // math/currency/brackets, NOT digits. Digits live on
        // default_symbols (the primary `?123` page).
        let l = Layout::default_numbers();
        assert_eq!(l.name, "numbers");
        // 10 columns to match the primary symbols layout for visual
        // consistency.
        assert_eq!(l.grid_columns, 10.0);
        // Has a way back to primary symbols.
        let has_page_back = l
            .rows
            .iter()
            .flat_map(|r| r.keys.iter())
            .any(|k| matches!(k.action, KeyAction::Numbers));
        assert!(
            has_page_back,
            "numbers layout must have a way back to primary symbols (?#= toggle)"
        );
    }

    #[test]
    fn symbols_layout_contains_all_digits() {
        // Digits live on the PRIMARY symbols page (`?123`), not on
        // `default_numbers`.
        let l = Layout::default_symbols();
        let labels: std::collections::HashSet<String> = l
            .rows
            .iter()
            .flat_map(|r| r.keys.iter().map(|k| k.label.clone()))
            .collect();
        for d in '0'..='9' {
            assert!(
                labels.contains(&d.to_string()),
                "primary symbols layout missing digit '{}'",
                d
            );
        }
    }

    #[test]
    fn cardinal_grid_words_has_5_rows_5_cols_each_summing_to_5() {
        crate::keyboard::code_mode::set(crate::keyboard::code_mode::CodeMode::Prose);
        let l = Layout::default_cardinal_grid_words();
        assert_eq!(l.rows.len(), 5, "letter rows + utility + action = 5");
        assert_eq!(l.grid_columns, 5.0);
        for (i, row) in l.rows.iter().enumerate() {
            let sum: f32 = row.keys.iter().map(|k| k.width).sum();
            assert!(
                (sum - 5.0).abs() < 0.001,
                "row {} width sum is {} (must equal grid_columns=5.0)",
                i,
                sum
            );
        }
    }

    #[test]
    fn cardinal_grid_words_letter_cells_match_original_cardinal_grid() {
        // The point of sharing letter positions is that users who
        // already learned default_cardinal_grid's a/n/i/t/s/c/h/r
        // muscle memory carry it directly into the words layout.
        // If the letter cells diverge, that promise breaks.
        crate::keyboard::code_mode::set(crate::keyboard::code_mode::CodeMode::Prose);
        let words = Layout::default_cardinal_grid_words();
        let plain = Layout::default_cardinal_grid();

        // Letter rows in plain are rows[0..3], 3 keys each. In words
        // they're rows[0..3], first 3 keys of each (cols 0-2).
        for letter_row in 0..3 {
            for col in 0..3 {
                assert_eq!(
                    words.rows[letter_row].keys[col].label,
                    plain.rows[letter_row].keys[col].label,
                    "row {} col {} label diverges from cardinal_grid",
                    letter_row,
                    col
                );
            }
        }
    }

    #[test]
    fn cardinal_grid_words_has_lang_cycle_button_in_bottom_row() {
        crate::keyboard::code_mode::set(crate::keyboard::code_mode::CodeMode::Prose);
        let l = Layout::default_cardinal_grid_words();
        let bottom = &l.rows[4];
        let lang = bottom
            .keys
            .iter()
            .find(|k| matches!(k.action, KeyAction::CodeLangCycle));
        assert!(
            lang.is_some(),
            "bottom row must contain the CodeLangCycle button"
        );
        assert_eq!(lang.unwrap().label, "EN", "label reflects current mode (Prose → EN)");
    }

    #[test]
    fn cardinal_grid_words_cycle_button_label_follows_mode() {
        use crate::keyboard::code_mode::{set, CodeMode};
        for (mode, expected_label) in [
            (CodeMode::Prose, "EN"),
            (CodeMode::C, "C"),
            (CodeMode::Rust, "Rs"),
            (CodeMode::Python, "Py"),
        ] {
            set(mode);
            let l = Layout::default_cardinal_grid_words();
            let lang_label = &l.rows[4]
                .keys
                .iter()
                .find(|k| matches!(k.action, KeyAction::CodeLangCycle))
                .unwrap()
                .label;
            assert_eq!(
                lang_label, expected_label,
                "mode {:?} should label the lang button '{}'",
                mode, expected_label
            );
        }
        set(CodeMode::Prose); // reset for other tests
    }

    #[test]
    fn cardinal_grid_words_prose_mode_emits_words_with_spaces() {
        // Pick a known prose cluster cell (row 0 col 3 = articles
        // cluster: tap "the") and verify the Literal emission has a
        // trailing space so chaining works without manual space taps.
        use crate::keyboard::code_mode::{set, CodeMode};
        set(CodeMode::Prose);
        let l = Layout::default_cardinal_grid_words();
        let articles = &l.rows[0].keys[3];
        assert_eq!(articles.label, "the");
        match &articles.action {
            KeyAction::Literal(s) => assert_eq!(s, "the "),
            other => panic!("articles cell should be Literal, got {:?}", other),
        }
        // Cardinal swipe gestures are populated.
        let g = articles.gestures.as_ref().expect("articles must have gestures");
        assert_eq!(g.n.as_deref(), Some("a "));
        assert_eq!(g.e.as_deref(), Some("an "));
        assert_eq!(g.s.as_deref(), Some("this "));
        assert_eq!(g.w.as_deref(), Some("that "));
    }

    #[test]
    fn cardinal_grid_words_code_modes_replace_word_cells() {
        // Verifies that switching the code_mode actually changes the
        // emissions on the word column. Word column starts at index 3
        // (rows 0/1/2 keep columns 3 and 4 as word/template cells).
        use crate::keyboard::code_mode::{set, CodeMode};

        set(CodeMode::Prose);
        let prose = Layout::default_cardinal_grid_words();
        let prose_cell = &prose.rows[0].keys[3];

        set(CodeMode::Rust);
        let rust = Layout::default_cardinal_grid_words();
        let rust_cell = &rust.rows[0].keys[3];

        // Labels must differ — Prose row0col3 is "the", Rust is "fn".
        assert_ne!(
            prose_cell.label, rust_cell.label,
            "code_mode change must produce a different cluster head"
        );
        assert_eq!(prose_cell.label, "the");
        assert_eq!(rust_cell.label, "fn");

        set(CodeMode::Python);
        let py = Layout::default_cardinal_grid_words();
        assert_eq!(py.rows[0].keys[3].label, "def");

        set(CodeMode::C);
        let c = Layout::default_cardinal_grid_words();
        assert_eq!(c.rows[0].keys[3].label, "#inc");

        set(CodeMode::Prose); // reset for other tests
    }

    #[test]
    fn literal_emits_with_first_char_uppercased_when_shifted() {
        // The hook that makes word cells respect sentence-start auto-
        // cap. At sentence start the engine's auto_cap sets keypad to
        // Shifted; the engine's handle_key_tap reads shift state and
        // passes it to emit_char. With shifted=true, "the " becomes
        // "The ". Multi-char literals only get their FIRST grapheme
        // shifted.
        let key = Key::special("the", KeyAction::Literal("the ".into()));
        assert_eq!(key.emit_char(false).as_deref(), Some("the "));
        assert_eq!(key.emit_char(true).as_deref(), Some("The "));

        // Code literals: "fn " stays "fn " on shift=false (which is
        // how the engine calls it — auto-cap is for prose, not for
        // code start-of-line which conceptually shouldn't title-case
        // keywords anyway).
        let fn_key = Key::special("fn", KeyAction::Literal("fn ".into()));
        assert_eq!(fn_key.emit_char(false).as_deref(), Some("fn "));
        // If the user explicitly forces shift before tapping a code
        // cell, they DO get title case — this is the user's choice
        // and we don't second-guess it.
        assert_eq!(fn_key.emit_char(true).as_deref(), Some("Fn "));

        // Empty literal: don't crash on the first-char path.
        let empty = Key::special("", KeyAction::Literal("".into()));
        assert_eq!(empty.emit_char(true).as_deref(), Some(""));
    }
}

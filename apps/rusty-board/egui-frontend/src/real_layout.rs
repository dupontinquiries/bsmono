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
    // With the second layer (board_layouts::second_layer): a positional symbol
    // on every letter key's long-press. A page without letters is untouched.
    board_layouts::io::import_from_string(json).map(|l| l.with_second_layer()).map_err(|e| e.to_string())
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
    /// A caret move: a cardinal swipe on the spacebar, in any layout.
    ///
    /// `docs/single-hand-mode.md` lists it under Full state as "(existing)" — existing in
    /// GTK. The TRAY's spacebar has carried it since `tray.rs` was ported; the full board's
    /// had not, so the same gesture on the same key meant different things depending on
    /// whether the keyboard was minimized.
    Arrow(Arrow),
    /// A steno key's tap or cardinal swipe. Carries the button id
    /// ("L1"/"L2"/"R1"/"R2") and gesture code ("tap"/"N"/"E"/"S"/"W") —
    /// see `crate::decode_steno_token`'s doc comment for why the
    /// committed phoneme text can't carry this identity itself. The
    /// caller is expected to parse these into
    /// `steno::engine::{StenoButton, StenoGesture}` and feed the
    /// engine directly, NOT commit them as text.
    Steno(String, String),
    /// A key of the Morse board (`morse.rs`).
    Morse(MorseKey),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MorseKey {
    /// The keyer: tap = dot, hold = dash. Its duration is measured by the
    /// hold path (`key_hold::Kind::Morse`), not by this release.
    Key,
    /// The pattern with a dash in front.
    DashFront,
    /// The pattern with dots and dashes swapped.
    Invert,
}

/// The emit token for a Morse board key.
pub fn morse_emit(k: MorseKey) -> String {
    format!("{ACT}{}", match k {
        MorseKey::Key => "morse_key",
        MorseKey::DashFront => "morse_dash_front",
        MorseKey::Invert => "morse_invert",
    })
}

/// Reserved emit-token prefix for action keys crossing the render layer. `\u{0}` can't occur in
/// layout text, so tokens never collide with real emissions.
/// The prefix a non-text key action crosses the render layer as. Public so
/// `layout_from_json` emits the same tokens this module's `classify` decodes — two
/// projections of the same layouts must not disagree about what a mic key means.
pub(crate) const ACT: &str = "\u{0}act:";

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

/// The render-layer token for the Voice action, for layouts built in code (the cardinal
/// boards) that want a mic key without a `KeyAction`. Same bytes `action_emit` produces for
/// `KeyAction::Voice`, so `classify` reads both identically — asserted in a test.
pub fn voice_emit() -> String {
    action_emit(&KeyAction::Voice, "")
}

/// A caret direction. Its own type rather than `tray::Dir` so the render layer does not
/// depend on the tray widget, and rather than a bare string so a typo is a compile error.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Arrow {
    Left,
    Right,
    Up,
    Down,
}

impl Arrow {
    /// The X keysym name the sink emits, matching `emit_key`'s vocabulary.
    pub fn keysym(self) -> &'static str {
        match self {
            Arrow::Left => "Left",
            Arrow::Right => "Right",
            Arrow::Up => "Up",
            Arrow::Down => "Down",
        }
    }

    /// What a key shows for this arrow.
    pub fn glyph(self) -> &'static str {
        match self {
            Arrow::Left => "←",
            Arrow::Right => "→",
            Arrow::Up => "↑",
            Arrow::Down => "↓",
        }
    }

    /// The token a spacebar swipe emits.
    pub fn token(self) -> String {
        format!("{ACT}{}", match self {
            Arrow::Left => "left",
            Arrow::Right => "right",
            Arrow::Up => "up",
            Arrow::Down => "down",
        })
    }
}

/// Give a layout's spacebar its four arrow swipes.
///
/// "Cardinal swipe on the spacebar in ANY layout" is the doc's wording, so this is applied
/// to the rendered layout rather than written into each layout's data — five boards with
/// five copies of the same four gestures is how one of them ends up missing an arrow.
///
/// A spacebar that ALREADY has cardinal gestures is left alone: a layout that deliberately
/// put something else on its space swipes means it, and silently overwriting that would be
/// this function deciding it knows better than the layout file.
pub fn attach_space_arrows(layout: &mut crate::Layout) {
    for row in &mut layout.rows {
        for k in &mut row.keys {
            let is_space = classify(&k.emit) == Press::Text(" ".to_string());
            if !is_space || !k.cardinal.is_empty() {
                continue;
            }
            use uikit_egui::slide_menu::Dir8;
            k.cardinal = vec![
                (Dir8::W, Arrow::Left.token()),
                (Dir8::E, Arrow::Right.token()),
                (Dir8::N, Arrow::Up.token()),
                (Dir8::S, Arrow::Down.token()),
            ];
        }
    }
}

/// What a key SHOWS for a gesture value. Most values are their own label ("é", "1"), but an
/// action token is `\0act:<name>` -- a string for the dispatcher, not for a person. The
/// spacebar's arrow swipes are the case that reached the screen: the cardinal boards
/// printed `act:down` (with a missing-glyph box for the NUL) around their spacebar, and
/// every board slid the raw token onto the spacebar during an arrow swipe.
pub fn display_label(value: &str) -> std::borrow::Cow<'_, str> {
    use std::borrow::Cow;
    if !value.starts_with(ACT) {
        return Cow::Borrowed(value);
    }
    Cow::Borrowed(match classify(value) {
        Press::Arrow(a) => a.glyph(),
        Press::Backspace => "⌫",
        Press::Return => "⏎",
        Press::Shift => "⇧",
        Press::Text(t) if t == " " => "␣",
        Press::Text(t) if t == "\t" => "⇥",
        Press::SymbolsToggle => "?123",
        Press::Voice => "🎤",
        Press::Morse(MorseKey::Key) => "·−",
        Press::Morse(MorseKey::DashFront) => "−◂",
        Press::Morse(MorseKey::Invert) => "⇅",
        // An action this function has no glyph for is shown as its name, never as the
        // token with its NUL.
        _ => value.trim_start_matches(ACT),
    })
}

/// Turn a `keyboard_ui` emission back into a typed [`Press`].
pub fn classify(emit: &str) -> Press {
    if let Some((button, gesture)) = crate::decode_steno_token(emit) {
        return Press::Steno(button.to_string(), gesture.to_string());
    }
    match emit.strip_prefix(ACT) {
        Some("shift") => Press::Shift,
        Some("backspace") => Press::Backspace,
        Some("return") => Press::Return,
        Some("space") => Press::Text(" ".to_string()),
        Some("tab") => Press::Text("\t".to_string()),
        Some("symbols_toggle") => Press::SymbolsToggle,
        Some("voice") => Press::Voice,
        Some("left") => Press::Arrow(Arrow::Left),
        Some("right") => Press::Arrow(Arrow::Right),
        Some("up") => Press::Arrow(Arrow::Up),
        Some("down") => Press::Arrow(Arrow::Down),
        Some("morse_key") => Press::Morse(MorseKey::Key),
        Some("morse_dash_front") => Press::Morse(MorseKey::DashFront),
        Some("morse_invert") => Press::Morse(MorseKey::Invert),
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
/// Copy a layout's explicit per-direction assignments onto a key.
///
/// Directions the author left `None` stay empty rather than being
/// filled from `extended`: a half-declared key is a DELIBERATE
/// half-declaration, and topping it up would put characters on
/// directions the author chose to leave free.
fn apply_declared(key: &mut crate::Key, g: &board_layouts::key::KeyGestures) {
    use uikit_egui::slide_menu::Dir8;
    for (dir, v) in [
        (Dir8::N, &g.n), (Dir8::NE, &g.ne), (Dir8::E, &g.e), (Dir8::SE, &g.se),
        (Dir8::S, &g.s), (Dir8::SW, &g.sw), (Dir8::W, &g.w), (Dir8::NW, &g.nw),
    ] {
        if let Some(out) = v {
            key.cardinal.push((dir, out.clone()));
        }
    }
    key.rotate_cw = g.cw.clone();
    key.rotate_ccw = g.ccw.clone();
}

/// Place a key's variants on cardinal directions.
///
/// ## Why this order
///
/// `CARDINAL_ORDER` is N, S, E, W first, then the diagonals. The four
/// true cardinals are the swipes a thumb makes reliably without
/// looking; a diagonal on a phone key is a smaller target and easier to
/// misjudge, so the FIRST variants — the ones a layout author listed
/// first, and therefore the likeliest — get the easiest gestures.
///
/// ## Why a digit goes to CW rotation instead
///
/// A leading digit in `extended` is the key's numpad digit — `q` has
/// `["1"]`, `w` has `["2"]`. Those belong on the clockwise rotation,
/// which is where the GTK layout puts them and where `Key::rotate_cw`
/// already expects them. Putting `1` on a north swipe would spend the
/// best gesture on a character the rotation already reaches, and leave
/// the accents on diagonals.
///
/// ## Nothing is invented
///
/// A key with no variants gets no cardinals. More variants than
/// directions: the extras stay in `variants` for the hold popup, which
/// is what that popup is for — this adds a fast path to the common
/// ones, it does not replace the slow path to all of them.
fn assign_cardinals(key: &mut crate::Key) {
    use uikit_egui::slide_menu::Dir8;
    const CARDINAL_ORDER: [Dir8; 8] = [
        Dir8::N, Dir8::S, Dir8::E, Dir8::W,
        Dir8::NE, Dir8::NW, Dir8::SE, Dir8::SW,
    ];
    let mut rest = key.variants.iter();
    let first = key.variants.first().cloned();
    // A single-character digit leading the list is the numpad digit.
    if let Some(d) = first.filter(|v| v.len() == 1 && v.chars().all(|c| c.is_ascii_digit())) {
        key.rotate_cw = Some(d);
        rest.next();
    }
    for (dir, v) in CARDINAL_ORDER.iter().zip(rest) {
        key.cardinal.push((*dir, v.clone()));
    }
}

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
                    // THE ROLE AND THE LATCH, from the layout file and the board's state —
                    // not guessed from the label. `key_type` is a field on the shared
                    // `board_layouts::Key` and this projection dropped it, so on the REAL
                    // board (the one the phone runs) Enter was a grey cap like every other
                    // and a caps lock was invisible.
                    let mut key = crate::Key::emitting(&label, &emit, k.width)
                        .with_role(crate::KeyRole::from_key_type(k.key_type))
                        .with_latch(match (&k.action, shift) {
                            (KeyAction::Shift, Shift::Once) => crate::Latch::Once,
                            (KeyAction::Shift, Shift::Caps) => crate::Latch::Locked,
                            _ => crate::Latch::Off,
                        });
                    key.variants = if shift.active() && !k.extended_shifted.is_empty() {
                        k.extended_shifted.clone()
                    } else {
                        k.extended.clone()
                    };
                    // CARDINAL DIRECTIONS, from the variants that are
                    // already there.
                    //
                    // The 11-gesture classifier has been complete since
                    // it was written and NO layout carries cardinal
                    // data, so ten of the eleven gestures reached
                    // nothing — every swipe fell through to the tap
                    // letter. The data to fix that was already in the
                    // file as `extended`; only the placement was
                    // missing.
                    // DECLARED gestures win; the heuristic is the
                    // fallback.
                    //
                    // `Key::gestures` is a serde field on the layout
                    // schema and the loader never read it — so a layout
                    // that carefully assigned é to north would have had
                    // that silently replaced by whatever `extended`
                    // happened to list first. The author's placement is
                    // the answer; deriving one is what to do when there
                    // is no answer.
                    match &k.gestures {
                        Some(g) => apply_declared(&mut key, g),
                        None => assign_cardinals(&mut key),
                    }
                    if let KeyAction::Steno(button_id) = &k.action {
                        key.steno_button = Some(button_id.clone());
                    }
                    key
                })
                .collect();
            crate::Row { keys, weight: r.weight }
        })
        .collect();
    // Compass glyphs at rest only where an author PLACED the directions (declared gestures,
    // steno buttons); qwerty's derived accent compasses stay hidden until a key is pressed.
    let hints = file
        .rows
        .iter()
        .flat_map(|r| &r.keys)
        .any(|k| k.gestures.is_some() || matches!(k.action, KeyAction::Steno(_)));
    crate::Layout { name: file.display_name.clone(), rows, hints }
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
    /// The last projection, keyed by what it depends on. `to_render` clones every key of the
    /// active file (labels, variants, cardinal tables); the screen asked for it twice a frame
    /// (once to draw, once for the swipe overlay), so a 40-key board was ~600 string clones per
    /// frame that produced the same layout as the frame before. `Rc` so callers hold it across
    /// their own `&mut self` work without a borrow of the board.
    render_cache: Option<(usize, Shift, std::rc::Rc<crate::Layout>)>,
}

impl RealBoard {
    /// A board over the shipped layouts, starting on the first (QWERTY), plus the two
    /// experimental boards the GTK build ships from code: its cardinal grid (the reference
    /// implementation of the 11-gesture keys — `KeyGestures` with cw/ccw digits) and the
    /// stenotype. Neither was reachable from the sidebar before, so the steno engine — mounted,
    /// tested, 24 tests — had never had a key stroked into it, and the GTK cardinal layout could
    /// not be compared against the code-built `cardinal`/`cardinal8` demo boards.
    pub fn new() -> RealBoard {
        let mut files = builtin();
        files.push(board_layouts::Layout::default_cardinal_grid());
        files.push(board_layouts::Layout::default_stenotype());
        RealBoard { files, active: 0, shift: Shift::Off, before_symbols: None, render_cache: None }
    }

    /// The render layout for the current file + shift state, cached until either changes.
    /// This is what the screen calls every frame; `render()` remains for one-off projections.
    pub fn render_rc(&mut self) -> std::rc::Rc<crate::Layout> {
        if let Some((a, s, rc)) = &self.render_cache {
            if *a == self.active && *s == self.shift {
                return rc.clone();
            }
        }
        let rc = std::rc::Rc::new(self.render());
        self.render_cache = Some((self.active, self.shift, rc.clone()));
        rc
    }

    /// Indices into `files` that get a sidebar tab (everything but the symbols page, which is
    /// reached through the ?123 key). The sidebar maps tab position ↔ file through this rather
    /// than assuming `qwerty=0, dvorak=1`, which stopped being the whole list.
    pub fn tab_files(&self) -> Vec<usize> {
        self.files.iter().enumerate().filter(|(_, f)| f.name != "symbols").map(|(i, _)| i).collect()
    }

    pub fn with_files(files: Vec<LayoutFile>) -> RealBoard {
        RealBoard { files, active: 0, shift: Shift::Off, before_symbols: None, render_cache: None }
    }

    pub fn current(&self) -> &LayoutFile {
        &self.files[self.active]
    }

    /// The render layout for the current file + shift state (hand to `keyboard_ui`).
    pub fn render(&self) -> crate::Layout {
        let mut l = to_render(self.current(), self.shift);
        // Same attachment as the code-built board's. Both projections, one rule — the
        // alternative is a spacebar whose swipes depend on which of two render paths drew
        // it, which is the class of difference nobody finds until they switch boards.
        attach_space_arrows(&mut l);
        l
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
    /// Action tokens are for the dispatcher: what a key SHOWS for one is a glyph, never
    /// the token with its NUL.
    #[test]
    fn action_tokens_display_as_glyphs() {
        assert_eq!(display_label(&Arrow::Left.token()), "←");
        assert_eq!(display_label(&Arrow::Down.token()), "↓");
        assert_eq!(display_label("é"), "é", "a plain value is its own label");
        assert!(!display_label(&format!("{ACT}frobnicate")).contains('\u{0}'));
        // Every gesture value on every board in the app displays without a NUL.
        let mut n = 0;
        let mut boards: Vec<crate::Layout> = crate::Board::new().layout_names().iter().enumerate().map(|(i, _)| {
            let mut b = crate::Board::new();
            b.set_layout(i);
            b.rendered().into_owned()
        }).collect();
        boards.extend(RealBoard::new().files.iter().map(|f| {
            let mut l = to_render(f, Shift::Off);
            attach_space_arrows(&mut l);
            l
        }));
        for l in &boards {
            for k in l.rows.iter().flat_map(|r| &r.keys) {
                for (_, v) in &k.cardinal {
                    assert!(!display_label(v).contains('\u{0}'), "{:?} on {} shows a raw token", v, l.name);
                    n += 1;
                }
            }
        }
        assert!(n > 20, "the scan found {n} gesture values");
    }


    #[test]
    fn a_letter_with_accents_reaches_them_by_swipe() {
        // `e` carries ["3","é","è","ê","ë"]. Before this, all four
        // accents were reachable only by hold-and-pick, and ten of the
        // eleven gestures did nothing on every key in every layout.
        let l = super::to_render(&super::parse(super::QWERTY_JSON).expect("qwerty parses"),
                              Default::default());
        let e = l.rows.iter().flat_map(|r| &r.keys)
            .find(|k| k.emit == "e").expect("a key for e");
        assert!(!e.cardinal.is_empty(), "no cardinals on e");
        let reachable: Vec<&str> = e.cardinal.iter().map(|(_, s)| s.as_str()).collect();
        assert!(reachable.contains(&"é"), "{reachable:?}");
    }

    #[test]
    fn the_numpad_digit_goes_to_rotation_not_a_swipe() {
        // `q` has ["1"]: that is its numpad digit, and CW rotation is
        // where the GTK layout puts it. Spending a north swipe on it
        // would waste the best gesture on a character the rotation
        // already reaches.
        let l = super::to_render(&super::parse(super::QWERTY_JSON).expect("qwerty parses"),
                              Default::default());
        let q = l.rows.iter().flat_map(|r| &r.keys)
            .find(|k| k.emit == "q").expect("a key for q");
        assert_eq!(q.rotate_cw.as_deref(), Some("1"));
        assert!(q.cardinal.is_empty(), "the digit should not also be a swipe: {:?}", q.cardinal);
    }

    #[test]
    fn the_easiest_directions_are_used_first() {
        // N/S/E/W are the swipes a thumb makes reliably without looking;
        // a diagonal is a smaller target. The variants a layout author
        // listed first are the likeliest, so they get the easiest
        // gestures.
        use uikit_egui::slide_menu::Dir8;
        let mut k = crate::Key::new("x");
        k.variants = vec!["a".into(), "b".into(), "c".into()];
        super::assign_cardinals(&mut k);
        let dirs: Vec<Dir8> = k.cardinal.iter().map(|(d, _)| *d).collect();
        assert_eq!(dirs, vec![Dir8::N, Dir8::S, Dir8::E]);
    }

    #[test]
    fn a_declared_gesture_beats_the_derived_one() {
        // The bug: `Key::gestures` is a serde field the loader never
        // read, so a layout that carefully put é on north would have had
        // it silently replaced by whatever `extended` listed first.
        use uikit_egui::slide_menu::Dir8;
        let mut k = crate::Key::new("e");
        k.variants = vec!["3".into(), "é".into()];
        let g = board_layouts::key::KeyGestures {
            n: Some("ë".into()), ..Default::default()
        };
        super::apply_declared(&mut k, &g);
        assert_eq!(k.cardinal, vec![(Dir8::N, "ë".to_string())],
                   "the author's placement should win: {:?}", k.cardinal);
    }

    #[test]
    fn a_half_declared_key_is_not_topped_up() {
        // Leaving a direction empty is a decision. Filling it from
        // `extended` would put characters on directions the author
        // deliberately left free.
        let mut k = crate::Key::new("e");
        k.variants = vec!["a".into(), "b".into(), "c".into()];
        let g = board_layouts::key::KeyGestures {
            n: Some("x".into()), ..Default::default()
        };
        super::apply_declared(&mut k, &g);
        assert_eq!(k.cardinal.len(), 1, "{:?}", k.cardinal);
    }

    #[test]
    fn a_key_with_no_variants_gets_no_cardinals() {
        // Nothing is invented. A swipe that emits a character the layout
        // never mentioned is worse than a swipe that does nothing.
        let mut k = crate::Key::new("x");
        super::assign_cardinals(&mut k);
        assert!(k.cardinal.is_empty());
        assert!(k.rotate_cw.is_none());
    }

    #[test]
    fn extra_variants_stay_available_on_hold() {
        // Eight directions, and a key can have more. The hold popup is
        // the slow path to all of them; this adds a fast path to the
        // common ones rather than replacing it.
        let mut k = crate::Key::new("x");
        k.variants = (0..12).map(|i| format!("v{i}")).collect();
        super::assign_cardinals(&mut k);
        assert_eq!(k.cardinal.len(), 8, "more than eight directions used");
        assert_eq!(k.variants.len(), 12, "the hold popup lost its list");
    }
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

    #[test]
    fn stenotype_layout_marks_its_four_keys_with_steno_button() {
        let layout = board_layouts::Layout::default_stenotype();
        let rendered = to_render(&layout, Shift::Off);
        let steno_keys: Vec<&crate::Key> = rendered.rows[0].keys.iter().collect();
        assert_eq!(steno_keys.len(), 4, "L1 L2 R1 R2");
        let ids: Vec<&str> = steno_keys.iter().map(|k| k.steno_button.as_deref().unwrap()).collect();
        assert_eq!(ids, vec!["L1", "L2", "R1", "R2"]);
        // Phoneme labels still show for display (the whole point of the
        // token/label split): tap label is the base phoneme, and the
        // cardinal directions carry the OTHER phonemes as labels while
        // their VALUE is a steno token, not the phoneme text.
        let l1 = &steno_keys[0];
        assert_eq!(l1.label, "t");
        assert_eq!(l1.cardinal.len(), 4, "N/E/S/W only — no diagonals declared");
        for (_, label) in &l1.cardinal {
            assert_ne!(label, "t", "cardinal labels are the OTHER phonemes, not the tap one");
        }
    }

    #[test]
    fn stenotype_utility_row_keys_are_not_steno_keys() {
        let layout = board_layouts::Layout::default_stenotype();
        let rendered = to_render(&layout, Shift::Off);
        assert!(
            rendered.rows[1].keys.iter().all(|k| k.steno_button.is_none()),
            "settings/symbols/space/backspace/return must commit normally"
        );
    }

    #[test]
    fn steno_key_gestures_classify_as_press_steno_not_text() {
        let layout = board_layouts::Layout::default_stenotype();
        let rendered = to_render(&layout, Shift::Off);
        let l1 = &rendered.rows[0].keys[0];
        // Simulate what `paint_key` would return for each gesture: it
        // never returns `key.emit`/the cardinal label for a steno key, it
        // returns `crate::steno_token(button, gesture_code)`.
        let tap_value = crate::steno_token("L1", "tap");
        assert_eq!(classify(&tap_value), Press::Steno("L1".to_string(), "tap".to_string()));
        for (dir, _label) in &l1.cardinal {
            let code = crate::dir_code(*dir);
            let value = crate::steno_token("L1", code);
            assert_eq!(classify(&value), Press::Steno("L1".to_string(), code.to_string()));
        }
    }

    #[test]
    fn voice_emit_classifies_as_voice_like_the_real_layout_action() {
        assert_eq!(classify(&voice_emit()), Press::Voice);
        assert_eq!(voice_emit(), action_emit(&KeyAction::Voice, "🎤"));
    }

    #[test]
    fn the_cached_projection_is_shared_until_shift_or_layout_changes() {
        let mut b = RealBoard::new();
        let a = b.render_rc();
        let a2 = b.render_rc();
        assert!(std::rc::Rc::ptr_eq(&a, &a2), "same file, same shift: same allocation");
        b.shift = Shift::Once;
        let c = b.render_rc();
        assert!(!std::rc::Rc::ptr_eq(&a, &c), "shift changed: re-projected");
        assert_ne!(a.rows[0].keys[0].emit, c.rows[0].keys[0].emit, "and the projection actually differs");
        b.active = 1;
        let d = b.render_rc();
        assert!(!std::rc::Rc::ptr_eq(&c, &d));
        let plain = b.render();
        assert_eq!(d.name, plain.name, "the cache is the plain projection, not a variant of it");
        assert_eq!(d.rows.len(), plain.rows.len());
        for (x, y) in d.rows.iter().zip(&plain.rows) {
            let xs: Vec<&str> = x.keys.iter().map(|k| k.emit.as_str()).collect();
            let ys: Vec<&str> = y.keys.iter().map(|k| k.emit.as_str()).collect();
            assert_eq!(xs, ys);
        }
    }

    #[test]
    fn the_real_board_reaches_the_cardinal_grid_and_the_stenotype() {
        let b = RealBoard::new();
        let tabs = b.tab_files();
        assert_eq!(tabs.len(), 4, "qwerty, dvorak, cardinal grid, stenotype — symbols has no tab");
        assert!(tabs.iter().all(|&i| b.files[i].name != "symbols"));
        // the GTK cardinal grid renders with gestures AND rotation digits (the reference)
        let cg = b.files.iter().find(|f| f.name == "cardinal_grid_en").expect("cardinal grid present");
        let r = to_render(cg, Shift::Off);
        let a = r.rows.iter().flat_map(|r| &r.keys).find(|k| k.emit == "a").expect("the `a` cell");
        assert!(a.cardinal.len() >= 4, "{:?}", a.cardinal);
        assert_eq!(a.rotate_cw.as_deref(), Some("1"));
        // the stenotype renders with steno buttons, so a stroke reaches the engine
        let st = b.files.iter().find(|f| f.name == "stenotype").expect("stenotype present");
        let r = to_render(st, Shift::Off);
        assert!(r.rows.iter().flat_map(|r| &r.keys).any(|k| k.steno_button.is_some()));
    }
}

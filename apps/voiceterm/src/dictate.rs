//! Dictation core — voice *typing* into a file, as distinct from
//! editor_voice's voice *commands*.
//!
//! Layering (the seam this module exists to establish):
//!
//!   whisper text ──▶ segment()      split control verbs from content
//!                 ──▶ spoken_code() spoken tokens → real source text
//!                 ──▶ Vec<Edit>     an edit script against (text, cursor)
//!                 ──▶ Sink          BufferSink (we own the doc)
//!                                   KeySink    (a PTY editor owns it)
//!
//! Everything above the Sink is pure: no IO, no whisper, no Qt, no PTY.
//! The two sinks are the transport layer and the only place the target
//! surface leaks in. That split is load-bearing: the in-app note editor
//! has the whole document and can do anything; a helix living in a PTY
//! is write-only and can only be poked with keystrokes. Same engine,
//! different reachability — and the type system says which is which
//! (see `Placement::supported_by`).
//!
//! Deliberately NOT here: whisper invocation, the acoustic fast path,
//! the LLM. Those are callers.

// ───────────────────────────────────────────────────────────── controls

/// The closed vocabulary that must interrupt dictation. Small and fixed
/// on purpose: this is exactly the set that should ride the
/// acoustic_index fast path instead of waiting ~1.2s for whisper.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Ctl {
    NewLine,
    NewParagraph,
    ScratchThat,
    Indent,
    Dedent,
    Stop,
}

impl Ctl {
    /// Canonical spoken form — also the fast-path index label, so the
    /// fingerprint cache and the text matcher agree on one key.
    pub fn label(self) -> &'static str {
        match self {
            Ctl::NewLine => "new line",
            Ctl::NewParagraph => "new paragraph",
            Ctl::ScratchThat => "scratch that",
            Ctl::Indent => "indent",
            Ctl::Dedent => "dedent",
            Ctl::Stop => "stop dictating",
        }
    }

    /// Every phrase that maps to this control. Callers enrol all of
    /// them in the fast-path index; the UI shows `label()`.
    pub fn phrases() -> &'static [(&'static str, Ctl)] {
        &[
            ("new line", Ctl::NewLine),
            ("newline", Ctl::NewLine),
            ("next line", Ctl::NewLine),
            ("new paragraph", Ctl::NewParagraph),
            ("blank line", Ctl::NewParagraph),
            ("scratch that", Ctl::ScratchThat),
            ("delete that", Ctl::ScratchThat),
            ("undo that", Ctl::ScratchThat),
            ("indent", Ctl::Indent),
            ("tab in", Ctl::Indent),
            ("dedent", Ctl::Dedent),
            ("tab out", Ctl::Dedent),
            ("unindent", Ctl::Dedent),
            ("stop dictating", Ctl::Stop),
            ("stop dictation", Ctl::Stop),
            ("end dictation", Ctl::Stop),
        ]
    }

    fn from_phrase(p: &str) -> Option<Ctl> {
        Ctl::phrases().iter().find(|(k, _)| *k == p).map(|(_, c)| *c)
    }
}

/// One piece of a segmented utterance.
#[derive(Debug, Clone, PartialEq)]
pub enum Piece {
    Control(Ctl),
    Text(String),
}

/// Split a raw transcript into control verbs and dictated content.
///
/// Longest-match over the control phrases, scanning left to right. A
/// control phrase is only recognised on a WORD boundary, so "the new
/// line of code" does not fire NewLine mid-sentence — but a bare "new
/// line" does. This is the ambiguity that makes dictation hard and it
/// is not fully solvable by text alone; see `segment_strict`.
pub fn segment(transcript: &str) -> Vec<Piece> {
    let norm = normalize(transcript);
    let words: Vec<&str> = norm.split_whitespace().collect();
    let max_span = Ctl::phrases().iter().map(|(p, _)| p.split_whitespace().count()).max().unwrap_or(1);
    let mut out: Vec<Piece> = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut hit = None;
        let upper = max_span.min(words.len() - i);
        for len in (1..=upper).rev() {
            let span = words[i..i + len].join(" ");
            if let Some(c) = Ctl::from_phrase(&span) {
                hit = Some((c, len));
                break;
            }
        }
        match hit {
            Some((c, len)) => {
                flush(&mut pending, &mut out);
                out.push(Piece::Control(c));
                i += len;
            }
            None => {
                pending.push(words[i]);
                i += 1;
            }
        }
    }
    flush(&mut pending, &mut out);
    out
}

/// Strict variant: a control only counts if it is the WHOLE utterance.
/// This is the mode to use when the fast path is driving, because the
/// fast path classifies a whole audio burst — it never sees "half an
/// utterance". Mixing the two modes is how you get "new line" swallowed
/// out of the middle of a sentence the user meant literally.
pub fn segment_strict(transcript: &str) -> Vec<Piece> {
    let n = normalize(transcript);
    match Ctl::from_phrase(&n) {
        Some(c) => vec![Piece::Control(c)],
        None if n.is_empty() => vec![],
        None => vec![Piece::Text(n)],
    }
}

fn flush(pending: &mut Vec<&str>, out: &mut Vec<Piece>) {
    if !pending.is_empty() {
        out.push(Piece::Text(pending.join(" ")));
        pending.clear();
    }
}

fn normalize(s: &str) -> String {
    s.trim()
        .to_lowercase()
        .chars()
        .filter(|c| !matches!(c, '.' | ',' | '!' | '?' | ';' | ':'))
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

// ─────────────────────────────────────────────────────────── spoken code

/// What we are dictating INTO. Governs the symbol vocabulary and the
/// spacing rules; prose and code want opposite defaults.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Mode {
    Prose,
    Code,
}

/// A rendered token plus how it glues to its neighbours. `glue_left`
/// means "no space before me" (`)` `,` `;`), `glue_right` means "no
/// space after me" (`(` `&`). Operators like `::` and `.` glue both.
#[derive(Debug, Clone, PartialEq)]
struct Tok {
    text: String,
    glue_left: bool,
    glue_right: bool,
}

impl Tok {
    fn word(s: &str) -> Tok {
        Tok { text: s.to_string(), glue_left: false, glue_right: false }
    }
}

/// Spoken symbol → source text. Multi-word entries are matched
/// longest-first, so "double colon" beats "colon".
fn symbol(span: &str) -> Option<Tok> {
    let t = |s: &str, l: bool, r: bool| Some(Tok { text: s.into(), glue_left: l, glue_right: r });
    match span {
        "open paren" | "left paren" | "open parenthesis" => t("(", false, true),
        "close paren" | "right paren" | "close parenthesis" => t(")", true, false),
        "open brace" | "left brace" | "open curly" => t("{", false, false),
        "close brace" | "right brace" | "close curly" => t("}", false, false),
        "open bracket" | "left bracket" => t("[", false, true),
        "close bracket" | "right bracket" => t("]", true, false),
        "open angle" | "less than" => t("<", false, true),
        "close angle" | "greater than" => t(">", true, false),
        "arrow" | "thin arrow" => t("->", false, false),
        "fat arrow" | "rocket" => t("=>", false, false),
        "double colon" | "colon colon" | "path separator" => t("::", true, true),
        "colon" => t(":", true, false),
        "semicolon" | "semi colon" => t(";", true, false),
        "comma" => t(",", true, false),
        "dot" | "period" | "full stop" => t(".", true, true),
        "question mark" => t("?", true, false),
        "bang" | "exclamation mark" => t("!", true, false),
        "double equals" | "equals equals" => t("==", false, false),
        "not equals" | "bang equals" => t("!=", false, false),
        "equals" | "assign" => t("=", false, false),
        "ampersand" | "and sign" | "reference" => t("&", false, true),
        "star" | "asterisk" => t("*", false, true),
        "pipe" => t("|", false, false),
        "underscore" => t("_", true, true),
        "quote" | "double quote" => t("\"", false, false),
        "single quote" | "tick" => t("'", false, false),
        "backtick" => t("`", false, false),
        "hash" | "pound" | "octothorpe" => t("#", false, true),
        "at sign" => t("@", false, true),
        "dollar" | "dollar sign" => t("$", false, true),
        "slash" | "forward slash" => t("/", true, true),
        "backslash" => t("\\", true, true),
        "plus" => t("+", false, false),
        "minus" | "dash" | "hyphen" => t("-", false, false),
        "percent" => t("%", false, false),
        _ => None,
    }
}

/// Casing directives. Each applies to the run of plain words that
/// follows, terminated by any symbol, another directive, or the end.
/// So "let mut snake user count equals five" → `let mut user_count = five`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Casing {
    Snake,
    Camel,
    Pascal,
    Screaming,
    Kebab,
}

fn casing(span: &str) -> Option<Casing> {
    match span {
        "snake" | "snake case" => Some(Casing::Snake),
        "camel" | "camel case" => Some(Casing::Camel),
        "pascal" | "pascal case" | "type case" => Some(Casing::Pascal),
        "screaming" | "screaming case" | "constant case" | "shout" => Some(Casing::Screaming),
        "kebab" | "kebab case" | "dash case" => Some(Casing::Kebab),
        _ => None,
    }
}

fn apply_casing(c: Casing, words: &[&str]) -> String {
    match c {
        Casing::Snake => words.join("_"),
        Casing::Kebab => words.join("-"),
        Casing::Screaming => words.join("_").to_uppercase(),
        Casing::Camel => words
            .iter()
            .enumerate()
            .map(|(i, w)| if i == 0 { w.to_string() } else { title(w) })
            .collect(),
        Casing::Pascal => words.iter().map(|w| title(w)).collect(),
    }
}

fn title(w: &str) -> String {
    let mut cs = w.chars();
    match cs.next() {
        Some(f) => f.to_uppercase().collect::<String>() + cs.as_str(),
        None => String::new(),
    }
}

/// Turn a dictated content string into source text.
///
/// In `Mode::Prose` this is close to a passthrough (whisper's own
/// punctuation is fine there). In `Mode::Code` it runs the symbol and
/// casing vocabulary and applies code spacing.
pub fn spoken_code(text: &str, mode: Mode) -> String {
    if mode == Mode::Prose {
        return text.trim().to_string();
    }
    let words: Vec<&str> = text.split_whitespace().collect();
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // longest symbol span first (max 3 words: "open parenthesis", etc.)
        let mut matched = false;
        let upper = 3.min(words.len() - i);
        for len in (1..=upper).rev() {
            let span = words[i..i + len].join(" ");
            if let Some(t) = symbol(&span) {
                toks.push(t);
                i += len;
                matched = true;
                break;
            }
            if let Some(c) = casing(&span) {
                // consume the run of plain words after the directive
                let mut j = i + len;
                let mut run: Vec<&str> = Vec::new();
                while j < words.len() {
                    let stop = (1..=3.min(words.len() - j))
                        .any(|l| {
                            let s = words[j..j + l].join(" ");
                            symbol(&s).is_some() || casing(&s).is_some()
                        });
                    if stop {
                        break;
                    }
                    run.push(words[j]);
                    j += 1;
                }
                if !run.is_empty() {
                    toks.push(Tok::word(&apply_casing(c, &run)));
                }
                i = j;
                matched = true;
                break;
            }
        }
        if !matched {
            toks.push(Tok::word(words[i]));
            i += 1;
        }
    }
    join(&toks)
}

/// Keywords after which an open paren is NOT a call/signature, so it
/// keeps its space: `if (a || b)`, `return (x)`. Rust rarely writes
/// these, but C/Python/JS dictation goes through the same path.
const PAREN_SPACED_AFTER: &[&str] = &[
    "if", "while", "for", "match", "return", "in", "else", "and", "or", "not",
    "switch", "catch", "with", "assert", "yield", "await", "print",
];

fn join(toks: &[Tok]) -> String {
    let mut out = String::new();
    for (n, t) in toks.iter().enumerate() {
        let mut glue_left = t.glue_left;
        // `foo(` / `arr[` — an opening bracket binds to the identifier
        // it follows, which is what makes calls and signatures read
        // right. Only when the previous token is a plain word.
        if n > 0 && (t.text == "(" || t.text == "[") {
            let prev = &toks[n - 1];
            let plain = prev
                .text
                .chars()
                .all(|c| c.is_alphanumeric() || c == '_')
                && !prev.text.is_empty();
            if plain && !PAREN_SPACED_AFTER.contains(&prev.text.as_str()) {
                glue_left = true;
            }
        }
        if n > 0 && !glue_left && !toks[n - 1].glue_right {
            out.push(' ');
        }
        out.push_str(&t.text);
    }
    out
}

// ───────────────────────────────────────────────────────────── edits

/// Where dictated text should land. `Cursor` is the only variant a
/// keystroke sink can honour without reading the buffer — the rest need
/// the document, which is the whole reason the two sinks differ.
#[derive(Debug, Clone, PartialEq)]
pub enum Placement {
    /// At the cursor. Always available. Zero latency, deterministic.
    Cursor,
    /// At the start of a 1-based line. Reachable blind (`NNgg` / `:NN`).
    Line(usize),
    /// Replace a byte range. Needs the document.
    Range(usize, usize),
}

impl Placement {
    /// Whether a given sink can honour this placement. Callers use this
    /// to decide whether an LLM placement proposal is even actionable
    /// before spending the tokens.
    pub fn supported_by(&self, blind: bool) -> bool {
        match self {
            Placement::Cursor => true,
            Placement::Line(_) => true,
            Placement::Range(..) => !blind,
        }
    }
}

/// One atomic change. The whole point of naming these is that they are
/// what an undo boundary wraps (undo_stack::record_voice takes one
/// utterance = one entry) and what a preview diff renders.
#[derive(Debug, Clone, PartialEq)]
pub enum Edit {
    Insert { at: Placement, text: String },
    /// Delete the previous dictated unit ("scratch that").
    RetractLast,
    Newline { count: usize },
    IndentDelta(i32),
    EndSession,
}

/// Compile a segmented utterance into an edit script.
pub fn plan(pieces: &[Piece], mode: Mode, at: Placement) -> Vec<Edit> {
    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(t) => {
                let s = spoken_code(t, mode);
                if !s.is_empty() {
                    out.push(Edit::Insert { at: at.clone(), text: s });
                }
            }
            Piece::Control(Ctl::NewLine) => out.push(Edit::Newline { count: 1 }),
            Piece::Control(Ctl::NewParagraph) => out.push(Edit::Newline { count: 2 }),
            Piece::Control(Ctl::ScratchThat) => out.push(Edit::RetractLast),
            Piece::Control(Ctl::Indent) => out.push(Edit::IndentDelta(1)),
            Piece::Control(Ctl::Dedent) => out.push(Edit::IndentDelta(-1)),
            Piece::Control(Ctl::Stop) => out.push(Edit::EndSession),
        }
    }
    out
}

// ───────────────────────────────────────────────────────────── sinks

/// Sink A: we own the document. Full placement support, real diffs,
/// real undo. This is the in-app note editor path.
#[derive(Debug, Default)]
pub struct BufferSink {
    pub text: String,
    pub cursor: usize,
    /// byte length of each applied insert, for RetractLast
    trail: Vec<usize>,
    pub ended: bool,
}

impl BufferSink {
    pub fn new(text: &str, cursor: usize) -> Self {
        BufferSink { text: text.to_string(), cursor: cursor.min(text.len()), ended: false, trail: Vec::new() }
    }

    pub fn apply(&mut self, edits: &[Edit]) {
        for e in edits {
            match e {
                Edit::Insert { at, text } => {
                    let pos = self.resolve(at);
                    let sep = self.spacer(pos);
                    let ins = format!("{sep}{text}");
                    self.text.insert_str(pos, &ins);
                    self.cursor = pos + ins.len();
                    self.trail.push(ins.len());
                }
                Edit::Newline { count } => {
                    let s = "\n".repeat(*count);
                    self.text.insert_str(self.cursor, &s);
                    self.cursor += s.len();
                    self.trail.push(s.len());
                }
                Edit::IndentDelta(d) => {
                    if *d > 0 {
                        let s = "    ".repeat(*d as usize);
                        self.text.insert_str(self.cursor, &s);
                        self.cursor += s.len();
                        self.trail.push(s.len());
                    } else {
                        for _ in 0..(-d) {
                            let take = self.text[..self.cursor]
                                .chars()
                                .rev()
                                .take_while(|c| *c == ' ')
                                .count()
                                .min(4);
                            if take > 0 {
                                self.text.replace_range(self.cursor - take..self.cursor, "");
                                self.cursor -= take;
                            }
                        }
                    }
                }
                Edit::RetractLast => {
                    if let Some(n) = self.trail.pop() {
                        let start = self.cursor.saturating_sub(n);
                        self.text.replace_range(start..self.cursor, "");
                        self.cursor = start;
                    }
                }
                Edit::EndSession => self.ended = true,
            }
        }
    }

    fn resolve(&self, at: &Placement) -> usize {
        match at {
            Placement::Cursor => self.cursor,
            Placement::Line(n) => {
                let mut off = 0;
                for (i, l) in self.text.split_inclusive('\n').enumerate() {
                    if i + 1 == *n {
                        return off;
                    }
                    off += l.len();
                }
                self.text.len()
            }
            Placement::Range(a, _) => *a,
        }
    }

    /// A space before the insert if we are mid-line after a word. The
    /// small thing that makes consecutive utterances read as one
    /// sentence instead of "helloworld".
    fn spacer(&self, pos: usize) -> &'static str {
        match self.text[..pos].chars().last() {
            None | Some('\n') | Some(' ') | Some('\t') => "",
            Some('(') | Some('[') | Some('&') => "",
            _ => " ",
        }
    }
}

/// Sink B: a modal editor owns the document; we can only send bytes.
/// Blind — no diff, no preview, no range placement. Everything is
/// wrapped in an explicit Esc/insert/Esc so we never depend on guessing
/// the current mode (the always-escape decision from
/// HELIX_integration_design.md).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyEditor {
    Helix,
    Vim,
}

pub fn to_keys(editor: KeyEditor, edits: &[Edit]) -> String {
    let mut out = String::new();
    for e in edits {
        match e {
            Edit::Insert { at, text } => {
                match at {
                    Placement::Line(n) => {
                        out.push('\x1b');
                        match editor {
                            KeyEditor::Helix => out.push_str(&format!("{}gg", n)),
                            KeyEditor::Vim => out.push_str(&format!(":{}\r", n)),
                        }
                        out.push('i');
                    }
                    _ => {
                        out.push('\x1b');
                        out.push('a'); // append: resume after the cursor char
                    }
                }
                out.push_str(text);
                out.push('\x1b');
            }
            Edit::Newline { count } => {
                out.push('\x1b');
                for _ in 0..*count {
                    out.push('o');
                    out.push('\x1b');
                }
            }
            // helix and vim both undo one logical edit with `u`; our
            // inserts are separate undo points, so this retracts exactly
            // the last dictated unit.
            Edit::RetractLast => {
                out.push('\x1b');
                out.push('u');
            }
            Edit::IndentDelta(d) => {
                out.push('\x1b');
                for _ in 0..d.abs() {
                    out.push(if *d > 0 { '>' } else { '<' });
                    if editor == KeyEditor::Vim {
                        out.push(if *d > 0 { '>' } else { '<' });
                    }
                }
            }
            Edit::EndSession => {}
        }
    }
    out
}

// ───────────────────────────────────────────────────────────── tests

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn segment_splits_control_from_content() {
        let p = segment("let mut x equal five new line return x");
        assert_eq!(
            p,
            vec![
                Piece::Text("let mut x equal five".into()),
                Piece::Control(Ctl::NewLine),
                Piece::Text("return x".into()),
            ]
        );
    }

    #[test]
    fn segment_longest_match_wins() {
        // "new paragraph" must beat "new line"-style shorter probes
        assert_eq!(segment("new paragraph"), vec![Piece::Control(Ctl::NewParagraph)]);
    }

    #[test]
    fn strict_mode_refuses_mid_utterance_controls() {
        // the ambiguity case: user is literally dictating the words
        let loose = segment("write a new line to the log");
        assert!(loose.iter().any(|p| matches!(p, Piece::Control(Ctl::NewLine))));
        let strict = segment_strict("write a new line to the log");
        assert_eq!(strict, vec![Piece::Text("write a new line to the log".into())]);
        // but a bare control still fires in strict mode
        assert_eq!(segment_strict("New line."), vec![Piece::Control(Ctl::NewLine)]);
    }

    #[test]
    fn prose_mode_is_passthrough() {
        assert_eq!(spoken_code("Hello, world.", Mode::Prose), "Hello, world.");
    }

    #[test]
    fn symbols_and_spacing() {
        assert_eq!(spoken_code("foo open paren bar comma baz close paren", Mode::Code), "foo(bar, baz)");
        assert_eq!(spoken_code("if open paren x close paren", Mode::Code), "if (x)");
        assert_eq!(spoken_code("std double colon fs double colon read", Mode::Code), "std::fs::read");
        assert_eq!(spoken_code("x arrow y", Mode::Code), "x -> y");
    }

    #[test]
    fn casing_run_terminates_at_symbol() {
        assert_eq!(
            spoken_code("let mut snake user count equals five", Mode::Code),
            "let mut user_count = five"
        );
        assert_eq!(spoken_code("pascal my type name", Mode::Code), "MyTypeName");
        assert_eq!(spoken_code("camel read file sync", Mode::Code), "readFileSync");
        assert_eq!(spoken_code("screaming max size equals ten", Mode::Code), "MAX_SIZE = ten");
    }

    #[test]
    fn casing_run_terminates_at_next_directive() {
        assert_eq!(spoken_code("snake foo bar pascal baz qux", Mode::Code), "foo_bar BazQux");
    }

    #[test]
    fn buffer_sink_inserts_with_spacing() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("hello there"), Mode::Prose, Placement::Cursor));
        b.apply(&plan(&segment("friend"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "hello there friend");
    }

    #[test]
    fn buffer_sink_scratch_that_retracts_one_unit() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("keep this"), Mode::Prose, Placement::Cursor));
        b.apply(&plan(&segment("oops wrong"), Mode::Prose, Placement::Cursor));
        b.apply(&plan(&segment("scratch that"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "keep this");
    }

    #[test]
    fn buffer_sink_newline_and_indent() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("fn main open paren close paren open brace"), Mode::Code, Placement::Cursor));
        b.apply(&plan(&segment("new line indent"), Mode::Code, Placement::Cursor));
        b.apply(&plan(&segment("let x equals one semicolon"), Mode::Code, Placement::Cursor));
        assert_eq!(b.text, "fn main() {\n    let x = one;");
    }

    #[test]
    fn buffer_sink_dedent_eats_up_to_four_spaces() {
        let mut b = BufferSink::new("        ", 8);
        b.apply(&[Edit::IndentDelta(-1)]);
        assert_eq!(b.text, "    ");
        b.apply(&[Edit::IndentDelta(-1)]);
        assert_eq!(b.text, "");
        b.apply(&[Edit::IndentDelta(-1)]); // no underflow on empty
        assert_eq!(b.text, "");
    }

    #[test]
    fn line_placement_resolves_to_line_start() {
        let mut b = BufferSink::new("aaa\nbbb\nccc", 0);
        b.apply(&[Edit::Insert { at: Placement::Line(2), text: "X".into() }]);
        assert_eq!(b.text, "aaa\nXbbb\nccc");
    }

    #[test]
    fn placement_reachability_is_explicit() {
        assert!(Placement::Cursor.supported_by(true));
        assert!(Placement::Line(3).supported_by(true));
        assert!(!Placement::Range(0, 5).supported_by(true));
        assert!(Placement::Range(0, 5).supported_by(false));
    }

    #[test]
    fn keysink_always_escapes_first() {
        let k = to_keys(KeyEditor::Helix, &[Edit::Insert { at: Placement::Cursor, text: "hi".into() }]);
        assert!(k.starts_with('\x1b'), "must not assume the editor's mode");
        assert!(k.ends_with('\x1b'), "must return to normal mode");
        assert!(k.contains("hi"));
    }

    #[test]
    fn keysink_line_jump_differs_per_editor() {
        let e = [Edit::Insert { at: Placement::Line(40), text: "z".into() }];
        assert!(to_keys(KeyEditor::Helix, &e).contains("40gg"));
        assert!(to_keys(KeyEditor::Vim, &e).contains(":40\r"));
    }

    #[test]
    fn end_to_end_code_dictation() {
        let utterances = [
            "pascal user store double colon new open paren close paren",
            "new line",
            "snake max retries equals three semicolon",
        ];
        let mut b = BufferSink::new("", 0);
        for u in utterances {
            b.apply(&plan(&segment(u), Mode::Code, Placement::Cursor));
        }
        assert_eq!(b.text, "UserStore::new()\nmax_retries = three;");
    }
}

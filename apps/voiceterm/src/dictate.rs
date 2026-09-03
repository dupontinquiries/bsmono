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

use crate::code_symbols::SymbolTable;
use crate::repair::{self, Repair};

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
    /// Switch what the transform does to subsequent content. A comment
    /// and the code under it are dictated in the same breath; without
    /// this the speaker has to leave the session to change register.
    SetMode(Mode),
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
            Ctl::SetMode(Mode::Code) => "code mode",
            Ctl::SetMode(Mode::Prose) => "prose mode",
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
            ("code mode", Ctl::SetMode(Mode::Code)),
            ("switch to code", Ctl::SetMode(Mode::Code)),
            ("prose mode", Ctl::SetMode(Mode::Prose)),
            ("text mode", Ctl::SetMode(Mode::Prose)),
            ("switch to prose", Ctl::SetMode(Mode::Prose)),
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
    // Match control phrases case-insensitively, but carry the ORIGINAL
    // casing into Piece::Text. Lowercasing the content here was a real
    // bug: whisper's capitalization is the only signal distinguishing
    // "Paris" from "parris", and a `Vec` dictated in code mode became
    // `vec`. Only the matcher wants a normalized form.
    let words: Vec<&str> = transcript.split_whitespace().collect();
    // Match on a stripped, lowercased view; keep the raw words for text.
    // Whisper's sentence punctuation is part of PROSE and noise in code,
    // so the decision of whether to drop it belongs to the mode-aware
    // transform, not to segmentation.
    let lower: Vec<String> = words.iter().map(|w| strip_punct(w).to_lowercase()).collect();
    let max_span = Ctl::phrases().iter().map(|(p, _)| p.split_whitespace().count()).max().unwrap_or(1);
    let mut out: Vec<Piece> = Vec::new();
    let mut pending: Vec<&str> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        let mut hit = None;
        let upper = max_span.min(words.len() - i);
        for len in (1..=upper).rev() {
            let span = lower[i..i + len].join(" ");
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
    let raw = transcript.split_whitespace().collect::<Vec<_>>().join(" ");
    if raw.is_empty() {
        return vec![];
    }
    match Ctl::from_phrase(&strip_punct(&raw).to_lowercase()) {
        Some(c) => vec![Piece::Control(c)],
        None => vec![Piece::Text(raw)],
    }
}

fn flush(pending: &mut Vec<&str>, out: &mut Vec<Piece>) {
    if !pending.is_empty() {
        out.push(Piece::Text(pending.join(" ")));
        pending.clear();
    }
}

/// Drop sentence punctuation, PRESERVING case. Used for phrase
/// MATCHING, and for code content where whisper's guess at prose
/// structure is noise.
fn strip_punct(s: &str) -> String {
    s.trim()
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
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Mode {
    /// Whisper's own punctuation and capitalization are already right
    /// for prose, so the transform is near-passthrough. The default,
    /// because it is the safe one: a Code-mode misfire turns spoken
    /// words into symbols.
    #[default]
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
///
/// Several arms carry whisper HOMOPHONES rather than things anyone
/// says: "coma" for comma, "dote" for dot, "open parent" for open
/// paren, "braise" for brace, "aero" for arrow. Measured against
/// degraded transcripts, these are the substitutions that actually
/// occur, and each one otherwise leaves a stray English word in the
/// middle of a line of code.
///
/// One judgment call: "column" → `:`. Colon is among the highest
/// frequency symbols in typed code, and "column" is what whisper
/// returns for it often enough to matter — but "column" is also a real
/// identifier word in database code. Both failure directions are
/// visible and repairable; this bets on the commoner one. Delete the
/// alias if your buffers are full of columns.
fn symbol(span: &str) -> Option<Tok> {
    let t = |s: &str, l: bool, r: bool| Some(Tok { text: s.into(), glue_left: l, glue_right: r });
    match span {
        "open paren" | "left paren" | "open parenthesis" | "open parent" | "opening paren" => t("(", false, true),
        "close paren" | "right paren" | "close parenthesis" | "close parent" | "closing paren" => t(")", true, false),
        "open brace" | "left brace" | "open curly" | "open braise" | "open brays" => t("{", false, false),
        "close brace" | "right brace" | "close curly" | "close braise" | "close brays" => t("}", false, false),
        "open bracket" | "left bracket" => t("[", false, true),
        "close bracket" | "right bracket" => t("]", true, false),
        "open angle" | "less than" => t("<", false, true),
        "close angle" | "greater than" => t(">", true, false),
        "arrow" | "thin arrow" | "aero" => t("->", false, false),
        "fat arrow" | "rocket" | "fat aero" => t("=>", false, false),
        "double colon" | "colon colon" | "path separator" | "double column" | "double colin" => t("::", true, true),
        "colon" | "column" | "colin" => t(":", true, false),
        "semicolon" | "semi colon" | "semi-colon" | "semicolin" => t(";", true, false),
        "comma" | "coma" => t(",", true, false),
        "dot" | "period" | "full stop" | "dote" => t(".", true, true),
        "question mark" => t("?", true, false),
        "bang" | "exclamation mark" => t("!", true, false),
        "double equals" | "equals equals" => t("==", false, false),
        "not equals" | "bang equals" => t("!=", false, false),
        "equals" | "assign" | "equal" => t("=", false, false),
        "ampersand" | "and sign" | "reference" | "amperstand" | "ampersat" => t("&", false, true),
        "star" | "asterisk" | "asterix" => t("*", false, true),
        "pipe" => t("|", false, false),
        "underscore" | "under score" => t("_", true, true),
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
/// Identifier casing conventions. Public so the repair layer can reuse
/// the same transform rather than growing a second copy of it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Casing {
    Snake,
    Camel,
    Pascal,
    Screaming,
    Kebab,
}

pub fn casing(span: &str) -> Option<Casing> {
    match span {
        "snake" | "snake case" => Some(Casing::Snake),
        "camel" | "camel case" => Some(Casing::Camel),
        "pascal" | "pascal case" | "type case" => Some(Casing::Pascal),
        "screaming" | "screaming case" | "constant case" | "shout" => Some(Casing::Screaming),
        "kebab" | "kebab case" | "dash case" => Some(Casing::Kebab),
        _ => None,
    }
}

pub fn apply_casing(c: Casing, words: &[&str]) -> String {
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
    spoken_code_with(text, mode, None)
}

/// As `spoken_code`, but resolving bare word runs against the
/// identifiers actually present in the buffer.
///
/// This is what removes the ceremony: with a table built from the open
/// file, "read to string" becomes `read_to_string` without the speaker
/// prefixing a casing directive. Resolution order at each position is
/// symbol, then explicit casing directive, then the table, then a
/// number word, then the plain word — explicit always beats inferred,
/// so a directive can override a bad table hit.
pub fn spoken_code_with(text: &str, mode: Mode, syms: Option<&SymbolTable>) -> String {
    if mode == Mode::Prose {
        // Whisper's punctuation and capitalization are already what
        // prose wants; the only fix prose needs is the continuation
        // capital, and that belongs to the sink, which can see what
        // precedes the insert.
        return text.trim().to_string();
    }
    let stripped = strip_punct(text);
    let words: Vec<&str> = strip_fillers(&stripped.split_whitespace().collect::<Vec<_>>());
    // Whisper capitalizes the first word of every utterance, so a code
    // line dictated as its own burst arrives as "Let raw equals...".
    // Only KEYWORDS are lowered, not any capitalized leading word: a
    // line starting with a type name the buffer has not seen yet
    // ("Config open brace") is a real case, and guessing there would
    // corrupt a name the speaker deliberately capitalized.
    let leading = words.first().map(|w| lower_if_keyword(w));
    let words: Vec<&str> = match &leading {
        Some(Some(k)) => std::iter::once(*k).chain(words.iter().skip(1).copied()).collect(),
        _ => words,
    };
    let mut toks: Vec<Tok> = Vec::new();
    let mut i = 0;
    while i < words.len() {
        // longest symbol span first (max 3 words: "open parenthesis", etc.)
        let mut matched = false;
        let upper = 3.min(words.len() - i);
        for len in (1..=upper).rev() {
            let span = words[i..i + len].join(" ").to_lowercase();
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
                            let s = words[j..j + l].join(" ").to_lowercase();
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
            // buffer identifiers: longest run that resolves. Bounded at
            // 5 words because that is already a long name and the scan
            // is O(span) per position.
            if let Some(t) = syms {
                if let Some((n, id)) = t.resolve_longest(&words[i..], 5) {
                    toks.push(Tok::word(id));
                    i += n;
                    matched = true;
                }
            }
        }
        if !matched {
            // Rust primitive types, spoken as letter-plus-number. Bounded
            // closed set, very high frequency, and the numeral pass would
            // otherwise shred "f sixty four" into "f 60 4". The transform
            // is already Rust-leaning (-> :: =>), so this is consistent
            // rather than a new assumption.
            let upper = 3.min(words.len() - i);
            for len in (2..=upper).rev() {
                let span = words[i..i + len].join(" ").to_lowercase();
                if let Some(t) = primitive_type(&span) {
                    toks.push(Tok::word(t));
                    i += len;
                    matched = true;
                    break;
                }
            }
        }
        if !matched {
            // Path position: a single word next to `::` is naming code,
            // not prose, so the table may resolve it there even though
            // bare single words are refused everywhere else.
            if let Some(t) = syms {
                let after_sep = toks.last().map(|p| p.text == "::").unwrap_or(false);
                let before_sep = (1..=2.min(words.len() - i - 1)).any(|l| {
                    words.get(i + 1..i + 1 + l).map(|w| {
                        symbol(&w.join(" ").to_lowercase()).map(|s| s.text == "::").unwrap_or(false)
                    }) == Some(true)
                });
                if after_sep || before_sep {
                    if let Some(id) = t.resolve_path_segment(words[i]) {
                        toks.push(Tok::word(id));
                        i += 1;
                        matched = true;
                    }
                }
            }
        }
        if !matched {
            if let Some(n) = numeral(&words[i].to_lowercase()) {
                toks.push(Tok::word(&n.to_string()));
                i += 1;
                matched = true;
            }
        }
        if !matched {
            toks.push(Tok::word(words[i]));
            i += 1;
        }
    }
    join(&toks)
}

/// Keywords across the languages this transform targets. Only used to
/// undo whisper's utterance-initial capital.
const LEADING_KEYWORDS: &[&str] = &[
    "let", "fn", "pub", "use", "mod", "impl", "struct", "enum", "trait", "type", "const",
    "static", "match", "if", "else", "for", "while", "loop", "return", "break", "continue",
    "unsafe", "async", "await", "move", "where", "def", "class", "import", "from", "var",
    "function", "public", "private", "void", "new", "try", "catch", "throw", "print", "with",
    "elif", "lambda", "yield", "assert", "delete", "export", "interface", "package",
];

fn lower_if_keyword(w: &str) -> Option<&'static str> {
    let mut cs = w.chars();
    let f = cs.next()?;
    if !f.is_uppercase() || cs.any(|c| c.is_uppercase()) {
        return None;
    }
    let lw = w.to_lowercase();
    LEADING_KEYWORDS.iter().find(|k| **k == lw).copied()
}

/// Disfluencies whisper transcribes literally. Removed in Code mode
/// only.
///
/// Prose keeps them on purpose: "um" in dictated dialogue or an
/// interview transcript is content, and silently deleting words from
/// someone's prose is a worse failure than leaving one in. In code it
/// is never content.
///
/// Deliberately short. "like", "so", "right", "well" and "I mean" are
/// all real disfluencies and all real words; stripping them would break
/// more than it fixes.
const FILLERS: &[&str] = &["um", "uh", "erm", "uhm", "ahem", "mhm", "hmm"];
const FILLER_PAIRS: &[[&str; 2]] = &[["you", "know"], ["sort", "of"], ["kind", "of"]];

fn strip_fillers<'a>(words: &[&'a str]) -> Vec<&'a str> {
    let mut out: Vec<&str> = Vec::with_capacity(words.len());
    let mut i = 0;
    while i < words.len() {
        let lw = words[i].to_lowercase();
        if i + 1 < words.len() {
            let lw2 = words[i + 1].to_lowercase();
            if FILLER_PAIRS.iter().any(|p| p[0] == lw && p[1] == lw2) {
                i += 2;
                continue;
            }
        }
        if FILLERS.contains(&lw.as_str()) {
            i += 1;
            continue;
        }
        out.push(words[i]);
        i += 1;
    }
    out
}

/// Spoken forms of the Rust primitive numeric types.
fn primitive_type(span: &str) -> Option<&'static str> {
    Some(match span {
        "f thirty two" => "f32",
        "f sixty four" => "f64",
        "u eight" => "u8",
        "u sixteen" => "u16",
        "u thirty two" => "u32",
        "u sixty four" => "u64",
        "u one twenty eight" | "u128" => "u128",
        "i eight" => "i8",
        "i sixteen" => "i16",
        "i thirty two" => "i32",
        "i sixty four" => "i64",
        "i one twenty eight" => "i128",
        _ => return None,
    })
}

/// Number words → digits. Code mode only, where "thirty" is far more
/// likely to be a literal than a variable. Deliberately stops at the
/// simple cases: compound forms ("twenty five", "three hundred") need
/// accumulator parsing whose failure mode is silently wrong constants,
/// which is worse than leaving the words alone for the speaker to fix.
fn numeral(w: &str) -> Option<u32> {
    Some(match w {
        "zero" => 0, "one" => 1, "two" => 2, "three" => 3, "four" => 4,
        "five" => 5, "six" => 6, "seven" => 7, "eight" => 8, "nine" => 9,
        "ten" => 10, "eleven" => 11, "twelve" => 12, "thirteen" => 13,
        "fourteen" => 14, "fifteen" => 15, "sixteen" => 16, "seventeen" => 17,
        "eighteen" => 18, "nineteen" => 19, "twenty" => 20, "thirty" => 30,
        "forty" => 40, "fifty" => 50, "sixty" => 60, "seventy" => 70,
        "eighty" => 80, "ninety" => 90, "hundred" => 100, "thousand" => 1000,
        _ => return None,
    })
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
    // Quotes are the one symbol whose spacing depends on WHICH of the
    // pair it is, and the spoken form is identical for both. Parity is
    // the only signal available: odd occurrence opens (glue right),
    // even closes (glue left). Wrong only if the speaker leaves a quote
    // unbalanced, which is a syntax error they will see immediately.
    let mut open_quotes: std::collections::HashMap<&str, bool> = std::collections::HashMap::new();
    let mut prev_glue_right = false;
    for (n, t) in toks.iter().enumerate() {
        let mut glue_left = t.glue_left;
        let mut glue_right = t.glue_right;
        if matches!(t.text.as_str(), "\"" | "'" | "`") {
            let open = open_quotes.entry(t.text.as_str()).or_insert(false);
            if *open {
                glue_left = true;
                glue_right = false;
            } else {
                glue_left = false;
                glue_right = true;
            }
            *open = !*open;
        }
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
        if n > 0 && !glue_left && !prev_glue_right {
            out.push(' ');
        }
        out.push_str(&t.text);
        prev_glue_right = glue_right;
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
    /// Fix the previous dictated unit in place instead of discarding it.
    Repair(Repair),
    Newline { count: usize },
    IndentDelta(i32),
    SetMode(Mode),
    EndSession,
}

/// Compile a segmented utterance into an edit script.
pub fn plan(pieces: &[Piece], mode: Mode, at: Placement) -> Vec<Edit> {
    plan_with(pieces, mode, at, None)
}

/// As `plan`, with the buffer's identifier table available to the
/// spoken-code transform.
pub fn plan_with(
    pieces: &[Piece],
    mode: Mode,
    at: Placement,
    syms: Option<&SymbolTable>,
) -> Vec<Edit> {
    let mut out = Vec::new();
    for p in pieces {
        match p {
            Piece::Text(t) => {
                let s = spoken_code_with(t, mode, syms);
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
            Piece::Control(Ctl::SetMode(m)) => out.push(Edit::SetMode(*m)),
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
    /// Span of the last Insert: (start, spacer_len, text_len). Repairs
    /// act on the TEXT part only, so the leading space survives a
    /// re-case and `Join` has something to remove. Newlines and indents
    /// insert after it, so the offsets stay valid until the next Insert.
    last_insert: Option<(usize, usize, usize)>,
    /// Register for inserted text. Prose gets the continuation-casing
    /// fix below; Code does not, because whisper's capitalization of a
    /// code utterance is noise the symbol table already overrides.
    pub mode: Mode,
    /// Current indent level, carried across Newline. Without this every
    /// dictated line starts at column 0 and the speaker has to say
    /// "indent" after every "new line" — which is how a real editor's
    /// auto-indent earns its keep.
    indent: usize,
    pub ended: bool,
}

/// One indent level, in spaces. Not configurable yet; when it is, it
/// belongs with the per-filetype settings, not here.
const INDENT_WIDTH: usize = 4;

impl BufferSink {
    pub fn new(text: &str, cursor: usize) -> Self {
        let cursor = floor_char_boundary(text, cursor);
        BufferSink {
            text: text.to_string(),
            cursor,
            ended: false,
            trail: Vec::new(),
            last_insert: None,
            mode: Mode::Prose,
            indent: 0,
        }
    }

    pub fn apply(&mut self, edits: &[Edit]) {
        for e in edits {
            match e {
                Edit::Insert { at, text } => {
                    let pos = self.resolve(at);
                    let sep = self.spacer(pos);
                    let body = if self.mode == Mode::Prose {
                        decapitalize_continuation(text, self.preceding(pos))
                    } else {
                        text.clone()
                    };
                    let ins = format!("{sep}{body}");
                    self.text.insert_str(pos, &ins);
                    self.cursor = pos + ins.len();
                    self.trail.push(ins.len());
                    self.last_insert = Some((pos, sep.len(), body.len()));
                }
                Edit::Newline { count } => {
                    let s = format!(
                        "{}{}",
                        "\n".repeat(*count),
                        " ".repeat(self.indent * INDENT_WIDTH)
                    );
                    self.text.insert_str(self.cursor, &s);
                    self.cursor += s.len();
                    self.trail.push(s.len());
                }
                Edit::IndentDelta(d) => {
                    // Adjust both the persistent level (which future
                    // Newlines inherit) and the current line.
                    if *d > 0 {
                        self.indent += *d as usize;
                        let s = " ".repeat(*d as usize * INDENT_WIDTH);
                        self.text.insert_str(self.cursor, &s);
                        self.cursor += s.len();
                        self.trail.push(s.len());
                    } else {
                        self.indent = self.indent.saturating_sub(d.unsigned_abs() as usize);
                        for _ in 0..d.unsigned_abs() {
                            let take = self.text[..self.cursor]
                                .chars()
                                .rev()
                                .take_while(|c| *c == ' ')
                                .count()
                                .min(INDENT_WIDTH);
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
                    self.last_insert = None;
                }
                Edit::Repair(r) => {
                    self.repair(r);
                }
                Edit::SetMode(m) => self.mode = *m,
                Edit::EndSession => self.ended = true,
            }
        }
    }

    pub fn with_mode(mut self, m: Mode) -> Self {
        self.mode = m;
        self
    }

    /// The text of the last dictated unit, if one is still addressable.
    pub fn last_unit(&self) -> Option<&str> {
        let (start, sep, len) = self.last_insert?;
        self.text.get(start + sep..start + sep + len)
    }

    /// Apply a repair to the last unit. Returns false when the repair
    /// does not apply (nothing dictated yet, or a `Change` whose target
    /// is absent) so the caller can say so instead of leaving the user
    /// wondering whether it worked.
    pub fn repair(&mut self, r: &Repair) -> bool {
        let Some((start, sep, len)) = self.last_insert else { return false };
        if *r == Repair::Join {
            if sep == 0 {
                return false;
            }
            self.text.replace_range(start..start + sep, "");
            self.cursor -= sep;
            self.last_insert = Some((start, 0, len));
            if let Some(t) = self.trail.last_mut() {
                *t -= sep;
            }
            return true;
        }
        let Some(unit) = self.last_unit().map(|s| s.to_string()) else { return false };
        let Some(new) = repair::apply(&unit, r) else { return false };
        let from = start + sep;
        self.text.replace_range(from..from + len, &new);
        let delta = new.len() as isize - len as isize;
        self.cursor = (self.cursor as isize + delta) as usize;
        self.last_insert = Some((start, sep, new.len()));
        if let Some(t) = self.trail.last_mut() {
            *t = (*t as isize + delta) as usize;
        }
        true
    }

    /// Last non-space character before `pos` ON THE SAME LINE.
    ///
    /// Stopping at the newline matters: a new line starts a new
    /// sentence as far as capitalization goes, and scanning past it
    /// would lowercase the first word of every line after the first.
    fn preceding(&self, pos: usize) -> Option<char> {
        self.text[..pos]
            .chars()
            .rev()
            .take_while(|c| *c != '\n')
            .find(|c| !c.is_whitespace())
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
            // Snap: a caller-supplied offset is not guaranteed to be a
            // char boundary, and slicing at one that is not panics.
            Placement::Range(a, _) => floor_char_boundary(&self.text, *a),
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

/// Largest char boundary <= `i`, clamped to the string length.
///
/// Byte offsets arrive from outside — an editor that counts characters,
/// a saved cursor position from before the buffer changed, a Placement
/// the LLM proposed. Slicing at a non-boundary panics, and a panic on
/// the UI thread is a crash, so every externally-supplied offset gets
/// snapped rather than trusted. (`str::floor_char_boundary` is still
/// unstable as of 1.96.)
pub fn floor_char_boundary(s: &str, i: usize) -> usize {
    let mut i = i.min(s.len());
    while i > 0 && !s.is_char_boundary(i) {
        i -= 1;
    }
    i
}

/// Drop the stray capital whisper puts on every utterance's first word
/// when the utterance is CONTINUING a sentence.
///
/// Whisper transcribes each burst independently, so "the quick brown
/// fox" followed by "jumped over" arrives as "Jumped over" and the
/// buffer reads "the quick brown fox Jumped over". Lowering it is right
/// far more often than wrong.
///
/// When it IS wrong the word is a proper noun, and the fix is one short
/// utterance away — `repair::Repair::Capitalize` acts on exactly this
/// unit. That asymmetry is the whole argument: a stray capital every
/// other sentence is a constant tax, a lowered proper noun is
/// occasional and already has a one-phrase undo.
///
/// Left alone: anything after a sentence terminator or at the start of
/// a line, all-caps words (acronyms), and words with internal capitals
/// (`GitHub`, `iPhone`), which are never whisper's doing.
pub fn decapitalize_continuation(text: &str, preceding: Option<char>) -> String {
    let continuing = match preceding {
        None => false,
        Some(c) => !matches!(c, '.' | '!' | '?' | ':' | ';'),
    };
    if !continuing {
        return text.to_string();
    }
    let mut words = text.splitn(2, ' ');
    let Some(first) = words.next() else { return text.to_string() };
    let rest = words.next();
    let mut cs = first.chars();
    let Some(f) = cs.next() else { return text.to_string() };
    let tail: String = cs.collect();
    // only a lone leading capital: ALLCAPS and CamelCase stay
    if !f.is_uppercase() || tail.chars().any(|c| c.is_uppercase()) {
        return text.to_string();
    }
    // "I" is a word, not whisper being helpful
    if first == "I" {
        return text.to_string();
    }
    let lowered = f.to_lowercase().collect::<String>() + &tail;
    match rest {
        Some(r) => format!("{lowered} {r}"),
        None => lowered,
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
            // A repair needs to know what the last unit WAS, which a
            // blind sink does not. Undo-then-retype is the only honest
            // translation and it is worse than saying so: the caller
            // gets nothing and should tell the user repairs need the
            // in-app editor.
            Edit::Repair(_) => {}
            // Mode is our own state; the editor has no equivalent.
            Edit::SetMode(_) => {}
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
            "let mut user_count = 5"
        );
        assert_eq!(spoken_code("pascal my type name", Mode::Code), "MyTypeName");
        assert_eq!(spoken_code("camel read file sync", Mode::Code), "readFileSync");
        assert_eq!(spoken_code("screaming max size equals ten", Mode::Code), "MAX_SIZE = 10");
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
        assert_eq!(b.text, "fn main() {\n    let x = 1;");
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
    fn symbol_table_removes_the_casing_ceremony() {
        let buf = "fn read_to_string(p: &Path) -> Result<String> { UserStore::new() }";
        let t = SymbolTable::from_source(buf);
        // no "snake" directive needed — the buffer already knows
        assert_eq!(
            spoken_code_with("std double colon fs double colon read to string", Mode::Code, Some(&t)),
            "std::fs::read_to_string"
        );
        assert_eq!(spoken_code_with("user store", Mode::Code, Some(&t)), "UserStore");
        // and without the table you are back to the literal words
        assert_eq!(spoken_code("read to string", Mode::Code), "read to string");
    }

    #[test]
    fn explicit_directive_overrides_the_table() {
        let t = SymbolTable::from_source("read_to_string");
        assert_eq!(
            spoken_code_with("pascal read to string", Mode::Code, Some(&t)),
            "ReadToString"
        );
    }

    #[test]
    fn table_never_eats_a_symbol_word() {
        // a buffer containing an identifier whose spoken form starts
        // with a symbol word must not shadow the symbol
        let t = SymbolTable::from_source("comma_separated");
        assert_eq!(spoken_code_with("a comma b", Mode::Code, Some(&t)), "a, b");
    }

    #[test]
    fn number_words_become_digits_in_code_only() {
        assert_eq!(spoken_code("timeout equals thirty", Mode::Code), "timeout = 30");
        assert_eq!(spoken_code("thirty birds", Mode::Prose), "thirty birds");
        // compound forms are deliberately left alone rather than guessed
        assert_eq!(spoken_code("twenty five", Mode::Code), "20 5");
    }

    #[test]
    fn indent_persists_across_newlines() {
        let mut b = BufferSink::new("", 0);
        for u in ["open brace", "new line", "indent", "let a equals one semicolon", "new line", "let b equals two semicolon"] {
            b.apply(&plan(&segment(u), Mode::Code, Placement::Cursor));
        }
        assert_eq!(b.text, "{\n    let a = 1;\n    let b = 2;");
    }

    #[test]
    fn dedent_lowers_the_level_for_later_lines() {
        let mut b = BufferSink::new("", 0);
        for u in ["indent", "a", "new line", "dedent", "b", "new line", "c"] {
            b.apply(&plan(&segment(u), Mode::Code, Placement::Cursor));
        }
        assert_eq!(b.text, "    a\nb\nc");
    }

    #[test]
    fn repair_fixes_the_last_unit_in_place() {
        let mut b = BufferSink::new("let x = ", 8);
        b.apply(&plan(&segment("read to string"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.last_unit(), Some("read to string"));
        assert!(b.repair(&Repair::Recase(crate::dictate::Casing::Snake)));
        assert_eq!(b.text, "let x = read_to_string");
        // cursor tracked the shrink
        assert_eq!(b.cursor, b.text.len());
    }

    #[test]
    fn repair_survives_a_following_newline() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("path"), Mode::Prose, Placement::Cursor));
        b.apply(&[Edit::Newline { count: 1 }]);
        assert!(b.repair(&Repair::Capitalize));
        assert_eq!(b.text, "Path\n");
    }

    #[test]
    fn join_closes_the_leading_space() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("foo"), Mode::Prose, Placement::Cursor));
        b.apply(&plan(&segment("bar"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "foo bar");
        assert!(b.repair(&Repair::Join));
        assert_eq!(b.text, "foobar");
        assert_eq!(b.cursor, 6);
        // idempotent: nothing left to join
        assert!(!b.repair(&Repair::Join));
    }

    #[test]
    fn repair_reports_failure_rather_than_mangling() {
        let mut b = BufferSink::new("", 0);
        // nothing dictated yet
        assert!(!b.repair(&Repair::Capitalize));
        b.apply(&plan(&segment("read to string"), Mode::Prose, Placement::Cursor));
        assert!(!b.repair(&Repair::Change { from: "absent".into(), to: "x".into() }));
        assert_eq!(b.text, "read to string", "a failed repair changes nothing");
    }

    #[test]
    fn scratch_that_clears_the_repair_target() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("oops"), Mode::Prose, Placement::Cursor));
        b.apply(&[Edit::RetractLast]);
        assert_eq!(b.last_unit(), None);
        assert!(!b.repair(&Repair::Capitalize));
    }

    #[test]
    fn repairs_chain() {
        let mut b = BufferSink::new("", 0);
        b.apply(&plan(&segment("read to string"), Mode::Prose, Placement::Cursor));
        assert!(b.repair(&Repair::Change { from: "to".into(), to: "into".into() }));
        assert_eq!(b.text, "read into string");
        assert!(b.repair(&Repair::Recase(crate::dictate::Casing::Snake)));
        assert_eq!(b.text, "read_into_string");
    }

    #[test]
    fn keysink_cannot_repair_and_says_nothing() {
        // a blind sink has no idea what the last unit was; emitting a
        // guess would corrupt the buffer
        let k = to_keys(KeyEditor::Helix, &[Edit::Repair(Repair::Capitalize)]);
        assert_eq!(k, "");
    }

    #[test]
    fn prose_continuation_loses_whispers_stray_capital() {
        let mut b = BufferSink::new("", 0).with_mode(Mode::Prose);
        for u in ["the quick brown fox", "Jumped over the lazy dog"] {
            b.apply(&plan(&segment(u), Mode::Prose, Placement::Cursor));
        }
        assert_eq!(b.text, "the quick brown fox jumped over the lazy dog");
    }

    #[test]
    fn a_new_sentence_keeps_its_capital() {
        let mut b = BufferSink::new("She left.", 9).with_mode(Mode::Prose);
        b.apply(&plan(&segment("Then it rained"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "She left. Then it rained");
    }

    #[test]
    fn a_new_line_starts_a_new_sentence() {
        let mut b = BufferSink::new("first line", 10).with_mode(Mode::Prose);
        b.apply(&[Edit::Newline { count: 1 }]);
        b.apply(&plan(&segment("Second line"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "first line\nSecond line");
    }

    #[test]
    fn acronyms_and_internal_capitals_survive() {
        assert_eq!(decapitalize_continuation("HTTP is fine", Some('s')), "HTTP is fine");
        assert_eq!(decapitalize_continuation("GitHub said no", Some('s')), "GitHub said no");
        assert_eq!(decapitalize_continuation("I went", Some('s')), "I went", "lone I is all-caps");
    }

    #[test]
    fn a_lowered_proper_noun_is_one_repair_away() {
        // the acknowledged failure mode, and its escape hatch
        let mut b = BufferSink::new("we flew to", 10).with_mode(Mode::Prose);
        b.apply(&plan(&segment("Paris"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "we flew to paris");
        assert!(b.repair(&Repair::Capitalize));
        assert_eq!(b.text, "we flew to Paris");
    }

    #[test]
    fn code_mode_leaves_capitalization_alone() {
        let mut b = BufferSink::new("let x =", 7).with_mode(Mode::Code);
        b.apply(&plan(&segment("Vec new"), Mode::Code, Placement::Cursor));
        assert_eq!(b.text, "let x = Vec new");
    }

    #[test]
    fn mode_switches_mid_session() {
        let mut b = BufferSink::new("", 0).with_mode(Mode::Prose);
        b.apply(&plan(&segment("this is a comment"), b.mode, Placement::Cursor));
        b.apply(&plan(&segment("code mode"), b.mode, Placement::Cursor));
        assert_eq!(b.mode, Mode::Code);
        b.apply(&plan(&segment("let x equals one semicolon"), b.mode, Placement::Cursor));
        assert_eq!(b.text, "this is a comment let x = 1;");
    }

    #[test]
    fn mode_control_phrases_parse() {
        assert_eq!(segment("code mode"), vec![Piece::Control(Ctl::SetMode(Mode::Code))]);
        assert_eq!(segment("prose mode"), vec![Piece::Control(Ctl::SetMode(Mode::Prose))]);
        assert_eq!(segment_strict("switch to code"), vec![Piece::Control(Ctl::SetMode(Mode::Code))]);
    }

    #[test]
    fn segmentation_preserves_case_for_content() {
        // whisper's capitalization is the only signal separating a
        // proper noun from a homophone; the matcher may lowercase, the
        // content may not
        assert_eq!(segment("we flew to Paris"), vec![Piece::Text("we flew to Paris".into())]);
        assert_eq!(segment_strict("HTTP and TLS"), vec![Piece::Text("HTTP and TLS".into())]);
        // ...while controls still match regardless of how whisper cased them
        assert_eq!(segment("New Line"), vec![Piece::Control(Ctl::NewLine)]);
        assert_eq!(segment_strict("Scratch that."), vec![Piece::Control(Ctl::ScratchThat)]);
    }

    #[test]
    fn code_mode_symbols_match_case_insensitively() {
        assert_eq!(spoken_code("Open Paren x Close Paren", Mode::Code), "(x)");
        assert_eq!(spoken_code("timeout equals Thirty", Mode::Code), "timeout = 30");
        // and a literal capitalized identifier survives
        assert_eq!(spoken_code("Vec new", Mode::Code), "Vec new");
    }

    #[test]
    fn prose_keeps_whispers_sentence_punctuation() {
        let mut b = BufferSink::new("", 0).with_mode(Mode::Prose);
        for u in ["We flew to Paris.", "It rained the whole time."] {
            b.apply(&plan(&segment(u), Mode::Prose, Placement::Cursor));
        }
        // the period ends the sentence, so "It" keeps its capital
        assert_eq!(b.text, "We flew to Paris. It rained the whole time.");
    }

    #[test]
    fn code_mode_drops_whispers_sentence_punctuation() {
        // "let x equals five." must not end with a stray period
        assert_eq!(spoken_code("let x equals five.", Mode::Code), "let x = 5");
    }

    #[test]
    fn quotes_pair_by_parity() {
        assert_eq!(spoken_code("quote hello quote", Mode::Code), "\"hello\"");
        assert_eq!(
            spoken_code("let s equals quote Paris quote semicolon", Mode::Code),
            "let s = \"Paris\";"
        );
        // two pairs on one line
        assert_eq!(spoken_code("quote a quote quote b quote", Mode::Code), "\"a\" \"b\"");
    }

    #[test]
    fn a_cursor_that_is_not_a_char_boundary_is_snapped_not_trusted() {
        // 2 lands inside the 'é'. A caller counting characters, or
        // reusing a stale offset, hits this; panicking is a crash.
        let mut b = BufferSink::new("héllo", 2);
        assert_eq!(b.cursor, 1);
        b.apply(&plan(&segment("x"), Mode::Prose, Placement::Cursor));
        assert_eq!(b.text, "h x\u{e9}llo", "inserted at the snapped boundary, with the usual spacer");
    }

    #[test]
    fn floor_char_boundary_behaves() {
        let s = "aé日";
        assert_eq!(floor_char_boundary(s, 0), 0);
        assert_eq!(floor_char_boundary(s, 1), 1);
        assert_eq!(floor_char_boundary(s, 2), 1, "inside é");
        assert_eq!(floor_char_boundary(s, 3), 3);
        assert_eq!(floor_char_boundary(s, 5), 3, "inside 日");
        assert_eq!(floor_char_boundary(s, 999), s.len());
        assert_eq!(floor_char_boundary("", 5), 0);
    }

    #[test]
    fn range_placement_snaps_too() {
        let mut b = BufferSink::new("日本語", 0);
        b.apply(&[Edit::Insert { at: Placement::Range(2, 2), text: "x".into() }]);
        assert!(b.text.starts_with('x'), "{}", b.text);
    }

    #[test]
    fn multibyte_text_survives_the_whole_pipeline() {
        let mut b = BufferSink::new("", 0).with_mode(Mode::Prose);
        for u in ["Le café était", "à côté du jardin"] {
            b.apply(&plan(&segment(u), Mode::Prose, Placement::Cursor));
        }
        assert_eq!(b.text, "Le café était à côté du jardin");
        assert!(b.repair(&Repair::Uppercase));
        assert!(b.text.ends_with("À CÔTÉ DU JARDIN"), "{}", b.text);
        b.apply(&[Edit::RetractLast]);
        assert_eq!(b.text, "Le café était");
        assert!(b.text.is_char_boundary(b.cursor));
    }

    #[test]
    fn dedent_does_not_cut_a_multibyte_char() {
        let mut b = BufferSink::new("  é", "  é".len());
        b.apply(&[Edit::IndentDelta(-1)]);
        // only spaces are eaten; the é is untouched
        assert_eq!(b.text, "  é");
    }

    #[test]
    fn code_mode_drops_disfluencies() {
        assert_eq!(spoken_code("um let x equals five", Mode::Code), "let x = 5");
        assert_eq!(spoken_code("let x equals uh five", Mode::Code), "let x = 5");
        assert_eq!(spoken_code("let x you know equals five", Mode::Code), "let x = 5");
    }

    #[test]
    fn prose_keeps_disfluencies() {
        // deleting words from someone's prose is worse than leaving one in
        assert_eq!(spoken_code("Um, we tried three.", Mode::Prose), "Um, we tried three.");
    }

    #[test]
    fn whisper_homophones_resolve_to_their_symbols() {
        // measured against degraded transcripts, not invented
        assert_eq!(spoken_code("foo open parent a coma b close parent", Mode::Code), "foo(a, b)");
        assert_eq!(spoken_code("raw dote as str", Mode::Code), "raw.as str");
        assert_eq!(spoken_code("let x equal five semi colon", Mode::Code), "let x = 5;");
        assert_eq!(spoken_code("if x open braise", Mode::Code), "if x {");
        assert_eq!(spoken_code("let x column i thirty two", Mode::Code), "let x: i32");
        assert_eq!(spoken_code("std double column fs", Mode::Code), "std::fs");
    }

    #[test]
    fn a_dropped_word_is_left_visibly_wrong() {
        // "foo paren a close paren" is missing "open". Guessing at it
        // would mean guessing at every bare noun; a visible artifact the
        // speaker can repair is the better failure.
        assert_eq!(spoken_code("foo paren a close paren", Mode::Code), "foo paren a)");
    }

    #[test]
    fn a_leading_keyword_loses_whispers_capital() {
        assert_eq!(spoken_code("Let x equals five", Mode::Code), "let x = 5");
        assert_eq!(spoken_code("Match state open brace", Mode::Code), "match state {");
        assert_eq!(spoken_code("Return none", Mode::Code), "return none");
    }

    #[test]
    fn a_leading_type_name_keeps_its_capital() {
        // the case that stops this from being a general lowercase rule
        assert_eq!(spoken_code("Config open brace", Mode::Code), "Config {");
        assert_eq!(spoken_code("UserStore new", Mode::Code), "UserStore new");
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
        assert_eq!(b.text, "UserStore::new()\nmax_retries = 3;");
    }
}

//! Editor voice mode: when the focused PTY is running a modal editor
//! (helix/vim), translate spoken phrases into the editor's keystrokes
//! so a developer can edit files by voice — on the move, one-handed,
//! on a phone (the project's mobility thesis).
//!
//! Same shape and safety model as repl_voice: a pure (editor,
//! verb-table) translation, no IO, no state, fully unit-testable. The
//! caller detects the editor (reuse the existing is_editor_active /
//! term_foreground_command — NO new detection surface) and writes the
//! returned keystrokes to the PTY.
//!
//! Design constraints from the mobility thesis:
//!   - Favor the HIGH-VALUE edits you'd actually do by voice on the
//!     move (save, navigate, undo, search, line ops) over a complete
//!     keymap. A voice editor is not trying to replace the keyboard;
//!     it's trying to remove the friction of the common moves.
//!   - Tolerate speech-recognition noise: accept natural phrasings
//!     and synonyms ("save it" / "write" / "save"), strip filler.
//!   - SAFETY: edits that DESTROY or OVERWRITE need an explicit,
//!     unambiguous verb. We never auto-issue a quit-without-saving or
//!     a global replace from a fuzzy phrase. Anything carrying a
//!     user-supplied argument (a filename, a search term) is
//!     sanitized before it can reach the editor's command line.

/// The modal editors we translate for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Editor {
    Helix,
    Vim,
}

impl Editor {
    pub fn label(&self) -> &'static str {
        match self {
            Editor::Helix => "helix",
            Editor::Vim => "vim",
        }
    }
}

/// v0.1.484: self-surfacing — the man-pages principle applied to
/// voice. At web2qml's feature count, a capability that doesn't
/// announce what it accepts recreates the Pulsar integration burden
/// (a real integration point the user must already KNOW about).
/// commands() returns the speakable phrases for this editor, grouped,
/// so the UI can show "what can I say right now?" the moment an
/// editor is focused — exactly how man-pages lists itself.
///
/// Each entry is (spoken phrase, what it does). Representative, not
/// exhaustive (synonyms collapsed to the canonical phrasing).
pub fn commands(editor: Editor) -> Vec<(&'static str, &'static str)> {
    // Shared file ops + navigation that both editors translate.
    let mut v: Vec<(&str, &str)> = vec![
        ("save", "write the file"),
        ("save and quit", "write and close"),
        ("quit", "close (warns if unsaved)"),
        ("down", "move/select down"),
        ("up", "move/select up"),
        ("next word", "next word"),
        ("back", "previous word"),
        ("go to line 42", "jump to a line number"),
        ("find foo", "search for text"),
        ("open file main.rs", "open a file by name"),
        ("undo", "undo last change"),
        ("redo", "redo"),
        ("delete line", "delete the current line"),
        ("paste", "paste"),
        ("top", "go to start of file"),
        ("bottom", "go to end of file"),
        ("insert", "insert / edit"),
        ("append", "append"),
        ("escape", "back to normal mode"),
    ];
    // Helix-specific selection-first verbs (the user's muscle memory).
    if editor == Editor::Helix {
        v.extend_from_slice(&[
            ("till semicolon", "select till a char (t)"),
            ("till back paren", "select back till a char (T)"),
            ("visual", "enter select mode (v)"),
            ("insert at line start", "insert at line start (I)"),
            ("append at line end", "append at line end (A)"),
        ]);
    }
    v
}

/// Map a foreground-command name (from term_foreground_command) to an
/// Editor. Returns None for non-modal or unknown editors — we only
/// translate for editors whose modal command vocabulary we model.
/// (nano/micro/emacs are NOT modal in the same way; voice editing
/// there would need a different grammar, out of scope here.)
pub fn detect_editor(foreground_cmd: &str) -> Option<Editor> {
    match foreground_cmd.trim() {
        "hx" | "helix" => Some(Editor::Helix),
        "vim" | "nvim" | "vi" => Some(Editor::Vim),
        _ => None,
    }
}

/// A translated editor action: the keystrokes to send, plus whether
/// it is "destructive/irreversible enough" that the caller may want
/// to confirm first (e.g. quit-without-save). Kept simple: the
/// keystrokes string is what goes to the PTY; `confirm` is advisory.
#[derive(Debug, Clone, PartialEq)]
pub struct EditorAction {
    /// Exact bytes to write to the PTY (already includes any needed
    /// `<ret>` as a literal "\r" / ":" prefix etc.).
    pub keys: String,
    /// True if the caller should consider confirming before sending
    /// (currently only quit-without-saving). Advisory.
    pub confirm: bool,
}

impl EditorAction {
    fn keys(s: &str) -> Option<EditorAction> {
        Some(EditorAction { keys: s.to_string(), confirm: false })
    }
    fn keys_confirm(s: &str) -> Option<EditorAction> {
        Some(EditorAction { keys: s.to_string(), confirm: true })
    }
}

/// Translate a spoken phrase into an EditorAction for the editor.
/// None if the phrase doesn't map to a known command.
pub fn translate(editor: Editor, phrase: &str) -> Option<EditorAction> {
    let p = strip_filler(phrase.trim().to_lowercase());
    if p.is_empty() { return None; }
    match editor {
        Editor::Helix => translate_helix(&p),
        Editor::Vim => translate_vim(&p),
    }
}

fn strip_filler(p: String) -> String {
    let mut s = p;
    // NOTE: do NOT strip "go " — it collides with legitimate
    // commands like "go to line N" / "go to top". Filler is
    // politeness words and verbal disfluencies only.
    //
    // v0.1.583: added the verbal disfluencies whisper actually
    // transcribes ("um", "uh", "er", "hmm") — these are the MOST common
    // leading noise in real voice and previously broke exact-match
    // commands like the helix pickers ("um show project files" missed).
    // Also LOOP until stable so stacked fillers ("um please save") fully
    // strip, not just the first. Each entry keeps its trailing space so
    // we never eat a real word (e.g. "um " won't touch "unmount").
    let leads = [
        "please ", "now ", "okay ", "ok ", "can you ", "let's ", "lets ",
        "um ", "uh ", "er ", "hmm ", "uhh ", "umm ",
    ];
    loop {
        let mut changed = false;
        for lead in leads {
            if let Some(rest) = s.strip_prefix(lead) {
                s = rest.to_string();
                changed = true;
            }
        }
        if !changed {
            break;
        }
    }
    s.trim().to_string()
}

// Helix command line uses `:` ; normal-mode motions are bare keys.
// We send `<ret>` as "\r".
fn translate_helix(p: &str) -> Option<EditorAction> {
    // File ops (command mode).
    match p {
        "save" | "save it" | "write" | "write file" =>
            return EditorAction::keys(":w\r"),
        "save and quit" | "save and close" | "write and quit" =>
            return EditorAction::keys(":wq\r"),
        // quit WITHOUT saving is destructive — flag for confirm.
        "quit without saving" | "force quit" | "discard and quit" =>
            return EditorAction::keys_confirm(":q!\r"),
        "quit" | "close" | "close file" =>
            return EditorAction::keys(":q\r"),
        // v0.1.576 (user request): helix space-menu pickers. <space> is
        // helix's leader; these open its built-in fuzzy pickers. Placed
        // in this TOP exact-match block so they're checked BEFORE the
        // `find `/`search ` prefix handlers below — otherwise "find
        // files" would be caught as a /-search for "files" instead of
        // opening the file picker.
        "show project files" | "show files" | "file picker"
        | "open file picker" | "find file" | "find files"
        | "fuzzy find" | "fuzzy find files" =>
            return EditorAction::keys(" f"),       // <space>f
        "search project" | "search in project" | "global search"
        | "search everywhere" | "grep project" =>
            return EditorAction::keys(" /"),       // <space>/ (global search)
        "show buffers" | "list buffers" | "open buffers"
        | "switch buffer" | "buffer picker" =>
            return EditorAction::keys(" b"),       // <space>b
        "show symbols" | "symbol picker" | "list symbols"
        | "jump to symbol" =>
            return EditorAction::keys(" s"),       // <space>s
        "command palette" | "show commands" | "open palette" =>
            return EditorAction::keys(" ?"),       // <space>? (command palette)
        "file explorer" | "show explorer" | "open explorer" =>
            return EditorAction::keys(" e"),       // <space>e
        _ => {}
    }
    // Common normal-mode motions / edits — bare keys.
    if let Some(k) = helix_motion(p) {
        return EditorAction::keys(k);
    }
    // v0.1.594 (user idea: "rollback words for undo actions"). Two
    // flavors:
    //   1. Helix's NATIVE jumplist — "jump back"/"go back" → Ctrl-o,
    //      "jump forward" → Ctrl-i. This is helix's own "where was I"
    //      and is the most reliable rollback (it restores real file
    //      positions, not screen guesses).
    //   2. INVERSE-motion replay — "rollback <motion>" sends the
    //      opposite key (j↔k, w↔b, top↔bottom, line-start↔line-end,
    //      next↔previous). Approximate: motions like f<char> have no
    //      exact inverse and are intentionally NOT inverted here (we'd
    //      have to know the jump distance). The true position-history
    //      stack ("what line/char we were at") needs status-line
    //      parsing + the running build — see TODO_motion_rollback.md.
    match p {
        "jump back" | "go back" | "back in history" | "previous location"
            => return EditorAction::keys("\u{0f}"),   // Ctrl-o
        "jump forward" | "go forward" | "forward in history" | "next location"
            => return EditorAction::keys("\u{09}"),   // Ctrl-i (== Tab)
        _ => {}
    }
    if let Some(rest) = after_any(p, &["rollback ", "undo movement ", "undo motion ",
                                        "reverse ", "go back from "]) {
        if let Some(inv) = helix_inverse_motion(rest.trim()) {
            return EditorAction::keys(inv);
        }
    }
    // Parametric: "go to line N" → "<N>G" (helix: `:N<ret>` also works;
    // use the goto form which is unambiguous).
    if let Some(rest) = after_any(p, &["go to line ", "goto line ", "line ", "jump to line "]) {
        let n = first_int(rest)?;
        return EditorAction::keys(&format!(":{}\r", n));
    }
    // v0.1.487 (user muscle-memory): "till <char>" → helix `t<char>`
    // (extend selection up to, but not including, the next <char>).
    // The char is spoken by name ("till semicolon") since punctuation
    // doesn't transcribe reliably. Also "till back <char>" → `T<char>`.
    if let Some(rest) = after_any(p, &["till back ", "until back "]) {
        let ch = spoken_char(rest)?;
        return EditorAction::keys(&format!("T{}", ch));
    }
    if let Some(rest) = after_any(p, &["till ", "until ", "to "]) {
        let ch = spoken_char(rest)?;
        return EditorAction::keys(&format!("t{}", ch));
    }
    // v0.1.599: helix MATCH MODE — one of helix's signature features.
    //   mi( select inside parens     ma( select around (incl brackets)
    //   miw inside word              mip inside paragraph
    //   mi" inside quotes            mm  jump to matching bracket
    // Voice: "select inside parens", "select around braces", "select
    // inside word/paragraph/function", "matching bracket". Text-object
    // targets (word/paragraph/function/type) map to helix's letter
    // codes; bracket/quote targets go through spoken_char. Placed before
    // find-char so "select inside ..." isn't mis-routed.
    if p == "matching bracket" || p == "match bracket" || p == "jump to match"
        || p == "go to matching bracket" {
        return EditorAction::keys("mm");
    }
    for (prefixes, mode) in [
        (&["select inside ", "match inside ", "inside "][..], 'i'),
        (&["select around ", "match around ", "around "][..], 'a'),
    ] {
        if let Some(rest) = after_any(p, prefixes) {
            let rest = rest.trim().trim_start_matches("the ").trim();
            // named text objects first
            let obj = match rest {
                "word" => Some('w'),
                "paragraph" => Some('p'),
                "function" | "function body" => Some('f'),
                "type" | "type definition" => Some('t'),
                "argument" | "parameter" => Some('a'),
                "comment" => Some('c'),
                _ => None,
            };
            if let Some(o) = obj {
                return EditorAction::keys(&format!("m{}{}", mode, o));
            }
            // bracket/quote target — normalize natural plurals/names that
            // spoken_char (singular-only) misses ("parens", "braces").
            let bracket = match rest {
                "parens" | "paren" | "parentheses" | "parenthesis"
                | "round brackets" => Some('('),
                "braces" | "brace" | "curly braces" | "curly brace"
                | "curlies" => Some('{'),
                "brackets" | "bracket" | "square brackets"
                | "square bracket" => Some('['),
                "angle brackets" | "angles" => Some('<'),
                "quotes" | "quote" | "double quotes" | "double quote" => Some('"'),
                "single quotes" | "single quote" | "ticks" => Some('\''),
                "backticks" | "backtick" => Some('`'),
                _ => None,
            };
            if let Some(ch) = bracket {
                return EditorAction::keys(&format!("m{}{}", mode, ch));
            }
            // otherwise try the general char vocab
            if let Some(ch) = spoken_char(rest) {
                return EditorAction::keys(&format!("m{}{}", mode, ch));
            }
            return None; // "select inside <unknown>" → no guess
        }
    }
    // v0.1.593 (user reached for this on dogfood): helix `f<char>`
    // (find next <char> in line) and `F<char>` (find previous). Distinct
    // from `/` search, which is for a WORD/phrase. The char is spoken by
    // name ("find character semicolon", "jump to next paren") since
    // punctuation doesn't transcribe. Placed BEFORE the general find/
    // search below so a single named char goes to `f`, while "find <word>"
    // still falls through to `/word`.
    if let Some(rest) = after_any(p, &["find back character ", "find previous character ",
                                        "find back char ", "jump back to "]) {
        let ch = spoken_char(rest)?;
        return EditorAction::keys(&format!("F{}", ch));
    }
    if let Some(rest) = after_any(p, &["find character ", "find char ",
                                        "jump to next ", "jump to character ",
                                        "find next character "]) {
        let ch = spoken_char(rest)?;
        return EditorAction::keys(&format!("f{}", ch));
    }
    // Search: "find X" / "search for X" → "/X<ret>". The term is
    // sanitized so a noisy transcript can't inject a command.
    if let Some(rest) = after_any(p, &["find ", "search for ", "search "]) {
        let term = sanitize_search(rest)?;
        return EditorAction::keys(&format!("/{}\r", term));
    }
    // Open a file by name: ":open X". Path sanitized. (Exact picker
    // phrases like "open file picker" are caught by the top match block
    // before this prefix runs.)
    if let Some(rest) = after_any(p, &["open file ", "open ", "edit file "]) {
        let path = sanitize_path(rest)?;
        return EditorAction::keys(&format!(":open {}\r", path));
    }
    None
}

/// Map a spoken character NAME to its literal char, for parametric
/// motions like "till <char>". Voice can't reliably transcribe raw
/// punctuation, so the user speaks the name. Single printable
/// characters are also accepted verbatim (e.g. "till a"). Returns
/// None if the name isn't recognized.
fn spoken_char(s: &str) -> Option<char> {
    let t = s.trim();
    // A single literal char spoken directly ("till a", "till 5").
    if t.chars().count() == 1 {
        let c = t.chars().next()?;
        if c.is_ascii_graphic() { return Some(c); }
    }
    Some(match t {
        "semicolon" => ';',
        "colon" => ':',
        "comma" => ',',
        "period" | "dot" | "full stop" => '.',
        "open paren" | "open parenthesis" | "left paren" | "paren" => '(',
        "close paren" | "closing paren" | "right paren" | "end paren" => ')',
        "open brace" | "left brace" | "brace" => '{',
        "close brace" | "closing brace" | "right brace" => '}',
        "open bracket" | "left bracket" | "bracket" => '[',
        "close bracket" | "closing bracket" | "right bracket" => ']',
        "quote" | "double quote" => '"',
        "single quote" | "apostrophe" | "tick" => '\'',
        "backtick" | "back tick" => '`',
        "equals" | "equal sign" => '=',
        "space" => ' ',
        "underscore" => '_',
        "dash" | "hyphen" | "minus" => '-',
        "slash" | "forward slash" => '/',
        "backslash" => '\\',
        "pipe" | "bar" => '|',
        "less than" | "open angle" | "left angle" => '<',
        "greater than" | "close angle" | "right angle" => '>',
        "star" | "asterisk" => '*',
        "at" | "at sign" => '@',
        "hash" | "pound" | "number sign" => '#',
        "dollar" | "dollar sign" => '$',
        "ampersand" | "and sign" => '&',
        _ => return None,
    })
}

/// v0.1.594: inverse of a spoken motion, for "rollback <motion>".
/// Only motions with a CLEAN inverse are listed — directional line/word
/// motions, file top/bottom, line ends, and match next/prev. Motions
/// without an exact inverse (f<char>, search, go-to-line) return None so
/// "rollback" of them is a no-op rather than a wrong guess.
fn helix_inverse_motion(spoken: &str) -> Option<&'static str> {
    Some(match spoken {
        "down" | "line down" | "next line" => "k",
        "up" | "line up" | "previous line" => "j",
        "next word" | "word forward" | "word" => "b",
        "previous word" | "back word" | "word back" | "back" => "w",
        "top" | "go to top" | "beginning of file" => "ge",
        "bottom" | "go to bottom" | "end of file" => "gg",
        "start of line" | "line start" => "gl",
        "end of line" | "line end" => "gh",
        "next" | "next match" => "N",
        "previous" | "previous match" => "n",
        _ => return None,
    })
}

fn helix_motion(p: &str) -> Option<&'static str> {
    Some(match p {
        "undo" => "u",
        "redo" => "U",
        // v0.1.487 (user muscle-memory): core line/word motions.
        // In helix these SELECT as they move (selection-first model),
        // which is exactly why the v/i/a edit flow below works on them.
        "down" | "line down" | "next line" => "j",
        "up" | "line up" | "previous line" => "k",
        "next word" | "word forward" | "word" => "w",
        "previous word" | "back word" | "word back" | "back" => "b",
        "delete line" | "delete this line" => "xd",   // select line then delete
        "delete word" => "wd",
        "copy line" | "yank line" => "xy",
        "paste" => "p",
        "paste before" => "P",
        "top" | "go to top" | "beginning of file" => "gg",
        "bottom" | "go to bottom" | "end of file" => "ge",
        "start of line" | "line start" => "gh",
        "end of line" | "line end" => "gl",
        "next" | "next match" => "n",
        "previous" | "previous match" | "previous match back" => "N",
        "select line" => "x",
        // v0.1.487 (user muscle-memory): visual/select mode + the
        // follow-up edit verbs that operate on a selection in helix.
        // "visual" / "select"  → `v` (toggle select mode); the user
        // then issues one of the verbs below by voice.
        "visual" | "visual mode" | "select mode" | "select" => "v",
        // helix selection-edit verbs (what you do AFTER v on a sel):
        //   i = insert before selection      a = append after selection
        //   I = insert at line start         A = insert at line end
        // (helix's I/A are line-anchored insert points.)
        // v0.1.594: widened synonyms — the user reaches for "prepend",
        // "insert at start", "append to line/end", etc. naturally.
        "insert" | "insert mode" | "insert before" | "go to insert" => "i",
        "append" | "append after" | "append here" => "a",
        "insert at line start" | "line start insert" | "big insert"
        | "insert at start" | "insert at the start" | "insert at beginning"
        | "prepend" | "prepend to line" | "prepend line" => "I",
        "append at line end" | "line end append" | "big append"
        | "append at end" | "append at the end" | "append to line"
        | "append to end" | "add to end of line" | "insert at end" => "A",
        "escape" | "normal mode" | "cancel" | "exit" => "\u{1b}", // ESC
        "open line below" | "new line below" | "open below" => "o",
        "open line above" | "new line above" | "open above" => "O",
        _ => return None,
    })
}

// vim: ex commands via `:`, motions bare.
fn translate_vim(p: &str) -> Option<EditorAction> {
    match p {
        "save" | "save it" | "write" | "write file" =>
            return EditorAction::keys(":w\r"),
        "save and quit" | "save and close" | "write and quit" =>
            return EditorAction::keys(":wq\r"),
        "quit without saving" | "force quit" | "discard and quit" =>
            return EditorAction::keys_confirm(":q!\r"),
        "quit" | "close" | "close file" =>
            return EditorAction::keys(":q\r"),
        _ => {}
    }
    if let Some(k) = vim_motion(p) {
        return EditorAction::keys(k);
    }
    if let Some(rest) = after_any(p, &["go to line ", "goto line ", "line ", "jump to line "]) {
        let n = first_int(rest)?;
        return EditorAction::keys(&format!(":{}\r", n));
    }
    if let Some(rest) = after_any(p, &["find ", "search for ", "search "]) {
        let term = sanitize_search(rest)?;
        return EditorAction::keys(&format!("/{}\r", term));
    }
    if let Some(rest) = after_any(p, &["open file ", "open ", "edit file "]) {
        let path = sanitize_path(rest)?;
        return EditorAction::keys(&format!(":e {}\r", path));
    }
    None
}

fn vim_motion(p: &str) -> Option<&'static str> {
    Some(match p {
        "undo" => "u",
        "redo" => "\u{12}", // Ctrl-R
        "down" | "line down" | "next line" => "j",
        "up" | "line up" | "previous line" => "k",
        "next word" | "word forward" | "word" => "w",
        "previous word" | "back word" | "word back" | "back" => "b",
        "delete line" | "delete this line" => "dd",
        "delete word" => "dw",
        "copy line" | "yank line" => "yy",
        "paste" => "p",
        "paste before" => "P",
        "top" | "go to top" | "beginning of file" => "gg",
        "bottom" | "go to bottom" | "end of file" => "G",
        "start of line" | "line start" => "0",
        "end of line" | "line end" => "$",
        "next" | "next match" => "n",
        "previous" | "previous match" => "N",
        "escape" | "normal mode" | "cancel" => "\u{1b}",
        "insert" | "insert mode" => "i",
        "append" => "a",
        "open line below" | "new line below" => "o",
        "open line above" | "new line above" => "O",
        _ => return None,
    })
}

fn after_any<'a>(p: &'a str, prefixes: &[&str]) -> Option<&'a str> {
    for pre in prefixes {
        if let Some(rest) = p.strip_prefix(pre) {
            return Some(rest.trim());
        }
    }
    None
}

fn first_int(s: &str) -> Option<u32> {
    let mut digits = String::new();
    for c in s.chars() {
        if c.is_ascii_digit() { digits.push(c); }
        else if !digits.is_empty() { break; }
    }
    digits.parse().ok()
}

/// Sanitize a search term: it gets typed after `/` then `<ret>`, so
/// it must not contain control chars or a carriage return that would
/// prematurely submit or inject. Allow normal printable text + common
/// code chars; reject empties and anything with control bytes.
fn sanitize_search(s: &str) -> Option<String> {
    let t = s.trim();
    if t.is_empty() { return None; }
    // No control chars (incl. \r / \n / ESC) — those would alter the
    // command stream. Printable ASCII + space is plenty for a search.
    if t.chars().any(|c| (c as u32) < 0x20) { return None; }
    Some(t.to_string())
}

/// Sanitize a file path/name for `:open`/`:e`. First whitespace token
/// only (paths spoken by voice are single tokens); reject control
/// chars, shell metacharacters, and anything empty.
fn sanitize_path(s: &str) -> Option<String> {
    let tok = s.split_whitespace().next()?;
    if tok.is_empty() { return None; }
    if tok.chars().any(|c| (c as u32) < 0x20) { return None; }
    // Reject shell/command metacharacters that could be dangerous if
    // the editor passes the name to a shell (e.g. :!) — conservative.
    let bad = [';', '|', '&', '$', '`', '<', '>', '(', ')', '"', '\'', '*', '?'];
    if tok.chars().any(|c| bad.contains(&c)) { return None; }
    Some(tok.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn keys(editor: Editor, phrase: &str) -> Option<String> {
        translate(editor, phrase).map(|a| a.keys)
    }

    #[test]
    fn detect_editor_kinds() {
        assert_eq!(detect_editor("hx"), Some(Editor::Helix));
        assert_eq!(detect_editor("helix"), Some(Editor::Helix));
        assert_eq!(detect_editor("vim"), Some(Editor::Vim));
        assert_eq!(detect_editor("nvim"), Some(Editor::Vim));
        // non-modal / unknown → None (out of scope)
        assert_eq!(detect_editor("nano"), None);
        assert_eq!(detect_editor("bash"), None);
        assert_eq!(detect_editor(""), None);
    }

    #[test]
    fn helix_file_ops() {
        assert_eq!(keys(Editor::Helix, "save"), Some(":w\r".to_string()));
        assert_eq!(keys(Editor::Helix, "save it"), Some(":w\r".to_string()));
        assert_eq!(keys(Editor::Helix, "save and quit"), Some(":wq\r".to_string()));
        assert_eq!(keys(Editor::Helix, "quit"), Some(":q\r".to_string()));
    }

    #[test]
    fn quit_without_saving_flags_confirm() {
        let a = translate(Editor::Helix, "quit without saving").unwrap();
        assert_eq!(a.keys, ":q!\r");
        assert!(a.confirm, "destructive quit must flag confirm");
        // a plain quit does NOT flag confirm
        let b = translate(Editor::Helix, "quit").unwrap();
        assert!(!b.confirm);
    }

    #[test]
    fn helix_motions() {
        assert_eq!(keys(Editor::Helix, "undo"), Some("u".to_string()));
        assert_eq!(keys(Editor::Helix, "next word"), Some("w".to_string()));
        assert_eq!(keys(Editor::Helix, "delete line"), Some("xd".to_string()));
        assert_eq!(keys(Editor::Helix, "top"), Some("gg".to_string()));
        assert_eq!(keys(Editor::Helix, "escape"), Some("\u{1b}".to_string()));
    }

    #[test]
    fn filler_tolerance() {
        assert_eq!(keys(Editor::Helix, "please save"), Some(":w\r".to_string()));
        // v0.1.583: verbal disfluencies whisper transcribes must strip
        // so exact-match commands (the pickers) survive real voice.
        assert_eq!(keys(Editor::Helix, "um show project files"),
            Some(" f".to_string()));
        assert_eq!(keys(Editor::Helix, "uh save"), Some(":w\r".to_string()));
        // stacked fillers fully strip (loop-until-stable)
        assert_eq!(keys(Editor::Helix, "um please save"), Some(":w\r".to_string()));
        // but a real word starting with "um" is NOT eaten (trailing space)
        assert_eq!(keys(Editor::Helix, "umount"), None,
            "'umount' must not be mangled by the 'um ' filter");
        assert_eq!(keys(Editor::Helix, "now undo"), Some("u".to_string()));
    }

    #[test]
    fn go_to_line() {
        assert_eq!(keys(Editor::Helix, "go to line 42"), Some(":42\r".to_string()));
        assert_eq!(keys(Editor::Vim, "line 7"), Some(":7\r".to_string()));
    }

    #[test]
    fn search() {
        assert_eq!(keys(Editor::Helix, "find foo"), Some("/foo\r".to_string()));
        assert_eq!(keys(Editor::Vim, "search for bar"), Some("/bar\r".to_string()));
        // multi-word search term is preserved
        assert_eq!(keys(Editor::Helix, "find hello world"),
            Some("/hello world\r".to_string()));
    }

    #[test]
    fn helix_match_mode() {
        // v0.1.599: mi/ma select-inside/around + matching bracket.
        assert_eq!(keys(Editor::Helix, "select inside parens"), Some("mi(".to_string()));
        assert_eq!(keys(Editor::Helix, "select around braces"), Some("ma{".to_string()));
        assert_eq!(keys(Editor::Helix, "select inside word"), Some("miw".to_string()));
        assert_eq!(keys(Editor::Helix, "select inside paragraph"), Some("mip".to_string()));
        assert_eq!(keys(Editor::Helix, "select inside function"), Some("mif".to_string()));
        assert_eq!(keys(Editor::Helix, "select inside double quote"), Some("mi\"".to_string()));
        assert_eq!(keys(Editor::Helix, "matching bracket"), Some("mm".to_string()));
        // "the" filler tolerated
        assert_eq!(keys(Editor::Helix, "select inside the brace"), Some("mi{".to_string()));
        // unknown target → no wrong guess
        assert_eq!(keys(Editor::Helix, "select inside galaxy"), None);
    }

    #[test]
    fn helix_find_char_motions() {
        // v0.1.593: f<char> / F<char> find-char-in-line, distinct from
        // / search. Char spoken by name.
        assert_eq!(keys(Editor::Helix, "find character semicolon"),
            Some("f;".to_string()));
        assert_eq!(keys(Editor::Helix, "jump to next paren"),
            Some("f(".to_string()));
        assert_eq!(keys(Editor::Helix, "find back character semicolon"),
            Some("F;".to_string()));
        // a single spoken letter works too
        assert_eq!(keys(Editor::Helix, "find character x"),
            Some("fx".to_string()));
        // CRITICAL: "find <word>" must STILL be / search, not f — the
        // find-char paths only fire on the explicit "character"/"jump to
        // next" phrasings, so a plain word falls through.
        assert_eq!(keys(Editor::Helix, "find construction"),
            Some("/construction\r".to_string()),
            "plain word find must stay / search, not f");
    }

    #[test]
    fn helix_rollback_and_jumplist() {
        // v0.1.594: native jumplist
        assert_eq!(keys(Editor::Helix, "jump back"), Some("\u{0f}".to_string())); // Ctrl-o
        assert_eq!(keys(Editor::Helix, "jump forward"), Some("\u{09}".to_string())); // Ctrl-i
        // inverse-motion replay
        assert_eq!(keys(Editor::Helix, "rollback down"), Some("k".to_string()));
        assert_eq!(keys(Editor::Helix, "rollback up"), Some("j".to_string()));
        assert_eq!(keys(Editor::Helix, "rollback next word"), Some("b".to_string()));
        assert_eq!(keys(Editor::Helix, "rollback top"), Some("ge".to_string()));
        assert_eq!(keys(Editor::Helix, "reverse end of line"), Some("gh".to_string()));
        // a motion with no clean inverse → no-op (None), not a wrong guess
        assert_eq!(keys(Editor::Helix, "rollback find character x"), None,
            "f<char> has no exact inverse; rollback must not guess");
    }

    #[test]
    fn helix_insert_synonyms() {
        // v0.1.594: natural phrasings for insert-mode motions.
        assert_eq!(keys(Editor::Helix, "prepend"), Some("I".to_string()));
        assert_eq!(keys(Editor::Helix, "insert at start"), Some("I".to_string()));
        assert_eq!(keys(Editor::Helix, "append to line"), Some("A".to_string()));
        assert_eq!(keys(Editor::Helix, "insert at end"), Some("A".to_string()));
        assert_eq!(keys(Editor::Helix, "open below"), Some("o".to_string()));
        assert_eq!(keys(Editor::Helix, "open above"), Some("O".to_string()));
    }

    #[test]
    fn helix_space_pickers() {
        // v0.1.576: "show project files" → helix <space>f file picker.
        assert_eq!(keys(Editor::Helix, "show project files"),
            Some(" f".to_string()));
        assert_eq!(keys(Editor::Helix, "file picker"), Some(" f".to_string()));
        assert_eq!(keys(Editor::Helix, "global search"), Some(" /".to_string()));
        assert_eq!(keys(Editor::Helix, "show buffers"), Some(" b".to_string()));
        assert_eq!(keys(Editor::Helix, "symbol picker"), Some(" s".to_string()));
        assert_eq!(keys(Editor::Helix, "command palette"), Some(" ?".to_string()));
        // exact picker phrases must NOT be shadowed by the `find ` prefix:
        // "find files" is the picker, while "find foo" is a /-search.
        assert_eq!(keys(Editor::Helix, "find files"), Some(" f".to_string()));
        assert_eq!(keys(Editor::Helix, "find foo"), Some("/foo\r".to_string()));
    }

    #[test]
    fn open_file_sanitized() {
        assert_eq!(keys(Editor::Helix, "open file main.rs"),
            Some(":open main.rs\r".to_string()));
        assert_eq!(keys(Editor::Vim, "edit file lib.rs"),
            Some(":e lib.rs\r".to_string()));
        // a path with shell metacharacters is REJECTED
        assert_eq!(keys(Editor::Helix, "open file foo;rm"), None);
        assert_eq!(keys(Editor::Helix, "open file $(evil)"), None);
    }

    #[test]
    fn search_rejects_control_chars() {
        // a transcript can't realistically contain a raw \r, but guard
        // anyway: a term with a control char is refused.
        assert_eq!(sanitize_search("foo\rbar"), None);
        assert_eq!(sanitize_search("  "), None);
        assert_eq!(sanitize_search("normal text"), Some("normal text".to_string()));
    }

    #[test]
    fn vim_distinct_from_helix() {
        // delete-line differs: vim "dd", helix "xd"
        assert_eq!(keys(Editor::Vim, "delete line"), Some("dd".to_string()));
        assert_eq!(keys(Editor::Helix, "delete line"), Some("xd".to_string()));
        // redo differs: vim Ctrl-R, helix "U"
        assert_eq!(keys(Editor::Vim, "redo"), Some("\u{12}".to_string()));
        assert_eq!(keys(Editor::Helix, "redo"), Some("U".to_string()));
    }

    #[test]
    fn unknown_phrase_none() {
        assert_eq!(keys(Editor::Helix, "make me a sandwich"), None);
        assert_eq!(keys(Editor::Helix, ""), None);
    }

    #[test]
    fn commands_list_is_self_describing() {
        // Each editor's advertised list must translate FOR THAT
        // EDITOR (helix lists helix-specific verbs that vim lacks).
        for editor in [Editor::Helix, Editor::Vim] {
            let c = commands(editor);
            assert!(!c.is_empty());
            for (say, _does) in &c {
                assert!(translate(editor, say).is_some(),
                    "{:?} lists {:?} but it doesn't translate", editor, say);
            }
        }
    }

    // v0.1.487: the user's actual helix muscle-memory spec —
    // j,k,w,b,t{char},v(visual → i a A I t T ESC).
    #[test]
    fn helix_core_motions_jkwb() {
        assert_eq!(keys(Editor::Helix, "down"), Some("j".to_string()));
        assert_eq!(keys(Editor::Helix, "up"), Some("k".to_string()));
        assert_eq!(keys(Editor::Helix, "next word"), Some("w".to_string()));
        assert_eq!(keys(Editor::Helix, "word"), Some("w".to_string()));
        assert_eq!(keys(Editor::Helix, "back"), Some("b".to_string()));
        assert_eq!(keys(Editor::Helix, "previous word"), Some("b".to_string()));
    }

    #[test]
    fn helix_till_char_spoken() {
        assert_eq!(keys(Editor::Helix, "till semicolon"), Some("t;".to_string()));
        assert_eq!(keys(Editor::Helix, "till close paren"), Some("t)".to_string()));
        assert_eq!(keys(Editor::Helix, "till comma"), Some("t,".to_string()));
        assert_eq!(keys(Editor::Helix, "till a"), Some("ta".to_string()));
        assert_eq!(keys(Editor::Helix, "till back semicolon"), Some("T;".to_string()));
        // unrecognized char name → None (don't guess a keystroke)
        assert_eq!(keys(Editor::Helix, "till wibble"), None);
    }

    #[test]
    fn helix_visual_and_followups() {
        assert_eq!(keys(Editor::Helix, "visual"), Some("v".to_string()));
        assert_eq!(keys(Editor::Helix, "select"), Some("v".to_string()));
        assert_eq!(keys(Editor::Helix, "insert"), Some("i".to_string()));
        assert_eq!(keys(Editor::Helix, "append"), Some("a".to_string()));
        assert_eq!(keys(Editor::Helix, "insert at line start"), Some("I".to_string()));
        assert_eq!(keys(Editor::Helix, "append at line end"), Some("A".to_string()));
        assert_eq!(keys(Editor::Helix, "escape"), Some("\u{1b}".to_string()));
    }
}

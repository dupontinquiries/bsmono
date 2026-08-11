//! editor_keys — spoken phrases into modal-editor keystrokes, from
//! `editor_voice.rs`.
//!
//! The mobility thesis, stated plainly in the original: edit files by voice
//! *"on the move, one-handed, on a phone"*. And an equally plain limit —
//! *"a voice editor is not trying to replace the keyboard; it's trying to
//! remove the friction of the common moves."*
//!
//! That limit is the design. A complete keymap dictated aloud would be slower
//! than typing and wrong more often; the table covers save, navigate, undo,
//! search and line ops, and everything else stays a keyboard job.
//!
//! ## The safety rule, and why it is not paranoia
//!
//! *"Edits that DESTROY or OVERWRITE need an explicit, unambiguous verb. We
//! never auto-issue a quit-without-saving or a global replace from a fuzzy
//! phrase."*
//!
//! These bytes go straight to a PTY holding somebody's unsaved buffer. A
//! transcriber that hears "quit" inside "don't quit" and issues `:q!` has
//! destroyed work that cannot be recovered by undo — the editor is gone. So
//! destructive actions need their own phrase, and carry an advisory `confirm`
//! flag on top.
//!
//! ## Arguments are sanitised because they reach a command line
//!
//! A filename or search term goes after `:w ` or `/`, which is an editor's
//! **command line**. A term containing a newline ends the command and starts
//! another — the same class of bug as the audit log's escaping, in a place
//! where the second command runs.

/// The modal editors translated for.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Editor { Helix, Vim }

impl Editor {
    pub fn label(self) -> &'static str {
        match self { Editor::Helix => "helix", Editor::Vim => "vim" }
    }
}

/// Keystrokes to write, plus whether to ask first.
#[derive(Clone, PartialEq, Eq, Debug)]
pub struct EditorAction {
    /// Exact bytes for the PTY.
    pub keys: String,
    /// Advisory: the caller should confirm before sending. Set only for
    /// actions that lose work.
    pub confirm: bool,
}

impl EditorAction {
    fn keys(s: &str) -> Option<EditorAction> {
        Some(EditorAction { keys: s.into(), confirm: false })
    }
    fn confirming(s: &str) -> Option<EditorAction> {
        Some(EditorAction { keys: s.into(), confirm: true })
    }
}

/// Filler a transcriber adds that carries no instruction.
const FILLER: &[&str] = &[
    "please", "just", "can you", "could you", "now", "the", "a", "it",
];

/// Normalise a spoken phrase for lookup.
///
/// Filler is stripped as whole words, never as substrings: removing "it" from
/// "split" leaves "spl", and a table lookup on that misses silently.
pub fn normalise(phrase: &str) -> String {
    let mut lower = phrase.trim().trim_end_matches(['.', '?', '!']).to_lowercase();
    // MULTI-WORD filler first, and only at the start. "could you" is two words,
    // so a single-word filter leaves "could you undo that" — caught by a test.
    // Anchored to the start because "the file you saved" contains "you" as
    // content, and stripping it anywhere would eat the sentence.
    loop {
        let before = lower.len();
        for f in FILLER.iter().filter(|f| f.contains(' ')) {
            if let Some(rest) = lower.strip_prefix(*f) {
                lower = rest.trim_start().to_string();
                break;
            }
        }
        if lower.len() == before { break; }
    }
    let kept: Vec<&str> = lower
        .split_whitespace()
        .filter(|w| !FILLER.contains(w))
        .collect();
    kept.join(" ")
}

/// Strip anything that could end the editor's command line.
///
/// Newline and carriage return are the dangerous ones: after `:w ` a newline
/// submits the command and whatever follows becomes the NEXT one. Same class of
/// bug as the audit log's escaping, except here the second command runs.
pub fn sanitize_arg(arg: &str) -> String {
    arg.chars()
        .filter(|c| !matches!(c, '\n' | '\r' | '\x1b' | '\0'))
        .collect::<String>()
        .trim()
        .to_string()
}

/// The argument after a recognised prefix, sanitised.
///
/// Returns only the argument: my first version also returned the matched
/// prefix, which nothing used and which needed a `'static` bound the borrow
/// checker would not grant. A return value nobody reads is not worth a
/// lifetime.
/// Prepositions that are never an argument on their own.
///
/// "search for" with nothing after it strips to the bare "search " prefix and
/// leaves "for" — which would search the buffer for the word "for". Caught by a
/// test; the fix is to treat a lone preposition as no argument at all.
const NOT_AN_ARG: &[&str] = &["for", "to", "in", "at", "as", "with"];

fn split_arg(p: &str, prefixes: &[&str]) -> Option<String> {
    for pre in prefixes {
        if let Some(rest) = p.strip_prefix(pre) {
            let arg = sanitize_arg(rest);
            // An empty argument is not a command — "save as" alone would
            // otherwise write to an empty filename.
            if arg.is_empty() || NOT_AN_ARG.contains(&arg.as_str()) { return None; }
            return Some(arg);
        }
    }
    None
}

/// Case-preserving variant: matches the prefix case-insensitively but returns
/// the argument exactly as spoken.
fn split_arg_ci(raw: &str, prefixes: &[&str]) -> Option<String> {
    let lower = raw.to_lowercase();
    for pre in prefixes {
        if lower.starts_with(pre) {
            let arg = sanitize_arg(&raw[pre.len()..]);
            if arg.is_empty() || NOT_AN_ARG.contains(&arg.to_lowercase().as_str()) {
                return None;
            }
            return Some(arg);
        }
    }
    None
}

const SAVE_AS: &[&str] = &["save as ", "write to ", "save to "];
const SEARCH: &[&str] = &["search for ", "find ", "search "];

/// Translate a phrase for an editor.
///
/// `None` means "not a command I recognise" — which must stay distinguishable
/// from a command that does nothing, because the caller falls back to other
/// handlers on None and would swallow the utterance on an empty action.
pub fn translate(editor: Editor, phrase: &str) -> Option<EditorAction> {
    let p = normalise(phrase);
    if p.is_empty() { return None; }

    // Arguments first: "save as notes.txt" must not match the bare "save" and
    // write the current file under its old name.
    //
    // The VERB is matched on the normalised phrase, but the ARGUMENT is taken
    // from the original — normalise() lowercases, and my first version searched
    // for "todo" when the user said "TODO". Case matters in a search term and
    // it matters more in a filename, where Linux will happily create a second
    // file differing only in case.
    let raw = phrase.trim().trim_end_matches(['.', '?', '!']);
    if split_arg(&p, SAVE_AS).is_some() {
        if let Some(arg) = split_arg_ci(raw, SAVE_AS) {
            return EditorAction::keys(&format!(":w {arg}\r"));
        }
    }
    if split_arg(&p, SEARCH).is_some() {
        if let Some(arg) = split_arg_ci(raw, SEARCH) {
            return EditorAction::keys(&format!("/{arg}\r"));
        }
    }

    match p.as_str() {
        // --- save ---
        "save" | "save file" | "write" | "save this" => EditorAction::keys(":w\r"),
        "save and quit" | "save and close" | "write and quit" =>
            EditorAction::keys(":wq\r"),

        // --- quit ---
        // The plain one warns on unsaved changes by itself, so it is not
        // destructive and needs no confirmation.
        "quit" | "close" | "exit" => EditorAction::keys(":q\r"),
        // Destructive: its own phrase, and a confirm on top. A transcriber
        // hearing "quit" inside "don't quit" must never reach this.
        "quit without saving" | "force quit" | "discard and quit" =>
            EditorAction::confirming(":q!\r"),

        // --- undo / redo ---
        "undo" | "undo that" | "take that back" => EditorAction::keys("u"),
        "redo" | "do that again" => match editor {
            Editor::Helix => EditorAction::keys("U"),
            Editor::Vim => EditorAction::keys("\x12"), // ctrl-r
        },

        // --- navigation ---
        "top" | "go to top" | "beginning of file" => EditorAction::keys("gg"),
        "bottom" | "go to bottom" | "end of file" => match editor {
            Editor::Helix => EditorAction::keys("ge"),
            Editor::Vim => EditorAction::keys("G"),
        },
        "start of line" | "beginning of line" => EditorAction::keys("gh"),
        "end of line" => EditorAction::keys("gl"),
        "next" | "next match" => EditorAction::keys("n"),
        "previous" | "previous match" => EditorAction::keys("N"),

        // --- line ops ---
        "delete line" | "delete this line" => match editor {
            Editor::Helix => EditorAction::keys("xd"),
            Editor::Vim => EditorAction::keys("dd"),
        },
        "copy line" | "yank line" => match editor {
            Editor::Helix => EditorAction::keys("xy"),
            Editor::Vim => EditorAction::keys("yy"),
        },
        "paste" => EditorAction::keys("p"),
        "new line" | "open line" => EditorAction::keys("o"),
        "indent" => EditorAction::keys(">"),
        "unindent" | "outdent" => EditorAction::keys("<"),

        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_common_moves_translate_for_both_editors() {
        // The thesis: remove the friction of the moves you would actually make
        // one-handed on a phone, not replace the keyboard.
        for e in [Editor::Helix, Editor::Vim] {
            assert_eq!(translate(e, "save").unwrap().keys, ":w\r");
            assert_eq!(translate(e, "undo").unwrap().keys, "u");
            assert_eq!(translate(e, "top").unwrap().keys, "gg");
        }
    }

    #[test]
    fn the_editors_differ_where_their_keymaps_do() {
        // Helix is selection-first; a shared table that pretended otherwise
        // would send vim keys to helix and silently do the wrong edit.
        assert_eq!(translate(Editor::Vim, "delete line").unwrap().keys, "dd");
        assert_eq!(translate(Editor::Helix, "delete line").unwrap().keys, "xd");
        assert_eq!(translate(Editor::Vim, "redo").unwrap().keys, "\x12");
        assert_eq!(translate(Editor::Helix, "redo").unwrap().keys, "U");
    }

    #[test]
    fn a_destructive_quit_needs_its_own_phrase() {
        // These bytes go to a PTY holding an unsaved buffer. A transcriber
        // hearing "quit" inside "don't quit" that issued :q! has destroyed work
        // undo cannot recover — the editor is gone.
        assert_eq!(translate(Editor::Vim, "quit").unwrap().keys, ":q\r");
        assert!(!translate(Editor::Vim, "quit").unwrap().confirm,
                "plain quit warns by itself; confirming it teaches people to click through");
        let forced = translate(Editor::Vim, "force quit").unwrap();
        assert_eq!(forced.keys, ":q!\r");
        assert!(forced.confirm);
    }

    #[test]
    fn a_fuzzy_phrase_never_reaches_the_destructive_action() {
        // The stated rule: no quit-without-saving from a fuzzy phrase.
        for p in ["don't quit", "quit maybe", "i might quit"] {
            let got = translate(Editor::Vim, p);
            assert!(got.as_ref().map(|a| a.keys != ":q!\r").unwrap_or(true),
                    "{p} produced a forced quit");
        }
    }

    #[test]
    fn a_newline_in_an_argument_cannot_start_a_second_command() {
        // After `:w ` a newline SUBMITS, and whatever follows becomes the next
        // command. Same class as the audit log's escaping, except here the
        // second command runs.
        let a = translate(Editor::Vim, "save as notes.txt\r:q!").unwrap();
        assert!(!a.keys.contains(":q!"), "injected a second command: {:?}", a.keys);
        assert_eq!(a.keys.matches('\r').count(), 1, "more than one submit: {:?}", a.keys);
    }

    #[test]
    fn escape_and_nul_are_stripped_from_arguments() {
        // An escape sequence in a filename reaches the terminal, not just the
        // editor.
        assert_eq!(sanitize_arg("a\x1b[2Jb"), "a[2Jb");
        assert_eq!(sanitize_arg("a\0b"), "ab");
        assert_eq!(sanitize_arg("  spaced.txt  "), "spaced.txt");
    }

    #[test]
    fn an_argument_phrase_beats_the_bare_verb() {
        // "save as notes.txt" must not match "save" and write the current file
        // under its old name — silently the wrong file.
        let a = translate(Editor::Vim, "save as notes.txt").unwrap();
        assert_eq!(a.keys, ":w notes.txt\r");
        assert_eq!(translate(Editor::Vim, "save").unwrap().keys, ":w\r");
    }

    #[test]
    fn an_argument_verb_with_no_argument_is_not_a_command() {
        // "save as" with nothing after it would otherwise write to an empty
        // filename.
        assert!(translate(Editor::Vim, "save as").is_none());
        assert!(translate(Editor::Vim, "search for").is_none());
        assert!(translate(Editor::Vim, "save as   ").is_none());
    }

    #[test]
    fn a_search_term_is_passed_through_after_the_slash() {
        let a = translate(Editor::Helix, "search for TODO").unwrap();
        assert_eq!(a.keys, "/TODO\r");
    }

    #[test]
    fn filler_is_stripped_as_words_not_substrings() {
        // Removing "it" from "split" leaves "spl", and a table lookup on that
        // misses silently.
        assert_eq!(normalise("please save it"), "save");
        assert_eq!(normalise("could you just undo that"), "undo that");
        assert_eq!(normalise("split"), "split", "filler eaten out of a word");
    }

    #[test]
    fn transcriber_punctuation_does_not_defeat_the_lookup() {
        assert_eq!(translate(Editor::Vim, "Save.").unwrap().keys, ":w\r");
        assert_eq!(translate(Editor::Vim, "undo?").unwrap().keys, "u");
    }

    #[test]
    fn an_unrecognised_phrase_is_none_so_the_caller_can_fall_back() {
        // None must stay distinguishable from an action that does nothing: the
        // caller tries other handlers on None and would SWALLOW the utterance
        // on an empty action.
        assert!(translate(Editor::Vim, "make me a sandwich").is_none());
        assert!(translate(Editor::Vim, "").is_none());
        assert!(translate(Editor::Vim, "please just").is_none(), "all filler is nothing");
    }

    #[test]
    fn every_command_line_action_submits_exactly_once() {
        // A colon command that never submits leaves the editor sitting in its
        // command line, and the next spoken word is typed into it.
        for p in ["save", "save and quit", "quit", "force quit", "save as x.txt"] {
            let a = translate(Editor::Vim, p).unwrap();
            if a.keys.starts_with(':') {
                assert_eq!(a.keys.matches('\r').count(), 1, "{p}: {:?}", a.keys);
                assert!(a.keys.ends_with('\r'), "{p} does not submit");
            }
        }
    }

    #[test]
    fn only_work_losing_actions_ask_for_confirmation() {
        // Confirming everything trains people to dismiss the prompt, which
        // makes it useless on the one action that needed it.
        let confirming: Vec<&str> = ["save", "quit", "undo", "delete line", "force quit",
                                     "quit without saving", "discard and quit"]
            .into_iter()
            .filter(|p| translate(Editor::Vim, p).map(|a| a.confirm).unwrap_or(false))
            .collect();
        assert_eq!(confirming, vec!["force quit", "quit without saving", "discard and quit"]);
    }
}

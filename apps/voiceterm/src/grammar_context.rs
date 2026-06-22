//! Grammar-context labeling: derive WHICH voice grammar is active
//! from the terminal context, as a short label. Used to tag voice
//! recordings so the corpus self-labels — you can later query "show
//! my helix voice commands" or cross-reference an utterance against
//! the grammar that was live when it happened. (The kss/reVos
//! instinct: instrument the data so the human stays on content.)
//!
//! Pure composition over the existing detectors (editor_voice +
//! repl_voice) — no new detection logic, so the label always agrees
//! with what voice_parse_intent would actually route to.

use crate::editor_voice;
use crate::repl_voice;

/// Derive the active voice-grammar label from the foreground command
/// and the terminal's last non-empty line. Returns a short, stable
/// label suitable for tagging a recording and for later querying.
///
/// Precedence matches voice_parse_intent's dispatch order:
///   1. editor (helix/vim) — most specific foreground context
///   2. REPL (gdb/lldb/python/sqlite/redis) — bare prompt detected
///   3. otherwise the tab context: "terminal" or "web"
///
/// `on_terminal_tab` lets the caller distinguish shell-voice from
/// web-voice when no special grammar is active.
pub fn grammar_label(
    foreground_cmd: &str,
    last_nonempty_line: &str,
    on_terminal_tab: bool,
) -> String {
    if let Some(editor) = editor_voice::detect_editor(foreground_cmd) {
        return format!("editor:{}", editor.label());
    }
    if let Some(repl) = repl_voice::detect_repl(last_nonempty_line) {
        return format!("repl:{}", repl.label());
    }
    if on_terminal_tab {
        "shell".to_string()
    } else {
        "web".to_string()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn editor_takes_precedence() {
        // helix focused, even at a prompt-looking line → editor wins
        assert_eq!(grammar_label("hx", "(gdb)", true), "editor:helix");
        assert_eq!(grammar_label("vim", "", true), "editor:vim");
    }

    #[test]
    fn repl_when_no_editor() {
        assert_eq!(grammar_label("", "(gdb)", true), "repl:gdb");
        assert_eq!(grammar_label("bash", ">>> ", true), "repl:python");
        assert_eq!(grammar_label("", "127.0.0.1:6379>", true), "repl:redis");
    }

    #[test]
    fn shell_vs_web_fallback() {
        // ordinary shell prompt, on terminal tab → shell
        assert_eq!(grammar_label("bash", "user@host:~$", true), "shell");
        // same context but on the web tab → web
        assert_eq!(grammar_label("", "", false), "web");
    }

    #[test]
    fn label_agrees_with_dispatch_precedence() {
        // editor beats repl (matches voice_parse_intent order): a
        // shell that's NOT an editor but whose last line is a repl
        // prompt → repl; an editor → editor regardless of line.
        assert_eq!(grammar_label("nvim", "sqlite>", true), "editor:vim");
        assert_eq!(grammar_label("python3", ">>> ", true), "repl:python");
    }
}

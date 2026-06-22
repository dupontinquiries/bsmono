//! v0.1.686: filetype → command associations learned from shell history.
//!
//! User: "I was trying to `ffplay cymatics/tribal.mp3` ... the command never
//! caught on ... maybe we should make calls to zsh history to observe
//! filetype associations." Idea: scan history for commands that take a file
//! argument with a known extension (e.g. `ffplay song.mp3`, `mpv clip.mkv`,
//! `xdg-open doc.pdf`) and learn that `.mp3` is usually handled by `ffplay`.
//! Then a voiced "play tribal.mp3" can resolve to the right binary even when
//! the decision cache hasn't seen that exact phrase.
//!
//! PURE + testable: parse history lines → ext→ranked-commands map. The host
//! reads ~/.zsh_history (or $HISTFILE) and feeds lines here; entity
//! resolution consults the map. No code copied from anywhere.

use std::collections::HashMap;

/// A learned association: how often `command` was used to open files with a
/// given extension.
#[derive(Debug, Clone, PartialEq)]
pub struct Assoc {
    pub command: String,
    pub count: usize,
}

/// Map of lowercased extension (no dot) → commands ranked by frequency.
#[derive(Debug, Default, Clone)]
pub struct AssocTable {
    map: HashMap<String, Vec<Assoc>>,
}

impl AssocTable {
    /// Best command for an extension (most frequent), if any.
    pub fn best_for(&self, ext: &str) -> Option<&str> {
        self.map.get(&ext.to_lowercase())
            .and_then(|v| v.first())
            .map(|a| a.command.as_str())
    }
    /// All commands for an extension, ranked.
    pub fn for_ext(&self, ext: &str) -> &[Assoc] {
        self.map.get(&ext.to_lowercase()).map(|v| v.as_slice()).unwrap_or(&[])
    }
    pub fn is_empty(&self) -> bool { self.map.is_empty() }
    pub fn ext_count(&self) -> usize { self.map.len() }
}

/// zsh extended-history lines look like `: 1700000000:0;ffplay song.mp3`.
/// Strip that metadata prefix if present, returning the command text.
fn strip_zsh_meta(line: &str) -> &str {
    if let Some(rest) = line.strip_prefix(": ") {
        if let Some(semi) = rest.find(';') {
            // ensure the part before ';' looks like `<ts>:<dur>`
            let meta = &rest[..semi];
            if meta.split(':').all(|p| p.chars().all(|c| c.is_ascii_digit())) {
                return &rest[semi + 1..];
            }
        }
    }
    line
}

/// Extract the extension (lowercased, no dot) of a token if it looks like a
/// file with an extension. Ignores flags (-x), URLs, and pure numbers.
fn token_ext(tok: &str) -> Option<String> {
    if tok.starts_with('-') || tok.contains("://") { return None; }
    let base = tok.rsplit('/').next().unwrap_or(tok);
    let (stem, ext) = base.rsplit_once('.')?;
    if stem.is_empty() { return None; } // ".bashrc" — dotfile, not an ext
    // a plausible extension: 1-5 alphanumeric chars
    if (1..=5).contains(&ext.len()) && ext.chars().all(|c| c.is_ascii_alphanumeric()) {
        Some(ext.to_lowercase())
    } else {
        None
    }
}

/// The "command" of a line is its first bare word (skip leading env-var
/// assignments like `FOO=bar cmd` and `sudo`). Returns None for empties.
fn line_command(words: &[&str]) -> Option<usize> {
    let mut i = 0;
    while i < words.len() {
        let w = words[i];
        if w == "sudo" || w == "command" || w == "nohup" || w == "time" {
            i += 1; continue;
        }
        if w.contains('=') && !w.starts_with('=') {
            // env assignment prefix
            i += 1; continue;
        }
        return Some(i);
    }
    None
}

/// Build the association table from history lines. A line contributes
/// (command, ext) for each file-with-extension argument it carries, EXCEPT
/// when the command is a generic shell builtin we don't want to associate
/// (cd, ls, rm, cp, mv, cat, echo, etc.) — those don't imply a "handler".
pub fn build(history_lines: &[&str]) -> AssocTable {
    const SKIP_CMDS: &[&str] = &[
        "cd", "ls", "ll", "la", "rm", "cp", "mv", "cat", "echo", "touch",
        "mkdir", "rmdir", "chmod", "chown", "stat", "file", "wc", "head",
        "tail", "du", "df", "ln", "find", "which", "type", "test",
    ];
    let mut counts: HashMap<(String, String), usize> = HashMap::new();
    for raw in history_lines {
        let cmd_line = strip_zsh_meta(raw).trim();
        if cmd_line.is_empty() { continue; }
        // take only the first pipeline segment (before | ; &&)
        let seg = cmd_line.split(['|', ';']).next().unwrap_or(cmd_line);
        let seg = seg.split("&&").next().unwrap_or(seg).trim();
        let words: Vec<&str> = seg.split_whitespace().collect();
        let ci = match line_command(&words) { Some(i) => i, None => continue };
        let cmd = words[ci];
        // strip a path from the command (./foo -> foo, /usr/bin/mpv -> mpv)
        let cmd_base = cmd.rsplit('/').next().unwrap_or(cmd).to_string();
        if cmd_base.is_empty() || SKIP_CMDS.contains(&cmd_base.as_str()) { continue; }
        // collect extensions among the argument tokens
        let mut seen_exts: Vec<String> = Vec::new();
        for &arg in &words[ci + 1..] {
            if let Some(ext) = token_ext(arg) {
                if !seen_exts.contains(&ext) { seen_exts.push(ext); }
            }
        }
        for ext in seen_exts {
            *counts.entry((ext, cmd_base.clone())).or_insert(0) += 1;
        }
    }
    // assemble ranked map
    let mut map: HashMap<String, Vec<Assoc>> = HashMap::new();
    for ((ext, cmd), n) in counts {
        map.entry(ext).or_default().push(Assoc { command: cmd, count: n });
    }
    for v in map.values_mut() {
        v.sort_by(|a, b| b.count.cmp(&a.count).then(a.command.cmp(&b.command)));
    }
    AssocTable { map }
}

/// v0.1.687: given a filename (a token with an extension) and the learned
/// association table, return the command line that opens it (e.g.
/// "tribal.mp3" → Some("ffplay 'tribal.mp3'")), or None if the extension has
/// no learned handler. The host surfaces this as a "Run in terminal"
/// candidate when a voiced utterance names a file with a generic open verb
/// ("play"/"open"/"watch"...) — fixing "play tribal.mp3" not resolving.
/// Quotes the filename if it contains shell-significant chars.
pub fn file_open_command(filename: &str, table: &AssocTable) -> Option<String> {
    let ext = token_ext(filename)?;
    let cmd = table.best_for(&ext)?;
    Some(format!("{} {}", cmd, shell_quote_arg(filename)))
}

/// Minimal shell quoting for a single arg (single-quote unless already safe).
fn shell_quote_arg(s: &str) -> String {
    let safe = !s.is_empty()
        && s.chars().all(|c| c.is_alphanumeric() || "._-/".contains(c));
    if safe { s.to_string() } else { format!("'{}'", s.replace('\'', "'\\''")) }
}

/// Generic "open this file" verbs that should defer to the learned handler
/// (as opposed to a verb that is itself a command). Used by the caller to
/// decide whether to consult file_open_command.
pub fn is_open_verb(word: &str) -> bool {
    matches!(word.to_lowercase().as_str(),
        "play" | "open" | "watch" | "view" | "show" | "launch" | "run"
        | "start" | "listen")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn table() -> AssocTable {
        build(&["ffplay a.mp3", "ffplay b.mp3", "mpv c.mkv", "xdg-open d.pdf"])
    }

    #[test]
    fn file_open_uses_learned_handler() {
        let t = table();
        assert_eq!(file_open_command("tribal.mp3", &t).as_deref(), Some("ffplay tribal.mp3"));
        assert_eq!(file_open_command("film.mkv", &t).as_deref(), Some("mpv film.mkv"));
        assert_eq!(file_open_command("doc.pdf", &t).as_deref(), Some("xdg-open doc.pdf"));
        // unknown extension → None
        assert!(file_open_command("data.xyz", &t).is_none());
        // not a file (no ext) → None
        assert!(file_open_command("tribal", &t).is_none());
    }

    #[test]
    fn file_open_quotes_spaces_and_paths() {
        let t = table();
        // a path with a space gets quoted
        let cmd = file_open_command("cymatics/my tune.mp3", &t).unwrap();
        assert_eq!(cmd, "ffplay 'cymatics/my tune.mp3'");
        // a clean relative path is left unquoted
        let cmd2 = file_open_command("cymatics/tribal.mp3", &t).unwrap();
        assert_eq!(cmd2, "ffplay cymatics/tribal.mp3");
    }

    #[test]
    fn open_verbs_detected() {
        assert!(is_open_verb("play") && is_open_verb("Open") && is_open_verb("watch"));
        assert!(!is_open_verb("cargo") && !is_open_verb("grep"));
    }

    #[test]
    fn learns_ffplay_for_mp3() {
        let h = vec![
            "ffplay cymatics/tribal.mp3",
            "ffplay song.mp3",
            "mpv movie.mkv",
            "ffplay another.mp3",
            "vlc clip.mp3",
        ];
        let t = build(&h);
        // ffplay used 3× for mp3, vlc 1× → ffplay is best
        assert_eq!(t.best_for("mp3"), Some("ffplay"));
        assert_eq!(t.best_for("mkv"), Some("mpv"));
        let mp3 = t.for_ext("mp3");
        assert_eq!(mp3[0].command, "ffplay");
        assert_eq!(mp3[0].count, 3);
    }

    #[test]
    fn strips_zsh_extended_meta() {
        let h = vec![": 1700000000:0;ffplay beat.mp3"];
        let t = build(&h);
        assert_eq!(t.best_for("mp3"), Some("ffplay"));
    }

    #[test]
    fn skips_generic_builtins() {
        let h = vec!["rm old.mp3", "cp a.mp3 b.mp3", "ls dir.mp3"];
        let t = build(&h);
        // none of these imply a "handler" for mp3
        assert!(t.best_for("mp3").is_none());
    }

    #[test]
    fn ignores_flags_and_urls() {
        let h = vec!["curl https://x.com/a.mp3", "ffplay -nodisp real.mp3"];
        let t = build(&h);
        // the -nodisp flag is skipped; real.mp3 still associates ffplay.
        // the URL arg to curl is ignored (contains ://), so curl→mp3 not learned.
        assert_eq!(t.best_for("mp3"), Some("ffplay"));
    }

    #[test]
    fn handles_sudo_and_env_prefix() {
        let h = vec!["sudo mount disk.img", "FOO=1 mpv show.mkv"];
        let t = build(&h);
        assert_eq!(t.best_for("img"), Some("mount"));
        assert_eq!(t.best_for("mkv"), Some("mpv"));
    }

    #[test]
    fn dotfiles_are_not_extensions() {
        let h = vec!["vim .bashrc", "cat .gitignore"];
        let t = build(&h);
        assert!(t.is_empty(), "dotfiles shouldn't create ext associations");
    }

    #[test]
    fn first_pipeline_segment_only() {
        let h = vec!["ffplay tune.mp3 | tee log.txt"];
        let t = build(&h);
        assert_eq!(t.best_for("mp3"), Some("ffplay"));
        // tee/txt is in the second segment → not associated
        assert!(t.best_for("txt").is_none());
    }

    #[test]
    fn strips_command_path() {
        let h = vec!["/usr/bin/mpv film.mkv", "./play.sh x.mp3"];
        let t = build(&h);
        assert_eq!(t.best_for("mkv"), Some("mpv"));
        assert_eq!(t.best_for("mp3"), Some("play.sh"));
    }
}

//! clip_kind — is this clipped selection code or prose, and in what language?
//!
//! `TODO_notes_clip.md`'s "next pure slice": *"given a selection + a little context (is it
//! inside a fenced block? does it look like code…) → `ClipKind::Code{lang}` vs `Text`.
//! Pure + testable, removes a guess from the clip path. Mirrors `notebook::classify`'s
//! conservative style."*
//!
//! The one clip site in the app hardcodes `ClipKind::Text`, so a clipped diff or function
//! becomes a blockquote — markdown will reflow it, eat the leading spaces and italicise
//! anything between asterisks. Getting this wrong in the prose direction is a formatting
//! annoyance; getting it wrong in the code direction wraps a paragraph in a fence, which is
//! ugly but lossless. So the bias is toward Code only on real evidence, and the evidence is
//! ranked:
//!
//! 1. **A fence in the selection.** ```` ```rust ```` is the author saying so; nothing
//!    outranks it, and the language comes free.
//! 2. **The command that produced it.** `cat main.rs`, `bat src/lib.rs`, `git diff` —
//!    the command names the file, and the extension names the language. This is the
//!    signal the terminal has and a web page does not.
//! 3. **The shape.** Braces and semicolons at line ends, `fn`/`def`/`function` openers,
//!    consistent indentation, sigils no prose uses (`=>`, `::`, `->`, `#include`). One
//!    signal is a coincidence — an English sentence can end in a semicolon — so shape
//!    alone needs two, which is what keeps a paragraph out of a fence.
//!
//! A language is a hint, never a requirement: `Code { lang: "" }` is a fenced block with
//! no highlighting, which is right for a shell transcript or an unknown file.

use crate::note_clip::ClipKind;

/// What the clip path knows besides the text itself.
#[derive(Debug, Clone, Copy, Default)]
pub struct Context<'a> {
    /// The command whose output the selection came from, when it came from a terminal.
    pub command: &'a str,
    /// The file the selection came from, when it came from a file view.
    pub path: &'a str,
}

impl<'a> Context<'a> {
    pub fn from_command(command: &'a str) -> Context<'a> {
        Context { command, path: "" }
    }
    pub fn from_path(path: &'a str) -> Context<'a> {
        Context { command: "", path }
    }
}

/// Extensions worth naming. Anything else falls back to an empty lang, which fences
/// without highlighting rather than guessing a wrong grammar.
const LANGS: &[(&str, &str)] = &[
    ("rs", "rust"),
    ("py", "python"),
    ("js", "javascript"),
    ("mjs", "javascript"),
    ("ts", "typescript"),
    ("tsx", "typescript"),
    ("jsx", "javascript"),
    ("c", "c"),
    ("h", "c"),
    ("cc", "cpp"),
    ("cpp", "cpp"),
    ("hpp", "cpp"),
    ("go", "go"),
    ("java", "java"),
    ("rb", "ruby"),
    ("sh", "bash"),
    ("bash", "bash"),
    ("zsh", "bash"),
    ("fish", "fish"),
    ("nu", "nu"),
    ("toml", "toml"),
    ("yaml", "yaml"),
    ("yml", "yaml"),
    ("json", "json"),
    ("sql", "sql"),
    ("html", "html"),
    ("css", "css"),
    ("lua", "lua"),
    ("nix", "nix"),
    ("qml", "qml"),
    ("kt", "kotlin"),
    ("swift", "swift"),
    ("php", "php"),
    ("pl", "perl"),
    ("vim", "vim"),
];

/// The language for a path or file name, by extension. Empty when unknown — including for
/// a file with no extension, where guessing from the name (`Makefile`, `Dockerfile`) is a
/// different table and not one this needs.
pub fn lang_for_path(path: &str) -> String {
    let name = path.rsplit(['/', '\\']).next().unwrap_or(path);
    let Some((_, ext)) = name.rsplit_once('.') else { return String::new() };
    let ext = ext.trim_end_matches([',', ')', ':', ';', '"', '\'']).to_ascii_lowercase();
    LANGS
        .iter()
        .find(|(e, _)| *e == ext)
        .map(|(_, l)| (*l).to_string())
        .unwrap_or_default()
}

/// The commands that print a file more or less verbatim. `less`/`more` are here too: what
/// lands in the scrollback is the file.
const FILE_PRINTERS: &[&str] = &["cat", "bat", "head", "tail", "less", "more", "type", "nl"];

/// The language a COMMAND implies, and whether it implies code at all.
///
/// `git diff` and `git show` are code whatever the file was; `cat` and friends take the
/// language from the argument they printed. A command that is not one of these implies
/// nothing — `ls` output is not code, and `echo` of a brace is not either.
pub fn lang_for_command(command: &str) -> Option<String> {
    let cmd = command.trim();
    if cmd.is_empty() {
        return None;
    }
    // strip a leading `sudo` / env assignments, which do not change what ran
    let mut words = cmd.split_whitespace().skip_while(|w| *w == "sudo" || w.contains('='));
    let head = words.next()?;
    let head = head.rsplit('/').next().unwrap_or(head);
    if head == "git" {
        let sub = words.next().unwrap_or("");
        return matches!(sub, "diff" | "show" | "log" | "blame").then(String::new);
    }
    if head == "diff" {
        return Some(String::new());
    }
    if FILE_PRINTERS.contains(&head) {
        // the first argument that is not a flag names the file
        let arg = words.find(|w| !w.starts_with('-'))?;
        return Some(lang_for_path(arg));
    }
    None
}

/// A fence in the text: its language, if the selection carries one.
///
/// Returns `Some("")` for a bare ```` ``` ```` — the author fenced it and named nothing.
pub fn fenced_lang(text: &str) -> Option<String> {
    for line in text.lines() {
        let t = line.trim_start();
        if let Some(rest) = t.strip_prefix("```").or_else(|| t.strip_prefix("~~~")) {
            let lang = rest.trim().split_whitespace().next().unwrap_or("");
            // `--- rust` and `{rust}` forms turn up in the wild; take the word either way
            let lang = lang.trim_matches(|c: char| !c.is_ascii_alphanumeric() && c != '+' && c != '#');
            return Some(lang.to_ascii_lowercase());
        }
    }
    None
}

/// How many independent code signals the text's own shape shows, 0..=5.
///
/// Deliberately coarse. Each signal is something prose does occasionally and code does
/// constantly, so one is a coincidence and two together are not.
pub fn shape_signals(text: &str) -> u32 {
    let lines: Vec<&str> = text.lines().filter(|l| !l.trim().is_empty()).collect();
    if lines.is_empty() {
        return 0;
    }
    let n = lines.len() as f32;
    let mut signals = 0;

    // 1. lines ending in a brace, bracket or semicolon — statement punctuation
    let terminated = lines
        .iter()
        .filter(|l| {
            let t = l.trim_end();
            t.ends_with(';') || t.ends_with('{') || t.ends_with('}') || t.ends_with(':') && t.len() < 80
        })
        .count() as f32;
    if terminated / n >= 0.3 {
        signals += 1;
    }

    // 2. an opener: a declaration keyword at the start of a line
    const OPENERS: &[&str] = &[
        "fn ", "def ", "function ", "class ", "struct ", "impl ", "pub ", "const ", "let ",
        "var ", "public ", "private ", "import ", "from ", "use ", "package ", "#include",
        "if (", "for (", "while (", "return ", "async ", "export ", "module ", "type ",
        // the shell writes its conditions with brackets, not parentheses
        "if [", "while [", "case ", "elif ",
    ];
    if lines.iter().any(|l| {
        let t = l.trim_start();
        OPENERS.iter().any(|o| t.starts_with(o))
    }) {
        signals += 1;
    }

    // 3. sigils prose does not use
    const SIGILS: &[&str] = &["=>", "::", "->", "!=", "==", "&&", "||", "</", "/>", "$(", "){"];
    let sigil_lines = lines.iter().filter(|l| SIGILS.iter().any(|s| l.contains(s))).count() as f32;
    if sigil_lines / n >= 0.25 {
        signals += 1;
    }

    // 4. block words — the shell's punctuation is words, not braces, so a script scores
    //    nothing on the three signals above: `if [ -f "$f" ]; then / cp … / fi` has no
    //    semicolon at a line end, no `if (`, no sigil. (Found by the test for exactly
    //    that snippet.) A line that IS a block word, or ends in one.
    const BLOCK_WORDS: &[&str] = &["then", "do", "fi", "done", "esac", "end", "else", "elif"];
    let block_lines = lines
        .iter()
        .filter(|l| {
            let t = l.trim();
            BLOCK_WORDS.contains(&t) || BLOCK_WORDS.iter().any(|w| t.ends_with(&format!(" {w}")))
        })
        .count();
    if block_lines >= 2 || (block_lines == 1 && lines.len() <= 4) {
        signals += 1;
    }

    // 5. indentation that is structural: at least a third of lines indented, and more
    //    than one distinct depth (a quoted paragraph is uniformly indented)
    let depths: Vec<usize> = lines.iter().map(|l| l.len() - l.trim_start().len()).collect();
    let indented = depths.iter().filter(|d| **d > 0).count() as f32;
    let mut distinct: Vec<usize> = depths.clone();
    distinct.sort_unstable();
    distinct.dedup();
    if indented / n >= 0.34 && distinct.len() > 1 {
        signals += 1;
    }
    signals
}

/// The whole decision. `ClipKind::Text` unless there is real evidence otherwise.
pub fn detect(text: &str, ctx: Context<'_>) -> ClipKind {
    if text.trim().is_empty() {
        return ClipKind::Text;
    }
    // 1. the author fenced it
    if let Some(lang) = fenced_lang(text) {
        return ClipKind::Code { lang };
    }
    // 2. the command or the path named a file
    let from_ctx = lang_for_command(ctx.command).or_else(|| {
        let l = lang_for_path(ctx.path);
        (!ctx.path.trim().is_empty()).then_some(l)
    });
    if let Some(lang) = from_ctx {
        return ClipKind::Code { lang };
    }
    // 3. shape, two signals or more
    if shape_signals(text) >= 2 {
        return ClipKind::Code { lang: String::new() };
    }
    ClipKind::Text
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_lang(k: &ClipKind) -> Option<&str> {
        match k {
            ClipKind::Code { lang } => Some(lang.as_str()),
            _ => None,
        }
    }

    #[test]
    fn a_fence_outranks_everything_and_carries_its_language() {
        let t = "```rust\nfn main() {}\n```";
        assert_eq!(code_lang(&detect(t, Context::default())), Some("rust"));
        // even when the command says otherwise
        assert_eq!(code_lang(&detect(t, Context::from_command("cat notes.txt"))), Some("rust"));
        assert_eq!(fenced_lang("~~~ python\nx = 1\n~~~").as_deref(), Some("python"));
        assert_eq!(fenced_lang("```\nplain\n```").as_deref(), Some(""), "a bare fence is still a fence");
        assert_eq!(fenced_lang("no fence here"), None);
    }

    #[test]
    fn the_command_names_the_language_when_it_printed_a_file() {
        assert_eq!(lang_for_command("cat src/main.rs").as_deref(), Some("rust"));
        assert_eq!(lang_for_command("sudo bat -p /etc/nginx/nginx.conf").as_deref(), Some(""), "unknown extension fences without a language");
        assert_eq!(lang_for_command("RUST_LOG=debug head -n20 app.py").as_deref(), Some("python"));
        assert_eq!(lang_for_command("git diff").as_deref(), Some(""));
        assert_eq!(lang_for_command("git status"), None, "status output is not code");
        assert_eq!(lang_for_command("ls -la"), None);
        assert_eq!(lang_for_command("echo hello {"), None, "a brace in an echo is not a program");
        assert_eq!(lang_for_command(""), None);
        // and a path, for the file view
        assert_eq!(lang_for_path("/home/k/proj/lib.rs"), "rust");
        assert_eq!(lang_for_path("Makefile"), "", "no extension, no guess");
        assert_eq!(lang_for_path("notes.md"), "");
    }

    #[test]
    fn prose_stays_prose_even_when_it_punctuates_like_code() {
        let prose = "The compost heap wants turning weekly; a fortnight is too long.\n\
                     Straw over the beds before the first frost, and mulch the paths.";
        assert_eq!(detect(prose, Context::default()), ClipKind::Text);
        assert!(shape_signals(prose) < 2, "one semicolon is not a program");
        // a quoted paragraph: uniformly indented, no second depth
        let quoted = "    Everything that is said is said by someone.\n    And everything said is said to someone.";
        assert_eq!(detect(quoted, Context::default()), ClipKind::Text);
        // an ls listing has neither punctuation nor openers
        assert_eq!(detect("Cargo.toml\nbuild.sh\nsrc\nold.bak", Context::from_command("ls")), ClipKind::Text);
        assert_eq!(detect("", Context::from_command("cat x.rs")), ClipKind::Text, "nothing selected is nothing");
    }

    #[test]
    fn shape_alone_catches_code_pasted_with_no_context() {
        let rs = "fn main() {\n    let x = compute();\n    if x > 3 {\n        println!(\"{x}\");\n    }\n}";
        assert!(shape_signals(rs) >= 2);
        assert_eq!(code_lang(&detect(rs, Context::default())), Some(""), "code without a source fences unlabelled");
        let py = "def main():\n    total = 0\n    for row in rows:\n        total += row.n\n    return total";
        assert!(shape_signals(py) >= 2, "signals {}", shape_signals(py));
        assert!(matches!(detect(py, Context::default()), ClipKind::Code { .. }));
        let sh = "if [ -f \"$f\" ]; then\n  cp \"$f\" \"$d\"\nfi";
        assert!(matches!(detect(sh, Context::default()), ClipKind::Code { .. }));
    }

    #[test]
    fn a_selection_from_a_file_view_takes_the_files_language() {
        assert_eq!(code_lang(&detect("anything at all", Context::from_path("src/lib.rs"))), Some("rust"));
        assert_eq!(
            code_lang(&detect("anything at all", Context::from_path("README"))),
            Some(""),
            "a file with no extension still fences: the view knows it is a file"
        );
        assert_eq!(detect("anything at all", Context::from_path("")), ClipKind::Text);
    }
}

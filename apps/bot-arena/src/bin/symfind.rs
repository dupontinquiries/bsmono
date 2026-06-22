//! symfind — a lightweight, language-aware symbol locator for the CLI coding
//! toolkit. It answers two questions without a full parser:
//!
//!   symfind <name> [dir]          where is <name> defined, and where referenced?
//!   symfind --body <name> <file>  print the exact line span of the function/block
//!                                 named <name> (the "locate the edit target" primitive)
//!
//! Definitions are detected with per-language keyword heuristics (fn/struct/def/
//! class/func/...); references are whole-word matches elsewhere. Output is
//! grep-friendly `path:line: [def|ref] text`. Read-only.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, PartialEq)]
enum Lang { Rust, Python, JsTs, CFamily, Go, Other }

fn lang_of(path: &Path) -> Lang {
    match path.extension().and_then(|s| s.to_str()) {
        Some("rs") => Lang::Rust,
        Some("py") => Lang::Python,
        Some("js") | Some("ts") | Some("jsx") | Some("tsx") | Some("mjs") => Lang::JsTs,
        Some("c") | Some("h") | Some("cc") | Some("cpp") | Some("cxx") | Some("hpp") => Lang::CFamily,
        Some("go") => Lang::Go,
        _ => Lang::Other,
    }
}

fn is_ident_char(c: char) -> bool { c.is_alphanumeric() || c == '_' }

/// Does `line` contain `name` as a standalone identifier (not a substring of a
/// longer identifier)?
fn contains_word(line: &str, name: &str) -> bool {
    let bytes = line.as_bytes();
    let nb = name.as_bytes();
    if nb.is_empty() { return false; }
    let mut i = 0;
    while let Some(rel) = line[i..].find(name) {
        let start = i + rel;
        let end = start + name.len();
        let before_ok = start == 0 || !is_ident_char(line[..start].chars().next_back().unwrap());
        let after_ok = end >= bytes.len() || !is_ident_char(line[end..].chars().next().unwrap_or(' '));
        if before_ok && after_ok { return true; }
        i = start + 1;
        if i >= line.len() { break; }
    }
    false
}

/// Remove leading tokens that are in `words` (and an `extern "C"`-style quoted
/// string following `extern`). Returns the remaining trimmed slice.
fn strip_leading<'a>(mut t: &'a str, words: &[&str]) -> &'a str {
    loop {
        t = t.trim_start();
        let tok: String = t.chars().take_while(|c| is_ident_char(*c) || *c == '(' || *c == ')' || *c == ':').collect();
        // handle `pub(crate)` etc.: the visibility token may include parens
        let head = t.split_whitespace().next().unwrap_or("");
        if words.contains(&head) || (head.starts_with("pub(") && words.contains(&"pub")) {
            t = &t[head.len()..];
            // `extern "C"` → also drop the quoted abi
            let rest = t.trim_start();
            if rest.starts_with('"') {
                if let Some(end) = rest[1..].find('"') { t = &rest[end + 2..]; }
            }
            continue;
        }
        let _ = tok;
        return t.trim_start();
    }
}

/// Does the (already vis/modifier-stripped) text start with `kw name<boundary>`?
fn kw_then_name(s: &str, kw: &str, name: &str) -> bool {
    if !s.starts_with(kw) { return false; }
    let rest = s[kw.len()..].trim_start();
    rest.starts_with(name)
        && rest[name.len()..].chars().next().map(|c| !is_ident_char(c)).unwrap_or(true)
}

/// Heuristic: is this line a *definition* of `name` in the given language?
fn is_def_line(lang: Lang, name: &str, line: &str) -> bool {
    let t = line.trim_start();
    const VIS: &[&str] = &["pub", "export", "public", "private", "protected", "final", "open"];
    const MODS: &[&str] = &["pub", "export", "public", "private", "protected", "final", "open",
                            "async", "unsafe", "extern", "inline", "default", "const", "static", "virtual"];
    match lang {
        Lang::Rust => {
            // value bindings keep const/static as the keyword
            let v = strip_leading(t, VIS);
            if kw_then_name(v, "const ", name) || kw_then_name(v, "static ", name) { return true; }
            let s = strip_leading(t, MODS);
            ["fn ", "struct ", "enum ", "trait ", "type ", "mod ", "union "]
                .iter().any(|k| kw_then_name(s, k, name))
                || s.starts_with(&format!("macro_rules! {name}"))
        }
        Lang::Python => {
            let s = strip_leading(t, &["async"]);
            kw_then_name(s, "def ", name) || kw_then_name(s, "class ", name)
        }
        Lang::JsTs => {
            let s = strip_leading(t, &["export", "default", "async", "static", "public", "private"]);
            ["function ", "class ", "const ", "let ", "var "].iter().any(|k| kw_then_name(s, k, name))
        }
        Lang::Go => kw_then_name(t, "func ", name) || kw_then_name(t, "type ", name)
            || t.starts_with(&format!("func ({}", "")) && contains_word(t, name) && t.contains(&format!(") {name}(")),
        Lang::CFamily => {
            if t.starts_with("#define") { return contains_word(t, name); }
            let s = strip_leading(t, &["static", "inline", "extern", "const", "unsigned", "signed", "struct", "enum", "union", "typedef", "public", "private"]);
            if ["struct ", "enum ", "union ", "class ", "typedef "].iter().any(|k| kw_then_name(t, k, name)) { return true; }
            // function definition: `... name( ... )` ending in `{` or `)`
            if let Some(p) = s.find(name) {
                let after = &s[p + name.len()..];
                let before_ok = p == 0 || !is_ident_char(s[..p].chars().next_back().unwrap());
                return before_ok && after.trim_start().starts_with('(')
                    && (t.trim_end().ends_with('{') || t.trim_end().ends_with(')'));
            }
            false
        }
        Lang::Other => false,
    }
}

fn collect_files(dir: &Path, out: &mut Vec<PathBuf>, depth: usize) {
    if depth > 12 { return; }
    let skip = ["target", "node_modules", ".git", "build", "dist", "__pycache__", "venv", ".venv"];
    let Ok(rd) = std::fs::read_dir(dir) else { return; };
    for e in rd.flatten() {
        let p = e.path();
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if name.starts_with('.') { continue; }
        if p.is_dir() {
            if skip.contains(&name) { continue; }
            collect_files(&p, out, depth + 1);
        } else if lang_of(&p) != Lang::Other {
            out.push(p);
        }
    }
}

fn find(name: &str, root: &Path) {
    let mut files = Vec::new();
    if root.is_dir() { collect_files(root, &mut files, 0); }
    else { files.push(root.to_path_buf()); }
    files.sort();
    let (mut defs, mut refs) = (0usize, 0usize);
    for f in &files {
        let lang = lang_of(f);
        let Ok(text) = std::fs::read_to_string(f) else { continue; };
        for (n, line) in text.lines().enumerate() {
            if !contains_word(line, name) { continue; }
            let kind = if is_def_line(lang, name, line) { defs += 1; "def" } else { refs += 1; "ref" };
            println!("{}:{}: [{}] {}", f.display(), n + 1, kind, line.trim());
        }
    }
    eprintln!("symfind: {} definition(s), {} reference(s) of `{}` across {} file(s)",
        defs, refs, name, files.len());
}

/// Print the line span of the function/block named `name` in `file`. Brace-matched
/// for C-family/Rust/Go/JS; indentation-scoped for Python.
fn body(name: &str, file: &Path) {
    let Ok(text) = std::fs::read_to_string(file) else {
        eprintln!("symfind: cannot read {}", file.display()); std::process::exit(1);
    };
    let lang = lang_of(file);
    let lines: Vec<&str> = text.lines().collect();
    let Some(start) = lines.iter().position(|l| is_def_line(lang, name, l)) else {
        eprintln!("symfind: no definition of `{}` found in {}", name, file.display());
        std::process::exit(1);
    };
    let end = if lang == Lang::Python {
        // until the next line at the same-or-lower indent that isn't blank
        let base = indent(lines[start]);
        let mut e = start + 1;
        while e < lines.len() {
            if !lines[e].trim().is_empty() && indent(lines[e]) <= base { break; }
            e += 1;
        }
        e
    } else {
        // brace match from the first '{' at/after start
        let mut depth = 0i32;
        let mut seen = false;
        let mut e = start;
        'outer: while e < lines.len() {
            for ch in lines[e].chars() {
                if ch == '{' { depth += 1; seen = true; }
                else if ch == '}' { depth -= 1; if seen && depth == 0 { break 'outer; } }
            }
            e += 1;
        }
        (e + 1).min(lines.len())
    };
    for (i, l) in lines[start..end.min(lines.len())].iter().enumerate() {
        println!("{}: {}", start + i + 1, l);
    }
    eprintln!("symfind: `{}` spans lines {}–{} of {}", name, start + 1, end.min(lines.len()), file.display());
}

fn indent(s: &str) -> usize { s.chars().take_while(|c| *c == ' ' || *c == '\t').count() }

fn main() {
    let args: Vec<String> = std::env::args().collect();
    if args.len() >= 4 && args[1] == "--body" {
        body(&args[2], Path::new(&args[3]));
        return;
    }
    if args.len() < 2 {
        eprintln!("usage: symfind <name> [dir]            # find definitions + references");
        eprintln!("       symfind --body <name> <file>    # print the named block's line span");
        std::process::exit(2);
    }
    let name = &args[1];
    let root = PathBuf::from(args.get(2).cloned().unwrap_or_else(|| ".".to_string()));
    find(name, &root);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn word_boundary_matching() {
        assert!(contains_word("let foo = 1;", "foo"));
        assert!(!contains_word("let foobar = 1;", "foo"));
        assert!(!contains_word("let barfoo = 1;", "foo"));
        assert!(contains_word("foo(bar)", "foo"));
        assert!(contains_word("x.foo()", "foo"));
    }

    #[test]
    fn def_detection_per_language() {
        assert!(is_def_line(Lang::Rust, "add", "pub fn add(a: i32) -> i32 {"));
        assert!(is_def_line(Lang::Rust, "Point", "struct Point { x: i32 }"));
        assert!(!is_def_line(Lang::Rust, "add", "    let y = add(2, 3);"));
        assert!(is_def_line(Lang::Python, "run", "def run(self):"));
        assert!(is_def_line(Lang::Python, "Widget", "class Widget:"));
        assert!(is_def_line(Lang::Go, "Main", "func Main() {"));
        assert!(is_def_line(Lang::CFamily, "main", "int main(int argc, char **argv) {"));
        assert!(is_def_line(Lang::CFamily, "MAX", "#define MAX 100"));
    }

    #[test]
    fn body_span_brace_matched() {
        let dir = std::env::temp_dir().join(format!("symfind-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        std::fs::create_dir_all(&dir).unwrap();
        let f = dir.join("x.rs");
        std::fs::write(&f, "fn other() {}\nfn target() {\n    let a = 1;\n    a + 1\n}\nfn after() {}\n").unwrap();
        let text = std::fs::read_to_string(&f).unwrap();
        let lines: Vec<&str> = text.lines().collect();
        let start = lines.iter().position(|l| is_def_line(Lang::Rust, "target", l)).unwrap();
        assert_eq!(start, 1, "target starts on line 2 (index 1)");
        let _ = std::fs::remove_dir_all(&dir);
    }
}

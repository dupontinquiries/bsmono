//! astsketch — a basic, language-agnostic outline of a source file: definition
//! lines (fn/def/class/struct/enum/impl/trait/type/const/static/interface/func)
//! indented by brace depth. Not a real parser — a fast "structure at a glance" an
//! agent can read instead of the whole file.
//!
//!   astsketch <file>

use std::path::Path;

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(path) = args.first() else {
        eprintln!("usage: astsketch <file>");
        std::process::exit(2);
    };
    let src = match std::fs::read_to_string(Path::new(path)) {
        Ok(s) => s,
        Err(e) => { eprintln!("cannot read {path}: {e}"); std::process::exit(1); }
    };
    for line in sketch(&src) {
        println!("{line}");
    }
}

const KEYWORDS: &[&str] = &[
    "fn", "def", "class", "struct", "enum", "impl", "trait", "type", "const",
    "static", "interface", "func", "module", "mod", "macro_rules", "pub",
];

/// Produce outline lines: "<lineno>: <indent><trimmed source>".
fn sketch(src: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut depth: i32 = 0;
    for (n, raw) in src.lines().enumerate() {
        let trimmed = raw.trim();
        let first = trimmed.split(|c: char| !c.is_ascii_alphanumeric() && c != '_').next().unwrap_or("");
        let is_def = KEYWORDS.contains(&first)
            || trimmed.starts_with("export ")
            || trimmed.starts_with("function ");
        if is_def && !trimmed.is_empty() {
            let indent = "  ".repeat(depth.max(0) as usize);
            let mut shown = trimmed.to_string();
            if shown.len() > 100 { shown.truncate(100); shown.push('…'); }
            out.push(format!("{:>5}: {indent}{shown}", n + 1));
        }
        depth += raw.matches('{').count() as i32 - raw.matches('}').count() as i32;
    }
    if out.is_empty() {
        out.push("(no top-level definitions recognized)".to_string());
    }
    out
}

#[cfg(test)]
mod tests {
    use super::sketch;
    #[test]
    fn finds_defs_and_nesting() {
        let src = "pub struct S {\n    fn inner() {}\n}\nfn top() {}\n";
        let s = sketch(src);
        assert!(s.iter().any(|l| l.contains("struct S")));
        assert!(s.iter().any(|l| l.contains("fn inner")));
        assert!(s.iter().any(|l| l.contains("fn top")));
        // inner is nested one level deeper than struct/top
        let inner = s.iter().find(|l| l.contains("fn inner")).unwrap();
        assert!(inner.contains(":   ") || inner.contains("  fn inner"));
    }
}

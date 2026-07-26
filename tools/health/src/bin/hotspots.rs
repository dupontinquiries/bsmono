//! hotspots — the longest functions and largest files in a tree, lexer-accurately.
//!
//! The disease: this repo's hard edits all slow down in the same places — a 12.9K-line
//! GTK main.rs, several 10K-line lib.rs. grep can find big FILES; finding big
//! FUNCTIONS needs real brace matching (strings and comments full of braces), which is
//! what the shared lexer provides. Output: top-N functions by body line count and
//! top-N files by total lines, with thresholds so CI can watch the ceiling.
//!
//! Usage: hotspots [--top N] [--fn-threshold LINES] DIR...   (.rs files)
//! Exit: 0; with --fn-threshold, 1 if any function exceeds it.

#[path = "../../../sanity/src/lex.rs"]
mod lex;

fn is_ident(b: u8) -> bool { b.is_ascii_alphanumeric() || b == b'_' }

/// (name, start_line, body_lines) for every fn with a brace body.
fn scan_fns(code: &str, pos: &[(u32, u32)]) -> Vec<(String, u32, u32)> {
    let cb = code.as_bytes();
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = code[from..].find("fn ") {
        let at = from + rel;
        from = at + 3;
        if at > 0 && is_ident(cb[at - 1]) { continue; }
        let mut i = at + 3;
        while i < cb.len() && cb[i].is_ascii_whitespace() { i += 1; }
        let ns = i;
        while i < cb.len() && is_ident(cb[i]) { i += 1; }
        if i == ns { continue; }
        let name = code[ns..i].to_string();
        // find the body '{' — skip past the parameter parens first so a closure in a
        // default value can't fake the body, then take the first top-level '{'
        while i < cb.len() && cb[i] != b'(' && cb[i] != b';' && cb[i] != b'{' { i += 1; }
        if i < cb.len() && cb[i] == b'(' {
            let mut d = 1i32; i += 1;
            while i < cb.len() && d > 0 {
                match cb[i] { b'(' => d += 1, b')' => d -= 1, _ => {} }
                i += 1;
            }
        }
        // now scan to '{' or ';' (trait decl) — return types/where clauses pass by
        while i < cb.len() && cb[i] != b'{' && cb[i] != b';' { i += 1; }
        if i >= cb.len() || cb[i] == b';' { continue; }
        let open_line = pos[i].0;
        let mut d = 1i32; i += 1;
        while i < cb.len() && d > 0 {
            match cb[i] { b'{' => d += 1, b'}' => d -= 1, _ => {} }
            i += 1;
        }
        let close_line = if i > 0 && i <= pos.len() { pos[i - 1].0 } else { open_line };
        out.push((name, pos[ns].0, close_line.saturating_sub(open_line).saturating_add(1)));
    }
    out
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut top = 15usize;
    let mut fn_threshold: Option<u32> = None;
    if let Some(p) = args.iter().position(|a| a == "--top") {
        if p + 1 < args.len() { top = args.remove(p + 1).parse().unwrap_or(15); args.remove(p); }
    }
    if let Some(p) = args.iter().position(|a| a == "--fn-threshold") {
        if p + 1 < args.len() { fn_threshold = args.remove(p + 1).parse().ok(); args.remove(p); }
    }
    if args.is_empty() {
        eprintln!("usage: hotspots [--top N] [--fn-threshold LINES] DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args { lex::collect_files(std::path::Path::new(a), &["rs"], &mut files); }
    let mut fns: Vec<(String, String, u32, u32)> = Vec::new(); // file, name, line, body
    let mut sizes: Vec<(u32, String)> = Vec::new();
    for f in &files {
        let Ok(src) = std::fs::read(f) else { continue };
        let mut code = String::with_capacity(src.len());
        let mut pos = Vec::with_capacity(src.len());
        lex::walk_code(&src, |b, l, c| { code.push(b as char); pos.push((l, c)); });
        let total = src.iter().filter(|b| **b == b'\n').count() as u32 + 1;
        sizes.push((total, f.display().to_string()));
        for (name, line, body) in scan_fns(&code, &pos) {
            fns.push((f.display().to_string(), name, line, body));
        }
    }
    fns.sort_by_key(|x| std::cmp::Reverse(x.3));
    sizes.sort_by_key(|x| std::cmp::Reverse(x.0));
    println!("── longest functions ────────────────────────────────────");
    for (file, name, line, body) in fns.iter().take(top) {
        println!("  {body:>5} lines  {name}  {file}:{line}");
    }
    println!("── largest files ────────────────────────────────────────");
    for (n, f) in sizes.iter().take(top.min(10)) {
        println!("  {n:>6} lines  {f}");
    }
    let mut over = 0usize;
    if let Some(t) = fn_threshold {
        for (file, name, line, body) in &fns {
            if *body > t { println!("{file}:{line}:1: fn {name} is {body} lines (> {t})"); over += 1; }
        }
    }
    std::process::exit(if over > 0 { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    fn scan(src: &str) -> Vec<(String, u32, u32)> {
        let mut code = String::new(); let mut pos = Vec::new();
        lex::walk_code(src.as_bytes(), |b, l, c| { code.push(b as char); pos.push((l, c)); });
        scan_fns(&code, &pos)
    }
    #[test]
    fn body_lines_are_counted_and_braces_in_strings_do_not_confuse() {
        let src = "fn a() {\n let s = \"}{\";\n x();\n}\nfn b() -> u8 { 1 }\ntrait T { fn c(&self); }\n";
        let f = scan(src);
        assert_eq!(f.len(), 2, "{f:?}"); // c has no body
        assert_eq!(f[0], ("a".into(), 1, 4));
        assert_eq!(f[1].0, "b"); assert_eq!(f[1].2, 1);
    }
    #[test]
    fn where_clauses_and_nested_fns_resolve_to_their_own_bodies() {
        let src = "fn outer<T>(t: T) -> u8 where T: Into<u8> {\n fn inner() { }\n 1\n}\n";
        let f = scan(src);
        assert_eq!(f.len(), 2);
        assert_eq!(f[0].0, "outer"); assert_eq!(f[0].2, 4);
        assert_eq!(f[1].0, "inner"); assert_eq!(f[1].2, 1);
    }
}

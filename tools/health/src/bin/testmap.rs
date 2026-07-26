//! testmap — which crates the suite-gated method actually covers.
//!
//! This repo's whole way of working is "change lands with its tests green" — which is
//! only true where tests EXIST. testmap walks every Cargo.toml-rooted crate, counts
//! `#[test]` functions (code-only, via the lexer) and `tests/` integration files
//! against src line counts, and prints the zero-test crates FIRST: each one is a place
//! the method silently doesn't apply. A bin-only tool crate at zero can be a
//! judgment call; an engine crate at zero is a finding.
//!
//! Usage: testmap DIR...      Exit: 0 (report); --fail-on-zero exits 1 if any
//! zero-test crate has more than --min-lines (default 200) of src.

#[path = "../../../sanity/src/lex.rs"]
mod lex;

use std::path::{Path, PathBuf};

fn find_crates(path: &Path, out: &mut Vec<PathBuf>) {
    let rd = match std::fs::read_dir(path) { Ok(r) => r, Err(_) => return };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    let mut here_is_crate = false;
    for p in &entries {
        if p.file_name().and_then(|s| s.to_str()) == Some("Cargo.toml") && path.join("src").is_dir() {
            here_is_crate = true;
        }
    }
    if here_is_crate { out.push(path.to_path_buf()); }
    for p in entries {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if p.is_dir() && !matches!(name, "target" | ".git" | "node_modules" | "build" | "vendor") {
            find_crates(&p, out);
        }
    }
}

struct Row { name: String, tests: usize, tfiles: usize, src_lines: usize }

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut min_lines = 200usize;
    if let Some(p) = args.iter().position(|a| a == "--min-lines") {
        if p + 1 < args.len() { min_lines = args.remove(p + 1).parse().unwrap_or(200); args.remove(p); }
    }
    let fail_on_zero = if let Some(p) = args.iter().position(|a| a == "--fail-on-zero") { args.remove(p); true } else { false };
    if args.is_empty() {
        eprintln!("usage: testmap [--min-lines N] [--fail-on-zero] DIR...");
        std::process::exit(2);
    }
    let mut crates = Vec::new();
    for a in &args { find_crates(Path::new(a), &mut crates); }

    let mut rows = Vec::new();
    for c in &crates {
        let mut srcs = Vec::new();
        lex::collect_files(&c.join("src"), &["rs"], &mut srcs);
        let mut tests = 0usize; let mut src_lines = 0usize;
        for f in &srcs {
            let Ok(raw) = std::fs::read(f) else { continue };
            src_lines += raw.iter().filter(|b| **b == b'\n').count() + 1;
            let mut code = String::with_capacity(raw.len());
            lex::walk_code(&raw, |b, _, _| code.push(b as char));
            let mut from = 0;
            while let Some(rel) = code[from..].find("#[test]") { tests += 1; from += rel + 7; }
        }
        let mut tfiles = Vec::new();
        lex::collect_files(&c.join("tests"), &["rs"], &mut tfiles);
        for f in &tfiles {
            let Ok(raw) = std::fs::read(f) else { continue };
            let mut code = String::with_capacity(raw.len());
            lex::walk_code(&raw, |b, _, _| code.push(b as char));
            let mut from = 0;
            while let Some(rel) = code[from..].find("#[test]") { tests += 1; from += rel + 7; }
        }
        rows.push(Row { name: c.display().to_string(), tests, tfiles: tfiles.len(), src_lines });
    }
    rows.sort_by_key(|r| (r.tests, std::cmp::Reverse(r.src_lines)));
    println!("── test coverage map (zero-test crates first) ───────────");
    println!("  tests  t-files  src-lines  crate");
    let mut zero_big = 0usize;
    for r in &rows {
        let flag = if r.tests == 0 && r.src_lines > min_lines { zero_big += 1; "  ← ZERO" } else { "" };
        println!("  {:>5}  {:>7}  {:>9}  {}{}", r.tests, r.tfiles, r.src_lines, r.name, flag);
    }
    std::process::exit(if fail_on_zero && zero_big > 0 { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn test_attr_in_a_string_does_not_count() {
        let raw = "fn f() { let s = \"#[test]\"; }\n#[test]\nfn t() {}\n";
        let mut code = String::new();
        lex::walk_code(raw.as_bytes(), |b, _, _| code.push(b as char));
        let mut n = 0; let mut from = 0;
        while let Some(rel) = code[from..].find("#[test]") { n += 1; from += rel + 7; }
        assert_eq!(n, 1);
    }
}

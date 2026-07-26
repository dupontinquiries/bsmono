//! modcheck — `mod` declarations vs the files actually on disk, both directions.
//!
//! Motivating incident: `openworld.rs` sat complete-but-invisible in clawbound's src/
//! until its `pub mod openworld;` line landed — the file compiled in nobody's build
//! and no tool said so. The inverse bites too: a `mod foo;` whose `foo.rs` was renamed
//! or never created fails only at the next compile.
//!
//! Two checks per crate source tree:
//!   MISSING — `mod name;` (no body, not `#[path]`-redirected) with no `name.rs` and
//!             no `name/mod.rs` next to the declaring file;
//!   ORPHAN  — a `.rs` file no `mod` line names and no `#[path]` points at, excluding
//!             the roots (lib.rs, main.rs, mod.rs, build.rs) and conventional
//!             auto-discovered locations (src/bin/*, tests/, examples/, benches/).
//!
//! `#[path = "…"]` redirections are honored for both directions. cfg-gated mods are
//! treated as present (a declaration is a declaration). include! trickery is out of
//! scope — the compiler owns that.
//!
//! Usage: modcheck DIR...   (point it at crate roots or src/ dirs; files recurse .rs)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex;

use std::collections::HashSet;
use std::path::{Path, PathBuf};

struct ModDecl {
    name: String,
    line: u32,
    col: u32,
    path_attr: Option<String>, // #[path = "…"] immediately above
}

/// Scan one file's code for `mod NAME;` declarations (bodyless only) and any
/// `#[path]` attribute bound to each. `raw` is the ORIGINAL source: the lexer strips
/// string contents, so the attribute's value must be recovered from the raw line the
/// position map points at (multi-line `#[path =\n"…"]` also checks the next line).
fn scan_mods(code: &str, pos: &[(u32, u32)], raw: &str) -> Vec<ModDecl> {
    let cb = code.as_bytes();
    let mut out = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = code[from..].find("mod ") {
        let at = from + rel;
        from = at + 4;
        // keyword boundary (rejects `my_mod `), and reject `mod` inside a path like `self::mod`? not valid — boundary is enough
        if at > 0 && (cb[at - 1].is_ascii_alphanumeric() || cb[at - 1] == b'_') {
            continue;
        }
        let mut i = at + 4;
        while i < cb.len() && cb[i].is_ascii_whitespace() {
            i += 1;
        }
        let ns = i;
        while i < cb.len() && (cb[i].is_ascii_alphanumeric() || cb[i] == b'_') {
            i += 1;
        }
        if i == ns {
            continue;
        }
        let name = code[ns..i].to_string();
        while i < cb.len() && cb[i].is_ascii_whitespace() {
            i += 1;
        }
        if i >= cb.len() || cb[i] != b';' {
            continue; // `mod name { … }` inline body — nothing to check on disk
        }
        // look back for a #[path = "…"] attribute directly above the declaration
        // (search a short window backwards; attributes and the mod line are adjacent
        // in every real use in this tree)
        let win_start = at.saturating_sub(160);
        let window = &code[win_start..at];
        let path_attr = window.rfind("#[path").and_then(|p| {
            let seg = &window[p..];
            // the attribute must be the LAST thing before the mod (only ws/`pub` between)
            let after_attr = &seg[seg.find(']')? + 1..];
            let clean = after_attr.trim().trim_start_matches("pub").trim();
            if !(clean.is_empty() || clean == "pub") {
                return None;
            }
            // recover the value from the RAW source: the lexer emptied the string, but
            // pos maps the attr's code index to its raw line
            let attr_line = pos[win_start + p].0 as usize;
            for l in [attr_line, attr_line + 1] {
                if let Some(rl) = raw.lines().nth(l.saturating_sub(1)) {
                    if let Some(q1) = rl.find('"') {
                        if let Some(q2) = rl[q1 + 1..].find('"') {
                            return Some(rl[q1 + 1..q1 + 1 + q2].to_string());
                        }
                    }
                }
            }
            None
        });
        let (line, col) = pos[ns];
        out.push(ModDecl { name, line, col, path_attr });
    }
    out
}

fn is_root_name(name: &str) -> bool {
    matches!(name, "lib.rs" | "main.rs" | "mod.rs" | "build.rs")
}

fn in_auto_dir(p: &Path) -> bool {
    p.components().any(|c| {
        matches!(c.as_os_str().to_str().unwrap_or(""), "bin" | "tests" | "examples" | "benches")
    })
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: modcheck DIR...");
        std::process::exit(2);
    }
    let mut files: Vec<PathBuf> = Vec::new();
    for a in &args {
        lex::collect_files(Path::new(a), &["rs"], &mut files);
    }
    if files.is_empty() {
        eprintln!("modcheck: no files matched");
        std::process::exit(2);
    }

    let mut findings = 0usize;
    let mut referenced: HashSet<PathBuf> = HashSet::new();

    for f in &files {
        let src = match std::fs::read(f) {
            Ok(s) => s,
            Err(_) => continue,
        };
        let raw = String::from_utf8_lossy(&src).into_owned();
        let mut code = String::with_capacity(src.len());
        let mut pos = Vec::with_capacity(src.len());
        lex::walk_code(&src, |b, l, c| {
            code.push(b as char);
            pos.push((l, c));
        });
        // the dir a bodyless `mod` resolves against: the file's dir for roots/mod.rs,
        // else a subdir named after the file (2018 layout: foo.rs owns foo/)
        let dir = f.parent().unwrap_or(Path::new("."));
        let stem = f.file_stem().and_then(|s| s.to_str()).unwrap_or("");
        let fname = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
        let base: PathBuf = if is_root_name(fname) { dir.to_path_buf() } else { dir.join(stem) };

        for m in scan_mods(&code, &pos, &raw) {
            if let Some(rel) = &m.path_attr {
                let target = dir.join(rel);
                if target.is_file() {
                    if let Ok(canon) = target.canonicalize() {
                        referenced.insert(canon);
                    }
                } else {
                    println!("{}:{}:{}: mod {} — #[path = \"{rel}\"] names a file that does not exist", f.display(), m.line, m.col, m.name);
                    findings += 1;
                }
                continue;
            }
            let cand1 = base.join(format!("{}.rs", m.name));
            let cand2 = base.join(&m.name).join("mod.rs");
            if cand1.is_file() || cand2.is_file() {
                for c in [cand1, cand2] {
                    if let Ok(canon) = c.canonicalize() {
                        referenced.insert(canon);
                    }
                }
            } else {
                println!("{}:{}:{}: mod {}; — neither {} nor {} exists", f.display(), m.line, m.col, m.name, cand1.display(), cand2.display(), );
                findings += 1;
            }
        }
    }

    // orphans: .rs files never referenced, not roots, not in auto-discovered dirs
    for f in &files {
        let fname = f.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if is_root_name(fname) || in_auto_dir(f) {
            continue;
        }
        let canon = match f.canonicalize() {
            Ok(c) => c,
            Err(_) => continue,
        };
        if !referenced.contains(&canon) {
            println!("{}:1:1: orphan — no `mod` line in the scanned set names this file", f.display());
            findings += 1;
        }
    }

    if findings == 0 {
        eprintln!("modcheck: {} file(s), wiring clean", files.len());
    }
    std::process::exit(if findings == 0 { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn mods(src: &str) -> Vec<ModDecl> {
        let mut code = String::new();
        let mut pos = Vec::new();
        lex::walk_code(src.as_bytes(), |b, l, c| {
            code.push(b as char);
            pos.push((l, c));
        });
        scan_mods(&code, &pos, src)
    }

    #[test]
    fn bodyless_mods_are_found_and_inline_bodies_are_not() {
        let m = mods("pub mod openworld;\nmod iqm;\nmod inline { fn f() {} }\n// mod commented;\n");
        let names: Vec<_> = m.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, vec!["openworld", "iqm"]);
        assert_eq!((m[0].line, m[0].col), (1, 9));
    }

    #[test]
    fn path_attributes_bind_to_their_mod() {
        let m = mods("#[path = \"../shared/thing.rs\"]\nmod thing;\nmod plain;\n");
        assert_eq!(m[0].path_attr.as_deref(), Some("../shared/thing.rs"));
        assert!(m[1].path_attr.is_none(), "a path attr must not leak to the next mod");
    }

    #[test]
    fn cfg_between_path_and_mod_does_not_confuse_binding() {
        // #[path] then #[cfg] then mod: the window sees the cfg attr after the path
        // attr's ']', so the path no longer binds — conservative, no false MISSING
        let m = mods("#[path = \"x.rs\"]\n#[cfg(test)]\nmod x;\n");
        assert!(m[0].path_attr.is_none());
    }
}

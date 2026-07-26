//! msrvcheck — std APIs newer than the MSRV you're compiling under.
//!
//! Motivating incident: `Option::is_none_or` (stabilized 1.82) written while the
//! sandbox toolchain was 1.75 — the failure surfaced only at the compile this repo was
//! trying to avoid. The table below is the curated list of "recently stabilized, easy
//! to reach for" std methods with their versions; anything newer than `--msrv`
//! (default 1.75) is flagged at its exact call site.
//!
//! Matching is `.name(` on code bytes (the shared lexer keeps strings/comments out),
//! so a same-named method on your own type is flagged too — conservative on purpose;
//! the message says which std item the name collides with so dismissal takes seconds.
//! The table is data: extend it as new APIs join the reach-for set.
//!
//! Usage: msrvcheck [--msrv 1.NN] FILE|DIR...   (dirs recurse to .rs)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex;

/// (method name, minor version stabilized, what it is) — 1.x only, table sorted by name.
const TABLE: &[(&str, u32, &str)] = &[
    ("cast_mut", 65, "pointer::cast_mut"),
    ("div_ceil", 73, "integer div_ceil"),
    ("div_floor", 88, "integer div_floor"),
    ("is_none_or", 82, "Option::is_none_or"),
    ("is_some_and", 70, "Option::is_some_and"),
    ("midpoint", 85, "numeric midpoint"),
    ("next_multiple_of", 73, "integer next_multiple_of"),
    ("split_once", 52, "str::split_once"),
    ("total_cmp", 62, "float total_cmp"),
    ("trim_ascii", 80, "slice/str trim_ascii"),
    ("unsigned_abs", 60, "integer unsigned_abs"),
];

fn check(src: &[u8], msrv_minor: u32) -> Vec<(u32, u32, String)> {
    // rebuild the code-only text WITH positions so we can string-search it
    let mut code: Vec<u8> = Vec::with_capacity(src.len());
    let mut pos: Vec<(u32, u32)> = Vec::with_capacity(src.len());
    lex::walk_code(src, |b, l, c| {
        code.push(b);
        pos.push((l, c));
    });

    let mut findings = Vec::new();
    for (name, minor, what) in TABLE {
        if *minor <= msrv_minor {
            continue;
        }
        let pat: Vec<u8> = {
            let mut v = vec![b'.'];
            v.extend_from_slice(name.as_bytes());
            v.push(b'(');
            v
        };
        let mut from = 0usize;
        while from + pat.len() <= code.len() {
            if &code[from..from + pat.len()] == pat.as_slice() {
                let (l, c) = pos[from + 1]; // point at the name, not the dot
                findings.push((l, c, format!("`.{name}(` — {what} is Rust 1.{minor}, newer than --msrv 1.{msrv_minor}")));
                from += pat.len();
            } else {
                from += 1;
            }
        }
    }
    findings.sort();
    findings
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut msrv_minor = 75u32;
    if let Some(p) = args.iter().position(|a| a == "--msrv") {
        if p + 1 >= args.len() {
            eprintln!("usage: msrvcheck [--msrv 1.NN] FILE|DIR...");
            std::process::exit(2);
        }
        let v = args.remove(p + 1);
        args.remove(p);
        let minor = v.strip_prefix("1.").unwrap_or(&v);
        msrv_minor = match minor.parse() {
            Ok(m) => m,
            Err(_) => {
                eprintln!("msrvcheck: bad --msrv '{v}' (want e.g. 1.75)");
                std::process::exit(2);
            }
        };
    }
    if args.is_empty() {
        eprintln!("usage: msrvcheck [--msrv 1.NN] FILE|DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args {
        lex::collect_files(std::path::Path::new(a), &["rs"], &mut files);
    }
    if files.is_empty() {
        eprintln!("msrvcheck: no files matched");
        std::process::exit(2);
    }
    let mut total = 0usize;
    for f in &files {
        let src = match std::fs::read(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: read error: {e}", f.display());
                total += 1;
                continue;
            }
        };
        for (l, c, msg) in check(&src, msrv_minor) {
            println!("{}:{l}:{c}: {msg}", f.display());
            total += 1;
        }
    }
    if total == 0 {
        eprintln!("msrvcheck: {} file(s) clean at 1.{msrv_minor}", files.len());
    }
    std::process::exit(if total == 0 { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_incident_is_caught_at_175_and_clean_at_196() {
        let src = b"if p.extension().is_none_or(|x| x != \"iqm\") { }";
        let f = check(src, 75);
        assert_eq!(f.len(), 1);
        assert!(f[0].2.contains("1.82"), "{}", f[0].2);
        assert!(check(src, 96).is_empty());
    }

    #[test]
    fn mentions_in_strings_and_comments_do_not_count() {
        let src = b"// .is_none_or( in a comment\nlet s = \".is_none_or(\";";
        assert!(check(src, 75).is_empty());
    }

    #[test]
    fn apis_at_or_below_msrv_pass() {
        let src = b"x.is_some_and(|v| v > 0) && s.split_once(':').is_some()";
        assert!(check(src, 75).is_empty());
    }
}

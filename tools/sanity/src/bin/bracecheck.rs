//! bracecheck — balanced `()[]{}` or the exact spot they aren't.
//!
//! Motivating incident: an extracted test harness died on rustc's "unclosed delimiter"
//! with the mismatch hundreds of lines from its cause. This answers the same question
//! in milliseconds, names the FIRST offending byte, and prints the still-open stack so
//! the missing closer is findable by eye.
//!
//! Rust-aware by default via the shared lexer (a `{` in a string, comment, or char
//! literal does not count). `--plain` skips the lexer for C/GLSL/JSON/plain text —
//! note `--plain` counts EVERY bracket byte, including ones inside string literals,
//! so prefer the default for anything string-heavy.
//!
//! Usage: bracecheck [--plain] FILE|DIR...   (dirs recurse: .rs, or with --plain also
//! .c .h .cpp .hpp .glsl .comp .vert .frag .json .toml)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex;

struct Open {
    ch: u8,
    line: u32,
    col: u32,
}

fn closes(open: u8, close: u8) -> bool {
    matches!((open, close), (b'(', b')') | (b'[', b']') | (b'{', b'}'))
}

/// Check one file's bytes; returns the findings as (line, col, message).
fn check(src: &[u8], rust_aware: bool) -> Vec<(u32, u32, String)> {
    let mut findings = Vec::new();
    let mut stack: Vec<Open> = Vec::new();

    let mut handle = |b: u8, line: u32, col: u32, stack: &mut Vec<Open>, findings: &mut Vec<(u32, u32, String)>| {
        match b {
            b'(' | b'[' | b'{' => stack.push(Open { ch: b, line, col }),
            b')' | b']' | b'}' => match stack.pop() {
                Some(o) if closes(o.ch, b) => {}
                Some(o) => {
                    findings.push((
                        line,
                        col,
                        format!(
                            "'{}' closes '{}' opened at {}:{} — mismatched pair",
                            b as char, o.ch as char, o.line, o.col
                        ),
                    ));
                    // put nothing back: treat as consumed, keep scanning for more signal
                }
                None => findings.push((line, col, format!("'{}' with nothing open", b as char))),
            },
            _ => {}
        }
    };

    if rust_aware {
        lex::walk_code(src, |b, l, c| handle(b, l, c, &mut stack, &mut findings));
    } else {
        let mut line: u32 = 1;
        let mut col: u32 = 1;
        for &b in src {
            handle(b, line, col, &mut stack, &mut findings);
            if b == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
        }
    }

    // whatever is still open at EOF is a finding, innermost last so the likeliest
    // culprit (deepest unclosed) prints closest to the eye
    for o in &stack {
        findings.push((o.line, o.col, format!("'{}' opened here is never closed", o.ch as char)));
    }
    findings
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let plain = if let Some(p) = args.iter().position(|a| a == "--plain") {
        args.remove(p);
        true
    } else {
        false
    };
    if args.is_empty() {
        eprintln!("usage: bracecheck [--plain] FILE|DIR...");
        std::process::exit(2);
    }
    let exts: &[&str] = if plain {
        &["rs", "c", "h", "cpp", "hpp", "glsl", "comp", "vert", "frag", "json", "toml"]
    } else {
        &["rs"]
    };
    let mut files = Vec::new();
    for a in &args {
        lex::collect_files(std::path::Path::new(a), exts, &mut files);
    }
    if files.is_empty() {
        eprintln!("bracecheck: no files matched");
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
        for (l, c, msg) in check(&src, !plain) {
            println!("{}:{l}:{c}: {msg}", f.display());
            total += 1;
        }
    }
    if total == 0 {
        eprintln!("bracecheck: {} file(s) balanced", files.len());
    }
    std::process::exit(if total == 0 { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn balanced_code_is_clean_even_with_bracket_soup_in_strings() {
        let src = br#"fn f() { let s = "([{"; let c = '{'; g([1, 2], (3)); }"#;
        assert!(check(src, true).is_empty());
    }

    #[test]
    fn the_first_mismatch_is_located_exactly() {
        let f = check(b"fn f() { (] }", true);
        assert_eq!(f.len(), 1);
        let (line, col, msg) = &f[0];
        assert_eq!((*line, *col), (1, 11));
        assert!(msg.contains("closes '('"), "{msg}");
    }

    #[test]
    fn unclosed_at_eof_reports_the_opener() {
        let f = check(b"fn f() {\n  g(\n", true);
        assert_eq!(f.len(), 2); // the { and the (
        assert!(f.iter().any(|(l, c, m)| *l == 1 && *c == 8 && m.contains("never closed")));
        assert!(f.iter().any(|(l, c, m)| *l == 2 && *c == 4 && m.contains("never closed")));
    }

    #[test]
    fn plain_mode_counts_string_contents_too() {
        // in plain mode the { inside the string is a real finding — documented behavior
        let f = check(br#"x = "{";"#, false);
        assert_eq!(f.len(), 1);
    }

    #[test]
    fn stray_closer_is_reported() {
        let f = check(b"}", true);
        assert_eq!(f.len(), 1);
        assert!(f[0].2.contains("nothing open"));
    }
}

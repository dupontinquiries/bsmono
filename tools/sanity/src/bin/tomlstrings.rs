//! tomlstrings — TOML strings that don't close on their line.
//!
//! Motivating incident: manifest/projects.toml shipped a basic string ending `…\"` —
//! the backslash escaped the intended closing quote, the "string" swallowed the rest
//! of the file's structure, and only Python's tomllib said so. This is the Rust gate
//! for exactly that class: within a line, every `"…"` (escape-aware) and `'…'`
//! (literal, no escapes) must close; multi-line `"""`/`'''` blocks are tracked and
//! skipped. Comments (`#`, outside strings) end scanning for their line.
//!
//! This is NOT a TOML parser — keys, tables, and values are not validated (that's
//! tomllib's job at build time). It is the string-closure lint that catches the
//! trailing-escape bug the instant the file is saved.
//!
//! Usage: tomlstrings FILE|DIR...   (dirs recurse to .toml)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex; // only for collect_files; TOML lexing is its own tiny rules below

#[derive(PartialEq, Clone, Copy)]
enum Multi {
    None,
    Basic,   // """
    Literal, // '''
}

fn check(src: &str) -> Vec<(u32, u32, String)> {
    let mut findings = Vec::new();
    let mut multi = Multi::None;

    for (li, line) in src.lines().enumerate() {
        let ln = li as u32 + 1;
        let b = line.as_bytes();
        let mut i = 0usize;

        // inside a multi-line string: only its terminator matters on this line
        if multi != Multi::None {
            let term: &[u8] = if multi == Multi::Basic { b"\"\"\"" } else { b"'''" };
            let mut j = 0usize;
            let mut closed = false;
            while j + 3 <= b.len() {
                // in basic multiline, a backslash escapes — but only the quote kind cares
                if multi == Multi::Basic && b[j] == b'\\' {
                    j += 2;
                    continue;
                }
                if &b[j..j + 3] == term {
                    closed = true;
                    i = j + 3;
                    break;
                }
                j += 1;
            }
            if !closed {
                continue; // still inside the block on the next line — fine
            }
            multi = Multi::None;
            // fall through to scan the remainder of the line from i
        }

        while i < b.len() {
            let c = b[i];
            if c == b'#' {
                break; // comment to EOL
            }
            if c == b'"' || c == b'\'' {
                // multi-line opener?
                let trip: &[u8] = if c == b'"' { b"\"\"\"" } else { b"'''" };
                if i + 3 <= b.len() && &b[i..i + 3] == trip {
                    // does it also CLOSE on this line?
                    let mut j = i + 3;
                    let mut closed = false;
                    while j + 3 <= b.len() {
                        if c == b'"' && b[j] == b'\\' {
                            j += 2;
                            continue;
                        }
                        if &b[j..j + 3] == trip {
                            closed = true;
                            break;
                        }
                        j += 1;
                    }
                    if closed {
                        i = j + 3;
                        continue;
                    }
                    multi = if c == b'"' { Multi::Basic } else { Multi::Literal };
                    break; // rest of the line is string content
                }
                // single-line string: find the closer with escape rules
                let open_col = i as u32 + 1;
                let mut j = i + 1;
                let mut closed = false;
                while j < b.len() {
                    if c == b'"' && b[j] == b'\\' {
                        j += 2; // escape eats the next byte — THE bug this tool exists for
                        continue;
                    }
                    if b[j] == c {
                        closed = true;
                        break;
                    }
                    j += 1;
                }
                if !closed {
                    let hint = if c == b'"' && b.len() >= 2 && b[b.len() - 2] == b'\\' && b[b.len() - 1] == b'"' {
                        " (the line ends \\\" — the backslash escapes what was meant to close it)"
                    } else {
                        ""
                    };
                    findings.push((ln, open_col, format!("{} string opened here never closes on its line{hint}", if c == b'"' { "basic" } else { "literal" })));
                    break;
                }
                i = j + 1;
                continue;
            }
            i += 1;
        }
    }
    if multi != Multi::None {
        findings.push((src.lines().count() as u32, 1, "multi-line string still open at end of file".to_string()));
    }
    findings
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: tomlstrings FILE|DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args {
        lex::collect_files(std::path::Path::new(a), &["toml"], &mut files);
    }
    if files.is_empty() {
        eprintln!("tomlstrings: no files matched");
        std::process::exit(2);
    }
    let mut total = 0usize;
    for f in &files {
        let src = match std::fs::read_to_string(f) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: read error: {e}", f.display());
                total += 1;
                continue;
            }
        };
        for (l, c, msg) in check(&src) {
            println!("{}:{l}:{c}: {msg}", f.display());
            total += 1;
        }
    }
    if total == 0 {
        eprintln!("tomlstrings: {} file(s) clean", files.len());
    }
    std::process::exit(if total == 0 { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn clean_toml_is_clean() {
        let s = "name = \"ok\"\npath = 'also ok'\n# comment \" with quote\nnotes = \"escaped \\\" quote inside\"\n";
        assert!(check(s).is_empty());
    }

    #[test]
    fn the_manifest_bug_is_caught_with_the_hint() {
        let s = "notes = \"ends with an escape\\\"\n";
        let f = check(s);
        assert_eq!(f.len(), 1);
        assert!(f[0].2.contains("backslash escapes"), "{}", f[0].2);
        assert_eq!((f[0].0, f[0].1), (1, 9));
    }

    #[test]
    fn multiline_blocks_span_lines_without_findings() {
        let s = "a = \"\"\"\nline \" one\nline ' two\n\"\"\"\nb = 1\n";
        assert!(check(s).is_empty());
        let s2 = "a = '''\nnever closed\n";
        let f = check(s2);
        assert_eq!(f.len(), 1);
        assert!(f[0].2.contains("end of file"));
    }

    #[test]
    fn literal_strings_take_no_escapes() {
        // in a literal string the backslash is just a byte; this one closes fine
        assert!(check("p = 'C:\\dir\\'\n").is_empty());
    }
}

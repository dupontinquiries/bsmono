//! lex.rs — the shared minimal Rust lexer: walk a source file and hand each byte that
//! is REAL CODE (not comment, not string/char-literal content) to a callback, with its
//! line and column. Every sanity primitive builds on this so that a `{` inside a string
//! or a `//` inside a URL never confuses a check.
//!
//! Handled, because each has burned somebody: line comments; block comments WITH
//! NESTING (Rust allows `/* /* */ */`); `"..."` strings with `\` escapes; raw strings
//! `r"..."` / `r#"..."#` / more hashes, and their `br` byte forms; `b"..."` byte
//! strings; char literals `'x'`, `'\n'`, `'\u{1F600}'`; and the lifetime/char
//! ambiguity — `'a` in `<'a>` is NOT an unterminated char literal. The disambiguation
//! is the standard bounded attempt: at a `'`, try to lex a char literal; if no closing
//! `'` appears where one must, it was a lifetime and the tick is passed through as
//! code.
//!
//! Deliberately NOT handled: nothing else. This is a lexer for sanity gates, not a
//! parser; keeping it small is what keeps it trustworthy without a compiler run.
//! Kept 1.75-compatible (plain loops, no post-1.75 std APIs) so it builds under either
//! in-sandbox toolchain.

/// Walk `src`, calling `f(byte, line, col)` for every byte that is code (including the
/// quotes that DELIMIT strings — callers who only care about brackets simply ignore
/// other bytes; callers like tomlstrings want the delimiters). Content INSIDE strings,
/// chars, and comments is skipped. Lines are 1-based, cols are 1-based bytes.
pub fn walk_code<F: FnMut(u8, u32, u32)>(src: &[u8], mut f: F) {
    let n = src.len();
    let mut i = 0usize;
    let mut line: u32 = 1;
    let mut col: u32 = 1;

    // advance the cursor over src[i], maintaining line/col
    macro_rules! bump {
        () => {{
            if src[i] == b'\n' {
                line += 1;
                col = 1;
            } else {
                col += 1;
            }
            i += 1;
        }};
    }

    while i < n {
        let c = src[i];

        // ── comments ────────────────────────────────────────────────────────────
        if c == b'/' && i + 1 < n && src[i + 1] == b'/' {
            while i < n && src[i] != b'\n' {
                bump!();
            }
            continue; // the \n itself goes through as code next iteration
        }
        if c == b'/' && i + 1 < n && src[i + 1] == b'*' {
            bump!();
            bump!();
            let mut depth = 1u32;
            while i < n && depth > 0 {
                if src[i] == b'/' && i + 1 < n && src[i + 1] == b'*' {
                    depth += 1;
                    bump!();
                    bump!();
                } else if src[i] == b'*' && i + 1 < n && src[i + 1] == b'/' {
                    depth -= 1;
                    bump!();
                    bump!();
                } else {
                    bump!();
                }
            }
            continue;
        }

        // ── raw strings: r"…", r#"…"#, br#"…"# … ────────────────────────────────
        // (must be checked before plain strings and before identifiers eat the r)
        let (is_raw, raw_skip) = if c == b'r' && i + 1 < n && (src[i + 1] == b'"' || src[i + 1] == b'#') {
            (true, 1usize)
        } else if c == b'b' && i + 2 < n && src[i + 1] == b'r' && (src[i + 2] == b'"' || src[i + 2] == b'#') {
            (true, 2usize)
        } else {
            (false, 0)
        };
        if is_raw {
            // don't treat `r` in an identifier like `for` as a raw-string start:
            // only when the previous byte is not an identifier byte
            let prev_ident = i > 0 && (src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_');
            if !prev_ident {
                // count hashes
                let mut j = i + raw_skip;
                let mut hashes = 0usize;
                while j < n && src[j] == b'#' {
                    hashes += 1;
                    j += 1;
                }
                if j < n && src[j] == b'"' {
                    // emit the opening delimiter region as code? No: strings are opaque;
                    // emit ONLY the two quote bytes so quote-counting tools still see a
                    // balanced pair, brackets inside stay hidden.
                    f(b'"', line, col);
                    // advance to just past the opening quote
                    while i <= j {
                        bump!();
                    }
                    // scan for closing  "###… with `hashes` hashes
                    'raw: while i < n {
                        if src[i] == b'"' {
                            let mut k = i + 1;
                            let mut got = 0usize;
                            while k < n && got < hashes && src[k] == b'#' {
                                got += 1;
                                k += 1;
                            }
                            if got == hashes {
                                f(b'"', line, col);
                                while i < k {
                                    bump!();
                                }
                                break 'raw;
                            }
                        }
                        bump!();
                    }
                    continue;
                }
            }
        }

        // ── plain / byte strings ────────────────────────────────────────────────
        if c == b'"' || (c == b'b' && i + 1 < n && src[i + 1] == b'"' && !(i > 0 && (src[i - 1].is_ascii_alphanumeric() || src[i - 1] == b'_'))) {
            if c == b'b' {
                bump!(); // the b prefix is skipped; quote handled below
            }
            f(b'"', line, col);
            bump!(); // opening quote
            while i < n {
                if src[i] == b'\\' && i + 1 < n {
                    bump!();
                    bump!();
                    continue;
                }
                if src[i] == b'"' {
                    f(b'"', line, col);
                    bump!();
                    break;
                }
                bump!();
            }
            continue;
        }

        // ── char literal vs lifetime ────────────────────────────────────────────
        if c == b'\'' {
            // try to lex a char literal with bounded lookahead
            let mut j = i + 1;
            let mut ok = false;
            if j < n && src[j] == b'\\' {
                // escape: \x41, \n, \u{...}
                j += 1;
                if j < n && src[j] == b'u' {
                    j += 1;
                    if j < n && src[j] == b'{' {
                        j += 1;
                        while j < n && src[j] != b'}' && j - i < 12 {
                            j += 1;
                        }
                        if j < n && src[j] == b'}' {
                            j += 1;
                        }
                    }
                } else if j < n && src[j] == b'x' {
                    j += 3; // x + two hex digits
                } else {
                    j += 1; // simple escape char
                }
                ok = j < n && src[j] == b'\'';
            } else if j < n && src[j] != b'\'' {
                // one UTF-8 scalar: advance over continuation bytes
                j += 1;
                while j < n && (src[j] & 0xC0) == 0x80 {
                    j += 1;
                }
                ok = j < n && src[j] == b'\'';
            }
            if ok {
                // a real char literal: opaque, emit nothing (no bracket/quote semantics
                // needed from it), skip past the closing tick
                while i <= j {
                    bump!();
                }
                continue;
            }
            // lifetime (or stray tick): fall through as code
        }

        f(c, line, col);
        bump!();
    }
}

/// Recursively collect files under `path` with any of `exts` (no dot). A bare file is
/// taken as-is regardless of extension — naming a file means you meant it.
pub fn collect_files(path: &std::path::Path, exts: &[&str], out: &mut Vec<std::path::PathBuf>) {
    if path.is_file() {
        out.push(path.to_path_buf());
        return;
    }
    let rd = match std::fs::read_dir(path) {
        Ok(r) => r,
        Err(_) => return,
    };
    let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
    entries.sort();
    for p in entries {
        let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
        if p.is_dir() {
            if name == "target" || name == ".git" || name == "node_modules" || name == "build" {
                continue;
            }
            collect_files(&p, exts, out);
        } else if let Some(e) = p.extension().and_then(|s| s.to_str()) {
            if exts.iter().any(|x| *x == e) {
                out.push(p);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn code_only(src: &str) -> String {
        let mut s = String::new();
        walk_code(src.as_bytes(), |b, _, _| s.push(b as char));
        s
    }

    #[test]
    fn comments_are_stripped_including_nested_blocks() {
        assert_eq!(code_only("a // x {\nb"), "a \nb");
        assert_eq!(code_only("a /* { /* [ */ } */ b"), "a  b");
    }

    #[test]
    fn strings_hide_their_brackets_but_keep_their_quotes() {
        assert_eq!(code_only(r#"f("{[(")"#), "f(\"\")");
        assert_eq!(code_only("x = b\"[[[\";"), "x = \"\";");
    }

    #[test]
    fn raw_strings_with_hashes_are_opaque() {
        assert_eq!(code_only("r#\"{ \" }\"#;"), "\"\";");
        assert_eq!(code_only("br##\"a\"# b\"##;"), "\"\";");
        // an identifier ending in r followed by a quote is NOT a raw string
        assert_eq!(code_only("for\"x\""), "for\"\"");
    }

    #[test]
    fn char_literals_are_opaque_and_lifetimes_pass_through() {
        assert_eq!(code_only("let c = '{';"), "let c = ;");
        assert_eq!(code_only("let c = '\\n';"), "let c = ;");
        assert_eq!(code_only("fn f<'a>(x: &'a str)"), "fn f<'a>(x: &'a str)");
    }

    #[test]
    fn line_and_col_are_tracked() {
        let mut hits = Vec::new();
        walk_code(b"ab\ncd", |b, l, c| hits.push((b, l, c)));
        assert_eq!(hits, vec![(b'a', 1, 1), (b'b', 1, 2), (b'\n', 1, 3), (b'c', 2, 1), (b'd', 2, 2)]);
    }
}

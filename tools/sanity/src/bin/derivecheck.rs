//! derivecheck — `#[derive(Eq)]` / `#[derive(Hash)]` on a type with `f32`/`f64` fields.
//!
//! Motivating incident: `MoeConfig` gained `scale: f32` and its long-standing
//! `#[derive(… PartialEq, Eq)]` stopped compiling — caught only by the extracted test
//! harness, one full edit-cycle late. Floats implement neither `Eq` (NaN ≠ NaN) nor
//! `Hash`; the derive is legal to WRITE and only explodes at expansion, which makes it
//! exactly the mistake a no-compile session ships.
//!
//! Scope, honestly: the scanner reads the item immediately following each derive
//! attribute (struct with named fields, tuple struct, or enum with data variants) at
//! its top nesting level. `f32`/`f64` buried inside another named type (e.g. a `Vec3`
//! that itself lacks `Eq`) is the compiler's to find — this gate catches the direct
//! case, which is the one that actually happened.
//!
//! Usage: derivecheck FILE|DIR...   (dirs recurse to .rs)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex;

fn check(src: &[u8]) -> Vec<(u32, u32, String)> {
    // code-only text with positions
    let mut code = String::with_capacity(src.len());
    let mut pos: Vec<(u32, u32)> = Vec::with_capacity(src.len());
    lex::walk_code(src, |b, l, c| {
        code.push(b as char);
        pos.push((l, c));
    });
    let cb = code.as_bytes();

    let mut findings = Vec::new();
    let mut from = 0usize;
    while let Some(rel) = code[from..].find("#[derive(") {
        let at = from + rel;
        from = at + 9;
        // derive list up to its closing paren (derives don't nest parens in practice,
        // but count anyway so `derive(Foo(bar))`-shaped custom paths can't desync us)
        let mut depth = 1i32;
        let mut j = at + 9;
        while j < cb.len() && depth > 0 {
            match cb[j] {
                b'(' => depth += 1,
                b')' => depth -= 1,
                _ => {}
            }
            j += 1;
        }
        let list = &code[at + 9..j.saturating_sub(1)];
        let has_bad = list.split(',').any(|t| {
            let t = t.trim();
            t == "Eq" || t == "Hash" || t.ends_with("::Eq") || t.ends_with("::Hash")
        });
        if !has_bad {
            continue;
        }
        let which: Vec<&str> = ["Eq", "Hash"]
            .iter()
            .filter(|w| list.split(',').any(|t| { let t = t.trim(); t == **w || t.ends_with(&format!("::{w}")) }))
            .copied()
            .collect();

        // the item after the derive: skip further attributes and visibility, find the
        // struct/enum body braces (a tuple struct's parens), scan ITS top level
        let mut k = j;
        loop {
            // skip whitespace
            while k < cb.len() && cb[k].is_ascii_whitespace() {
                k += 1;
            }
            // another attribute? skip its bracketed body
            if k + 1 < cb.len() && cb[k] == b'#' && cb[k + 1] == b'[' {
                let mut d = 0i32;
                while k < cb.len() {
                    match cb[k] {
                        b'[' => d += 1,
                        b']' => {
                            d -= 1;
                            if d == 0 {
                                k += 1;
                                break;
                            }
                        }
                        _ => {}
                    }
                    k += 1;
                }
                continue;
            }
            break;
        }
        // find the body opener: '{' (named/enum) or '(' (tuple) — whichever comes
        // first before a ';' (a unit struct has neither and carries no floats)
        let mut opener = 0u8;
        let mut body_start = 0usize;
        {
            let mut m = k;
            while m < cb.len() {
                match cb[m] {
                    b'{' | b'(' => {
                        opener = cb[m];
                        body_start = m + 1;
                        break;
                    }
                    b';' => break,
                    _ => {}
                }
                m += 1;
            }
        }
        if opener == 0 {
            continue; // unit struct / not an item we scan
        }
        let closer = if opener == b'{' { b'}' } else { b')' };
        // collect the body at top level and look for f32/f64 as whole type tokens
        let mut d = 1i32;
        let mut m = body_start;
        while m < cb.len() && d > 0 {
            let c = cb[m];
            if c == opener || (opener == b'{' && c == b'{') || (opener == b'(' && c == b'(') {
                d += 1;
            } else if c == closer {
                d -= 1;
                if d == 0 {
                    break;
                }
            } else if d <= 2 && (c == b'f') && m + 3 <= cb.len() {
                // depth 1 = struct fields / tuple fields; depth 2 = enum variant
                // payloads (`B { v: f32 }`, `A(f64)`). Deeper is out of scope.
                let tok = &code[m..(m + 3).min(code.len())];
                let before_ok = m == 0 || !(cb[m - 1].is_ascii_alphanumeric() || cb[m - 1] == b'_');
                let after = cb.get(m + 3).copied().unwrap_or(b' ');
                let after_ok = !(after.is_ascii_alphanumeric() || after == b'_');
                if before_ok && after_ok && (tok == "f32" || tok == "f64") {
                    let (l, c2) = pos[m];
                    findings.push((l, c2, format!("`{tok}` field under #[derive({})] — floats implement neither", which.join(", "))));
                }
            }
            m += 1;
        }
    }
    findings
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: derivecheck FILE|DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args {
        lex::collect_files(std::path::Path::new(a), &["rs"], &mut files);
    }
    if files.is_empty() {
        eprintln!("derivecheck: no files matched");
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
        for (l, c, msg) in check(&src) {
            println!("{}:{l}:{c}: {msg}", f.display());
            total += 1;
        }
    }
    if total == 0 {
        eprintln!("derivecheck: {} file(s) clean", files.len());
    }
    std::process::exit(if total == 0 { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_moeconfig_incident_is_caught() {
        let src = b"#[derive(Clone, Copy, Debug, PartialEq, Eq)]\npub struct MoeConfig {\n    pub n_routed: u32,\n    pub scale: f32,\n}\n";
        let f = check(src);
        assert_eq!(f.len(), 1);
        assert_eq!(f[0].0, 4);
        assert!(f[0].2.contains("Eq"), "{}", f[0].2);
    }

    #[test]
    fn partialeq_alone_is_fine_and_so_are_int_fields() {
        assert!(check(b"#[derive(PartialEq)]\nstruct S { x: f32 }\n").is_empty());
        assert!(check(b"#[derive(Eq, Hash)]\nstruct S { x: u32, y: i64 }\n").is_empty());
    }

    #[test]
    fn tuple_structs_and_enums_are_scanned() {
        let f = check(b"#[derive(Hash)]\nstruct P(f64, u8);\n");
        assert_eq!(f.len(), 1);
        let f2 = check(b"#[derive(Eq, PartialEq)]\nenum E { A(u8), B { v: f32 } }\n");
        assert_eq!(f2.len(), 1);
    }

    #[test]
    fn nested_generics_do_not_hide_at_depth_and_floats_below_top_level_are_skipped() {
        // Vec<f32> at top level of the field list: the f32 sits inside <> which this
        // scanner does NOT descend into for named structs? It stays at depth 1 (angle
        // brackets are not counted), so the token IS found — that's the conservative
        // side and it is correct to flag: Vec<f32> is not Eq either.
        let f = check(b"#[derive(Eq)]\nstruct S { v: Vec<f32> }\n");
        assert_eq!(f.len(), 1);
        // but a float inside a NESTED braced item (a const fn body in an impl that
        // follows) is out of scope — the body scan ends at the matching brace
        assert!(check(b"#[derive(Eq)]\nstruct S { x: u8 }\nfn f() -> f32 { 0.0 }\n").is_empty());
    }
}

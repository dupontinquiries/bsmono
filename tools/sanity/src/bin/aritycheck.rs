//! aritycheck — call sites whose argument count no longer matches the definition.
//!
//! Motivating incident: `moe_config` grew a fifth parameter and four test call sites
//! kept passing four arguments — invisible until the extracted harness compiled, one
//! full cycle late. This finds that drift statically: pass 1 collects every
//! `fn name(params)` in the scanned set with its parameter count; pass 2 finds every
//! direct call `name(args)` and compares counts.
//!
//! Heuristics, stated plainly (this is a scanner, not a resolver):
//!   • only names defined EXACTLY ONCE in the scanned set are checked — two `fn new`
//!     in different modules would false-positive, so duplicates are skipped;
//!   • method calls `.name(` are checked only when the definition's first parameter
//!     is `self`-like (`self`, `&self`, `&mut self`), and the self parameter is not
//!     counted; free calls `name(` are checked against non-method definitions;
//!   • macro invocations `name!(`, definitions themselves, `fn`-pointer mentions
//!     without parens, and paths ending in the name (`m::name(`) all count as calls
//!     of the same single definition — that's the point of the single-definition rule;
//!   • counting is top-level commas inside the parens with (), [], {}, and |…|
//!     closure pipes tracked; `..` struct spread and turbofish don't add commas at
//!     top level, generic angle brackets are NOT tracked (a `foo::<A, B>(x)` turbofish
//!     comma sits inside `::<…>` — mitigated by skipping `::<` runs explicitly);
//!   • trait-default vs impl overrides, cfg-gated twins, and closures assigned to a
//!     variable of the same name are beyond static reach — the compiler owns those.
//!
//! Usage: aritycheck FILE|DIR...   (dirs recurse to .rs; defs and calls are matched
//! across the whole scanned set, so point it at a crate's src/)
//! Output: file:line:col: message      Exit: 0 clean, 1 findings, 2 usage.

#[path = "../lex.rs"]
mod lex;

struct CodeFile {
    path: std::path::PathBuf,
    code: String,
    pos: Vec<(u32, u32)>,
}

struct Def {
    params: usize,
    takes_self: bool,
    count: usize, // how many definitions share the name (only count==1 is enforced)
}

fn is_ident_byte(b: u8) -> bool {
    b.is_ascii_alphanumeric() || b == b'_'
}

/// Count top-level arguments between `open_idx` (the '(') and its matching ')'.
/// Returns (count, end_index_of_close_paren). Empty parens count 0.
fn count_args(cb: &[u8], open_idx: usize) -> (usize, usize) {
    let mut depth = 1i32;
    let mut i = open_idx + 1;
    let mut commas = 0usize;
    let mut any = false;
    let mut in_pipes = false; // |a, b| closure params — their commas are not ours
    let mut prev_sig: u8 = b'('; // previous significant (non-ws) byte at any depth
    while i < cb.len() && depth > 0 {
        let c = cb[i];
        let depth_before = depth;
        match c {
            b'(' | b'[' | b'{' => depth += 1,
            b')' | b']' | b'}' => depth -= 1,
            b'|' if depth == 1 => {
                if in_pipes {
                    in_pipes = false; // closing pipe of |…|
                } else if i + 1 < cb.len() && cb[i + 1] == b'|' {
                    // logical-or or empty-closure `||`: net-neutral, skip the pair
                    i += 2;
                    prev_sig = b'|';
                    continue;
                } else if matches!(prev_sig, b'(' | b',' | b'=' | b'{' | b'[' | b'>' | b'&') {
                    in_pipes = true; // a closure can only START where an expression starts
                }
                // otherwise: bitwise `|` between operands — no state change
            }
            b':' if depth == 1 && i + 1 < cb.len() && cb[i + 1] == b':' && i + 2 < cb.len() && cb[i + 2] == b'<' => {
                // turbofish ::< … > — skip to the matching '>' (angle depth)
                let mut ad = 1i32;
                let mut j = i + 3;
                while j < cb.len() && ad > 0 {
                    match cb[j] {
                        b'<' => ad += 1,
                        b'>' => ad -= 1,
                        _ => {}
                    }
                    j += 1;
                }
                i = j;
                prev_sig = b'>';
                continue;
            }
            b',' if depth == 1 && !in_pipes => commas += 1,
            _ => {}
        }
        // content flag: anything non-ws at top level except the terminating ')'
        // (measured BEFORE the depth change, so a nested-only arg like `f((a))` counts)
        if depth_before == 1 && !c.is_ascii_whitespace() && !(c == b')' && depth == 0) {
            any = true;
        }
        if !c.is_ascii_whitespace() {
            prev_sig = c;
        }
        if depth == 0 {
            break;
        }
        i += 1;
    }
    let count = if !any { 0 } else { commas + 1 };
    (count, i)
}

fn scan_defs(files: &[CodeFile], defs: &mut std::collections::HashMap<String, Def>) {
    for f in files {
        let cb = f.code.as_bytes();
        let mut from = 0usize;
        while let Some(rel) = f.code[from..].find("fn ") {
            let at = from + rel;
            from = at + 3;
            // must be the keyword: previous byte not ident (handles `pub fn`, line start)
            if at > 0 && is_ident_byte(cb[at - 1]) {
                continue;
            }
            // name
            let mut i = at + 3;
            while i < cb.len() && cb[i].is_ascii_whitespace() {
                i += 1;
            }
            let name_start = i;
            while i < cb.len() && is_ident_byte(cb[i]) {
                i += 1;
            }
            if i == name_start {
                continue;
            }
            let name = f.code[name_start..i].to_string();
            // generics before the paren: skip <…> if present
            while i < cb.len() && cb[i].is_ascii_whitespace() {
                i += 1;
            }
            if i < cb.len() && cb[i] == b'<' {
                let mut ad = 1i32;
                i += 1;
                while i < cb.len() && ad > 0 {
                    match cb[i] {
                        b'<' => ad += 1,
                        b'>' => ad -= 1,
                        _ => {}
                    }
                    i += 1;
                }
            }
            while i < cb.len() && cb[i].is_ascii_whitespace() {
                i += 1;
            }
            if i >= cb.len() || cb[i] != b'(' {
                continue;
            }
            let (mut params, _end) = count_args(cb, i);
            // self detection: first non-ws run inside the parens
            let mut j = i + 1;
            while j < cb.len() && cb[j].is_ascii_whitespace() {
                j += 1;
            }
            let head_end = (j + 24).min(cb.len());
            let head = &f.code[j..head_end];
            // normalize `& 'a mut self` shapes: strip leading &, a lifetime tick+ident,
            // `mut`, and whitespace, then require `self` at a word boundary — so a free
            // fn whose first param is `&'a str` is NOT mistaken for a method.
            let takes_self = {
                let mut h = head;
                h = h.trim_start();
                h = h.strip_prefix('&').unwrap_or(h).trim_start();
                if let Some(rest) = h.strip_prefix('\'') {
                    let end = rest.find(|ch: char| !(ch.is_ascii_alphanumeric() || ch == '_')).unwrap_or(rest.len());
                    h = rest[end..].trim_start();
                }
                h = h.strip_prefix("mut ").unwrap_or(h).trim_start();
                h.starts_with("self")
                    && !h[4..].chars().next().map(|ch| ch.is_ascii_alphanumeric() || ch == '_').unwrap_or(false)
            };
            if takes_self && params > 0 {
                params -= 1;
            }
            let e = defs.entry(name).or_insert(Def { params, takes_self, count: 0 });
            e.count += 1;
            // if the counts disagree between duplicate defs, count>1 disables checking anyway
            if e.count == 1 {
                e.params = params;
                e.takes_self = takes_self;
            }
        }
    }
}

fn check_calls(files: &[CodeFile], defs: &std::collections::HashMap<String, Def>) -> Vec<(std::path::PathBuf, u32, u32, String)> {
    let mut findings = Vec::new();
    for f in files {
        let cb = f.code.as_bytes();
        for (name, def) in defs {
            if def.count != 1 {
                continue;
            }
            let nb = name.as_bytes();
            let mut from = 0usize;
            while from + nb.len() < cb.len() {
                let Some(rel) = f.code[from..].find(name.as_str()) else { break };
                let at = from + rel;
                from = at + nb.len();
                // whole-word check
                if at > 0 && is_ident_byte(cb[at - 1]) {
                    continue;
                }
                let after = at + nb.len();
                if after >= cb.len() || is_ident_byte(cb[after]) {
                    continue;
                }
                // skip the definition itself: preceded by `fn ` (allowing whitespace)
                {
                    let mut k = at;
                    while k > 0 && cb[k - 1].is_ascii_whitespace() {
                        k -= 1;
                    }
                    if k >= 2 && &cb[k - 2..k] == b"fn" && (k < 3 || !is_ident_byte(cb[k - 3])) {
                        continue;
                    }
                }
                // macro? name!(
                if cb[after] == b'!' {
                    continue;
                }
                // method vs free call form
                let dotted = at > 0 && cb[at - 1] == b'.';
                if dotted != def.takes_self {
                    continue; // free fn used as path in method position or vice versa — out of scope
                }
                // generics on the call: name::<…>( — skip the turbofish to the paren
                let mut p = after;
                while p < cb.len() && cb[p].is_ascii_whitespace() {
                    p += 1;
                }
                if p + 2 < cb.len() && cb[p] == b':' && cb[p + 1] == b':' && cb[p + 2] == b'<' {
                    let mut ad = 1i32;
                    let mut j = p + 3;
                    while j < cb.len() && ad > 0 {
                        match cb[j] {
                            b'<' => ad += 1,
                            b'>' => ad -= 1,
                            _ => {}
                        }
                        j += 1;
                    }
                    p = j;
                    while p < cb.len() && cb[p].is_ascii_whitespace() {
                        p += 1;
                    }
                }
                if p >= cb.len() || cb[p] != b'(' {
                    continue; // a mention, not a call
                }
                let (got, _) = count_args(cb, p);
                if got != def.params {
                    let (l, c) = f.pos[at];
                    findings.push((
                        f.path.clone(),
                        l,
                        c,
                        format!("`{name}` called with {got} arg(s); its one definition takes {}", def.params),
                    ));
                }
            }
        }
    }
    findings.sort();
    findings
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    if args.is_empty() {
        eprintln!("usage: aritycheck FILE|DIR...");
        std::process::exit(2);
    }
    let mut paths = Vec::new();
    for a in &args {
        lex::collect_files(std::path::Path::new(a), &["rs"], &mut paths);
    }
    if paths.is_empty() {
        eprintln!("aritycheck: no files matched");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for p in paths {
        let src = match std::fs::read(&p) {
            Ok(s) => s,
            Err(e) => {
                eprintln!("{}: read error: {e}", p.display());
                continue;
            }
        };
        let mut code = String::with_capacity(src.len());
        let mut pos = Vec::with_capacity(src.len());
        lex::walk_code(&src, |b, l, c| {
            code.push(b as char);
            pos.push((l, c));
        });
        files.push(CodeFile { path: p, code, pos });
    }
    let mut defs = std::collections::HashMap::new();
    scan_defs(&files, &mut defs);
    let findings = check_calls(&files, &defs);
    for (p, l, c, msg) in &findings {
        println!("{}:{l}:{c}: {msg}", p.display());
    }
    if findings.is_empty() {
        eprintln!("aritycheck: {} file(s), {} single-definition name(s) checked, clean", files.len(), defs.values().filter(|d| d.count == 1).count());
    }
    std::process::exit(if findings.is_empty() { 0 } else { 1 });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn run(src: &str) -> Vec<String> {
        let mut code = String::new();
        let mut pos = Vec::new();
        lex::walk_code(src.as_bytes(), |b, l, c| {
            code.push(b as char);
            pos.push((l, c));
        });
        let files = vec![CodeFile { path: "t.rs".into(), code, pos }];
        let mut defs = std::collections::HashMap::new();
        scan_defs(&files, &mut defs);
        check_calls(&files, &defs).into_iter().map(|(_, _, _, m)| m).collect()
    }

    #[test]
    fn the_moe_config_incident_is_caught() {
        let src = "fn moe_config(a: u32, b: u32, c: u32, d: u32, e: Meta) -> X { x() }\nfn t() { let m = moe_config(1, 2, 3, 4); }\nfn x() -> X { X }\n";
        let f = run(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("4 arg(s)") && f[0].contains("takes 5"), "{}", f[0]);
    }

    #[test]
    fn matching_calls_nested_args_and_turbofish_are_clean() {
        let src = "fn pair(a: u32, b: u32) {}\nfn t() { pair(f(1, 2), [3, 4].len() as u32); pair::<>(1, 2); }\nfn f(x: u32, y: u32) -> u32 { x + y }\n";
        assert!(run(src).is_empty(), "{:?}", run(src));
    }

    #[test]
    fn closures_pipes_do_not_add_arguments_but_bitwise_or_does_not_eat_them() {
        let src = "fn apply(f2: impl Fn(u32, u32) -> u32) {}\nfn t() { apply(|a, b| a + b); }\n";
        assert!(run(src).is_empty(), "{:?}", run(src));
        let src2 = "fn pair(a: u32, b: u32) {}\nfn t(x: u32, y: u32) { pair(x | y, 3); pair((x), ((y))); }\n";
        assert!(run(src2).is_empty(), "{:?}", run(src2));
        // free fn whose first param is a lifetime-ref is NOT a method
        let src3 = "fn label(s: &'static str, n: u32) {}\nfn t() { label(\"x\", 1); }\n";
        assert!(run(src3).is_empty(), "{:?}", run(src3));
    }

    #[test]
    fn methods_use_self_free_fns_do_not_and_duplicates_are_skipped() {
        let src = "impl S { fn go(&self, n: u32) {} }\nfn t(s: S) { s.go(1); s.go(1, 2); }\nfn dup() {}\nmod m { fn dup(a: u32) {} }\n";
        let f = run(src);
        assert_eq!(f.len(), 1, "{f:?}");
        assert!(f[0].contains("`go` called with 2"));
    }

    #[test]
    fn zero_arg_forms_and_macros_are_handled() {
        let src = "fn ping() {}\nfn t() { ping(); println!(\"ping(1,2)\"); }\n";
        assert!(run(src).is_empty(), "{:?}", run(src));
    }
}

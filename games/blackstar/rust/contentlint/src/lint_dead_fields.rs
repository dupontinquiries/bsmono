//! lint_dead_fields — Rust twin of tools/lint_dead_fields.py (the recurring
//! "defined-but-never-applied" bug gate). Ported per maintenance/RUST_PORT_PLAN.md;
//! the Python original stays as the parity reference until the twin has run green on
//! every host the suite runs on. OUTPUT AND EXIT SEMANTICS ARE CONTRACTUAL: byte-for-byte
//! identical stdout to the Python on the same tree, exit 1 iff any un-whitelisted dead
//! field exists. The parity diff is part of the test wiring, not a promise.
//!
//! What it checks (same as the Python): for each config struct in STRUCTS, extract the
//! field names from its `typedef struct { ... } name;` in the header, then require each
//! field to be read somewhere in the C sources via `->field` or `.field` (word-bounded).
//! A field set in a data table but never accessed is dead data — every entry in the
//! original's history (class hp_mul, chassis xform_dur_mul, greeble_boost, …) shipped
//! that way and was found by hand.
//!
//! std-only: no regex crate. The scanning is hand-rolled against the same grammar the
//! Python regexes accepted (struct bodies with no nested braces; `//` and `/* */`
//! comments; comma declarator lists; `[dims]` stripped; `*` pointers stripped).

use std::fs;
use std::path::{Path, PathBuf};

const STRUCTS: &[(&str, &str)] = &[
    ("ex_chassis_info_t", "engine_src/exengine/chassis.h"),
    ("ex_class_info_t", "engine_src/exengine/pclass.h"),
    ("ex_sig_stats_t", "engine_src/exengine/signature.h"),
    ("ex_sig_combat_t", "engine_src/exengine/signature.h"),
];
const WHITELIST: &[&str] = &["name", "tagline"];

fn is_ident(s: &str) -> bool {
    let mut ch = s.chars();
    match ch.next() {
        Some(c) if c.is_ascii_alphabetic() || c == '_' => {}
        _ => return false,
    }
    ch.all(|c| c.is_ascii_alphanumeric() || c == '_')
}

fn strip_comments(body: &str) -> String {
    // block comments first (non-greedy across lines), then line comments — same order as
    // the Python (re.sub of /*...*/ with DOTALL, then //...).
    let mut out = String::with_capacity(body.len());
    let b = body.as_bytes();
    let mut i = 0;
    while i < b.len() {
        if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'*' {
            i += 2;
            while i + 1 < b.len() && !(b[i] == b'*' && b[i + 1] == b'/') {
                i += 1;
            }
            i = (i + 2).min(b.len());
        } else if b[i] == b'/' && i + 1 < b.len() && b[i + 1] == b'/' {
            while i < b.len() && b[i] != b'\n' {
                i += 1;
            }
        } else {
            out.push(b[i] as char);
            i += 1;
        }
    }
    out
}

/// The field identifiers of `typedef struct { ... } name;` — the same grammar the
/// Python's `typedef\s+struct\s*\{([^{}]*)\}\s*name\s*;` accepted (no nested braces).
fn struct_fields(text: &str, name: &str) -> Vec<String> {
    let mut fields = Vec::new();
    let mut search_from = 0;
    while let Some(rel) = text[search_from..].find("typedef") {
        let td = search_from + rel;
        search_from = td + 7;
        // typedef \s+ struct
        let rest = &text[td + 7..];
        let rest_trim = rest.trim_start();
        if !rest_trim.starts_with("struct") {
            continue;
        }
        let after_struct = &rest_trim[6..];
        let after_struct_trim = after_struct.trim_start();
        if !after_struct_trim.starts_with('{') {
            continue; // named or forward struct — the Python regex skipped these too
        }
        let body_start_off = after_struct_trim[1..].as_ptr() as usize - text.as_ptr() as usize;
        let Some(close_rel) = text[body_start_off..].find('}') else { continue };
        let body = &text[body_start_off..body_start_off + close_rel];
        if body.contains('{') {
            continue; // nested braces: outside the accepted grammar, as in the Python
        }
        // } \s* NAME \s* ;
        let after_close = &text[body_start_off + close_rel + 1..];
        let after_close_trim = after_close.trim_start();
        if !after_close_trim.starts_with(name) {
            continue;
        }
        let after_name = &after_close_trim[name.len()..];
        if !after_name.trim_start().starts_with(';') {
            continue;
        }
        // parse the body: strip comments; split ';'; per declaration drop the type token,
        // then each comma-separated declarator: strip '*', drop [dims], keep identifiers.
        let clean = strip_comments(body);
        for decl in clean.split(';') {
            let decl = decl.trim();
            if decl.is_empty() {
                continue;
            }
            let parts: Vec<&str> = decl.split_whitespace().collect();
            if parts.len() < 2 {
                continue;
            }
            let declarators = parts[1..].join(" ");
            for tok in declarators.split(',') {
                let mut t = tok.trim().trim_start_matches('*').trim().to_string();
                while let (Some(a), Some(b)) = (t.find('['), t.find(']')) {
                    if a < b {
                        t.replace_range(a..=b, "");
                    } else {
                        break;
                    }
                }
                let t = t.trim_matches(|c| c == '*' || c == ' ').to_string();
                if is_ident(&t) {
                    fields.push(t);
                }
            }
        }
        return fields; // first match wins, like re.search
    }
    fields
}

/// Word-bounded `->field` / `.field` anywhere in the blob.
fn field_is_accessed(blob: &str, field: &str) -> bool {
    for pat in [format!("->{field}"), format!(".{field}")] {
        let mut from = 0;
        while let Some(rel) = blob[from..].find(&pat) {
            let hit = from + rel;
            let end = hit + pat.len();
            let boundary_ok = blob[end..]
                .chars()
                .next()
                .map(|c| !(c.is_ascii_alphanumeric() || c == '_'))
                .unwrap_or(true);
            if boundary_ok {
                return true;
            }
            from = end;
        }
    }
    false
}

fn gather_sources(root: &Path) -> String {
    // engine_src/*.c + engine_src/exengine/**/*.c — the same two globs as the Python.
    let mut blob = String::new();
    let mut push_dir = |dir: &Path, recursive: bool| {
        let mut stack: Vec<PathBuf> = vec![dir.to_path_buf()];
        while let Some(d) = stack.pop() {
            let Ok(rd) = fs::read_dir(&d) else { continue };
            let mut entries: Vec<_> = rd.flatten().map(|e| e.path()).collect();
            entries.sort();
            for p in entries {
                if p.is_dir() {
                    if recursive {
                        stack.push(p);
                    }
                } else if p.extension().is_some_and(|e| e == "c") {
                    if let Ok(s) = fs::read_to_string(&p) {
                        blob.push_str(&s);
                    } else if let Ok(bytes) = fs::read(&p) {
                        blob.push_str(&String::from_utf8_lossy(&bytes)); // errors="ignore"
                    }
                }
            }
        }
    };
    push_dir(&root.join("engine_src"), false);
    push_dir(&root.join("engine_src").join("exengine"), true);
    blob
}

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let mut root = PathBuf::from(".");
    if let Some(i) = args.iter().position(|a| a == "--root") {
        if let Some(r) = args.get(i + 1) {
            root = PathBuf::from(r);
        }
    }

    let blob = gather_sources(&root);
    let mut dead: Vec<(&str, String)> = Vec::new();
    let mut checked = 0u32;
    for (sname, hpath) in STRUCTS {
        let htext = match fs::read_to_string(root.join(hpath)) {
            Ok(t) => t,
            Err(_) => match fs::read(root.join(hpath)) {
                Ok(b) => String::from_utf8_lossy(&b).into_owned(),
                Err(_) => {
                    println!("  (skip {sname}: {hpath} not found)");
                    continue;
                }
            },
        };
        for f in struct_fields(&htext, sname) {
            if WHITELIST.contains(&f.as_str()) {
                continue;
            }
            checked += 1;
            if field_is_accessed(&blob, &f) {
                continue;
            }
            dead.push((sname, f));
        }
    }

    println!("dead-field lint: checked {checked} fields across {} config structs", STRUCTS.len());
    for (sname, f) in &dead {
        println!("  DEAD: {sname}.{f} is set in its data table but never read via ->/. in any source");
    }
    if !dead.is_empty() {
        println!("FAILED: {} dead field(s).", dead.len());
        std::process::exit(1);
    }
    println!("OK: every config field is applied somewhere.");
}

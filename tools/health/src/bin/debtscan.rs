//! debtscan — marker debt (comments) and abort-debt (code), counted separately.
//!
//! Two different debts share the word: TODO/FIXME/HACK/XXX are NOTES — they live in
//! comments, harmless at runtime, and their count per area is a planning signal.
//! unwrap/expect/panic!/todo!/unimplemented!/dbg! are ABORTS — they live in code and
//! each is a place the program can die (dbg! additionally must never ship). Scanning
//! them the same way lies in both directions: a doc-string "don't unwrap here" isn't
//! debt, and a TODO only counts where a human wrote it. So markers are scanned in the
//! RAW text and abort-calls in CODE ONLY via the shared lexer.
//!
//! Output: per-directory rollup (depth --depth, default 2) plus the top offender
//! files. `--fail-on-dbg` exits 1 if any dbg! exists — the one marker with no
//! legitimate committed form.
//!
//! Usage: debtscan [--depth N] [--fail-on-dbg] DIR...   (.rs files)

#[path = "../../../sanity/src/lex.rs"]
mod lex;

use std::collections::BTreeMap;

const MARKERS: &[&str] = &["TODO", "FIXME", "HACK", "XXX"];
const ABORTS: &[&str] = &[".unwrap(", ".expect(", "panic!(", "todo!(", "unimplemented!(", "dbg!("];

fn count_occurrences(hay: &str, needle: &str) -> usize {
    let mut n = 0; let mut from = 0;
    while let Some(rel) = hay[from..].find(needle) { n += 1; from += rel + needle.len(); }
    n
}

fn main() {
    let mut args: Vec<String> = std::env::args().skip(1).collect();
    let mut depth = 2usize;
    if let Some(p) = args.iter().position(|a| a == "--depth") {
        if p + 1 < args.len() { depth = args.remove(p + 1).parse().unwrap_or(2); args.remove(p); }
    }
    let fail_on_dbg = if let Some(p) = args.iter().position(|a| a == "--fail-on-dbg") { args.remove(p); true } else { false };
    if args.is_empty() {
        eprintln!("usage: debtscan [--depth N] [--fail-on-dbg] DIR...");
        std::process::exit(2);
    }
    let mut files = Vec::new();
    for a in &args { lex::collect_files(std::path::Path::new(a), &["rs"], &mut files); }

    // area -> [markers, aborts, dbg]
    let mut areas: BTreeMap<String, [usize; 3]> = BTreeMap::new();
    let mut per_file: Vec<(usize, String)> = Vec::new();
    let mut dbg_sites: Vec<String> = Vec::new();

    for f in &files {
        let Ok(raw) = std::fs::read_to_string(f) else { continue };
        let mut code = String::with_capacity(raw.len());
        lex::walk_code(raw.as_bytes(), |b, _, _| code.push(b as char));

        let m: usize = MARKERS.iter().map(|k| count_occurrences(&raw, k)).sum();
        let mut a = 0usize; let mut d = 0usize;
        for k in ABORTS {
            let c = count_occurrences(&code, k);
            if *k == "dbg!(" { d = c; } else { a += c; }
        }
        if d > 0 {
            // exact lines for dbg!, they must be actionable
            for (ln, line) in raw.lines().enumerate() {
                if line.contains("dbg!(") { dbg_sites.push(format!("{}:{}: {}", f.display(), ln + 1, line.trim())); }
            }
        }
        let comps: Vec<String> = f.components().map(|c| c.as_os_str().to_string_lossy().into_owned()).collect();
        let area = comps[..depth.min(comps.len().saturating_sub(1))].join("/");
        let e = areas.entry(if area.is_empty() { ".".into() } else { area }).or_insert([0; 3]);
        e[0] += m; e[1] += a; e[2] += d;
        if m + a + d > 0 { per_file.push((m + a + d, f.display().to_string())); }
    }

    println!("── debt by area (markers / abort-calls / dbg!) ──────────");
    for (area, [m, a, d]) in &areas {
        if m + a + d == 0 { continue; }
        println!("  {m:>5} / {a:>5} / {d:>3}   {area}");
    }
    per_file.sort_by_key(|x| std::cmp::Reverse(x.0));
    println!("── top offender files ───────────────────────────────────");
    for (n, f) in per_file.iter().take(10) { println!("  {n:>5}  {f}"); }
    if !dbg_sites.is_empty() {
        println!("── dbg! sites (must be zero in committed code) ──────────");
        for s in &dbg_sites { println!("  {s}"); }
    }
    std::process::exit(if fail_on_dbg && !dbg_sites.is_empty() { 1 } else { 0 });
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn markers_count_raw_but_aborts_count_code_only() {
        let raw = "// TODO: later\nfn f(o: Option<u8>) -> u8 {\n  let s = \"don't .unwrap( here\";\n  o.unwrap()\n}\n";
        let mut code = String::new();
        lex::walk_code(raw.as_bytes(), |b, _, _| code.push(b as char));
        assert_eq!(count_occurrences(raw, "TODO"), 1);
        assert_eq!(count_occurrences(&code, ".unwrap("), 1); // the string mention is gone
        assert_eq!(count_occurrences(raw, ".unwrap("), 2);   // raw would have lied
    }
}

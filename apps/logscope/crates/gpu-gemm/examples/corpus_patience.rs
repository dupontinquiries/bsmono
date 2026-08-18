//! corpus_patience.rs — the trade the whole reaping case turns on, measured.
//!
//! `corpus_falsekill.rs` established that the detector condemns under 1% of known-good files when
//! it is allowed to read the whole thing. That is the wrong question for `reap`, which needs to kill
//! EARLY: yield is capped by `1/at_fraction`, so a detector that fires at 80% of the budget buys 11%
//! and one that fires at 20% buys 3.57x.
//!
//! Firing early means condemning a call before the branch has had a chance to define it, and Rust
//! resolves forward references. `Scanner::stale(idx, patience)` is the knob: condemn a suspicion
//! only once it has gone `patience` bytes unresolved. Small patience kills early and kills good
//! branches; large patience is safe and worthless.
//!
//! This measures both sides on real code:
//!
//! * FALSE KILLS — unmutated files, fed incrementally. Any trigger is wrong.
//! * DETECTION — the same files with one free call renamed to a plausible non-existent variant,
//!   which is the best-documented shape of LLM code failure. Any trigger is right.
//! * at_fraction — where in the file the trigger fires, as a fraction of its length. This is the
//!   number `reap::slot_yield` has been taking on faith.
//!
//! The mutations are a MODEL of how a model fails, not observed behaviour. No generated code was
//! available here. Treat the detection column as conditional on hallucinated-name failures being
//! the failure mode, which is the assumption the whole detector rests on and which nothing here
//! establishes.
//!
//! Run: `cargo run --release --example corpus_patience -- <dir>...`

use gpu_kernels::corpus::{calls_at, strip_noncode, Scanner, SymbolIndex};
use gpu_kernels::reap::slot_yield;

fn collect(dir: &std::path::Path, out: &mut Vec<(String, String)>, cap: usize) {
    if out.len() >= cap { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut e: Vec<_> = rd.filter_map(|x| x.ok()).collect();
    e.sort_by_key(|x| x.path());
    for en in e {
        if out.len() >= cap { return; }
        let p = en.path();
        if p.is_dir() {
            let n = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if n == "target" || n == ".git" || n == "node_modules" { continue; }
            collect(&p, out, cap);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(src) = std::fs::read_to_string(&p) {
                if src.len() > 800 { out.push((p.display().to_string(), src)); }
            }
        }
    }
}

/// Rename one resolvable free call to a plausible variant that exists nowhere.
///
/// The variants are the ones models reach for: a synonym prefix, a redundant suffix, a plural. A
/// mutation that produced obvious garbage would measure nothing.
fn mutate(src: &str, idx: &SymbolIndex, defined: &SymbolIndex, pick: usize) -> Option<(String, String, usize)> {
    let stripped = strip_noncode(src);
    let cands: Vec<(String, usize)> = calls_at(&stripped).into_iter()
        .filter(|(n, _)| (idx.contains(n) || defined.contains(n)) && n.len() > 4)
        .collect();
    if cands.is_empty() { return None; }
    let (name, off) = &cands[pick % cands.len()];
    let variants = [
        format!("{name}_impl"), format!("{name}_inner"), format!("get_{name}"),
        format!("{name}s"), name.replacen("get_", "fetch_", 1), name.replacen("parse_", "read_", 1),
    ];
    let repl = variants.iter().find(|v| **v != *name && !idx.contains(v) && !defined.contains(v))?;
    let mut out = String::with_capacity(src.len() + 8);
    out.push_str(&src[..*off]);
    out.push_str(repl);
    out.push_str(&src[off + name.len()..]);
    Some((out, repl.clone(), *off))
}

/// Feed `src` in chunks and return the byte offset where `stale` first fires.
fn first_trigger(src: &str, idx: &SymbolIndex, patience: usize, chunk: usize) -> Option<usize> {
    let mut sc = Scanner::new();
    let mut i = chunk;
    while i < src.len() {
        while !src.is_char_boundary(i) { i += 1; }
        sc.feed(&src[..i], false);
        if let Some((_, off)) = sc.stale(idx, patience) { return Some(off); }
        i += chunk;
    }
    sc.feed(src, true);
    sc.stale(idx, patience).map(|(_, off)| off)
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dirs = if args.is_empty() { vec![".".to_string()] } else { args };
    let cap: usize = std::env::var("CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(150);
    let chunk: usize = std::env::var("CHUNK").ok().and_then(|s| s.parse().ok()).unwrap_or(120);

    let mut files = Vec::new();
    for d in &dirs { collect(std::path::Path::new(d), &mut files, cap); }
    if files.is_empty() { eprintln!("no files"); return; }

    let mut idx = SymbolIndex::new();
    for (_, s) in &files { idx.add_source(s); }
    println!("{} files, {} distinct identifiers, chunk {chunk}B\n", files.len(), idx.distinct());

    // Build the mutated set once so every patience level sees identical inputs.
    let mut mutated: Vec<(String, usize)> = Vec::new();
    for (i, (_, src)) in files.iter().enumerate() {
        let mut defined = SymbolIndex::new();
        defined.add_source(src);
        if let Some((m, _, off)) = mutate(src, &idx, &defined, i) { mutated.push((m, off)); }
    }
    println!("mutated {} of {} files (one free call renamed to a plausible non-existent variant)\n",
             mutated.len(), files.len());

    println!("{:>9} {:>13} {:>12} {:>14} {:>12} {:>10}",
             "patience", "false kills", "detected", "at_fraction", "slot yield", "verdict");
    println!("{}", "-".repeat(76));

    for patience in [0usize, 40, 120, 400, 1200, 4000] {
        let fk = files.iter()
            .filter(|(_, s)| first_trigger(s, &idx, patience, chunk).is_some())
            .count();

        let (mut hit, mut frac_acc) = (0usize, 0f64);
        for (m, _) in &mutated {
            if let Some(off) = first_trigger(m, &idx, patience, chunk) {
                hit += 1;
                frac_acc += off as f64 / m.len() as f64;
            }
        }
        let at = if hit == 0 { 1.0 } else { frac_acc / hit as f64 };
        let y = slot_yield(0.9 * (hit as f32 / mutated.len().max(1) as f32), at as f32);
        let fk_rate = fk as f32 / files.len() as f32;
        let verdict = if fk_rate > 0.05 { "SHREDS" } else if y < 1.2 { "inert" } else { "usable" };
        println!("{:>9} {:>9}/{:<3} {:>8}/{:<3} {:>14.3} {:>11.2}x {:>10}",
                 patience, fk, files.len(), hit, mutated.len(), at, y, verdict);
    }

    println!("\nat_fraction is where the trigger fires as a share of the file. reap::slot_yield is");
    println!("capped by 1/at_fraction, and the yield column already discounts for missed detections.");
}

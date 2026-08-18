//! corpus_falsekill.rs — run the early-kill detector over known-good code and count the damage.
//!
//! Every other study in this crate feeds the machinery synthetic data. This one does not. It walks
//! real Rust from the monorepo, builds the symbol index from every file EXCEPT the one under test,
//! and asks: would this branch have been killed?
//!
//! Each file is a stand-in for a branch's output — code that compiled, passed review and shipped. So
//! every condemnation is a false kill, and the rate is the number that decides whether the detector
//! is usable at all. `reap::worth_it` prices false kills against the failure ratio; this supplies
//! the rate that goes into it.
//!
//! Leave-one-out is the right shape because it mirrors the real situation: the branch is writing
//! something the corpus has not seen, using vocabulary the corpus mostly has.
//!
//! Run: `cargo run --release --example corpus_falsekill -- <dir> [dir...]`

use gpu_kernels::corpus::{Scanner, SymbolIndex, Unresolved};
use std::collections::HashMap;

fn collect(dir: &std::path::Path, out: &mut Vec<(String, String)>, cap: usize) {
    if out.len() >= cap { return; }
    let Ok(rd) = std::fs::read_dir(dir) else { return };
    let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
    entries.sort_by_key(|e| e.path());
    for e in entries {
        if out.len() >= cap { return; }
        let p = e.path();
        if p.is_dir() {
            let name = p.file_name().and_then(|s| s.to_str()).unwrap_or("");
            if name == "target" || name == ".git" || name == "node_modules" { continue; }
            collect(&p, out, cap);
        } else if p.extension().and_then(|s| s.to_str()) == Some("rs") {
            if let Ok(src) = std::fs::read_to_string(&p) {
                // Very small files are mostly `mod` lines and say nothing either way.
                if src.len() > 400 { out.push((p.display().to_string(), src)); }
            }
        }
    }
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let dirs: Vec<String> = if args.is_empty() { vec![".".into()] } else { args };
    let cap: usize = std::env::var("CAP").ok().and_then(|s| s.parse().ok()).unwrap_or(400);

    let mut files = Vec::new();
    for d in &dirs { collect(std::path::Path::new(d), &mut files, cap); }
    if files.is_empty() { eprintln!("no .rs files found under {dirs:?}"); return; }

    // Per-file symbol counts, so leave-one-out is a subtraction rather than 400 rebuilds.
    let mut global = SymbolIndex::new();
    let mut per_file: Vec<HashMap<String, u32>> = Vec::with_capacity(files.len());
    for (_, src) in &files {
        global.add_source(src);
        let mut m: HashMap<String, u32> = HashMap::new();
        for id in gpu_kernels::corpus::identifiers(src) { *m.entry(id).or_insert(0) += 1; }
        per_file.push(m);
    }

    println!("{} files, {} distinct identifiers, {} total occurrences\n",
             files.len(), global.distinct(), global.total());

    let (mut killed, mut total_unres) = (0usize, 0usize);
    let mut worst: Vec<(usize, String, Vec<String>)> = Vec::new();
    let mut by_name: HashMap<String, usize> = HashMap::new();

    for (i, (path, src)) in files.iter().enumerate() {
        // Leave-one-out: rebuild the index from every other file's counts.
        let mut idx = SymbolIndex::new();
        for (j, m) in per_file.iter().enumerate() {
            if j == i { continue; }
            for (k, v) in m { for _ in 0..*v { idx.add_source(k); } }
        }
        let mut sc = Scanner::new();
        sc.feed(src, true);
        sc.settle(&idx);
        let names: Vec<String> = sc.unresolved().iter()
            .map(|Unresolved::Call(n)| n.clone()).collect();
        if !names.is_empty() {
            killed += 1;
            total_unres += names.len();
            for n in &names { *by_name.entry(n.clone()).or_insert(0) += 1; }
            worst.push((names.len(), path.clone(), names));
        }
    }

    println!("FALSE KILLS: {killed}/{} files ({:.1}%), {total_unres} unresolved calls in total",
             files.len(), 100.0 * killed as f32 / files.len() as f32);

    worst.sort_by(|a, b| b.0.cmp(&a.0));
    if !worst.is_empty() {
        println!("\nworst offenders:");
        for (n, path, names) in worst.iter().take(8) {
            let short: Vec<&str> = names.iter().take(6).map(|s| s.as_str()).collect();
            println!("  {n:>3}  {}", path.rsplit('/').next().unwrap_or(path));
            println!("       {short:?}");
        }
        let mut common: Vec<(&String, &usize)> = by_name.iter().collect();
        common.sort_by(|a, b| b.1.cmp(a.1));
        println!("\nnames condemned across the most files (these are detector gaps, not typos):");
        for (n, c) in common.iter().take(12) { println!("  {c:>3}x  {n}"); }
    }

    // What the rate means downstream.
    let rate = killed as f32 / files.len() as f32;
    println!("\nfeeding {rate:.3} into reap::worth_it, 32 slots, reap 90% at 20% of budget:");
    for success in [0.01f32, 0.05, 0.10] {
        let (gained, lost) = gpu_kernels::reap::worth_it(32, 0.9, 0.2, rate, success);
        let verdict = if gained > lost { "pays" } else { "LOSES" };
        println!("  success rate {success:.2}: +{gained:.3} solutions, -{lost:.3} lost  -> {verdict}");
    }
}

//! loc — count lines of code by file extension (a tiny tokei). Splits code vs
//! blank vs comment (// and # heuristics).  loc [dir]

use std::collections::BTreeMap;
use std::path::Path;

fn noise(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "node_modules" | "__pycache__"
        | ".pytest_cache" | ".venv" | "dist")
}

#[derive(Default, Clone, Copy)]
struct Stat { files: usize, code: usize, blank: usize, comment: usize }

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut by_ext: BTreeMap<String, Stat> = BTreeMap::new();
    walk(Path::new(&root), &mut by_ext);
    let mut total = Stat::default();
    println!("{:<12} {:>7} {:>9} {:>9} {:>9}", "language", "files", "code", "comment", "blank");
    println!("{}", "-".repeat(50));
    for (ext, s) in &by_ext {
        println!("{:<12} {:>7} {:>9} {:>9} {:>9}", ext, s.files, s.code, s.comment, s.blank);
        total.files += s.files; total.code += s.code; total.comment += s.comment; total.blank += s.blank;
    }
    println!("{}", "-".repeat(50));
    println!("{:<12} {:>7} {:>9} {:>9} {:>9}", "TOTAL", total.files, total.code, total.comment, total.blank);
}

fn walk(path: &Path, acc: &mut BTreeMap<String, Stat>) {
    if path.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            for e in rd.flatten() {
                if !noise(&e.file_name().to_string_lossy()) { walk(&e.path(), acc); }
            }
        }
        return;
    }
    let ext = path.extension().and_then(|x| x.to_str()).unwrap_or("(none)").to_string();
    let Ok(text) = std::fs::read_to_string(path) else { return; };
    let s = acc.entry(ext).or_default();
    s.files += 1;
    for line in text.lines() {
        let t = line.trim();
        if t.is_empty() { s.blank += 1; }
        else if t.starts_with("//") || t.starts_with('#') || t.starts_with("/*") || t.starts_with('*') { s.comment += 1; }
        else { s.code += 1; }
    }
}

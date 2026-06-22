//! findtext — a tiny recursive grep (literal substring), for when ripgrep isn't
//! installed.  findtext <pattern> [dir]   → file:line: matching line

use std::path::Path;

fn noise(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "node_modules" | "__pycache__"
        | ".pytest_cache" | ".venv" | "dist")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let Some(pat) = args.first() else { eprintln!("usage: findtext <pattern> [dir]"); std::process::exit(2); };
    let root = args.get(1).map(String::as_str).unwrap_or(".");
    let mut hits = 0usize;
    search(Path::new(root), pat, &mut hits);
    if hits == 0 { std::process::exit(1); } // grep convention: no match = non-zero
}

fn search(path: &Path, pat: &str, hits: &mut usize) {
    if path.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            let mut entries: Vec<_> = rd.flatten().filter(|e| !noise(&e.file_name().to_string_lossy())).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries { search(&e.path(), pat, hits); }
        }
    } else if let Ok(text) = std::fs::read_to_string(path) {
        for (n, line) in text.lines().enumerate() {
            if line.contains(pat) {
                println!("{}:{}: {}", path.display(), n + 1, line.trim());
                *hits += 1;
            }
        }
    }
}

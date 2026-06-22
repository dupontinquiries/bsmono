//! todos — scan a tree for actionable markers (TODO/FIXME/HACK/XXX/BUG), printing
//! file:line: marker text.  todos [dir]

use std::path::Path;

const MARKERS: &[&str] = &["TODO", "FIXME", "HACK", "XXX", "BUG"];

fn noise(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "node_modules" | "__pycache__"
        | ".pytest_cache" | ".venv" | "dist")
}

fn main() {
    let root = std::env::args().nth(1).unwrap_or_else(|| ".".into());
    let mut n = 0usize;
    walk(Path::new(&root), &mut n);
    eprintln!("\n{n} markers found");
}

fn walk(path: &Path, n: &mut usize) {
    if path.is_dir() {
        if let Ok(rd) = std::fs::read_dir(path) {
            let mut entries: Vec<_> = rd.flatten().filter(|e| !noise(&e.file_name().to_string_lossy())).collect();
            entries.sort_by_key(|e| e.file_name());
            for e in entries { walk(&e.path(), n); }
        }
    } else if let Ok(text) = std::fs::read_to_string(path) {
        for (i, line) in text.lines().enumerate() {
            if MARKERS.iter().any(|m| line.contains(m)) {
                println!("{}:{}: {}", path.display(), i + 1, line.trim());
                *n += 1;
            }
        }
    }
}

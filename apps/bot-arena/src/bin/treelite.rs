//! treelite — a tiny `tree`: print a directory as an indented tree, skipping
//! VCS/build noise, with an optional depth limit and file/dir counts.
//!
//!   treelite [dir] [-L depth]

use std::path::Path;

fn noise(name: &str) -> bool {
    matches!(name, ".git" | "target" | "build" | "node_modules" | "__pycache__"
        | ".pytest_cache" | ".venv" | "dist" | ".cargo")
}

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let mut dir = ".".to_string();
    let mut max_depth = usize::MAX;
    let mut i = 0;
    while i < args.len() {
        match args[i].as_str() {
            "-L" | "--level" => { i += 1; if i < args.len() { max_depth = args[i].parse().unwrap_or(usize::MAX); } }
            s => dir = s.to_string(),
        }
        i += 1;
    }
    let root = Path::new(&dir);
    println!("{}", root.display());
    let (mut dirs, mut files) = (0usize, 0usize);
    walk(root, "", 0, max_depth, &mut dirs, &mut files);
    println!("\n{dirs} directories, {files} files");
}

fn walk(dir: &Path, prefix: &str, depth: usize, max_depth: usize, dirs: &mut usize, files: &mut usize) {
    if depth >= max_depth { return; }
    let mut entries: Vec<_> = match std::fs::read_dir(dir) {
        Ok(rd) => rd.flatten().filter(|e| !noise(&e.file_name().to_string_lossy())).collect(),
        Err(_) => return,
    };
    entries.sort_by_key(|e| e.file_name());
    let n = entries.len();
    for (idx, e) in entries.into_iter().enumerate() {
        let last = idx + 1 == n;
        let connector = if last { "└── " } else { "├── " };
        let name = e.file_name().to_string_lossy().to_string();
        println!("{prefix}{connector}{name}");
        if e.path().is_dir() {
            *dirs += 1;
            let child_prefix = format!("{prefix}{}", if last { "    " } else { "│   " });
            walk(&e.path(), &child_prefix, depth + 1, max_depth, dirs, files);
        } else {
            *files += 1;
        }
    }
}

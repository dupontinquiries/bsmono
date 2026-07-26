//! extract-models -- recursively extract a model .zip library, preserving the zip
//! NAMES as folder names (they carry classification metadata) and handling nested
//! zips. Rust port of extract_models.py.

use std::env;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};

const MODEL_EXT: &[&str] = &["gltf", "glb", "obj", "fbx", "stl", "dae"];

fn find_zips(root: &Path, out: &mut Vec<PathBuf>) {
    if let Ok(entries) = fs::read_dir(root) {
        for e in entries.flatten() {
            let p = e.path();
            if p.is_dir() {
                find_zips(&p, out);
            } else if p
                .extension()
                .map(|x| x.eq_ignore_ascii_case("zip"))
                .unwrap_or(false)
            {
                out.push(p);
            }
        }
    }
}

fn dir_nonempty(p: &Path) -> bool {
    p.is_dir() && fs::read_dir(p).map(|mut it| it.next().is_some()).unwrap_or(false)
}

fn extract_one(zip_path: &Path, dest: &Path) -> io::Result<()> {
    let file = fs::File::open(zip_path)?;
    let mut archive = zip::ZipArchive::new(file)
        .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
    fs::create_dir_all(dest)?;
    let base = dest.canonicalize().unwrap_or_else(|_| dest.to_path_buf());
    for i in 0..archive.len() {
        let mut entry = archive
            .by_index(i)
            .map_err(|e| io::Error::new(io::ErrorKind::InvalidData, e))?;
        let rel = match entry.enclosed_name() {
            Some(p) => p.to_owned(),
            None => continue, // path traversal guard (zip crate provides this)
        };
        let target = dest.join(&rel);
        // extra guard: keep target under dest
        if let Ok(c) = target.canonicalize() {
            if !c.starts_with(&base) {
                continue;
            }
        }
        if entry.is_dir() {
            fs::create_dir_all(&target)?;
        } else {
            if let Some(parent) = target.parent() {
                fs::create_dir_all(parent)?;
            }
            let mut out = fs::File::create(&target)?;
            io::copy(&mut entry, &mut out)?;
        }
    }
    Ok(())
}

fn extract_all(root: &Path, max_passes: usize) -> (usize, usize, usize) {
    let (mut extracted, mut skipped, mut failed) = (0, 0, 0);
    for _pass in 0..max_passes {
        let mut zips = Vec::new();
        find_zips(root, &mut zips);
        let mut found_new = false;
        for zp in zips {
            let stem = zp.file_stem().and_then(|s| s.to_str()).unwrap_or("model");
            let dest = zp.with_file_name(stem);
            if dir_nonempty(&dest) {
                skipped += 1;
                continue;
            }
            match extract_one(&zp, &dest) {
                Ok(()) => {
                    extracted += 1;
                    found_new = true;
                    println!(
                        "  extracted: {}",
                        dest.strip_prefix(root).unwrap_or(&dest).display()
                    );
                }
                Err(e) => {
                    println!("  ! failed: {} - {}", zp.display(), e);
                    failed += 1;
                }
            }
        }
        if !found_new {
            break;
        }
    }
    (extracted, skipped, failed)
}

fn has_model(dir: &Path) -> bool {
    fs::read_dir(dir)
        .map(|it| {
            it.flatten().any(|e| {
                e.path()
                    .extension()
                    .and_then(|x| x.to_str())
                    .map(|x| MODEL_EXT.iter().any(|m| x.eq_ignore_ascii_case(m)))
                    .unwrap_or(false)
            })
        })
        .unwrap_or(false)
}

fn count_models(root: &Path) -> usize {
    let mut n = 0;
    fn walk(d: &Path, n: &mut usize) {
        if has_model(d) {
            *n += 1;
        }
        if let Ok(entries) = fs::read_dir(d) {
            for e in entries.flatten() {
                if e.path().is_dir() {
                    walk(&e.path(), n);
                }
            }
        }
    }
    walk(root, &mut n);
    n
}

fn print_tree(dir: &Path, prefix: &str, depth: usize, max_depth: usize) {
    if depth > max_depth {
        return;
    }
    let mut dirs: Vec<PathBuf> = fs::read_dir(dir)
        .map(|it| it.flatten().map(|e| e.path()).filter(|p| p.is_dir()).collect())
        .unwrap_or_default();
    dirs.sort();
    for (i, d) in dirs.iter().enumerate() {
        let last = i + 1 == dirs.len();
        let branch = if last { "└── " } else { "├── " };
        let tag = if has_model(d) { "  [model]" } else { "" };
        println!(
            "{}{}{}{}",
            prefix,
            branch,
            d.file_name().unwrap().to_string_lossy(),
            tag
        );
        let next = format!("{}{}", prefix, if last { "    " } else { "│   " });
        print_tree(d, &next, depth + 1, max_depth);
    }
}

fn main() {
    let args: Vec<String> = env::args().collect();
    let mut root: Option<String> = None;
    let mut tree_only = false;
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--tree-only" => tree_only = true,
            s if !s.starts_with("--") => root = Some(s.to_string()),
            _ => {}
        }
        i += 1;
    }
    let root = match root {
        Some(r) => PathBuf::from(r),
        None => {
            eprintln!("usage: extract-models ROOT [--tree-only]");
            std::process::exit(1);
        }
    };
    if !root.is_dir() {
        eprintln!("not a directory: {}", root.display());
        std::process::exit(1);
    }
    if !tree_only {
        println!("extracting zips (preserving names as folders)...");
        let (ex, sk, fa) = extract_all(&root, 4);
        println!("\n{} extracted, {} already done, {} failed", ex, sk, fa);
    }
    println!("\nmodel folders found: {}", count_models(&root));
    println!("\ntree:");
    println!("{}/", root.file_name().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| ".".into()));
    print_tree(&root, "", 1, 6);
}

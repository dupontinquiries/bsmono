//! Import a Rust app from a .zip: extract it into a fresh temp directory and locate
//! the crate to build. Extraction shells out (unzip → bsdtar → python3 -m zipfile, in
//! that order) so it works without adding a zip dependency; the command construction
//! and the crate-finding are pure and unit-tested.

use std::path::{Path, PathBuf};

/// A fresh, unique temp directory for one import.
pub fn import_dir() -> PathBuf {
    let nanos = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    std::env::temp_dir().join(format!("gitmind-import-{}-{}", std::process::id(), nanos))
}

fn q(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Shell that extracts `zip` into `dir`, trying the extractors most likely to be
/// present. Prints GITMIND_NO_UNZIP and fails if none exist. Pure — unit-tested.
pub fn extract_command(zip: &str, dir: &str) -> String {
    let z = q(zip);
    let d = q(dir);
    format!(
        "mkdir -p {d}\n\
         if command -v unzip >/dev/null 2>&1; then unzip -o -q {z} -d {d}\n\
         elif command -v bsdtar >/dev/null 2>&1; then bsdtar -xf {z} -C {d}\n\
         elif command -v python3 >/dev/null 2>&1; then python3 -m zipfile -e {z} {d}\n\
         else echo GITMIND_NO_UNZIP; exit 1; fi"
    )
}

fn collect_crates(dir: &Path, depth: u32, out: &mut Vec<PathBuf>) {
    if depth > 6 {
        return;
    }
    if dir.join("Cargo.toml").is_file() {
        out.push(dir.to_path_buf());
        return; // a crate root — don't descend into it
    }
    if let Ok(rd) = std::fs::read_dir(dir) {
        for e in rd.flatten() {
            if e.file_type().map(|t| t.is_dir()).unwrap_or(false) {
                if e.file_name().to_string_lossy().starts_with('.') {
                    continue;
                }
                collect_crates(&e.path(), depth + 1, out);
            }
        }
    }
}

/// Find the crate to build in an extracted tree — the shallowest directory containing
/// a Cargo.toml (zips usually wrap the project in one folder). Pure — unit-tested.
pub fn find_crate_root(dir: &Path) -> Option<PathBuf> {
    let mut found = Vec::new();
    collect_crates(dir, 0, &mut found);
    found.into_iter().min_by_key(|p| p.components().count())
}

/// Shell that overlays the extracted files (`src`) onto an existing repo (`dst`) with
/// rsync, always protecting the repo's own .git/.gitmind. `mirror` adds --delete so the
/// repo ends up matching the zip exactly (files the zip dropped are removed too). Prints
/// GITMIND_NO_RSYNC if rsync is missing. Pure — unit-tested.
pub fn overlay_command(src: &str, dst: &str, mirror: bool) -> String {
    let s = q(src);
    let d = q(dst);
    let del = if mirror { " --delete" } else { "" };
    format!(
        "if ! command -v rsync >/dev/null 2>&1; then echo GITMIND_NO_RSYNC; exit 3; fi\n\
         rsync -a{del} --exclude='.git' --exclude='.gitmind' {s}/ {d}/"
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    #[test]
    fn extract_tries_multiple_tools() {
        let s = extract_command("/d/My App.zip", "/tmp/x");
        assert!(s.contains("mkdir -p '/tmp/x'"));
        assert!(s.contains("unzip -o -q '/d/My App.zip' -d '/tmp/x'"), "space-safe quoting");
        assert!(s.contains("bsdtar") && s.contains("python3 -m zipfile"), "fallbacks");
        assert!(s.contains("GITMIND_NO_UNZIP"), "clear failure when none present");
    }

    #[test]
    fn finds_shallowest_crate() {
        let base = std::env::temp_dir().join(format!("gm-import-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        // zip extracted to base/myapp-1.0/ with the crate inside, plus a nested sub-crate
        fs::create_dir_all(base.join("myapp-1.0/src")).unwrap();
        fs::write(base.join("myapp-1.0/Cargo.toml"), "[package]\nname=\"myapp\"\n").unwrap();
        fs::create_dir_all(base.join("myapp-1.0/vendor/dep/src")).unwrap();
        fs::write(base.join("myapp-1.0/vendor/dep/Cargo.toml"), "[package]\nname=\"dep\"\n").unwrap();
        let root = find_crate_root(&base).unwrap();
        assert!(root.ends_with("myapp-1.0"), "picks the top-level crate, not the vendored one: {root:?}");
        assert!(find_crate_root(&base.join("nope")).is_none());
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn overlay_protects_git_and_can_mirror() {
        let o = overlay_command("/tmp/extract", "/home/me/Code/app", false);
        assert!(o.contains("rsync -a --exclude='.git' --exclude='.gitmind' '/tmp/extract'/ '/home/me/Code/app'/"));
        assert!(!o.contains("--delete"), "overlay never deletes");
        assert!(o.contains("GITMIND_NO_RSYNC"), "clear error when rsync absent");
        let m = overlay_command("/tmp/extract", "/dst", true);
        assert!(m.contains("rsync -a --delete --exclude='.git'"), "mirror deletes, still protects .git");
    }
}

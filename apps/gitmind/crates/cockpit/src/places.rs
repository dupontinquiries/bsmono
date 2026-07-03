//! Where imported projects live, and how they're initialized. The repos base defaults
//! to ~/Code and is remembered across runs (most-recent first, like the build-repo
//! history); a project lands at <base>/<language>/<name>. The destination math, name
//! sanitizing, language detection, and history dedupe are pure and unit-tested.

use std::path::{Path, PathBuf};

/// The always-available default repos base, ~/Code.
pub fn default_repos_base() -> String {
    std::env::var("HOME").map(|h| format!("{h}/Code")).unwrap_or_else(|_| "Code".into())
}

/// Remembered repos bases, most-recent first, with the ~/Code default always present.
pub fn repos_bases() -> Vec<String> {
    let mut hist: Vec<String> = crate::fleet::config_file("repos-base")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let def = default_repos_base();
    if !hist.iter().any(|x| x == &def) {
        hist.push(def);
    }
    hist
}

/// Remember a repos base: move to front, dedupe, cap. Creating a new path is just
/// using it — the directory itself is made when a project is placed.
pub fn set_repos_base(path: &str) -> Result<(), String> {
    let p = path.trim().to_string();
    if p.is_empty() {
        return Ok(());
    }
    let hist = push_history(repos_bases(), &p, 8);
    let f = crate::fleet::config_file("repos-base").ok_or("no $HOME set")?;
    if let Some(dir) = f.parent() {
        std::fs::create_dir_all(dir).map_err(|e| format!("create {}: {e}", dir.display()))?;
    }
    std::fs::write(&f, format!("{}\n", hist.join("\n"))).map_err(|e| format!("write {}: {e}", f.display()))
}

/// Move `item` to the front of `list`, dedupe, cap length. Pure.
pub fn push_history(mut list: Vec<String>, item: &str, cap: usize) -> Vec<String> {
    list.retain(|x| x != item);
    list.insert(0, item.to_string());
    list.truncate(cap);
    list
}

/// The folders gitmind scans for repos/duplicates. Defaults to $HOME; add a mounted drive
/// or a specific project area to scope discovery (and keep an external disk's copies in
/// view). Always includes $HOME so nothing local is missed.
pub fn scan_roots() -> Vec<String> {
    let mut roots: Vec<String> = crate::fleet::config_file("scan-roots")
        .and_then(|p| std::fs::read_to_string(p).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() && !roots.iter().any(|x| x == &home) {
            roots.insert(0, home);
        }
    }
    roots
}

/// Add a scan root (e.g. an external drive's mountpoint). $HOME stays implicit.
pub fn add_scan_root(path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let p = expand_home(path.trim(), &home);
    if p.is_empty() { return Ok(()); }
    let existing: Vec<String> = crate::fleet::config_file("scan-roots")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let hist = push_history(existing, &p, 12);
    let f = crate::fleet::config_file("scan-roots").ok_or("no $HOME set")?;
    if let Some(dir) = f.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    std::fs::write(&f, format!("{}\n", hist.join("\n"))).map_err(|e| e.to_string())
}

/// Remove a scan root.
pub fn remove_scan_root(path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let p = expand_home(path.trim(), &home);
    let existing: Vec<String> = crate::fleet::config_file("scan-roots")
        .and_then(|f| std::fs::read_to_string(f).ok())
        .map(|s| s.lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect())
        .unwrap_or_default();
    let kept: Vec<String> = existing.into_iter().filter(|x| x != &p).collect();
    let f = crate::fleet::config_file("scan-roots").ok_or("no $HOME set")?;
    if let Some(dir) = f.parent() { std::fs::create_dir_all(dir).map_err(|e| e.to_string())?; }
    std::fs::write(&f, format!("{}\n", kept.join("\n"))).map_err(|e| e.to_string())
}

/// Expand a leading ~ to `home`. Pure.
pub fn expand_home(path: &str, home: &str) -> String {
    if path == "~" {
        home.to_string()
    } else if let Some(rest) = path.strip_prefix("~/") {
        format!("{home}/{rest}")
    } else {
        path.to_string()
    }
}

/// A filesystem-safe project directory name. Pure.
pub fn sanitize_name(name: &str) -> String {
    let s: String = name.trim().chars()
        .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') { c } else { '-' })
        .collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "app".to_string() } else { s }
}

/// Detect the language/build-system subfolder for a crate dir. Pure (reads the dir).
pub fn detect_lang(dir: &Path) -> &'static str {
    let has = |f: &str| dir.join(f).exists();
    if has("Cargo.toml") { "rust" }
    else if has("CMakeLists.txt") { "cmake" }
    else if has("go.mod") { "go" }
    else if has("package.json") { "node" }
    else if has("pyproject.toml") || has("setup.py") || has("requirements.txt") { "python" }
    else { "other" }
}

/// The suggested destination: <base>/<lang>/<name>, with ~ expanded and name sanitized.
pub fn suggested_dest(base: &str, lang: &str, name: &str, home: &str) -> PathBuf {
    PathBuf::from(expand_home(base, home)).join(lang).join(sanitize_name(name))
}

/// The default .gitmind/config.toml for a freshly-initialized gitmind repo. Pure.
pub fn gitmind_config_toml() -> String {
    "# gitmind repo — git and gitmind side-by-side.\n\
     # The object store (store.redb) is gitignored and rebuilt on demand.\n\
     [gitmind]\n\
     version = 1\n\
     store = \"store.redb\"\n\
     committed_store = false\n\
     zstd_level = 9\n".to_string()
}

/// Lines that should be present in .gitignore for an initialized repo. Delegates to the
/// build-artifact catalog so there's a single source of truth. Pure.
pub fn gitignore_lines(gitmind: bool) -> Vec<&'static str> {
    crate::artifacts::gitignore_lines(gitmind)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn destination_math() {
        assert_eq!(expand_home("~/Code", "/home/me"), "/home/me/Code");
        assert_eq!(expand_home("/abs/path", "/home/me"), "/abs/path");
        assert_eq!(sanitize_name(" My Cool App! "), "My-Cool-App");
        assert_eq!(sanitize_name("///"), "app");
        let d = suggested_dest("~/Code", "rust", "Hello World", "/home/me");
        assert_eq!(d, PathBuf::from("/home/me/Code/rust/Hello-World"));
    }

    #[test]
    fn history_dedupes_front() {
        let h = push_history(vec!["a".into(), "b".into()], "b", 8);
        assert_eq!(h, vec!["b", "a"], "moved to front, no dup");
        let capped = push_history((0..10).map(|i| i.to_string()).collect(), "x", 3);
        assert_eq!(capped, vec!["x", "0", "1"]);
    }

    #[test]
    fn lang_detection_and_init_content() {
        let base = std::env::temp_dir().join(format!("gm-places-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        std::fs::write(base.join("Cargo.toml"), "x").unwrap();
        assert_eq!(detect_lang(&base), "rust");
        std::fs::remove_file(base.join("Cargo.toml")).unwrap();
        std::fs::write(base.join("CMakeLists.txt"), "x").unwrap();
        assert_eq!(detect_lang(&base), "cmake");
        assert!(gitmind_config_toml().contains("[gitmind]"));
        assert!(gitignore_lines(true).contains(&"/.gitmind/store.redb"));
        assert!(!gitignore_lines(false).contains(&"/.gitmind/store.redb"));
        let _ = std::fs::remove_dir_all(&base);
    }
}

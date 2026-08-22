//! icontheme — resolve a bare icon name (`.desktop`'s `Icon=firefox`) to an
//! actual file, per the freedesktop icon theme spec's directory-matching
//! rules. This is the seam that turns [`crate::shell`]'s initial-letter glyph
//! placeholder into a real icon: pure enough to test against synthetic theme
//! trees, isolated from anything that touches a GPU or a display.
//!
//! What's covered: `index.theme` parsing, `Inherits` (recursive, `hicolor`
//! always appended per spec), and the three directory types' size-matching
//! (`Fixed`, `Scalable`, `Threshold`). What's deliberately NOT covered: SVG
//! rasterization (out of scope — the caller only decodes what it can, and
//! PNG is what most installed themes ship at the sizes a launcher tile
//! wants), `@2x`/HiDPI `Scale` beyond parsing it, and cache validation
//! (`icon-theme.cache`) — a directory walk is simpler and correct, just
//! slower than the binary cache; fine for a few hundred apps scanned once.

use std::path::{Path, PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum DirType {
    Fixed,
    Scalable,
    Threshold,
}

#[derive(Clone, Debug, PartialEq)]
struct IconDir {
    /// Relative path, e.g. `"48x48/apps"`.
    path: String,
    size: u32,
    scale: u32,
    min_size: u32,
    max_size: u32,
    threshold: u32,
    dir_type: DirType,
}

/// A parsed `index.theme`: enough to search its directories and follow
/// inheritance.
#[derive(Clone, Debug, PartialEq)]
pub struct ThemeIndex {
    pub name: String,
    pub inherits: Vec<String>,
    dirs: Vec<IconDir>,
}

/// Parse an `index.theme` file body (the `[Icon Theme]` header plus one
/// section per directory).
pub fn parse_index_theme(text: &str) -> ThemeIndex {
    let mut name = String::new();
    let mut inherits = Vec::new();
    let mut dirs = Vec::new();

    let mut section = String::new();
    let mut cur: Option<IconDir> = None;
    let mut directories_list: Vec<String> = Vec::new();

    let flush = |cur: &mut Option<IconDir>, dirs: &mut Vec<IconDir>| {
        if let Some(d) = cur.take() {
            dirs.push(d);
        }
    };

    for raw in text.lines() {
        let line = raw.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(s) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
            flush(&mut cur, &mut dirs);
            section = s.to_string();
            if section != "Icon Theme" {
                cur = Some(IconDir {
                    path: section.clone(),
                    size: 48,
                    scale: 1,
                    min_size: 0,
                    max_size: 0,
                    threshold: 2,
                    dir_type: DirType::Threshold,
                });
            }
            continue;
        }
        let Some((key, value)) = line.split_once('=') else { continue };
        let (key, value) = (key.trim(), value.trim());
        if section == "Icon Theme" {
            match key {
                "Name" => name = value.to_string(),
                "Inherits" => inherits = value.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                "Directories" => directories_list = value.split(',').filter(|s| !s.is_empty()).map(String::from).collect(),
                _ => {}
            }
        } else if let Some(d) = cur.as_mut() {
            match key {
                "Size" => d.size = value.parse().unwrap_or(d.size),
                "Scale" => d.scale = value.parse().unwrap_or(1),
                "MinSize" => d.min_size = value.parse().unwrap_or(0),
                "MaxSize" => d.max_size = value.parse().unwrap_or(0),
                "Threshold" => d.threshold = value.parse().unwrap_or(2),
                "Type" => {
                    d.dir_type = match value {
                        "Fixed" => DirType::Fixed,
                        "Scalable" => DirType::Scalable,
                        _ => DirType::Threshold,
                    }
                }
                _ => {}
            }
        }
    }
    flush(&mut cur, &mut dirs);

    // MinSize/MaxSize default to Size when unset (spec default).
    for d in &mut dirs {
        if d.min_size == 0 {
            d.min_size = d.size;
        }
        if d.max_size == 0 {
            d.max_size = d.size;
        }
    }

    // A theme with a `Directories=` list but sections that came out empty
    // (malformed/missing section headers) still gets its declared dirs at
    // the spec default (48/Threshold) rather than silently searching none.
    if dirs.is_empty() && !directories_list.is_empty() {
        dirs = directories_list
            .into_iter()
            .map(|path| IconDir { path, size: 48, scale: 1, min_size: 48, max_size: 48, threshold: 2, dir_type: DirType::Threshold })
            .collect();
    }

    ThemeIndex { name, inherits, dirs }
}

/// Does this directory's declared size range cover `size`, per the spec's
/// three directory-type rules?
fn dir_matches(d: &IconDir, size: u32, scale: u32) -> bool {
    if d.scale != scale {
        return false;
    }
    match d.dir_type {
        DirType::Fixed => d.size == size,
        DirType::Scalable => size >= d.min_size && size <= d.max_size,
        DirType::Threshold => {
            let lo = d.size.saturating_sub(d.threshold);
            let hi = d.size + d.threshold;
            size >= lo && size <= hi
        }
    }
}

/// Spec's distance function, for picking the closest directory when nothing
/// matches exactly (also used to rank multiple matches).
fn dir_distance(d: &IconDir, size: u32) -> u32 {
    match d.dir_type {
        DirType::Fixed => size.abs_diff(d.size),
        DirType::Scalable => {
            if size < d.min_size {
                d.min_size - size
            } else if size > d.max_size {
                size - d.max_size
            } else {
                0
            }
        }
        DirType::Threshold => {
            if size < d.size.saturating_sub(d.threshold) {
                d.size - size
            } else if size > d.size + d.threshold {
                size - d.size
            } else {
                0
            }
        }
    }
}

const EXTENSIONS: [&str; 3] = ["png", "svg", "xpm"];

/// Search one theme's directories (already parsed) under `theme_base` for
/// `icon_name` at `size`. Exact-size matches win; otherwise the closest
/// directory by the spec's distance function.
fn find_in_theme_dir(theme_base: &Path, theme: &ThemeIndex, icon_name: &str, size: u32, scale: u32) -> Option<PathBuf> {
    let mut best: Option<(u32, PathBuf)> = None;
    for d in &theme.dirs {
        for ext in EXTENSIONS {
            let candidate = theme_base.join(&d.path).join(format!("{icon_name}.{ext}"));
            if !candidate.is_file() {
                continue;
            }
            let matched = dir_matches(d, size, scale);
            let dist = if matched { 0 } else { dir_distance(d, size) };
            if matched {
                return Some(candidate); // spec: first exact match wins, don't keep scanning
            }
            if best.as_ref().is_none_or(|(bd, _)| dist < *bd) {
                best = Some((dist, candidate));
            }
        }
    }
    best.map(|(_, p)| p)
}

/// Load and parse a theme's `index.theme` from the first base dir that has
/// one for it.
fn load_theme(icon_dirs: &[PathBuf], theme_name: &str) -> Option<(PathBuf, ThemeIndex)> {
    for base in icon_dirs {
        let theme_dir = base.join(theme_name);
        let idx_path = theme_dir.join("index.theme");
        if let Ok(text) = std::fs::read_to_string(&idx_path) {
            return Some((theme_dir, parse_index_theme(&text)));
        }
    }
    None
}

/// Resolve `icon_name` to a file, per spec: search `theme_name`'s directories,
/// then its `Inherits` chain (recursively), then `hicolor` (always implied),
/// then the unthemed fallback (`<icon_dirs>/<name>.<ext>` and `pixmaps/`).
/// `icon_dirs` are icon theme roots in precedence order (see
/// [`crate::desktop::icon_theme_dirs`]); `size`/`scale` pick the directory.
pub fn find_icon(icon_dirs: &[PathBuf], theme_name: &str, icon_name: &str, size: u32, scale: u32) -> Option<PathBuf> {
    if icon_name.is_empty() {
        return None;
    }
    // Themes already visited, so a cycle in Inherits can't loop forever.
    let mut visited = std::collections::HashSet::new();
    let mut queue = vec![theme_name.to_string()];
    while let Some(name) = queue.pop() {
        if !visited.insert(name.clone()) {
            continue;
        }
        if let Some((base, theme)) = load_theme(icon_dirs, &name) {
            if let Some(p) = find_in_theme_dir(&base, &theme, icon_name, size, scale) {
                return Some(p);
            }
            queue.extend(theme.inherits.iter().cloned());
        }
        if name != "hicolor" {
            queue.push("hicolor".to_string()); // spec: implicit fallback for every theme
        }
    }
    // Unthemed fallback: the icon dirs themselves, and each root's pixmaps/.
    for base in icon_dirs {
        for ext in EXTENSIONS {
            let direct = base.join(format!("{icon_name}.{ext}"));
            if direct.is_file() {
                return Some(direct);
            }
        }
        if let Some(parent) = base.parent() {
            for ext in EXTENSIONS {
                let px = parent.join("pixmaps").join(format!("{icon_name}.{ext}"));
                if px.is_file() {
                    return Some(px);
                }
            }
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    struct Fixture {
        root: PathBuf,
    }
    impl Fixture {
        fn new(tag: &str) -> Fixture {
            let root = std::env::temp_dir().join(format!("launcher-icontheme-test-{tag}-{}", std::process::id()));
            let _ = fs::remove_dir_all(&root);
            fs::create_dir_all(&root).unwrap();
            Fixture { root }
        }
        fn theme(&self, name: &str, index_theme: &str) -> PathBuf {
            let dir = self.root.join(name);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join("index.theme"), index_theme).unwrap();
            dir
        }
        fn put_icon(&self, theme: &str, subdir: &str, file: &str) {
            let dir = self.root.join(theme).join(subdir);
            fs::create_dir_all(&dir).unwrap();
            fs::write(dir.join(file), b"not a real png, just a marker").unwrap();
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.root);
        }
    }

    #[test]
    fn parses_index_theme_with_defaults() {
        let idx = parse_index_theme(
            "[Icon Theme]\nName=Test\nInherits=hicolor\n\n[48x48/apps]\nSize=48\nContext=Applications\nType=Fixed\n",
        );
        assert_eq!(idx.name, "Test");
        assert_eq!(idx.inherits, vec!["hicolor"]);
        assert_eq!(idx.dirs.len(), 1);
        assert_eq!(idx.dirs[0].size, 48);
        assert_eq!(idx.dirs[0].dir_type, DirType::Fixed);
        // MinSize/MaxSize default to Size when unset.
        assert_eq!(idx.dirs[0].min_size, 48);
        assert_eq!(idx.dirs[0].max_size, 48);
    }

    #[test]
    fn exact_fixed_size_match() {
        let f = Fixture::new("exact");
        f.theme("Test", "[Icon Theme]\nName=Test\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        f.put_icon("Test", "48x48/apps", "firefox.png");
        let found = find_icon(&[f.root.clone()], "Test", "firefox", 48, 1).unwrap();
        assert!(found.ends_with("48x48/apps/firefox.png"));
    }

    #[test]
    fn threshold_dir_matches_nearby_sizes_and_falls_back_beyond_it() {
        let f = Fixture::new("threshold");
        f.theme("Test", "[Icon Theme]\nName=Test\n\n[48x48/apps]\nSize=48\nThreshold=4\nType=Threshold\n");
        f.put_icon("Test", "48x48/apps", "dolphin.png");
        // 50 is within [48-4, 48+4] = [44,52]: an exact directory match.
        assert!(find_icon(&[f.root.clone()], "Test", "dolphin", 50, 1).is_some());
        // 60 is outside the threshold band, but the spec still wants the
        // closest available directory over showing nothing — same file,
        // found via the distance fallback rather than an exact match.
        let far = find_icon(&[f.root.clone()], "Test", "dolphin", 60, 1).unwrap();
        assert!(far.ends_with("48x48/apps/dolphin.png"));
    }

    #[test]
    fn falls_back_to_closest_size_when_no_exact_match() {
        let f = Fixture::new("closest");
        f.theme(
            "Test",
            "[Icon Theme]\nName=Test\n\n[16x16/apps]\nSize=16\nType=Fixed\n\n[128x128/apps]\nSize=128\nType=Fixed\n",
        );
        f.put_icon("Test", "16x16/apps", "konsole.png");
        f.put_icon("Test", "128x128/apps", "konsole.png");
        // Asking for 48: 128 is the closer of the two Fixed sizes (80 away
        // vs 32 away)... wait, 48-16=32, 128-48=80: 16x16 is closer.
        let found = find_icon(&[f.root.clone()], "Test", "konsole", 48, 1).unwrap();
        assert!(found.ends_with("16x16/apps/konsole.png"));
    }

    #[test]
    fn inherits_chain_is_followed() {
        let f = Fixture::new("inherit");
        f.theme("Child", "[Icon Theme]\nName=Child\nInherits=Parent\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        f.theme("Parent", "[Icon Theme]\nName=Parent\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        // Only the parent theme actually ships the icon.
        f.put_icon("Parent", "48x48/apps", "files.png");
        let found = find_icon(&[f.root.clone()], "Child", "files", 48, 1).unwrap();
        assert!(found.ends_with("Parent/48x48/apps/files.png"));
    }

    #[test]
    fn hicolor_is_an_implicit_fallback() {
        let f = Fixture::new("hicolor");
        f.theme("Breeze", "[Icon Theme]\nName=Breeze\n\n[48x48/apps]\nSize=48\nType=Fixed\n"); // no Inherits at all
        f.theme("hicolor", "[Icon Theme]\nName=Hicolor\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        f.put_icon("hicolor", "48x48/apps", "vlc.png");
        let found = find_icon(&[f.root.clone()], "Breeze", "vlc", 48, 1).unwrap();
        assert!(found.ends_with("hicolor/48x48/apps/vlc.png"));
    }

    #[test]
    fn unthemed_pixmaps_fallback() {
        let f = Fixture::new("pixmaps");
        // icon_dirs entries are typically .../icons; pixmaps/ is a sibling.
        let icons_root = f.root.join("icons");
        fs::create_dir_all(&icons_root).unwrap();
        let pixmaps = f.root.join("pixmaps");
        fs::create_dir_all(&pixmaps).unwrap();
        fs::write(pixmaps.join("oldapp.xpm"), b"marker").unwrap();
        let found = find_icon(&[icons_root], "Breeze", "oldapp", 48, 1).unwrap();
        assert!(found.ends_with("pixmaps/oldapp.xpm"));
    }

    #[test]
    fn missing_icon_is_none_not_a_panic() {
        let f = Fixture::new("missing");
        f.theme("Test", "[Icon Theme]\nName=Test\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        assert!(find_icon(&[f.root.clone()], "Test", "does-not-exist", 48, 1).is_none());
    }

    #[test]
    fn inherits_cycle_does_not_hang() {
        let f = Fixture::new("cycle");
        f.theme("A", "[Icon Theme]\nName=A\nInherits=B\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        f.theme("B", "[Icon Theme]\nName=B\nInherits=A\n\n[48x48/apps]\nSize=48\nType=Fixed\n");
        // Neither ships the icon; this must terminate and return None.
        assert!(find_icon(&[f.root.clone()], "A", "nope", 48, 1).is_none());
    }
}

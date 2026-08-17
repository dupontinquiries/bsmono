//! icons — resolving an `Icon=` name to a file on disk.
//!
//! A deliberate subset of the icon-theme spec, sized for a launcher:
//!
//! * Absolute paths pass through.
//! * Themes are searched in order: the configured theme, its `Inherits=`
//!   chain (from `index.theme`, breadth-first, cycle-guarded), then
//!   `hicolor`, then the flat `pixmaps` dir.
//! * Within a theme we probe a fixed size ladder near the requested size
//!   rather than parsing every `Directories=` stanza — the ladder covers what
//!   real themes ship, and a probe that misses costs one `stat`.
//! * `.png` and `.svg` are accepted (`.svg` also from `scalable/`); `.xpm`
//!   is not — nothing renders it downstream.
//!
//! What this does NOT do: Scale=2 directories, Threshold matching, per-dir
//! MinSize/MaxSize. If a theme's exotic layout ever matters, that's the seam
//! to widen — the search loop below is the whole algorithm.

use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// Contexts a launcher actually pulls from. `apps` first because that is the
/// overwhelmingly common hit; the rest cover the system row (shutdown etc.).
const CONTEXTS: &[&str] = &["apps", "actions", "devices", "places", "status", "categories"];

/// The size ladder, nearest-first around a hint chosen at call time.
const SIZES: &[u32] = &[512, 256, 192, 128, 96, 64, 48, 32, 24, 22, 16];

/// Roots that can contain `icons/<theme>/…`, precedence order.
pub fn icon_roots() -> Vec<PathBuf> {
    let mut roots = Vec::new();
    roots.push(config_kit::data_home().join("icons"));
    if let Some(home) = std::env::var_os("HOME") {
        roots.push(PathBuf::from(home).join(".icons"));
    }
    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in sys.split(':').filter(|d| !d.is_empty()) {
        roots.push(PathBuf::from(d).join("icons"));
    }
    roots
}

/// Read a theme's `Inherits=` list from any root that has its index.theme.
fn inherits(roots: &[PathBuf], theme: &str) -> Vec<String> {
    for root in roots {
        let idx = root.join(theme).join("index.theme");
        let Ok(text) = std::fs::read_to_string(&idx) else { continue };
        for line in text.lines() {
            let line = line.trim();
            if let Some(v) = line.strip_prefix("Inherits=") {
                return v.split(',').map(|s| s.trim().to_string()).filter(|s| !s.is_empty()).collect();
            }
        }
        return Vec::new(); // index.theme found, no Inherits
    }
    Vec::new()
}

/// The theme search order: `theme`, its inherits chain, then hicolor.
fn theme_chain(roots: &[PathBuf], theme: &str) -> Vec<String> {
    let mut chain = Vec::new();
    let mut seen = HashSet::new();
    let mut queue = vec![theme.to_string()];
    while let Some(t) = queue.pop() {
        if !seen.insert(t.clone()) || chain.len() > 8 {
            continue;
        }
        // Push inherits in reverse so they pop in declared order.
        for parent in inherits(roots, &t).into_iter().rev() {
            queue.push(parent);
        }
        chain.push(t);
    }
    if !seen.contains("hicolor") {
        chain.push("hicolor".to_string());
    }
    chain
}

fn probe(dir: &Path, name: &str) -> Option<PathBuf> {
    for ext in ["png", "svg"] {
        let p = dir.join(format!("{name}.{ext}"));
        if p.is_file() {
            return Some(p);
        }
    }
    None
}

/// Resolve an icon name (or absolute path) to a file, preferring sizes at or
/// above `size_hint` (nearest), falling back through the chain and pixmaps.
pub fn resolve(roots: &[PathBuf], theme: &str, name: &str, size_hint: u32) -> Option<PathBuf> {
    if name.is_empty() {
        return None;
    }
    // Absolute path (some entries ship one, sometimes without extension).
    if name.starts_with('/') {
        let p = Path::new(name);
        if p.is_file() {
            return Some(p.to_path_buf());
        }
        return probe(p.parent()?, p.file_name()?.to_str()?);
    }

    // Nearest-size order: exact/above ascending distance, then below.
    let mut sizes: Vec<u32> = SIZES.to_vec();
    sizes.sort_by_key(|&s| {
        if s >= size_hint { (0, s - size_hint) } else { (1, size_hint - s) }
    });

    // SPEC FIRST. icontheme reads each theme's index.theme and searches the
    // directories it declares; the grid walk below only finds icons in themes
    // shaped the way the grid guesses. Trying the spec path first means a
    // theme with `scalable/apps` or any other legal layout resolves properly,
    // and the grid stays as the fallback for themes that ship no index.theme
    // at all — which exists, and which the spec implementation cannot serve.
    if let Some(p) = crate::icontheme::find_icon(roots, theme, name, size_hint, 1) {
        return Some(p);
    }

    for t in theme_chain(roots, theme) {
        for root in roots {
            let troot = root.join(&t);
            if !troot.is_dir() {
                continue;
            }
            for &s in &sizes {
                for ctx in CONTEXTS {
                    if let Some(p) = probe(&troot.join(format!("{s}x{s}")).join(ctx), name) {
                        return Some(p);
                    }
                }
            }
            for ctx in CONTEXTS {
                if let Some(p) = probe(&troot.join("scalable").join(ctx), name) {
                    return Some(p);
                }
                // Breeze and friends also ship `symbolic/` and bare context dirs.
                if let Some(p) = probe(&troot.join(ctx), name) {
                    return Some(p);
                }
            }
        }
    }

    // Last resort: /usr/share/pixmaps and siblings.
    let sys = std::env::var("XDG_DATA_DIRS")
        .unwrap_or_else(|_| "/usr/local/share:/usr/share".to_string());
    for d in sys.split(':').filter(|d| !d.is_empty()) {
        if let Some(p) = probe(&Path::new(d).join("pixmaps"), name) {
            return Some(p);
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tree(tag: &str) -> PathBuf {
        let t = std::env::temp_dir().join(format!("hatch-icons-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&t);
        t
    }

    fn touch(p: PathBuf) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(&p, b"x").unwrap();
    }

    #[test]
    fn the_configured_theme_wins_over_hicolor() {
        let root = tree("theme-wins");
        touch(root.join("mytheme/48x48/apps/editor.png"));
        touch(root.join("hicolor/48x48/apps/editor.png"));
        let p = resolve(&[root.clone()], "mytheme", "editor", 48).unwrap();
        assert!(p.starts_with(root.join("mytheme")), "{p:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn inherits_chain_is_followed_before_hicolor() {
        let root = tree("inherits");
        touch(root.join("child/index.theme"));
        std::fs::write(root.join("child/index.theme"), "[Icon Theme]\nInherits=parent\n").unwrap();
        touch(root.join("parent/64x64/apps/cam.png"));
        touch(root.join("hicolor/64x64/apps/cam.png"));
        let p = resolve(&[root.clone()], "child", "cam", 64).unwrap();
        assert!(p.starts_with(root.join("parent")), "{p:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn nearest_size_at_or_above_the_hint_is_preferred() {
        let root = tree("nearest");
        touch(root.join("hicolor/32x32/apps/a.png"));
        touch(root.join("hicolor/128x128/apps/a.png"));
        let p = resolve(&[root.clone()], "hicolor", "a", 64).unwrap();
        assert!(p.to_string_lossy().contains("128x128"), "{p:?}");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn scalable_svg_is_found_when_no_raster_exists() {
        let root = tree("scalable");
        touch(root.join("hicolor/scalable/apps/vec.svg"));
        let p = resolve(&[root.clone()], "hicolor", "vec", 64).unwrap();
        assert!(p.extension().unwrap() == "svg");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn a_theme_whose_layout_the_grid_cannot_guess_now_resolves() {
        // The reason for porting icontheme in. This theme declares its
        // directories in index.theme as `apps/48` — legal, used in the wild,
        // and invisible to a hardcoded `48x48/apps` grid. Before the spec
        // path went in front, this returned None.
        let root = tree("indexed");
        let theme = root.join("wonky");
        std::fs::create_dir_all(&theme).unwrap();
        std::fs::write(
            theme.join("index.theme"),
            "[Icon Theme]\nName=Wonky\nDirectories=apps/48\n\n\
             [apps/48]\nSize=48\nType=Fixed\n",
        )
        .unwrap();
        touch(theme.join("apps/48/firefox.png"));

        let got = resolve(&[root.clone()], "wonky", "firefox", 48);
        assert_eq!(got, Some(theme.join("apps/48/firefox.png")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn the_grid_still_serves_a_theme_with_no_index_file() {
        // Kept as the fallback on purpose: themes that ship no index.theme
        // exist, and the spec implementation cannot resolve them at all.
        let root = tree("gridonly");
        touch(root.join("plain/48x48/apps/thunderbird.png"));
        let got = resolve(&[root.clone()], "plain", "thunderbird", 48);
        assert_eq!(got, Some(root.join("plain/48x48/apps/thunderbird.png")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn an_absolute_path_passes_through_and_a_missing_name_is_none() {
        let root = tree("abs");
        let f = root.join("direct.png");
        touch(f.clone());
        assert_eq!(resolve(&[], "any", f.to_str().unwrap(), 48), Some(f));
        assert_eq!(resolve(&[root.clone()], "hicolor", "nope", 48), None);
        let _ = std::fs::remove_dir_all(&root);
    }
}

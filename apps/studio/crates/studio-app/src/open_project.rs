//! open_project — choosing a project to open, from the palette.
//!
//! Two ways to a file, so choosing never dead-ends:
//!
//! * the native file picker (the `picker` feature: rfd over the XDG portal on
//!   Linux, filtered to Studio projects);
//! * a list of projects found nearby -- where Studio was started, and the
//!   projects folder -- a few levels deep. Run from the repository, that list
//!   holds the demo fixtures. It is also the whole answer on a desktop with no
//!   portal, where the picker can only return nothing.
//!
//! Switching never loses work and never writes a file that was only looked
//! at: if the live project differs from the one last opened or saved, the
//! palette asks first (save then open / discard and open / stay).

use std::path::{Path, PathBuf};

/// What Studio opens: a project file, or a project archive.
pub const EXTENSIONS: [&str; 2] = ["studio", "studioarch"];

pub fn is_project(p: &Path) -> bool {
    p.extension().and_then(|e| e.to_str()).map(|e| EXTENSIONS.contains(&e)).unwrap_or(false)
}

/// Folders never worth walking: hidden ones, build output, dependency caches.
fn skip(name: &str) -> bool {
    name.starts_with('.') || matches!(name, "target" | "node_modules" | "__pycache__")
}

/// Projects under `roots`, at most `depth` folders down, sorted and without
/// duplicates, at most `limit` of them.
pub fn find(roots: &[PathBuf], depth: usize, limit: usize) -> Vec<PathBuf> {
    let mut out: Vec<PathBuf> = Vec::new();
    fn walk(dir: &Path, depth: usize, out: &mut Vec<PathBuf>, limit: usize) {
        if out.len() >= limit * 4 { return; }
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        let mut entries: Vec<_> = rd.filter_map(|e| e.ok()).collect();
        entries.sort_by_key(|e| e.file_name());
        for e in entries {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            let Ok(ft) = e.file_type() else { continue };
            if ft.is_file() && is_project(&p) { out.push(p); }
            else if ft.is_dir() && depth > 0 && !skip(&name) { walk(&p, depth - 1, out, limit); }
        }
    }
    for r in roots { walk(r, depth, &mut out, limit); }
    let mut seen = std::collections::BTreeSet::new();
    out.retain(|p| seen.insert(p.canonicalize().unwrap_or_else(|_| p.clone())));
    out.sort();
    out.truncate(limit);
    out
}

/// Where to look: where Studio was started, and the projects folder.
pub fn search_roots() -> Vec<PathBuf> {
    let mut r = Vec::new();
    if let Ok(cwd) = std::env::current_dir() { r.push(cwd); }
    r.push(crate::project_dir::projects_root());
    r
}

/// The projects to offer, the same list every time it is asked for (the
/// palette refers to a project by its place in this list).
pub fn nearby() -> Vec<PathBuf> { find(&search_roots(), 4, 40) }

/// A project's label in the palette: its file name, and the last two folders
/// it is in -- enough to tell two projects apart. The full path was long
/// enough to overprint the file name in the palette's row (seen on screen).
pub fn label(p: &Path) -> (String, String) {
    let name = p.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default();
    let parts: Vec<String> = p.parent().map(|d| d.components().rev().take(2)
        .map(|c| c.as_os_str().to_string_lossy().into_owned()).collect()).unwrap_or_default();
    let folder = parts.into_iter().rev().filter(|c| c != "/").collect::<Vec<_>>().join("/");
    (name, folder)
}

/// Whether switching away would lose work: the live project differs from the
/// one last opened or saved.
pub fn needs_save(current: &studio_core::Project, saved: Option<&studio_core::Project>) -> bool {
    saved.map(|s| s != current).unwrap_or(true)
}

/// The native file picker, or `None` when the feature is off, the desktop has
/// no portal to ask, or the person cancelled -- the caller cannot tell those
/// apart, so it always falls back to the list rather than doing nothing.
#[cfg(feature = "picker")]
pub fn pick() -> Option<PathBuf> {
    rfd::FileDialog::new()
        .set_title("Open a Studio project")
        .add_filter("Studio project", &EXTENSIONS)
        .pick_file()
}
#[cfg(not(feature = "picker"))]
pub fn pick() -> Option<PathBuf> { None }

#[cfg(test)]
mod tests {
    use super::*;
    use studio_core::*;

    #[test]
    fn projects_are_found_nearby_and_junk_is_skipped() {
        let d = std::env::temp_dir().join(format!("open-proj-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        for (path, body) in [("fixtures/cartoon-fragile/fragile-cartoon.studio", "x"), ("fixtures/a/b/deep.studio", "x"),
                             ("backup.studioarch", "x"), ("notes.txt", "x"), ("target/debug/stale.studio", "x"),
                             (".cache/hidden.studio", "x"), ("one/two/three/four/five/too-deep.studio", "x")] {
            let p = d.join(path);
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, body).unwrap();
        }
        let got: Vec<String> = find(&[d.clone(), d.clone()], 4, 40).iter()
            .map(|p| p.strip_prefix(&d).unwrap().display().to_string()).collect();
        assert_eq!(got, vec!["backup.studioarch", "fixtures/a/b/deep.studio", "fixtures/cartoon-fragile/fragile-cartoon.studio"],
                   "archives too; not build output, hidden folders, other files, or past the depth; each once");
        assert_eq!(find(&[d.clone()], 4, 2).len(), 2, "the limit holds");
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn switching_asks_only_when_the_project_changed() {
        let mut p = Project::default();
        let saved = p.clone();
        assert!(!needs_save(&p, Some(&saved)), "opened and only looked at: switch straight away");
        p.apply(&Op::SetMarker { at: Ticks::from_seconds(1.0), name: "a note".into() }).unwrap();
        assert!(needs_save(&p, Some(&saved)), "an edit since: ask first");
        assert!(needs_save(&p, None), "no record of a save: ask, never assume");
    }

    #[test]
    fn labels_name_the_file_and_its_folder() {
        let (n, f) = label(Path::new("/home/me/films/fragile/fragile-cartoon.studio"));
        assert_eq!((n.as_str(), f.as_str()), ("fragile-cartoon.studio", "films/fragile"), "the last two folders, not the whole path");
        assert_eq!(label(Path::new("/x.studio")).1, "", "at the root there is no folder to show");
        assert!(is_project(Path::new("x.studio")) && is_project(Path::new("x.studioarch")) && !is_project(Path::new("x.mp4")));
    }
}

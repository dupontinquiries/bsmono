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

/// What the command line asks to open.
#[derive(Debug, Clone, PartialEq)]
pub enum Launch {
    /// Open this project (it may not exist yet: Ctrl+S creates it, as with STUDIO_PROJECT).
    Project(PathBuf),
    /// Print the usage and stop.
    Help,
    /// Nothing on the command line: STUDIO_PROJECT, or the default.
    Default,
    /// Something that is not a project; the message says what was wrong.
    Refused(String),
}

pub const USAGE: &str = "usage: studio [PROJECT | FOLDER]

  PROJECT   a .studio project or .studioarch archive to open
  FOLDER    a folder holding exactly one project, e.g. fixtures/cartoon-fragile

With nothing given, STUDIO_PROJECT is opened if set, else your last untitled project.
In the app: Ctrl+P (or Shift+Space) -> 'open project' to switch.";

/// Read the command line (without the program name). A project path wins
/// over STUDIO_PROJECT; a folder with exactly one project opens that project.
pub fn from_args(args: &[String]) -> Launch {
    let rest: Vec<&String> = args.iter().filter(|a| !a.is_empty()).collect();
    if rest.iter().any(|a| *a == "-h" || *a == "--help") { return Launch::Help; }
    let Some(first) = rest.first() else { return Launch::Default };
    if rest.len() > 1 { return Launch::Refused(format!("one project at a time; got {} arguments", rest.len())); }
    let p = PathBuf::from(first);
    if p.is_dir() {
        let found: Vec<PathBuf> = std::fs::read_dir(&p).map(|rd| rd.filter_map(|e| e.ok()).map(|e| e.path())
            .filter(|q| q.is_file() && is_project(q)).collect()).unwrap_or_default();
        return match found.len() {
            1 => Launch::Project(found.into_iter().next().unwrap()),
            0 => Launch::Refused(format!("no .studio project in {}", p.display())),
            n => Launch::Refused(format!("{n} projects in {}; name one", p.display())),
        };
    }
    if is_project(&p) { return Launch::Project(p); }
    if first.starts_with('-') { return Launch::Refused(format!("unknown option {first}")); }
    Launch::Refused(format!("not a Studio project (.studio or .studioarch): {first}"))
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
    fn the_command_line_opens_a_project_or_a_folder_with_one() {
        let d = std::env::temp_dir().join(format!("launch-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join("one")).unwrap();
        std::fs::create_dir_all(d.join("two")).unwrap();
        std::fs::create_dir_all(d.join("none")).unwrap();
        std::fs::write(d.join("one/film.studio"), "x").unwrap();
        std::fs::write(d.join("one/notes.txt"), "x").unwrap();
        std::fs::write(d.join("two/a.studio"), "x").unwrap();
        std::fs::write(d.join("two/b.studioarch"), "x").unwrap();
        let s = |p: &Path| p.display().to_string();
        assert_eq!(from_args(&[]), Launch::Default);
        assert_eq!(from_args(&[s(&d.join("one/film.studio"))]), Launch::Project(d.join("one/film.studio")));
        assert_eq!(from_args(&[s(&d.join("one"))]), Launch::Project(d.join("one/film.studio")), "a folder with one project");
        assert_eq!(from_args(&["new-film.studio".into()]), Launch::Project("new-film.studio".into()), "not there yet: Ctrl+S makes it");
        assert!(matches!(from_args(&[s(&d.join("two"))]), Launch::Refused(m) if m.contains("2 projects")));
        assert!(matches!(from_args(&[s(&d.join("none"))]), Launch::Refused(m) if m.contains("no .studio")));
        assert!(matches!(from_args(&["clip.mp4".into()]), Launch::Refused(m) if m.contains("not a Studio project")));
        assert!(matches!(from_args(&["--fullscreen".into()]), Launch::Refused(m) if m.contains("unknown option")));
        assert!(matches!(from_args(&["a.studio".into(), "b.studio".into()]), Launch::Refused(_)));
        assert_eq!(from_args(&["--help".into()]), Launch::Help);
        let _ = std::fs::remove_dir_all(&d);
    }

    #[test]
    fn labels_name_the_file_and_its_folder() {
        let (n, f) = label(Path::new("/home/me/films/fragile/fragile-cartoon.studio"));
        assert_eq!((n.as_str(), f.as_str()), ("fragile-cartoon.studio", "films/fragile"), "the last two folders, not the whole path");
        assert_eq!(label(Path::new("/x.studio")).1, "", "at the root there is no folder to show");
        assert!(is_project(Path::new("x.studio")) && is_project(Path::new("x.studioarch")) && !is_project(Path::new("x.mp4")));
    }
}

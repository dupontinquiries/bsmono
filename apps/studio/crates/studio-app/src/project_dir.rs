//! project_dir — the versioned save structure around `project.studio`.
//!
//! The `.studio` file already carries everything a session is — the snapshot
//! ops (tracks, clips, busses, effect chains, sends, automation), the undo
//! history when asked, and the rack (channels, patterns, rolls) — and its
//! media lives beside it as relative paths, so a project folder moves as a
//! unit. What it did not have is TIME: a save overwrote the last one, and
//! "the mix from Tuesday" was gone. This module makes the folder a git
//! repository: every save is a commit, history is `git log`, and getting
//! Tuesday back is a checkout.
//!
//! ## Why git is safe here, and where it is not
//!
//! Git delta-compresses text beautifully and mangles nothing — but asking
//! it to delta compressed media wastes time finding deltas that do not
//! exist. `.gitattributes` marks the media types `-delta`: each blob is
//! stored once, zlib-wrapped, never diffed. The real compression rule is
//! the asset store's, upheld one layer up: **never store PCM** — a `.opus`
//! stays `.opus`, a bounce is a one-time WAV the ops reference. For
//! projects whose media outgrows git entirely (video), [`MediaMode::
//! External`] gitignores `media/` and commits a MANIFEST instead: name and
//! byte length per file, so every version of the project still says which
//! media it needs while the bytes live in a content-addressed store —
//! `apps/gitmind`'s chunk store (AV1 transcode for video, near-dup linking
//! for stills; see its `MEDIA.md`) is the intended tenant of that seam,
//! reached by path rather than by a cross-app crate dependency the repo's
//! layering would refuse.
//!
//! ## The containment rule (learned the hard way)
//!
//! The first version of this ran `git init` and `git add -A` in whatever
//! directory the project file happened to sit in. Launched from a source
//! checkout, that directory is the USER'S OWN REPOSITORY: a save committed
//! 2,734 files of their source tree with the message "save". That is a
//! genuinely dangerous bug and the fix is structural, not a patch —
//!
//! * a project lives in ITS OWN directory under a projects root
//!   ([`projects_root`]), never in the current working directory;
//! * that directory is marked ours with [`MARKER`] at creation;
//! * [`guard`] refuses to touch any directory that is not marked ours, and
//!   refuses outright if the directory or ANY ancestor is already a git
//!   work tree we did not create;
//! * commits name explicit pathspecs. There is no `-A` anywhere in this
//!   file anymore, so even a mis-scoped directory could only ever stage
//!   the handful of files a project consists of.
//!
//! ## What other Linux DAWs do, and what this borrows
//!
//! Zrythm makes the project a DIRECTORY under `XDG_DATA_HOME` and keeps
//! timestamped snapshots in `backups/` — the layout copied here, with git
//! playing the role of `backups/` (every save a commit beats N numbered
//! copies). Reaper defaults to a projects ROOT and offers a per-project
//! subdirectory, which is the "never save loose in the cwd" half. Audacity
//! went the other way entirely: `.aup` plus a `_data` folder became the
//! single-file `.aup3` after years of users moving or deleting the folder
//! and breaking their sessions — which is exactly the argument for
//! `.studioarch`, and why THIS directory is the working copy while the
//! archive remains the thing you send.
//!
//! ## Degradation is a message, never a lost save
//!
//! The save itself is the caller's write of `project.studio`; this module
//! only versions what is on disk. No git binary → the save still happened,
//! the status line says history was skipped. Nothing to commit → that is a
//! fact (`Ok(None)`), not an error.

use std::path::{Path, PathBuf};
use std::process::Command;

/// Marks a directory as one this app created and may write git history
/// into. Its absence is a refusal, not a prompt.
pub const MARKER: &str = ".studio-project";

/// Where projects live by default: `$STUDIO_PROJECTS`, else
/// `$XDG_DATA_HOME/studio/projects`, else `~/.local/share/studio/projects`
/// — the freedesktop location Zrythm uses for the same reason, and never
/// the current working directory, which is how the source-tree accident
/// happened.
pub fn projects_root() -> PathBuf {
    if let Ok(p) = std::env::var("STUDIO_PROJECTS") {
        if !p.trim().is_empty() { return PathBuf::from(p); }
    }
    if let Ok(x) = std::env::var("XDG_DATA_HOME") {
        if !x.trim().is_empty() { return PathBuf::from(x).join("studio/projects"); }
    }
    match std::env::var("HOME") {
        Ok(h) if !h.trim().is_empty() =>
            PathBuf::from(h).join(".local/share/studio/projects"),
        // No HOME (a service, a container): a visible subdirectory of the
        // cwd, still never the cwd itself.
        _ => PathBuf::from("studio-projects"),
    }
}

/// Where the sample library is: `$STUDIO_LIBRARY`; else `./library` IF
/// it exists (the documented cwd convention, kept for everyone who has
/// one); else `$XDG_DATA_HOME/studio/library`, else
/// `~/.local/share/studio/library`. The freedesktop location, so a
/// packaged build launched from a menu (cwd `/`, or `$HOME`) finds the
/// same samples a terminal launch does instead of an empty sidebar
/// telling you to set a variable — `projects_root`'s rule, one root over.
/// The resolver is pure so the fallback order can be pinned without
/// touching the process environment from a test.
pub fn library_root() -> PathBuf {
    let cwd_has_library = std::path::Path::new("library").is_dir();
    library_root_from(std::env::var("STUDIO_LIBRARY").ok(), cwd_has_library,
                      std::env::var("XDG_DATA_HOME").ok(), std::env::var("HOME").ok())
}

pub fn library_root_from(explicit: Option<String>, cwd_has_library: bool,
                         xdg_data_home: Option<String>, home: Option<String>) -> PathBuf {
    if let Some(p) = explicit {
        if !p.trim().is_empty() { return PathBuf::from(p); }
    }
    if cwd_has_library { return PathBuf::from("library"); }
    if let Some(x) = xdg_data_home {
        if !x.trim().is_empty() { return PathBuf::from(x).join("studio/library"); }
    }
    match home {
        Some(h) if !h.trim().is_empty() => PathBuf::from(h).join(".local/share/studio/library"),
        _ => PathBuf::from("library"),
    }
}

/// A file-system-safe directory name for a project title.
pub fn slug(name: &str) -> String {
    let s: String = name.chars().map(|c| {
        if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }
    }).collect();
    let s = s.trim_matches('-').to_string();
    if s.is_empty() { "untitled".into() } else { s.chars().take(64).collect() }
}

/// The directory a project of this name belongs in, under `root` (usually
/// [`projects_root`], or wherever the last archive was written — a person
/// who archives to an SD card wants their working copy on the SD card).
pub fn project_dir(root: &Path, name: &str) -> PathBuf {
    root.join(slug(name))
}

/// Why a directory may not be versioned.
#[derive(Debug, Clone, PartialEq)]
pub enum Refusal {
    /// The directory, or an ancestor, is a git work tree this app did not
    /// create. Never touched: someone's source checkout is not a project.
    ForeignRepo(PathBuf),
    /// Not marked as a project directory and not empty — refusing rather
    /// than adopting whatever happens to be there.
    NotOurs(PathBuf),
}

impl std::fmt::Display for Refusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Refusal::ForeignRepo(p) => write!(f,
                "{} is inside a git repository this app did not create — \
                 refusing to commit there. Save into a project directory \
                 (under {}) instead.", p.display(), projects_root().display()),
            Refusal::NotOurs(p) => write!(f,
                "{} is not an empty or studio-owned directory — refusing to \
                 put project history in it.", p.display()),
        }
    }
}

/// May this directory hold project history? Marked-ours passes. Otherwise
/// it must be empty (or absent) AND have no git work tree above it.
///
/// The ancestor walk is the important half: `~/code/c9/apps/studio` has no
/// `.git` of its own, and committing there is exactly what damaged a real
/// repository.
pub fn guard(dir: &Path) -> Result<(), Refusal> {
    if dir.join(MARKER).is_file() { return Ok(()); }
    let mut at = Some(dir.to_path_buf());
    while let Some(p) = at {
        if p.join(".git").exists() && !p.join(MARKER).is_file() {
            return Err(Refusal::ForeignRepo(p));
        }
        at = p.parent().map(|x| x.to_path_buf());
    }
    // Adoptable when everything in it is something a project consists of.
    // "Empty" alone would refuse a real project folder saved by an older
    // build; "anything goes" would adopt a source tree. The known-set test
    // is the line between them: one stray `src/` and this refuses.
    match std::fs::read_dir(dir) {
        Err(_) => Ok(()), // does not exist yet: ours to create
        Ok(rd) => {
            for e in rd.flatten() {
                let name = e.file_name().to_string_lossy().into_owned();
                if !is_project_file(&name) {
                    return Err(Refusal::NotOurs(dir.to_path_buf()));
                }
            }
            Ok(())
        }
    }
}

/// The complete inventory of what a project directory may contain. Kept
/// as one list because [`guard`] and the commit pathspec must agree: a
/// file worth committing is a file worth adopting, and nothing else is
/// either.
pub fn is_project_file(name: &str) -> bool {
    const EXACT: [&str; 7] = [MARKER, "meta.toml", ".gitattributes", ".gitignore",
                              "media.list", "media", ".git"];
    EXACT.contains(&name)
        || name.ends_with(".studio")
        || name.ends_with(".studioarch")
        || name.ends_with(".workspace")
}

/// Create (or adopt) a project directory: makes it, marks it ours.
pub fn ensure_project_dir(dir: &Path) -> Result<(), String> {
    guard(dir).map_err(|r| r.to_string())?;
    std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
    if !dir.join(MARKER).is_file() {
        std::fs::write(dir.join(MARKER),
            "# a studio project directory; history and media live here\nformat = 1\n")
            .map_err(|e| format!("marker: {e}"))?;
    }
    Ok(())
}

/// Project metadata, written as `meta.toml` beside the project file — the
/// human-readable card catalogue entry git history makes greppable.
#[derive(Debug, Clone, PartialEq)]
pub struct Meta {
    pub name: String,
    pub bpm: f32,
    pub tracks: usize,
    pub clips: usize,
    pub busses: usize,
    pub patterns: usize,
    pub channels: usize,
}

impl Meta {
    /// Hand-rolled TOML: six scalar fields do not earn a dependency, and a
    /// writer this small cannot drift from a parser it does not have.
    pub fn to_toml(&self) -> String {
        let name = self.name.replace('\\', "\\\\").replace('"', "\\\"");
        format!(
            "# written by studio on save; informational, not load-bearing\n\
             generator = \"studio\"\n\
             name = \"{name}\"\n\
             bpm = {:.2}\n\
             tracks = {}\n\
             clips = {}\n\
             busses = {}\n\
             patterns = {}\n\
             channels = {}\n",
            self.bpm, self.tracks, self.clips, self.busses, self.patterns, self.channels)
    }
}

/// Where the media bytes live relative to the history.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MediaMode {
    /// Media is committed. Right for audio projects: compressed originals,
    /// stored once each under `-delta`, and a clone is the whole project.
    Committed,
    /// `media/` is gitignored; a committed `media.list` manifest (name and
    /// byte length per file) says what each version needs. Right when the
    /// media is video: the bytes belong in a content-addressed store
    /// (gitmind's, or any other) that this manifest lets hydrate the folder.
    External,
}

/// One entry of `git log`.
#[derive(Debug, Clone, PartialEq)]
pub struct Commit {
    pub id: String,
    pub unix_time: i64,
    pub message: String,
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = Command::new("git").arg("-C").arg(dir).args(args).output()
        .map_err(|e| format!("git not runnable: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).into_owned())
    }
}

/// Is a git binary present at all? The one question the caller needs for
/// its status line — everything past it is this module's problem.
pub fn git_available() -> bool {
    Command::new("git").arg("--version").output()
        .map(|o| o.status.success()).unwrap_or(false)
}

const GITATTRIBUTES: &str = "\
# compressed media: store whole, never delta — the deltas do not exist\n\
*.opus -delta\n*.ogg -delta\n*.flac -delta\n*.mp3 -delta\n*.m4a -delta\n\
*.wav -delta\n*.mp4 -delta\n*.mkv -delta\n*.webm -delta\n*.studio -delta\n";

/// Version whatever is in `dir` right now: write `meta.toml`, ensure the
/// repo and its attribute/ignore policy exist, and commit. `Ok(Some(id))`
/// is a new commit, `Ok(None)` is "nothing changed since last save" — a
/// fact, not a failure. `Err` means history was skipped; the SAVE already
/// happened and must be reported as such by the caller.
pub fn save_versioned(dir: &Path, meta: &Meta, mode: MediaMode, message: &str)
                      -> Result<Option<String>, String> {
    // The containment rule, enforced before anything is written: this
    // function will not create or extend git history in a directory that
    // is not a studio project.
    ensure_project_dir(dir)?;
    std::fs::write(dir.join("meta.toml"), meta.to_toml())
        .map_err(|e| format!("meta.toml: {e}"))?;
    if !dir.join(".gitattributes").exists() {
        std::fs::write(dir.join(".gitattributes"), GITATTRIBUTES)
            .map_err(|e| format!(".gitattributes: {e}"))?;
    }
    match mode {
        MediaMode::Committed => {}
        MediaMode::External => {
            // The ignore and the manifest travel together: history must
            // always be able to say what media a version needs, precisely
            // because the bytes are not in it.
            std::fs::write(dir.join(".gitignore"), "media/\n")
                .map_err(|e| format!(".gitignore: {e}"))?;
            let mut list = String::from("# media the project references; bytes live externally\n");
            if let Ok(entries) = std::fs::read_dir(dir.join("media")) {
                let mut rows: Vec<(String, u64)> = entries.flatten()
                    .filter_map(|e| {
                        let len = e.metadata().ok()?.len();
                        Some((e.file_name().to_string_lossy().into_owned(), len))
                    }).collect();
                rows.sort();
                for (name, len) in rows {
                    list.push_str(&format!("{name}\t{len}\n"));
                }
            }
            std::fs::write(dir.join("media.list"), list)
                .map_err(|e| format!("media.list: {e}"))?;
        }
    }
    if !dir.join(".git").exists() {
        git(dir, &["init", "-q"])?;
    }
    // EXPLICIT pathspecs, never `-A`: a project is a known handful of
    // files, and the wildcard is what turned a save into a commit of
    // someone's source tree. Missing paths are skipped rather than
    // failing the save.
    let mut spec: Vec<&str> = vec![MARKER, "meta.toml", ".gitattributes"];
    for extra in ["project.studio", ".gitignore", "media.list"] {
        if dir.join(extra).exists() { spec.push(extra); }
    }
    // `media` only in Committed mode — naming an ignored path explicitly
    // is an ERROR to git, not a silent skip, so External mode must not
    // ask for the directory it just told git to ignore.
    if mode == MediaMode::Committed && dir.join("media").exists() {
        spec.push("media");
    }
    let mut args = vec!["add", "--"];
    args.extend(spec);
    git(dir, &args)?;
    // Identity inline: a save must work on a machine that never ran
    // `git config`, and a project's history is the app's, not the user's
    // global persona.
    match git(dir, &["-c", "user.name=studio", "-c", "user.email=studio@localhost",
                     "commit", "-q", "-m", message]) {
        Ok(_) => {}
        Err(e) => {
            // "nothing to commit" arrives as a nonzero exit with the
            // explanation on stdout, which our Err doesn't carry — ask the
            // repo directly instead of parsing prose.
            let clean = git(dir, &["status", "--porcelain"]).map(|s| s.trim().is_empty());
            if clean == Ok(true) { return Ok(None); }
            return Err(e);
        }
    }
    Ok(Some(git(dir, &["rev-parse", "--short", "HEAD"])?.trim().to_string()))
}

/// The newest `limit` commits, newest first.
pub fn history(dir: &Path, limit: usize) -> Result<Vec<Commit>, String> {
    let out = git(dir, &["log", &format!("-{limit}"), "--format=%h%x09%ct%x09%s"])?;
    Ok(out.lines().filter_map(|l| {
        let mut it = l.splitn(3, '\t');
        Some(Commit {
            id: it.next()?.to_string(),
            unix_time: it.next()?.parse().ok()?,
            message: it.next()?.to_string(),
        })
    }).collect())
}

/// The bytes of one file as it stood at `commit` — "the mix from Tuesday",
/// read without touching the working tree.
pub fn file_at(dir: &Path, commit: &str, file: &str) -> Result<Vec<u8>, String> {
    let out = Command::new("git").arg("-C").arg(dir)
        .args(["show", &format!("{commit}:{file}")]).output()
        .map_err(|e| format!("git not runnable: {e}"))?;
    if out.status.success() { Ok(out.stdout) }
    else { Err(String::from_utf8_lossy(&out.stderr).into_owned()) }
}

fn _paths_are_send() { fn f<T: Send>() {} f::<PathBuf>(); }

#[cfg(test)]
mod library_root_tests {
    use super::*;

    #[test]
    fn the_library_falls_back_in_the_documented_order() {
        let f = |e: Option<&str>, cwd: bool, x: Option<&str>, h: Option<&str>|
            library_root_from(e.map(String::from), cwd, x.map(String::from), h.map(String::from));
        assert_eq!(f(Some("/mnt/packs"), true, Some("/x"), Some("/h")), PathBuf::from("/mnt/packs"),
                   "an explicit root wins over everything");
        assert_eq!(f(Some("  "), true, Some("/x"), Some("/h")), PathBuf::from("library"),
                   "a blank variable is unset; a ./library that exists is honoured");
        assert_eq!(f(None, false, Some("/x"), Some("/h")), PathBuf::from("/x/studio/library"),
                   "no ./library: the freedesktop data dir");
        assert_eq!(f(None, false, None, Some("/h")), PathBuf::from("/h/.local/share/studio/library"),
                   "no XDG_DATA_HOME: its default under HOME");
        assert_eq!(f(None, false, None, None), PathBuf::from("library"),
                   "no HOME at all (a service): the old cwd default, so nothing breaks");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn meta() -> Meta {
        Meta { name: "cut \"one\"".into(), bpm: 140.0, tracks: 3, clips: 6,
               busses: 1, patterns: 2, channels: 8 }
    }
    fn tempdir(tag: &str) -> PathBuf {
        let d = std::env::temp_dir().join(format!("studio-pdir-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        d
    }

    #[test]
    fn every_save_is_a_commit_and_an_unchanged_save_is_a_fact_not_an_error() {
        let dir = tempdir("commits");
        std::fs::write(dir.join("project.studio"), b"v1").unwrap();
        let first = save_versioned(&dir, &meta(), MediaMode::Committed, "first save")
            .expect("history").expect("a commit");
        // Same bytes again: nothing changed, and the save must say so as a
        // value rather than failing the whole flow over git's exit code.
        assert_eq!(save_versioned(&dir, &meta(), MediaMode::Committed, "again").unwrap(), None);
        std::fs::write(dir.join("project.studio"), b"v2").unwrap();
        let second = save_versioned(&dir, &meta(), MediaMode::Committed, "second save")
            .unwrap().expect("a commit");
        assert_ne!(first, second);
        let log = history(&dir, 10).unwrap();
        assert_eq!(log.len(), 2, "newest first, no phantom commit from the no-op save");
        assert_eq!(log[0].message, "second save");
        assert_eq!(log[1].message, "first save");
        assert!(log[0].unix_time >= log[1].unix_time);
        // Tuesday's mix is readable without touching the working tree.
        assert_eq!(file_at(&dir, &first, "project.studio").unwrap(), b"v1");
        assert_eq!(file_at(&dir, &second, "project.studio").unwrap(), b"v2");
    }

    #[test]
    fn a_save_never_commits_someone_elses_repository() {
        // THE bug: launched from a source checkout, the old version ran
        // `git init` + `git add -A` in the user's own tree and committed
        // 2,734 files of their code with the message "save". Both shapes
        // of that accident are refused now — the directory itself being a
        // repo, and (the one that actually happened) an ANCESTOR being
        // one, several levels up.
        let root = tempdir("foreign");
        std::fs::create_dir_all(root.join(".git")).unwrap();
        std::fs::write(root.join("Cargo.toml"), b"[workspace]").unwrap();
        assert!(matches!(guard(&root), Err(Refusal::ForeignRepo(_))));
        let deep = root.join("apps/studio");
        std::fs::create_dir_all(&deep).unwrap();
        assert!(matches!(guard(&deep), Err(Refusal::ForeignRepo(_))),
                "an ancestor repository is the case that did the damage");
        // And the refusal is total: no repo created, no commit made.
        let before = std::fs::read_dir(&deep).unwrap().count();
        let err = save_versioned(&deep, &meta(), MediaMode::Committed, "save")
            .expect_err("must refuse");
        assert!(err.contains("did not create"), "{err}");
        assert_eq!(std::fs::read_dir(&deep).unwrap().count(), before,
                   "nothing was written into the foreign tree");
        assert!(!deep.join(".git").exists());
    }

    #[test]
    fn a_directory_of_stray_files_is_refused_but_a_real_project_folder_is_adopted() {
        // Adoptable: everything present is something a project consists of
        // (an older build's save, say).
        let ok = tempdir("adopt");
        std::fs::write(ok.join("cut.studio"), b"x").unwrap();
        std::fs::write(ok.join("cut.workspace"), b"y").unwrap();
        assert!(guard(&ok).is_ok());
        // Not adoptable: one thing that is not ours, and this stays out.
        let stray = tempdir("stray");
        std::fs::write(stray.join("cut.studio"), b"x").unwrap();
        std::fs::create_dir_all(stray.join("src")).unwrap();
        assert!(matches!(guard(&stray), Err(Refusal::NotOurs(_))));
        // Marked ours: adopted regardless of what accumulated inside.
        let ours = tempdir("ours");
        std::fs::write(ours.join(MARKER), b"format = 1").unwrap();
        std::fs::create_dir_all(ours.join("whatever")).unwrap();
        assert!(guard(&ours).is_ok());
    }

    #[test]
    fn a_commit_stages_project_files_and_nothing_else() {
        // Even inside a project directory, the commit names its paths:
        // no `-A` means a stray file cannot ride along.
        let dir = tempdir("pathspec");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("project.studio"), b"v1").unwrap();
        save_versioned(&dir, &meta(), MediaMode::Committed, "save").unwrap();
        std::fs::write(dir.join("scratch.txt"), b"not mine").unwrap();
        std::fs::write(dir.join("project.studio"), b"v2").unwrap();
        save_versioned(&dir, &meta(), MediaMode::Committed, "save 2").unwrap();
        let tracked = git(&dir, &["ls-files"]).unwrap();
        assert!(tracked.contains("project.studio"));
        assert!(tracked.contains(MARKER), "the marker travels with the history");
        assert!(!tracked.contains("scratch.txt"),
                "a stray file is never staged: {tracked}");
    }

    #[test]
    fn projects_live_under_a_root_never_in_the_working_directory() {
        // The other half of the fix: a project has its own directory, and
        // the default root is the freedesktop location — the same place
        // Zrythm puts its Projects folder, and never `.`.
        let root = projects_root();
        assert!(root.is_absolute() || root.starts_with("studio-projects"),
                "{}", root.display());
        assert_ne!(root, PathBuf::from("."));
        assert_eq!(project_dir(Path::new("/data"), "My Cut #2"),
                   PathBuf::from("/data/My-Cut--2"));
        assert_eq!(slug("///"), "untitled", "an unnameable project still gets a home");
    }

    #[test]
    fn the_metadata_and_the_delta_policy_land_beside_the_project() {
        let dir = tempdir("meta");
        std::fs::write(dir.join("project.studio"), b"x").unwrap();
        save_versioned(&dir, &meta(), MediaMode::Committed, "save").unwrap();
        let toml = std::fs::read_to_string(dir.join("meta.toml")).unwrap();
        assert!(toml.contains("name = \"cut \\\"one\\\"\""), "quotes escaped: {toml}");
        assert!(toml.contains("bpm = 140.00"));
        assert!(toml.contains("patterns = 2"));
        let attrs = std::fs::read_to_string(dir.join(".gitattributes")).unwrap();
        assert!(attrs.contains("*.opus -delta"), "compressed media is never delta'd");
    }

    #[test]
    fn external_media_is_ignored_but_its_manifest_is_history() {
        let dir = tempdir("external");
        std::fs::write(dir.join("project.studio"), b"x").unwrap();
        std::fs::create_dir_all(dir.join("media")).unwrap();
        std::fs::write(dir.join("media/kick.opus"), vec![0u8; 1234]).unwrap();
        save_versioned(&dir, &meta(), MediaMode::External, "save").unwrap();
        let tracked = git(&dir, &["ls-files"]).unwrap();
        assert!(!tracked.contains("media/kick.opus"),
                "the bytes live in the external store, not in git: {tracked}");
        assert!(tracked.contains("media.list"), "but every version says what it needs");
        let list = std::fs::read_to_string(dir.join("media.list")).unwrap();
        assert!(list.contains("kick.opus\t1234"), "{list}");
    }
}

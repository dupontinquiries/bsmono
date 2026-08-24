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
//! ## Degradation is a message, never a lost save
//!
//! The save itself is the caller's write of `project.studio`; this module
//! only versions what is on disk. No git binary → the save still happened,
//! the status line says history was skipped. Nothing to commit → that is a
//! fact (`Ok(None)`), not an error.

use std::path::{Path, PathBuf};
use std::process::Command;

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
    std::fs::create_dir_all(dir).map_err(|e| format!("create {dir:?}: {e}"))?;
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
    git(dir, &["add", "-A"])?;
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

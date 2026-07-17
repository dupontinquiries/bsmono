//! gitmind_session — let logscope work on a `.gitmind` project safely: **load** it (mirror to an
//! isolated `/tmp/<session>` copy), let the session make its changes there, then **reapply** those
//! changes back over the real project as a single new commit. The `/tmp` copy means a session can
//! never corrupt the project mid-edit; the reapply makes its work land as one reviewable commit.
//!
//! Reuses the fleet's own pieces: `sandbox::copy_tree` (the tested mirror that skips `.git`/`target`)
//! and plain `git` for the commit (the same porcelain gitmind's GitCli drives).
use crate::mount::{self, Workspace};
use std::path::{Path, PathBuf};

/// A `.gitmind` project is marked by `.gitmind/config.toml` carrying a `[gitmind]` section.
pub fn is_gitmind_project(project: &Path) -> bool {
    std::fs::read_to_string(project.join(".gitmind").join("config.toml"))
        .map(|s| s.contains("[gitmind]"))
        .unwrap_or(false)
}

/// A loaded work session: the original project, the session name, and the isolated `/tmp` scratch
/// (a `mount::Workspace` — mirrored with a fresh git baseline, so a session can commit/diff there).
#[derive(Debug, Clone)]
pub struct Session {
    pub project: PathBuf,
    pub name: String,
    pub ws: Workspace,
}
impl Session {
    /// The scratch directory the session edits.
    pub fn work_dir(&self) -> &Path {
        &self.ws.dir
    }
}

/// Filesystem-safe form of a session name for a directory.
pub fn sanitize(name: &str) -> String {
    let s: String = name.chars().map(|c| if c.is_alphanumeric() || c == '-' || c == '_' { c } else { '-' }).collect();
    let t = s.trim_matches('-');
    if t.is_empty() { "session".into() } else { t.to_string() }
}

/// The commit message a reapplied session lands under.
pub fn commit_message(session: &str) -> String {
    format!("logscope session: {session}")
}

/// Load: verify it's a `.gitmind` project, then isolate it into `tmp_root/<session>/…` via
/// `mount::isolate` (the fleet's tested mirror-with-git-baseline — no second implementation).
pub fn load_gitmind(project: &Path, session: &str, tmp_root: &Path) -> Result<Session, String> {
    if !is_gitmind_project(project) {
        return Err(format!("{} is not a .gitmind project (no .gitmind/config.toml)", project.display()));
    }
    let m = mount::detect(project).map_err(|e| e.to_string())?;
    let scratch_root = tmp_root.join(sanitize(session));
    std::fs::create_dir_all(&scratch_root).map_err(|e| e.to_string())?;
    let ws = mount::isolate(&m, &scratch_root).map_err(|e| e.to_string())?;
    Ok(Session { project: project.to_path_buf(), name: session.to_string(), ws })
}

/// Relative paths of files that were added or modified in the work copy vs the project. (Deletions
/// aren't reapplied — a session adds/edits; removing files stays an explicit, separate action.)
pub fn changed_files(work_dir: &Path, project: &Path) -> Vec<String> {
    let mut out = Vec::new();
    fn walk(dir: &Path, work_root: &Path, project: &Path, out: &mut Vec<String>) {
        let Ok(rd) = std::fs::read_dir(dir) else { return };
        for e in rd.flatten() {
            let p = e.path();
            let name = e.file_name().to_string_lossy().into_owned();
            if name == ".git" || name == "target" {
                continue;
            }
            if p.is_dir() {
                walk(&p, work_root, project, out);
            } else if let Ok(rel) = p.strip_prefix(work_root) {
                let rels = rel.to_string_lossy().replace('\\', "/");
                let orig = project.join(&rels);
                let differs = match (std::fs::read(&p), std::fs::read(&orig)) {
                    (Ok(a), Ok(b)) => a != b,
                    (Ok(_), Err(_)) => true, // new file
                    _ => false,
                };
                if differs {
                    out.push(rels);
                }
            }
        }
    }
    walk(work_dir, work_dir, project, &mut out);
    out.sort();
    out
}

/// Reapply the session: copy its changed files back over the project, then `git add -A` + commit so
/// the work lands as one new commit. Returns the new commit hash.
pub fn reapply(session: &Session) -> Result<String, String> {
    let changed = changed_files(session.work_dir(), &session.project);
    if changed.is_empty() {
        return Err("no changes to reapply".into());
    }
    for rel in &changed {
        let src = session.work_dir().join(rel);
        let dst = session.project.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&src, &dst).map_err(|e| format!("copy {rel}: {e}"))?;
    }
    git(&session.project, &["add", "-A"])?;
    git(&session.project, &["commit", "-m", &commit_message(&session.name)])?;
    git(&session.project, &["rev-parse", "HEAD"])
}

/// Reapply changed files from an arbitrary `work_dir` (e.g. a fan branch's isolated workspace) over
/// `project` as one new commit. Same mechanism as `reapply` but for a raw directory rather than a Session —
/// used by `:pick` to land the keeper branch of an agent fan.
pub fn reapply_dir(work_dir: &Path, project: &Path, label: &str) -> Result<String, String> {
    let changed = changed_files(work_dir, project);
    if changed.is_empty() {
        return Err("no changes to reapply".into());
    }
    for rel in &changed {
        let src = work_dir.join(rel);
        let dst = project.join(rel);
        if let Some(parent) = dst.parent() {
            std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
        }
        std::fs::copy(&src, &dst).map_err(|e| format!("copy {rel}: {e}"))?;
    }
    git(project, &["add", "-A"])?;
    git(project, &["commit", "-m", &commit_message(label)])?;
    git(project, &["rev-parse", "HEAD"])
}

fn git(dir: &Path, args: &[&str]) -> Result<String, String> {
    let out = std::process::Command::new("git")
        .args(args)
        .current_dir(dir)
        .output()
        .map_err(|e| e.to_string())?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(p: &Path, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn detects_project_and_sanitizes_and_messages() {
        let tmp = std::env::temp_dir().join(format!("gm-detect-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&tmp);
        write(&tmp.join(".gitmind/config.toml"), "[gitmind]\nversion = 1\n");
        assert!(is_gitmind_project(&tmp));
        assert!(!is_gitmind_project(&tmp.join("nope")));
        assert_eq!(sanitize("fix/voice bug!"), "fix-voice-bug");
        assert_eq!(commit_message("s1"), "logscope session: s1");
        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn changed_files_finds_edits_and_new_files_only() {
        let base = std::env::temp_dir().join(format!("gm-diff-{}", std::process::id()));
        let (proj, work) = (base.join("proj"), base.join("work"));
        let _ = std::fs::remove_dir_all(&base);
        write(&proj.join("a.txt"), "hello");
        write(&proj.join("keep.txt"), "same");
        write(&work.join("a.txt"), "hello world"); // edited
        write(&work.join("keep.txt"), "same"); // unchanged
        write(&work.join("new.txt"), "brand new"); // added
        let changed = changed_files(&work, &proj);
        assert_eq!(changed, vec!["a.txt".to_string(), "new.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    // End-to-end: a real git repo, load → edit in /tmp → reapply → the edit is a NEW commit.
    #[test]
    fn load_edit_reapply_lands_a_new_commit() {
        let base = std::env::temp_dir().join(format!("gm-e2e-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let proj = base.join("myproj");
        write(&proj.join(".gitmind/config.toml"), "[gitmind]\nversion = 1\n");
        write(&proj.join("src/lib.rs"), "fn main() {}\n");
        for args in [
            vec!["init", "-q"],
            vec!["config", "user.email", "t@t"],
            vec!["config", "user.name", "t"],
            vec!["add", "-A"],
            vec!["commit", "-q", "-m", "initial"],
        ] {
            git(&proj, &args).unwrap();
        }
        let before = git(&proj, &["rev-list", "--count", "HEAD"]).unwrap();

        let tmp_root = base.join("tmp");
        let session = load_gitmind(&proj, "add-greeting", &tmp_root).unwrap();
        assert!(session.work_dir().starts_with(tmp_root.join("add-greeting")), "scratch under the session name");
        // work happens in /tmp, never touching the project
        write(&session.work_dir().join("src/lib.rs"), "fn main() { println!(\"hi\"); }\n");
        assert_eq!(std::fs::read_to_string(proj.join("src/lib.rs")).unwrap(), "fn main() {}\n", "project untouched pre-reapply");

        let hash = reapply(&session).unwrap();
        assert_eq!(hash.len(), 40, "got a commit hash");
        let after = git(&proj, &["rev-list", "--count", "HEAD"]).unwrap();
        assert_eq!(after.parse::<u32>().unwrap(), before.parse::<u32>().unwrap() + 1, "exactly one new commit");
        assert!(git(&proj, &["log", "-1", "--pretty=%s"]).unwrap().contains("logscope session: add-greeting"));
        assert!(std::fs::read_to_string(proj.join("src/lib.rs")).unwrap().contains("println!"), "edit landed in the project");
        let _ = std::fs::remove_dir_all(&base);
    }
}

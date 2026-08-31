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

/// Turn a plain directory (a Cargo root, say) into a `.gitmind` project this module can load: write the
/// minimal `[gitmind]` marker config and `git init` if no repo exists (load/reapply need a baseline to
/// diff/commit against). ADDITIVE ONLY — refuses to touch an existing `.gitmind/config.toml`, and an
/// existing `.git` is left exactly as-is. This is the Cargo tab's "make this repo workable" button.
pub fn init_gitmind(project: &Path, name: &str) -> Result<(), String> {
    let cfg = project.join(".gitmind").join("config.toml");
    if cfg.exists() {
        return Err(format!("{} already exists — this is already a gitmind project", cfg.display()));
    }
    if !project.is_dir() {
        return Err(format!("{} is not a directory", project.display()));
    }
    std::fs::create_dir_all(cfg.parent().unwrap_or(project)).map_err(|e| format!("mkdir .gitmind: {e}"))?;
    let name = if name.trim().is_empty() { "project" } else { name.trim() };
    std::fs::write(&cfg, format!("[gitmind]\nname = \"{name}\"\ncreated_by = \"logscope cargo tab\"\n"))
        .map_err(|e| format!("write config.toml: {e}"))?;
    if !project.join(".git").exists() {
        let ok = std::process::Command::new("git").args(["init", "-q"]).current_dir(project)
            .status().map(|s| s.success()).unwrap_or(false);
        if !ok {
            return Err("config.toml written, but `git init` failed — init a repo manually, then :load-gitmind".into());
        }
    }
    Ok(())
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

/// Do two files differ, without holding either one in memory.
///
/// The previous form was `std::fs::read(a) != std::fs::read(b)` — both files allocated in full, for
/// every file in the tree, to answer a yes/no. That is the same failure that took the backend out
/// through the agent's read tools, in the path that decides what gets committed to the REAL project:
/// one large file anywhere in a `.gitmind` workspace and `:reapply` dies instead of landing the
/// session's work.
///
/// Length first (a mismatch is the common case and costs a stat), then a streamed chunk compare that
/// stops at the first difference. Peak memory is two buffers regardless of file size, and an
/// unreadable file is reported as "differs" so it is offered for review rather than silently
/// dropped from the reapply.
fn files_differ(a: &Path, b: &Path) -> bool {
    let (Ok(ma), Ok(mb)) = (std::fs::metadata(a), std::fs::metadata(b)) else { return true };
    if ma.len() != mb.len() { return true; }
    let (Ok(fa), Ok(fb)) = (std::fs::File::open(a), std::fs::File::open(b)) else { return true };
    let (mut ra, mut rb) = (std::io::BufReader::new(fa), std::io::BufReader::new(fb));
    let (mut ba, mut bb) = ([0u8; 32 * 1024], [0u8; 32 * 1024]);
    loop {
        let na = match read_full(&mut ra, &mut ba) { Ok(n) => n, Err(_) => return true };
        let nb = match read_full(&mut rb, &mut bb) { Ok(n) => n, Err(_) => return true };
        if na != nb { return true; }
        if na == 0 { return false; }
        if ba[..na] != bb[..nb] { return true; }
    }
}

/// Fill `buf` as far as the reader allows. `Read::read` may return a short count without being at
/// EOF, and comparing short reads position-by-position would report two identical files as
/// differing the moment the two readers chunked differently.
fn read_full(r: &mut impl std::io::Read, buf: &mut [u8]) -> std::io::Result<usize> {
    let mut n = 0;
    while n < buf.len() {
        match r.read(&mut buf[n..]) {
            Ok(0) => break,
            Ok(k) => n += k,
            Err(ref e) if e.kind() == std::io::ErrorKind::Interrupted => {}
            Err(e) => return Err(e),
        }
    }
    Ok(n)
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
            // symlink_metadata, not is_dir: a symlink to an ancestor makes this walk descend
            // forever, and a symlink to a device file makes the compare below read from it.
            let Ok(md) = std::fs::symlink_metadata(&p) else { continue };
            if md.file_type().is_symlink() { continue; }
            if md.is_dir() {
                walk(&p, work_root, project, out);
            } else if let Ok(rel) = p.strip_prefix(work_root) {
                let rels = rel.to_string_lossy().replace('\\', "/");
                let orig = project.join(&rels);
                let differs = if orig.exists() { files_differ(&p, &orig) } else { true };
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

/// Write a set of `(relative_path, text)` files into `project` and commit them as one commit. Used by the
/// doc merge to land the unioned doc tree + generated index. Returns the new commit hash.
pub fn write_and_commit(project: &Path, files: &[(String, String)], label: &str) -> Result<String, String> {
    if files.is_empty() { return Err("nothing to write".into()); }
    for (rel, text) in files {
        let dst = project.join(rel);
        if let Some(parent) = dst.parent() { std::fs::create_dir_all(parent).map_err(|e| e.to_string())?; }
        std::fs::write(&dst, text).map_err(|e| format!("write {rel}: {e}"))?;
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

    #[test]
    fn init_gitmind_marks_a_plain_dir_and_refuses_twice() {
        let d = std::env::temp_dir().join(format!("gm-init-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(d.join(".git")).unwrap(); // pre-existing repo → init never shells out to git
        assert!(!is_gitmind_project(&d));
        init_gitmind(&d, "demo").expect("first init succeeds");
        assert!(is_gitmind_project(&d), "the marker config makes it loadable");
        let again = init_gitmind(&d, "demo");
        assert!(again.is_err() && again.unwrap_err().contains("already"), "additive: never clobbers");
        let _ = std::fs::remove_dir_all(&d);
    }

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


    #[cfg(target_os = "linux")]
    fn peak_rss_kib() -> u64 {
        std::fs::read_to_string("/proc/self/status").ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("VmHWM:")).map(str::to_string))
            .and_then(|l| l.split_whitespace().nth(1).and_then(|n| n.parse().ok()))
            .unwrap_or(0)
    }

    fn big(path: &Path, mib: usize, tail: &str) {
        use std::io::Write;
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        let mut f = std::io::BufWriter::new(std::fs::File::create(path).unwrap());
        let line = "// sixty-four bytes of perfectly ordinary source text right here\n";
        for _ in 0..(mib * (1024 * 1024) / line.len()) { f.write_all(line.as_bytes()).unwrap(); }
        f.write_all(tail.as_bytes()).unwrap();
        f.flush().unwrap();
    }

    /// `:reapply` compares every file in the workspace against the project to decide what to commit.
    /// It used to do that with `fs::read(a) != fs::read(b)` — both files allocated in full — in the
    /// path that writes to the user's REAL project. One large file and the reapply dies instead of
    /// landing the session's work.
    #[cfg(target_os = "linux")]
    #[test]
    fn comparing_large_files_does_not_allocate_them() {
        let base = std::env::temp_dir().join(format!("gm-cmp-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (proj, work) = (base.join("proj"), base.join("work"));
        // Same size, differing only at the very end — the worst case: length cannot reject it, and
        // the streamed compare has to read all the way through.
        big(&proj.join("src/huge.rs"), 128, "// original tail\n");
        big(&work.join("src/huge.rs"), 128, "// modified tail\n");
        let before = peak_rss_kib();
        let changed = changed_files(&work, &proj);
        let after = peak_rss_kib();
        assert_eq!(changed, vec!["src/huge.rs".to_string()], "the difference is still found");
        let grew_mib = after.saturating_sub(before) / 1024;
        assert!(grew_mib < 16, "comparing two 128 MiB files grew peak RSS by {grew_mib} MiB");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn identical_large_files_are_not_reported_as_changed() {
        let base = std::env::temp_dir().join(format!("gm-same-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (proj, work) = (base.join("proj"), base.join("work"));
        // Big enough to span many chunk boundaries: a short read handled wrong reports these as
        // differing, which would make reapply commit every unchanged file in the tree.
        big(&proj.join("src/same.rs"), 4, "// tail\n");
        big(&work.join("src/same.rs"), 4, "// tail\n");
        assert!(changed_files(&work, &proj).is_empty(), "identical files must not be reported changed");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_length_difference_is_caught_without_reading_content() {
        let base = std::env::temp_dir().join(format!("gm-len-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (proj, work) = (base.join("proj"), base.join("work"));
        write(&proj.join("a.txt"), "same length!!");
        write(&work.join("a.txt"), "same length!!x");
        assert_eq!(changed_files(&work, &proj), vec!["a.txt".to_string()]);
        let _ = std::fs::remove_dir_all(&base);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn a_symlink_does_not_send_the_walk_into_a_loop() {
        let base = std::env::temp_dir().join(format!("gm-sym-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let (proj, work) = (base.join("proj"), base.join("work"));
        write(&work.join("real.txt"), "content");
        std::fs::create_dir_all(&proj).unwrap();
        std::os::unix::fs::symlink(&work, work.join("loop")).unwrap();
        assert_eq!(changed_files(&work, &proj), vec!["real.txt".to_string()],
                   "the walk terminates and still finds the real file");
        let _ = std::fs::remove_dir_all(&base);
    }
}

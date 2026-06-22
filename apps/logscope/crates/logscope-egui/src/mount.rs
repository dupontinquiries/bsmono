//! mount.rs — safe system integration. "Mount" a real project (a directory, or a single file) into an
//! isolated scratch workspace under `tmp/logscope/{uuid}`, let the agent and its compiler oracle work
//! there WITHOUT ever touching the user's files, then ship the result two ways the human can stage on
//! their own terms:
//!   • a git **bundle** — a self-contained, cloneable repo of the work branch (`git clone x.bundle`)
//!   • a **patch** — `git diff base..work`, applyable straight onto the user's working tree
//!
//! The scratch gets a FRESH `git init` whose baseline is the exact mounted state (including any
//! uncommitted edits), so the agent works on what's actually on disk and the diff is clean. Nothing is
//! written back automatically: the bundle/patch are the entire interface between the sandbox and the
//! user's real tree.

use std::io;
use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum MountKind { CargoProject, CargoWorkspace, SingleFile, PlainDir }
impl MountKind {
    pub fn is_cargo(self) -> bool { matches!(self, MountKind::CargoProject | MountKind::CargoWorkspace) }
}

#[derive(Clone, Debug)]
pub struct Mount {
    pub kind: MountKind,
    pub root: PathBuf,               // the directory mirrored into the scratch (project root, or file's parent)
    pub file: Option<PathBuf>,       // set when a single file was mounted
    pub source_is_git: bool,
    pub source_head: Option<String>, // the source repo's short HEAD, if any (used only in the apply hint)
}

/// Inspect a path: is it a cargo project / workspace / single file / plain dir, and is it under git?
pub fn detect(path: &Path) -> io::Result<Mount> {
    let path = path.canonicalize()?;
    let md = std::fs::metadata(&path)?;
    if md.is_file() {
        let root = path.parent().unwrap_or_else(|| Path::new(".")).to_path_buf();
        return Ok(Mount { kind: MountKind::SingleFile, source_is_git: is_git(&root), source_head: git_head(&root), file: Some(path), root });
    }
    let cargo = path.join("Cargo.toml");
    let kind = if cargo.exists() {
        if cargo_is_workspace(&cargo) { MountKind::CargoWorkspace } else { MountKind::CargoProject }
    } else { MountKind::PlainDir };
    Ok(Mount { kind, source_is_git: is_git(&path), source_head: git_head(&path), file: None, root: path })
}

#[derive(Clone, Debug)]
pub struct Workspace {
    pub id: String,        // the uuid
    pub dir: PathBuf,      // tmp/logscope/{uuid}
    pub base: String,      // baseline tag (logscope-base)
    pub branch: String,    // work branch (logscope-work)
}

/// Mirror the mounted tree into `tmp/logscope/{uuid}` and lay down a fresh git baseline = the current
/// on-disk state. The agent then edits inside `ws.dir`.
pub fn isolate(mount: &Mount, scratch_root: &Path) -> io::Result<Workspace> {
    let id = gen_uuid();
    let dir = scratch_root.join(&id);
    if dir.exists() { let _ = std::fs::remove_dir_all(&dir); }
    std::fs::create_dir_all(&dir)?;
    copy_tree(&mount.root, &dir)?; // skips target/ and .git — we re-init clean
    git(&dir, &["init", "-q"])?;
    git(&dir, &["config", "user.email", "logscope@local"])?;
    git(&dir, &["config", "user.name", "logscope"])?;
    git(&dir, &["checkout", "-q", "-b", "logscope-work"])?;
    git(&dir, &["add", "-A"])?;
    git(&dir, &["commit", "-q", "--allow-empty", "-m", "logscope baseline (mounted state)"])?;
    git(&dir, &["tag", "logscope-base"])?;
    Ok(Workspace { id, dir, base: "logscope-base".into(), branch: "logscope-work".into() })
}

/// Commit whatever the agent changed in the scratch as a new commit on the work branch. Returns false
/// if there was nothing to commit.
pub fn snapshot(ws: &Workspace, message: &str) -> io::Result<bool> {
    git(&ws.dir, &["add", "-A"])?;
    let nothing = Command::new("git").current_dir(&ws.dir).args(["diff", "--cached", "--quiet"]).status()?.success();
    if nothing { return Ok(false); }
    git(&ws.dir, &["commit", "-q", "-m", message])?;
    Ok(true)
}

#[derive(Clone, Debug)]
pub struct Artifacts { pub bundle: PathBuf, pub patch: PathBuf, pub changed: Vec<String> }

/// Produce the handoff artifacts: a cloneable git bundle of the work branch + a patch of base..work.
pub fn bundle(ws: &Workspace, out_dir: &Path) -> io::Result<Artifacts> {
    std::fs::create_dir_all(out_dir)?;
    let bundle = out_dir.join(format!("logscope-{}.bundle", ws.id));
    let patch = out_dir.join(format!("logscope-{}.patch", ws.id));
    // self-contained bundle of the whole work branch (baseline + the agent's commits) → cloneable.
    // HEAD must travel or a bare `git clone` leaves no working tree checked out.
    git(&ws.dir, &["bundle", "create", path_str(&bundle)?, "HEAD", &ws.branch])?;
    let diff = git_out(&ws.dir, &["diff", &ws.base, &ws.branch])?;
    std::fs::write(&patch, diff)?;
    let changed = git_out(&ws.dir, &["diff", "--name-only", &ws.base, &ws.branch])?
        .lines().filter(|l| !l.is_empty()).map(|l| l.to_string()).collect();
    Ok(Artifacts { bundle, patch, changed })
}

/// The exact commands to review (clone the bundle) or stage (apply the patch / fetch the branch).
pub fn apply_hint(a: &Artifacts, mount: &Mount) -> String {
    let mut s = String::new();
    s.push_str("review as a standalone repo:\n");
    s.push_str(&format!("  git clone {} logscope-review\n", a.bundle.display()));
    s.push_str("stage into your working tree:\n");
    s.push_str(&format!("  cd {} && git apply --stat {}   # preview\n", mount.root.display(), a.patch.display()));
    s.push_str(&format!("  cd {} && git apply {}           # apply\n", mount.root.display(), a.patch.display()));
    if mount.source_is_git {
        s.push_str("or pull the branch in without touching your tree:\n");
        s.push_str(&format!("  cd {} && git fetch {} logscope-work:logscope-work\n", mount.root.display(), a.bundle.display()));
    }
    s
}

// ── helpers ───────────────────────────────────────────────────────────────────────────────────────
fn gen_uuid() -> String {
    use std::hash::{Hash, Hasher};
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_nanos()).unwrap_or(0);
    let mut h = std::collections::hash_map::DefaultHasher::new();
    (nanos, std::process::id()).hash(&mut h);
    format!("{:016x}", h.finish())
}
fn path_str(p: &Path) -> io::Result<&str> { p.to_str().ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "non-utf8 path")) }
fn git(dir: &Path, args: &[&str]) -> io::Result<()> {
    let out = Command::new("git").current_dir(dir).args(args).output()?;
    if !out.status.success() {
        return Err(io::Error::new(io::ErrorKind::Other, format!("git {:?}: {}", args, String::from_utf8_lossy(&out.stderr).trim())));
    }
    Ok(())
}
fn git_out(dir: &Path, args: &[&str]) -> io::Result<String> {
    let out = Command::new("git").current_dir(dir).args(args).output()?;
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}
fn is_git(dir: &Path) -> bool {
    Command::new("git").current_dir(dir).args(["rev-parse", "--is-inside-work-tree"]).output().map(|o| o.status.success()).unwrap_or(false)
}
fn git_head(dir: &Path) -> Option<String> {
    let out = Command::new("git").current_dir(dir).args(["rev-parse", "--short", "HEAD"]).output().ok()?;
    out.status.success().then(|| String::from_utf8_lossy(&out.stdout).trim().to_string())
}
fn cargo_is_workspace(cargo: &Path) -> bool {
    std::fs::read_to_string(cargo).map(|s| s.contains("[workspace]")).unwrap_or(false)
}
fn copy_tree(src: &Path, dst: &Path) -> io::Result<()> {
    for e in std::fs::read_dir(src)? {
        let e = e?;
        let ns = e.file_name().to_string_lossy().into_owned();
        if ns == "target" || ns == ".git" { continue; } // build output + vcs: never mirror
        let (from, to) = (e.path(), dst.join(&ns));
        let ft = e.file_type()?;
        if ft.is_dir() { std::fs::create_dir_all(&to)?; copy_tree(&from, &to)?; }
        else if ft.is_file() { std::fs::copy(&from, &to)?; }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::process::Command;

    // End-to-end: mount a cargo project → isolate into scratch → the agent edits → snapshot → bundle,
    // then prove (1) the bundle clones to a standalone repo with the change and (2) the patch applies
    // cleanly onto a fresh copy of the user's tree (and creates a new file the agent added).
    #[test]
    fn mount_isolate_bundle_roundtrip() {
        let tmp = std::env::temp_dir().join(format!("ls-mount-{}-{}", std::process::id(), gen_uuid()));
        let _ = std::fs::remove_dir_all(&tmp);
        let proj = tmp.join("myproj");
        std::fs::create_dir_all(proj.join("src")).unwrap();
        std::fs::write(proj.join("Cargo.toml"), "[package]\nname = \"myproj\"\nversion = \"0.1.0\"\nedition = \"2021\"\n").unwrap();
        std::fs::write(proj.join("src/main.rs"), "fn main() { println!(\"v1\"); }\n").unwrap();

        let m = detect(&proj).unwrap();
        assert_eq!(m.kind, MountKind::CargoProject);
        assert!(m.kind.is_cargo());

        let scratch = tmp.join("scratch");
        let ws = isolate(&m, &scratch).unwrap();
        assert!(ws.dir.starts_with(&scratch), "scratch lives under tmp/logscope/{{uuid}}");
        assert!(ws.dir.join("Cargo.toml").exists() && ws.dir.join("src/main.rs").exists());
        assert!(!ws.dir.join(".git").join("..").join("myproj").exists()); // sanity: it's its own dir

        // the agent works ONLY inside the isolated workspace
        std::fs::write(ws.dir.join("src/main.rs"), "fn main() { println!(\"v2 — edited by the agent\"); }\n").unwrap();
        std::fs::write(ws.dir.join("NOTES.md"), "added by the agent\n").unwrap();
        assert!(snapshot(&ws, "agent: change greeting + add notes").unwrap(), "there were changes");
        // a second snapshot with no changes is a no-op
        assert!(!snapshot(&ws, "noop").unwrap());

        let out = tmp.join("out");
        let arts = bundle(&ws, &out).unwrap();
        assert!(arts.bundle.exists() && arts.patch.exists());
        assert!(arts.changed.iter().any(|f| f.contains("main.rs")), "main.rs in changed set: {:?}", arts.changed);
        assert!(arts.changed.iter().any(|f| f.contains("NOTES.md")), "new file in changed set");

        // (1) bundle clones to a standalone repo carrying the change
        let review = tmp.join("review");
        assert!(Command::new("git").args(["clone", "-q", arts.bundle.to_str().unwrap(), review.to_str().unwrap()]).status().unwrap().success(), "bundle clones");
        assert!(std::fs::read_to_string(review.join("src/main.rs")).unwrap().contains("v2"), "clone has the edit");
        assert!(review.join("NOTES.md").exists(), "clone has the new file");

        // (2) patch applies onto a FRESH copy of the user's tree (never touched the original)
        let real = tmp.join("real");
        std::fs::create_dir_all(real.join("src")).unwrap();
        std::fs::copy(proj.join("Cargo.toml"), real.join("Cargo.toml")).unwrap();
        std::fs::copy(proj.join("src/main.rs"), real.join("src/main.rs")).unwrap();
        Command::new("git").current_dir(&real).args(["init", "-q"]).status().unwrap();
        assert!(Command::new("git").current_dir(&real).args(["apply", arts.patch.to_str().unwrap()]).status().unwrap().success(), "patch applies");
        assert!(std::fs::read_to_string(real.join("src/main.rs")).unwrap().contains("v2"), "patch produced the edit");
        assert!(real.join("NOTES.md").exists(), "patch created the new file");

        // the original source is untouched
        assert!(std::fs::read_to_string(proj.join("src/main.rs")).unwrap().contains("v1"), "the user's tree was never modified");

        let _ = std::fs::remove_dir_all(&tmp);
    }

    #[test]
    fn detect_classifies_paths() {
        let tmp = std::env::temp_dir().join(format!("ls-detect-{}-{}", std::process::id(), gen_uuid()));
        std::fs::create_dir_all(&tmp).unwrap();
        // plain dir
        assert_eq!(detect(&tmp).unwrap().kind, MountKind::PlainDir);
        // cargo workspace
        std::fs::write(tmp.join("Cargo.toml"), "[workspace]\nmembers = [\"a\"]\n").unwrap();
        assert_eq!(detect(&tmp).unwrap().kind, MountKind::CargoWorkspace);
        // single file
        let f = tmp.join("notes.txt");
        std::fs::write(&f, "hi").unwrap();
        let m = detect(&f).unwrap();
        assert_eq!(m.kind, MountKind::SingleFile);
        assert_eq!(m.file.as_deref(), Some(f.canonicalize().unwrap().as_path()));
        let _ = std::fs::remove_dir_all(&tmp);
    }
}

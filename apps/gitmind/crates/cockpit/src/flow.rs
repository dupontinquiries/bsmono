//! End-to-end flows composed from the cockpit primitives, returned as plain structs so
//! every frontend drives identical behavior: the GUI wraps these in JSON + background
//! jobs, the CLI prints them. No Qt, no JSON, no threading here.

use std::path::{Path, PathBuf};
use crate::{buildflags, importer, merge, places, reclaim};
use crate::ops::{build_locally, ensure_gitignore, native_triple_local, now_stamp, tool_path_env};

/// What an imported zip turned out to be.
pub struct Import {
    pub dir: PathBuf,
    pub crate_path: PathBuf,
    pub app: String,
    pub is_bin: bool,
    pub description: String,
    pub has_git: bool,
    pub zip_repo: Option<PathBuf>,
    pub has_gitmind: bool,
}

/// Extract a zip (file:// or path) into a fresh temp dir and detect the crate + whether
/// it carries its own git/gitmind history.
pub fn import(zip: &str) -> Result<Import, String> {
    let path = zip.strip_prefix("file://").unwrap_or(zip);
    if path.trim().is_empty() {
        return Err("no file chosen".into());
    }
    if !Path::new(path).is_file() {
        return Err(format!("not a file: {path}"));
    }
    let dir = importer::import_dir();
    let script = importer::extract_command(path, &dir.display().to_string());
    let out = std::process::Command::new("sh").arg("-c").arg(&script)
        .env("PATH", tool_path_env()).output()
        .map_err(|e| format!("couldn't run the extractor: {e}"))?;
    if !out.status.success() {
        let blob = String::from_utf8_lossy(&out.stdout) + String::from_utf8_lossy(&out.stderr);
        return Err(if blob.contains("GITMIND_NO_UNZIP") {
            "no zip extractor found — install unzip (or bsdtar / python3)".into()
        } else {
            format!("couldn't extract the zip: {}", blob.trim())
        });
    }
    let crate_path = importer::find_crate_root(&dir).ok_or_else(|| {
        let _ = std::fs::remove_dir_all(&dir);
        "no Cargo.toml found in the zip".to_string()
    })?;
    let toml = std::fs::read_to_string(crate_path.join("Cargo.toml")).unwrap_or_default();
    let app = buildflags::crate_name(&toml).unwrap_or_else(|| "app".into());
    let is_bin = crate_path.join("src/main.rs").is_file()
        || crate_path.join("src/bin").is_dir()
        || buildflags::declares_binary(&toml);
    let zip_repo = importer::find_repo_root(&dir);
    Ok(Import {
        has_git: zip_repo.is_some(),
        has_gitmind: zip_repo.as_ref().map(|p| p.join(".gitmind").exists()).unwrap_or(false),
        zip_repo,
        description: buildflags::package_description(&toml).unwrap_or_default(),
        dir, crate_path, app, is_bin,
    })
}

/// Build a crate natively for this device and launch it detached. Returns the binary.
pub fn build_and_run(crate_path: &Path, app: &str) -> Result<PathBuf, String> {
    let native = native_triple_local();
    let (bin, _) = build_locally(app, crate_path, &native, &[], false).map_err(|e| format!("build: {e}"))?;
    std::process::Command::new(&bin).current_dir(crate_path).env("PATH", tool_path_env())
        .spawn().map_err(|e| format!("built, but couldn't launch: {e}"))?;
    Ok(bin)
}

/// Move an imported crate to `dest`, optionally git/gitmind-initialize it, and remember
/// `base`. Returns the final path.
pub fn place(crate_path: &Path, dest: &str, base: &str, init: &str) -> Result<PathBuf, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let dest = places::expand_home(dest.trim(), &home);
    if dest.is_empty() {
        return Err("choose a destination".into());
    }
    if !crate_path.is_dir() {
        return Err("the imported files are no longer there — re-import".into());
    }
    let dpath = PathBuf::from(&dest);
    if dpath.exists() {
        return Err(format!("{dest} already exists — pick another name"));
    }
    // locate the temp wrapper for cleanup
    let temp_root = {
        let mut found = None;
        let mut p: Option<&Path> = Some(crate_path);
        while let Some(cur) = p {
            if cur.file_name().map(|n| n.to_string_lossy().starts_with("gitmind-import-")).unwrap_or(false) {
                found = Some(cur.to_path_buf());
                break;
            }
            p = cur.parent();
        }
        found
    };
    if let Some(parent) = dpath.parent() {
        std::fs::create_dir_all(parent).map_err(|e| format!("create {}: {e}", parent.display()))?;
    }
    let mv = std::process::Command::new("mv").arg(crate_path).arg(&dpath)
        .env("PATH", tool_path_env()).output().map_err(|e| format!("move failed: {e}"))?;
    if !mv.status.success() {
        return Err(format!("move failed: {}", String::from_utf8_lossy(&mv.stderr).trim()));
    }
    let gitmind = init == "gitmind";
    if init == "git" || gitmind {
        let _ = std::process::Command::new("git").args(["init", "-q"])
            .current_dir(&dpath).env("PATH", tool_path_env()).status();
        ensure_gitignore(&dpath, &places::gitignore_lines(gitmind));
        if gitmind {
            let gmdir = dpath.join(".gitmind");
            let _ = std::fs::create_dir_all(&gmdir);
            let _ = std::fs::write(gmdir.join("config.toml"), places::gitmind_config_toml());
        }
    }
    let _ = places::set_repos_base(base);
    if let Some(tr) = temp_root { if tr != dpath { let _ = std::fs::remove_dir_all(&tr); } }
    Ok(dpath)
}

/// Result of overlaying a zip onto an existing repo on a fresh branch.
pub struct ApplyResult {
    pub changes: usize,
    pub branch: String,
    pub base: String,
    pub shortstat: String,
    pub mirror: bool,
    pub files: Vec<(String, String)>, // (status, path)
    pub noop_msg: Option<String>,
}

fn git_in(repo: &Path, args: &[&str]) -> (bool, String, String) {
    match std::process::Command::new("git").args(args).current_dir(repo).env("PATH", tool_path_env()).output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string(), String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) => (false, String::new(), e.to_string()),
    }
}

/// Overlay an imported crate's files onto `target` (a clean git repo) on a new branch,
/// commit, and report the diffstat. `mirror` also deletes files the zip dropped.
pub fn apply_overlay(crate_path: &Path, target: &str, mirror: bool, zipname: &str) -> Result<ApplyResult, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let target = places::expand_home(target.trim(), &home);
    let tpath = PathBuf::from(&target);
    if target.trim().is_empty() { return Err("pick a repo to update".into()); }
    if !tpath.join(".git").exists() { return Err("that folder isn't a git repo".into()); }
    if !crate_path.is_dir() { return Err("the imported files are no longer there — re-import".into()); }
    let (ok, dirty, err) = git_in(&tpath, &["status", "--porcelain"]);
    if !ok { return Err(format!("git status failed: {err}")); }
    if !dirty.is_empty() { return Err("the repo has uncommitted changes — commit or stash them first".into()); }
    let (_, base, _) = git_in(&tpath, &["rev-parse", "--abbrev-ref", "HEAD"]);
    let branch = format!("gitmind-import-{}", now_stamp());
    let (okb, _, errb) = git_in(&tpath, &["checkout", "-b", &branch]);
    if !okb { return Err(format!("couldn't create branch: {errb}")); }
    let script = importer::overlay_command(&crate_path.display().to_string(), &target, mirror);
    let ov = std::process::Command::new("sh").arg("-c").arg(&script).env("PATH", tool_path_env()).output();
    let bad = |msg: String, base: &str, branch: &str| -> String {
        let _ = git_in(&tpath, &["checkout", base]);
        let _ = git_in(&tpath, &["branch", "-D", branch]);
        msg
    };
    match ov {
        Ok(o) if o.status.success() => {}
        Ok(o) => {
            let blob = String::from_utf8_lossy(&o.stdout) + String::from_utf8_lossy(&o.stderr);
            let msg = if blob.contains("GITMIND_NO_RSYNC") { "rsync is required to update an existing repo — install it (apk add rsync)".to_string() } else { format!("overlay failed: {}", blob.trim()) };
            return Err(bad(msg, &base, &branch));
        }
        Err(e) => return Err(bad(format!("overlay failed: {e}"), &base, &branch)),
    }
    git_in(&tpath, &["add", "-A"]);
    let (_, shortstat, _) = git_in(&tpath, &["diff", "--cached", "--shortstat"]);
    if shortstat.is_empty() {
        let _ = git_in(&tpath, &["checkout", &base]);
        let _ = git_in(&tpath, &["branch", "-D", &branch]);
        return Ok(ApplyResult { changes: 0, branch: String::new(), base, shortstat: String::new(), mirror, files: vec![], noop_msg: Some("no changes — the zip matches the repo".into()) });
    }
    let (_, namestat, _) = git_in(&tpath, &["diff", "--cached", "--name-status"]);
    let files: Vec<(String, String)> = namestat.lines().take(60).filter_map(|l| {
        let mut it = l.split('\t');
        Some((it.next()?.to_string(), it.next()?.to_string()))
    }).collect();
    let msg = if zipname.is_empty() { "import from zip".to_string() } else { format!("import from {zipname}") };
    git_in(&tpath, &["commit", "-q", "-m", &msg]);
    Ok(ApplyResult { changes: files.len(), branch, base, shortstat, mirror, files, noop_msg: None })
}

/// Explore merge strategies for a zip that carries git history, in a throwaway workspace.
pub fn explore(zip_repo: &str, target: &str) -> Result<merge::Exploration, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let target = places::expand_home(target.trim(), &home);
    if zip_repo.trim().is_empty() { return Err("import a repo first".into()); }
    if target.trim().is_empty() { return Err("pick a repo to merge into".into()); }
    if !Path::new(&target).join(".git").exists() { return Err("that folder isn't a git repo".into()); }
    if !Path::new(zip_repo).join(".git").exists() { return Err("the zip doesn't contain a git repo — use overlay instead".into()); }
    let ws = std::env::temp_dir().join(format!("gitmind-merge-{}-{}", std::process::id(), now_stamp()));
    merge::run_exploration(Path::new(&target), Path::new(zip_repo), &ws, &tool_path_env())
        .map_err(|e| { let _ = std::fs::remove_dir_all(&ws); e })
}

/// Land a chosen exploration result into the real repo as a new branch (fetched from the
/// workspace; existing branches untouched). Returns the new branch name.
pub fn land_merge(workspace: &str, branch: &str, target: &str, strategy: &str) -> Result<String, String> {
    let home = std::env::var("HOME").unwrap_or_default();
    let target = places::expand_home(target.trim(), &home);
    if workspace.is_empty() || branch.is_empty() { return Err("explore first, then pick a strategy".into()); }
    let newbranch = format!("gitmind-merge-{}-{}", strategy, now_stamp());
    let spec = format!("{branch}:{newbranch}");
    let out = std::process::Command::new("git").args(["fetch", "--quiet", workspace, &spec])
        .current_dir(&target).env("PATH", tool_path_env()).output()
        .map_err(|e| format!("git fetch: {e}"))?;
    if !out.status.success() {
        return Err(format!("couldn't land the branch: {}", String::from_utf8_lossy(&out.stderr).trim()));
    }
    if let Some(ws_root) = Path::new(workspace).parent() {
        if ws_root.file_name().map(|n| n.to_string_lossy().starts_with("gitmind-merge-")).unwrap_or(false) {
            let _ = std::fs::remove_dir_all(ws_root);
        }
    }
    Ok(newbranch)
}

/// A discovered repo workspace.
pub struct Repo {
    pub path: String,
    pub name: String,
    pub git: bool,
    pub gitmind: bool,
    pub key: String,
}

/// Walk up to 6 levels under `root` for git/gitmind workspaces, one entry per repo root,
/// each tagged with its project identity key (shared across copies of the same project).
pub fn discover_repos(root: &Path) -> Vec<Repo> {
    let path_env = crate::ops::tool_path_env();
    let cmd = "find \"$GMROOT\" -maxdepth 6 \\( -name .git -o -name .gitmind \\) 2>/dev/null | head -300";
    let out = std::process::Command::new("sh").arg("-c").arg(cmd)
        .env("GMROOT", root).env("PATH", &path_env).output();
    let mut roots: Vec<PathBuf> = Vec::new();
    if let Ok(o) = out {
        for line in String::from_utf8_lossy(&o.stdout).lines() {
            let l = line.trim();
            if l.is_empty() { continue; }
            if let Some(parent) = Path::new(l).parent() {
                let p = parent.to_path_buf();
                if !roots.contains(&p) { roots.push(p); }
            }
        }
    }
    roots.sort();
    roots.iter().map(|r| Repo {
        path: r.display().to_string(),
        name: r.file_name().map(|n| n.to_string_lossy().into_owned()).unwrap_or_default(),
        git: r.join(".git").exists(),
        gitmind: r.join(".gitmind").exists(),
        key: crate::identity::project_key(r, &path_env),
    }).collect()
}

/// Find projects that exist in more than one workspace under `root`, grouped by identity.
pub fn duplicates(root: &Path) -> Vec<crate::identity::DupGroup> {
    let entries = discover_repos(root).into_iter()
        .map(|r| (r.path, r.key, r.name)).collect();
    crate::identity::group_duplicates(entries)
}

/// Reclaimable per-crate target dirs under `root`.
pub struct ReclaimScan {
    pub items: Vec<(String, u64)>, // (path, bytes)
    pub total_bytes: u64,
}

pub fn reclaim_scan(root: &Path) -> ReclaimScan {
    let keep = crate::ops::build_base();
    let dirs = reclaim::find_reclaimable(root, &keep);
    let mut total = 0u64;
    let items = dirs.iter().map(|p| {
        let b = reclaim::dir_size(p);
        total += b;
        (p.display().to_string(), b)
    }).collect();
    ReclaimScan { items, total_bytes: total }
}

/// Delete the reclaimable target dirs (re-validated per dir). Returns (count, freed).
pub fn reclaim_apply(root: &Path) -> (usize, u64) {
    let keep = crate::ops::build_base();
    let mut freed = 0u64;
    let mut count = 0usize;
    for p in reclaim::find_reclaimable(root, &keep) {
        if reclaim::is_safe_target(&p, root, &keep) {
            let b = reclaim::dir_size(&p);
            if std::fs::remove_dir_all(&p).is_ok() { freed += b; count += 1; }
        }
    }
    (count, freed)
}

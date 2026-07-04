//! End-to-end flows composed from the cockpit primitives, returned as plain structs so
//! every frontend drives identical behavior: the GUI wraps these in JSON + background
//! jobs, the CLI prints them. No Qt, no JSON, no threading here.

use std::path::{Path, PathBuf};
use crate::{buildflags, importer, merge, places};
use crate::ops::{build_locally, ensure_gitignore, native_triple_local, now_stamp, tool_path_env};

/// What an imported zip turned out to be.
pub struct Import {
    pub dir: PathBuf,
    pub crate_path: PathBuf,
    pub app: String,
    pub is_bin: bool,
    pub description: String,
    pub system: String,
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
    let proj = crate::buildsystem::detect(&dir).ok_or_else(|| {
        let _ = std::fs::remove_dir_all(&dir);
        "no supported project found in the zip (looked for Rust, Python, Node, Go, CMake, Make)".to_string()
    })?;
    let zip_repo = importer::find_repo_root(&dir);
    Ok(Import {
        has_git: zip_repo.is_some(),
        has_gitmind: zip_repo.as_ref().map(|p| p.join(".gitmind").exists()).unwrap_or(false),
        zip_repo,
        system: proj.kind.label().to_string(),
        app: proj.name,
        is_bin: proj.runnable,
        description: proj.description,
        crate_path: proj.root,
        dir,
    })
}

/// Build an imported project (any supported build system) for this device and launch it
/// detached. Returns the runnable artifact.
pub fn build_and_run(crate_path: &Path, _app: &str) -> Result<PathBuf, String> {
    let proj = crate::buildsystem::detect(crate_path).ok_or("couldn't tell what kind of project this is")?;
    let pe = tool_path_env();
    let target = native_triple_local();
    let built = crate::buildsystem::build(&proj, &target, &[], &pe)?;
    std::process::Command::new(&built.run).args(&built.args).current_dir(&proj.root).env("PATH", &pe)
        .spawn().map_err(|e| format!("built, but couldn't launch: {e}"))?;
    Ok(built.install)
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
        ensure_gitignore(&dpath, &crate::artifacts::gitignore_lines(gitmind));
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

/// Reclaimable build junk under `root` (all systems), biggest first.
pub struct ReclaimScan {
    pub items: Vec<(String, u64, String)>, // (path, bytes, label)
    pub total_bytes: u64,
}

pub fn reclaim_scan(root: &Path) -> ReclaimScan {
    let keep = crate::ops::build_base();
    let junk = crate::artifacts::scan(root, &keep);
    let total = junk.iter().map(|j| j.bytes).sum();
    let items = junk.into_iter().map(|j| (j.path.display().to_string(), j.bytes, j.label)).collect();
    ReclaimScan { items, total_bytes: total }
}

/// Delete reclaimable build junk under `root` (re-validated per entry). Returns (count, freed).
pub fn reclaim_apply(root: &Path) -> (usize, u64) {
    crate::artifacts::clean(root, &crate::ops::build_base())
}

/// Prepare a project to ingest: remove regeneratable build junk and make sure .gitignore
/// excludes it (so a venv / node_modules / target tree never lands in git or the store).
pub struct IngestPrep {
    pub system: String,
    pub cleaned: usize,
    pub freed: u64,
    pub ignored: usize,
}

pub fn prepare_ingest(dir: &Path) -> IngestPrep {
    let system = crate::buildsystem::detect(dir).map(|d| d.kind.label().to_string()).unwrap_or_else(|| "project".into());
    let (cleaned, freed) = crate::artifacts::clean(dir, &crate::ops::build_base());
    let lines = crate::artifacts::gitignore_lines(dir.join(".gitmind").exists());
    let before = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    ensure_gitignore(dir, &lines);
    let after = std::fs::read_to_string(dir.join(".gitignore")).unwrap_or_default();
    let ignored = after.lines().count().saturating_sub(before.lines().count());
    IngestPrep { system, cleaned, freed, ignored }
}

// ── add an imported/built app to the local apps list, runnable from the GUI/CLI ──

/// An app added to the local catalog.
pub struct Installed {
    pub name: String,
    pub label: String,
    pub path: String,
    pub run_args: String,
}

fn run_args_path() -> Option<PathBuf> {
    std::env::var("HOME").ok().filter(|h| !h.is_empty())
        .map(|h| PathBuf::from(format!("{h}/.config/gitmind/run-args.json")))
}

/// The saved launch flags for a local app (empty if none).
pub fn run_args(name: &str) -> String {
    let Some(p) = run_args_path() else { return String::new() };
    let Ok(text) = std::fs::read_to_string(&p) else { return String::new() };
    serde_json::from_str::<serde_json::Value>(&text).ok()
        .and_then(|v| v.get(name).and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_default()
}

/// Remember the launch flags for a local app.
pub fn set_run_args(name: &str, args: &str) -> Result<(), String> {
    let p = run_args_path().ok_or_else(|| "no HOME".to_string())?;
    if let Some(dir) = p.parent() { let _ = std::fs::create_dir_all(dir); }
    let mut map: serde_json::Map<String, serde_json::Value> = std::fs::read_to_string(&p).ok()
        .and_then(|t| serde_json::from_str(&t).ok()).unwrap_or_default();
    map.insert(name.to_string(), serde_json::Value::String(args.to_string()));
    std::fs::write(&p, serde_json::to_string_pretty(&map).unwrap_or_default()).map_err(|e| e.to_string())
}

/// Install a built binary into the local app catalog (var store + manifest + .desktop) and
/// remember any launch flags. The app then appears in the apps list and runs locally.
pub fn add_to_apps(bin: &Path, name: &str, commit: &str, run_args: &str) -> Result<Installed, String> {
    let bytes = std::fs::read(bin).map_err(|e| format!("read binary: {e}"))?;
    let store = gitmind_core::varcache::open_var_store()?;
    let label = format!("import-{}", crate::ops::now_stamp());
    let commit = if commit.trim().is_empty() { "import" } else { commit };
    let v = gitmind_core::binpin::install(&store, name, &label, commit, "host", &bytes, true)?;
    let _ = gitmind_core::binpin::write_desktop(name, &label, false, None);
    set_run_args(name, run_args)?;
    Ok(Installed { name: name.into(), label, path: v.path, run_args: run_args.into() })
}

/// Build an imported project (any supported build system) natively, then add it to the
/// local apps list with launch flags.
pub fn build_and_add(crate_path: &Path, _app: &str, run_args: &str) -> Result<Installed, String> {
    let proj = crate::buildsystem::detect(crate_path).ok_or("couldn't tell what kind of project this is")?;
    let pe = tool_path_env();
    let target = native_triple_local();
    let built = crate::buildsystem::build(&proj, &target, &[], &pe)?;
    add_to_apps(&built.install, &proj.name, "import", run_args)
}

/// Launch a locally-installed app (its current pinned version) with its saved flags plus
/// any extra args, detached. Returns a short status.
pub fn run_local(name: &str, extra: &str) -> Result<String, String> {
    let bin = gitmind_core::binpin::bin_dir().join(name);
    if !bin.exists() {
        return Err(format!("{name} isn't installed locally — add it to your apps first"));
    }
    let mut argv: Vec<String> = run_args(name).split_whitespace().map(|s| s.to_string()).collect();
    argv.extend(extra.split_whitespace().map(|s| s.to_string()));
    std::process::Command::new(&bin).args(&argv).env("PATH", tool_path_env())
        .spawn().map_err(|e| format!("couldn't launch {name}: {e}"))?;
    Ok(format!("launched {name}{}", if argv.is_empty() { String::new() } else { format!(" {}", argv.join(" ")) }))
}

// ── folder-scoped discovery + comparing duplicate copies ──

/// Discover repos across several scan roots (e.g. $HOME plus a mounted drive), one entry
/// per repo, deduped by path.
pub fn discover_scoped(roots: &[String]) -> Vec<Repo> {
    let mut seen = std::collections::BTreeSet::new();
    let mut out = Vec::new();
    for root in roots {
        if root.trim().is_empty() { continue; }
        for r in discover_repos(Path::new(root)) {
            if seen.insert(r.path.clone()) { out.push(r); }
        }
    }
    out
}

/// Duplicate projects across the configured scan roots ($HOME + any added drives).
pub fn duplicates_all() -> Vec<crate::identity::DupGroup> {
    let roots = crate::places::scan_roots();
    let entries = discover_scoped(&roots).into_iter().map(|r| (r.path, r.key, r.name)).collect();
    crate::identity::group_duplicates(entries)
}

/// One copy within a duplicate group, relative to the newest copy.
pub struct CopyStatus {
    pub path: String,
    pub role: String,  // "newest" | "older" | "same" | "diverged" | "newer" | "unrelated"
    pub behind: usize, // commits this copy is missing vs the newest
    pub ahead: usize,  // commits this copy has that the newest lacks (diverged)
}

fn commit_count(repo: &str, path_env: &str) -> usize {
    std::process::Command::new("git").args(["rev-list", "--count", "HEAD"]).current_dir(repo)
        .env("PATH", path_env).output().ok()
        .filter(|o| o.status.success())
        .and_then(|o| String::from_utf8_lossy(&o.stdout).trim().parse().ok())
        .unwrap_or(0)
}

/// For a set of copies of one project, pick the newest (most commits) and classify each
/// other copy against it: older (a clean fast-forward away), identical, or diverged (needs
/// a merge to roll together). Returns (newest_path, statuses).
pub fn compare_group(paths: &[String]) -> (String, Vec<CopyStatus>) {
    let pe = crate::ops::tool_path_env();
    let newest = paths.iter().max_by_key(|p| commit_count(p, &pe)).cloned().unwrap_or_default();
    let statuses = paths.iter().map(|p| {
        if *p == newest {
            return CopyStatus { path: p.clone(), role: "newest".into(), behind: 0, ahead: 0 };
        }
        let r = crate::identity::relate(Path::new(&newest), Path::new(p), &pe);
        let (role, behind, ahead) = match r.relation.as_str() {
            "ahead" => ("older", r.a_ahead, 0),
            "same" => ("same", 0, 0),
            "diverged" => ("diverged", r.a_ahead, r.b_ahead),
            "behind" => ("newer", 0, r.b_ahead),
            _ => ("unrelated", 0, 0),
        };
        CopyStatus { path: p.clone(), role: role.into(), behind, ahead }
    }).collect();
    (newest, statuses)
}

// ── guarded roll-up (fast-forward a strict-older copy) + merge preview ──

fn git_at(cwd: &Path, path_env: &str, args: &[&str]) -> (bool, String) {
    match std::process::Command::new("git").args(args).current_dir(cwd).env("PATH", path_env).output() {
        Ok(o) => (o.status.success(), String::from_utf8_lossy(&o.stdout).trim().to_string()),
        Err(_) => (false, String::new()),
    }
}

fn parse_commits(raw: &str) -> Vec<(String, String)> {
    raw.lines().filter_map(|l| {
        let mut it = l.splitn(2, '\u{1f}');
        Some((it.next()?.to_string(), it.next().unwrap_or("").to_string()))
    }).collect()
}

fn parse_namestatus(raw: &str) -> Vec<(String, String)> {
    raw.lines().take(200).filter_map(|l| {
        let mut it = l.split('\t');
        Some((it.next()?.to_string(), it.next()?.to_string()))
    }).collect()
}

/// What fast-forwarding `older` up to `newer` would bring in — computed in a throwaway
/// bare clone, so nothing is touched. `can_fast_forward` is true only when `older` is a
/// strict ancestor of `newer` (a clean, lossless advance).
pub struct RollupPreview {
    pub relation: String,
    pub can_fast_forward: bool,
    pub behind: usize,
    pub ahead: usize,
    pub incoming: Vec<(String, String)>, // (short oid, subject)
    pub files: Vec<(String, String)>,    // (status, path)
}

pub fn rollup_preview(older: &Path, newer: &Path) -> RollupPreview {
    let pe = crate::ops::tool_path_env();
    let rel = crate::identity::relate(older, newer, &pe);
    let can_ff = rel.relation == "behind" || rel.relation == "same";
    // gather the incoming commits + file changes in a throwaway bare clone of older
    let nanos = std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.subsec_nanos()).unwrap_or(0);
    let tmp = std::env::temp_dir().join(format!("gm-rollup-{}-{}", std::process::id(), nanos));
    let _ = std::fs::remove_dir_all(&tmp);
    let mut incoming = Vec::new();
    let mut files = Vec::new();
    if git_at(Path::new("/"), &pe, &["clone", "--bare", "--quiet", "--local", &older.display().to_string(), &tmp.display().to_string()]).0 {
        let (_, a_head) = git_at(&tmp, &pe, &["rev-parse", "HEAD"]);
        if git_at(&tmp, &pe, &["fetch", "--no-tags", "--quiet", &newer.display().to_string(), "HEAD:refs/gmn"]).0 && !a_head.is_empty() {
            let range = format!("{a_head}..refs/gmn");
            let (_, log) = git_at(&tmp, &pe, &["log", "--format=%h%x1f%s", &range]);
            incoming = parse_commits(&log);
            let (_, ns) = git_at(&tmp, &pe, &["diff", "--name-status", &range]);
            files = parse_namestatus(&ns);
        }
    }
    let _ = std::fs::remove_dir_all(&tmp);
    RollupPreview { relation: rel.relation, can_fast_forward: can_ff, behind: rel.b_ahead, ahead: rel.a_ahead, incoming, files }
}

/// Roll a strict-older copy up to the newer one by fast-forwarding it in place. Guarded:
/// the copy must have a clean working tree and be a strict ancestor of `newer`, so this
/// can only *advance* it — it never rewrites history, never merges, and never deletes the
/// folder. Diverged copies are refused (use the merge preview instead).
pub fn rollup_apply(older: &Path, newer: &Path) -> Result<String, String> {
    let pe = crate::ops::tool_path_env();
    if !older.join(".git").exists() { return Err("that copy isn't a git repo".into()); }
    let (ok, dirty) = git_at(older, &pe, &["status", "--porcelain"]);
    if !ok { return Err("couldn't read the copy's status".into()); }
    if !dirty.is_empty() { return Err("that copy has uncommitted changes — commit or stash them first".into()); }
    let rel = crate::identity::relate(older, newer, &pe);
    match rel.relation.as_str() {
        "same" => return Ok("already up to date".into()),
        "behind" => {}
        "diverged" => return Err("these have diverged — preview a merge instead of a fast-forward".into()),
        "ahead" => return Err("this copy is newer than the other — nothing to roll up".into()),
        _ => return Err("these don't share history".into()),
    }
    // bring the newer tip in and fast-forward the current branch (safe: --ff-only)
    if !git_at(older, &pe, &["fetch", "--no-tags", "--quiet", &newer.display().to_string(), "HEAD"]).0 {
        return Err("couldn't read the newer copy".into());
    }
    let (ff_ok, _) = git_at(older, &pe, &["merge", "--ff-only", "FETCH_HEAD"]);
    if !ff_ok {
        return Err("couldn't fast-forward (the copy may have local commits) — preview a merge instead".into());
    }
    Ok(format!("fast-forwarded {} commit(s) — this copy now matches the newer one", rel.b_ahead))
}

/// What a chosen exploration strategy would change, relative to the target branch —
/// read-only, from the throwaway workspace the exploration built.
pub struct MergePreview {
    pub shortstat: String,
    pub commits: Vec<(String, String)>,
    pub files: Vec<(String, String)>,
}

pub fn merge_preview(workspace: &str, target_branch: &str, branch: &str) -> Result<MergePreview, String> {
    let pe = crate::ops::tool_path_env();
    let ws = Path::new(workspace);
    if !ws.exists() { return Err("that preview has expired — explore again".into()); }
    let range = format!("{target_branch}..{branch}");
    let (ok, shortstat) = git_at(ws, &pe, &["diff", "--shortstat", &range]);
    if !ok { return Err("couldn't read the preview".into()); }
    let (_, log) = git_at(ws, &pe, &["log", "--format=%h%x1f%s", &range]);
    let (_, ns) = git_at(ws, &pe, &["diff", "--name-status", &range]);
    Ok(MergePreview {
        shortstat: if shortstat.is_empty() { "no file changes".into() } else { shortstat },
        commits: parse_commits(&log),
        files: parse_namestatus(&ns),
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn g(repo: &Path, args: &[&str]) {
        let mut full: Vec<&str> = vec!["-c", "user.name=T", "-c", "user.email=t@t"];
        full.extend_from_slice(args);
        let o = std::process::Command::new("git").args(&full).current_dir(repo).output().unwrap();
        assert!(o.status.success(), "git {:?}: {}", args, String::from_utf8_lossy(&o.stderr));
    }

    #[test]
    fn rollup_previews_then_fast_forwards_a_strict_older_copy() {
        let base = std::env::temp_dir().join(format!("gm-ru-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let newer = base.join("newer");
        fs::create_dir_all(&newer).unwrap();
        g(&newer, &["init", "-q"]);
        fs::write(newer.join("f"), "1\n").unwrap();
        g(&newer, &["add", "-A"]); g(&newer, &["commit", "-qm", "base"]);
        // older = clone at base, then newer advances
        let older = base.join("older");
        g(&base, &["clone", "-q", &newer.display().to_string(), &older.display().to_string()]);
        fs::write(newer.join("g"), "2\n").unwrap();
        g(&newer, &["add", "-A"]); g(&newer, &["commit", "-qm", "add g"]);

        let pv = rollup_preview(&older, &newer);
        assert!(pv.can_fast_forward, "older is a strict ancestor → ff possible ({})", pv.relation);
        assert_eq!(pv.behind, 1, "older is one commit behind");
        assert!(pv.incoming.iter().any(|(_, s)| s == "add g"), "preview lists the incoming commit");
        assert!(pv.files.iter().any(|(_, p)| p == "g"), "preview lists the new file");

        // apply the fast-forward — older now matches newer, in place, nothing deleted
        let msg = rollup_apply(&older, &newer).expect("ff applies");
        assert!(msg.contains("matches"), "{msg}");
        assert!(older.join("g").exists(), "the incoming file is now present in the older copy");
        let a = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&older).output().unwrap();
        let b = std::process::Command::new("git").args(["rev-parse", "HEAD"]).current_dir(&newer).output().unwrap();
        assert_eq!(a.stdout, b.stdout, "heads match after fast-forward");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn diverged_copies_refuse_fast_forward() {
        let base = std::env::temp_dir().join(format!("gm-ru2-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let a = base.join("a");
        fs::create_dir_all(&a).unwrap();
        g(&a, &["init", "-q"]);
        fs::write(a.join("f"), "1\n").unwrap();
        g(&a, &["add", "-A"]); g(&a, &["commit", "-qm", "base"]);
        let b = base.join("b");
        g(&base, &["clone", "-q", &a.display().to_string(), &b.display().to_string()]);
        // both diverge
        fs::write(a.join("x"), "a\n").unwrap(); g(&a, &["add", "-A"]); g(&a, &["commit", "-qm", "a work"]);
        fs::write(b.join("y"), "b\n").unwrap(); g(&b, &["add", "-A"]); g(&b, &["commit", "-qm", "b work"]);
        assert!(rollup_apply(&a, &b).is_err(), "diverged → fast-forward refused, no data touched");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn merge_preview_shows_what_a_strategy_changes() {
        let base = std::env::temp_dir().join(format!("gm-mp-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        let target = base.join("target");
        let other = base.join("other");
        fs::create_dir_all(&target).unwrap();
        g(&target, &["init", "-q"]);
        fs::write(target.join("f"), "base\n").unwrap();
        g(&target, &["add", "-A"]); g(&target, &["commit", "-qm", "base"]);
        g(&base, &["clone", "-q", &target.display().to_string(), &other.display().to_string()]);
        fs::write(other.join("feature"), "new\n").unwrap();
        g(&other, &["add", "-A"]); g(&other, &["commit", "-qm", "feature work"]);

        let ws = base.join("ws");
        let pe = std::env::var("PATH").unwrap_or_default();
        let ex = crate::merge::run_exploration(&target, &other, &ws, &pe).expect("explore");
        let win = ex.trials.iter().find(|t| t.ok).expect("a clean strategy");
        let pv = merge_preview(&ex.workspace.display().to_string(), &ex.target_branch, &win.branch).expect("preview");
        assert!(pv.files.iter().any(|(_, p)| p == "feature"), "preview shows the file the merge introduces");
        assert!(pv.commits.iter().any(|(_, s)| s == "feature work"), "preview shows the incoming commit");
        let _ = fs::remove_dir_all(&base);
    }

    #[test]
    fn imports_a_json_bundle_and_scans_downloads() {
        let base = std::env::temp_dir().join(format!("gm-dl-{}", std::process::id()));
        let _ = fs::remove_dir_all(&base);
        fs::create_dir_all(&base).unwrap();
        // a suite json bundle
        let suite = crate::suite::Suite {
            name: "mini".into(), description: "two apps".into(),
            apps: vec![crate::suite::SuiteApp::simple("a", "/x/a"), crate::suite::SuiteApp::simple("b", "/x/b")],
        };
        let bundle = base.join("mini.json");
        fs::write(&bundle, suite.to_json()).unwrap();
        let loaded = import_bundle(&bundle.display().to_string()).expect("bundle loads");
        assert_eq!(loaded.apps.len(), 2);
        assert_eq!(loaded.name, "mini");
        // a plain zip file (content irrelevant here) shows up as an app
        fs::write(base.join("game.zip"), b"PK\x03\x04").unwrap();
        let items = scan_downloads(&base);
        assert!(items.iter().any(|i| i.kind == "bundle" && i.name == "mini" && i.apps == 2), "json bundle detected");
        assert!(items.iter().any(|i| i.kind == "app" && i.name == "game"), "zip detected as an app");
        let _ = fs::remove_dir_all(&base);
    }
}

// ── suites: bundle multiple apps and install them together ──

/// Generate a default "all" suite by scanning `root` for binary crates, one entry each
/// with sane defaults (native build, default features, no flags). Deduped by crate name.
pub fn default_suite(root: &Path) -> crate::suite::Suite {
    let pe = crate::ops::tool_path_env();
    let cmd = "find \"$GMROOT\" -maxdepth 8 -name Cargo.toml \
               -not -path '*/target/*' -not -path '*/vendor/*' -not -path '*/node_modules/*' \
               2>/dev/null | head -400";
    let out = std::process::Command::new("sh").arg("-c").arg(cmd)
        .env("GMROOT", root).env("PATH", &pe).output();
    let mut apps: Vec<crate::suite::SuiteApp> = Vec::new();
    let mut seen = std::collections::BTreeSet::new();
    if let Ok(o) = out {
        let mut tomls: Vec<String> = String::from_utf8_lossy(&o.stdout).lines().map(|l| l.trim().to_string()).filter(|l| !l.is_empty()).collect();
        tomls.sort();
        for toml_path in tomls {
            let dir = match Path::new(&toml_path).parent() { Some(d) => d.to_path_buf(), None => continue };
            let toml = std::fs::read_to_string(&toml_path).unwrap_or_default();
            let is_bin = dir.join("src/main.rs").is_file() || dir.join("src/bin").is_dir() || buildflags::declares_binary(&toml);
            if !is_bin { continue; }
            let name = match buildflags::crate_name(&toml) { Some(n) => n, None => continue }; // skip virtual/workspace manifests
            if seen.insert(name.clone()) {
                apps.push(crate::suite::SuiteApp::simple(&name, &dir.display().to_string()));
            }
        }
    }
    apps.sort_by(|a, b| a.name.cmp(&b.name));
    crate::suite::Suite {
        name: "all".into(),
        description: format!("All {} apps — native build, default features", apps.len()),
        apps,
    }
}

/// Result of installing one app from a suite.
pub struct SuiteAppResult {
    pub name: String,
    pub ok: bool,
    pub message: String,
}

/// Build one suite app for this device and add it to the local apps list (+ .desktop
/// launcher). Native target unless the app pins one.
pub fn install_suite_app(app: &crate::suite::SuiteApp) -> Result<Installed, String> {
    let target = if app.target.trim().is_empty() { native_triple_local() } else { app.target.clone() };
    let (bin, _) = build_locally(&app.name, &crate::ops::resolve_app_dir(&app.path), &target, &app.features, app.no_default)
        .map_err(|e| format!("build: {e}"))?;
    add_to_apps(&bin, &app.name, "suite", &app.run_flags)
}

/// Like [`install_suite_app`] but streams the build log to `on_line`.
pub fn install_suite_app_logged(app: &crate::suite::SuiteApp, on_line: &mut dyn FnMut(&str)) -> Result<Installed, String> {
    let target = if app.target.trim().is_empty() { native_triple_local() } else { app.target.clone() };
    let (bin, _) = crate::ops::build_locally_logged(
        &app.name,
        &crate::ops::resolve_app_dir(&app.path),
        &target,
        &app.features,
        app.no_default,
        on_line,
    )
    .map_err(|e| format!("build: {e}"))?;
    add_to_apps(&bin, &app.name, "suite", &app.run_flags)
}

/// Install every app in a suite, collecting a per-app result (never aborts on one failure).
pub fn install_suite(suite: &crate::suite::Suite) -> Vec<SuiteAppResult> {
    suite.apps.iter().map(|a| match install_suite_app(a) {
        Ok(i) => SuiteAppResult { name: a.name.clone(), ok: true, message: format!("installed @{}", i.label) },
        Err(e) => SuiteAppResult { name: a.name.clone(), ok: false, message: e },
    }).collect()
}

/// Build one app **here** for `target` (cross-compiling if the device's libc differs) and
/// install it on a remote device: push the binary, then drop it into `~/.local/bin` with a
/// `.desktop` launcher. This is the "desktop builds, phone runs" path for a single app.
pub fn deploy_app_remote(app: &crate::suite::SuiteApp, target: &str, user: &str, host: &str) -> Result<String, String> {
    let tgt = if target.trim().is_empty() { native_triple_local() } else { target.to_string() };
    let (bin, _) = build_locally(&app.name, &crate::ops::resolve_app_dir(&app.path), &tgt, &app.features, app.no_default)
        .map_err(|e| format!("build: {e}"))?;
    crate::deploy::scp_push(&bin.display().to_string(), user, host, &format!("/tmp/{}", app.name))
        .map_err(|e| format!("push: {e}"))?;
    crate::deploy::remote_capture(user, host, &crate::installer::push_install_script(&app.name, false))
        .map(|_| format!("installed {} on {host}", app.name))
        .map_err(|e| format!("install: {e}"))
}

/// Build **every** app in a group here and install them all on a remote device — the
/// "export a bundle, desktop builds all the apps, run through them on the phone" flow. Never
/// aborts on one app's failure.
/// Like [`deploy_app_remote`] but streams the build log to `on_line`.
pub fn deploy_app_remote_logged(app: &crate::suite::SuiteApp, target: &str, user: &str, host: &str, on_line: &mut dyn FnMut(&str)) -> Result<String, String> {
    let tgt = if target.trim().is_empty() { native_triple_local() } else { target.to_string() };
    let (bin, _) = crate::ops::build_locally_logged(&app.name, &crate::ops::resolve_app_dir(&app.path), &tgt, &app.features, app.no_default, on_line)
        .map_err(|e| format!("build: {e}"))?;
    on_line(&format!("• pushing {} → {host}…", app.name));
    crate::deploy::scp_push(&bin.display().to_string(), user, host, &format!("/tmp/{}", app.name)).map_err(|e| format!("push: {e}"))?;
    crate::deploy::remote_capture(user, host, &crate::installer::push_install_script(&app.name, false))
        .map(|_| format!("installed {} on {host}", app.name))
        .map_err(|e| format!("install: {e}"))
}

/// Like [`install_group_remote`] but streams per-app progress + build output to `on_line`.
pub fn install_group_remote_logged(group: &crate::suite::Suite, target: &str, user: &str, host: &str, on_line: &mut dyn FnMut(&str)) -> Vec<SuiteAppResult> {
    let n = group.apps.len();
    group.apps.iter().enumerate().map(|(i, a)| {
        on_line(&format!("── [{}/{}] {} ──", i + 1, n, a.name));
        match deploy_app_remote_logged(a, target, user, host, on_line) {
            Ok(msg) => SuiteAppResult { name: a.name.clone(), ok: true, message: msg },
            Err(e) => {
                on_line(&format!("  ✗ {} — {e}", a.name));
                SuiteAppResult { name: a.name.clone(), ok: false, message: e }
            }
        }
    }).collect()
}

pub fn install_group_remote(group: &crate::suite::Suite, target: &str, user: &str, host: &str) -> Vec<SuiteAppResult> {
    group.apps.iter().map(|a| match deploy_app_remote(a, target, user, host) {
        Ok(msg) => SuiteAppResult { name: a.name.clone(), ok: true, message: msg },
        Err(e) => SuiteAppResult { name: a.name.clone(), ok: false, message: e },
    }).collect()
}

/// Generate the default suite from `root` and save it, returning it.
pub fn create_default_suite(root: &Path) -> Result<crate::suite::Suite, String> {
    let s = default_suite(root);
    if s.apps.is_empty() {
        return Err("no buildable apps found — check the folder".into());
    }
    crate::suite::save_suite(&s)?;
    Ok(s)
}

// ── BigSilicon app-store groups ──

/// The curated app-store groups shown on the store page. Built in (not file-dependent) so
/// the store works on a fresh device; `bigsilicons-apps` is the union — "build everything
/// in one go". Paths are repo-relative `apps/<name>` (resolved against the gitmind root at
/// build time by the same machinery suites use).
pub fn store_groups() -> Vec<crate::suite::Suite> {
    fn group(name: &str, desc: &str, apps: &[&str]) -> crate::suite::Suite {
        crate::suite::Suite {
            name: name.to_string(),
            description: desc.to_string(),
            apps: apps.iter().map(|a| crate::suite::SuiteApp::simple(a, &format!("apps/{a}"))).collect(),
        }
    }
    let coding = ["gitmind", "logscope", "bot-arena"];
    let media = ["av1tuner", "av1tuner-m1", "snapclone", "paperbound", "cybertron", "unendlich"];
    let music = ["beatspace", "krapper", "bo-suite", "fao-codec"];
    let work = ["voiceterm", "ambient-learner", "rusty-board"];
    let all: Vec<&str> = coding.iter().chain(&media).chain(&music).chain(&work).copied().collect();
    // bsettings: crate `bigsilicon-settings` at apps/settings-app, GUI bin needs the `gui` feature.
    // Its path/features differ from the simple apps/<name> convention, so it's a custom entry.
    let bsettings = || crate::suite::SuiteApp {
        name: "bsettings".into(),
        path: "apps/settings-app".into(),
        features: vec!["gui".into()],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
    };
    let mut work_group = group("work", "Productivity, comms & input", &work);
    work_group.apps.push(bsettings());
    let mut all_group = group("bigsilicons-apps", "Every BigSilicon app — build all in one go", &all);
    all_group.apps.push(bsettings());
    vec![
        group("coding", "Development & analysis tools", &coding),
        group("media", "Video, camera, imaging & games", &media),
        group("music", "Audio synthesis, codecs & music", &music),
        work_group,
        all_group,
    ]
}

/// Look up one store group by name (for "build this group").
pub fn group_by_name(name: &str) -> Option<crate::suite::Suite> {
    store_groups().into_iter().find(|g| g.name == name)
}

#[cfg(test)]
mod store_group_tests {
    #[test]
    fn bigsilicons_is_the_union_and_paths_are_app_relative() {
        let groups = super::store_groups();
        let all = super::group_by_name("bigsilicons-apps").expect("all group");
        assert_eq!(all.apps.len(), 17, "every app is in the all group (incl. bsettings)");
        assert!(all.apps.iter().all(|a| a.path.starts_with("apps/")));
        assert!(all.apps.iter().any(|a| a.name == "bsettings" && a.path == "apps/settings-app" && a.features == vec!["gui".to_string()]), "bsettings registered with its real path + gui feature");
        // the four categories partition into the union
        let cat_total: usize = groups.iter().filter(|g| g.name != "bigsilicons-apps").map(|g| g.apps.len()).sum();
        assert_eq!(cat_total, all.apps.len(), "categories cover exactly the all-group");
    }
}

// ── Packaging: build RPM/apk, publish a repo, host it ────────────────────────

use crate::packaging::{self, PkgMeta};

/// Target repository format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RepoFormat {
    Rpm,
    Apk,
}

impl RepoFormat {
    pub fn parse(s: &str) -> Option<RepoFormat> {
        match s.to_lowercase().as_str() {
            "rpm" | "fedora" | "dnf" => Some(RepoFormat::Rpm),
            "apk" | "alpine" | "postmarketos" | "pmos" => Some(RepoFormat::Apk),
            _ => None,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            RepoFormat::Rpm => "rpm",
            RepoFormat::Apk => "apk",
        }
    }
}

/// A staged package tree ready to hand to the native build tool.
#[derive(Debug, Clone)]
pub struct Staged {
    pub work_dir: std::path::PathBuf,
    pub manifest: std::path::PathBuf, // the .spec or APKBUILD
    pub program: String,
    pub args: Vec<String>,
    pub run_in: std::path::PathBuf, // cwd for the build command
}

fn crate_version(toml: &str) -> Option<String> {
    for line in toml.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("version") {
            if let Some(eq) = rest.trim_start().strip_prefix('=') {
                let v = eq.trim().trim_matches('"');
                if !v.is_empty() {
                    return Some(v.to_string());
                }
            }
        }
        if t.starts_with("[") && !t.starts_with("[package]") {
            break; // only the [package] section's version
        }
    }
    None
}

/// Write the spec/APKBUILD + copy the package files into a build tree. Deterministic; does
/// not invoke any native tool. `bin_dir_files` must already exist on disk.
pub fn stage_package(meta: &PkgMeta, format: RepoFormat, out_dir: &std::path::Path) -> Result<Staged, String> {
    match format {
        RepoFormat::Rpm => {
            let top = out_dir.join("rpmbuild");
            for sub in ["SPECS", "SOURCES", "BUILD", "RPMS", "SRPMS"] {
                std::fs::create_dir_all(top.join(sub)).map_err(|e| e.to_string())?;
            }
            // copy sources by basename
            for f in &meta.files {
                let base = f.src.file_name().ok_or("bad source file")?;
                std::fs::copy(&f.src, top.join("SOURCES").join(base)).map_err(|e| format!("stage source: {e}"))?;
            }
            let spec = top.join("SPECS").join(format!("{}.spec", meta.name));
            std::fs::write(&spec, packaging::rpm_spec(meta)).map_err(|e| e.to_string())?;
            let (program, args) = packaging::rpmbuild_cmd(&spec, &top);
            Ok(Staged { work_dir: top.clone(), manifest: spec, program, args, run_in: top })
        }
        RepoFormat::Apk => {
            let dir = out_dir.join("apk").join(&meta.name);
            std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
            for f in &meta.files {
                let base = f.src.file_name().ok_or("bad source file")?;
                std::fs::copy(&f.src, dir.join(base)).map_err(|e| format!("stage source: {e}"))?;
            }
            let manifest = dir.join("APKBUILD");
            std::fs::write(&manifest, packaging::apkbuild(meta)).map_err(|e| e.to_string())?;
            let (program, args) = packaging::abuild_cmd();
            Ok(Staged { work_dir: dir.clone(), manifest, program, args, run_in: dir })
        }
    }
}

/// Outcome of a package build.
#[derive(Debug, Clone)]
pub struct PackageOutcome {
    pub staged: Staged,
    pub artifact: Option<std::path::PathBuf>, // the built .rpm/.apk if the native tool ran
    pub built: bool,
    pub message: String,
}

/// Build a project's binary, stage a package, and run the native packager on-device
/// (best-effort — if the tool is absent, the staged tree + a hint are returned so the user
/// can build it where the tools exist).
pub fn package_project(app_dir: &std::path::Path, format: RepoFormat, out_dir: &std::path::Path) -> Result<PackageOutcome, String> {
    // build the binary (any supported language)
    let detected = crate::buildsystem::detect(app_dir).ok_or("no recognized build system in that directory")?;
    let path_env = crate::ops::tool_path_env();
    let built = crate::buildsystem::build(&detected, "", &[], &path_env)?;
    let binary = built.install;

    // metadata
    let toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let name = crate::buildflags::crate_name(&toml).unwrap_or_else(|| detected.name.clone());
    let version = crate_version(&toml).unwrap_or_else(|| "0.1.0".to_string());
    let arch = packaging::arch_for_triple(&crate::ops::native_triple_local());
    let meta = PkgMeta::new(&name, &version, arch)
        .with_summary(&detected.description)
        .add_binary(&binary, &name);

    let staged = stage_package(&meta, format, out_dir)?;

    // run the native packager
    match std::process::Command::new(&staged.program).args(&staged.args).current_dir(&staged.run_in).output() {
        Ok(o) if o.status.success() => {
            let artifact = newest_artifact(out_dir, format);
            Ok(PackageOutcome { staged, artifact, built: true, message: format!("built {} package for {name} {version}", format.as_str()) })
        }
        Ok(o) => {
            let err = String::from_utf8_lossy(&o.stderr);
            Ok(PackageOutcome { built: false, artifact: None, message: format!("{} failed: {}", staged.program, err.lines().last().unwrap_or("").trim()), staged })
        }
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(PackageOutcome {
            message: format!("'{}' not found — staged the tree at {}; run it on a {} host", staged.program, staged.work_dir.display(), format.as_str()),
            built: false,
            artifact: None,
            staged,
        }),
        Err(e) => Err(format!("running {}: {e}", staged.program)),
    }
}

fn newest_artifact(dir: &std::path::Path, format: RepoFormat) -> Option<std::path::PathBuf> {
    let ext = match format {
        RepoFormat::Rpm => "rpm",
        RepoFormat::Apk => "apk",
    };
    let mut best: Option<(std::time::SystemTime, std::path::PathBuf)> = None;
    fn walk(dir: &std::path::Path, ext: &str, best: &mut Option<(std::time::SystemTime, std::path::PathBuf)>) {
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                if p.is_dir() {
                    walk(&p, ext, best);
                } else if p.extension().and_then(|x| x.to_str()) == Some(ext) {
                    if let Ok(m) = e.metadata().and_then(|m| m.modified()) {
                        if best.as_ref().map(|(t, _)| m > *t).unwrap_or(true) {
                            *best = Some((m, p));
                        }
                    }
                }
            }
        }
    }
    walk(dir, ext, &mut best);
    best.map(|(_, p)| p)
}

/// Publish a built package into a repo dir and regenerate its index (createrepo_c / apk
/// index), best-effort on-device.
pub fn repo_add(pkg: &std::path::Path, repo_dir: &std::path::Path, format: RepoFormat) -> Result<String, String> {
    std::fs::create_dir_all(repo_dir).map_err(|e| e.to_string())?;
    let base = pkg.file_name().ok_or("bad package path")?;
    std::fs::copy(pkg, repo_dir.join(base)).map_err(|e| format!("copy into repo: {e}"))?;
    let (program, args) = match format {
        RepoFormat::Rpm => packaging::createrepo_cmd(repo_dir),
        RepoFormat::Apk => packaging::apk_index_cmd(repo_dir),
    };
    match std::process::Command::new(&program).args(&args).output() {
        Ok(o) if o.status.success() => Ok(format!("added {} and reindexed repo", base.to_string_lossy())),
        Ok(o) => Err(format!("{program} failed: {}", String::from_utf8_lossy(&o.stderr).lines().last().unwrap_or("").trim())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(format!("copied {} into repo — run '{program}' on a {} host to index", base.to_string_lossy(), format.as_str())),
        Err(e) => Err(format!("running {program}: {e}")),
    }
}

/// The client configuration the user applies to install from a gitmind-hosted repo.
pub fn repo_client_config(format: RepoFormat, baseurl: &str, repo_name: &str) -> String {
    match format {
        RepoFormat::Rpm => format!(
            "# save as /etc/yum.repos.d/{repo_name}.repo, then: sudo dnf install <pkg>\n{}",
            packaging::dnf_repo_file(repo_name, baseurl)
        ),
        RepoFormat::Apk => format!(
            "# add the repo (unsigned): \n{}\n# or one-shot: {}",
            format!("echo {} | sudo tee -a /etc/apk/repositories", packaging::apk_repo_line(baseurl)),
            packaging::apk_add_cmd(baseurl, "<pkg>")
        ),
    }
}

/// Serve a repo directory over HTTP so dnf/apk can fetch from it. Minimal GET-only server;
/// blocks. `port` 0 lets the OS choose (returns the bound addr via the callback).
pub fn serve_repo(repo_dir: &std::path::Path, port: u16) -> Result<(), String> {
    use std::io::{Read, Write};
    let listener = std::net::TcpListener::bind(("0.0.0.0", port)).map_err(|e| e.to_string())?;
    let root = repo_dir.to_path_buf();
    for stream in listener.incoming() {
        let Ok(mut s) = stream else { continue };
        let root = root.clone();
        std::thread::spawn(move || {
            let mut buf = [0u8; 2048];
            let n = s.read(&mut buf).unwrap_or(0);
            let req = String::from_utf8_lossy(&buf[..n]);
            let target = req.split_whitespace().nth(1).unwrap_or("/");
            let resp = match packaging::resolve_in_root(&root, target) {
                Some(path) if path.is_file() => match std::fs::read(&path) {
                    Ok(body) => {
                        let ct = packaging::content_type(&path);
                        let mut head = format!("HTTP/1.0 200 OK\r\nContent-Type: {ct}\r\nContent-Length: {}\r\n\r\n", body.len()).into_bytes();
                        head.extend_from_slice(&body);
                        head
                    }
                    Err(_) => b"HTTP/1.0 500 Internal Server Error\r\n\r\n".to_vec(),
                },
                _ => b"HTTP/1.0 404 Not Found\r\n\r\n".to_vec(),
            };
            let _ = s.write_all(&resp);
        });
    }
    Ok(())
}

#[cfg(test)]
mod packaging_flow_tests {
    use super::*;
    use crate::packaging::PkgMeta;

    #[test]
    fn crate_version_reads_package_section() {
        assert_eq!(crate_version("[package]\nname = \"x\"\nversion = \"1.2.3\"\n").as_deref(), Some("1.2.3"));
        // version in a later section is ignored
        assert_eq!(crate_version("[package]\nname = \"x\"\n[dependencies]\nfoo = { version = \"9\" }\n"), None);
    }

    #[test]
    fn repo_client_config_includes_baseurl() {
        assert!(repo_client_config(RepoFormat::Rpm, "http://host:8080/", "fleet").contains("baseurl=http://host:8080/"));
        assert!(repo_client_config(RepoFormat::Apk, "http://host:8080/", "fleet").contains("http://host:8080"));
    }

    #[test]
    fn stage_rpm_and_apk_write_manifest_and_sources() {
        let base = std::env::temp_dir().join(format!("gmpkg-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        std::fs::create_dir_all(&base).unwrap();
        // a fake built binary
        let bin = base.join("gitmind");
        std::fs::write(&bin, b"\x7fELF fake").unwrap();
        let meta = PkgMeta::new("gitmind", "0.3.0", "aarch64").add_binary(&bin, "gitmind");

        // RPM
        let rpm_out = base.join("rpm");
        let staged = stage_package(&meta, RepoFormat::Rpm, &rpm_out).unwrap();
        assert!(staged.manifest.ends_with("SPECS/gitmind.spec"));
        assert!(std::fs::read_to_string(&staged.manifest).unwrap().contains("Version:        0.3.0"));
        assert!(rpm_out.join("rpmbuild/SOURCES/gitmind").is_file(), "binary staged into SOURCES");
        assert_eq!(staged.program, "rpmbuild");

        // APK
        let apk_out = base.join("apk");
        let staged = stage_package(&meta, RepoFormat::Apk, &apk_out).unwrap();
        assert!(staged.manifest.ends_with("APKBUILD"));
        assert!(std::fs::read_to_string(&staged.manifest).unwrap().contains("pkgver=0.3.0"));
        assert!(apk_out.join("apk/gitmind/gitmind").is_file(), "binary staged next to APKBUILD");
        assert_eq!(staged.program, "abuild");

        let _ = std::fs::remove_dir_all(&base);
    }
}

/// Guess the repo format for the current host from /etc/os-release (Alpine/postmarketOS →
/// apk, otherwise rpm). Used as the default when the user doesn't pass --rpm/--apk.
pub fn host_repo_format() -> RepoFormat {
    let os = std::fs::read_to_string("/etc/os-release").unwrap_or_default().to_lowercase();
    if os.contains("alpine") || os.contains("postmarketos") {
        RepoFormat::Apk
    } else {
        RepoFormat::Rpm
    }
}

// ── DHT rendezvous: reach fleet devices across networks ──────────────────────

use crate::rendezvous::{self, Advert, AdvertStore};

fn hostname() -> String {
    std::fs::read_to_string("/etc/hostname")
        .ok()
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty())
        .or_else(|| std::env::var("HOSTNAME").ok())
        .unwrap_or_else(|| "device".to_string())
}

/// This device's fleet account, derived from its ssh identity public key.
pub fn self_account() -> Option<String> {
    let pubkey = crate::deploy::local_identity_pubkey()?;
    Some(rendezvous::account_id(pubkey.as_bytes()))
}

/// The default advert store: a file drop under the gitmind config dir. This works as a
/// local cache and over a shared mount; on-device the snapcore DHT store is primary (it
/// implements the same `AdvertStore` seam via announce/discover).
pub fn default_advert_store() -> rendezvous::FileStore {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    rendezvous::FileStore::new(format!("{home}/.config/gitmind/rendezvous"))
}

/// Build this device's advertisement from locally-known addresses.
pub fn build_self_advert(
    ssh_port: u16,
    public: Option<(String, u16)>,
    relay: Option<(String, u16)>,
    ttl_secs: u64,
) -> Option<Advert> {
    let account = self_account()?;
    let user = crate::netshare::current_user();
    let host = hostname();
    let lan = crate::netshare::local_ip().map(|ip| (ip, ssh_port));
    Some(rendezvous::build_local_advert(
        &account,
        &user,
        &host,
        lan.as_ref().map(|(h, p)| (h.as_str(), *p)),
        public.as_ref().map(|(h, p)| (h.as_str(), *p)),
        relay.as_ref().map(|(h, p)| (h.as_str(), *p)),
        ttl_secs,
    ))
}

/// Publish our advert to the DHT/store so others can reach us off-LAN.
pub fn advertise(store: &mut dyn AdvertStore, advert: &Advert) -> Result<(), String> {
    rendezvous::publish(store, advert)
}

/// Resolve a peer account to its advert.
pub fn resolve_peer(store: &dyn AdvertStore, account: &str) -> Option<Advert> {
    rendezvous::resolve(store, account)
}

/// Resolve + pick the best `(user, host, port)` to reach a peer from here (LAN if same
/// subnet, else public/NAT, else relay).
pub fn reach(store: &dyn AdvertStore, account: &str) -> Option<(String, String, u16)> {
    let advert = rendezvous::resolve(store, account)?;
    rendezvous::best_target(&advert, crate::netshare::local_ip().as_deref())
}

// ── Downloads scanning + bundle import ──

/// The user's Downloads folder.
pub fn downloads_dir() -> PathBuf {
    let home = std::env::var("HOME").unwrap_or_else(|_| ".".into());
    PathBuf::from(format!("{home}/Downloads"))
}

fn installed_names() -> std::collections::BTreeSet<String> {
    gitmind_core::binpin::list_apps().into_iter().collect()
}

/// Load a suite bundle: either a suite JSON directly, or a .zip containing a
/// `gitmind-suite.json` manifest (a portable bundle — the app paths are resolved inside
/// the extracted copy so the bundled code is what gets built).
pub fn import_bundle(path: &str) -> Result<crate::suite::Suite, String> {
    let path = path.strip_prefix("file://").unwrap_or(path);
    if path.ends_with(".json") {
        let text = std::fs::read_to_string(path).map_err(|e| format!("read: {e}"))?;
        return crate::suite::Suite::parse(&text).ok_or_else(|| "that JSON isn't a gitmind suite".to_string());
    }
    // a zip bundle: extract and find the manifest
    let dir = importer::import_dir();
    let script = importer::extract_command(path, &dir.display().to_string());
    let out = std::process::Command::new("sh").arg("-c").arg(&script)
        .env("PATH", tool_path_env()).output().map_err(|e| format!("extract: {e}"))?;
    if !out.status.success() {
        return Err("couldn't extract the bundle".into());
    }
    let manifest = find_shallowest(&dir, &["gitmind-suite.json", "suite.json"])
        .ok_or("no gitmind-suite.json found in the bundle")?;
    let mut suite = crate::suite::Suite::parse(&std::fs::read_to_string(&manifest).unwrap_or_default())
        .ok_or("the bundle manifest is malformed")?;
    let base = manifest.parent().unwrap_or(&dir).to_path_buf();
    for app in &mut suite.apps {
        let p = Path::new(&app.path);
        if p.is_relative() { app.path = base.join(p).display().to_string(); }
    }
    Ok(suite)
}

fn find_shallowest(dir: &Path, names: &[&str]) -> Option<PathBuf> {
    fn walk(dir: &Path, names: &[&str], depth: u32) -> Option<PathBuf> {
        if depth > 4 { return None; }
        for n in names { let c = dir.join(n); if c.is_file() { return Some(c); } }
        let mut subs: Vec<PathBuf> = std::fs::read_dir(dir).ok()?.flatten()
            .filter(|e| e.file_type().map(|t| t.is_dir()).unwrap_or(false)).map(|e| e.path()).collect();
        subs.sort();
        for s in subs { if let Some(f) = walk(&s, names, depth + 1) { return Some(f); } }
        None
    }
    walk(dir, names, 0)
}

/// A .zip or bundle sitting in Downloads that can be installed into the apps list.
pub struct DownloadItem {
    pub path: String,
    pub name: String,
    pub kind: String,   // "app" | "bundle"
    pub apps: usize,    // for bundles
    pub installed: bool,
}

/// Scan a folder (top level) for importable zips and suite bundles.
pub fn scan_downloads(dir: &Path) -> Vec<DownloadItem> {
    let installed = installed_names();
    let mut items = Vec::new();
    let Ok(rd) = std::fs::read_dir(dir) else { return items };
    for e in rd.flatten() {
        let p = e.path();
        let ext = p.extension().and_then(|x| x.to_str()).unwrap_or("").to_lowercase();
        let stem = p.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        if ext == "zip" {
            if let Some(s) = zip_bundle_manifest(&p) {
                items.push(DownloadItem { path: p.display().to_string(), name: s.0, kind: "bundle".into(), apps: s.1, installed: false });
            } else {
                items.push(DownloadItem { path: p.display().to_string(), name: stem.clone(), kind: "app".into(), apps: 0, installed: installed.contains(&stem) });
            }
        } else if ext == "json" {
            if let Ok(t) = std::fs::read_to_string(&p) {
                if let Some(s) = crate::suite::Suite::parse(&t) {
                    items.push(DownloadItem { path: p.display().to_string(), name: s.name.clone(), kind: "bundle".into(), apps: s.apps.len(), installed: false });
                }
            }
        }
    }
    items.sort_by(|a, b| a.name.cmp(&b.name));
    items
}

/// Cheaply peek whether a zip is a bundle (contains a suite manifest) via `unzip -l`.
/// Returns (suite name best-effort, app count) if so.
fn zip_bundle_manifest(zip: &Path) -> Option<(String, usize)> {
    let out = std::process::Command::new("unzip").arg("-l").arg(zip)
        .env("PATH", tool_path_env()).output().ok()?;
    if !out.status.success() { return None; }
    let listing = String::from_utf8_lossy(&out.stdout);
    if listing.contains("gitmind-suite.json") || listing.contains("suite.json") {
        let stem = zip.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_default();
        Some((stem, 0))
    } else {
        None
    }
}

/// Import a zipped app and add it to the local apps list (installs a new version — an
/// update — if an app of the same name already exists).
pub fn install_zip_app(zip: &str) -> Result<(Installed, bool), String> {
    let imp = import(zip)?;
    let was_installed = installed_names().contains(&imp.app);
    let inst = build_and_add(&imp.crate_path, &imp.app, "")?;
    Ok((inst, was_installed))
}

// ── daemon lifecycle (install + status), init-system aware ───────────────────

/// Detect the target's init system from its os-release over SSH.
pub fn detect_init_remote(user: &str, host: &str) -> crate::daemons::InitSystem {
    let os = crate::deploy::remote_capture(user, host, "cat /etc/os-release 2>/dev/null || true").unwrap_or_default();
    crate::daemons::InitSystem::detect_from_os_release(&os)
}

/// Install + enable every gitmind daemon whose binary is present on the target, writing the
/// right service units for its init system (systemd user units on desktop, OpenRC on the phone).
/// Graceful: missing daemon binaries are skipped, so a partial deploy still starts what it can.
pub fn install_daemons_remote(user: &str, host: &str, bin_dir: &str, home: &str) -> Result<String, String> {
    let init = detect_init_remote(user, host);
    let script = crate::daemons::install_daemons_script(bin_dir, home, init);
    crate::deploy::remote_capture(user, host, &script)
}

/// A one-line status per daemon on the target (active / inactive), init-system aware.
pub fn daemons_status_remote(user: &str, host: &str) -> Result<String, String> {
    let init = detect_init_remote(user, host);
    let names: Vec<&str> = crate::daemons::all().iter().map(|d| d.name).collect();
    let cmd = match init {
        crate::daemons::InitSystem::Systemd => format!(
            "for s in {}; do printf '%s: ' \"$s\"; systemctl --user is-active \"$s\" 2>/dev/null || echo inactive; done",
            names.join(" ")
        ),
        crate::daemons::InitSystem::OpenRc => format!(
            "for s in {}; do printf '%s: ' \"$s\"; rc-service \"$s\" status >/dev/null 2>&1 && echo started || echo stopped; done",
            names.join(" ")
        ),
    };
    crate::deploy::remote_capture(user, host, &cmd)
}

#[cfg(test)]
mod daemon_flow_tests {
    #[test]
    fn status_script_lists_all_daemons() {
        // (pure check of the naming that feeds the remote command)
        let names: Vec<&str> = crate::daemons::all().iter().map(|d| d.name).collect();
        assert!(names.contains(&"settingsd") && names.contains(&"voiced"));
    }
}

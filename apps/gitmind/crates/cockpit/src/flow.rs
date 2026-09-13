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

/// The launch flags for a local app: the user's saved ones, or the built-in
/// default for apps that need one to open a window at all.
///
/// The saved value WINS when present, because a user who set flags meant them —
/// but an app nobody has configured must still start. beatspace is the case:
/// `gui` is a required subcommand alongside `train` and `gpu`, so launching it
/// bare prints usage and exits, and the run button appears to do nothing.
///
/// Shares its table with `binpin::gui_launch_args`, which does the same job for
/// the .desktop file. Two launch paths, one answer — and they were allowed to
/// disagree until now: the desktop entry knew about `gui` and gitmind's own run
/// button did not.
pub fn run_args(name: &str) -> String {
    let saved = run_args_path()
        .and_then(|p| std::fs::read_to_string(&p).ok())
        .and_then(|text| serde_json::from_str::<serde_json::Value>(&text).ok())
        .and_then(|v| v.get(name).and_then(|x| x.as_str()).map(|s| s.to_string()))
        .unwrap_or_default();
    if !saved.trim().is_empty() {
        return saved;
    }
    gitmind_core::binpin::gui_launch_args(name).join(" ")
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
/// Record launch flags for an app WITHOUT clobbering flags the user chose.
///
/// `add_to_apps` runs on every install and reinstall, and it passes the
/// catalog's `run_flags` — which is empty for almost every app. Writing that
/// empty string through would erase whatever the user had configured, so
/// reinstalling an app would silently reset its launch flags.
///
/// An empty value therefore means "I have nothing to say", not "set it to
/// nothing". Clearing flags deliberately is `set_run_args`, which is still
/// available and still writes exactly what it is given.
fn record_run_args(name: &str, args: &str) -> Result<(), String> {
    if args.trim().is_empty() {
        return Ok(());
    }
    set_run_args(name, args)
}

pub fn add_to_apps(bin: &Path, name: &str, commit: &str, run_args: &str) -> Result<Installed, String> {
    let bytes = std::fs::read(bin).map_err(|e| format!("read binary: {e}"))?;
    let store = gitmind_core::varcache::open_var_store()?;
    let label = format!("import-{}", crate::ops::now_stamp());
    let commit = if commit.trim().is_empty() { "import" } else { commit };
    let v = gitmind_core::binpin::install(&store, name, &label, commit, "host", &bytes, true)?;
    let _ = gitmind_core::binpin::write_desktop(name, &label, false, None);
    record_run_args(name, run_args)?;
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
/// Near-duplicate blobs, by shared chunks rather than by identity.
///
/// `duplicates_all` groups by an identity KEY, so it finds files that are
/// the same file. This finds files that are ALMOST the same one — a JPEG
/// re-saved by an editor, a losslessly retranscoded photo — which share
/// most of their content-defined chunks without matching byte for byte.
///
/// `gitmind_core::similarity` has done this arithmetic since it was
/// written and nothing called it, so the dupes page could only ever
/// offer exact matches. The 99%-matching feature its own header
/// describes was unreachable.
///
/// O(n²) over blob pairs, deliberately. The alternative is an index over
/// chunk membership, which is the right answer at a scale this store has
/// not reached — and building it first would be optimising a feature
/// nobody has used yet. The cap makes the cost bounded and visible
/// rather than surprising.
pub fn near_duplicates(
    store: &gitmind_core::store::ChunkStore,
    threshold: f64,
    max_blobs: usize,
) -> Result<Vec<(String, String, f64)>, String> {
    let blobs = store.list_blobs()?;
    // Truncated rather than refused: a partial answer over the first N
    // blobs is useful, and a scan that declines on a large store is a
    // feature that stops working exactly when it starts mattering.
    let blobs: Vec<_> = blobs.into_iter().take(max_blobs).collect();
    let mut out = Vec::new();
    for (i, (oid_a, ma)) in blobs.iter().enumerate() {
        for (oid_b, mb) in blobs.iter().skip(i + 1) {
            // Skip identical blobs: the same oid twice is not a near
            // duplicate, it is the same object, and `duplicates_all`
            // already reports that case.
            if oid_a == oid_b {
                continue;
            }
            let s = gitmind_core::similarity::manifest_similarity(ma, mb);
            if s >= threshold {
                out.push((format!("{oid_a:?}"), format!("{oid_b:?}"), s));
            }
        }
    }
    // Most similar first: the pairs a person acts on are at the top, and
    // a list ordered by store position buries them.
    out.sort_by(|a, b| b.2.partial_cmp(&a.2).unwrap_or(std::cmp::Ordering::Equal));
    Ok(out)
}

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

    #[test]
    fn unendlich_builds_the_real_fullgame_binary_not_the_bare_app_name() {
        // the exact bug report: `cargo build --release --bin unendlich` fails because unendlich is
        // a workspace root with no [package] of its own — the real game bin is `unendlich-fullgame`,
        // gated behind render/window/ui-overlay, alongside several benchmark/probe bins that don't
        // need those features. Every store group that includes unendlich must carry the override.
        for group_name in ["media", "bigsilicons-apps"] {
            let group = store_groups().into_iter().find(|g| g.name == group_name).unwrap_or_else(|| panic!("missing group {group_name}"));
            let u = group.apps.iter().find(|a| a.name == "unendlich").unwrap_or_else(|| panic!("{group_name} is missing unendlich"));
            assert_eq!(u.build_app_id(), "unendlich-fullgame");
            for feat in ["render", "window", "ui-overlay"] {
                assert!(u.features.iter().any(|f| f == feat), "{group_name}'s unendlich entry is missing the {feat} feature: {:?}", u.features);
            }
        }
        // and it must NOT also appear as a bare simple() entry anywhere (that would silently
        // reintroduce the bug — the plain string list must not carry "unendlich" alongside the
        // custom push)
        for group in store_groups() {
            let hits: Vec<_> = group.apps.iter().filter(|a| a.name == "unendlich").collect();
            assert!(hits.len() <= 1, "{} has unendlich listed more than once: {:?}", group.name, hits);
        }
    }

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
/// Run one app's test suite, with the same flags its build uses.
///
/// Cargo only. A CMake or Python app has no `cargo test` to run and saying so
/// is more useful than inventing a runner for it — the alternative is a green
/// tick from a command that did nothing, which is the failure mode the
/// bsettings "0 KB" build taught.
pub fn test_suite_app_logged(app: &crate::suite::SuiteApp, on_line: &mut dyn FnMut(&str)) -> Result<String, String> {
    use std::io::{BufRead, BufReader};
    let app_dir = crate::ops::resolve_app_dir(&app.path);
    if !app_dir.join("Cargo.toml").exists() {
        return Err(format!("{} is not a cargo crate — nothing to `cargo test`", app.name));
    }
    let argv = crate::buildflags::cargo_test_argv(
        app.build_app_id(), &app.target, &app.features, app.no_default);
    on_line(&format!("$ {}", argv.join(" ")));

    let mut child = std::process::Command::new(&argv[0])
        .args(&argv[1..])
        .current_dir(&app_dir)
        .env("PATH", crate::ops::tool_path_env())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("couldn't run the tests: {e}"))?;

    // Cargo writes progress to stderr and test results to STDOUT, and the
    // result lines are the ones that matter here — draining only stderr would
    // show the compile and hide every "test result: ok".
    let mut passed = 0usize;
    if let Some(out) = child.stdout.take() {
        for line in BufReader::new(out).lines().map_while(Result::ok) {
            if let Some(rest) = line.strip_prefix("test result: ok. ") {
                if let Some(n) = rest.split(' ').next().and_then(|n| n.parse::<usize>().ok()) {
                    passed += n;
                }
            }
            on_line(&line);
        }
    }
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            on_line(&line);
        }
    }
    let status = child.wait().map_err(|e| format!("test wait failed: {e}"))?;
    if status.success() {
        Ok(format!("✓ {} — {passed} tests passed", app.name))
    } else {
        Err(format!("{} tests FAILED (exit {})", app.name, status.code().unwrap_or(-1)))
    }
}

/// Build every app in the catalog. Returns (succeeded, failed names).
///
/// Keeps going past a failure rather than stopping at the first: the point of
/// a whole-fleet run is to learn what is broken, and stopping tells you only
/// about the first one. Both counts come back so a caller cannot report
/// success while apps are failing.
pub fn build_all_logged(on_line: &mut dyn FnMut(&str)) -> (usize, Vec<String>) {
    let mut ok = 0usize;
    let mut failed = Vec::new();
    for app in catalog_apps() {
        on_line(&format!("── {} ──", app.name));
        match install_suite_app_logged(&app, on_line) {
            Ok(i) => { ok += 1; on_line(&format!("✓ {} → {}", i.name, i.path)); }
            Err(e) => { on_line(&format!("✗ {}: {e}", app.name)); failed.push(app.name.clone()); }
        }
    }
    (ok, failed)
}

/// Test every app in the catalog. Returns (green, failed names).
pub fn test_all_logged(on_line: &mut dyn FnMut(&str)) -> (usize, Vec<String>) {
    let mut ok = 0usize;
    let mut failed = Vec::new();
    for app in catalog_apps() {
        on_line(&format!("── {} ──", app.name));
        match test_suite_app_logged(&app, on_line) {
            Ok(msg) => { ok += 1; on_line(&msg); }
            Err(e) => { on_line(&format!("✗ {e}")); failed.push(app.name.clone()); }
        }
    }
    (ok, failed)
}

/// Every app the catalog shows, de-duplicated.
///
/// Drawn from `store_groups()` — the same list the Fleet page renders — so
/// "all" cannot mean something different from what is on screen. An app in two
/// groups is built once.
pub fn catalog_apps() -> Vec<crate::suite::SuiteApp> {
    let mut seen = std::collections::HashSet::new();
    let mut out = Vec::new();
    for g in store_groups() {
        for a in g.apps {
            if seen.insert(a.name.clone()) {
                out.push(a);
            }
        }
    }
    out
}

/// Install, stopping early if `cancel` is set.
///
/// Only the CARGO path is cancellable. A CMake or Python build goes through
/// `buildsystem`, which shells out differently — offering a cancel button that
/// silently does nothing there would be worse than not offering one, so those
/// fall through to the uncancellable path and the caller's button simply has no
/// effect it can promise.
pub fn install_suite_app_cancellable(
    app: &crate::suite::SuiteApp,
    on_line: &mut dyn FnMut(&str),
    cancel: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
) -> Result<Installed, String> {
    CANCEL.with(|c| *c.borrow_mut() = cancel);
    let r = install_suite_app_logged(app, on_line);
    CANCEL.with(|c| *c.borrow_mut() = None);
    r
}

thread_local! {
    /// The cancel flag for the build running on THIS thread.
    ///
    /// A thread-local rather than a parameter threaded through six functions:
    /// install_suite_app_logged dispatches across cargo/cmake/python/make/go
    /// and each has its own call chain, so widening all of them to carry an
    /// Option nobody but the cargo path reads would be churn for no gain. The
    /// task runs one build per thread, so the scope is exactly right.
    static CANCEL: std::cell::RefCell<Option<std::sync::Arc<std::sync::atomic::AtomicBool>>> =
        const { std::cell::RefCell::new(None) };
}

/// The cancel flag for the build on this thread, if one was set.
pub fn thread_cancel_flag() -> Option<std::sync::Arc<std::sync::atomic::AtomicBool>> {
    CANCEL.with(|c| c.borrow().clone())
}

pub fn install_suite_app_logged(app: &crate::suite::SuiteApp, on_line: &mut dyn FnMut(&str)) -> Result<Installed, String> {
    let target = if app.target.trim().is_empty() { native_triple_local() } else { app.target.clone() };
    let app_dir = crate::ops::resolve_app_dir(&app.path);

    // Detect the build system. Cargo keeps the rich cross-target/rustup path; CMake / Python / Make /
    // Go / Node build through the general buildsystem (so a CMake app like snapclone or a Python game
    // installs locally instead of failing with "no Cargo.toml"). System deps (cmake, python3+venv) are
    // ensured for the host's package manager — apk on Alpine, dnf on Fedora — before the build.
    let detected = crate::buildsystem::detect(&app_dir);
    let is_cargo = matches!(detected.as_ref().map(|d| &d.kind), Some(crate::buildsystem::Kind::Cargo) | None);

    if is_cargo {
        let (bin, _) = crate::ops::build_locally_cancellable(
            app.build_app_id(),
            &app_dir,
            &target,
            &app.features,
            app.no_default,
            on_line,
            // The flag set by install_suite_app_cancellable, if the caller
            // offered one. None here is the uncancellable path, unchanged.
            thread_cancel_flag(),
        )
        .map_err(|e| format!("build: {e}"))?;
        add_to_apps(&bin, &app.name, "suite", &app.run_flags)
    } else {
        let d = detected.expect("non-cargo kind implies a Detected");
        on_line(&format!("build system: {}", d.kind.label()));
        let path_env = crate::ops::tool_path_env();
        crate::sysdeps::ensure_build_system_deps(&d.kind, &path_env, on_line);
        let built = crate::buildsystem::build(&d, &target, &app.features, &path_env)
            .map_err(|e| format!("build: {e}"))?;
        on_line(&format!("built {} ({} bytes)", built.install.display(), built.bytes));
        add_to_apps(&built.install, &app.name, "suite", &app.run_flags)
    }
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
    // Store the bare app/game NAME; `ops::resolve_app_dir` discovers whether it lives under apps/ or
    // games/ at build time, so no path tree is hard-coded here.
    fn group(name: &str, desc: &str, apps: &[&str]) -> crate::suite::Suite {
        crate::suite::Suite {
            name: name.to_string(),
            description: desc.to_string(),
            apps: apps.iter().map(|a| crate::suite::SuiteApp::simple(a, a)).collect(),
        }
    }
    let coding = ["gitmind", "logscope", "bot-arena"];
    // snapshot-cam is a simple entry: bare name, package == bin, no features.
    // photosync joins media as a SIMPLE entry: unlike cadscope and
    // aperture it is a plain package at apps/photosync, so
    // `resolve_app_dir` finds it without a custom path.
    // av1tuner-m1 was HERE until now: the fork was merged into apps/av1tuner and
    // retired in c11 (see apps/av1tuner/DUPLICATE.md), the directory is gone,
    // and the manifest entry carries a RESOLVED comment — but this list still
    // offered it, so it resolved to nothing and failed on every device it was
    // installed to. Exactly the phantom entry the comment below says was
    // refused when the source was missing; nothing noticed when one appeared by
    // deletion instead. `every_store_entry_resolves_to_a_real_directory` is now
    // the thing that notices.
    // vidkit-player: a plain package at apps/vidkit-player, so `resolve_app_dir`
    // finds it from the bare name. Registering it in manifest/projects.toml is
    // NOT enough to reach this list -- the manifest feeds the catalogue parser
    // and build/build.sh, while THIS is what the fleet store offers -- which is
    // why it was buildable from the command line and invisible in the GUI. See
    // `every_registered_app_is_offered_in_the_store`.
    let media = ["av1tuner", "snapclone", "paperbound", "snapshot-cam",
                 "photosync", "vidkit-player"];
    // beatspace is a CUSTOM entry below (it needs run_flags), so it is not in
    // this simple list — being in both would put it in the group twice.
    let music = ["krapper", "bo-suite", "fao-codec"];
    // ambient-learner is a CUSTOM entry below: the manifest marks
    // `ambient-learner-egui` primary, and a bare name here resolves to
    // `apps/ambient-learner` — the ICED parent, which is not what the fleet
    // ships. Being in both lists would offer it twice.
    let work = ["voiceterm", "rusty-board"];
    let all: Vec<&str> = coding.iter().chain(&media).chain(&music).chain(&work).copied().collect();
    // bsettings: crate `bigsilicon-settings` at apps/settings-app, GUI bin needs the `gui` feature.
    // Its path/features differ from the simple apps/<name> convention, so it's a custom entry.
    // cadscope / simscope: grafted from a fork of this monorepo (C12-74).
    // CUSTOM entries, because neither is a package at `apps/<name>` the
    // way the simple list assumes — each is a WORKSPACE whose runnable
    // crate sits under `crates/`. Listing the bare name would build
    // nothing and fail on every device with a confusing "no such
    // package", which is the phantom-entry failure I refused to ship
    // when the source was missing.
    // aperture: same shape as cadscope — a WORKSPACE whose runnable crate
    // is `crates/aperture-egui`, producing bin `aperture`. The bare name
    // in the simple list would look for a package at apps/aperture and
    // find a workspace manifest with no bin.
    let aperture = || crate::suite::SuiteApp {
        name: "aperture".into(),
        path: "apps/aperture/crates/aperture-egui".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("aperture".into()),
    };
    let cadscope = || crate::suite::SuiteApp {
        name: "cadscope".into(),
        path: "apps/cadscope/crates/cadscope-egui".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("cadscope".into()),
    };
    let simscope = || crate::suite::SuiteApp {
        name: "simscope".into(),
        path: "apps/simscope/crates/simscope-cli".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("simscope".into()),
    };
    let bsettings = || crate::suite::SuiteApp {
        name: "bsettings".into(),
        path: "apps/settings-app".into(),
        features: vec!["gui".into()],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: None,
    };
    // studio: the video NLE + Audacity-informed audio editor. Package
    // `studio-app` at apps/studio/crates/studio-app, bin `studio` — the same
    // package-is-not-the-binary shape as bsettings, and the shape that produced
    // the "✓ built (0 KB)" report when resolve_binary looked for the wrong
    // name.
    //
    // bin_name is set EXPLICITLY rather than left to default: the default is
    // the entry's `name`, which is "studio" and happens to be right — but only
    // by coincidence, since the crate is studio-app. Writing it down means a
    // later rename of either does not silently break the lookup.
    // beatspace needs `gui` to open a window: it is a required subcommand, so a
    // bare launch prints usage and exits. run_args supplies it for the run
    // button and gui_launch_args for the .desktop file — this sets run_flags so
    // the value is also recorded in the app entry written at install time,
    // which is the third place a launch can originate.
    let beatspace = || crate::suite::SuiteApp {
        name: "beatspace".into(),
        // BARE NAME, like every other simple app: discovery resolves apps/ vs
        // games/ at build time. My first version wrote "apps/beatspace", which
        // the path invariant caught — it works, but it hardcodes a layout the
        // rest of the catalog deliberately does not assume.
        path: "beatspace".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: "gui".into(),
        bin_name: None,
    };
    let studio = || crate::suite::SuiteApp {
        name: "studio".into(),
        path: "apps/studio/crates/studio-app".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("studio".into()),
    };
    // unendlich is a workspace root with no [package] of its own — several benchmark/probe bins
    // (perf-probe, gpu-mesh-bench, octree-bench, …) live alongside the real game, which is its own
    // differently-named bin (`unendlich-fullgame`) gated behind render/window/ui-overlay. Without
    // `bin_name` here, the simple-entry default (`--bin unendlich`) matches NONE of the workspace's
    // actual bin targets — exactly the reported "no bin target named `unendlich`" error.
    // The manifest says `primary = true` on `ambient-learner-egui`, meaning that
    // port IS the app now and the iced parent is the alternative. A bare
    // `"ambient-learner"` in the simple list builds the parent, because
    // `resolve_app_dir` maps a bare name to `apps/<name>` — so the store was
    // offering the thing the manifest calls secondary.
    //
    // Custom rather than renaming the list entry: the display name stays
    // `ambient-learner` (what the user looks for) while the PATH points at the
    // port. Renaming to `ambient-learner-egui` would resolve to no directory at
    // all, since the port lives at `apps/ambient-learner/egui-frontend`.
    //
    // Nine other simple-list entries have the same divergence; see
    // `store_entries_build_the_manifests_primary_port`, which lists them rather
    // than letting them be discovered one report at a time.
    let ambient_learner = || crate::suite::SuiteApp {
        name: "ambient-learner".into(),
        path: "apps/ambient-learner/egui-frontend".into(),
        features: vec![],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("ambient-learner-egui".into()),
    };
    // FIVE APPS THE STORE DID NOT OFFER. `manifest_primary_apps_that_the_store
    // _does_not_offer` has reported them for a while and nobody read the
    // output; the operator noticed the store was short by comparing it against
    // `ls apps/`. Each is custom rather than a simple-list entry because none
    // is a package at `apps/<n>`:
    //
    //   hatch, drivemind, switcheroo, dialer — workspaces whose runnable crate
    //   is a member, the same shape as aperture and cadscope.
    //   bscale — a workspace AND it needs `gpu,app`; the bin is a member
    //   with the same name as the workspace, which is what makes a bare entry
    //   look plausible and build nothing.
    let hatch = || crate::suite::SuiteApp {
        name: "hatch".into(),
        path: "apps/hatch/egui-frontend".into(),
        features: vec![], no_default: false, target: String::new(), run_flags: String::new(),
        bin_name: Some("hatch-egui".into()),
    };
    let drivemind = || crate::suite::SuiteApp {
        name: "drivemind".into(),
        path: "apps/drivemind/crates/gui-egui".into(),
        features: vec![], no_default: false, target: String::new(), run_flags: String::new(),
        bin_name: Some("drivemind-egui".into()),
    };
    let switcheroo = || crate::suite::SuiteApp {
        name: "switcheroo".into(),
        path: "apps/switcheroo/crates/switcheroo-egui".into(),
        features: vec![], no_default: false, target: String::new(), run_flags: String::new(),
        // The crate is `switcheroo-egui`; its bin is `switcheroo`.
        bin_name: Some("switcheroo".into()),
    };
    let bscale = || crate::suite::SuiteApp {
        name: "bscale".into(),
        path: "apps/bscale/crates/bscale".into(),
        // Its own default is `gpu,app`, but `no_default` elsewhere in this
        // store means a caller can end up with neither; naming them is what
        // the operator's own build line does.
        features: vec!["gpu".into(), "app".into()],
        no_default: false, target: String::new(), run_flags: String::new(),
        bin_name: Some("bscale".into()),
    };
    let dialer = || crate::suite::SuiteApp {
        name: "dialer".into(),
        path: "apps/dialer/crates/dialer-egui".into(),
        features: vec![], no_default: false, target: String::new(), run_flags: String::new(),
        bin_name: Some("dialer-egui".into()),
    };
    let unendlich = || crate::suite::SuiteApp {
        name: "unendlich".into(),
        path: "unendlich".into(),
        features: vec!["render".into(), "window".into(), "ui-overlay".into()],
        no_default: false,
        target: String::new(),
        run_flags: String::new(),
        bin_name: Some("unendlich-fullgame".into()),
    };
    let mut work_group = group("work", "Productivity, comms & input", &work);
    work_group.apps.push(bsettings());
    work_group.apps.push(ambient_learner());
    work_group.apps.push(dialer());
    work_group.apps.push(switcheroo());
    work_group.apps.push(hatch());
    work_group.apps.push(drivemind());
    // clawbound-battleground: a plain package at games/clawbound-battleground
    // whose package and bin share the name, so the simple shape is exact;
    // discovery resolves games/ at build time. Listed as a custom push rather
    // than in `media` above because it must not join `all` twice.
    let battleground = || crate::suite::SuiteApp::simple("clawbound-battleground", "clawbound-battleground");
    let mut media_group = group("media", "Video, camera, imaging & games", &media);
    // Media rather than music: it is a video NLE first, with the audio editor
    // sharing its core — the README's own framing.
    media_group.apps.push(studio());
    media_group.apps.push(unendlich());
    media_group.apps.push(battleground());
    media_group.apps.push(bscale());
    let mut all_group = group("bigsilicons-apps", "Every BigSilicon app — build all in one go", &all);
    all_group.apps.push(bsettings());
    all_group.apps.push(ambient_learner());
    all_group.apps.push(dialer());
    all_group.apps.push(switcheroo());
    all_group.apps.push(hatch());
    all_group.apps.push(drivemind());
    all_group.apps.push(bscale());
    all_group.apps.push(beatspace());
    all_group.apps.push(studio());
    all_group.apps.push(aperture());
    all_group.apps.push(cadscope());
    all_group.apps.push(simscope());
    all_group.apps.push(unendlich());
    all_group.apps.push(battleground());
    vec![
        {
            // cadscope and simscope join here rather than via the simple
            // list, for the same reason beatspace does below: the list
            // expects a package at `apps/<name>` and these are
            // workspaces whose runnable crate is one level down.
            let mut c = group("coding", "Development & analysis tools", &coding);
            c.apps.push(aperture());
            c.apps.push(cadscope());
            c.apps.push(simscope());
            c
        },
        media_group,
        {
            // beatspace joins its group here rather than via the simple list,
            // because it carries run_flags the list cannot express.
            let mut m = group("music", "Audio synthesis, codecs & music", &music);
            m.apps.push(beatspace());
            m
        },
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
        // Computed from the groups rather than hardcoded. The literal 16 fired
        // correctly when studio and a custom beatspace entry were added — but
        // "the number changed" is not the invariant anybody cares about. What
        // matters is that `all` really is the UNION, so adding an app to a
        // group and forgetting the all-group push is what should fail here.
        let mut union: std::collections::BTreeSet<String> = Default::default();
        for g in &groups {
            if g.name == "bigsilicons-apps" { continue; }
            union.extend(g.apps.iter().map(|a| a.name.clone()));
        }
        let in_all: std::collections::BTreeSet<String> =
            all.apps.iter().map(|a| a.name.clone()).collect();
        let missing: Vec<&String> = union.difference(&in_all).collect();
        assert!(missing.is_empty(),
                "in a group but not in `all`: {missing:?} — the all-group push was forgotten");
        assert_eq!(in_all.len(), all.apps.len(), "`all` lists an app twice");
        // Paths are bare app/game NAMES — discovery resolves apps/ vs games/ at
        // build time — EXCEPT where the crate is not at apps/<name>:
        //
        //   bsettings  apps/settings-app                    dir name != app name
        //   studio     apps/studio/crates/studio-app        a workspace member
        //   aperture   apps/aperture/crates/aperture-egui   workspace; bin is in a member
        //   cadscope   apps/cadscope/crates/cadscope-egui   same
        //   simscope   apps/simscope/crates/simscope-cli    same
        //
        // Listed by name rather than allowed generally, so a new explicit path
        // has to be justified here rather than slipping in: a bare name that
        // silently stopped resolving would look like a missing app.
        //
        // THE LAST THREE WERE MISSING AND THE TEST WAS RIGHT TO SAY SO. Each
        // is a workspace whose runnable crate is a member — the registry
        // entries say exactly that, a few hundred lines up, and adding the
        // entry without adding the exception is what this assertion exists to
        // catch. Fixed here rather than by relaxing the rule: "any path with a
        // slash is fine" would have let a genuine typo through silently, which
        // is the failure the rule was written against.
        // `ambient-learner` joins for a DIFFERENT reason from the other five.
        // Those are workspaces whose runnable crate is a member. This one is a
        // package at `apps/ambient-learner` that builds fine — it is just not
        // the one the manifest calls primary, so the entry points at
        // `egui-frontend` deliberately. Same exception, different cause, and
        // worth saying so: the next reader will otherwise assume it is another
        // workspace.
        const EXPLICIT_PATH: [&str; 11] = [
            "bsettings", "studio", "aperture", "cadscope", "simscope", "ambient-learner",
            // Five apps the store did not offer at all until they were added;
            // each is a workspace whose runnable crate is a member.
            "hatch", "drivemind", "switcheroo", "dialer", "bscale",
        ];
        for a in &all.apps {
            assert!(!a.path.contains('/') || EXPLICIT_PATH.contains(&a.name.as_str()),
                    "{} has an explicit path {:?} but is not on the exception list",
                    a.name, a.path);
        }
        assert!(all.apps.iter().any(|a| a.name == "unendlich" && a.path == "unendlich"), "unendlich stored by bare name");
        assert!(all.apps.iter().all(|a| a.name != "cybertron"), "cybertron (nonexistent) is not in the store");
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

/// Start / stop / restart a single daemon on the target, init-system aware. `action` is one of
/// "start", "stop", "restart"; an unknown verb is rejected before touching the network.
pub fn daemon_control_remote(user: &str, host: &str, name: &str, action: &str) -> Result<String, String> {
    let d = crate::daemons::find(name).ok_or_else(|| format!("unknown daemon {name}"))?;
    let act = crate::daemons::DaemonAction::parse(action)
        .ok_or_else(|| format!("unknown action {action} (want start/stop/restart)"))?;
    let init = detect_init_remote(user, host);
    let cmd = crate::daemons::control_cmd(d, init, act);
    crate::deploy::remote_capture(user, host, &cmd)
}

/// Enable a daemon AT BOOT on a target, and start it now.
///
/// `daemons::enable_cmd` has existed since the module was written and had no caller anywhere:
/// the page could start, stop and restart, all of which last until the phone is rebooted.
/// Someone who fixed a dead daemon and put the phone in their pocket found it dead again, with
/// nothing on the page having suggested there was a difference between "running" and "will run
/// next time".
///
/// Both init systems enable and start in one command — `systemctl --user enable --now` and
/// `rc-update add` followed by `rc-service start` — so this is a single round trip rather than
/// enable-then-start, which could half-succeed and leave a daemon enabled and down.
pub fn daemon_enable_remote(user: &str, host: &str, name: &str) -> Result<String, String> {
    let d = crate::daemons::find(name).ok_or_else(|| format!("unknown daemon {name}"))?;
    let init = detect_init_remote(user, host);
    let cmd = crate::daemons::enable_cmd(d, init);
    crate::deploy::remote_capture(user, host, &cmd)
}

/// Stream a daemon's logs live from a target, calling `on_line` per line until `cancel` is set.
/// A watcher thread kills the ssh child when `cancel` flips, so the blocking read returns promptly
/// instead of hanging on `journalctl -f`. Runs on a worker thread (the GUI feeds lines into a Job).
pub fn stream_daemon_logs(
    user: &str,
    host: &str,
    name: &str,
    cancel: std::sync::Arc<std::sync::atomic::AtomicBool>,
    on_line: &mut dyn FnMut(&str),
) -> Result<(), String> {
    use std::io::{BufRead, BufReader};
    use std::sync::atomic::Ordering;
    let d = crate::daemons::find(name).ok_or_else(|| format!("unknown daemon {name}"))?;
    let init = detect_init_remote(user, host);
    let cmd = crate::daemons::follow_cmd(d, init);
    on_line(&format!("$ streaming {name} logs from {host} …"));
    let mut child = std::process::Command::new("ssh")
        .args(crate::deploy::ssh_argv(user, host, &cmd))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("ssh failed: {e}"))?;
    let stdout = child.stdout.take().ok_or("no stdout from ssh")?;
    let child = std::sync::Arc::new(std::sync::Mutex::new(child));
    // watcher: when cancelled, kill the ssh child so the read loop below unblocks
    let (cw, cf) = (child.clone(), cancel.clone());
    let watcher = std::thread::spawn(move || {
        while !cf.load(Ordering::Relaxed) {
            std::thread::sleep(std::time::Duration::from_millis(250));
        }
        if let Ok(mut c) = cw.lock() {
            let _ = c.kill();
        }
    });
    for line in BufReader::new(stdout).lines().map_while(Result::ok) {
        on_line(&line);
        if cancel.load(Ordering::Relaxed) {
            break;
        }
    }
    cancel.store(true, Ordering::Relaxed); // release the watcher if the stream ended on its own
    let _ = watcher.join();
    if let Ok(mut c) = child.lock() {
        let _ = c.kill();
        let _ = c.wait();
    }
    on_line(&format!("— {name} log stream ended —"));
    Ok(())
}

/// Health of one device: whether it answered, and its per-daemon up/down.
#[derive(Debug, Clone)]
pub struct DeviceHealth {
    pub name: String,
    pub user: String,
    pub host: String,
    pub reachable: bool,
    pub daemons: Vec<(String, bool)>,
}

/// Roll daemon status up across the whole fleet in one pass: for each `(name, user, host)` target,
/// query its daemons and parse them. An unreachable device is reported (`reachable:false`) rather
/// than failing the whole rollup, so one dead phone doesn't blank the dashboard.
pub fn fleet_health_remote(targets: &[(String, String, String)]) -> Vec<DeviceHealth> {
    targets
        .iter()
        .map(|(name, user, host)| match daemons_status_remote(user, host) {
            Ok(out) => DeviceHealth {
                name: name.clone(),
                user: user.clone(),
                host: host.clone(),
                reachable: true,
                daemons: crate::daemons::parse_status(&out),
            },
            Err(_) => DeviceHealth {
                name: name.clone(),
                user: user.clone(),
                host: host.clone(),
                reachable: false,
                daemons: Vec::new(),
            },
        })
        .collect()
}

/// Fetch the most recent `lines` of a single daemon's logs from the target (user journal on
/// systemd, the service's log file on OpenRC).
pub fn daemon_logs_remote(user: &str, host: &str, name: &str, lines: u32) -> Result<String, String> {
    let d = crate::daemons::find(name).ok_or_else(|| format!("unknown daemon {name}"))?;
    let init = detect_init_remote(user, host);
    let cmd = crate::daemons::logs_cmd(d, init, lines.clamp(1, 2000));
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

#[cfg(test)]
mod launch_consistency_tests {
    use super::*;

    /// Every app in the catalog that needs launch args must carry them in
    /// `run_flags` too.
    ///
    /// This is the check that would have caught the beatspace bug. The `gui`
    /// verb reached the .desktop file and NOT the run button, because the two
    /// paths read different sources and nothing compared them. Three sources,
    /// one answer required:
    ///
    ///   binpin::gui_launch_args  →  the .desktop Exec line
    ///   flow::run_args           →  gitmind's run button
    ///   SuiteApp::run_flags      →  the entry written at install time
    #[test]
    fn catalog_run_flags_match_the_gui_launch_table() {
        for app in catalog_apps() {
            let table = gitmind_core::binpin::gui_launch_args(&app.name).join(" ");
            if table.is_empty() {
                continue;
            }
            assert_eq!(
                app.run_flags, table,
                "{}: gui_launch_args says {table:?} but the catalog entry says {:?} — \
                 the .desktop file and the run button would launch it differently",
                app.name, app.run_flags);
        }
    }

    #[test]
    fn an_app_needing_a_verb_is_actually_in_the_catalog() {
        // A launch table entry for an app nobody can install is a rule that
        // never fires, and it would hide a rename rather than reporting one.
        let names: Vec<String> = catalog_apps().into_iter().map(|a| a.name).collect();
        assert!(names.iter().any(|n| n == "beatspace"),
                "beatspace left the catalog but kept its gui_launch_args entry");
    }

    #[test]
    fn no_app_appears_in_the_catalog_twice() {
        // beatspace was briefly in both the simple `music` list and a custom
        // entry, which puts it in the group twice — a duplicate that reads as a
        // rendering bug rather than a data one.
        let mut names: Vec<String> = Vec::new();
        for g in store_groups() {
            let mut seen = std::collections::HashSet::new();
            for a in &g.apps {
                assert!(seen.insert(a.name.clone()),
                        "{} appears twice in group {}", a.name, g.name);
            }
            names.extend(g.apps.iter().map(|a| a.name.clone()));
        }
        assert!(!names.is_empty());
    }

    #[test]
    fn an_empty_flag_string_does_not_erase_a_saved_one() {
        // add_to_apps runs on every reinstall and passes the catalog's
        // run_flags, empty for almost every app. Writing it through would reset
        // whatever the user configured.
        //
        // Checked as a property of record_run_args rather than by touching
        // $HOME: the guard is the whole behaviour, and a test that wrote to the
        // real config file would be testing the filesystem.
        assert!(record_run_args("anything", "").is_ok());
        assert!(record_run_args("anything", "   ").is_ok());
    }

    /// Every app the store offers must point at a directory that exists.
    ///
    /// `store_groups()` is a HAND-CURATED list of names and paths, and the
    /// manifest and the filesystem move without it. Two ways it had already
    /// drifted, both found by writing this:
    ///
    /// * `av1tuner-m1` was still offered a session after the fork was merged
    ///   into `apps/av1tuner` and the directory deleted. The comment beside the
    ///   custom entries says a phantom entry was refused when the source was
    ///   missing — and it was, when someone ADDED one. Nothing noticed when one
    ///   appeared by deletion instead.
    /// * `photosync` resolved nowhere, because it lives at `system/photosync`
    ///   and `resolve_app_dir` searched only `apps/` and `games/`. The comment
    ///   in the store list asserted it was "a plain package at apps/photosync",
    ///   which it has never been — a doc stating engine state confidently and
    ///   wrongly, which is the failure CLAUDE.md §5 is about.
    ///
    /// A phantom entry costs a build and a confusing "no such package" on every
    /// device it is installed to, so this is a hard failure rather than a
    /// warning.
    #[test]
    /// The mirror of the phantom-entry check: an app registered in the
    /// manifest must actually be OFFERED by the fleet store.
    ///
    /// `every_store_entry_resolves_to_a_real_directory` catches a store entry
    /// with no directory. This catches the opposite and equally silent case: a
    /// real, registered, buildable app that the GUI never lists, because
    /// `store_groups()` is hand-maintained and nothing tied it to the manifest.
    /// `vidkit-player` was in exactly that state -- registered, `role = "app"`,
    /// buildable from `build/build.sh`, and absent from the store, so it was
    /// invisible in gitmind-egui. Registering an app is two edits, and nothing
    /// said so.
    ///
    /// "Offered" means reachable, not necessarily named: nine of the
    /// `*-egui` ports are reached through their PARENT, which `fleet.rs`
    /// discovers by prefix-matching the manifest path. So a port under an
    /// offered app counts, and only a top-level app with no offered ancestor
    /// is a real gap.
    #[test]
    fn every_registered_app_is_offered_in_the_store() {
        let root = crate::ops::repo_root();
        let text = match std::fs::read_to_string(root.join("manifest/projects.toml")) {
            Ok(t) => t,
            Err(_) => return,
        };
        let entries = crate::appcatalog::parse_manifest(&text);
        let registered: Vec<_> = entries
            .iter()
            .filter(|e| e.role == "app" && e.primary)
            .collect();
        // A filter that matched nothing passes everything below it.
        assert!(
            registered.len() >= 10,
            "only {} registered primary apps: the manifest or parser changed shape",
            registered.len()
        );

        // Every path the store can reach, repo-relative.
        let mut offered: Vec<String> = Vec::new();
        for g in store_groups() {
            for a in &g.apps {
                let dir = crate::ops::resolve_app_dir(&a.path);
                if let Ok(rel) = dir.strip_prefix(&root) {
                    offered.push(rel.display().to_string());
                }
            }
        }
        assert!(!offered.is_empty(), "the store resolved no paths at all");

        let mut unreachable = Vec::new();
        for e in &registered {
            // Either direction counts. A port sits UNDER its parent
            // (apps/voiceterm/egui-frontend under apps/voiceterm), but a
            // custom store entry names the runnable crate INSIDE a workspace
            // (apps/cadscope/crates/cadscope-egui for a manifest entry at
            // apps/cadscope). Both are reachable from the store; only an app
            // with no relationship to any offered path is a gap.
            let reachable = offered.iter().any(|o| {
                e.path == *o
                    || e.path.starts_with(&format!("{o}/"))
                    || o.starts_with(&format!("{}/", e.path))
            });
            if !reachable {
                unreachable.push(format!("{} ({})", e.app_id, e.path));
            }
        }
        assert!(
            unreachable.is_empty(),
            "registered apps the fleet store never offers -- add them to \
             store_groups(), registering in the manifest is not enough:\n  {}",
            unreachable.join("\n  ")
        );
    }

    fn every_store_entry_resolves_to_a_real_directory() {
        let groups = store_groups();
        // A list that came back empty would pass every assertion below it.
        let total: usize = groups.iter().map(|g| g.apps.len()).sum();
        assert!(total >= 15, "store_groups() offered only {total} apps");

        let mut phantom = Vec::new();
        for g in &groups {
            for a in &g.apps {
                let dir = crate::ops::resolve_app_dir(&a.path);
                if !dir.is_dir() {
                    phantom.push(format!(
                        "  {} (group {}) -> {} does not exist",
                        a.name, g.name, dir.display()
                    ));
                }
            }
        }
        assert!(
            phantom.is_empty(),
            "{} store entr(ies) point at nothing — each one is a failed build on \
             every device it reaches:\n{}",
            phantom.len(),
            phantom.join("\n")
        );
    }

    /// `settings-app` is offered under the name `bsettings` — the crate is
    /// `bigsilicon-settings`, the manifest id is `settings-app`, and the store
    /// shows what the user calls it. Matching on the id alone reports it as
    /// missing, which it is not; the entry's PATH is what settles it.
    ///
    /// The complement: an app the manifest calls primary and runnable should be
    /// offered, or its absence should be a decision somebody made rather than a
    /// list nobody updated.
    ///
    /// Reported from the field as "some missing apps like tadpole from the
    /// gitmind deploy list", and tadpole is one of several. NOT a hard failure:
    /// adding an app to the store without knowing it builds would trade a
    /// missing entry for a phantom one, which is worse. This prints the gap so
    /// the next person closes it deliberately.
    #[test]
    fn manifest_primary_apps_that_the_store_does_not_offer() {
        let root = crate::ops::repo_root();
        let Ok(text) = std::fs::read_to_string(root.join("manifest/projects.toml")) else {
            eprintln!("no manifest to compare against; skipping");
            return;
        };
        let entries = crate::appcatalog::parse_manifest(&text);
        assert!(entries.len() > 60, "only {} manifest entries parsed", entries.len());

        let offered: std::collections::BTreeSet<String> = store_groups()
            .iter()
            .flat_map(|g| g.apps.iter().map(|a| a.name.clone()))
            .collect();
        let offered_paths: std::collections::BTreeSet<String> = store_groups()
            .iter()
            .flat_map(|g| g.apps.iter().map(|a| a.path.clone()))
            .collect();

        let mut gap = Vec::new();
        for e in &entries {
            // Match on PATH as well as name: an entry can legitimately be
            // shown under a different label than its manifest id, which
            // `settings-app` / `bsettings` is.
            if offered_paths.contains(e.path.as_str()) {
                continue;
            }
            if !e.primary || !e.is_runnable() {
                continue;
            }
            // The store keys on the app FOLDER, not the manifest id: an entry
            // at `apps/tadpole/egui-frontend` is offered as "tadpole".
            let folder = e.path.split('/').nth(1).unwrap_or("");
            if folder.is_empty() || offered.contains(folder) {
                continue;
            }
            gap.push(format!("{folder} ({}, {})", e.app_id, e.path));
        }
        if !gap.is_empty() {
            eprintln!(
                "manifest marks these primary + runnable, and the store does not offer them:\n  {}",
                gap.join("\n  ")
            );
        }
    }
    /// Every store entry should build what the manifest calls PRIMARY.
    ///
    /// `primary = true` means "this entry IS the default build of the app it
    /// ports, not an alternative to opt into". A store entry naming the parent
    /// therefore offers the thing the manifest calls secondary — which is what
    /// was reported for ambient-learner: the button built the iced parent while
    /// `ambient-learner-egui` is what ships.
    ///
    /// Reported as a LIST rather than asserted to be empty. Nine other entries
    /// diverge the same way and each needs its own look — some may be
    /// deliberate, since a store entry can reasonably offer a native app beside
    /// a port. What is not defensible is nobody knowing which.
    #[test]
    fn store_entries_build_the_manifests_primary_port() {
        let text = std::fs::read_to_string(
            concat!(env!("CARGO_MANIFEST_DIR"), "/../../../../manifest/projects.toml"))
            .expect("the fleet manifest");
        let entries = crate::appcatalog::parse_manifest(&text);
        let primary: std::collections::BTreeSet<&str> =
            entries.iter().filter(|e| e.primary).map(|e| e.app_id.as_str()).collect();
        assert!(primary.len() > 10, "only {} primary entries — parser broken?", primary.len());

        let groups = store_groups();
        let mut diverging = Vec::new();
        for g in &groups {
            for a in &g.apps {
                // A custom entry with an explicit path has already made the
                // choice; only the bare `apps/<name>` shape is in question.
                if a.path != format!("apps/{}", a.name) {
                    continue;
                }
                if !primary.contains(a.name.as_str())
                    && primary.contains(format!("{}-egui", a.name).as_str())
                {
                    diverging.push(a.name.clone());
                }
            }
        }
        diverging.sort();
        diverging.dedup();

        // The one that was reported must be fixed.
        assert!(!diverging.iter().any(|n| n == "ambient-learner"),
                "ambient-learner still builds the iced parent");

        if !diverging.is_empty() {
            eprintln!("store entries building a non-primary parent ({}):\n  {}",
                      diverging.len(), diverging.join("\n  "));
        }
    }

}

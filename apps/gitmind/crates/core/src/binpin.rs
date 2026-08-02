//! `binpin` — run and pin binaries built from different commits.
//!
//! The workflow this replaces: `rsync` a local build into `~/.local/bin`, give
//! it a versioned name, and pin it with GNOME's Pins app. That works but is
//! all manual. Here gitmind owns it: each installed version is content-addressed
//! into the shared var store (so a binary that didn't change between commits is
//! stored once), recorded in a per-app JSON manifest (the commit log of which
//! version is which), materialized into `~/.local/bin/{app}@{label}`, and
//! exposed as a `.desktop` entry so it shows up in the GNOME grid and can be
//! pinned/favorited. A `current` symlink (`~/.local/bin/{app}`) makes swapping
//! the active version a single atomic relink.
//!
//! All paths are env-overridable (handy for tests and for non-standard setups):
//!   GITMIND_BIN      → install dir            (default ~/.local/bin)
//!   GITMIND_APPS     → manifest dir           (default ~/.local/share/gitmind/apps)
//!   GITMIND_DESKTOP  → .desktop dir           (default ~/.local/share/applications)

use crate::oid::Oid;
use crate::store::ChunkStore;
use serde::{Deserialize, Serialize};
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};

#[derive(Clone, Debug, Serialize, Deserialize, PartialEq)]
pub struct AppVersion {
    /// Human label: head03, v1.2.0, a short oid, a branch name.
    pub label: String,
    /// The gitmind commit this binary was built from (the "log").
    pub commit: String,
    /// Content-addressed blob oid of the binary bytes (dedup key).
    pub oid: String,
    /// Build target triple, or "host".
    pub target: String,
    /// Unix seconds when installed.
    pub installed: u64,
    /// Installed file path (`{bin}/{app}@{label}`).
    pub path: String,
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct AppManifest {
    pub name: String,
    /// Label of the active version (the `current` symlink target).
    pub current: Option<String>,
    pub versions: Vec<AppVersion>,
}

fn home() -> PathBuf {
    PathBuf::from(std::env::var("HOME").unwrap_or_else(|_| ".".into()))
}
pub fn bin_dir() -> PathBuf {
    std::env::var("GITMIND_BIN").map(PathBuf::from).unwrap_or_else(|_| home().join(".local/bin"))
}
pub fn apps_dir() -> PathBuf {
    std::env::var("GITMIND_APPS").map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/gitmind/apps"))
}
pub fn applications_dir() -> PathBuf {
    std::env::var("GITMIND_DESKTOP").map(PathBuf::from)
        .unwrap_or_else(|_| home().join(".local/share/applications"))
}

fn manifest_path(name: &str) -> PathBuf {
    apps_dir().join(format!("{name}.json"))
}

pub fn load_manifest(name: &str) -> AppManifest {
    std::fs::read(manifest_path(name))
        .ok()
        .and_then(|b| serde_json::from_slice(&b).ok())
        .unwrap_or_else(|| AppManifest { name: name.to_string(), current: None, versions: vec![] })
}

fn save_manifest(m: &AppManifest) -> Result<(), String> {
    std::fs::create_dir_all(apps_dir()).map_err(|e| e.to_string())?;
    let bytes = serde_json::to_vec_pretty(m).map_err(|e| e.to_string())?;
    std::fs::write(manifest_path(&m.name), bytes).map_err(|e| e.to_string())
}

fn now_secs() -> u64 {
    std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

/// List installed apps (manifest names).
pub fn list_apps() -> Vec<String> {
    let mut out = Vec::new();
    if let Ok(rd) = std::fs::read_dir(apps_dir()) {
        for e in rd.flatten() {
            if let Some(n) = e.path().file_stem().and_then(|s| s.to_str()) {
                if e.path().extension().and_then(|x| x.to_str()) == Some("json") {
                    out.push(n.to_string());
                }
            }
        }
    }
    out.sort();
    out
}

/// Install a binary as `{app}@{label}`: store bytes content-addressed in the
/// var store (dedup), write the executable, record the manifest version. If a
/// version with the same label exists it is replaced. Makes it `current` when
/// it's the first version or `make_current` is set.
pub fn install(
    store: &ChunkStore,
    name: &str,
    label: &str,
    commit: &str,
    target: &str,
    bytes: &[u8],
    make_current: bool,
) -> Result<AppVersion, String> {
    // Content-address the bytes (dedup across versions/projects). Use a
    // constant ingest name so identical binaries collapse to one blob oid
    // regardless of app/label, while staying restorable by that oid.
    let report = store.ingest("binpin", bytes)?;
    let oid = report.blob_oid.to_hex();

    let bdir = bin_dir();
    std::fs::create_dir_all(&bdir).map_err(|e| e.to_string())?;
    let file = bdir.join(format!("{name}@{label}"));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let mut perm = std::fs::metadata(&file).map_err(|e| e.to_string())?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&file, perm).map_err(|e| e.to_string())?;

    let version = AppVersion {
        label: label.to_string(),
        commit: commit.to_string(),
        oid,
        target: target.to_string(),
        installed: now_secs(),
        path: file.to_string_lossy().to_string(),
    };

    let mut m = load_manifest(name);
    m.versions.retain(|v| v.label != label);
    m.versions.push(version.clone());
    let first = m.current.is_none();
    save_manifest(&m)?;
    if make_current || first {
        use_version(name, label)?;
    }
    Ok(version)
}

/// Point `{bin}/{app}` at `{app}@{label}` (atomic relink) and record it.
pub fn use_version(name: &str, label: &str) -> Result<(), String> {
    let m = load_manifest(name);
    if !m.versions.iter().any(|v| v.label == label) {
        return Err(format!("no such version {name}@{label}"));
    }
    let bdir = bin_dir();
    let link = bdir.join(name);
    let tmp = bdir.join(format!(".{name}.tmp"));
    let _ = std::fs::remove_file(&tmp);
    std::os::unix::fs::symlink(format!("{name}@{label}"), &tmp).map_err(|e| e.to_string())?;
    std::fs::rename(&tmp, &link).map_err(|e| e.to_string())?; // atomic swap
    let mut m = m;
    m.current = Some(label.to_string());
    save_manifest(&m)
}

/// Remove one version: its file, its `.desktop`, and its manifest entry.
pub fn remove(name: &str, label: &str) -> Result<(), String> {
    let bdir = bin_dir();
    let _ = std::fs::remove_file(bdir.join(format!("{name}@{label}")));
    let _ = std::fs::remove_file(applications_dir().join(desktop_filename(name, label)));
    let mut m = load_manifest(name);
    m.versions.retain(|v| v.label != label);
    if m.current.as_deref() == Some(label) {
        m.current = None;
        let _ = std::fs::remove_file(bdir.join(name));
    }
    save_manifest(&m)
}

/// Remove an app entirely: every version, the manifest, and the current binary.
///
/// `remove()` takes a LABEL and removes one version. There was no way to remove
/// an app, so the GUI called `remove(name, "")` — an empty label matches no
/// version, retains everything, and saves an unchanged manifest. It reported
/// success and did nothing, which is the worst of both.
pub fn remove_app(name: &str) -> Result<(), String> {
    let m = load_manifest(name);
    let bdir = bin_dir();
    for v in &m.versions {
        let _ = std::fs::remove_file(bdir.join(format!("{name}@{}", v.label)));
        let _ = std::fs::remove_file(applications_dir().join(desktop_filename(name, &v.label)));
    }
    let _ = std::fs::remove_file(bdir.join(name));
    // The manifest goes last: if anything above failed, the app is still listed
    // and can be removed again. Deleting it first would strand the binaries with
    // nothing recording that they exist.
    std::fs::remove_file(manifest_path(name))
        .or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
        .map_err(|e| e.to_string())
}

/// Every installed version, as `(app, label, is_current)`.
///
/// `list_apps` returns manifest NAMES only, which is why the GUI could not show
/// or remove a specific version — it had nothing to name one with.
pub fn list_versions() -> Vec<(String, String, bool)> {
    let mut out = Vec::new();
    for name in list_apps() {
        let m = load_manifest(&name);
        for v in m.versions {
            let cur = m.current.as_deref() == Some(v.label.as_str());
            out.push((name.clone(), v.label, cur));
        }
    }
    out
}

pub fn desktop_filename(name: &str, label: &str) -> String {
    format!("gitmind-{name}@{label}.desktop")
}

/// Build a freedesktop `.desktop` entry for a pinned version.
pub fn desktop_entry(v: &AppVersion, name: &str, terminal: bool, icon: Option<&str>) -> String {
    let short = &v.commit[..v.commit.len().min(8)];
    let mut s = String::from("[Desktop Entry]\n");
    s.push_str("Type=Application\n");
    s.push_str(&format!("Name={name} @ {}\n", v.label));
    s.push_str(&format!("Comment=gitmind-pinned {name} from commit {short} ({})\n", v.target));
    s.push_str(&format!("Exec={} %U\n", v.path));
    s.push_str(&format!("Icon={}\n", icon.unwrap_or("application-x-executable")));
    s.push_str(&format!("Terminal={}\n", if terminal { "true" } else { "false" }));
    s.push_str("Categories=Development;\n");
    s.push_str(&format!("X-GitMind-Commit={}\n", v.commit));
    s.push_str(&format!("X-GitMind-Label={}\n", v.label));
    s
}

/// Write the `.desktop` file for a version into the applications dir so GNOME
/// shows it (and lets the user pin/favorite it). Returns the file path.
pub fn write_desktop(name: &str, label: &str, terminal: bool, icon: Option<&str>) -> Result<PathBuf, String> {
    let m = load_manifest(name);
    let v = m.versions.iter().find(|v| v.label == label)
        .ok_or_else(|| format!("no such version {name}@{label}"))?;
    let dir = applications_dir();
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = dir.join(desktop_filename(name, label));
    std::fs::write(&path, desktop_entry(v, name, terminal, icon)).map_err(|e| e.to_string())?;
    Ok(path)
}

/// Reinstall a version's binary from the var store (e.g. on a new machine
/// after pulling the manifest), addressed by its recorded oid.
pub fn materialize_from_store(store: &ChunkStore, name: &str, label: &str) -> Result<PathBuf, String> {
    let m = load_manifest(name);
    let v = m.versions.iter().find(|v| v.label == label)
        .ok_or_else(|| format!("no such version {name}@{label}"))?;
    let oid = Oid::from_hex(&v.oid).ok_or("bad stored oid")?;
    let bytes = store.restore_bytes(oid)?;
    let bdir = bin_dir();
    std::fs::create_dir_all(&bdir).map_err(|e| e.to_string())?;
    let file = bdir.join(format!("{name}@{label}"));
    std::fs::write(&file, bytes).map_err(|e| e.to_string())?;
    let mut perm = std::fs::metadata(&file).map_err(|e| e.to_string())?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(&file, perm).map_err(|e| e.to_string())?;
    Ok(file)
}

/// Resolve the binary path for `{app}@{label}` (for `run`).
pub fn version_path(name: &str, label: &str) -> Option<PathBuf> {
    let p = bin_dir().join(format!("{name}@{label}"));
    p.exists().then_some(p)
}

/// Build a binary from a commit by materializing its tree and running a build
/// command, then return the produced binary's bytes. `build_cmd` runs in the
/// materialized worktree; `artifact` is the path (relative to it) of the binary
/// to capture. For cargo: cmd `cargo build --release [--target T]`, artifact
/// `target/release/<bin>` (or `target/<T>/release/<bin>`). Cross toolchains are
/// the caller's responsibility (e.g. `cargo`, `cross`, or `zig cc`).
pub fn build_from_commit(
    store: &ChunkStore,
    commit: &str,
    workdir: &Path,
    build_cmd: &[String],
    artifact: &str,
) -> Result<Vec<u8>, String> {
    let oid = crate::native::resolve_ref_or_commit(store, commit)?;
    let meta = crate::portage::native_commit(store, &oid)?;
    std::fs::create_dir_all(workdir).map_err(|e| e.to_string())?;
    let mut n = 0;
    crate::native::materialize_tree(store, &meta.tree, workdir, &mut n)?;
    if let Some((prog, args)) = build_cmd.split_first() {
        let status = std::process::Command::new(prog)
            .args(args)
            .current_dir(workdir)
            .status()
            .map_err(|e| format!("build command failed to start: {e}"))?;
        if !status.success() {
            return Err(format!("build command exited with {status}"));
        }
    }
    let art = workdir.join(artifact);
    std::fs::read(&art).map_err(|e| format!("artifact not found at {}: {e}", art.display()))
}

#[cfg(test)]
mod remove_tests {
    use super::*;

    /// Point HOME at a scratch dir so the store is real but disposable — these
    /// functions touch the filesystem by design, and a mock would test the mock.
    struct Scratch(std::path::PathBuf);
    impl Scratch {
        fn new(tag: &str) -> Scratch {
            let p = std::env::temp_dir().join(format!("gitmind-test-{tag}-{}", std::process::id()));
            let _ = std::fs::remove_dir_all(&p);
            std::fs::create_dir_all(&p).unwrap();
            std::env::set_var("HOME", &p);
            Scratch(p)
        }
    }
    impl Drop for Scratch {
        fn drop(&mut self) { let _ = std::fs::remove_dir_all(&self.0); }
    }

    fn pin(name: &str, label: &str) {
        let mut m = load_manifest(name);
        m.versions.push(AppVersion {
            label: label.into(), commit: "c0ffee".into(), oid: "0".into(),
            target: "host".into(), installed: 0, path: String::new(),
        });
        if m.current.is_none() { m.current = Some(label.into()); }
        save_manifest(&m).unwrap();
    }

    #[test]
    fn removing_with_an_empty_label_no_longer_looks_like_success() {
        // THE bug: the GUI called remove(name, "") because list_apps gives bare
        // names. An empty label matches no version, so retain() kept everything
        // and the manifest was saved unchanged — while the UI said "removed".
        let _s = Scratch::new("empty-label");
        pin("demo", "v1");
        remove("demo", "").unwrap();
        assert_eq!(load_manifest("demo").versions.len(), 1,
                   "an empty label still removes nothing — which is why remove_app exists");
    }

    #[test]
    fn remove_app_takes_the_whole_app_with_it() {
        let _s = Scratch::new("whole-app");
        pin("demo", "v1");
        pin("demo", "v2");
        assert_eq!(list_apps(), vec!["demo".to_string()]);
        remove_app("demo").unwrap();
        assert!(list_apps().is_empty(), "the app is gone from the list");
        assert!(load_manifest("demo").versions.is_empty());
    }

    #[test]
    fn removing_an_app_that_is_not_there_is_not_an_error() {
        // Two clicks on remove, or a stale list. Failing the second is noise
        // about a state the user already wanted.
        let _s = Scratch::new("absent");
        assert!(remove_app("never-installed").is_ok());
    }

    #[test]
    fn list_versions_names_something_remove_can_use() {
        // The root cause: list_apps returns names with no way to identify a
        // version, so the GUI had nothing to pass as a label.
        let _s = Scratch::new("versions");
        pin("demo", "v1");
        pin("demo", "v2");
        let v = list_versions();
        assert_eq!(v.len(), 2);
        assert!(v.iter().any(|(a, l, cur)| a == "demo" && l == "v1" && *cur));
        assert!(v.iter().any(|(a, l, cur)| a == "demo" && l == "v2" && !*cur));
        // and each one round-trips through remove
        remove("demo", "v2").unwrap();
        assert_eq!(list_versions().len(), 1);
    }

    #[test]
    fn removing_the_current_version_clears_the_pointer() {
        // Otherwise `current` names a version that is gone and `gitmind run`
        // fails with a missing file rather than saying nothing is selected.
        let _s = Scratch::new("current");
        pin("demo", "v1");
        remove("demo", "v1").unwrap();
        assert_eq!(load_manifest("demo").current, None);
    }
}

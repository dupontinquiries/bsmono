//! Cross-cutting operations shared by every frontend: resolving cargo, building locally
//! (native vs cross, shared target cache), target detection, gitmind-root discovery,
//! gitignore upkeep, and fleet-user provisioning. Frontend-agnostic — no Qt, no I/O
//! beyond std::process.

use std::path::{Path, PathBuf};

/// The commit oid of the last change touching an app's directory — the seed for its tile
/// colour. Empty if not in a git repo or git isn't available.
pub fn app_oid(root: &Path, dir: &Path) -> String {
    std::process::Command::new("git")
        .arg("-C").arg(root)
        .args(["log", "-1", "--format=%H", "--"])
        .arg(dir)
        .output()
        .ok()
        .filter(|o| o.status.success())
        .map(|o| String::from_utf8_lossy(&o.stdout).trim().to_string())
        .unwrap_or_default()
}

/// Resolve the cargo binary even when launched from a desktop launcher (whose PATH
/// usually omits ~/.cargo/bin): honour $CARGO, then ~/.cargo/bin/cargo, then the name.
pub fn cargo_bin() -> String {
    if let Ok(c) = std::env::var("CARGO") {
        if !c.trim().is_empty() {
            return c;
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        let cand = format!("{home}/.cargo/bin/cargo");
        if std::path::Path::new(&cand).is_file() {
            return cand;
        }
    }
    "cargo".to_string()
}

/// A PATH that includes ~/.cargo/bin and the standard locations, so a build (and its
/// rustc/linker subprocesses) resolves even from a launcher.
pub fn tool_path_env() -> String {
    let home = std::env::var("HOME").unwrap_or_default();
    let cur = std::env::var("PATH").unwrap_or_default();
    format!("{home}/.cargo/bin:/usr/local/bin:/usr/bin:/bin{}", if cur.is_empty() { String::new() } else { format!(":{cur}") })
}

/// This device's native target triple, normalized to the gitmind vocabulary (e.g.
/// Alpine's `aarch64-alpine-linux-musl` → `aarch64-unknown-linux-musl`). From rustc;
/// falls back to an Alpine check only if rustc isn't reachable.
pub fn native_triple_local() -> String {
    std::process::Command::new("rustc")
        .arg("-vV")
        .env("PATH", tool_path_env())
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::buildflags::host_triple_from_rustc(&String::from_utf8_lossy(&o.stdout)))
        .map(|h| crate::buildflags::normalize_native(&h))
        .unwrap_or_else(|| {
            let libc = if std::path::Path::new("/etc/alpine-release").exists() { "musl" } else { "gnu" };
            format!("aarch64-unknown-linux-{libc}")
        })
}

/// Walk up from `start` to the gitmind repo root — the directory holding a `.gitmind`
/// marker. None if not under a gitmind-managed repo.
pub fn gitmind_root(start: &Path) -> Option<PathBuf> {
    let mut cur = Some(start);
    while let Some(d) = cur {
        if d.join(".gitmind").exists() {
            return Some(d.to_path_buf());
        }
        cur = d.parent();
    }
    None
}

/// The base directory all builds go into, so they never spill `target/` trees across the
/// repos themselves. Override with ~/.config/gitmind/build-base; otherwise ~/.var/gitmind/
/// build, falling back to /tmp/gitmind/build when there's no usable HOME.
pub fn build_base() -> PathBuf {
    if let Ok(home) = std::env::var("HOME") {
        if !home.is_empty() {
            let cfg = PathBuf::from(format!("{home}/.config/gitmind/build-base"));
            if let Ok(s) = std::fs::read_to_string(&cfg) {
                let t = s.trim();
                if !t.is_empty() {
                    return PathBuf::from(crate::places::expand_home(t, &home));
                }
            }
            return PathBuf::from(format!("{home}/.var/gitmind/build"));
        }
    }
    PathBuf::from("/tmp/gitmind/build")
}

/// Remember a build base for next time (created lazily when a build runs).
pub fn set_build_base(path: &str) -> Result<(), String> {
    let home = std::env::var("HOME").map_err(|_| "no HOME".to_string())?;
    let dir = PathBuf::from(format!("{home}/.config/gitmind"));
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let want = crate::places::expand_home(path.trim(), &home);
    std::fs::write(dir.join("build-base"), format!("{want}\n")).map_err(|e| e.to_string())
}

/// The per-project build directory: `<build_base>/<crate>-<project-key>`. Every copy of a
/// project resolves to the same directory (keyed by git root identity), so duplicates
/// build once instead of each spilling their own multi-GB target/.
pub fn build_dir_for(app_dir: &Path, path_env: &str) -> PathBuf {
    build_base().join(crate::identity::build_slug(app_dir, path_env))
}

/// Where cargo writes the binary for `app` built in `app_dir`. `cargo_target` is the
/// `--target` value ("" for native). `target_dir_env` is an optional CARGO_TARGET_DIR
/// override — applied to the metadata query too so the resolved path matches.
pub fn resolve_binary(cargo: &str, app_dir: &Path, cargo_target: &str, app: &str, target_dir_env: Option<&str>) -> PathBuf {
    let mut cmd = std::process::Command::new(cargo);
    cmd.args(["metadata", "--no-deps", "--format-version", "1"])
        .current_dir(app_dir)
        .env("PATH", tool_path_env());
    if let Some(td) = target_dir_env {
        cmd.env("CARGO_TARGET_DIR", td);
    }
    let target_dir = cmd
        .output()
        .ok()
        .filter(|o| o.status.success())
        .and_then(|o| crate::buildflags::parse_target_directory(&String::from_utf8_lossy(&o.stdout)))
        .map(PathBuf::from)
        .unwrap_or_else(|| app_dir.join("target"));
    let dir = if cargo_target.trim().is_empty() {
        target_dir.join("release")
    } else {
        target_dir.join(cargo_target).join("release")
    };
    dir.join(app)
}

/// Build `app` in `app_dir` for `target` and return the binary path + byte size. Native
/// (no --target) when the libc matches this device; cross otherwise. gitmind repos share
/// the cargo target cache.
/// The monorepo root, so repo-relative app paths (`apps/<name>`) resolve no matter what
/// directory gitmind was launched from. Order: `$GITMIND_ROOT`, then walk up from the current
/// dir looking for the repo root (a dir with an `apps/` subdir plus a `.git`/`.gitmind` marker),
/// then fall back to the current dir.
pub fn repo_root() -> PathBuf {
    if let Ok(r) = std::env::var("GITMIND_ROOT") {
        let p = PathBuf::from(r);
        if p.join("apps").is_dir() {
            return p;
        }
    }
    if let Ok(cwd) = std::env::current_dir() {
        let mut cur: &Path = &cwd;
        loop {
            let has_apps = cur.join("apps").is_dir();
            let marked = cur.join(".git").exists() || cur.join(".gitmind").exists();
            if has_apps && marked {
                return cur.to_path_buf();
            }
            match cur.parent() {
                Some(p) => cur = p,
                None => break,
            }
        }
        return cwd;
    }
    PathBuf::from(".")
}

/// Resolve an app directory that may be repo-relative (`apps/foo`) to an absolute path against
/// [`repo_root`]. Absolute paths pass through unchanged.
pub fn resolve_app_dir(path: &str) -> PathBuf {
    let p = Path::new(path);
    if p.is_absolute() {
        p.to_path_buf()
    } else {
        repo_root().join(p)
    }
}

/// Like [`build_locally`] but streams cargo's output line-by-line to `on_line` as it runs, so a
/// UI can show a live build log. Returns the built binary path + size on success.
pub fn build_locally_logged(
    app: &str,
    app_dir: &Path,
    target: &str,
    feats: &[String],
    no_default: bool,
    on_line: &mut dyn FnMut(&str),
) -> Result<(PathBuf, u64), String> {
    use std::io::{BufRead, BufReader};
    if !app_dir.join("Cargo.toml").exists() {
        return Err(format!(
            "no Cargo.toml at {} — the app path didn't resolve to a crate (launch dir: {}). Set GITMIND_ROOT or run from the repo.",
            app_dir.display(),
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let cargo = cargo_bin();
    let native = native_triple_local();
    let path_env = tool_path_env();
    let cargo_target = if crate::buildflags::is_native_build(target, &native) { String::new() } else { target.to_string() };
    if !cargo_target.is_empty() {
        on_line(&format!("• ensuring rust target {cargo_target}…"));
        crate::sysdeps::ensure_rust_target(&cargo_target, &path_env).map_err(|e| format!("cross-build setup: {e}"))?;
    }
    let want = build_dir_for(app_dir, &path_env);
    let build_dir = if std::fs::create_dir_all(&want).is_ok() {
        want
    } else {
        let fb = PathBuf::from("/tmp/gitmind/build").join(crate::identity::build_slug(app_dir, &path_env));
        let _ = std::fs::create_dir_all(&fb);
        fb
    };
    // Workspace root with no [package] (gitmind: package `gitmind-cli`, binary `gitmind`) → build
    // `--bin <name>`, since `-p <name>` names a package that doesn't exist. The binary is still
    // named <name>, so binary resolution is unchanged.
    let root_toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let as_bin = crate::buildflags::declares_workspace(&root_toml)
        && crate::buildflags::crate_name(&root_toml).is_none()
        && !has_member_package(app_dir, app); // logscope HAS a member pkg `logscope` → keep -p
    let argv = crate::buildflags::cargo_build_argv_kind(app, &cargo_target, feats, no_default, as_bin);
    on_line(&format!("$ cargo {}", argv[1..].join(" ")));
    on_line(&format!("  (in {})", app_dir.display()));
    let mut child = std::process::Command::new(&cargo)
        .args(&argv[1..])
        .current_dir(app_dir)
        .env("PATH", &path_env)
        .env("CARGO_TERM_COLOR", "never")
        .env("CARGO_TARGET_DIR", &build_dir)
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "couldn't find cargo. Install Rust, or run from a shell where cargo is on PATH (~/.cargo/bin).".to_string()
            } else {
                format!("couldn't run the build: {e}")
            }
        })?;
    // cargo writes progress + errors to stderr; stream it live
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            on_line(&line);
        }
    }
    let status = child.wait().map_err(|e| format!("build wait failed: {e}"))?;
    if status.success() {
        let bin = resolve_binary(&cargo, app_dir, &cargo_target, app, Some(&build_dir.display().to_string()));
        let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
        on_line(&format!("✓ built {} ({} KB)", app, size / 1024));
        Ok((bin, size))
    } else {
        Err(format!("build failed (exit {})", status.code().unwrap_or(-1)))
    }
}

/// Does a workspace under `dir` contain a MEMBER package literally named `name`? If so, `-p name`
/// resolves (build that member); if not (gitmind: members are `gitmind-cli` etc., no `gitmind`), we
/// must build `--bin name` instead. Bounded walk; skips target/.git.
fn has_member_package(dir: &Path, name: &str) -> bool {
    fn walk(dir: &Path, name: &str, depth: u32) -> bool {
        if depth > 3 {
            return false;
        }
        if let Ok(txt) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            if crate::buildflags::crate_name(&txt).as_deref() == Some(name) {
                return true;
            }
        }
        if let Ok(rd) = std::fs::read_dir(dir) {
            for e in rd.flatten() {
                let p = e.path();
                let n = e.file_name().to_string_lossy().into_owned();
                if p.is_dir() && n != "target" && !n.starts_with('.') && walk(&p, name, depth + 1) {
                    return true;
                }
            }
        }
        false
    }
    walk(dir, name, 0)
}

pub fn build_locally(app: &str, app_dir: &Path, target: &str, feats: &[String], no_default: bool) -> Result<(PathBuf, u64), String> {
    if !app_dir.join("Cargo.toml").exists() {
        return Err(format!(
            "no Cargo.toml at {} — the app path didn't resolve to a crate (launch dir: {}). Set GITMIND_ROOT or run from the repo.",
            app_dir.display(),
            std::env::current_dir().map(|p| p.display().to_string()).unwrap_or_default()
        ));
    }
    let cargo = cargo_bin();
    let native = native_triple_local();
    let path_env = tool_path_env();
    let cargo_target = if crate::buildflags::is_native_build(target, &native) { String::new() } else { target.to_string() };
    // Cross-build? Make sure the target's std is installed first, so we don't fail with
    // "can't find crate for std" — gitmind runs `rustup target add <triple>` for you.
    if !cargo_target.is_empty() {
        crate::sysdeps::ensure_rust_target(&cargo_target, &path_env)
            .map_err(|e| format!("cross-build setup: {e}"))?;
    }
    // one canonical build dir per project identity — no target/ spilled into the repo, and
    // every copy of the project shares it. Fall back to /tmp if ~/.var isn't writable.
    let want = build_dir_for(app_dir, &path_env);
    let build_dir = if std::fs::create_dir_all(&want).is_ok() {
        want
    } else {
        let fb = PathBuf::from("/tmp/gitmind/build").join(crate::identity::build_slug(app_dir, &path_env));
        let _ = std::fs::create_dir_all(&fb);
        fb
    };
    // Workspace root with no [package] (gitmind: package `gitmind-cli`, binary `gitmind`) → build
    // `--bin <name>`, since `-p <name>` names a package that doesn't exist. The binary is still
    // named <name>, so binary resolution is unchanged.
    let root_toml = std::fs::read_to_string(app_dir.join("Cargo.toml")).unwrap_or_default();
    let as_bin = crate::buildflags::declares_workspace(&root_toml)
        && crate::buildflags::crate_name(&root_toml).is_none()
        && !has_member_package(app_dir, app); // logscope HAS a member pkg `logscope` → keep -p
    let argv = crate::buildflags::cargo_build_argv_kind(app, &cargo_target, feats, no_default, as_bin);
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(&argv[1..]).current_dir(app_dir).env("PATH", &path_env).env("CARGO_TARGET_DIR", &build_dir);
    match cmd.output() {
        Ok(o) if o.status.success() => {
            let bin = resolve_binary(&cargo, app_dir, &cargo_target, app, Some(&build_dir.display().to_string()));
            let size = std::fs::metadata(&bin).map(|m| m.len()).unwrap_or(0);
            Ok((bin, size))
        }
        Ok(o) => Err(String::from_utf8_lossy(&o.stderr).trim().to_string()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Err(
            "couldn't find cargo. Install Rust, or run from a shell where cargo is on PATH (it lives in ~/.cargo/bin).".to_string()
        ),
        Err(e) => Err(format!("couldn't run the build: {e}")),
    }
}

/// Append any missing lines to a directory's .gitignore (creating it if needed).
pub fn ensure_gitignore(dir: &Path, lines: &[&str]) {
    let path = dir.join(".gitignore");
    let existing = std::fs::read_to_string(&path).unwrap_or_default();
    let mut out = existing.clone();
    for line in lines {
        let present = existing.lines().any(|l| l.trim() == *line);
        if !present {
            if !out.is_empty() && !out.ends_with('\n') { out.push('\n'); }
            out.push_str(line);
            out.push('\n');
        }
    }
    if out != existing {
        let _ = std::fs::write(&path, out);
    }
}

/// Current time as an OpenSSH expiry-time stamp (YYYYMMDDHHMMSS).
pub fn now_stamp() -> String {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    crate::deploy::key_expiry_time(now, 0)
}

/// Mark a freshly-installed binary executable (best-effort; Unix perms).
pub fn set_executable(path: &str) {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(perms.mode() | 0o755);
        let _ = std::fs::set_permissions(path, perms);
    }
}

/// Create the fleet user on a device and install the restricted key into it, over ssh as
/// the sudo-capable `admin` account. On-device (useradd/adduser + sudo).
pub fn provision_fleet_user(admin: &str, host: &str, password: &str) -> Result<String, String> {
    let user = crate::fleet::fleet_user();
    let pubkey = crate::deploy::ensure_local_pubkey()?;
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let expiry = crate::deploy::key_expiry_time(now, crate::deploy::KEY_EXPIRY_DAYS);
    let key_line = crate::deploy::restricted_key_line(&crate::deploy::authorized_keys_options(host, Some(&expiry)), &pubkey);
    let script = crate::fleet::provision_user_script(&user, &key_line);
    let remote = crate::fleet::provision_remote_command(&crate::fleet::base64_encode(script.as_bytes()));
    let out = crate::deploy::ssh_sudo_run(admin, host, password, &remote, &format!("{password}\n"))?;
    if out.contains("gitmind-fleet-user-ready") {
        Ok(format!("created fleet user '{user}' on {host} — set the user to '{user}' to connect"))
    } else {
        Err("provisioning didn't confirm — does that account have sudo rights?".into())
    }
}

#[cfg(test)]
mod root_tests {
    use super::*;

    #[test]
    fn repo_root_finds_monorepo_and_resolves_app_paths() {
        // running under cargo, CWD is the crate dir; walking up must find the repo root
        // (a dir with apps/ + a .git/.gitmind marker).
        let root = repo_root();
        assert!(root.join("apps").is_dir(), "repo_root has an apps/ dir: {}", root.display());
        // a repo-relative app path resolves to an existing crate with a Cargo.toml
        let gm = resolve_app_dir("apps/gitmind");
        assert!(gm.is_absolute());
        assert!(gm.join("Cargo.toml").exists(), "resolved {} should be a crate", gm.display());
        // absolute paths pass through
        assert_eq!(resolve_app_dir("/tmp/x"), std::path::PathBuf::from("/tmp/x"));
    }

    #[test]
    fn member_package_check_distinguishes_gitmind_from_logscope() {
        let repo = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).ancestors().nth(4).unwrap().to_path_buf();
        // gitmind: NO member package named "gitmind" (it's gitmind-cli, bin gitmind) → build --bin
        assert!(!super::has_member_package(&repo.join("apps/gitmind"), "gitmind"));
        // logscope: HAS a member package named "logscope" (the core lib) → keep -p logscope
        assert!(super::has_member_package(&repo.join("apps/logscope"), "logscope"));
    }

    #[test]
    fn build_locally_errors_clearly_on_bad_path() {
        // a path with no Cargo.toml fails fast with a helpful message, not a silent no-op
        let err = build_locally("nope", Path::new("/definitely/not/a/crate"), "", &[], false).unwrap_err();
        assert!(err.contains("no Cargo.toml"), "clear error: {err}");
    }
}

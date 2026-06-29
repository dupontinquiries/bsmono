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

/// Shared cargo target dir for gitmind-managed repos — one cache so deps build once
/// across every clone instead of a multi-GB target/ tree per repo.
pub fn shared_target_dir() -> String {
    let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
    format!("{home}/.cache/gitmind/cargo-target")
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
pub fn build_locally(app: &str, app_dir: &Path, target: &str, feats: &[String], no_default: bool) -> Result<(PathBuf, u64), String> {
    let cargo = cargo_bin();
    let native = native_triple_local();
    let cargo_target = if crate::buildflags::is_native_build(target, &native) { String::new() } else { target.to_string() };
    let shared = gitmind_root(app_dir).map(|_| shared_target_dir());
    let argv = crate::buildflags::cargo_build_argv(app, &cargo_target, feats, no_default);
    let mut cmd = std::process::Command::new(&cargo);
    cmd.args(&argv[1..]).current_dir(app_dir).env("PATH", tool_path_env());
    if let Some(td) = &shared {
        let _ = std::fs::create_dir_all(td);
        cmd.env("CARGO_TARGET_DIR", td);
    }
    match cmd.output() {
        Ok(o) if o.status.success() => {
            let bin = resolve_binary(&cargo, app_dir, &cargo_target, app, shared.as_deref());
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

#![allow(dead_code)]
/// probe.rs — whois-style log ownership detection
///
/// Given a path, answers: who owns this log, what tier does it belong to,
/// and therefore how should rotation + privilege be handled?
///
/// Detection strategy (in order):
///   1. Path prefix matching — fast, covers 90% of cases
///   2. stat(2) uid/gid vs current uid/gid — catches edge cases
///   3. Capability/permission check — can we write without elevation?
///   4. Dotfile/stray-script heuristics — ~/.local, XDG dirs, etc.
///
/// This is deliberately not a policy engine. It returns facts. The UI
/// and rotation modules decide what to do with them.

use std::path::{Path, PathBuf};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};

#[cfg(unix)]
use nix::unistd::{getuid, getgid};
#[cfg(unix)]
use std::os::unix::fs::MetadataExt;

// ─── output types ────────────────────────────────────────────────────────────

/// Which management tier this log belongs to.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Tier {
    /// Owned by current user, in user XDG/home dirs. No privilege needed.
    /// Config goes in ~/.config/logscope/, schedule via systemd --user.
    User,

    /// Owned by root or a system service uid. Needs pkexec / sudo.
    /// Config goes in /etc/logrotate.d/ or /etc/logscope/.
    System,

    /// Shared group log (e.g. /var/log/nginx owned by www-data group,
    /// current user is in that group). Can write without sudo.
    SharedGroup { group_name: String },

    /// We can't determine ownership or access is denied.
    Unknown,
}

/// How the rotation backend should be invoked.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RotationBackend {
    /// Use Logscope's native Rust rotation engine.
    /// Generates a .toml config in the appropriate tier dir.
    Native,

    /// Write a logrotate-compatible .conf and invoke system logrotate.
    /// Preferred on Alpine/postmarketOS where logrotate is standard.
    Logrotate { conf_path: PathBuf },

    /// Logrotate is present but needs privilege escalation to write config.
    /// UI will show a pkexec prompt on Fedora/GNOME.
    LogrotatePrivileged { conf_path: PathBuf },
}

/// Full probe result for a single file.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProbeResult {
    pub path: PathBuf,
    pub tier: Tier,
    pub rotation_backend: RotationBackend,
    pub size_bytes: u64,
    pub owner_uid: u32,
    pub owner_gid: u32,
    pub owner_name: Option<String>,
    pub group_name: Option<String>,
    pub permissions_octal: String,
    pub is_writable_by_us: bool,
    pub is_symlink: bool,
    pub modified: Option<DateTime<Utc>>,
    /// Any interesting notes (e.g. "inside Docker volume", "on tmpfs")
    pub notes: Vec<String>,
}

// ─── public API ──────────────────────────────────────────────────────────────

/// Probe a single path. Never panics, returns Unknown tier on any error.
pub fn probe(path: &Path) -> ProbeResult {
    match probe_inner(path) {
        Ok(r) => r,
        Err(_) => ProbeResult {
            path: path.to_owned(),
            tier: Tier::Unknown,
            rotation_backend: RotationBackend::Native,
            size_bytes: 0,
            owner_uid: 0,
            owner_gid: 0,
            owner_name: None,
            group_name: None,
            permissions_octal: "?".into(),
            is_writable_by_us: false,
            is_symlink: false,
            modified: None,
            notes: vec!["probe failed".into()],
        },
    }
}

// ─── internals ───────────────────────────────────────────────────────────────

fn probe_inner(path: &Path) -> std::io::Result<ProbeResult> {
    let meta = std::fs::symlink_metadata(path)?;
    let is_symlink = meta.file_type().is_symlink();

    // Follow symlink for the real metadata
    let real_meta = std::fs::metadata(path)?;

    #[cfg(unix)]
    let (owner_uid, owner_gid, mode) = (
        real_meta.uid(),
        real_meta.gid(),
        real_meta.mode(),
    );

    #[cfg(not(unix))]
    let (owner_uid, owner_gid, mode) = (0u32, 0u32, 0o644u32);

    let permissions_octal = format!("{:o}", mode & 0o7777);

    let (owner_name, group_name) = resolve_names(owner_uid, owner_gid);

    #[cfg(unix)]
    let current_uid = getuid().as_raw();
    #[cfg(not(unix))]
    let current_uid = 0u32;

    #[cfg(unix)]
    let current_gid = getgid().as_raw();
    #[cfg(not(unix))]
    let current_gid = 0u32;

    let is_writable_by_us = check_writable(path, &real_meta, owner_uid, owner_gid,
                                            current_uid, current_gid, mode);

    let modified = real_meta
        .modified()
        .ok()
        .map(|t| DateTime::<Utc>::from(t));

    let tier = classify_tier(path, owner_uid, current_uid, owner_gid,
                              current_gid, &group_name, mode);

    let (rotation_backend, notes) = choose_backend(&tier, path);

    Ok(ProbeResult {
        path: path.to_owned(),
        tier,
        rotation_backend,
        size_bytes: real_meta.len(),
        owner_uid,
        owner_gid,
        owner_name,
        group_name,
        permissions_octal,
        is_writable_by_us,
        is_symlink,
        modified,
        notes,
    })
}

/// Classify tier using path prefix first, then uid fallback.
fn classify_tier(
    path: &Path,
    owner_uid: u32,
    current_uid: u32,
    owner_gid: u32,
    current_gid: u32,
    group_name: &Option<String>,
    mode: u32,
) -> Tier {
    let path_str = path.to_string_lossy();

    // ── path prefix rules (authoritative) ────────────────────────────────
    // System paths — always system tier regardless of who's asking
    let system_prefixes = [
        "/var/log/",
        "/var/run/",
        "/run/log/",
        "/run/systemd/",
        "/etc/",
        "/usr/",
        "/opt/",
        "/srv/",
        "/proc/",
        "/sys/",
    ];
    if system_prefixes.iter().any(|p| path_str.starts_with(p)) {
        return Tier::System;
    }

    // User home / XDG paths — always user tier
    let user_prefixes: Vec<String> = [
        dirs::home_dir().map(|p| p.to_string_lossy().into_owned()),
        dirs::cache_dir().map(|p| p.to_string_lossy().into_owned()),
        dirs::config_dir().map(|p| p.to_string_lossy().into_owned()),
        dirs::data_dir().map(|p| p.to_string_lossy().into_owned()),
        dirs::runtime_dir().map(|p| p.to_string_lossy().into_owned()),
    ]
    .into_iter()
    .flatten()
    .collect();

    if user_prefixes.iter().any(|p| path_str.starts_with(p.as_str())) {
        return Tier::User;
    }

    // ── uid fallback ──────────────────────────────────────────────────────
    if owner_uid == current_uid {
        return Tier::User;
    }

    // Group-writable and current user is in that group?
    let group_writable = (mode & 0o020) != 0;
    if group_writable && owner_gid == current_gid {
        if let Some(name) = group_name {
            return Tier::SharedGroup { group_name: name.clone() };
        }
    }

    // root-owned or unknown uid
    if owner_uid == 0 {
        return Tier::System;
    }

    Tier::Unknown
}

/// Pick the best rotation backend and collect any notes.
fn choose_backend(tier: &Tier, path: &Path) -> (RotationBackend, Vec<String>) {
    let mut notes = Vec::new();

    // Detect if logrotate is available on this system
    let logrotate_available = which_logrotate().is_some();
    if logrotate_available {
        notes.push("system logrotate detected".into());
    } else {
        notes.push("logrotate not found — using native rotation".into());
    }

    // Alpine / postmarketOS detection heuristics
    let is_alpine_like = std::path::Path::new("/etc/alpine-release").exists()
        || std::path::Path::new("/etc/postmarketos-release").exists()
        || is_musl_system();
    if is_alpine_like {
        notes.push("Alpine/postmarketOS detected — logrotate preferred".into());
    }

    match tier {
        Tier::User => {
            let conf_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("logscope/rotate");
            let conf_name = sanitize_conf_name(path);

            if logrotate_available && is_alpine_like {
                // On Alpine, prefer logrotate since it's the system standard
                (RotationBackend::Logrotate {
                    conf_path: conf_dir.join(format!("{conf_name}.conf")),
                }, notes)
            } else {
                // On modern systemd distros, native is cleaner for user logs
                (RotationBackend::Native, notes)
            }
        }

        Tier::System => {
            let conf_path = PathBuf::from("/etc/logrotate.d")
                .join(format!("logscope-{}", sanitize_conf_name(path)));

            if logrotate_available {
                (RotationBackend::LogrotatePrivileged { conf_path }, notes)
            } else {
                notes.push("no logrotate found — native rotation with pkexec".into());
                (RotationBackend::Native, notes)
            }
        }

        Tier::SharedGroup { .. } => {
            let conf_dir = dirs::config_dir()
                .unwrap_or_else(|| PathBuf::from("~/.config"))
                .join("logscope/rotate");
            let conf_name = sanitize_conf_name(path);
            (RotationBackend::Logrotate {
                conf_path: conf_dir.join(format!("{conf_name}.conf")),
            }, notes)
        }

        Tier::Unknown => (RotationBackend::Native, notes),
    }
}

// ─── helpers ─────────────────────────────────────────────────────────────────

/// Resolve uid → username, gid → groupname via /etc/passwd and /etc/group.
/// Pure Rust, no nss, no libc. Safe on Alpine (musl).
fn resolve_names(uid: u32, gid: u32) -> (Option<String>, Option<String>) {
    let uname = read_etc_passwd(uid);
    let gname = read_etc_group(gid);
    (uname, gname)
}

fn read_etc_passwd(uid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/passwd").ok()?;
    for line in content.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _ = fields.next(); // password placeholder
        let file_uid: u32 = fields.next()?.parse().ok()?;
        if file_uid == uid {
            return Some(name.to_owned());
        }
    }
    None
}

fn read_etc_group(gid: u32) -> Option<String> {
    let content = std::fs::read_to_string("/etc/group").ok()?;
    for line in content.lines() {
        let mut fields = line.split(':');
        let name = fields.next()?;
        let _ = fields.next();
        let file_gid: u32 = fields.next()?.parse().ok()?;
        if file_gid == gid {
            return Some(name.to_owned());
        }
    }
    None
}

fn check_writable(
    path: &Path,
    _meta: &std::fs::Metadata,
    owner_uid: u32,
    owner_gid: u32,
    current_uid: u32,
    current_gid: u32,
    mode: u32,
) -> bool {
    // root can write anything
    if current_uid == 0 { return true; }
    // owner write bit
    if owner_uid == current_uid && (mode & 0o200) != 0 { return true; }
    // group write bit
    if owner_gid == current_gid && (mode & 0o020) != 0 { return true; }
    // world write bit (unusual but handle it)
    if (mode & 0o002) != 0 { return true; }
    // Try opening for write as a definitive check
    std::fs::OpenOptions::new().write(true).open(path).is_ok()
}

fn which_logrotate() -> Option<PathBuf> {
    for dir in ["/usr/sbin", "/usr/bin", "/sbin", "/bin"] {
        let p = PathBuf::from(dir).join("logrotate");
        if p.exists() {
            return Some(p);
        }
    }
    None
}

fn is_musl_system() -> bool {
    // Quick heuristic: musl systems typically have /lib/ld-musl-*.so.1
    std::fs::read_dir("/lib")
        .ok()
        .map(|entries| {
            entries
                .flatten()
                .any(|e| e.file_name().to_string_lossy().starts_with("ld-musl-"))
        })
        .unwrap_or(false)
}

fn sanitize_conf_name(path: &Path) -> String {
    path.to_string_lossy()
        .replace(['/', '\\', ' ', ':'], "-")
        .trim_matches('-')
        .to_owned()
}

// ─── dotfile / stray script detection ────────────────────────────────────────

/// Find "stray" user scripts and dotfiles adjacent to or managing the given log.
/// These are one-off scripts that are worth tracking alongside the rotation config.
pub fn find_stray_scripts(log_path: &Path) -> Vec<PathBuf> {
    let mut found = Vec::new();

    // Same directory as the log
    if let Some(parent) = log_path.parent() {
        for ext in &["sh", "bash", "zsh", "py", "rb", "pl"] {
            if let Ok(entries) = std::fs::read_dir(parent) {
                for entry in entries.flatten() {
                    let p = entry.path();
                    if p.extension().and_then(|e| e.to_str()) == Some(ext) {
                        found.push(p);
                    }
                }
            }
        }
    }

    // ~/.local/bin — common place for user scripts
    if let Some(home) = dirs::home_dir() {
        let local_bin = home.join(".local/bin");
        if let Ok(entries) = std::fs::read_dir(&local_bin) {
            for entry in entries.flatten() {
                let p = entry.path();
                // Include if name contains "log" or "rotate"
                let name = p.file_name().unwrap_or_default().to_string_lossy().to_lowercase();
                if name.contains("log") || name.contains("rotate") || name.contains("clean") {
                    found.push(p);
                }
            }
        }
    }

    found.dedup();
    found
}

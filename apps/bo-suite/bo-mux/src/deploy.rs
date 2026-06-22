// src/deploy.rs
//
// The deploy/updater the GUI drives to install and keep the bo-mux daemon +
// systemd units up to date. The GUI ships with (or builds) a `bo-muxd`
// binary and the unit files; this module places them in the right user
// locations, (re)loads systemd, and performs versioned updates so the GUI
// can transparently upgrade the running daemon.
//
// Everything here is USER-level (no root): binary → ~/.local/bin, units →
// ~/.config/systemd/user, data → ~/.local/share/bo-mux. That keeps install
// permissionless and per-user, which suits a desktop media app.
//
// The functions return structured results so the GUI can show progress and
// surface errors. The actual `systemctl --user` calls are done via
// std::process::Command (sketched where a live system is required; the
// path/IO logic is real and testable).

use std::path::{Path, PathBuf};
use std::process::Command;

#[derive(Debug, Clone, PartialEq)]
pub struct DeployPaths {
    pub bin: PathBuf,        // ~/.local/bin/bo-muxd
    pub socket_unit: PathBuf,// ~/.config/systemd/user/bo-mux.socket
    pub service_unit: PathBuf,// ~/.config/systemd/user/bo-mux.service
    pub data_dir: PathBuf,   // ~/.local/share/bo-mux
    pub version_stamp: PathBuf, // ~/.local/share/bo-mux/.installed_version
}

impl DeployPaths {
    pub fn for_home(home: &Path) -> Self {
        Self {
            bin: home.join(".local/bin/bo-muxd"),
            socket_unit: home.join(".config/systemd/user/bo-mux.socket"),
            service_unit: home.join(".config/systemd/user/bo-mux.service"),
            data_dir: home.join(".local/share/bo-mux"),
            version_stamp: home.join(".local/share/bo-mux/.installed_version"),
        }
    }
    pub fn from_env() -> Option<Self> {
        std::env::var_os("HOME").map(|h| Self::for_home(Path::new(&h)))
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum DeployAction {
    FreshInstall,
    Upgrade { from: String, to: String },
    AlreadyCurrent,
}

/// Decide what the deployer should do, given the version already installed
/// (read from the stamp) and the version the GUI is shipping. Pure +
/// testable — the side-effecting install uses this to decide.
pub fn plan_action(installed: Option<&str>, shipping: &str) -> DeployAction {
    match installed {
        None => DeployAction::FreshInstall,
        Some(v) if v == shipping => DeployAction::AlreadyCurrent,
        Some(v) => DeployAction::Upgrade { from: v.to_string(), to: shipping.to_string() },
    }
}

/// Read the installed version stamp, if present.
pub fn installed_version(paths: &DeployPaths) -> Option<String> {
    std::fs::read_to_string(&paths.version_stamp).ok().map(|s| s.trim().to_string())
}

/// Result of an install/upgrade attempt.
#[derive(Debug)]
pub struct DeployReport {
    pub action: DeployAction,
    pub steps: Vec<String>,    // human-readable log for the GUI
    pub ok: bool,
    pub error: Option<String>,
}

/// Install or upgrade the daemon. `bin_src` and the unit sources are paths
/// to the files the GUI is shipping (e.g. bundled next to the GUI binary).
///
/// Steps:
///   1. ensure dirs exist
///   2. if upgrading, stop the running socket+service first
///   3. copy binary + unit files into place (atomic-ish: write temp, rename)
///   4. write the version stamp
///   5. systemctl --user daemon-reload && enable --now bo-mux.socket
pub fn install_or_upgrade(
    paths: &DeployPaths,
    shipping_version: &str,
    bin_src: &Path,
    socket_src: &Path,
    service_src: &Path,
) -> DeployReport {
    let mut steps = Vec::new();
    let action = plan_action(installed_version(paths).as_deref(), shipping_version);

    if action == DeployAction::AlreadyCurrent {
        return DeployReport { action, steps, ok: true, error: None };
    }

    macro_rules! step { ($($a:tt)*) => {{ steps.push(format!($($a)*)); }} }
    macro_rules! tryio {
        ($e:expr, $action:expr) => {
            match $e { Ok(v) => v, Err(e) =>
                return DeployReport { action: $action, steps, ok: false,
                    error: Some(format!("{}", e)) } }
        };
    }

    // 1. dirs
    for d in [paths.bin.parent(), paths.socket_unit.parent(), Some(paths.data_dir.as_path())].into_iter().flatten() {
        tryio!(std::fs::create_dir_all(d), action.clone());
    }
    step!("ensured directories");

    // 2. stop running units before swapping the binary (ignore errors if
    //    not yet installed / not running).
    if matches!(action, DeployAction::Upgrade { .. }) {
        let _ = systemctl(&["stop", "bo-mux.service"]);
        let _ = systemctl(&["stop", "bo-mux.socket"]);
        step!("stopped running daemon for upgrade");
    }

    // 3. copy files via temp + rename for atomic replace.
    tryio!(copy_atomic(bin_src, &paths.bin), action.clone());
    tryio!(make_executable(&paths.bin), action.clone());
    tryio!(copy_atomic(socket_src, &paths.socket_unit), action.clone());
    tryio!(copy_atomic(service_src, &paths.service_unit), action.clone());
    step!("installed binary + unit files");

    // 4. version stamp
    tryio!(std::fs::write(&paths.version_stamp, shipping_version), action.clone());
    step!("wrote version stamp {}", shipping_version);

    // 5. reload + (re)enable the socket
    let _ = systemctl(&["daemon-reload"]);
    match systemctl(&["enable", "--now", "bo-mux.socket"]) {
        Ok(_) => step!("enabled bo-mux.socket"),
        Err(e) => step!("warning: enable socket failed ({}); is systemd --user available?", e),
    }

    DeployReport { action, steps, ok: true, error: None }
}

/// Restart the daemon (e.g. after the GUI pushes a config change that needs
/// a reload). Socket stays up, so in-flight clients reconnect cleanly.
pub fn restart_daemon() -> Result<(), String> {
    systemctl(&["restart", "bo-mux.service"]).map(|_| ())
}

/// Tell a running daemon to shut down gracefully via the control socket
/// (used right before a binary swap if not relying on systemctl stop).
pub fn request_graceful_shutdown(socket_path: &Path) -> std::io::Result<()> {
    use std::io::Write;
    use std::os::unix::net::UnixStream;
    let mut s = UnixStream::connect(socket_path)?;
    s.write_all(b"{\"id\":0,\"cmd\":\"shutdown\"}\n")?;
    Ok(())
}

// ─── helpers ─────────────────────────────────────────────────────────────────

fn systemctl(args: &[&str]) -> Result<String, String> {
    let out = Command::new("systemctl").arg("--user").args(args).output()
        .map_err(|e| format!("spawn systemctl: {}", e))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).to_string())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

fn copy_atomic(src: &Path, dst: &Path) -> std::io::Result<()> {
    let tmp = dst.with_extension("tmp-install");
    std::fs::copy(src, &tmp)?;
    std::fs::rename(&tmp, dst)?;
    Ok(())
}

#[cfg(unix)]
fn make_executable(p: &Path) -> std::io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mut perm = std::fs::metadata(p)?.permissions();
    perm.set_mode(0o755);
    std::fs::set_permissions(p, perm)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn plan_fresh_when_no_stamp() {
        assert_eq!(plan_action(None, "1.2.0"), DeployAction::FreshInstall);
    }
    #[test]
    fn plan_current_when_match() {
        assert_eq!(plan_action(Some("1.2.0"), "1.2.0"), DeployAction::AlreadyCurrent);
    }
    #[test]
    fn plan_upgrade_when_differ() {
        assert_eq!(
            plan_action(Some("1.1.0"), "1.2.0"),
            DeployAction::Upgrade { from: "1.1.0".into(), to: "1.2.0".into() }
        );
    }
    #[test]
    fn paths_are_user_local() {
        let p = DeployPaths::for_home(Path::new("/home/kesslerd"));
        assert_eq!(p.bin, Path::new("/home/kesslerd/.local/bin/bo-muxd"));
        assert!(p.socket_unit.ends_with("systemd/user/bo-mux.socket"));
        assert!(p.service_unit.ends_with("systemd/user/bo-mux.service"));
    }
}

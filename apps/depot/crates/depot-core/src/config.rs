//! `depot.toml` and where the root directory lives.
//!
//! Root resolution, in order: `--root`, `$DEPOT_ROOT`, then a Linux default
//! that depends on who is running:
//!
//! * root (uid 0) or a systemd `DynamicUser` → `/var/lib/depot`. The FHS puts
//!   "variable state information" for system services under `/var/lib`, and
//!   systemd's `StateDirectory=depot` creates exactly that path, owned by the
//!   service user, with no unit-file chown dance.
//! * anyone else → `$XDG_DATA_HOME/depot` (`~/.local/share/depot`). XDG is the
//!   per-user mirror of the FHS: config in `~/.config`, state in
//!   `~/.local/state`, data in `~/.local/share`. A mirror's objects are data,
//!   not config, so they go under share.
//!
//! The config file itself lives INSIDE the root (`<root>/depot.toml`) rather
//! than in `/etc/depot/` — deliberately. A depot is a self-contained directory
//! you can `rsync` or carry on a disk; splitting its config into `/etc` would
//! make a moved depot silently pick up the wrong settings. The cost is that
//! `/etc` is where a sysadmin looks first; `depot status` prints the path.

use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

pub const CONFIG_FILE: &str = "depot.toml";

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub server: Server,
    pub rpm: Rpm,
    pub crates: Crates,
    pub nix: Nix,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Server {
    /// Bind address. 8000 not 80: ports below 1024 need CAP_NET_BIND_SERVICE
    /// or root, and a mirror does not need either. Put a reverse proxy in
    /// front if :80 or TLS is wanted.
    pub bind: String,
    /// Path (relative to root) of a file whose trimmed contents is the bearer
    /// token write endpoints require. If the file is absent, HTTP writes are
    /// refused outright — read-only is the safe default for a LAN service.
    pub admin_token_file: String,
    pub workers: usize,
    pub log: bool,
    /// Public base URL clients should use, e.g. "http://depot.lan:8000". Empty
    /// = derive from the request's Host header. Needed because cargo's
    /// config.json must carry an absolute `dl` URL.
    pub public_url: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Rpm {
    /// "gz" (readable by every dnf/yum ever shipped) or "zst" (dnf ≥ 4.4 and
    /// dnf5; needs depot built with the `zstd` feature). The metadata is a
    /// rounding error next to the packages, so gz is the default: universal
    /// beats 20% smaller on a file nobody notices.
    pub compression: String,
    /// "native": depot's own RPM header reader writes repodata. "createrepo_c":
    /// shell out to createrepo_c if it is on PATH (falls back to native).
    pub repodata: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Crates {
    /// The registry name clients will use in `[registries.<name>]`.
    pub registry_name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(default)]
pub struct Nix {
    pub store_dir: String,
    /// Substituter priority; lower wins. cache.nixos.org is 40, so 30 makes
    /// the LAN cache preferred when both are reachable.
    pub priority: u32,
}

impl Default for Config {
    fn default() -> Self {
        Config { server: Server::default(), rpm: Rpm::default(), crates: Crates::default(), nix: Nix::default() }
    }
}
impl Default for Server {
    fn default() -> Self {
        Server { bind: "0.0.0.0:8000".into(), admin_token_file: "keys/admin.token".into(), workers: 32, log: true, public_url: String::new() }
    }
}
impl Default for Rpm {
    fn default() -> Self {
        Rpm { compression: "gz".into(), repodata: "native".into() }
    }
}
impl Default for Crates {
    fn default() -> Self {
        Crates { registry_name: "depot".into() }
    }
}
impl Default for Nix {
    fn default() -> Self {
        Nix { store_dir: "/nix/store".into(), priority: 30 }
    }
}

impl Config {
    pub fn load(root: &Path) -> crate::Result<Config> {
        let p = root.join(CONFIG_FILE);
        if !p.exists() {
            return Ok(Config::default());
        }
        let s = std::fs::read_to_string(&p)?;
        toml::from_str(&s).map_err(|e| crate::invalid(format!("{}: {e}", p.display())))
    }

    pub fn save(&self, root: &Path) -> crate::Result<()> {
        let s = toml::to_string_pretty(self).map_err(|e| crate::invalid(e.to_string()))?;
        crate::write_atomic(&root.join(CONFIG_FILE), s.as_bytes())?;
        Ok(())
    }

    /// The default config file with every key present and commented — what
    /// `depot init` writes so the operator sees the knobs.
    pub fn annotated_default() -> String {
        let d = Config::default();
        format!(
            r#"# depot.toml — configuration for this depot. Lives inside the root so the
# directory stays self-contained (see DECISIONS.md §1).

[server]
# Bind address. 8000 by default: unprivileged, and firewalld/nftables will need
# a rule for it either way (`firewall-cmd --add-port=8000/tcp --permanent`).
bind = "{bind}"
# Bearer token for write endpoints (PUT /api/...). Relative to the root. If the
# file does not exist, HTTP writes are refused; `depot serve` still serves.
admin_token_file = "{tok}"
workers = {workers}
log = {log}
# Absolute URL clients use; "" derives it from each request's Host header.
# Set it when the depot is behind a proxy or a fixed DNS name.
public_url = "{pub_url}"

[rpm]
# "gz" is readable by every dnf; "zst" needs dnf >= 4.4 (Fedora 33+) and a
# depot built with --features zstd.
compression = "{comp}"
# "native" = depot's own RPM header reader; "createrepo_c" = shell out if on PATH.
repodata = "{repodata}"

[crates]
# The name clients use in ~/.cargo/config.toml [registries.<name>].
registry_name = "{reg}"

[nix]
store_dir = "{store}"
# Lower wins; cache.nixos.org is 40.
priority = {prio}
"#,
            bind = d.server.bind,
            tok = d.server.admin_token_file,
            workers = d.server.workers,
            log = d.server.log,
            pub_url = d.server.public_url,
            comp = d.rpm.compression,
            repodata = d.rpm.repodata,
            reg = d.crates.registry_name,
            store = d.nix.store_dir,
            prio = d.nix.priority,
        )
    }
}

/// Where the depot root should be if the caller did not say.
pub fn default_root() -> PathBuf {
    if let Ok(r) = std::env::var("DEPOT_ROOT") {
        if !r.is_empty() {
            return PathBuf::from(r);
        }
    }
    // systemd sets STATE_DIRECTORY for units with StateDirectory=; honour it
    // before guessing from uid.
    if let Ok(sd) = std::env::var("STATE_DIRECTORY") {
        if !sd.is_empty() {
            return PathBuf::from(sd.split(':').next().unwrap_or(&sd));
        }
    }
    if is_root() {
        return PathBuf::from("/var/lib/depot");
    }
    let data_home = std::env::var("XDG_DATA_HOME").ok().filter(|s| !s.is_empty()).map(PathBuf::from).unwrap_or_else(|| {
        let home = std::env::var("HOME").unwrap_or_else(|_| "/tmp".into());
        Path::new(&home).join(".local/share")
    });
    data_home.join("depot")
}

#[cfg(unix)]
fn is_root() -> bool {
    // No libc dependency: /proc/self/status has "Uid:\t0\t0\t0\t0".
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(1) == Some("0")))
        .unwrap_or(false)
}
#[cfg(not(unix))]
fn is_root() -> bool {
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn annotated_default_parses_back_to_default() {
        let c: Config = toml::from_str(&Config::annotated_default()).unwrap();
        assert_eq!(c.server.bind, "0.0.0.0:8000");
        assert_eq!(c.rpm.compression, "gz");
        assert_eq!(c.nix.priority, 30);
        assert_eq!(c.crates.registry_name, "depot");
    }

    #[test]
    fn partial_file_fills_defaults() {
        let c: Config = toml::from_str("[server]\nbind = \"127.0.0.1:1\"\n").unwrap();
        assert_eq!(c.server.bind, "127.0.0.1:1");
        assert_eq!(c.server.workers, 32);
        assert_eq!(c.rpm.repodata, "native");
    }

    #[test]
    fn env_root_wins() {
        std::env::set_var("DEPOT_ROOT", "/x/y");
        assert_eq!(default_root(), PathBuf::from("/x/y"));
        std::env::remove_var("DEPOT_ROOT");
        std::env::set_var("STATE_DIRECTORY", "/var/lib/depot:/var/lib/other");
        assert_eq!(default_root(), PathBuf::from("/var/lib/depot"));
        std::env::remove_var("STATE_DIRECTORY");
    }
}

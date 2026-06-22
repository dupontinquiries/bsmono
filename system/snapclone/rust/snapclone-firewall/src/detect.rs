//! Firewall backend detection — ported from FirewallPortManager::detectBackend().

use serde::{Deserialize, Serialize};
use tracing::{debug, info};

/// Detected firewall backend.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
pub enum Backend {
    Firewalld,  // firewalld D-Bus / firewall-cmd
    Ufw,        // ufw command
    Nftables,   // nft command (Alpine / postmarketOS)
    Iptables,   // iptables command (legacy)
    #[default]
    None,       // no firewall / unsupported
}

impl Backend {
    pub fn name(self) -> &'static str {
        match self {
            Self::Firewalld => "firewalld",
            Self::Ufw       => "ufw",
            Self::Nftables  => "nftables",
            Self::Iptables  => "iptables",
            Self::None      => "none",
        }
    }
    /// True if this backend can open ports without root (uses PolicyKit / sudo).
    pub fn needs_privilege(self) -> bool {
        !matches!(self, Self::None)
    }
}

/// Detect the active firewall backend by probing for running services/binaries.
/// Order matches C++ FirewallPortManager::detectBackend() priority.
pub fn detect_backend() -> Backend {
    // 1. firewalld — check if the service is running (pidof or systemctl)
    if service_is_running("firewalld") || binary_exists("firewall-cmd") {
        info!("Firewall backend: firewalld");
        return Backend::Firewalld;
    }

    // 2. ufw — check if active
    if binary_exists("ufw") && ufw_is_active() {
        info!("Firewall backend: ufw");
        return Backend::Ufw;
    }

    // 3. nftables — common on Alpine/postmarketOS
    if binary_exists("nft") && nftables_has_rules() {
        info!("Firewall backend: nftables");
        return Backend::Nftables;
    }

    // 4. iptables — legacy
    if binary_exists("iptables") {
        info!("Firewall backend: iptables");
        return Backend::Iptables;
    }

    debug!("No firewall detected");
    Backend::None
}

/// True if the current process has NET_ADMIN capability or is root.
/// Mirrors FirewallPortManager::isRootOrCapable().
pub fn has_caps() -> bool {
    // Effective UID 0 = root
    #[cfg(unix)]
    unsafe { if libc_getuid() == 0 { return true; } }

    // Check /proc/self/status for CapEff (Linux capability bitmask)
    if let Ok(status) = std::fs::read_to_string("/proc/self/status") {
        for line in status.lines() {
            if line.starts_with("CapEff:") {
                let hex = line.split(':').nth(1).unwrap_or("").trim();
                if let Ok(caps) = u64::from_str_radix(hex, 16) {
                    const CAP_NET_ADMIN: u64 = 1 << 12;
                    return caps & CAP_NET_ADMIN != 0;
                }
            }
        }
    }
    false
}

fn binary_exists(name: &str) -> bool {
    std::process::Command::new("which")
        .arg(name)
        .output()
        .map(|o| o.status.success())
        .unwrap_or(false)
}

fn service_is_running(name: &str) -> bool {
    std::process::Command::new("systemctl")
        .args(["is-active", "--quiet", name])
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

fn ufw_is_active() -> bool {
    std::process::Command::new("ufw")
        .arg("status")
        .output()
        .map(|o| parse_ufw_active(&String::from_utf8_lossy(&o.stdout)))
        .unwrap_or(false)
}

/// Pure classifier for `ufw status` output. Extracted from `ufw_is_active`
/// so it can be unit-tested without invoking ufw.
///
/// `ufw status` prints "Status: active" or "Status: inactive". The naive
/// `.contains("active")` is a FALSE POSITIVE: "active" is a substring of
/// "inactive", so a disabled-but-installed ufw was detected as active and
/// selected as the backend — rules would then be added to a firewall that
/// isn't enforcing anything. Match the full "status: active" token
/// (lowercased) and explicitly reject the "inactive" case.
fn parse_ufw_active(output: &str) -> bool {
    let out = output.to_lowercase();
    out.contains("status: active") && !out.contains("inactive")
}

fn nftables_has_rules() -> bool {
    std::process::Command::new("nft")
        .args(["list", "ruleset"])
        .output()
        .map(|o| !o.stdout.is_empty() && o.status.success())
        .unwrap_or(false)
}

// Minimal libc stub — avoids bringing in the full libc crate
#[cfg(unix)]
extern "C" { fn getuid() -> u32; }
#[cfg(unix)]
unsafe fn libc_getuid() -> u32 { unsafe { getuid() } }

#[cfg(test)]
mod tests {
    use super::*;

    // V32.34+: guards the ufw-status false-positive fix. `.contains("active")`
    // matched "inactive" too, so a disabled ufw read as enabled.
    #[test]
    fn ufw_inactive_is_not_active() {
        assert!(!parse_ufw_active("Status: inactive"));
        assert!(!parse_ufw_active("Status: inactive\n"));
        // Case-insensitive (ufw localizations / casing differences).
        assert!(!parse_ufw_active("STATUS: INACTIVE"));
    }

    #[test]
    fn ufw_active_is_active() {
        assert!(parse_ufw_active("Status: active"));
        assert!(parse_ufw_active("Status: active\n\nTo  Action  From\n--  ------  ----"));
    }

    #[test]
    fn ufw_empty_or_garbage_is_not_active() {
        assert!(!parse_ufw_active(""));
        assert!(!parse_ufw_active("ERROR: could not connect"));
    }

    #[test]
    fn backend_names_are_stable() {
        // These strings are persisted/logged and matched elsewhere; lock them.
        assert_eq!(Backend::Firewalld.name(), "firewalld");
        assert_eq!(Backend::Ufw.name(), "ufw");
        assert_eq!(Backend::Nftables.name(), "nftables");
        assert_eq!(Backend::Iptables.name(), "iptables");
        assert_eq!(Backend::None.name(), "none");
    }

    #[test]
    fn only_none_needs_no_privilege() {
        assert!(!Backend::None.needs_privilege());
        for b in [Backend::Firewalld, Backend::Ufw, Backend::Nftables, Backend::Iptables] {
            assert!(b.needs_privilege(), "{:?} should need privilege", b);
        }
    }

    #[test]
    fn default_backend_is_none() {
        assert_eq!(Backend::default(), Backend::None);
    }
}

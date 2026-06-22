//! v0.1.633: PURE pieces of the speaker sudo-shim (see
//! TODO_speaker_sudo_shim.md). Two pure decisions:
//!   1. `needs_privilege(cmd)` — does this command require elevation?
//!   2. `decision(tier, needs_priv)` — given the speaker's tier and
//!      whether the command needs privilege, allow / deny / allow-with-
//!      audit.
//!
//! Both are string/enum logic with NO side effects — buildable + testable
//! in-sandbox. The actual gating of the command-exec path + the
//! point-of-elevation audit write is build-gated wiring layered on top of
//! these (documented in the TODO); this module is the testable brain so
//! that wiring stays a thin, verifiable shim.
//!
//! Framing (from the design doc): this is ATTRIBUTION + app-level tier
//! permission, NOT OS impersonation. We never create real OS users; the
//! command runs as the logged-in OS user. The tier decides WHETHER a
//! privileged attempt is allowed at the app layer; the audit log records
//! the attempt regardless.

/// App-level permission tier for a speaker. Owner is the device holder;
/// everyone else is more restricted. Default for the sole existing
/// speaker is Owner (no behavior change on rollout).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Tier {
    /// The device owner — full rights.
    Owner,
    /// A known, trusted collaborator — may elevate, but it's logged.
    Trusted,
    /// A known guest — may run normal commands, but NOT elevate.
    Guest,
    /// Present but doesn't drive commands at all (handled upstream by
    /// `allowed=false`; included for completeness).
    Observer,
}

/// What the privileged-action gate should do.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    /// Run normally (non-privileged command, any allowed speaker).
    Allow,
    /// Privileged + permitted, but write a point-of-elevation audit entry
    /// (attribution: who asked) before running as the OS user.
    AllowWithAudit,
    /// Privileged + this tier may not elevate — refuse, and log the
    /// denied attempt.
    Deny,
}

/// Does this command require elevation? Cheap prefix/verb check on the
/// command as the user would run it. Conservative: when unsure, treat as
/// privileged (false-positive = an extra audit entry; false-negative =
/// an unlogged elevation, which is worse).
pub fn needs_privilege(cmd: &str) -> bool {
    let t = cmd.trim_start();
    // explicit elevation wrappers, as the first token
    let first = t.split_whitespace().next().unwrap_or("");
    let first = first.rsplit('/').next().unwrap_or(first); // strip any path
    if matches!(first, "sudo" | "doas" | "pkexec" | "su") {
        return true;
    }
    // common privileged management verbs even without an explicit wrapper
    // (e.g. a root shell, or distros where these need root). Match on the
    // program name only.
    matches!(
        first,
        "dnf" | "yum" | "apt" | "apt-get" | "pacman" | "zypper" | "rpm" | "dpkg"
            | "systemctl" | "service" | "mount" | "umount" | "fdisk" | "parted"
            | "useradd" | "userdel" | "usermod" | "passwd" | "visudo"
            | "iptables" | "nft" | "modprobe" | "insmod" | "rmmod"
    )
}

/// The gate decision.
pub fn decision(tier: Tier, needs_priv: bool) -> Decision {
    if !needs_priv {
        return Decision::Allow;
    }
    match tier {
        Tier::Owner => Decision::Allow, // owner elevates freely
        Tier::Trusted => Decision::AllowWithAudit, // permitted but logged
        Tier::Guest | Tier::Observer => Decision::Deny,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn explicit_wrappers_need_privilege() {
        for c in ["sudo dnf install x", "  doas reboot", "pkexec something", "su -"] {
            assert!(needs_privilege(c), "should be privileged: {:?}", c);
        }
    }

    #[test]
    fn wrapper_with_path_still_detected() {
        assert!(needs_privilege("/usr/bin/sudo dnf update"));
    }

    #[test]
    fn pkg_and_system_verbs_need_privilege() {
        for c in ["dnf install ripgrep", "systemctl restart nginx", "mount /dev/sdb /mnt", "passwd"] {
            assert!(needs_privilege(c), "should be privileged: {:?}", c);
        }
    }

    #[test]
    fn ordinary_commands_do_not() {
        for c in ["ls -la", "cat ~/notes.txt", "git status", "cargo build", "grep foo bar.txt", "echo hi"] {
            assert!(!needs_privilege(c), "should NOT be privileged: {:?}", c);
        }
    }

    #[test]
    fn substring_does_not_false_trigger() {
        // "sudoku", "applesauce", "service-worker.js" must NOT match —
        // we check the program name, not a substring.
        assert!(!needs_privilege("sudoku"));
        assert!(!needs_privilege("apt-cache-something-not-a-real-cmd")); // not exactly "apt"
        assert!(!needs_privilege("echo sudo"));  // sudo is an ARG, not the program
    }

    #[test]
    fn owner_elevates_freely() {
        assert_eq!(decision(Tier::Owner, true), Decision::Allow);
        assert_eq!(decision(Tier::Owner, false), Decision::Allow);
    }

    #[test]
    fn trusted_elevates_with_audit() {
        assert_eq!(decision(Tier::Trusted, true), Decision::AllowWithAudit);
        assert_eq!(decision(Tier::Trusted, false), Decision::Allow);
    }

    #[test]
    fn guest_and_observer_cannot_elevate() {
        assert_eq!(decision(Tier::Guest, true), Decision::Deny);
        assert_eq!(decision(Tier::Observer, true), Decision::Deny);
        // but a non-privileged command is fine for a guest
        assert_eq!(decision(Tier::Guest, false), Decision::Allow);
    }
}

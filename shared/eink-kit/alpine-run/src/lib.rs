//! The plan `alpine-run` executes, as data (testable without privileges).
//!
//! `alpine-run <root> <program> [args...]`:
//! 1. `unshare(CLONE_NEWNS)`: a private mount namespace. Every mount below
//!    vanishes when the process exits; the tablet's root filesystem, which
//!    gitmind never unlocks, is not touched.
//! 2. mark `/` recursively private, so nothing propagates back out;
//! 3. bind the host's live directories into the root (`BINDS`): devices,
//!    kernel interfaces, `/run` (the system D-Bus socket BlueZ is on),
//!    `/tmp` (AppLoad's qtfb socket), `/dev/shm` (qtfb's framebuffers) and
//!    `/home` (the user's data, and the root itself lives under it);
//! 4. copy the host's resolver config (Alpine's own is empty);
//! 5. `chroot(root)`, `chdir("/")`, exec the program with the environment
//!    unchanged (AppLoad's `QTFB_KEY`, eink-kit's `EINK_*` pass through).

/// Host path → path inside the root, in mount order (`/dev/shm` after `/dev`).
pub const BINDS: &[&str] = &["/dev", "/dev/pts", "/dev/shm", "/proc", "/sys", "/run", "/tmp", "/home"];

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Plan {
    pub root: String,
    pub program: String,
    pub args: Vec<String>,
    /// (host source, target inside the root)
    pub binds: Vec<(String, String)>,
}

pub fn plan(argv: &[String]) -> Result<Plan, String> {
    let usage = "usage: alpine-run <alpine-root> <program> [args...]";
    let raw = argv.get(1).ok_or(usage)?;
    let program = argv.get(2).ok_or(usage)?.clone();
    if !raw.starts_with('/') {
        return Err(format!("alpine root must be an absolute path: {raw:?}"));
    }
    let root = raw.trim_end_matches('/').to_string();
    if root.is_empty() {
        return Err("refusing / as the alpine root".into());
    }
    let binds = BINDS.iter().map(|b| (b.to_string(), format!("{root}{b}"))).collect();
    Ok(Plan { root, program, args: argv[3..].to_vec(), binds })
}

#[cfg(test)]
mod tests {
    use super::*;
    fn v(xs: &[&str]) -> Vec<String> { xs.iter().map(|s| s.to_string()).collect() }

    #[test]
    fn plan_binds_everything_the_apps_need_under_the_root() {
        let p = plan(&v(&["alpine-run", "/home/root/.local/share/gitmind/alpine/", "/usr/bin/bluealsa", "-p", "a2dp-source"])).unwrap();
        assert_eq!(p.root, "/home/root/.local/share/gitmind/alpine");
        assert_eq!(p.args, v(&["-p", "a2dp-source"]));
        let targets: Vec<&str> = p.binds.iter().map(|b| b.0.as_str()).collect();
        for need in ["/run", "/tmp", "/dev/shm", "/home"] {
            assert!(targets.contains(&need), "{need} (D-Bus / qtfb / data) must be bound");
        }
        let shm = targets.iter().position(|t| *t == "/dev/shm").unwrap();
        assert!(targets.iter().position(|t| *t == "/dev").unwrap() < shm, "/dev before /dev/shm");
    }

    #[test]
    fn plan_refuses_dangerous_or_missing_roots() {
        assert!(plan(&v(&["alpine-run"])).is_err());
        assert!(plan(&v(&["alpine-run", "relative/root", "sh"])).is_err());
        assert_eq!(plan(&v(&["alpine-run", "/", "sh"])).unwrap_err(), "refusing / as the alpine root");
    }
}

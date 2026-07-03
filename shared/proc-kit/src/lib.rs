//! proc-kit — run subprocesses and capture output, with a privilege-escalation ladder.
//!
//! Half the fleet's system tasks (loading v4l2loopback, installing system deps, building an
//! RPM/apk, poking `/etc`) need root *sometimes*, in a host-specific way. This crate is the
//! one place for "run this; if it turns out to need root, escalate the way this host
//! supports" — direct first, then `sudo -n` (non-interactive), then `doas`. Command
//! construction and permission-error detection are pure/testable; the runs use std.

use std::process::Command;

/// How to gain privileges on this host.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Escalation {
    /// Run as-is (already root, or the command doesn't need it).
    None,
    /// `sudo -n` — non-interactive; fails fast if a password would be required.
    Sudo,
    /// `doas` — the BSD/Alpine escalator.
    Doas,
}

impl Escalation {
    /// The argv prefix this method prepends.
    pub fn prefix(self) -> &'static [&'static str] {
        match self {
            Escalation::None => &[],
            Escalation::Sudo => &["sudo", "-n"],
            Escalation::Doas => &["doas"],
        }
    }
}

/// Wrap a command with an escalation method, returning the full argv.
pub fn escalate_argv(prog: &str, args: &[String], esc: Escalation) -> Vec<String> {
    let mut argv: Vec<String> = esc.prefix().iter().map(|s| s.to_string()).collect();
    argv.push(prog.to_string());
    argv.extend(args.iter().cloned());
    argv
}

/// Is `prog` on `$PATH`?
pub fn which(prog: &str) -> bool {
    if prog.contains('/') {
        return std::path::Path::new(prog).exists();
    }
    std::env::var_os("PATH")
        .map(|paths| std::env::split_paths(&paths).any(|d| d.join(prog).exists()))
        .unwrap_or(false)
}

/// The escalation this host supports: if we're already root, `None`; else `sudo` if present,
/// else `doas` if present, else `None` (nothing available — caller reports the failure).
pub fn detect_escalation() -> Escalation {
    if is_root() {
        return Escalation::None;
    }
    if which("sudo") {
        Escalation::Sudo
    } else if which("doas") {
        Escalation::Doas
    } else {
        Escalation::None
    }
}

fn is_root() -> bool {
    // avoid a libc dep: check the effective uid via the environment / id fallback
    std::env::var("EUID").ok().as_deref() == Some("0")
        || std::fs::read_to_string("/proc/self/status")
            .ok()
            .and_then(|s| s.lines().find(|l| l.starts_with("Uid:")).map(|l| l.split_whitespace().nth(2) == Some("0")))
            .unwrap_or(false)
}

/// True if a command's stderr looks like it failed for lack of privileges.
pub fn looks_like_permission_error(stderr: &str) -> bool {
    let s = stderr.to_lowercase();
    [
        "permission denied",
        "operation not permitted",
        "must be root",
        "are you root",
        "must be run as root",
        "you need to be root",
        "not permitted",
        "access denied",
        "requires superuser",
    ]
    .iter()
    .any(|needle| s.contains(needle))
}

/// The outcome of a run.
#[derive(Debug, Clone)]
pub struct RunOutput {
    pub status: i32,
    pub stdout: String,
    pub stderr: String,
    pub ok: bool,
}

/// Run a command and capture output. `Err` only if the process couldn't be spawned at all
/// (e.g. the program isn't installed); a non-zero exit is `Ok` with `ok=false`.
pub fn run(prog: &str, args: &[String]) -> Result<RunOutput, String> {
    let out = Command::new(prog).args(args).output().map_err(|e| {
        if e.kind() == std::io::ErrorKind::NotFound {
            format!("'{prog}' not found")
        } else {
            format!("running {prog}: {e}")
        }
    })?;
    Ok(RunOutput {
        status: out.status.code().unwrap_or(-1),
        stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
        stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        ok: out.status.success(),
    })
}

/// Run `prog args`; if it fails in a way that looks privilege-related, retry once via the
/// host's escalation method. Returns the successful run, or the most informative failure.
pub fn run_escalated(prog: &str, args: &[String]) -> Result<RunOutput, String> {
    let first = run(prog, args)?;
    if first.ok || !looks_like_permission_error(&first.stderr) {
        return Ok(first);
    }
    let esc = detect_escalation();
    if esc == Escalation::None {
        return Ok(first); // nothing to escalate with — surface the original failure
    }
    let argv = escalate_argv(prog, args, esc);
    run(&argv[0], &argv[1..].to_vec())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn escalate_argv_wraps_correctly() {
        let args = vec!["v4l2loopback".to_string(), "video_nr=10".to_string()];
        assert_eq!(escalate_argv("modprobe", &args, Escalation::None), vec!["modprobe", "v4l2loopback", "video_nr=10"]);
        assert_eq!(
            escalate_argv("modprobe", &args, Escalation::Sudo),
            vec!["sudo", "-n", "modprobe", "v4l2loopback", "video_nr=10"]
        );
        assert_eq!(
            escalate_argv("modprobe", &args, Escalation::Doas),
            vec!["doas", "modprobe", "v4l2loopback", "video_nr=10"]
        );
    }

    #[test]
    fn permission_error_detection() {
        assert!(looks_like_permission_error("modprobe: Permission denied"));
        assert!(looks_like_permission_error("Operation not permitted"));
        assert!(looks_like_permission_error("You need to be root to run this"));
        assert!(!looks_like_permission_error("no such device"));
        assert!(!looks_like_permission_error(""));
    }

    #[test]
    fn run_captures_output() {
        let out = run("echo", &["hello".to_string(), "world".to_string()]).expect("echo runs");
        assert!(out.ok);
        assert_eq!(out.stdout.trim(), "hello world");
        assert_eq!(out.status, 0);
    }

    #[test]
    fn run_missing_program_errors() {
        assert!(run("definitely-not-a-real-binary-xyz", &[]).is_err());
    }

    #[test]
    fn nonzero_exit_is_ok_but_not_success() {
        // `false` exits 1
        if which("false") {
            let out = run("false", &[]).expect("false runs");
            assert!(!out.ok);
            assert_ne!(out.status, 0);
        }
    }

    #[test]
    fn detect_escalation_returns_a_valid_variant() {
        // just ensure it doesn't panic and returns a known variant
        let e = detect_escalation();
        assert!(matches!(e, Escalation::None | Escalation::Sudo | Escalation::Doas));
    }
}

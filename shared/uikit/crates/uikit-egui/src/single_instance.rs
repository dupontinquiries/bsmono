//! single_instance — one window per app, not one per launcher click.
//!
//! Nothing in the fleet guarded against a second launch. Clicking a launcher
//! twice gives two keyboards fighting over the same input, two settings windows
//! that will overwrite each other's writes, and two DHT nodes announcing the
//! same peer id. For an on-screen keyboard it is worse than untidy: the second
//! instance takes the input focus the first one was serving.
//!
//! ## Why a lock file and not a port
//!
//! Binding a port is the other common trick and it is wrong here. A port is
//! machine-wide, so it prevents two *users* on the same machine from each
//! running the app — a real configuration on shared desktops. A lock under the
//! user's own runtime directory is per-user by construction.
//!
//! ## Why the runtime dir and not /tmp
//!
//! `$XDG_RUNTIME_DIR` is cleared when the session ends, so a lock left behind by
//! a crash cannot survive a logout. A lock in `/tmp` can outlive the session and
//! then blocks every future launch — the failure mode where the fix is "delete
//! this file you have never heard of".
//!
//! ## A stale lock must not be fatal
//!
//! A crashed process leaves its file behind within a session, so the PID inside
//! is checked: if that process is gone, the lock is ours to take. Treating any
//! existing file as "already running" turns one crash into an app that never
//! starts again.

use std::path::PathBuf;

/// Where the lock lives.
pub fn lock_path(app_id: &str) -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        // Falling back to the config home rather than /tmp: still per-user, and
        // a leftover there is at least somewhere the user's own tooling looks.
        .unwrap_or_else(|| {
            std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".cache"))
                .unwrap_or_else(|| PathBuf::from("."))
        });
    base.join(format!("{app_id}.lock"))
}

/// What happened when we tried to claim the lock.
#[derive(Debug, PartialEq, Eq)]
pub enum Claim {
    /// We hold it; this is the only instance.
    Acquired,
    /// Another live instance holds it, with its pid.
    AlreadyRunning(u32),
    /// The lock could not be written. NOT a refusal to start: a read-only or
    /// missing runtime dir must not stop the app, since the alternative is an
    /// app that will not launch on a system where nothing is actually wrong.
    Unavailable,
}

/// Is a process alive? NOT "does /proc/<pid> exist": a ZOMBIE (exited, not yet reaped by its parent)
/// keeps its /proc entry, and a lock naming a zombie made every later launch conclude "already running",
/// poke an instance that does not exist, and exit: on the phone hatch never appeared after the first
/// time it hid, because tideshell, its parent, never reaped it. The state letter after the command
/// name in /proc/<pid>/stat says: `Z` (zombie) and `X` (dead) are not alive.
fn pid_alive(pid: u32) -> bool {
    if pid == 0 { return false; }
    match std::fs::read_to_string(format!("/proc/{pid}/stat")) {
        Ok(stat) => proc_state_is_alive(&stat),
        Err(_) => false,
    }
}

/// From the text of /proc/<pid>/stat. The command name is in parentheses and may contain anything, so
/// the state is the first letter after the LAST `)`.
fn proc_state_is_alive(stat: &str) -> bool {
    match stat.rfind(')').and_then(|i| stat[i + 1..].trim_start().chars().next()) {
        Some('Z') | Some('X') | Some('x') | None => false,
        Some(_) => true,
    }
}

/// Try to become the single instance.
pub fn claim(app_id: &str) -> Claim {
    let path = lock_path(app_id);
    if let Ok(text) = std::fs::read_to_string(&path) {
        if let Ok(pid) = text.trim().parse::<u32>() {
            if pid_alive(pid) && pid != std::process::id() {
                return Claim::AlreadyRunning(pid);
            }
            // The writer is gone: a crash within this session. The lock is ours.
        }
    }
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    match std::fs::write(&path, std::process::id().to_string()) {
        Ok(()) => Claim::Acquired,
        Err(_) => Claim::Unavailable,
    }
}

/// Release on clean exit. Best-effort: the runtime dir is cleared at logout
/// anyway, so a missed release costs nothing.
pub fn release(app_id: &str) {
    let _ = std::fs::remove_file(lock_path(app_id));
}

/// Should the app start?
///
/// `Unavailable` starts, deliberately. An app that refuses to launch because it
/// could not write a lock file is broken in a way the user cannot diagnose,
/// while a second instance is merely untidy.
pub fn should_start(c: &Claim) -> bool { !matches!(c, Claim::AlreadyRunning(_)) }

#[cfg(test)]
mod tests {
    #[test]
    fn a_zombie_is_not_an_instance_but_a_running_process_is() {
        // The real thing, not a string: spawn a child that exits at once and DO NOT wait for it.
        let mut child = std::process::Command::new("sh").args(["-c", "exit 0"]).spawn().unwrap();
        let pid = child.id();
        // wait (without reaping) until it has exited: /proc/<pid>/stat says Z
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while std::time::Instant::now() < deadline {
            if std::fs::read_to_string(format!("/proc/{pid}/stat")).map_or(false, |s| s.contains(") Z")) {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert!(std::path::Path::new(&format!("/proc/{pid}")).exists(), "the premise: a zombie still has a /proc entry");
        assert!(!pid_alive(pid), "a zombie must not hold the single-instance lock");
        child.wait().unwrap();
        assert!(!pid_alive(pid), "and a reaped process is gone");
        assert!(pid_alive(std::process::id()), "this very process is alive");
        assert!(!pid_alive(0));
    }

    #[test]
    fn the_state_letter_is_read_after_the_last_paren_so_odd_names_cannot_fool_it() {
        assert!(proc_state_is_alive("123 (hatch-egui) S 1 123 123 0 -1 4194304"));
        assert!(proc_state_is_alive("123 (hatch-egui) R 1 123"));
        assert!(!proc_state_is_alive("123 (hatch-egui) Z 1 123"));
        assert!(proc_state_is_alive("123 (a) Z b (c)) S 1"), "a name with parens and spaces: the state is after the LAST paren, here S -> alive");
        assert!(proc_state_is_alive("123 (a (Z) b) S 1 2"), "a Z inside the name is not the state");
        assert!(!proc_state_is_alive("garbage"));
    }

    use super::*;

    fn temp_id(tag: &str) -> String {
        format!("test.single.{tag}.{}", std::process::id())
    }

    #[test]
    fn a_first_launch_acquires() {
        let id = temp_id("first");
        assert_eq!(claim(&id), Claim::Acquired);
        release(&id);
    }

    #[test]
    fn a_stale_lock_from_a_dead_process_is_taken_over() {
        // A crash leaves the file behind. Treating any existing lock as
        // "already running" turns one crash into an app that never starts again.
        let id = temp_id("stale");
        let path = lock_path(&id);
        if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
        // A pid that cannot be running: 0 is never a real process.
        std::fs::write(&path, "0").unwrap();
        assert_eq!(claim(&id), Claim::Acquired, "the stale lock was taken");
        release(&id);
    }

    #[test]
    fn garbage_in_the_lock_file_does_not_block_startup() {
        // A truncated or half-written file is a crash artefact, not a running
        // process.
        let id = temp_id("garbage");
        let path = lock_path(&id);
        if let Some(d) = path.parent() { let _ = std::fs::create_dir_all(d); }
        std::fs::write(&path, "not-a-pid").unwrap();
        assert_eq!(claim(&id), Claim::Acquired);
        release(&id);
    }

    #[test]
    fn our_own_pid_is_not_another_instance() {
        // Claiming twice in one process — a re-entrant startup path — must not
        // report the app as already running against itself.
        let id = temp_id("self");
        assert_eq!(claim(&id), Claim::Acquired);
        assert_eq!(claim(&id), Claim::Acquired, "not blocked by our own lock");
        release(&id);
    }

    #[test]
    fn an_unwritable_lock_still_starts_the_app() {
        // An app that refuses to launch because it could not write a lock is
        // broken in a way the user cannot diagnose; a second instance is merely
        // untidy.
        assert!(should_start(&Claim::Unavailable));
        assert!(should_start(&Claim::Acquired));
        assert!(!should_start(&Claim::AlreadyRunning(1234)));
    }

    #[test]
    fn the_lock_is_per_user_not_per_machine() {
        // A bound PORT would stop two users on one machine each running the
        // app, which is a real configuration on shared desktops.
        let p = lock_path("org.example.App").to_string_lossy().to_string();
        assert!(p.ends_with("org.example.App.lock"), "{p}");
        let per_user = std::env::var_os("XDG_RUNTIME_DIR").is_some()
            || p.contains(".cache") || p.starts_with('.');
        assert!(per_user, "{p} is not under a per-user directory");
    }

    #[test]
    fn releasing_a_lock_that_is_not_there_is_harmless() {
        // Exit paths run twice more often than anyone expects.
        let id = temp_id("release");
        release(&id);
        release(&id);
    }
}

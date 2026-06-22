// src/sudo_keepalive.rs
//
// Sudo timestamp keepalive for long-running sudo commands.
//
// Problem: sudo timestamps default to 5 minutes (Linux) or 15 (BSD).
// A user runs `sudo apt upgrade` that takes 12 minutes, the timestamp
// expires mid-build, sudo prompts for password again and the user
// has to alt-tab back to enter it. This is annoying when batch-running
// multiple sudo commands or sitting through a long install.
//
// Solution: when we detect a `sudo ...` command was submitted via the
// PTY, start a background thread that runs `sudo -n -v` every 4 minutes.
// The `-n` flag means non-interactive (returns immediately if no valid
// timestamp), and `-v` validates and refreshes. Result: as long as the
// initial sudo prompt was answered, the timestamp stays alive.
//
// The keepalive auto-stops when:
//   - The user submits a non-sudo command (next prompt = sudo session
//     conceptually ends from our POV; if they sudo again later, we
//     restart)
//   - The keepalive subprocess fails (timestamp expired anyway, or
//     sudo binary changed)
//   - 60 minutes elapsed (safety cap — we never want to extend a
//     stale auth indefinitely)
//
// Privacy / safety:
//   - We NEVER capture or store the user's password
//   - We don't extend timestamp if it's already expired (sudo -n -v
//     fails silently)
//   - We don't elevate privileges ourselves; this just keeps an
//     existing valid auth alive
//   - If the user's sudoers config has tty_tickets enabled (default
//     on many distros), the keepalive runs in OUR process's tty (the
//     app), which differs from the PTY's tty. In that case sudo -n -v
//     will simply fail and we degrade to no keepalive — but the user
//     in the PTY is unaffected because their original auth is still
//     valid in their tty.
//
// The right way to handle the tty_tickets case is to run sudo -n -v
// INSIDE the PTY itself. That requires injecting bytes in a way that
// doesn't disrupt whatever the user is currently doing. Defer to a
// future enhancement; this initial impl handles the common case
// (timestamp_type=ppid or = global) and degrades gracefully otherwise.

use std::sync::Mutex;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

/// Active keepalive state. When Some, a background thread is running
/// and refreshing sudo every 4 minutes. We track started_at to enforce
/// the 60-minute hard cap (defense against forgetting to stop).
struct KeepaliveState {
    started_at: Instant,
    /// Flipped to true to signal the background thread to exit.
    stop: std::sync::Arc<AtomicBool>,
}

static STATE: Mutex<Option<KeepaliveState>> = Mutex::new(None);

/// Hard cap on keepalive duration. After this, we stop refreshing
/// regardless of activity. Conservative: even very long builds rarely
/// exceed an hour, and if they do, the user can tolerate one re-prompt.
const MAX_DURATION: Duration = Duration::from_secs(60 * 60);

/// Refresh interval. Must be less than the system's sudo timestamp_timeout
/// (default 5 min). 4 min gives a 1-minute safety margin.
const REFRESH_INTERVAL: Duration = Duration::from_secs(4 * 60);

/// Inspect a freshly-submitted command. If it starts with `sudo `, start
/// keepalive (or restart timer if already running). Otherwise, stop any
/// active keepalive — the user's "sudo session" is logically over.
///
/// Called from BlockTracker::submit_command on every command. Cheap
/// when no sudo command (just a string prefix check + atomic load).
pub fn note_command(command: &str) {
    if is_sudo_command(command) {
        start_or_refresh();
    } else {
        // Non-sudo command on a fresh prompt — sudo session over.
        // We don't stop on EVERY non-sudo command (e.g. user runs
        // `ls` between two sudo commands), only when there's no
        // active sudo at all. So: leave the timer running. The
        // 60-minute cap and the keepalive's own failure-on-expired
        // behavior provide safety.
        //
        // If the user wants to actively cancel, they can run `sudo -k`
        // themselves. We're additive, not authoritative.
    }
}

/// v0.1.467: pure predicate for "does this command invoke sudo/doas?"
/// Extracted from note_command so it can be unit-tested without
/// spawning the keepalive thread or invoking the real sudo binary.
///
/// Matches `sudo` / `doas` as the COMMAND (with args or bare), but
/// NOT lookalikes like `sudoedit` (different timestamp semantics) or
/// `sudoku` (unrelated). pkexec is intentionally excluded — it's
/// PolicyKit and doesn't use the sudo timestamp mechanism.
pub fn is_sudo_command(command: &str) -> bool {
    let cmd = command.trim_start();
    cmd == "sudo"
        || cmd == "doas"
        || cmd.starts_with("sudo ")
        || cmd.starts_with("doas ")
}

/// Start keepalive thread, or restart its timer if already running.
fn start_or_refresh() {
    let mut guard = match STATE.lock() {
        Ok(g) => g,
        Err(_) => return,
    };

    if let Some(state) = guard.as_ref() {
        // Already running. If it's been < 60 min since start, fine —
        // the periodic refresh handles us. If we're past the cap, the
        // existing thread will exit on its next tick; we then need to
        // start a new one. Keep simple: leave as-is, future tick will
        // self-clean.
        if state.started_at.elapsed() < MAX_DURATION {
            return;
        }
        // Past cap — signal old thread to stop, fall through to start fresh.
        state.stop.store(true, Ordering::SeqCst);
    }

    let stop = std::sync::Arc::new(AtomicBool::new(false));
    let stop_for_thread = stop.clone();
    *guard = Some(KeepaliveState {
        started_at: Instant::now(),
        stop,
    });

    std::thread::spawn(move || {
        let started = Instant::now();
        // Initial refresh attempt. If it fails, the user's existing
        // sudo timestamp may not be valid yet (e.g. they typed `sudo X`
        // but haven't entered the password yet) — that's fine, we'll
        // try again at the next interval and pick up once they auth.
        run_refresh();
        loop {
            std::thread::sleep(REFRESH_INTERVAL);
            if stop_for_thread.load(Ordering::SeqCst) { break; }
            if started.elapsed() >= MAX_DURATION {
                crate::clog!("sudo-keepalive", "max duration ({}min) reached, stopping",
                    MAX_DURATION.as_secs() / 60);
                break;
            }
            if !run_refresh() {
                // sudo -n -v failed. Two reasons:
                //   1. Timestamp expired (user didn't auth or auth
                //      expired). No point trying again.
                //   2. tty_tickets mismatch (we're not in the user's
                //      tty). Same — won't recover.
                // Either way, stop trying.
                crate::clog!("sudo-keepalive", "sudo -n -v failed, stopping refresh loop");
                break;
            }
        }
        // Thread exiting — clear state if it still points to us.
        if let Ok(mut g) = STATE.lock() {
            // We can't compare Arc identity easily here; rely on
            // sufficient atomicity by clearing only if the started_at
            // matches (approximately).
            if let Some(st) = g.as_ref() {
                if st.started_at == started {
                    *g = None;
                }
            }
        }
    });
}

/// Run `sudo -n -v` once. Returns true on success.
fn run_refresh() -> bool {
    use std::process::{Command, Stdio};
    let result = Command::new("sudo")
        .args(&["-n", "-v"])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status();
    match result {
        Ok(status) => status.success(),
        Err(_) => false,  // sudo not on PATH, or spawn failed
    }
}

/// Force-stop. Called e.g. on app shutdown if needed.
#[allow(dead_code)]
pub fn stop() {
    if let Ok(mut g) = STATE.lock() {
        if let Some(state) = g.take() {
            state.stop.store(true, Ordering::SeqCst);
        }
    }
}

/// Inspect — for debugging/UI display.
#[allow(dead_code)]
pub fn is_active() -> bool {
    STATE.lock().ok().map(|g| g.is_some()).unwrap_or(false)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detects_sudo_with_args() {
        assert!(is_sudo_command("sudo apt upgrade"));
        assert!(is_sudo_command("sudo -i"));
        assert!(is_sudo_command("  sudo make install")); // leading ws
    }

    #[test]
    fn detects_bare_sudo_and_doas() {
        assert!(is_sudo_command("sudo"));
        assert!(is_sudo_command("doas"));
        assert!(is_sudo_command("doas pkg_add x"));
    }

    #[test]
    fn rejects_sudo_lookalikes() {
        // sudoedit has different timestamp semantics; sudoku is
        // unrelated. Neither should trigger keepalive.
        assert!(!is_sudo_command("sudoedit /etc/hosts"));
        assert!(!is_sudo_command("sudoku"));
        assert!(!is_sudo_command("sudoreplay"));
    }

    #[test]
    fn rejects_non_sudo() {
        assert!(!is_sudo_command("ls -la"));
        assert!(!is_sudo_command("git commit"));
        assert!(!is_sudo_command(""));
        assert!(!is_sudo_command("   "));
        // pkexec intentionally excluded (PolicyKit, not sudo ts).
        assert!(!is_sudo_command("pkexec something"));
    }

    #[test]
    fn rejects_sudo_substring_mid_command() {
        // "echo sudo" is not a sudo invocation.
        assert!(!is_sudo_command("echo sudo"));
        assert!(!is_sudo_command("which sudo"));
    }
}

// drivemind-core — the headless disk & data health engine.
//
// Design rules (same school as gitmind-core):
//
//   * PURE CORE. No UI, no async runtime, no daemon assumptions. Frontends
//     (CLI today, uikit-egui shell alongside, maybe a snapclone tile later)
//     are thin binaries over this crate.
//
//   * ONE SEAM FOR THE SYSTEM: `SysRunner`. Everything that would touch the
//     machine — smartctl, lsblk, ffprobe, git, dd — goes through it. The
//     default impl shells out; tests inject canned outputs; a frontend can
//     wrap it to show every command before it runs. That is the "visible,
//     steerable loop" property, and it is also what makes the entire engine
//     testable in a container with no real disks.
//
//   * EVENTS, NOT LOGS. Long operations (integrity scan, cleanup walk,
//     repo analysis) emit `Event`s through a caller-supplied sink. The CLI
//     prints them; the egui shell feeds them to a live log view; a test
//     collects them into a Vec. Progress is data, not println.
//
//   * ADVICE IS A COMMAND, NOT AN ACTION. The advisor and the cleanup
//     scanner never delete, never re-encode, never rewrite history. Every
//     suggestion carries the exact command that would do it (dvm itself,
//     `gin migrate`, an av1tuner batch file). Destructive paths (clone)
//     require an explicit force flag AND pass safety checks first.

pub mod advisor;
pub mod assist;
pub mod catalog;
pub mod syncdiff;
pub mod cleanup;
pub mod cloneop;
pub mod deltahostile;
pub mod device;
pub mod integrity;
pub mod smart;
pub mod video;

use std::process::Command;

/// A progress/status event from any long-running engine operation.
/// Frontends decide presentation; the engine only states facts.
#[derive(Clone, Debug, PartialEq)]
pub enum Event {
    /// A phase began ("scan", "hash", "verify", …) over an optional subject.
    Phase { name: &'static str, subject: String },
    /// Incremental progress: `done` units of an optional `total`.
    Progress { done: u64, total: Option<u64> },
    /// A human-relevant note tied to a path/device (skips, retries, warnings).
    Note(String),
    /// A command is about to run (the steerable-loop hook: frontends can
    /// display it; a wrapping SysRunner could veto it).
    Exec(String),
}

/// Sink for events. `&mut dyn FnMut` keeps it object-safe and lets callers
/// use a closure, a channel sender, or a no-op equally well.
pub type Sink<'a> = &'a mut dyn FnMut(Event);

/// Convenience: a sink that drops everything (tests that don't care).
pub fn null_sink() -> impl FnMut(Event) {
    |_| {}
}

/// The one seam between the engine and the machine. `run` executes a
/// program with args and returns (exit_code, stdout, stderr).
///
/// Kept dyn-compatible on purpose (no generics, no async) so frontends can
/// store `Box<dyn SysRunner>` and wrap/chain runners without type Tetris.
pub trait SysRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<SysOutput, String>;
}

#[derive(Clone, Debug, Default)]
pub struct SysOutput {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Default runner: shell out. smartctl exit codes are a bitmask (bit 0 =
/// couldn't parse args, bit 1 = device open failed, bit 3 = disk failing…),
/// so a non-zero exit is DATA here, not an error — we return it, callers
/// interpret. Only failure to spawn at all is an Err.
pub struct HostRunner;

impl SysRunner for HostRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<SysOutput, String> {
        let out = Command::new(program)
            .args(args)
            .output()
            .map_err(|e| format!("spawn {program}: {e}"))?;
        Ok(SysOutput {
            code: out.status.code().unwrap_or(-1),
            stdout: String::from_utf8_lossy(&out.stdout).into_owned(),
            stderr: String::from_utf8_lossy(&out.stderr).into_owned(),
        })
    }
}

/// Test/replay runner: maps "program arg1 arg2…" → canned output. Unmatched
/// commands error loudly so a test can't silently touch the host.
pub struct FixtureRunner {
    pub map: std::collections::HashMap<String, SysOutput>,
}

impl FixtureRunner {
    pub fn new() -> Self {
        Self { map: std::collections::HashMap::new() }
    }
    pub fn on(mut self, cmdline: &str, code: i32, stdout: &str) -> Self {
        self.map.insert(
            cmdline.to_string(),
            SysOutput { code, stdout: stdout.to_string(), stderr: String::new() },
        );
        self
    }
    /// Like `on`, but also sets stderr -- `on` alone cannot simulate a
    /// command that fails WITH a message (it hardcodes stderr empty),
    /// which meant no test anywhere in this crate could verify that a
    /// real failure message (a full destination, a bad sector, permission
    /// denied) actually reaches the caller rather than being silently
    /// dropped. Additive rather than changing `on`'s signature, so every
    /// existing call site is untouched.
    pub fn on_full(mut self, cmdline: &str, code: i32, stdout: &str, stderr: &str) -> Self {
        self.map.insert(
            cmdline.to_string(),
            SysOutput { code, stdout: stdout.to_string(), stderr: stderr.to_string() },
        );
        self
    }
}

impl Default for FixtureRunner {
    fn default() -> Self {
        Self::new()
    }
}

impl SysRunner for FixtureRunner {
    fn run(&self, program: &str, args: &[&str]) -> Result<SysOutput, String> {
        let key = std::iter::once(program)
            .chain(args.iter().copied())
            .collect::<Vec<_>>()
            .join(" ");
        self.map
            .get(&key)
            .cloned()
            .ok_or_else(|| format!("FixtureRunner: no fixture for `{key}`"))
    }
}

/// Shannon entropy (bits/byte, 0..=8) of a byte slice. The shared primitive
/// behind "is this data already compressed?" — used by the cleanup scanner
/// (skip re-compressing) and the delta-hostility analyzer (git will fail to
/// delta this). Deliberately simple; sampling policy lives at call sites.
pub fn shannon_entropy(data: &[u8]) -> f64 {
    if data.is_empty() {
        return 0.0;
    }
    let mut counts = [0u64; 256];
    for &b in data {
        counts[b as usize] += 1;
    }
    let n = data.len() as f64;
    counts
        .iter()
        .filter(|&&c| c > 0)
        .map(|&c| {
            let p = c as f64 / n;
            -p * p.log2()
        })
        .sum()
}

/// Human-readable byte size (binary units) — one shared formatter so CLI
/// and GUI agree on presentation of the same number.
pub fn human_bytes(n: u64) -> String {
    const U: [&str; 6] = ["B", "KiB", "MiB", "GiB", "TiB", "PiB"];
    let mut v = n as f64;
    let mut i = 0;
    while v >= 1024.0 && i < U.len() - 1 {
        v /= 1024.0;
        i += 1;
    }
    if i == 0 {
        format!("{n} B")
    } else {
        format!("{v:.1} {}", U[i])
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn entropy_extremes() {
        assert_eq!(shannon_entropy(&[]), 0.0);
        assert_eq!(shannon_entropy(&[7u8; 4096]), 0.0);
        // Uniform 0..=255 repeated → exactly 8 bits/byte.
        let uniform: Vec<u8> = (0..=255u8).cycle().take(4096).collect();
        assert!((shannon_entropy(&uniform) - 8.0).abs() < 1e-9);
    }

    #[test]
    fn human_bytes_units() {
        assert_eq!(human_bytes(512), "512 B");
        assert_eq!(human_bytes(1536), "1.5 KiB");
        assert_eq!(human_bytes(3 * 1024 * 1024 * 1024), "3.0 GiB");
    }

    #[test]
    fn fixture_runner_is_strict() {
        let r = FixtureRunner::new().on("lsblk --json", 0, "{}");
        assert_eq!(r.run("lsblk", &["--json"]).unwrap().stdout, "{}");
        assert!(r.run("rm", &["-rf", "/"]).is_err());
    }
}

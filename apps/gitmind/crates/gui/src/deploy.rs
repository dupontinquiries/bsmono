//! Deploy helpers for the gitmind GUI.
//!
//! These power the GUI deploy flow: discover SSH devices on the LAN, browse the
//! remote filesystem (the folder picker), push the monorepo working tree, and run
//! a remote build + `gin app install --desktop` so a launcher icon appears on the
//! device (e.g. Plasma Mobile). Pure `std` — no Qt — so the logic unit-tests in
//! isolation; the `Backend` qt_methods are thin wrappers over the functions here.

use std::net::{IpAddr, SocketAddr, TcpStream, UdpSocket};
use std::process::Command;
use std::sync::mpsc;
use std::sync::{Arc, Mutex};
use std::time::Duration;

/// One discovered host on the LAN with TCP 22 reachable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Device {
    pub ip: String,
    pub hostname: String,
}

// ───────────────────────── discovery ─────────────────────────

/// Derive the local /24 prefix (e.g. "192.168.1") by asking the OS which source
/// address it would use to reach a public IP. `UdpSocket::connect` only sets the
/// default route on the socket; no packets are sent.
pub fn local_subnet24() -> Option<String> {
    let sock = UdpSocket::bind("0.0.0.0:0").ok()?;
    sock.connect("8.8.8.8:80").ok()?;
    match sock.local_addr().ok()?.ip() {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            Some(format!("{}.{}.{}", o[0], o[1], o[2]))
        }
        IpAddr::V6(_) => None,
    }
}

/// Scan a /24 prefix for hosts with TCP 22 open. Threaded so a full /24 (254
/// hosts) finishes in roughly `timeout * 254 / threads`.
pub fn scan_ssh(prefix: &str, timeout: Duration, threads: usize) -> Vec<Device> {
    let hosts: Vec<String> = (1..=254).map(|i| format!("{prefix}.{i}")).collect();
    let queue = Arc::new(Mutex::new(hosts.into_iter()));
    let (tx, rx) = mpsc::channel::<String>();
    let mut handles = Vec::new();
    for _ in 0..threads.max(1) {
        let queue = Arc::clone(&queue);
        let tx = tx.clone();
        handles.push(std::thread::spawn(move || loop {
            let host = {
                let mut g = queue.lock().unwrap();
                g.next()
            };
            let Some(host) = host else { break };
            let Ok(addr) = format!("{host}:22").parse::<SocketAddr>() else { continue };
            if TcpStream::connect_timeout(&addr, timeout).is_ok() {
                let _ = tx.send(host);
            }
        }));
    }
    drop(tx);
    let mut found: Vec<Device> = rx
        .iter()
        .map(|ip| {
            let hostname = reverse_name(&ip).unwrap_or_default();
            Device { ip, hostname }
        })
        .collect();
    for h in handles {
        let _ = h.join();
    }
    found.sort_by_key(|d| last_octet(&d.ip));
    found
}

fn last_octet(ip: &str) -> u8 {
    ip.rsplit('.').next().and_then(|s| s.parse().ok()).unwrap_or(0)
}

/// Best-effort hostname for an IP: try mDNS (avahi-resolve), then getent/DNS.
/// Returns None if neither resolves — the GUI just shows the bare IP then.
fn reverse_name(ip: &str) -> Option<String> {
    if let Ok(out) = Command::new("avahi-resolve-address").arg(ip).output() {
        if out.status.success() {
            if let Some(name) = String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1) {
                if !name.is_empty() {
                    return Some(name.to_string());
                }
            }
        }
    }
    if let Ok(out) = Command::new("getent").args(["hosts", ip]).output() {
        if out.status.success() {
            if let Some(name) = String::from_utf8_lossy(&out.stdout).split_whitespace().nth(1) {
                return Some(name.to_string());
            }
        }
    }
    None
}

// ───────────────────────── remote folder picker ─────────────────────────

/// Single-quote a string for safe embedding in a remote shell command.
pub fn shell_quote(s: &str) -> String {
    format!("'{}'", s.replace('\'', "'\\''"))
}

/// Build the ssh argv that resolves `path` (empty/"~" = $HOME) and lists its
/// subdirectories: emits `pwd` (the absolute path) then the `ls -1Ap` listing.
/// Separated out so it can be asserted without touching the network.
pub fn list_remote_argv(user: &str, host: &str, path: &str) -> Vec<String> {
    let p = path.trim();
    let target = if p.is_empty() || p == "~" { "$HOME".to_string() } else { shell_quote(p) };
    vec![
        format!("{user}@{host}"),
        format!("cd {target} 2>/dev/null && pwd && ls -1Ap"),
    ]
}

/// List subdirectories of `path` on the device (over SSH) for the folder picker.
/// Returns the resolved absolute path and the subdirectory names (no "." / "..").
pub fn list_remote_dirs(user: &str, host: &str, path: &str) -> Result<(String, Vec<String>), String> {
    let out = Command::new("ssh")
        .args(list_remote_argv(user, host, path))
        .output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(if msg.is_empty() { "ssh: could not list that directory".into() } else { msg.into_owned() });
    }
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let pwd = lines.next().unwrap_or("").trim().to_string();
    let dirs = parse_dir_listing(&lines.collect::<Vec<_>>().join("\n"));
    Ok((pwd, dirs))
}

/// Parse `ls -1Ap` output into directory names. Pure — unit-tested.
pub fn parse_dir_listing(stdout: &str) -> Vec<String> {
    stdout
        .lines()
        .filter(|l| l.ends_with('/'))
        .map(|l| l.trim_end_matches('/'))
        .filter(|l| !l.is_empty() && *l != "." && *l != "..")
        .map(|l| l.to_string())
        .collect()
}

// ───────────────────────── push + remote deploy ─────────────────────────

/// rsync argv to push the monorepo working tree (minus build artifacts) to the
/// device. Pure builder so the exact flags are unit-tested.
pub fn push_argv(repo: &str, user: &str, host: &str, dir: &str) -> Vec<String> {
    let src = format!("{}/", repo.trim_end_matches('/'));
    let dest = format!("{user}@{host}:{}/", dir.trim_end_matches('/'));
    vec![
        "-az".into(),
        "--delete".into(),
        "--exclude".into(),
        "target/".into(),
        "--exclude".into(),
        "*-linux-x64".into(),
        "--exclude".into(),
        "*-linux-aarch64".into(),
        src,
        dest,
    ]
}

/// Push the monorepo to `dir` on the device via rsync-over-ssh. The device needs
/// `rsync` (`apk add rsync`); the working tree (including .git, so `gin import`
/// works there) is synced, build artifacts excluded.
pub fn push_repo(repo: &str, user: &str, host: &str, dir: &str) -> Result<String, String> {
    // ensure the destination exists first (rsync won't mkdir -p the final dir reliably)
    let _ = Command::new("ssh")
        .args([&format!("{user}@{host}"), &format!("mkdir -p -- {}", shell_quote(dir))])
        .status();
    let status = Command::new("rsync")
        .args(push_argv(repo, user, host, dir))
        .status()
        .map_err(|e| format!("rsync not available on the laptop? ({e})"))?;
    if status.success() {
        Ok(format!("pushed {repo} → {user}@{host}:{dir}"))
    } else {
        Err(format!("rsync exited with {:?}", status.code()))
    }
}

/// The remote shell command that builds an app from the pushed tree and registers
/// it with a .desktop launcher. Pure builder — unit-tested.
pub fn remote_deploy_cmd(dir: &str, app: &str) -> String {
    let (appdir, bin) = if app == "gin" { ("apps/gitmind", "gin") } else { ("", app) };
    let appdir = if appdir.is_empty() { format!("apps/{app}") } else { appdir.to_string() };
    format!(
        "cd {dir}/{appdir} && cargo build --release --bin {bin} && \
         ( cd {dir} && gin import >/dev/null 2>&1 || true ) && \
         gin app install {app} target/release/{bin} --use --desktop"
    )
}

/// On the device: build `app` from the pushed tree and install it with a .desktop
/// launcher (visible on Plasma Mobile). Requires the app's build deps already
/// present on the device (rust + the GUI toolkit; see deploy.sh header).
pub fn deploy_app(user: &str, host: &str, dir: &str, app: &str) -> Result<String, String> {
    let cmd = remote_deploy_cmd(dir, app);
    let status = Command::new("ssh")
        .args([&format!("{user}@{host}"), &cmd])
        .status()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if status.success() {
        Ok(format!("{app} built + installed on {host}; launcher created"))
    } else {
        Err(format!("remote build/install failed (exit {:?})", status.code()))
    }
}

// ───────────────────────── background jobs ─────────────────────────

/// Shared state for one background deploy job (discover / push / deploy).
///
/// The slow operations — a /24 scan, an rsync, a remote build — must not run on
/// the GUI thread or the whole window freezes. The Backend spawns a worker thread
/// that calls [`Job::finish`] when done; the QML side polls [`Job::poll`] on a
/// timer, so the UI stays live and the busy indicator can actually animate.
#[derive(Default)]
pub struct Job {
    inner: Mutex<JobInner>,
}

#[derive(Default, Clone)]
struct JobInner {
    running: bool,
    kind: String,
    done: bool,
    result: String,
}

impl Job {
    /// Claim the job for `kind`. Returns false if one is already running, so the
    /// caller can refuse to start a second concurrent operation.
    pub fn start(&self, kind: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.running {
            return false;
        }
        *g = JobInner { running: true, kind: kind.to_string(), done: false, result: String::new() };
        true
    }

    /// Worker thread reports the finished JSON result.
    pub fn finish(&self, result: String) {
        let mut g = self.inner.lock().unwrap();
        g.running = false;
        g.done = true;
        g.result = result;
    }

    /// GUI poll → (running, done, kind, result). When `done`, the state is
    /// consumed (reset to idle) so a result is delivered to the UI exactly once.
    pub fn poll(&self) -> (bool, bool, String, String) {
        let mut g = self.inner.lock().unwrap();
        let out = (g.running, g.done, g.kind.clone(), g.result.clone());
        if g.done {
            *g = JobInner::default();
        }
        out
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subnet_format_is_three_octets_or_none() {
        // env-dependent, but must never panic and, if Some, be a 3-octet prefix
        if let Some(p) = local_subnet24() {
            assert_eq!(p.split('.').count(), 3, "prefix should be a /24 like 192.168.1");
            assert!(p.split('.').all(|o| o.parse::<u8>().is_ok()));
        }
    }

    #[test]
    fn last_octet_parses() {
        assert_eq!(last_octet("192.168.1.42"), 42);
        assert_eq!(last_octet("10.0.0.1"), 1);
        assert_eq!(last_octet("garbage"), 0);
    }

    #[test]
    fn shell_quote_escapes_single_quotes() {
        assert_eq!(shell_quote("/home/user"), "'/home/user'");
        assert_eq!(shell_quote("a'b"), "'a'\\''b'");
    }

    #[test]
    fn dir_listing_keeps_only_subdirs() {
        let ls = "./\n../\napps/\nbuild/\nREADME.md\n.gitignore\nshared/\n";
        assert_eq!(parse_dir_listing(ls), vec!["apps", "build", "shared"]);
    }

    #[test]
    fn list_remote_argv_quotes_and_resolves_home() {
        let a = list_remote_argv("user", "fp5.local", "/home/user/my repo");
        assert_eq!(a[0], "user@fp5.local");
        assert!(a[1].contains("'/home/user/my repo'"), "path must be quoted: {}", a[1]);
        assert!(a[1].contains("pwd && ls"), "must emit pwd then listing");
        let h = list_remote_argv("user", "fp5.local", "");
        assert!(h[1].contains("$HOME"), "empty path -> $HOME: {}", h[1]);
    }

    #[test]
    fn push_argv_excludes_build_artifacts_and_trails_slashes() {
        let a = push_argv("/home/user/monorepo/", "user", "fp5.local", "/home/user/dst/");
        assert!(a.contains(&"target/".to_string()), "must exclude target/");
        assert!(a.iter().any(|x| x == "/home/user/monorepo/"), "src keeps one trailing slash");
        assert!(a.iter().any(|x| x == "user@fp5.local:/home/user/dst/"), "dest formatted");
    }

    #[test]
    fn remote_deploy_cmd_builds_and_makes_desktop() {
        let c = remote_deploy_cmd("/home/user/monorepo", "voiceterm");
        assert!(c.contains("cd /home/user/monorepo/apps/voiceterm"));
        assert!(c.contains("cargo build --release --bin voiceterm"));
        assert!(c.contains("gin app install voiceterm"));
        assert!(c.contains("--desktop"), "must create the launcher");
        // gin special-cases to apps/gitmind
        let g = remote_deploy_cmd("/repo", "gin");
        assert!(g.contains("cd /repo/apps/gitmind"));
        assert!(g.contains("--bin gin"));
    }

    #[test]
    fn job_runs_async_and_delivers_result_once() {
        use std::sync::Arc;
        use std::time::Duration;
        let job = Arc::new(Job::default());
        // idle to start
        let (r, d, _, _) = job.poll();
        assert!(!r && !d, "starts idle");
        // claim it; a second claim is refused while running
        assert!(job.start("discover"), "first claim wins");
        assert!(!job.start("push"), "second claim refused while a job runs");
        // worker finishes off-thread
        let j2 = Arc::clone(&job);
        let h = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(40));
            j2.finish(r#"{"ok":true}"#.to_string());
        });
        // poll until done — `running` stays true until the worker finishes
        let mut got = None;
        for _ in 0..400 {
            let (running, done, kind, result) = job.poll();
            if done {
                got = Some((kind, result));
                break;
            }
            assert!(running, "running until the worker finishes");
            std::thread::sleep(Duration::from_millis(5));
        }
        h.join().unwrap();
        let (kind, result) = got.expect("job should finish");
        assert_eq!(kind, "discover");
        assert_eq!(result, r#"{"ok":true}"#);
        // consumed: idle again, and ready for the next job
        let (r2, d2, _, _) = job.poll();
        assert!(!r2 && !d2, "idle after the result is consumed");
        assert!(job.start("push"), "can start a new job after consume");
    }
}

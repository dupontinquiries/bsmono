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

/// SSH options applied to *every* ssh / rsync / git-over-ssh call the GUI makes,
/// so a connection can never block a worker (or the UI) thread on an interactive
/// prompt — which was freezing the GUI:
///   `BatchMode=yes`                    → never prompt for a password (fail fast).
///   `ConnectTimeout=8`                 → never hang on an unreachable host.
///   `StrictHostKeyChecking=accept-new` → trust a *new* host's key automatically,
///       writing it to the shared `~/.ssh/known_hosts`, while still refusing a key
///       that has *changed*. This is what lets the GUI manage host-key trust
///       without a terminal prompt (no more known_hosts split between the two).
pub const SSH_OPTS: [&str; 6] = [
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=8",
    "-o", "StrictHostKeyChecking=accept-new",
];

/// `ssh <SSH_OPTS> user@host <remote_cmd>` as an argv (no shell).
fn ssh_argv(user: &str, host: &str, remote_cmd: &str) -> Vec<String> {
    let mut a: Vec<String> = SSH_OPTS.iter().map(|s| (*s).to_string()).collect();
    a.push(format!("{user}@{host}"));
    a.push(remote_cmd.to_string());
    a
}

/// The ssh command (with opts) as one string, for rsync `-e` / `GIT_SSH_COMMAND`.
fn ssh_command_str() -> String {
    format!("ssh {}", SSH_OPTS.join(" "))
}

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
    ssh_argv(user, host, &format!("cd {target} 2>/dev/null && pwd && ls -1Ap"))
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
        "-e".into(),
        ssh_command_str(),
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
        .args(ssh_argv(user, host, &format!("mkdir -p -- {}", shell_quote(dir))))
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
        .args(ssh_argv(user, host, &cmd))
        .status()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if status.success() {
        Ok(format!("{app} built + installed on {host}; launcher created"))
    } else {
        Err(format!("remote build/install failed (exit {:?})", status.code()))
    }
}

// ───────────────────────── host-key trust ─────────────────────────

/// Is `host` already in `~/.ssh/known_hosts`? Fast + local (`ssh-keygen -F`), so
/// it's safe to call straight from the UI thread to render a trust badge.
pub fn host_trusted(host: &str) -> bool {
    let h = host.trim();
    if h.is_empty() {
        return false;
    }
    Command::new("ssh-keygen")
        .args(["-F", h])
        .output()
        .map(|o| o.status.success() && !o.stdout.is_empty())
        .unwrap_or(false)
}

/// Drop comment (`#…`) and blank lines from `ssh-keyscan` output, leaving only the
/// host-key lines we append to known_hosts. Pure — unit-tested.
fn filter_scanned_keys(scan: &str) -> String {
    scan.lines()
        .filter(|l| {
            let t = l.trim_start();
            !t.is_empty() && !t.starts_with('#')
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Pull the fingerprint field (e.g. `SHA256:…`) out of `ssh-keygen -lf -` output.
/// Pure — unit-tested.
fn parse_fingerprint(out: &str) -> Option<String> {
    out.lines()
        .next()
        .and_then(|l| l.split_whitespace().nth(1))
        .map(|s| s.to_string())
}

/// Fetch the fingerprint of the keys `ssh-keyscan` returned (for display), by
/// piping them through `ssh-keygen -lf -`.
fn fingerprint_of(keys: &str) -> Option<String> {
    use std::io::Write;
    let mut child = Command::new("ssh-keygen")
        .args(["-lf", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(keys.as_bytes()).ok()?; // dropped here → EOF
    let out = child.wait_with_output().ok()?;
    parse_fingerprint(&String::from_utf8_lossy(&out.stdout))
}

/// Trust `host`'s SSH key by fetching it (`ssh-keyscan`, bounded by its own
/// timeout so this can't hang) and appending it to the shared `~/.ssh/known_hosts`
/// — the GUI's "trust this key" action. Returns the fingerprint that was trusted.
/// Doing this in one place (the GUI, one known_hosts) is what avoids the lock /
/// split you get from accepting keys in the terminal *and* the GUI separately.
pub fn trust_host(host: &str) -> Result<String, String> {
    let h = host.trim();
    if h.is_empty() {
        return Err("no host given".into());
    }
    if host_trusted(h) {
        return Ok(format!("{h} is already trusted"));
    }
    let scan = Command::new("ssh-keyscan")
        .args(["-T", "6", h])
        .output()
        .map_err(|e| format!("ssh-keyscan failed: {e}"))?;
    let keys = filter_scanned_keys(&String::from_utf8_lossy(&scan.stdout));
    if keys.trim().is_empty() {
        return Err(format!("no SSH key returned for {h} (unreachable, or no sshd there?)"));
    }
    let fp = fingerprint_of(&keys).unwrap_or_else(|| "key".to_string());
    let home = std::env::var("HOME").map_err(|_| "no $HOME set".to_string())?;
    let ssh_dir = format!("{home}/.ssh");
    std::fs::create_dir_all(&ssh_dir).map_err(|e| format!("create {ssh_dir}: {e}"))?;
    let known = format!("{ssh_dir}/known_hosts");
    use std::io::Write;
    let mut f = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&known)
        .map_err(|e| format!("open {known}: {e}"))?;
    writeln!(f, "{}", keys.trim_end()).map_err(|e| format!("write {known}: {e}"))?;
    Ok(format!("trusted {h} ({fp})"))
}

/// Remove every `~/.ssh/known_hosts` entry for `host` (`ssh-keygen -R`). Used by
/// re-trust to clear a stale / conflicting key before re-adding the current one.
pub fn untrust_host(host: &str) -> Result<(), String> {
    let h = host.trim();
    if h.is_empty() {
        return Err("no host".into());
    }
    // `-R` exits 0 even when there was nothing to remove; only a real error fails.
    let out = Command::new("ssh-keygen")
        .args(["-R", h])
        .output()
        .map_err(|e| format!("ssh-keygen -R: {e}"))?;
    if out.status.success() {
        Ok(())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// Clear `host`'s known_hosts entry and re-fetch its current key. Fixes a host
/// whose key changed or got into a bad state (e.g. the terminal/GUI split) — the
/// case `StrictHostKeyChecking=accept-new` deliberately *won't* auto-resolve.
pub fn retrust_host(host: &str) -> Result<String, String> {
    untrust_host(host)?;
    Ok(format!("re-{}", trust_host(host)?))
}

// ───────────────────────── ssh auth + key setup ─────────────────────────

/// Coarse classification of an ssh failure so the GUI can offer the right fix:
///   "auth"        — login/key rejected → offer "set up SSH key"
///   "hostkey"     — known_hosts conflict → offer "re-trust key"
///   "unreachable" — DNS / timeout / refused → nothing to fix here
///   "other"       — anything else (show the raw message)
/// Pure — unit-tested.
pub fn ssh_error_reason(stderr: &str) -> &'static str {
    let s = stderr.to_lowercase();
    if s.contains("permission denied")
        || s.contains("authentication failed")
        || s.contains("no more authentication methods")
    {
        "auth"
    } else if s.contains("host key verification failed")
        || s.contains("remote host identification has changed")
    {
        "hostkey"
    } else if s.contains("could not resolve")
        || s.contains("name or service not known")
        || s.contains("timed out")
        || s.contains("connection refused")
        || s.contains("no route to host")
    {
        "unreachable"
    } else {
        "other"
    }
}

/// The local public key, generating an ed25519 keypair (no passphrase) if the
/// user has none yet. Returns the *public* key text to install on a device.
pub fn ensure_local_pubkey() -> Result<String, String> {
    let home = std::env::var("HOME").map_err(|_| "no $HOME set".to_string())?;
    let ssh_dir = format!("{home}/.ssh");
    std::fs::create_dir_all(&ssh_dir).map_err(|e| format!("create {ssh_dir}: {e}"))?;
    for name in ["id_ed25519", "id_rsa", "id_ecdsa"] {
        let pubp = format!("{ssh_dir}/{name}.pub");
        if std::path::Path::new(&pubp).exists() {
            return std::fs::read_to_string(&pubp).map_err(|e| format!("read {pubp}: {e}"));
        }
    }
    let keyp = format!("{ssh_dir}/id_ed25519");
    let st = Command::new("ssh-keygen")
        .args(["-t", "ed25519", "-N", "", "-C", "gitmind-fleet", "-f", &keyp])
        .status()
        .map_err(|e| format!("ssh-keygen: {e}"))?;
    if !st.success() {
        return Err("ssh-keygen could not create a key".into());
    }
    std::fs::read_to_string(format!("{keyp}.pub")).map_err(|e| format!("read new pubkey: {e}"))
}

/// Body of the temporary SSH_ASKPASS helper: echo the password the GUI put in the
/// `GM_SSH_PW` env var, so ssh can read it without a TTY. Constant — unit-tested.
const ASKPASS_BODY: &str = "#!/bin/sh\nprintf '%s\\n' \"$GM_SSH_PW\"\n";

/// ssh argv (without the trailing remote command) used to install a key via
/// *password* auth: force password (the key isn't on the device yet) and
/// auto-trust the host key. Pure — unit-tested.
fn setup_key_argv(user: &str, host: &str) -> Vec<String> {
    vec![
        "-o".into(), "PubkeyAuthentication=no".into(),
        "-o".into(), "PreferredAuthentications=password".into(),
        "-o".into(), "ConnectTimeout=8".into(),
        "-o".into(), "StrictHostKeyChecking=accept-new".into(),
        format!("{user}@{host}"),
    ]
}

/// Install our public key into `user@host`'s `authorized_keys` using password auth,
/// fed non-interactively through SSH_ASKPASS (the GUI has no TTY). This is the
/// "set up SSH key" wizard: you enter the device password *once*, then every later
/// connection is passwordless (BatchMode key auth works). Returns a status note.
pub fn setup_key(user: &str, host: &str, password: &str) -> Result<String, String> {
    if user.trim().is_empty() || host.trim().is_empty() {
        return Err("user and host are required".into());
    }
    if password.is_empty() {
        return Err("enter the device password to install the key".into());
    }
    let pubkey = ensure_local_pubkey()?;
    // temp askpass helper (0700, current pid in the name, removed after)
    let script = std::env::temp_dir().join(format!("gm-askpass-{}", std::process::id()));
    std::fs::write(&script, ASKPASS_BODY).map_err(|e| format!("write askpass: {e}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("chmod askpass: {e}"))?;
    }
    let remote = "umask 077; mkdir -p ~/.ssh && cat >> ~/.ssh/authorized_keys && echo gitmind-key-installed";
    let spawned = Command::new("ssh")
        .env("GM_SSH_PW", password)
        .env("SSH_ASKPASS", &script)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()))
        .args(setup_key_argv(user, host))
        .arg(remote)
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn();
    let result = (|| -> Result<String, String> {
        let mut child = spawned.map_err(|e| format!("ssh: {e}"))?;
        use std::io::Write;
        child
            .stdin
            .take()
            .ok_or("ssh stdin unavailable")?
            .write_all(pubkey.as_bytes())
            .map_err(|e| format!("send key: {e}"))?;
        let out = child.wait_with_output().map_err(|e| format!("ssh wait: {e}"))?;
        if out.status.success() && String::from_utf8_lossy(&out.stdout).contains("gitmind-key-installed") {
            Ok(format!("installed your key on {user}@{host} — connections are passwordless now"))
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            match ssh_error_reason(&err) {
                "auth" => Err(format!("password rejected for {user}@{host}")),
                _ if err.trim().is_empty() => Err("key install failed".into()),
                _ => Err(err.trim().to_string()),
            }
        }
    })();
    let _ = std::fs::remove_file(&script); // best-effort cleanup, even on error
    result
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
        assert!(a.iter().any(|x| x == "user@fp5.local"), "target present: {a:?}");
        assert!(a.iter().any(|x| x.contains("'/home/user/my repo'")), "path must be quoted: {a:?}");
        assert!(a.iter().any(|x| x.contains("pwd && ls")), "must emit pwd then listing");
        // and it must be non-interactive so a prompt can't freeze the picker
        assert!(a.windows(2).any(|w| w[0] == "-o" && w[1] == "BatchMode=yes"), "BatchMode set: {a:?}");
        assert!(a.contains(&"StrictHostKeyChecking=accept-new".to_string()), "auto-trusts new hosts");
        let h = list_remote_argv("user", "fp5.local", "");
        assert!(h.iter().any(|x| x.contains("$HOME")), "empty path -> $HOME: {h:?}");
    }

    #[test]
    fn push_argv_excludes_build_artifacts_and_trails_slashes() {
        let a = push_argv("/home/user/monorepo/", "user", "fp5.local", "/home/user/dst/");
        assert!(a.contains(&"target/".to_string()), "must exclude target/");
        assert!(a.iter().any(|x| x == "/home/user/monorepo/"), "src keeps one trailing slash");
        assert!(a.iter().any(|x| x == "user@fp5.local:/home/user/dst/"), "dest formatted");
        // rsync must use the same non-interactive ssh
        assert!(
            a.windows(2).any(|w| w[0] == "-e" && w[1].starts_with("ssh ") && w[1].contains("BatchMode=yes")),
            "rsync -e ssh must be non-interactive: {a:?}"
        );
    }

    #[test]
    fn scan_filtering_and_fingerprint_parsing() {
        let scan = "# fp5.local:22 SSH-2.0-OpenSSH_9\nfp5.local ssh-ed25519 AAAAC3Nz\n\n# fp5.local:22 SSH-2.0\nfp5.local ssh-rsa AAAAB3Nz\n";
        let k = filter_scanned_keys(scan);
        assert_eq!(k, "fp5.local ssh-ed25519 AAAAC3Nz\nfp5.local ssh-rsa AAAAB3Nz", "comments/blanks dropped");
        let fp_line = "256 SHA256:n0tAr3alF1ngerPr1nt fp5.local (ED25519)";
        assert_eq!(parse_fingerprint(fp_line).as_deref(), Some("SHA256:n0tAr3alF1ngerPr1nt"));
        assert_eq!(parse_fingerprint("").as_deref(), None);
    }

    #[test]
    fn unknown_host_is_not_trusted_and_empty_host_rejected() {
        // a host that can't be in known_hosts -> ssh-keygen -F finds nothing
        assert!(!host_trusted("definitely.not.a.real.host.invalid"));
        assert!(!host_trusted("   "));
        assert!(trust_host("").is_err(), "empty host -> error, no network");
        assert!(untrust_host("").is_err(), "empty host -> error");
    }

    #[test]
    fn ssh_errors_classify_into_actionable_reasons() {
        assert_eq!(ssh_error_reason("kesslerd@fp5: Permission denied (publickey,password)."), "auth");
        assert_eq!(ssh_error_reason("Authentication failed."), "auth");
        assert_eq!(ssh_error_reason("Host key verification failed."), "hostkey");
        assert_eq!(ssh_error_reason("WARNING: REMOTE HOST IDENTIFICATION HAS CHANGED!"), "hostkey");
        assert_eq!(ssh_error_reason("ssh: connect to host fp5 port 22: Connection refused"), "unreachable");
        assert_eq!(ssh_error_reason("ssh: Could not resolve hostname fp5"), "unreachable");
        assert_eq!(ssh_error_reason("Operation timed out"), "unreachable");
        assert_eq!(ssh_error_reason("some other thing went wrong"), "other");
    }

    #[test]
    fn key_setup_argv_forces_password_and_autotrusts() {
        let a = setup_key_argv("kesslerd", "fp5.local");
        assert!(a.windows(2).any(|w| w[0] == "-o" && w[1] == "PubkeyAuthentication=no"), "force password: {a:?}");
        assert!(a.contains(&"PreferredAuthentications=password".to_string()));
        assert!(a.contains(&"StrictHostKeyChecking=accept-new".to_string()), "still auto-trust the host key");
        assert_eq!(a.last().unwrap(), "kesslerd@fp5.local", "target is last");
        // the askpass helper reads the password the GUI exports
        assert!(ASKPASS_BODY.contains("GM_SSH_PW"));
        // no-network guards
        assert!(setup_key("", "h", "pw").is_err());
        assert!(setup_key("u", "h", "").is_err(), "no password -> error before any ssh");
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

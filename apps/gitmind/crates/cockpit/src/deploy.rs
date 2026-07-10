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
///
/// Defence in depth: we also refuse to forward our agent or carry any port
/// forwardings outbound, so that if a device we connect to is compromised it can't
/// ride our connection (agent hijack / tunnel) to reach the rest of the fleet.
///   `ForwardAgent=no`        → never expose our keys' agent to the remote.
///   `ClearAllForwardings=yes`→ drop any -L/-R/-D tunnels even if configured.
///   `ControlMaster=no`       → no shared control socket a local attacker could reuse.
pub const SSH_OPTS: [&str; 12] = [
    "-o", "BatchMode=yes",
    "-o", "ConnectTimeout=8",
    "-o", "StrictHostKeyChecking=accept-new",
    "-o", "ForwardAgent=no",
    "-o", "ClearAllForwardings=yes",
    "-o", "ControlMaster=no",
];

/// `ssh <SSH_OPTS> user@host <remote_cmd>` as an argv (no shell).
pub(crate) fn ssh_argv(user: &str, host: &str, remote_cmd: &str) -> Vec<String> {
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

/// Run an arbitrary remote shell command over SSH and capture stdout. Used by the
/// app catalog (listing + details) so the catalog logic stays std-only/testable
/// while the SSH execution lives here. On-device.
pub fn remote_capture(user: &str, host: &str, cmd: &str) -> Result<String, String> {
    let out = Command::new("ssh")
        .args(ssh_argv(user, host, cmd))
        .output()
        .map_err(|e| format!("ssh failed: {e}"))?;
    if !out.status.success() {
        let msg = String::from_utf8_lossy(&out.stderr);
        return Err(if msg.trim().is_empty() {
            "ssh: remote command failed".into()
        } else {
            msg.trim().to_string()
        });
    }
    Ok(String::from_utf8_lossy(&out.stdout).into_owned())
}

/// The shell script that bootstraps rustup on a device: no-op if already present, else install via
/// curl (or wget) with the minimal profile, source the env, and add `target` if given. Pure so the
/// exact commands are unit-tested (nothing here runs locally).
pub fn rustup_bootstrap_script(target: &str) -> String {
    let add = if target.trim().is_empty() {
        String::new()
    } else {
        format!("rustup target add {} 2>&1 || true; ", target.trim())
    };
    format!(
        "set -e; \
         if command -v rustup >/dev/null 2>&1; then echo 'rustup already installed'; \
         elif command -v curl >/dev/null 2>&1; then curl --proto '=https' --tlsv1.2 -sSf https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path; \
         elif command -v wget >/dev/null 2>&1; then wget -qO- https://sh.rustup.rs | sh -s -- -y --profile minimal --no-modify-path; \
         else echo 'need curl or wget on the device to install rustup' >&2; exit 1; fi; \
         . \"$HOME/.cargo/env\" 2>/dev/null || export PATH=\"$HOME/.cargo/bin:$PATH\"; \
         {add}rustup --version"
    )
}

/// Bootstrap rustup on the remote device over SSH (non-interactive, minimal profile), then add
/// `target` if given. Idempotent — a no-op when rustup is already installed. This is what lets a
/// RemoteNative build add the target's std on a fresh device. Returns the combined output.
pub fn bootstrap_rustup_remote(user: &str, host: &str, target: &str) -> Result<String, String> {
    remote_capture(user, host, &rustup_bootstrap_script(target))
}

/// `scp <hardened opts> user@host:<remote> <local>` argv — pull a file (the built
/// binary) from the server to this device. Reuses SSH_OPTS so the same defence-in-
/// depth (no agent/tunnel) applies. Pure — unit-tested.
pub fn scp_pull_argv(user: &str, host: &str, remote: &str, local: &str) -> Vec<String> {
    let mut a: Vec<String> = SSH_OPTS.iter().map(|s| (*s).to_string()).collect();
    a.push(format!("{user}@{host}:{remote}"));
    a.push(local.to_string());
    a
}

/// Pull a remote file to a local path over scp. On-device.
pub fn scp_pull(user: &str, host: &str, remote: &str, local: &str) -> Result<(), String> {
    let st = Command::new("scp")
        .args(scp_pull_argv(user, host, remote, local))
        .status()
        .map_err(|e| format!("scp: {e}"))?;
    if st.success() {
        Ok(())
    } else {
        Err(format!("scp failed (exit {:?}) fetching {remote}", st.code()))
    }
}

/// `rsync -az -e "ssh <hardened opts>" user@host:<remote> <local>` argv — pull a file
/// with rsync (resumable, delta-friendly for re-installs) over the same hardened ssh.
/// The `-e` value is a single argv element rsync parses itself. Pure — unit-tested.
pub fn rsync_pull_argv(user: &str, host: &str, remote: &str, local: &str) -> Vec<String> {
    vec![
        "-az".to_string(),
        "-e".to_string(),
        ssh_command_str(),
        format!("{user}@{host}:{remote}"),
        local.to_string(),
    ]
}

/// Pull a remote file to a local path, preferring rsync for speed and falling back to
/// scp when rsync isn't installed (common on a minimal Alpine phone). On-device.
pub fn rsync_pull(user: &str, host: &str, remote: &str, local: &str) -> Result<(), String> {
    match Command::new("rsync")
        .args(rsync_pull_argv(user, host, remote, local))
        .status()
    {
        Ok(st) if st.success() => Ok(()),
        Ok(st) => Err(format!("rsync failed (exit {:?}) fetching {remote}", st.code())),
        // rsync not present → fall back to scp so the install still works
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => scp_pull(user, host, remote, local),
        Err(e) => Err(format!("rsync: {e}")),
    }
}

/// A parsed authorized_keys entry. `managed` = installed by gitmind (carries an
/// expiry-time); `expired` = its expiry-time is in the past.
#[derive(Debug, Clone, PartialEq)]
pub struct AuthKey {
    pub options: String,
    pub key_type: String,
    pub key_b64: String,
    pub comment: String,
    pub expiry: String,
    pub managed: bool,
    pub expired: bool,
}

impl AuthKey {
    /// A short, human identifier: the comment if present, else a tail of the key.
    pub fn id(&self) -> String {
        if !self.comment.trim().is_empty() {
            self.comment.trim().to_string()
        } else {
            let n = self.key_b64.len();
            format!("…{}", &self.key_b64[n.saturating_sub(12)..])
        }
    }
}

fn is_key_type(tok: &str) -> bool {
    tok.starts_with("ssh-") || tok.starts_with("ecdsa-") || tok.starts_with("sk-")
}

fn extract_expiry(options: &str) -> String {
    options.find("expiry-time=\"").map(|i| {
        let rest = &options[i + "expiry-time=\"".len()..];
        rest.split('"').next().unwrap_or("").to_string()
    }).unwrap_or_default()
}

/// Parse an authorized_keys file. `now_stamp` is the current time as YYYYMMDDHHMMSS
/// (from key_expiry_time(now, 0)) for the expired check. Pure — unit-tested.
pub fn parse_authorized_keys(text: &str, now_stamp: &str) -> Vec<AuthKey> {
    let mut out = Vec::new();
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            continue;
        }
        let toks: Vec<&str> = l.split_whitespace().collect();
        let (options, rest) = if !toks.is_empty() && is_key_type(toks[0]) {
            (String::new(), &toks[..])
        } else if toks.len() >= 2 {
            (toks[0].to_string(), &toks[1..])
        } else {
            continue;
        };
        if rest.len() < 2 {
            continue;
        }
        let key_type = rest[0].to_string();
        let key_b64 = rest[1].to_string();
        let comment = rest[2..].join(" ");
        let expiry = extract_expiry(&options);
        let managed = !expiry.is_empty();
        let expired = managed && expiry.as_str() < now_stamp;
        out.push(AuthKey { options, key_type, key_b64, comment, expiry, managed, expired });
    }
    out
}

/// Clean an authorized_keys file: drop gitmind-managed entries that are expired or are
/// older duplicates of the same key (keeping the newest expiry), leaving every other
/// line untouched. Returns the cleaned text + how many lines were removed. Pure.
pub fn dedupe_authorized_keys(text: &str, now_stamp: &str) -> (String, usize) {
    let entries = parse_authorized_keys(text, now_stamp);
    let mut best: std::collections::HashMap<String, String> = std::collections::HashMap::new();
    for e in &entries {
        if e.managed && !e.expired {
            best.entry(e.key_b64.clone())
                .and_modify(|cur| { if e.expiry.as_str() > cur.as_str() { *cur = e.expiry.clone(); } })
                .or_insert_with(|| e.expiry.clone());
        }
    }
    let mut kept_blob: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut out = String::new();
    let mut removed = 0usize;
    for line in text.lines() {
        let l = line.trim();
        if l.is_empty() || l.starts_with('#') {
            out.push_str(line);
            out.push('\n');
            continue;
        }
        let parsed = parse_authorized_keys(line, now_stamp);
        let keep = match parsed.first() {
            None => true,
            Some(e) if !e.managed => true,
            Some(e) if e.expired => false,
            Some(e) => {
                let is_best = best.get(&e.key_b64).map(|b| b == &e.expiry).unwrap_or(false);
                if is_best && !kept_blob.contains(&e.key_b64) {
                    kept_blob.insert(e.key_b64.clone());
                    true
                } else {
                    false
                }
            }
        };
        if keep {
            out.push_str(line);
            out.push('\n');
        } else {
            removed += 1;
        }
    }
    (out, removed)
}

/// Run a remote command feeding `stdin` to it (e.g. writing a file via `cat >`). On-device.
pub fn remote_feed(user: &str, host: &str, cmd: &str, stdin: &str) -> Result<String, String> {
    use std::io::Write;
    let mut child = Command::new("ssh")
        .args(ssh_argv(user, host, cmd))
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh: {e}"))?;
    if let Some(si) = child.stdin.as_mut() {
        si.write_all(stdin.as_bytes()).map_err(|e| format!("write: {e}"))?;
    }
    let out = child.wait_with_output().map_err(|e| format!("ssh: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(String::from_utf8_lossy(&out.stderr).trim().to_string())
    }
}

/// `scp <hardened opts> <local> user@host:<remote>` argv — push a file (a freshly
/// built binary) out to a device. Pure — unit-tested.
pub fn scp_push_argv(local: &str, user: &str, host: &str, remote: &str) -> Vec<String> {
    let mut a: Vec<String> = SSH_OPTS.iter().map(|s| (*s).to_string()).collect();
    a.push(local.to_string());
    a.push(format!("{user}@{host}:{remote}"));
    a
}

/// Same as `scp_push_argv` but forces the legacy SCP transfer protocol (`-O`). Modern
/// scp speaks SFTP by default, which fails on devices without the sftp-server subsystem
/// (common on minimal Alpine / postmarketOS) — the legacy protocol needs no subsystem.
pub fn scp_push_argv_legacy(local: &str, user: &str, host: &str, remote: &str) -> Vec<String> {
    let mut a = vec!["-O".to_string()];
    a.extend(scp_push_argv(local, user, host, remote));
    a
}

/// `rsync -az -e "ssh <opts>" <local> user@host:<remote>` argv — push over rsync, which
/// runs its own protocol over the ssh channel and so needs no sftp-server either.
pub fn rsync_push_argv(local: &str, user: &str, host: &str, remote: &str) -> Vec<String> {
    vec![
        "-az".to_string(),
        "-e".to_string(),
        ssh_command_str(),
        local.to_string(),
        format!("{user}@{host}:{remote}"),
    ]
}

/// Push a local file to a device, robust against minimal sshd setups. ssh (the mkdir)
/// can succeed while plain scp dies with 255 because the device has no sftp-server, so we
/// try, in order: rsync over ssh, then `scp -O` (legacy protocol), then plain scp — and
/// report an actionable error if every transport fails. On-device.
pub fn scp_push(local: &str, user: &str, host: &str, remote: &str) -> Result<(), String> {
    if let Some(dir) = remote.rsplit_once('/').map(|(d, _)| d) {
        // best-effort: ensure the target directory exists (also proves ssh auth works)
        let _ = remote_capture(user, host, &format!("mkdir -p {}", shell_quote(dir)));
    }
    let mut errs: Vec<String> = Vec::new();
    // 1) rsync over ssh — fastest, no sftp-server needed
    match Command::new("rsync").args(rsync_push_argv(local, user, host, remote)).status() {
        Ok(st) if st.success() => return Ok(()),
        Ok(st) => errs.push(format!("rsync exit {:?}", st.code())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {} // rsync not installed — fine
        Err(e) => errs.push(format!("rsync: {e}")),
    }
    // 2) scp with the legacy protocol — works when sftp-server is absent
    match Command::new("scp").args(scp_push_argv_legacy(local, user, host, remote)).status() {
        Ok(st) if st.success() => return Ok(()),
        Ok(st) => errs.push(format!("scp -O exit {:?}", st.code())),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Err("scp isn't installed on this device".into()),
        Err(e) => errs.push(format!("scp -O: {e}")),
    }
    // 3) plain scp (oldest fallback)
    match Command::new("scp").args(scp_push_argv(local, user, host, remote)).status() {
        Ok(st) if st.success() => Ok(()),
        Ok(st) => {
            let code = st.code();
            if code == Some(255) {
                Err(format!("couldn't reach {user}@{host} — check the device is on, the gitmind key is installed, and ssh works; some minimal devices also disable scp/sftp (gitmind already tried rsync and scp -O)"))
            } else {
                Err(format!("transfer to {host} failed (scp exit {:?}; tried: {})", code, errs.join(", ")))
            }
        }
        Err(e) => Err(format!("scp: {e} (tried: {})", errs.join(", "))),
    }
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

/// Like [`deploy_app`] but **captures** the remote build output and streams it line-by-line to
/// `on_line` (so the GUI shows a live log instead of sending it to gitmind's own terminal), and
/// surfaces the real failure text. Drains stdout on a thread while reading stderr to avoid a
/// pipe-buffer deadlock on a chatty build.
pub fn deploy_app_logged(user: &str, host: &str, dir: &str, app: &str, on_line: &mut dyn FnMut(&str)) -> Result<String, String> {
    use std::io::{BufRead, BufReader, Read};
    let cmd = remote_deploy_cmd(dir, app);
    on_line(&format!("$ ssh {host}: build + install {app} (in {dir})"));
    let mut child = Command::new("ssh")
        .args(ssh_argv(user, host, &cmd))
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::piped())
        .spawn()
        .map_err(|e| format!("ssh failed: {e}"))?;
    let out = child.stdout.take();
    let drain = std::thread::spawn(move || {
        let mut s = String::new();
        if let Some(o) = out {
            let _ = BufReader::new(o).read_to_string(&mut s);
        }
        s
    });
    if let Some(err) = child.stderr.take() {
        for line in BufReader::new(err).lines().map_while(Result::ok) {
            on_line(&line);
        }
    }
    let stdout_text = drain.join().unwrap_or_default();
    for line in stdout_text.lines() {
        on_line(line);
    }
    let status = child.wait().map_err(|e| format!("ssh wait failed: {e}"))?;
    if status.success() {
        on_line(&format!("✓ {app} installed on {host}"));
        Ok(format!("{app} built + installed on {host}; launcher created"))
    } else {
        Err(format!("remote build/install failed (exit {}) — see the log above", status.code().unwrap_or(-1)))
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
    let keys = fetch_peer_key(h)?;
    write_known_host(h, &keys)
}

/// Fetch a peer's host key **without** writing it — for fingerprint preview /
/// two-sided matching. Returns the raw `known_hosts` line(s). On-device.
pub fn fetch_peer_key(host: &str) -> Result<String, String> {
    let h = host.trim();
    if h.is_empty() {
        return Err("no host given".into());
    }
    let scan = Command::new("ssh-keyscan")
        .args(["-T", "6", h])
        .output()
        .map_err(|e| format!("ssh-keyscan failed: {e}"))?;
    let keys = filter_scanned_keys(&String::from_utf8_lossy(&scan.stdout));
    if keys.trim().is_empty() {
        return Err(format!("no SSH key returned for {h} (unreachable, or no sshd there?)"));
    }
    Ok(keys)
}

/// Append already-fetched key line(s) to `~/.ssh/known_hosts`. Used by both the
/// auto-trust path and "matched — trust" (where these are exactly the bytes whose
/// fingerprint the user just compared against the other device's screen).
pub fn write_known_host(host: &str, key_lines: &str) -> Result<String, String> {
    let h = host.trim();
    if h.is_empty() || key_lines.trim().is_empty() {
        return Err("nothing to trust".into());
    }
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
    writeln!(f, "{}", key_lines.trim_end()).map_err(|e| format!("write {known}: {e}"))?;
    let fp = fingerprint_of(key_lines).unwrap_or_else(|| "key".to_string());
    Ok(format!("trusted {h} ({fp})"))
}

// ──────────────────── key fingerprints (two-sided matching) ────────────────────

/// Parse an `ssh-keygen -l` summary line, e.g.
///   `256 SHA256:Zl3p… comment (ED25519)`
/// into (fingerprint, keytype). Pure — unit-tested.
pub fn parse_key_summary(line: &str) -> Option<(String, String)> {
    let mut it = line.split_whitespace();
    let _bits = it.next()?;
    let fp = it.next()?.to_string();
    let typ = line
        .rsplit('(')
        .next()
        .map(|t| t.trim_end_matches(')').trim().to_string())?;
    if fp.is_empty() || typ.is_empty() || typ == line {
        return None;
    }
    Some((fp, typ))
}

/// Run public-key text through `ssh-keygen -lv -` → (fingerprint, keytype,
/// randomart). The ASCII "randomart" box exists for exactly this: comparing the
/// same key by eye on two screens. On-device (shells out).
pub fn key_fingerprint_art(pubkey: &str) -> Option<(String, String, String)> {
    use std::io::Write;
    let mut child = Command::new("ssh-keygen")
        .args(["-lv", "-"])
        .stdin(std::process::Stdio::piped())
        .stdout(std::process::Stdio::piped())
        .stderr(std::process::Stdio::null())
        .spawn()
        .ok()?;
    child.stdin.take()?.write_all(pubkey.as_bytes()).ok()?;
    let out = child.wait_with_output().ok()?;
    let text = String::from_utf8_lossy(&out.stdout);
    let mut lines = text.lines();
    let (fp, typ) = parse_key_summary(lines.next()?)?;
    let art = lines.collect::<Vec<_>>().join("\n");
    Some((fp, typ, art))
}

/// This machine's SSH **host** public key (prefer ed25519), so the GUI can show
/// "this device's key" for a peer to match on screen.
pub fn local_host_pubkey() -> Option<String> {
    for p in [
        "/etc/ssh/ssh_host_ed25519_key.pub",
        "/etc/ssh/ssh_host_ecdsa_key.pub",
        "/etc/ssh/ssh_host_rsa_key.pub",
    ] {
        if let Ok(s) = std::fs::read_to_string(p) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
}

/// This user's SSH **identity** public key (the one `setup_key` installs), if any.
pub fn local_identity_pubkey() -> Option<String> {
    let home = std::env::var("HOME").ok()?;
    for n in ["id_ed25519", "id_ecdsa", "id_rsa"] {
        if let Ok(s) = std::fs::read_to_string(format!("{home}/.ssh/{n}.pub")) {
            if !s.trim().is_empty() {
                return Some(s);
            }
        }
    }
    None
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

/// Derive the /24 the fleet lives on from a target IPv4 (e.g. `192.168.1.7` →
/// `192.168.1.0/24`), or `None` if `host` isn't a dotted IPv4 (a hostname, etc.).
/// Pure — unit-tested.
pub fn lan_cidr_from_ip(host: &str) -> Option<String> {
    let h = host.trim();
    let octs: Vec<&str> = h.split('.').collect();
    if octs.len() != 4 {
        return None;
    }
    for o in &octs {
        let n: u16 = o.parse().ok()?;
        if n > 255 || o.is_empty() {
            return None;
        }
    }
    Some(format!("{}.{}.{}.0/24", octs[0], octs[1], octs[2]))
}

/// `authorized_keys` option prefix applied to the key we install — defence in
/// depth so a *stolen* fleet key can't be used to pivot:
///   `restrict`     → no port/agent/X11 forwarding, no PTY, no user-rc (this is the
///                    bit that stops "tunnel into all trusted devices").
///   `from="<cidr>"`→ the key only works from the local /24 (a remote attacker off
///                    the LAN can't use it at all), added when the host is an IP.
/// Pure — unit-tested.
/// Default lifetime for an installed access key. Keys carry an `expiry-time` so a
/// leaked/forgotten key stops working on its own; re-running "install my key" renews it.
pub const KEY_EXPIRY_DAYS: u64 = 7;

fn civil_from_days(z: i64) -> (i64, u32, u32) {
    let z = z + 719468;
    let era = (if z >= 0 { z } else { z - 146096 }) / 146097;
    let doe = z - era * 146097;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let m = (if mp < 10 { mp + 3 } else { mp - 9 }) as u32;
    (if m <= 2 { y + 1 } else { y }, m, d)
}

/// A unix time `days` ahead as an OpenSSH `expiry-time` stamp "YYYYMMDDHHMMSS" (UTC).
/// Pure — unit-tested.
pub fn key_expiry_time(now_unix: u64, days: u64) -> String {
    let secs = now_unix + days * 86400;
    let z = (secs / 86400) as i64;
    let tod = secs % 86400;
    let (y, m, d) = civil_from_days(z);
    format!("{:04}{:02}{:02}{:02}{:02}{:02}", y, m, d, tod / 3600, (tod % 3600) / 60, tod % 60)
}

pub fn authorized_keys_options(host: &str, expiry: Option<&str>) -> String {
    let base = match lan_cidr_from_ip(host) {
        Some(cidr) => format!("restrict,from=\"{cidr}\""),
        None => "restrict".to_string(),
    };
    match expiry {
        Some(exp) => format!("expiry-time=\"{exp}\",{base}"),
        None => base,
    }
}

/// Build the single `authorized_keys` line: `<options> <pubkey>`. Pure — tested.
pub fn restricted_key_line(options: &str, pubkey: &str) -> String {
    format!("{} {}\n", options, pubkey.trim())
}

/// The authorized_keys line for the interactive "set up SSH key" convenience: the user's
/// own key on their own account, installed PLAIN — no `restrict`/`no-pty`/expiry — exactly
/// like `ssh-copy-id`, so interactive shells keep working. The trailing `gitmind` comment
/// makes it identifiable for pruning. Pure — tested.
///
/// (The hardened `restrict` + forced-command + expiry form is `restricted_key_line`, used
/// ONLY for the dedicated, non-interactive fleet user — never an interactive account,
/// because `restrict` implies `no-pty` and would break `ssh user@host`.)
pub fn plain_key_line(pubkey: &str) -> String {
    let parts: Vec<&str> = pubkey.split_whitespace().collect();
    if parts.len() >= 2 {
        format!("{} {} gitmind\n", parts[0], parts[1])
    } else {
        format!("{}\n", pubkey.trim())
    }
}

/// Command (run over ssh) that removes gitmind's restrictive options from an
/// authorized_keys line — restoring interactive PTY access — after backing up the file.
pub fn unrestrict_command() -> &'static str {
    "cp ~/.ssh/authorized_keys ~/.ssh/authorized_keys.gm-bak 2>/dev/null; \
     sed -i -E 's/^[^ ]*(restrict|no-pty)[^ ]* +(ssh-|ecdsa-|sk-)/\\2/' ~/.ssh/authorized_keys && echo gitmind-ssh-unbroken"
}

/// Repair an account whose interactive ssh was broken by a restricted key: strip the
/// restrictive options over ssh. Key auth still works (only the PTY was refused), so a
/// non-interactive command connects fine.
pub fn unbreak_ssh(user: &str, host: &str) -> Result<String, String> {
    let out = remote_capture(user, host, unrestrict_command())?;
    if out.contains("gitmind-ssh-unbroken") {
        Ok(format!("restored interactive ssh for {user}@{host} — try `ssh {user}@{host}` again"))
    } else {
        Err(format!("couldn't repair it automatically: {}", out.trim()))
    }
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
    // This is the user's own key on their own interactive account, so install it PLAIN —
    // like ssh-copy-id. It must NOT be `restrict`ed: `restrict` implies `no-pty`, which
    // would break `ssh user@host` ("PTY allocation request failed"). Hardening (restrict +
    // forced command + expiry) is reserved for the dedicated fleet user, which is never
    // used interactively.
    let key_line = plain_key_line(&pubkey);
    // temp askpass helper (0700, current pid in the name, removed after)
    let script = std::env::temp_dir().join(format!("gm-askpass-{}", std::process::id()));
    std::fs::write(&script, ASKPASS_BODY).map_err(|e| format!("write askpass: {e}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("chmod askpass: {e}"))?;
    }
    // Ensure the file ends with a newline before appending, so our line never fuses onto a
    // previous key line (which would silently apply our comment/options to that key).
    let remote = "umask 077; mkdir -p ~/.ssh; f=~/.ssh/authorized_keys; \
                  [ -s \"$f\" ] && [ \"$(tail -c1 \"$f\" | wc -l)\" -eq 0 ] && echo >> \"$f\"; \
                  cat >> \"$f\" && echo gitmind-key-installed";
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
            .write_all(key_line.as_bytes())
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

/// ssh to `admin@host` with *password* auth (via SSH_ASKPASS, no TTY), run
/// `remote_cmd`, and write `stdin_data` to that command's stdin — used to run
/// `sudo -S …` non-interactively (the sudo password arrives on stdin while the ssh
/// password arrives via askpass; for a personal account they're the same string).
/// Returns stdout on success. On-device.
pub fn ssh_sudo_run(admin: &str, host: &str, password: &str, remote_cmd: &str, stdin_data: &str) -> Result<String, String> {
    if admin.trim().is_empty() || host.trim().is_empty() {
        return Err("admin and host are required".into());
    }
    if password.is_empty() {
        return Err("a password is required".into());
    }
    let script = std::env::temp_dir().join(format!("gm-askpass-{}", std::process::id()));
    std::fs::write(&script, ASKPASS_BODY).map_err(|e| format!("write askpass: {e}"))?;
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o700))
            .map_err(|e| format!("chmod askpass: {e}"))?;
    }
    let spawned = Command::new("ssh")
        .env("GM_SSH_PW", password)
        .env("SSH_ASKPASS", &script)
        .env("SSH_ASKPASS_REQUIRE", "force")
        .env("DISPLAY", std::env::var("DISPLAY").unwrap_or_else(|_| ":0".into()))
        .args(setup_key_argv(admin, host))
        .arg(remote_cmd)
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
            .write_all(stdin_data.as_bytes())
            .map_err(|e| format!("send stdin: {e}"))?;
        let out = child.wait_with_output().map_err(|e| format!("ssh wait: {e}"))?;
        if out.status.success() {
            Ok(String::from_utf8_lossy(&out.stdout).into_owned())
        } else {
            let err = String::from_utf8_lossy(&out.stderr);
            match ssh_error_reason(&err) {
                "auth" => Err(format!("password rejected for {admin}@{host}")),
                _ if err.trim().is_empty() => Err("remote command failed".into()),
                _ => Err(err.trim().to_string()),
            }
        }
    })();
    let _ = std::fs::remove_file(&script);
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
    /// Live log lines (build output etc.), for a streaming view while the job runs.
    log: Vec<String>,
}

impl Job {
    /// Claim the job for `kind`. Returns false if one is already running, so the
    /// caller can refuse to start a second concurrent operation.
    pub fn start(&self, kind: &str) -> bool {
        let mut g = self.inner.lock().unwrap();
        if g.running {
            return false;
        }
        *g = JobInner { running: true, kind: kind.to_string(), done: false, result: String::new(), log: Vec::new() };
        true
    }

    /// Worker thread appends a live log line (build output). Cheap; the GUI tails it.
    pub fn log_line(&self, line: &str) {
        let mut g = self.inner.lock().unwrap();
        g.log.push(line.to_string());
        // cap so a huge build can't grow the buffer without bound
        let len = g.log.len();
        if len > 2000 {
            g.log.drain(0..len - 2000);
        }
    }

    /// GUI poll for new log lines since index `from` → (next_index, new_lines). Survives across
    /// polls (only cleared on the next `start`), so the final output stays visible after `done`.
    pub fn tail(&self, from: usize) -> (usize, Vec<String>) {
        let g = self.inner.lock().unwrap();
        let start = from.min(g.log.len());
        (g.log.len(), g.log[start..].to_vec())
    }

    /// Worker thread reports the finished JSON result.
    pub fn finish(&self, result: String) {
        let mut g = self.inner.lock().unwrap();
        g.running = false;
        g.done = true;
        g.result = result;
    }

    /// GUI poll → (running, done, kind, result). When `done`, the *status* is consumed (so a
    /// result is delivered once) but the log is retained until the next `start`.
    pub fn poll(&self) -> (bool, bool, String, String) {
        let mut g = self.inner.lock().unwrap();
        let out = (g.running, g.done, g.kind.clone(), g.result.clone());
        if g.done {
            g.running = false;
            g.done = false;
            g.kind = String::new();
            g.result = String::new();
            // keep g.log so the finished build output stays on screen
        }
        out
    }
}

#[cfg(test)]
mod tests {
    #[test]
    fn rustup_bootstrap_is_idempotent_has_fallback_and_adds_target() {
        let s = super::rustup_bootstrap_script("aarch64-unknown-linux-musl");
        assert!(s.contains("command -v rustup")); // no-op when already installed
        assert!(s.contains("sh.rustup.rs"));
        assert!(s.contains("command -v curl") && s.contains("command -v wget")); // fallback
        assert!(s.contains("--profile minimal") && s.contains("-y")); // non-interactive
        assert!(s.contains("rustup target add aarch64-unknown-linux-musl"));
        // no target → no `target add`
        assert!(!super::rustup_bootstrap_script("").contains("target add"));
    }

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
        // the richer summary parse used for two-sided matching display
        let (fp, typ) = parse_key_summary(fp_line).expect("summary parses");
        assert_eq!(fp, "SHA256:n0tAr3alF1ngerPr1nt");
        assert_eq!(typ, "ED25519");
        assert_eq!(parse_key_summary("garbage with no parens"), None);
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
    fn outbound_ssh_refuses_agent_and_tunnels() {
        // the flags that stop a compromised peer pivoting through our connection
        let joined = SSH_OPTS.join(" ");
        assert!(joined.contains("ForwardAgent=no"), "no agent forwarding: {joined}");
        assert!(joined.contains("ClearAllForwardings=yes"), "drop tunnels: {joined}");
        // scp reuses the same hardened opts
        let s = scp_pull_argv("kesslerd", "192.168.1.7", "target/x/release/app", "/tmp/app");
        assert!(s.contains(&"ForwardAgent=no".to_string()));
        assert_eq!(s[s.len() - 2], "kesslerd@192.168.1.7:target/x/release/app");
        assert_eq!(s[s.len() - 1], "/tmp/app");
        // push is the same, reversed (local then user@host:remote)
        let p = scp_push_argv("/repo/target/app", "gitmind", "192.168.1.9", "/home/gitmind/.local/bin/app");
        assert!(p.contains(&"ForwardAgent=no".to_string()));
        assert_eq!(p[p.len() - 2], "/repo/target/app");
        assert_eq!(p[p.len() - 1], "gitmind@192.168.1.9:/home/gitmind/.local/bin/app");
        // legacy push forces the old SCP protocol so it works without sftp-server
        let pl = scp_push_argv_legacy("/repo/target/app", "gitmind", "192.168.1.9", "/home/gitmind/.local/bin/app");
        assert_eq!(pl[0], "-O", "legacy scp leads with -O");
        assert_eq!(pl[pl.len() - 1], "gitmind@192.168.1.9:/home/gitmind/.local/bin/app");
        // rsync push: -az, -e "ssh <opts>", then local and user@host:remote
        let rp = rsync_push_argv("/repo/target/app", "gitmind", "192.168.1.9", "/home/gitmind/.local/bin/app");
        assert_eq!(rp[0], "-az");
        assert!(rp[2].starts_with("ssh ") && rp[2].contains("ClearAllForwardings=yes"));
        assert_eq!(rp[rp.len() - 2], "/repo/target/app");
        assert_eq!(rp[rp.len() - 1], "gitmind@192.168.1.9:/home/gitmind/.local/bin/app");
        // rsync pull: -az, -e "ssh <opts>", then user@host:remote and local
        let r = rsync_pull_argv("gitmind", "192.168.1.7", "/repo/target/x/release/app", "/tmp/app");
        assert_eq!(r[0], "-az");
        assert_eq!(r[1], "-e");
        assert!(r[2].starts_with("ssh ") && r[2].contains("ForwardAgent=no"), "rsync -e carries hardened ssh: {}", r[2]);
        assert_eq!(r[3], "gitmind@192.168.1.7:/repo/target/x/release/app");
        assert_eq!(r[4], "/tmp/app");
    }

    #[test]
    fn installed_keys_are_restricted_and_lan_pinned() {
        assert_eq!(lan_cidr_from_ip("192.168.1.7").as_deref(), Some("192.168.1.0/24"));
        assert_eq!(lan_cidr_from_ip("10.0.0.42").as_deref(), Some("10.0.0.0/24"));
        assert_eq!(lan_cidr_from_ip("fp5.local"), None, "hostname -> no from= pin");
        assert_eq!(lan_cidr_from_ip("192.168.1.999"), None, "bad octet -> none");
        // IP target: restrict + from=; hostname target: restrict only
        assert_eq!(authorized_keys_options("192.168.1.7", None), "restrict,from=\"192.168.1.0/24\"");
        assert_eq!(authorized_keys_options("fp5.local", None), "restrict");
        // with an expiry, expiry-time leads the option list
        assert_eq!(authorized_keys_options("fp5.local", Some("20260706000000")), "expiry-time=\"20260706000000\",restrict");
        // key expiry stamp is YYYYMMDDHHMMSS in UTC, `days` ahead
        assert_eq!(key_expiry_time(0, 0), "19700101000000");
        assert_eq!(key_expiry_time(0, 7), "19700108000000");
        assert_eq!(key_expiry_time(1_700_000_000, 0), "20231114221320");
        // the installed line carries the options ahead of the key, one line
        let line = restricted_key_line("restrict,from=\"192.168.1.0/24\"", "ssh-ed25519 AAAAC3xyz gitmind-fleet\n");
        assert!(line.starts_with("restrict,from=\"192.168.1.0/24\" ssh-ed25519 "));
        assert!(line.ends_with('\n') && line.matches('\n').count() == 1);
        // the interactive user's key is PLAIN — no restrict/no-pty/expiry (that would break
        // `ssh user@host` with "PTY allocation request failed")
        let plain = plain_key_line("ssh-ed25519 AAAAC3xyz user@laptop");
        assert_eq!(plain, "ssh-ed25519 AAAAC3xyz gitmind\n");
        assert!(!plain.contains("restrict") && !plain.contains("no-pty") && !plain.contains("expiry-time"));
        // recovery command strips restrictive options and backs up
        assert!(unrestrict_command().contains("authorized_keys.gm-bak") && unrestrict_command().contains("restrict"));
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

    #[test]
    fn authorized_keys_parse_and_dedupe() {
        let now = "20260629000000";
        let user_key = "ssh-ed25519 AAAAuserKEY me@laptop";
        let expired = "expiry-time=\"20260101000000\",restrict ssh-ed25519 AAAAoldKEY gitmind";
        let live_old = "expiry-time=\"20260701000000\",restrict ssh-ed25519 AAAAdupKEY gitmind";
        let live_new = "expiry-time=\"20260706000000\",restrict ssh-ed25519 AAAAdupKEY gitmind";
        let text = format!("# comment\n{user_key}\n{expired}\n{live_old}\n{live_new}\n");
        let parsed = parse_authorized_keys(&text, now);
        assert_eq!(parsed.len(), 4);
        assert!(!parsed[0].managed, "user key not managed");
        assert!(parsed[1].managed && parsed[1].expired, "old gitmind key expired");
        assert_eq!(parsed[3].id(), "gitmind");
        // cleanup drops the expired one + the older duplicate; keeps user key, comment, newest
        let (clean, removed) = dedupe_authorized_keys(&text, now);
        assert_eq!(removed, 2, "expired + older dup removed");
        assert!(clean.contains(user_key), "user key untouched");
        assert!(clean.contains("# comment"), "comments preserved");
        assert!(clean.contains("20260706000000") && !clean.contains("20260701000000"), "kept newest dup only");
        assert!(!clean.contains("20260101000000"), "expired gone");
    }
}

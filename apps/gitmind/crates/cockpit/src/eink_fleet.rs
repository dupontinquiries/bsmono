//! eink_fleet — push a built binary to an e-ink tablet and start it, over
//! whatever channel reaches the device.
//!
//! ## Why this is not `reroll`
//!
//! `reroll` is shaped around postmarketOS phones: a fleet user created with
//! `useradd`, an apk index to refresh, a `.desktop` entry for the launcher.
//! None of that exists on a reMarkable. You are root, there is no package
//! index we own, and the "launcher" is whatever display server is running
//! (Oxide's Blight, or AppLoad's qtfb). Bending `reroll::Runner` to cover
//! both would add no-op methods on one side and missing ones on the other,
//! so this composes with it instead: a reroll can build the binary, and this
//! module takes it from there.
//!
//! ## The seams
//!
//! * [`Remote`] is the transport. SSH (Wi-Fi or USB networking) is the only
//!   implementation today. Bluetooth is the reason it is a trait: the
//!   monorepo already has an RFCOMM frame protocol in `bt-kit`, and a device
//!   agent speaking it can implement `run`/`push` without anything above
//!   this layer changing. Not built, because no device agent exists yet and
//!   whether reMarkable userland can reach its radio at all is unverified.
//! * [`RefreshProfile`] is how the launch asks for a display mode. It is an
//!   environment variable the on-device e-ink shell reads, NOT a waveform
//!   number. Which waveform is actually fastest-with-colour on a given panel
//!   is a device measurement, and it belongs in the shell's refresh policy
//!   where it can be observed and changed, not baked into the deploy tool.
//!
//! ## The tablet forgets
//!
//! On the Paper Pro family the root filesystem is read-only and `/etc` is an
//! overlay whose writable layer is tmpfs (`/var/volatile/etc`): anything
//! written there is gone after a reboot. xovi (and so AppLoad/qtfb) must be
//! started with `/home/root/xovi/start` after each boot, and a Vellum
//! launcher (Oxide) can be configured but not running. (All from reManager's
//! handling of the same device.) So this module:
//! * writes ONLY under `/home/root`, which persists (`assert_persistent`);
//! * separates "installed" from "active since this boot" and offers the
//!   activation step ([`Activation`]) instead of just failing;
//! * never unlocks the system (`mount -o remount,rw /`): because it writes
//!   only under `/home`, which survives reboots AND OS updates, it has no
//!   reason to, and unlocking done wrong can cost SSH access (the
//!   remarkable.guide FAQ: after unmounting the /etc overlay the dropbear
//!   host keys, bind-mounted from /home/root/.dropbear, must be re-bound or
//!   you cannot reconnect).
//!
//! Host keys: on these devices they are already persistent (that bind
//! mount), so a changed key means an OS update regenerated it (FAQ), not a
//! reboot. That is still never auto-accepted.
//!
//! Everything that builds a remote command is a pure function and is tested
//! as a string. The shell text runs under the device's `/bin/sh`, which may
//! be busybox, so it sticks to POSIX constructs.

use crate::deploy::shell_quote;

// ───────────────────────── device model ─────────────────────────

/// Whether the panel can show colour. Decides what "fastest" means.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Panel {
    Mono,
    Color,
}

/// reMarkable hardware we know how to target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RmModel {
    Rm1,
    Rm2,
    PaperPro,
    PaperProMove,
    PaperPure,
}

impl RmModel {
    /// Rust target for this device.
    ///
    /// gnu, not musl, on purpose: the Blight client links the device's own
    /// `libblight_protocol.so`, and a static musl binary cannot load a glibc
    /// shared object (the same wall `buildflags` documents for libwayland).
    /// Pin the glibc version at build time (`cargo zigbuild --target
    /// <triple>.<glibc>`) to what the device reports; that suffix is a build
    /// concern and deliberately not part of the triple stored here.
    pub fn triple(self) -> &'static str {
        match self {
            RmModel::Rm1 | RmModel::Rm2 => "armv7-unknown-linux-gnueabihf",
            RmModel::PaperPro | RmModel::PaperProMove | RmModel::PaperPure => {
                "aarch64-unknown-linux-gnu"
            }
        }
    }

    pub fn panel(self) -> Panel {
        match self {
            RmModel::PaperPro | RmModel::PaperProMove => Panel::Color,
            RmModel::Rm1 | RmModel::Rm2 | RmModel::PaperPure => Panel::Mono,
        }
    }

    /// Native panel size in pixels (portrait). Same numbers qtfb's
    /// `common.h` uses for its per-device framebuffer formats.
    pub fn size(self) -> (u32, u32) {
        match self {
            RmModel::Rm1 | RmModel::Rm2 | RmModel::PaperPure => (1404, 1872),
            RmModel::PaperPro => (1620, 2160),
            RmModel::PaperProMove => (954, 1696),
        }
    }

    /// The id eink-kit's `EINK_MODEL` expects (`device::Model::parse`).
    pub fn env_id(self) -> &'static str {
        match self {
            RmModel::Rm1 => "rm1",
            RmModel::Rm2 => "rm2",
            RmModel::PaperPro => "rmpp",
            RmModel::PaperProMove => "rmppm",
            RmModel::PaperPure => "rmppure",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            RmModel::Rm1 => "reMarkable 1",
            RmModel::Rm2 => "reMarkable 2",
            RmModel::PaperPro => "Paper Pro",
            RmModel::PaperProMove => "Paper Pro Move",
            RmModel::PaperPure => "Paper Pure",
        }
    }
}

/// One shell round-trip that reports what the device is, as `key=value`
/// lines. Every source is optional; the parser weighs what came back.
///
/// * `machine`: `/sys/devices/soc0/machine`. Oxide (liboxide
///   devicesettings.cpp, libblight_client state.cpp) identifies every model
///   from this one line, so it is the authoritative source.
/// * `motd`: the login banner, which carries the device codename.
/// * `hostname`: stock hostnames carry SoC and codename (`imx8mm-ferrari`).
/// * `soc`: `/sys/devices/soc0/soc_id` (e.g. `i.MX8MM`), plus panel sizes
///   from DRM connector modes and fb0: the fallback for a device whose
///   machine string is new to us.
pub fn probe_cmd() -> &'static str {
    "echo \"machine=$(head -n1 /sys/devices/soc0/machine 2>/dev/null)\"; \
     echo \"dtmodel=$(tr -d '\\0' < /proc/device-tree/model 2>/dev/null)\"; \
     echo \"soc=$(head -n1 /sys/devices/soc0/soc_id 2>/dev/null)\"; \
     echo \"motd=$(tr '\\n' ' ' < /etc/motd 2>/dev/null | tr -s ' ' | cut -c1-400)\"; \
     echo \"hostname=$(cat /proc/sys/kernel/hostname 2>/dev/null)\"; \
     for f in /sys/class/drm/card*-*/modes; do [ -r \"$f\" ] && head -n1 \"$f\" | sed 's/^/size=/'; done 2>/dev/null; \
     [ -r /sys/class/graphics/fb0/virtual_size ] && sed 's/^/size=/; s/,/x/' /sys/class/graphics/fb0/virtual_size; \
     echo \"arch=$(uname -m)\""
}

/// What a probe established, and how.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// From `uname -m`, which is reliable. Enough to build for the device
    /// even when the model is unknown.
    pub triple: Option<&'static str>,
    /// `None` means "ask the operator", recorded in the roster like every
    /// other identity the network cannot report.
    pub model: Option<RmModel>,
    /// Which evidence decided the model (or why none did). Shown to the
    /// operator so a wrong identification is visible, not silent.
    pub how: String,
    /// A codename seen in the motd that we do not map to a model. Reported,
    /// never guessed from.
    pub unknown_codename: Option<String>,
}

/// Codenames, from Oxide's device table and reManager's OS image names.
const CODENAMES: &[(&str, RmModel)] = &[
    ("ferrari", RmModel::PaperPro),
    ("chiappa", RmModel::PaperProMove),
    ("tatsu", RmModel::PaperPure),
];

fn model_from_machine(m: &str) -> Option<RmModel> {
    // Exact prefixes Oxide matches.
    let m = m.trim();
    if m.starts_with("reMarkable 2.0") {
        Some(RmModel::Rm2)
    } else if m.starts_with("reMarkable 1.0") || m.starts_with("reMarkable Prototype 1") {
        Some(RmModel::Rm1)
    } else {
        let l = m.to_ascii_lowercase();
        CODENAMES.iter().find(|(c, _)| l.contains(c)).map(|(_, m)| *m)
    }
}

/// SoC family plus panel size, for devices with an unrecognised machine
/// string. i.MX93 is shared by the Move and the Pure, so it needs the size.
fn model_from_soc(soc: &str, sizes: &[(u32, u32)]) -> Option<RmModel> {
    let has = |m: RmModel| {
        let (w, h) = m.size();
        sizes.iter().any(|&(a, b)| (a, b) == (w, h) || (a, b) == (h, w))
    };
    match soc.trim() {
        s if s.starts_with("i.MX6SL") => Some(RmModel::Rm1),
        s if s.starts_with("i.MX7D") => Some(RmModel::Rm2),
        s if s.starts_with("i.MX8MM") => Some(RmModel::PaperPro),
        s if s.starts_with("i.MX93") => {
            if has(RmModel::PaperProMove) {
                Some(RmModel::PaperProMove)
            } else if has(RmModel::PaperPure) {
                Some(RmModel::PaperPure)
            } else {
                None
            }
        }
        _ => None,
    }
}

/// Parse [`probe_cmd`] output. Order of trust: machine string, device tree,
/// motd codename, hostname codename, then SoC + panel size. A candidate whose triple contradicts
/// `uname -m` is rejected rather than trusted.
pub fn parse_probe(out: &str) -> Probe {
    let mut kv: std::collections::HashMap<&str, &str> = Default::default();
    let mut sizes = Vec::new();
    for line in out.lines() {
        if let Some((k, v)) = line.split_once('=') {
            if k == "size" {
                if let Some((w, h)) = v.trim().split_once('x') {
                    if let (Ok(w), Ok(h)) = (w.trim().parse(), h.trim().parse()) {
                        sizes.push((w, h));
                    }
                }
            } else {
                kv.insert(k, v.trim());
            }
        }
    }
    let get = |k: &str| kv.get(k).copied().unwrap_or("");
    let triple = match get("arch") {
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-gnu"),
        a if a.starts_with("armv7") => Some("armv7-unknown-linux-gnueabihf"),
        _ => None,
    };
    let fits = |m: RmModel| triple.is_none_or(|t| m.triple() == t);

    let motd = get("motd").to_ascii_lowercase();
    let motd_hit = CODENAMES.iter().find(|(c, _)| motd.contains(c)).map(|(_, m)| *m);
    let mut candidates: Vec<(Option<RmModel>, String)> = vec![
        (model_from_machine(get("machine")), format!("machine {:?}", get("machine"))),
        (model_from_machine(get("dtmodel")), format!("device tree {:?}", get("dtmodel"))),
        (motd_hit, "codename in /etc/motd".into()),
        (CODENAMES.iter().find(|(c, _)| get("hostname").to_ascii_lowercase().contains(c)).map(|(_, m)| *m),
            format!("hostname {:?}", get("hostname"))),
        (model_from_soc(get("soc"), &sizes), format!("SoC {:?} with panel {sizes:?}", get("soc"))),
    ];
    let mut rejected = Vec::new();
    let mut chosen = None;
    for (m, why) in candidates.drain(..) {
        match m {
            Some(m) if fits(m) => {
                chosen = Some((m, why));
                break;
            }
            Some(m) => rejected.push(format!("{} from {why} contradicts arch {:?}", m.label(), get("arch"))),
            None => {}
        }
    }
    // A codename-looking word in the motd we have no mapping for.
    let unknown_codename = if motd_hit.is_none() && !motd.is_empty() {
        motd.split(|c: char| !c.is_ascii_alphanumeric() && c != '-')
            .find(|w| w.len() >= 5 && w.chars().all(|c| c.is_ascii_lowercase() || c == '-')
                && !["remarkable", "welcome", "login", "version", "build", "release"].contains(w))
            .map(str::to_string)
    } else {
        None
    };
    let how = match &chosen {
        Some((m, why)) => format!("{} from {why}", m.label()),
        None if rejected.is_empty() => "no identifying evidence; set the model in the roster".into(),
        None => format!("rejected: {}", rejected.join("; ")),
    };
    Probe { triple, model: chosen.map(|(m, _)| m), how, unknown_codename }
}

// ───────────────────────── refresh profile ─────────────────────────

/// Environment variable the on-device e-ink shell reads.
pub const PROFILE_ENV: &str = "EINK_PROFILE";

/// What the app should optimise its refresh policy for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefreshProfile {
    /// Lowest latency that still renders colour. Colour panels only.
    FastColor,
    /// Lowest latency, greyscale. What "fastest" means on a mono panel, and
    /// the escape hatch on a colour panel when colour costs too much.
    FastMono,
    /// Settle-quality refreshes; for reading, not interacting.
    Quality,
}

impl RefreshProfile {
    /// The fastest profile a panel can honour: colour where there is colour.
    pub fn fastest(panel: Panel) -> RefreshProfile {
        match panel {
            Panel::Color => RefreshProfile::FastColor,
            Panel::Mono => RefreshProfile::FastMono,
        }
    }

    pub fn as_env(self) -> &'static str {
        match self {
            RefreshProfile::FastColor => "fast-color",
            RefreshProfile::FastMono => "fast-mono",
            RefreshProfile::Quality => "quality",
        }
    }

    pub fn parse(s: &str) -> Option<RefreshProfile> {
        match s.trim() {
            "fast-color" => Some(RefreshProfile::FastColor),
            "fast-mono" => Some(RefreshProfile::FastMono),
            "quality" => Some(RefreshProfile::Quality),
            _ => None,
        }
    }
}

// ───────────────────────── on-device layout ─────────────────────────

/// Where deployed apps live. Under `/home` because the root filesystem is
/// small and replaced by OS updates. On the Paper Pro family `/home` is
/// encrypted and only mounted after the PIN is entered, so a deploy to a
/// locked tablet fails at the first mkdir, which is the honest failure.
pub const BASE: &str = "/home/root/.local/share/gitmind";

/// App names are interpolated into paths and shell text. Same rule as
/// `fleet::valid_username` plus digits first, because crate names allow it.
pub fn valid_app_name(app: &str) -> bool {
    !app.is_empty()
        && app.len() <= 64
        && app.chars().all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-' || c == '_')
}

pub fn bin_path(app: &str) -> String {
    format!("{BASE}/bin/{app}")
}

/// Staged next to the final path, NOT in /tmp: /tmp is a different
/// filesystem, and only a same-filesystem `mv` is an atomic rename. That
/// atomicity is what lets a new build replace a running one (the old process
/// keeps its inode) without ever exposing a half-written binary.
pub fn staging_path(app: &str) -> String {
    format!("{BASE}/bin/.{app}.new")
}

pub fn pid_path(app: &str) -> String {
    format!("{BASE}/run/{app}.pid")
}

pub fn log_path(app: &str) -> String {
    format!("{BASE}/log/{app}.log")
}

// ───────────────────────── remote commands ─────────────────────────

fn check(app: &str) -> Result<(), String> {
    if valid_app_name(app) {
        Ok(())
    } else {
        Err(format!("refusing app name {app:?}: only [a-z0-9_-] reaches a device shell"))
    }
}

/// Create the layout so a push has somewhere to land.
pub fn prepare_cmd() -> String {
    format!("mkdir -p {}/bin {}/run {}/log", shell_quote(BASE), shell_quote(BASE), shell_quote(BASE))
}

/// Promote the staged binary.
pub fn install_cmd(app: &str) -> Result<String, String> {
    check(app)?;
    let (s, b) = (shell_quote(&staging_path(app)), shell_quote(&bin_path(app)));
    Ok(format!("chmod 755 {s} && mv -f {s} {b}"))
}

/// Stop the instance this tool started, if it is still that instance.
///
/// The pidfile alone is not trusted: after a crash the pid can be reused by
/// an unrelated process. So the pid is only killed if `/proc/<pid>/exe`
/// still points at our binary. After an install that link reads
/// "<path> (deleted)", which the prefix match accepts on purpose.
pub fn stop_cmd(app: &str) -> Result<String, String> {
    check(app)?;
    let (pid, bin) = (shell_quote(&pid_path(app)), bin_path(app));
    Ok(format!(
        "p=$(cat {pid} 2>/dev/null); \
         if [ -n \"$p\" ]; then \
           case \"$(readlink /proc/$p/exe 2>/dev/null)\" in \
             {bin}*) kill \"$p\" 2>/dev/null; \
                     for _ in 1 2 3; do kill -0 \"$p\" 2>/dev/null || break; sleep 1; done; \
                     kill -9 \"$p\" 2>/dev/null;; \
           esac; \
         fi; \
         rm -f {pid}",
        bin = shell_quote(&bin),
    ))
}

/// Everything a launch passes to the app's environment.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LaunchEnv {
    pub profile: RefreshProfile,
    pub model: Option<RmModel>,
    /// The libblight_protocol path the probe found. Passed as
    /// `EINK_BLIGHT_LIB` so the app loads exactly what was routed on, instead
    /// of re-searching a list that may not include where Vellum put it.
    pub blight_lib: Option<String>,
}

impl LaunchEnv {
    pub fn new(profile: RefreshProfile) -> LaunchEnv {
        LaunchEnv { profile, model: None, blight_lib: None }
    }
    fn render(&self) -> String {
        let mut s = format!("{PROFILE_ENV}={} EINK_BACKEND=blight", self.profile.as_env());
        if let Some(m) = self.model {
            s += &format!(" EINK_MODEL={}", m.env_id());
        }
        if let Some(l) = &self.blight_lib {
            s += &format!(" EINK_BLIGHT_LIB={}", shell_quote(l));
        }
        s
    }
}

/// Stop any previous instance, then start detached and print the new pid.
///
/// `nohup env ... BIN &` keeps `$!` equal to the app's pid, because nohup
/// and env both exec. Every fd is redirected so ssh returns immediately
/// instead of waiting on the child's stdout. `args` are passed through
/// individually quoted (e.g. the file `tomescope` should open).
///
/// An SSH launch can only reach the panel through Blight: AppLoad windows
/// exist only for processes AppLoad itself starts (it mints `QTFB_KEY`).
/// So the backend is pinned to blight here, which turns "Oxide missing" into
/// that exact message in the log tail instead of an ambiguous auto-probe.
pub fn launch_cmd(app: &str, env: &LaunchEnv, args: &[&str]) -> Result<String, String> {
    let stop = stop_cmd(app)?;
    let args: String = args.iter().map(|a| format!(" {}", shell_quote(a))).collect();
    Ok(format!(
        "{stop}; \
         cd {base} || exit 1; \
         nohup env {envs} {bin}{args} </dev/null >{log} 2>&1 & \
         echo $! > {pid}; echo \"started $!\"",
        base = shell_quote(BASE),
        envs = env.render(),
        bin = shell_quote(&bin_path(app)),
        log = shell_quote(&log_path(app)),
        pid = shell_quote(&pid_path(app)),
    ))
}

/// Is it still alive after `settle_secs`, and what did it say.
///
/// The settle wait runs on the device so the host side never sleeps, which
/// also keeps the fake transport in tests instantaneous.
pub fn status_cmd(app: &str, settle_secs: u32) -> Result<String, String> {
    check(app)?;
    let pid = shell_quote(&pid_path(app));
    Ok(format!(
        "sleep {settle_secs}; p=$(cat {pid} 2>/dev/null); \
         if [ -n \"$p\" ] && kill -0 \"$p\" 2>/dev/null; then echo \"running $p\"; else echo stopped; fi; \
         echo ---; tail -n 20 {log} 2>/dev/null; true",
        log = shell_quote(&log_path(app)),
    ))
}

/// What [`status_cmd`] reported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Status {
    pub pid: Option<u32>,
    /// The log tail, so a crash on launch shows its own error message.
    pub log_tail: String,
}

pub fn parse_status(out: &str) -> Status {
    let (head, tail) = out.split_once("---").unwrap_or((out, ""));
    let pid = head
        .lines()
        .find_map(|l| l.trim().strip_prefix("running "))
        .and_then(|p| p.trim().parse().ok());
    Status { pid, log_tail: tail.trim_start_matches('\n').trim_end().to_string() }
}

// ───────────────────────── transport + flow ─────────────────────────

/// A channel that can run a shell command on the device and copy a file to
/// it. SSH today; the trait exists for the Bluetooth agent (see module docs).
pub trait Remote {
    fn run(&mut self, cmd: &str) -> Result<String, String>;
    fn push(&mut self, local: &str, remote: &str) -> Result<(), String>;
}

/// SSH over Wi-Fi or USB networking (a reMarkable on USB answers at
/// 10.11.99.1). Uses `deploy`'s hardened ssh options unchanged.
pub struct SshRemote {
    pub user: String,
    pub host: String,
}

impl Remote for SshRemote {
    fn run(&mut self, cmd: &str) -> Result<String, String> {
        // Over USB every tablet is 10.11.99.1: file its key per device.
        crate::tablet_identity::prepare(&self.host);
        crate::deploy::remote_capture(&self.user, &self.host, cmd)
    }
    fn push(&mut self, local: &str, remote: &str) -> Result<(), String> {
        crate::tablet_identity::prepare(&self.host);
        crate::deploy::scp_push(local, &self.user, &self.host, remote)
    }
}

/// Streamed progress, same contract as `reroll::Event`: announced before
/// the work, so a step that hangs is visibly the one that hung.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Preparing,
    Pushing { bytes: Option<u64> },
    Installing,
    Launching { profile: RefreshProfile },
    Checking,
}

/// How a deploy-and-launch ended.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Running { pid: u32 },
    /// Started, then exited within the settle window. The log tail is the
    /// most useful thing we can show: it is the app's own last words.
    ExitedEarly { log_tail: String },
}

/// Push `local_bin` as `app`, promote it, restart it with `profile`, and
/// report whether it survived `settle_secs`.
pub fn deploy_and_launch(
    remote: &mut dyn Remote,
    app: &str,
    local_bin: &str,
    env: &LaunchEnv,
    args: &[&str],
    settle_secs: u32,
    on: &mut dyn FnMut(Event),
) -> Result<Outcome, String> {
    let profile = env.profile;
    check(app)?;
    on(Event::Preparing);
    remote.run(&prepare_cmd()).map_err(|e| format!("prepare: {e}"))?;
    on(Event::Pushing { bytes: std::fs::metadata(local_bin).ok().map(|m| m.len()) });
    remote.push(local_bin, &staging_path(app)).map_err(|e| format!("push: {e}"))?;
    on(Event::Installing);
    remote.run(&install_cmd(app)?).map_err(|e| format!("install: {e}"))?;
    on(Event::Launching { profile });
    remote.run(&launch_cmd(app, env, args)?).map_err(|e| format!("launch: {e}"))?;
    on(Event::Checking);
    let st = parse_status(&remote.run(&status_cmd(app, settle_secs)?).map_err(|e| format!("status: {e}"))?);
    Ok(match st.pid {
        Some(pid) => Outcome::Running { pid },
        None => Outcome::ExitedEarly { log_tail: st.log_tail },
    })
}

// ───────────────────────── enrolment ─────────────────────────
//
// First contact from a phone: the operator reads the root password off the
// tablet (Settings → Help → Copyrights and licenses, at the bottom, on
// current OS versions; the location has moved between releases) and types
// it once. After that every call is key auth.
//
// Order matters and is enforced here, not left to callers:
//   1. pin the host key BEFORE any password is sent (a password to an
//      unverified host is the one mistake enrolment must not make);
//   2. try key auth first, so re-enrolling never asks for the password again
//      and never appends a duplicate authorized_keys line;
//   3. only then install the key with the password;
//   4. probe what the device is and which display servers it has;
//   5. create the app layout.

/// Which display servers the device offers.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Displays {
    /// AppLoad's qtfb socket exists (AppLoad is running inside xochitl).
    pub qtfb: bool,
    /// AppLoad's extension directory, where external app manifests go.
    pub appload_dir: Option<String>,
    /// Path of Oxide's libblight_protocol, if found.
    pub blight_lib: Option<String>,
    /// A D-Bus name containing "blight" is registered (display server up).
    pub blight_running: bool,
    /// glibc version, for pinning the gnu build (`zigbuild --target <triple>.<ver>`).
    pub glibc: Option<String>,
    /// Root filesystem mounted read-only.
    pub rootfs_ro: bool,
    /// `/etc` is the tmpfs-backed overlay: writes there vanish at reboot.
    pub etc_overlay: bool,
    /// xovi's start script exists (AppLoad needs it run once per boot).
    pub xovi_installed: bool,
    /// `rm-ssh-over-wlan` exists (Paper Pro family: SSH over Wi-Fi is off by
    /// default and this turns it on).
    pub wlan_ssh_tool: bool,
    /// The tablet's Wi-Fi IPv4 address, so a first USB session can hand over
    /// to Wi-Fi (a postmarketOS phone's USB host mode is unreliable).
    pub wlan_addr: Option<String>,
    /// Vellum launcherctl: configured launcher, and the one running now.
    pub launcher_current: Option<String>,
    pub launcher_active: Option<String>,
}

/// A one-per-boot step the tablet needs before apps can reach its screen.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Activation {
    /// Run `/home/root/xovi/start`. Restarts the tablet's UI (a few seconds).
    StartXovi,
    /// `launcherctl start-launcher`: start the configured launcher (Oxide).
    StartLauncher,
}

impl Activation {
    pub fn label(self) -> &'static str {
        match self {
            Activation::StartXovi => "start xovi (restarts the tablet's UI)",
            Activation::StartLauncher => "start the launcher (Oxide)",
        }
    }
}

pub const XOVI_START: &str = "/home/root/xovi/start";

/// Turn on SSH over Wi-Fi (Paper Pro family; off by default). Run once over
/// USB; afterwards the tablet answers on its Wi-Fi address (shown under the
/// password in Copyrights and licenses).
pub const WLAN_SSH_ON: &str = "rm-ssh-over-wlan on";
pub const LAUNCHERCTL: &str = "/home/root/.vellum/bin/launcherctl";

/// Run the activation detached (the UI restart can outlive the SSH session),
/// then wait up to 30 s for its sign of life: the qtfb socket for xovi, the
/// launcher reported active for Oxide.
pub fn activation_cmd(a: Activation) -> String {
    match a {
        Activation::StartXovi => format!(
            "nohup {XOVI_START} </dev/null >/dev/null 2>&1 & \
             for _ in $(seq 30); do [ -S /tmp/qtfb.sock ] && echo ready && exit 0; sleep 1; done; \
             echo 'xovi started but no qtfb socket after 30 s (is AppLoad installed?)'; exit 1"
        ),
        Activation::StartLauncher => format!(
            "{LAUNCHERCTL} start-launcher </dev/null >/dev/null 2>&1; \
             for _ in $(seq 30); do a=$({LAUNCHERCTL} active-launcher 2>/dev/null | head -n1); \
             case \"$a\" in ''|none) sleep 1;; *) echo \"ready $a\"; exit 0;; esac; done; \
             echo 'launcher did not report active after 30 s'; exit 1"
        ),
    }
}

/// How an app gets onto this device's screen.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Route {
    /// Oxide's display server is up: push, then launch over SSH.
    Blight,
    /// AppLoad is installed: push, write a manifest, the operator taps the icon.
    AppLoad { dir: String },
}

impl Displays {
    /// Is a display server up right now? (Not "installed": up.)
    pub fn screen_ready(&self) -> bool {
        self.blight_running || self.qtfb
    }

    /// Route plus the per-boot step it still needs, if any. Prefers a route
    /// that is ready now; otherwise the one that one activation would make
    /// ready, Blight first.
    pub fn readiness(&self) -> Result<(Route, Option<Activation>), String> {
        if let Ok(r) = self.route() {
            let qtfb_missing = matches!(r, Route::AppLoad { .. }) && !self.qtfb;
            let oxide_idle = self.launcher_current.is_some() && self.launcher_active.is_none() && self.blight_lib.is_some();
            return Ok(match (&r, oxide_idle, qtfb_missing) {
                (Route::AppLoad { .. }, true, _) => (Route::Blight, Some(Activation::StartLauncher)),
                (Route::AppLoad { .. }, false, true) if self.xovi_installed => (r, Some(Activation::StartXovi)),
                _ => (r, None),
            });
        }
        if self.launcher_current.is_some() && self.blight_lib.is_some() {
            return Ok((Route::Blight, Some(Activation::StartLauncher)));
        }
        self.route().map(|r| (r, None))
    }

    /// Blight first: it is the only route gitmind can launch remotely, and
    /// the one with per-update waveforms.
    pub fn route(&self) -> Result<Route, String> {
        if self.blight_running && self.blight_lib.is_some() {
            return Ok(Route::Blight);
        }
        if let Some(dir) = &self.appload_dir {
            return Ok(Route::AppLoad { dir: dir.clone() });
        }
        Err(match (self.blight_lib.is_some(), self.blight_running) {
            (true, false) => "Oxide is installed but its display server is not running, and AppLoad was not found".into(),
            _ => "no display route: install Oxide (Blight) or AppLoad with Vellum (reManager can do this)".into(),
        })
    }
}

/// One round-trip listing display servers. Paths are best knowledge, not
/// verified on hardware: AppLoad's directory is xovi's per-extension home,
/// and Oxide's library location depends on how Vellum packages it, so the
/// probe also looks under /home/root/.vellum. Each line is `key=value`.
pub fn displays_probe_cmd() -> &'static str {
    "[ -S /tmp/qtfb.sock ] && echo qtfb=1; \
     for d in /home/root/xovi/exthome/appload; do [ -d \"$d\" ] && echo \"appload_dir=$d\"; done; \
     for f in /opt/lib/libblight_protocol.so* /usr/lib/libblight_protocol.so* /home/root/.vellum/lib/libblight_protocol.so*; do \
       [ -e \"$f\" ] && echo \"blight_lib=$f\" && break; done; \
     dbus-send --system --print-reply --dest=org.freedesktop.DBus /org/freedesktop/DBus org.freedesktop.DBus.ListNames 2>/dev/null \
       | grep -qi blight && echo blight_running=1; \
     v=$(ldd --version 2>&1 | head -n1 | grep -o '[0-9]\\+\\.[0-9]\\+$'); [ -n \"$v\" ] && echo \"glibc=$v\"; \
     awk '$2==\"/\" && $4 ~ /(^|,)ro(,|$)/ {print \"rootfs_ro=1\"; exit}' /proc/mounts 2>/dev/null; \
     awk '$1==\"overlay\" && $2==\"/etc\" {print \"etc_overlay=1\"; exit}' /proc/mounts 2>/dev/null; \
     [ -x /home/root/xovi/start ] && echo xovi_installed=1; \
     command -v rm-ssh-over-wlan >/dev/null 2>&1 && echo wlan_ssh_tool=1; \
     a=$(ip -4 -o addr show wlan0 2>/dev/null | awk '{print $4}' | cut -d/ -f1 | head -n1); [ -n \"$a\" ] && echo \"wlan_addr=$a\"; \
     L=/home/root/.vellum/bin/launcherctl; if [ -x $L ]; then \
       echo \"launcher_current=$($L current-launcher 2>/dev/null | head -n1)\"; \
       echo \"launcher_active=$($L active-launcher 2>/dev/null | head -n1)\"; fi; \
     true"
}

/// Every key `displays_probe_cmd` can emit. The test that runs the probe
/// checks its output against this, so a new probe line without parser
/// support fails a test instead of being silently ignored.
pub const DISPLAY_KEYS: &[&str] = &[
    "qtfb", "appload_dir", "blight_lib", "blight_running", "glibc",
    "rootfs_ro", "etc_overlay", "xovi_installed", "wlan_ssh_tool", "wlan_addr", "launcher_current", "launcher_active",
];

/// A launcherctl name, or None for "none"/empty. Output format unverified:
/// takes the first word, ignoring a leading `*` marker.
fn launcher_name(v: &str) -> Option<String> {
    let w = v.split_whitespace().find(|w| *w != "*")?.trim_start_matches('*');
    (!w.is_empty() && w != "none").then(|| w.to_string())
}

pub fn parse_displays(out: &str) -> Displays {
    let mut d = Displays::default();
    for line in out.lines().map(str::trim) {
        match line.split_once('=') {
            Some(("qtfb", "1")) => d.qtfb = true,
            Some(("appload_dir", v)) if !v.is_empty() => d.appload_dir = Some(v.to_string()),
            Some(("blight_lib", v)) if !v.is_empty() => d.blight_lib = Some(v.to_string()),
            Some(("blight_running", "1")) => d.blight_running = true,
            Some(("glibc", v)) if !v.is_empty() => d.glibc = Some(v.to_string()),
            Some(("rootfs_ro", "1")) => d.rootfs_ro = true,
            Some(("etc_overlay", "1")) => d.etc_overlay = true,
            Some(("xovi_installed", "1")) => d.xovi_installed = true,
            Some(("wlan_ssh_tool", "1")) => d.wlan_ssh_tool = true,
            Some(("wlan_addr", v)) if !v.is_empty() => d.wlan_addr = Some(v.to_string()),
            Some(("launcher_current", v)) => d.launcher_current = launcher_name(v),
            Some(("launcher_active", v)) => d.launcher_active = launcher_name(v),
            _ => {}
        }
    }
    d
}

/// Host-side effects enrolment needs. Real implementation over SSH below;
/// faked in tests so the ordering rules above are pinned.
pub trait Enroller {
    fn trust_host(&mut self) -> Result<String, String>;
    /// Does key auth already work? (BatchMode, never prompts.)
    fn key_check(&mut self) -> KeyCheck;
    fn install_key(&mut self, password: &str) -> Result<String, String>;
    fn run(&mut self, cmd: &str) -> Result<String, String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EnrollEvent {
    TrustingHost,
    Trusted(String),
    KeyAlreadyWorks,
    InstallingKey,
    KeyInstalled(String),
    Probing,
    Preparing,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Enrolled {
    pub probe: Probe,
    pub displays: Displays,
    /// True if this run used the password (first contact).
    pub used_password: bool,
}

pub const HOSTKEY_CHANGED: &str = "the tablet's SSH host key has changed since it was trusted. \
    That is expected after an OS update (every update regenerates it), and it is also what an \
    interception looks like. If the tablet was just updated and it is on a cable or network you \
    control, re-trust it.";

/// Enrol a tablet. `password` may be `None` when a key is already installed;
/// if it is needed and missing, the error says where to find it.
pub fn enroll(e: &mut dyn Enroller, password: Option<&str>, on: &mut dyn FnMut(EnrollEvent)) -> Result<Enrolled, String> {
    on(EnrollEvent::TrustingHost);
    let t = e.trust_host()?;
    on(EnrollEvent::Trusted(t));
    let first = e.key_check();
    match first {
        KeyCheck::HostKeyChanged => return Err(HOSTKEY_CHANGED.into()),
        KeyCheck::LegacyHostKey => return Err(LEGACY_HOSTKEY.into()),
        _ => {}
    }
    let used_password = if first == KeyCheck::Works {
        on(EnrollEvent::KeyAlreadyWorks);
        false
    } else {
        let pw = password.filter(|p| !p.is_empty()).ok_or_else(|| {
            "first contact needs the tablet's SSH password: on the tablet, Settings → General → Help → \
             About → Copyrights and licenses, under General information (it lists the username too); \
             developer mode must be enabled, which factory-resets the tablet"
                .to_string()
        })?;
        on(EnrollEvent::InstallingKey);
        let r = e.install_key(pw)?;
        on(EnrollEvent::KeyInstalled(r));
        if e.key_check() != KeyCheck::Works {
            return Err("key was installed but key auth still fails (sshd may not read ~/.ssh/authorized_keys)".into());
        }
        true
    };
    on(EnrollEvent::Probing);
    let probe = parse_probe(&e.run(probe_cmd())?);
    let displays = parse_displays(&e.run(displays_probe_cmd())?);
    on(EnrollEvent::Preparing);
    e.run(&prepare_cmd())?;
    Ok(Enrolled { probe, displays, used_password })
}

/// The default SSH user on every reMarkable model. reMarkable's own
/// developer-mode page says the username shown is "root at the time of
/// writing", so it is a default, never a constant: the Tablets page and
/// `--user` / `GITMIND_RM_USER` override it.
pub const DEFAULT_USER: &str = "root";

/// The real enroller over the hardened ssh options.
pub struct SshEnroller {
    pub user: String,
    pub host: String,
}

/// The public keys installed on a tablet: the usual key plus an ECDSA one.
/// The reMarkable 2's dropbear does not accept ed25519 (the key gitmind
/// generates by default), and ssh offers every default identity in turn, so
/// installing both makes one login work on every model.
pub fn tablet_pubkeys() -> Result<String, String> {
    let main = crate::deploy::ensure_local_pubkey()?;
    let ecdsa = crate::deploy::ensure_keypair("ecdsa")?;
    if main.split_whitespace().next() == ecdsa.split_whitespace().next() {
        return Ok(main);
    }
    Ok(format!("{}\n{}\n", main.trim_end(), ecdsa.trim_end()))
}

impl Enroller for SshEnroller {
    fn trust_host(&mut self) -> Result<String, String> {
        // A tablet on a USB cable: pin its key per DEVICE (every reMarkable
        // is 10.11.99.1; per-address trust made a second tablet look like
        // an interception of the first).
        match crate::tablet_identity::prepare(&self.host) {
            Some(link) => crate::tablet_identity::trust(&self.host, &link),
            None => crate::deploy::trust_host(&self.host),
        }
    }
    fn key_check(&mut self) -> KeyCheck {
        crate::tablet_identity::prepare(&self.host);
        match crate::deploy::remote_capture(&self.user, &self.host, "true") {
            Ok(_) => KeyCheck::Works,
            Err(e) if crate::deploy::ssh_error_reason(&e) == "hostkey" => KeyCheck::HostKeyChanged,
            Err(e) if crate::deploy::ssh_error_reason(&e) == "legacy-hostkey" => KeyCheck::LegacyHostKey,
            Err(_) => KeyCheck::Fails,
        }
    }
    fn install_key(&mut self, password: &str) -> Result<String, String> {
        crate::tablet_identity::prepare(&self.host);
        crate::deploy::setup_key_with(&self.user, &self.host, password, &tablet_pubkeys()?)
    }
    fn run(&mut self, cmd: &str) -> Result<String, String> {
        crate::tablet_identity::prepare(&self.host);
        crate::deploy::remote_capture(&self.user, &self.host, cmd)
    }
}

// ───────────────────────── persistence ─────────────────────────

/// Everything gitmind writes on a tablet must live here: the one tree that
/// survives a reboot on every model (reManager draws the same line).
pub const PERSISTENT_ROOT: &str = "/home/root/";

/// Refuse a write target outside the persistent tree. A file written to
/// the volatile `/etc` "works" until the next reboot, which is the worst
/// kind of working.
pub fn assert_persistent(path: &str) -> Result<(), String> {
    let clean = std::path::Path::new(path);
    let ok = clean.is_absolute()
        && path.starts_with(PERSISTENT_ROOT)
        && !clean.components().any(|c| matches!(c, std::path::Component::ParentDir));
    if ok { Ok(()) } else { Err(format!("{path}: gitmind only writes under {PERSISTENT_ROOT} on a tablet (the rest resets at boot)")) }
}

/// How key auth stands, distinguishing a changed host key (never auto-fixed).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum KeyCheck {
    Works,
    Fails,
    /// The pinned host key no longer matches. Expected after an OS update,
    /// which regenerates it; also exactly what an interception looks like,
    /// so only the operator decides.
    HostKeyChanged,
    /// The tablet only offers ssh-rsa, which modern OpenSSH refuses.
    LegacyHostKey,
}

pub const LEGACY_HOSTKEY: &str = "the tablet only offers an ssh-rsa host key (older OS versions), which this \
    phone's OpenSSH refuses by default. Add to ~/.ssh/config for this host:\n  \
    HostKeyAlgorithms +ssh-rsa\n  PubkeyAcceptedKeyTypes +ssh-rsa";

// ───────────────────────── building for a tablet ─────────────────────────

/// What to build for a device, decided by its ROUTE, not just its CPU.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildTarget {
    pub triple: &'static str,
    /// Link statically (`+crt-static`). True for the AppLoad route.
    pub static_crt: bool,
    /// Where the app runs: the tablet's own system, or the Alpine root.
    pub userland: Userland,
    /// Container image override (the Alpine root's release; see
    /// `alpine_root::image_for_release`). None: CONTAINER_IMAGE.
    pub container_image: Option<String>,
    /// One line on why, shown next to the build.
    pub why: &'static str,
}

/// * AppLoad/qtfb: the app talks to a socket and shared memory and loads no
///   system library, so a STATIC musl binary runs on any tablet OS version.
///   On a postmarketOS phone (musl, aarch64) `aarch64-unknown-linux-musl` is
///   the native target: the phone builds Paper Pro apps with no cross
///   toolchain.
/// * Blight: the app dlopens the tablet's glibc `libblight_protocol`, which a
///   static binary cannot do, so it must be a gnu build pinned to the
///   tablet's glibc (see `Displays::glibc`).
/// Where a tablet app runs.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Userland {
    /// On the tablet's own system: a static binary, no system libraries.
    #[default]
    Static,
    /// Inside the Alpine root gitmind keeps under /home (alpine_root),
    /// launched through alpine-run: a DYNAMIC musl build that links the
    /// root's libraries, which is what runtime-loaded plugins need (ALSA's
    /// bluez-alsa plugin: Bluetooth audio; D-Bus for BLE).
    AlpineRoot,
}

impl Userland {
    pub fn parse(s: &str) -> Option<Userland> {
        match s {
            "static" | "system" => Some(Userland::Static),
            "alpine" | "alpine-root" => Some(Userland::AlpineRoot),
            _ => None,
        }
    }
}

/// [`build_target`] for a chosen userland.
pub fn build_target_for(route: &Route, model: Option<RmModel>, userland: Userland) -> BuildTarget {
    let mut t = build_target(route, model);
    if userland == Userland::AlpineRoot && matches!(route, Route::AppLoad { .. }) {
        t.static_crt = false;
        t.userland = Userland::AlpineRoot;
        t.why = "Alpine root: dynamic musl linking the root's libraries (ALSA plugins, Bluetooth audio, D-Bus), launched through alpine-run";
    }
    t
}

pub fn build_target(route: &Route, model: Option<RmModel>) -> BuildTarget {
    let arm32 = matches!(model, Some(RmModel::Rm1 | RmModel::Rm2));
    match route {
        Route::AppLoad { .. } => BuildTarget {
            triple: if arm32 { "armv7-unknown-linux-musleabihf" } else { "aarch64-unknown-linux-musl" },
            static_crt: true,
            userland: Userland::Static,
            container_image: None,
            why: "AppLoad: static musl, runs on any tablet OS and builds natively on an aarch64 postmarketOS phone",
        },
        Route::Blight => BuildTarget {
            triple: if arm32 { "armv7-unknown-linux-gnueabihf" } else { "aarch64-unknown-linux-gnu" },
            static_crt: false,
            userland: Userland::Static,
            container_image: None,
            why: "Blight: glibc build (loads the tablet's libblight_protocol); pin glibc to the tablet's",
        },
    }
}

/// argv + env for `cargo build` of one package for a tablet.
///
/// Deliberately NOT `buildflags::build_env`: that classifies any egui app as
/// windowed and makes musl builds DYNAMIC so winit can dlopen Wayland. A
/// dynamic musl binary needs musl's loader, which a reMarkable does not have:
/// it would be pushed and never start. The tablet env forces `+crt-static`
/// and merges (never drops) an inherited RUSTFLAGS.
///
/// `native`: build for the host with no `--target`. That is the phone case.
/// postmarketOS's rustc (Alpine's) targets `aarch64-alpine-linux-musl` and
/// ships std only for it, so `--target aarch64-unknown-linux-musl` fails
/// there with "can't find crate for std"; Alpine also makes musl builds
/// dynamic by default, which `+crt-static` overrides. See [`native_for`].
pub fn tablet_build_cmd(app: &TabletApp, t: &BuildTarget, inherited_rustflags: Option<&str>, native: bool) -> (Vec<String>, Vec<(String, String)>) {
    tablet_build_cmd_on_path(app, t, inherited_rustflags, native, &std::env::var("PATH").unwrap_or_default())
}

/// [`tablet_build_cmd`], probing `path_env` for a cross linker.
///
/// Static musl cross builds (a laptop building for the AppLoad route) fall
/// back to Rust's bundled `rust-lld` with self-contained linking when no
/// real musl cross linker is on PATH. gitmind's general cross env names
/// `aarch64-linux-musl-gcc`, which most distros do not package; the tablet
/// build is pure Rust (no C), and Rust ships musl's crt objects and libc for
/// its musl targets, so lld can do the static link on its own.
/// UNVERIFIED here: no aarch64 musl std in the sandbox this was written in.
pub fn tablet_build_cmd_on_path(app: &TabletApp, t: &BuildTarget, inherited_rustflags: Option<&str>, native: bool, path_env: &str) -> (Vec<String>, Vec<(String, String)>) {
    // `--bin`: works in a single-crate dir and inside a workspace alike.
    let argv = crate::buildflags::cargo_build_argv_kind(&app.bin, if native { "" } else { t.triple }, &app.features, app.no_default, true);
    let mut env = if native { Vec::new() } else { crate::buildflags::cross_env_on_path(t.triple, path_env) };
    let mut flags: Vec<&str> = Vec::new();
    if t.static_crt {
        flags.push("-C target-feature=+crt-static");
        if !native {
            let var = crate::buildflags::linker_var(t.triple);
            let real = env.iter().find(|(k, _)| *k == var).is_some_and(|(_, l)| crate::nixtoolchain::on_path(l, path_env));
            if !real {
                env.retain(|(k, _)| *k != var);
                env.push((var, "rust-lld".into()));
                flags.push("-C link-self-contained=yes");
            }
        }
    }
    if !flags.is_empty() {
        let mine = flags.join(" ");
        let merged = match inherited_rustflags.map(str::trim).filter(|s| !s.is_empty()) {
            Some(existing) => format!("{existing} {mine}"),
            None => mine,
        };
        env.push(("RUSTFLAGS".into(), merged));
    }
    (argv, env)
}

/// A Rust toolchain found on this machine.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Toolchain {
    pub cargo: std::path::PathBuf,
    pub rustc: std::path::PathBuf,
    /// Where it was found, for the log: `$CARGO`, `rustup (~/.cargo/bin)`, a PATH dir.
    pub origin: String,
}

/// Every toolchain worth trying, best first: `$CARGO`, rustup's
/// `~/.cargo/bin`, then each PATH directory with both `cargo` and `rustc`.
///
/// Why not just "cargo on PATH": on Fedora (and elsewhere) the distro's
/// rustc/cargo in /usr/bin coexist with rustup's. `rustup target add`
/// installs into rustup's toolchain; PATH order decides which one runs; and
/// a GUI started from the desktop launcher often has no ~/.cargo/bin on PATH
/// at all (rustup adds it from the shell profile). Found in the first real
/// device session: the target was installed and gitmind could not see it.
pub fn toolchains(path_env: &str, home: Option<&str>, cargo_env: Option<&str>) -> Vec<Toolchain> {
    use std::path::{Path, PathBuf};
    let mut out: Vec<Toolchain> = Vec::new();
    let mut push = |cargo: PathBuf, rustc: PathBuf, origin: String| {
        if cargo.is_file() && rustc.is_file() {
            let key = std::fs::canonicalize(&cargo).unwrap_or(cargo.clone());
            if !out.iter().any(|t| std::fs::canonicalize(&t.cargo).unwrap_or(t.cargo.clone()) == key) {
                out.push(Toolchain { cargo, rustc, origin });
            }
        }
    };
    if let Some(c) = cargo_env.filter(|c| !c.is_empty()) {
        let c = PathBuf::from(c);
        let rustc = c.with_file_name("rustc");
        push(c, rustc, "$CARGO".into());
    }
    if let Some(h) = home {
        let bin = Path::new(h).join(".cargo/bin");
        push(bin.join("cargo"), bin.join("rustc"), "rustup (~/.cargo/bin)".into());
    }
    for dir in path_env.split(':').filter(|d| !d.is_empty()) {
        let d = Path::new(dir);
        push(d.join("cargo"), d.join("rustc"), format!("PATH ({dir})"));
    }
    out
}

/// Does this toolchain have `triple`'s std? Asks its rustc for the target's
/// libdir, which is exact for rustup and distro toolchains alike.
pub fn toolchain_has_target(t: &Toolchain, triple: &str) -> Option<bool> {
    let out = std::process::Command::new(&t.rustc).args(["--print", "target-libdir", "--target", triple]).output().ok()?;
    if !out.status.success() {
        return Some(false);
    }
    let dir = String::from_utf8_lossy(&out.stdout).trim().to_string();
    Some(!dir.is_empty() && std::path::Path::new(&dir).is_dir())
}

/// The toolchain to build `triple` with, or an error that lists every
/// toolchain checked (so "installed but not found" explains itself).
/// For a native build any toolchain will do: the first is returned.
pub fn pick_toolchain(triple: &str, native: bool, path_env: &str, home: Option<&str>, cargo_env: Option<&str>) -> Result<Toolchain, String> {
    let all = toolchains(path_env, home, cargo_env);
    if all.is_empty() {
        return Err(format!("no Rust toolchain found (looked at $CARGO, ~/.cargo/bin and PATH={path_env})"));
    }
    if native {
        return Ok(all[0].clone());
    }
    for t in &all {
        if toolchain_has_target(t, triple) == Some(true) {
            return Ok(t.clone());
        }
    }
    let checked: Vec<String> = all.iter().map(|t| format!("{} ({})", t.rustc.display(), t.origin)).collect();
    Err(format!("{}; checked: {}", missing_target_message(triple), checked.join(", ")))
}

/// [`pick_toolchain`] with this process's environment.
pub fn pick_toolchain_here(triple: &str, native: bool) -> Result<Toolchain, String> {
    pick_toolchain(triple, native, &std::env::var("PATH").unwrap_or_default(),
        std::env::var("HOME").ok().as_deref(), std::env::var("CARGO").ok().as_deref())
}

/// Point a build at one toolchain: its cargo as argv[0], and RUSTC set to the
/// matching rustc (cargo otherwise runs `rustc` from PATH, which may be the
/// OTHER toolchain, the one without the target).
pub fn use_toolchain(argv: &mut [String], env: &mut Vec<(String, String)>, t: &Toolchain) {
    if let Some(a0) = argv.first_mut() {
        *a0 = t.cargo.to_string_lossy().to_string();
    }
    env.retain(|(k, _)| k != "RUSTC");
    env.push(("RUSTC".into(), t.rustc.to_string_lossy().to_string()));
}

/// The finding for a missing target std (also produced by `diagnose` from
/// cargo's own error text).
pub fn missing_target_message(triple: &str) -> String {
    format!("the Rust standard library for {triple} is not installed on this machine")
}

/// A buildable tablet app in the monorepo: an eink-kit binary.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TabletApp {
    /// Crate directory (where `cargo build` runs).
    pub dir: std::path::PathBuf,
    pub bin: String,
    /// Features to enable for the tablet build (the one that turns on
    /// eink-kit, when it is optional).
    pub features: Vec<String>,
    /// Build without default features: set when eink-kit is behind a
    /// feature, because the defaults are then the DESKTOP shell (eframe,
    /// winit), which must not be compiled into a tablet binary.
    pub no_default: bool,
    /// An ordinary eframe app, built with eink-kit's eframe shim patched in
    /// (`shared/eink-kit/eframe-shim`) and `EINK_SHIM=1`: no source changes.
    pub via_shim: bool,
}

/// Where the eframe shim lives, relative to the repo root.
pub const SHIM_REL: &str = "shared/eink-kit/eframe-shim";

/// The cargo argument that swaps eframe for the shim at `shim_path`.
pub fn shim_config_arg(shim_path: &str) -> String {
    format!("patch.crates-io.eframe.path=\"{shim_path}\"")
}

/// Binaries of an ordinary eframe app: crates depending on `eframe` (not as
/// a dev-dependency). An optional eframe is enabled with the feature that
/// names it, unless a default feature already does. `(bin, features)`; a
/// `None` bin means "the package's own binary" (checked against src/main.rs).
pub fn eframe_app_bins(cargo_toml: &str) -> Vec<(Option<String>, Vec<String>)> {
    let mut section = String::new();
    let (mut pkg, mut dep, mut features, mut bins) = (None::<String>, None::<bool>, Vec::<(String, String)>::new(), Vec::<(String, String)>::new());
    for raw in cargo_toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.to_string();
            if section == "[[bin]]" {
                bins.push((String::new(), String::new()));
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        match section.as_str() {
            "[package]" if k == "name" => pkg = Some(v.trim_matches('"').to_string()),
            "[dependencies]" if k == "eframe" => dep = Some(v.contains("optional") && v.contains("true")),
            "[features]" => features.push((k.to_string(), v.to_string())),
            "[[bin]]" => {
                if let Some(last) = bins.last_mut() {
                    if k == "name" { last.0 = v.trim_matches('"').to_string(); }
                    if k == "required-features" { last.1 = v.to_string(); }
                }
            }
            _ => {}
        }
    }
    let Some(optional) = dep else { return Vec::new() };
    if pkg.as_deref() == Some("eframe") || pkg.as_deref() == Some("eink-kit") {
        return Vec::new();
    }
    let q = |list: &str, w: &str| list.contains(&format!("\"{w}\""));
    let default = features.iter().find(|(f, _)| f == "default").map(|(_, l)| l.clone()).unwrap_or_default();
    let gate = if optional {
        features.iter().find(|(f, l)| f != "default" && (q(l, "dep:eframe") || q(l, "eframe"))).map(|(f, _)| f.clone())
    } else {
        None
    };
    let wanted: Vec<String> = match &gate {
        Some(g) if !q(&default, g) => vec![g.clone()],
        _ => vec![],
    };
    if optional && gate.is_none() {
        return Vec::new(); // optional eframe no feature enables: not an app
    }
    if bins.is_empty() {
        return vec![(None, wanted)];
    }
    bins.into_iter()
        .filter(|(n, req)| !n.is_empty() && (req.is_empty() || gate.as_ref().is_some_and(|g| q(req, g))))
        .map(|(n, _)| (Some(n), wanted.clone()))
        .collect()
}

/// The tablet binaries a crate offers.
///
/// * eink-kit itself, or a crate depending on it unconditionally: every
///   binary (or the package name when none are declared).
/// * eink-kit behind a feature (the tomescope shape: `eink =
///   ["dep:eink-kit"]`, desktop shell on by default): only the binaries whose
///   `required-features` include that feature, built with it and without
///   default features.
pub fn eink_app_bins(cargo_toml: &str) -> Vec<(String, Vec<String>, bool)> {
    let mut section = String::new();
    let mut pkg: Option<String> = None;
    let mut is_kit = false;
    let mut dep: Option<bool> = None; // Some(optional?)
    let mut features: Vec<(String, String)> = Vec::new(); // (feature, raw list)
    let mut bins: Vec<(String, String)> = Vec::new(); // (name, raw required-features)
    for raw in cargo_toml.lines() {
        let line = raw.split('#').next().unwrap_or("").trim();
        if line.starts_with('[') {
            section = line.to_string();
            if section == "[[bin]]" {
                bins.push((String::new(), String::new()));
            }
            continue;
        }
        let Some((k, v)) = line.split_once('=') else { continue };
        let (k, v) = (k.trim(), v.trim());
        let unq = v.trim_matches('"');
        match section.as_str() {
            "[package]" if k == "name" => {
                is_kit = unq == "eink-kit";
                pkg = Some(unq.to_string());
            }
            "[[bin]]" => {
                if let Some(last) = bins.last_mut() {
                    if k == "name" { last.0 = unq.to_string(); }
                    if k == "required-features" { last.1 = v.to_string(); }
                }
            }
            "[features]" => features.push((k.to_string(), v.to_string())),
            s if s.contains("dependencies") && k == "eink-kit" => {
                dep = Some(v.contains("optional") && v.contains("true"));
            }
            _ => {}
        }
    }
    let quoted = |list: &str, want: &str| list.contains(&format!("\"{want}\""));
    match (is_kit, dep) {
        (true, _) | (false, Some(false)) => {
            let names: Vec<String> = if bins.is_empty() {
                pkg.into_iter().filter(|p| p != "eink-kit").collect()
            } else {
                bins.into_iter().map(|b| b.0).collect()
            };
            names.into_iter().map(|n| (n, vec![], false)).collect()
        }
        (false, Some(true)) => {
            let gates: Vec<String> = features.iter()
                .filter(|(_, list)| quoted(list, "dep:eink-kit") || quoted(list, "eink-kit"))
                .map(|(f, _)| f.clone())
                .collect();
            bins.into_iter()
                .filter_map(|(name, req)| {
                    let f = gates.iter().find(|g| quoted(&req, g))?;
                    Some((name, vec![f.clone()], true))
                })
                .collect()
        }
        (false, None) => Vec::new(),
    }
}

/// Every eink-kit app under `root/{apps,shared,games}` (a few levels of
/// crates deep, skipping build output). `games` holds unendlich-eink.
pub fn discover_tablet_apps(root: &std::path::Path) -> Vec<TabletApp> {
    let mut out = Vec::new();
    let mut stack: Vec<(std::path::PathBuf, u8)> = ["apps", "shared", "games"].iter().map(|d| (root.join(d), 0)).collect();
    while let Some((dir, depth)) = stack.pop() {
        if let Ok(t) = std::fs::read_to_string(dir.join("Cargo.toml")) {
            let mut eink = eink_app_bins(&t);
            // A tablet tool opting in explicitly (alpine-run):
            // [package.metadata.gitmind] tablet = true
            if eink.is_empty() && t.contains("[package.metadata.gitmind]") && t.lines().any(|l| l.replace(' ', "") == "tablet=true") {
                if let Some(pkg) = t.lines().skip_while(|l| l.trim() != "[package]").find_map(|l| l.trim().strip_prefix("name").and_then(|r| r.split('"').nth(1))) {
                    eink.push((pkg.to_string(), vec![], false));
                }
            }
            let had_eink = !eink.is_empty();
            for (bin, features, no_default) in eink {
                out.push(TabletApp { dir: dir.clone(), bin, features, no_default, via_shim: false });
            }
            // An ordinary eframe app, unless it already has an eink-kit shell.
            if !had_eink && dir.join("src").is_dir() {
                let pkg = t.lines().skip_while(|l| l.trim() != "[package]").find_map(|l| l.trim().strip_prefix("name").and_then(|r| r.split('"').nth(1)).map(str::to_string));
                for (bin, features) in eframe_app_bins(&t) {
                    let bin = match bin {
                        Some(b) => b,
                        None if dir.join("src/main.rs").is_file() => match &pkg { Some(p) => p.clone(), None => continue },
                        None => continue,
                    };
                    out.push(TabletApp { dir: dir.clone(), bin, features, no_default: false, via_shim: true });
                }
            }
        }
        if depth >= 3 {
            continue;
        }
        if let Ok(rd) = std::fs::read_dir(&dir) {
            for e in rd.flatten() {
                let n = e.file_name();
                if e.path().is_dir() && n != "target" && n != "node_modules" && !n.to_string_lossy().starts_with('.') {
                    stack.push((e.path(), depth + 1));
                }
            }
        }
    }
    out.sort_by(|a, b| a.bin.cmp(&b.bin));
    out
}

/// Can this machine build `t` natively? True when gitmind itself runs on the
/// same CPU and libc as the target, i.e. on an aarch64 postmarketOS phone
/// building for the AppLoad route of a Paper Pro-family tablet.
pub fn native_for(t: &BuildTarget) -> bool {
    let arch = if t.triple.starts_with("aarch64") { "aarch64" } else { "arm" };
    std::env::consts::ARCH == arch && cfg!(target_env = "musl") && t.triple.contains("musl")
}

/// Where cargo puts the binary for `tablet_build_cmd`, relative to the crate
/// (or workspace) directory.
pub fn tablet_build_output(bin: &str, t: &BuildTarget, native: bool) -> String {
    if native { format!("target/release/{bin}") } else { format!("target/{}/release/{bin}", t.triple) }
}

/// Read a local binary and check it suits the device's route before any
/// byte is pushed. Used by the CLI and the Tablets page alike.
pub fn preflight_binary(path: &str, route: &Route, model: Option<RmModel>) -> Result<BuildTarget, String> {
    preflight_binary_for(path, route, model, Userland::Static)
}

/// [`preflight_binary`] for a chosen userland (the Alpine root accepts a
/// musl-dynamic binary; the tablet's own system needs a static one).
pub fn preflight_binary_for(path: &str, route: &Route, model: Option<RmModel>, userland: Userland) -> Result<BuildTarget, String> {
    let t = build_target_for(route, model, userland);
    let bytes = std::fs::read(path).map_err(|e| format!("{path}: {e}"))?;
    check_tablet_binary(&bytes, &t).map_err(|e| format!("{path}: {e}"))?;
    Ok(t)
}

/// Is this file something the tablet can run on this route? Checks the ELF
/// machine (aarch64 / 32-bit ARM) and, for the static AppLoad route, that
/// there is no dynamic loader (`PT_INTERP`). Catches the likeliest mistake
/// (pushing a desktop build) and the subtlest (a dynamic musl build).
pub fn check_tablet_binary(bytes: &[u8], t: &BuildTarget) -> Result<(), String> {
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" {
        return Err("not an ELF executable".into());
    }
    let is64 = match bytes[4] { 1 => false, 2 => true, _ => return Err("unknown ELF class".into()) };
    if bytes[5] != 1 {
        return Err("big-endian ELF; tablets are little-endian".into());
    }
    let u16at = |o: usize| u16::from_le_bytes([bytes[o], bytes[o + 1]]);
    let u32at = |o: usize| -> Option<u32> { Some(u32::from_le_bytes(bytes.get(o..o + 4)?.try_into().ok()?)) };
    let u64at = |o: usize| -> Option<u64> { Some(u64::from_le_bytes(bytes.get(o..o + 8)?.try_into().ok()?)) };
    let machine = u16at(18);
    let want = if t.triple.starts_with("aarch64") { 183 } else { 40 };
    let name = |m: u16| match m { 183 => "aarch64", 40 => "32-bit ARM", 62 => "x86_64 (a desktop build?)", 3 => "x86", _ => "another architecture" };
    if machine != want {
        return Err(format!("built for {}, the tablet needs {}", name(machine), name(want)));
    }
    if is64 != t.triple.starts_with("aarch64") {
        return Err("ELF class does not match the target".into());
    }
    let (phoff, phentsize, phnum) = if is64 {
        (u64at(0x20).ok_or("truncated ELF")? as usize, u16at(0x36) as usize, u16at(0x38) as usize)
    } else {
        (u32at(0x1c).ok_or("truncated ELF")? as usize, u16at(0x2a) as usize, u16at(0x2c) as usize)
    };
    let mut dynamic = false;
    for i in 0..phnum {
        match u32at(phoff + i * phentsize) {
            Some(3) => dynamic = true, // PT_INTERP
            Some(_) => {}
            None => return Err("truncated program headers".into()),
        }
    }
    if t.static_crt && dynamic {
        return Err("dynamically linked; the tablet has no musl loader. Rebuild with +crt-static".into());
    }
    if t.userland == Userland::AlpineRoot && dynamic {
        let info = crate::elf_deps::elf_info(bytes)?;
        if !info.is_musl_dynamic() {
            return Err(format!("dynamically linked against {} (not musl); the Alpine root runs musl binaries", info.interp.unwrap_or_default()));
        }
    }
    Ok(())
}

// ───────────────────────── build plans: host or container ─────────────────────────

/// How to build a tablet app.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Engine {
    /// A host toolchain that has the target, else a container, else explain.
    Auto,
    /// A Rust toolchain on this machine (`pick_toolchain`).
    Host,
    /// podman (or docker) with the official `rust:alpine` image: the toolchain
    /// is the image's, so the host's Rust setup does not matter. The setup
    /// that worked for Fairphone musl builds on macOS.
    Container,
}

impl Engine {
    pub fn parse(s: &str) -> Option<Engine> {
        match s.trim() {
            "auto" => Some(Engine::Auto),
            "host" => Some(Engine::Host),
            "container" | "podman" | "docker" => Some(Engine::Container),
            _ => None,
        }
    }
}

/// The multi-arch official image. musl-native, rustup inside.
pub const CONTAINER_IMAGE: &str = "docker.io/library/rust:1-alpine";
/// A named volume for the crates.io cache, so rebuilds do not re-download.
pub const CONTAINER_CACHE: &str = "gitmind-tablet-cargo";

/// podman if present (rootless: output files stay yours), else docker.
pub fn container_runtime(path_env: &str) -> Option<&'static str> {
    ["podman", "docker"].into_iter().find(|r| crate::nixtoolchain::on_path(r, path_env))
}

/// The Cargo.lock a build of `dir` will touch: the nearest ancestor with one,
/// inside the repo. The shim patch rewrites it, so callers save and restore
/// it around the build (the operator's checkout must not change).
pub fn workspace_lock(dir: &std::path::Path, repo_root: &std::path::Path) -> Option<std::path::PathBuf> {
    dir.ancestors().take_while(|d| d.starts_with(repo_root)).map(|d| d.join("Cargo.lock")).find(|p| p.is_file())
}

/// Alpine packages the container installs for the native libraries eframe
/// apps pull in (found by compiling them against the shim): OpenSSL
/// (bo-suite), ALSA (voiceterm, aperture), Opus (studio), D-Bus (voiceterm),
/// libudev (gamepads), whisper.cpp's C++ build (aperture). `-static` where
/// Alpine splits static archives out. Package availability UNVERIFIED on a
/// real run: a missing one fails `apk add` visibly.
pub const CONTAINER_NATIVE_PKGS: &str = "musl-dev pkgconf linux-headers openssl-dev openssl-libs-static \
    alsa-lib-dev opus-dev dbus-dev eudev-dev cmake g++ clang-dev";

/// Everything needed to run one build and find its output.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildPlan {
    pub argv: Vec<String>,
    pub env: Vec<(String, String)>,
    pub cwd: std::path::PathBuf,
    /// Where the binary lands (absolute).
    pub output: std::path::PathBuf,
    /// One line for the log: which engine, which toolchain or image.
    pub note: String,
    /// Save before, restore after (the shim patch rewrites it).
    pub lockfile: Option<std::path::PathBuf>,
}

/// A container build of `app` for `t`, mounting `repo_root` (path
/// dependencies such as tomescope → shared/eink-kit must resolve inside).
///
/// * aarch64 targets: `--platform linux/arm64`, a NATIVE musl build inside
///   Alpine (fast on Apple Silicon; qemu-emulated on x86, which needs
///   qemu-user-static / binfmt on Linux).
/// * armv7 (rM2): the host-arch image cross-compiles, adding the target and
///   linking with rust-lld (unverified, like the host rust-lld path).
///
/// `:Z` relabels the mount for SELinux (Fedora); a separate target dir keeps
/// container output from mixing with host builds.
pub fn container_plan(runtime: &str, repo_root: &std::path::Path, app: &TabletApp, t: &BuildTarget) -> Result<BuildPlan, String> {
    if t.triple.contains("-gnu") {
        return Err("container builds are musl (Alpine); the Blight route needs a glibc build on the host".into());
    }
    if !t.static_crt && !t.triple.starts_with("aarch64") {
        return Err("the Alpine-root route builds natively in the arm64 container; armv7 (rM1/rM2) is not wired yet".into());
    }
    let rel = app.dir.strip_prefix(repo_root).map_err(|_| format!("{} is not inside {}", app.dir.display(), repo_root.display()))?;
    let rel = rel.to_string_lossy().to_string();
    let workdir = if rel.is_empty() { "/src".to_string() } else { format!("/src/{rel}") };
    let target_dir = format!("{workdir}/target/tablet-container");
    let mut cargo = vec!["cargo".to_string(), "build".into(), "--release".into(), "--bin".into(), app.bin.clone()];
    if app.via_shim {
        cargo.push("--config".into());
        cargo.push(format!("'{}'", shim_config_arg(&format!("/src/{SHIM_REL}"))));
    }
    if app.no_default {
        cargo.push("--no-default-features".into());
    }
    if !app.features.is_empty() {
        cargo.push("--features".into());
        cargo.push(app.features.join(","));
    }
    let native = t.triple.starts_with("aarch64");
    let mut script = if app.via_shim {
        // eframe apps bring native libraries; musl-dev alone is enough for eink-kit apps
        format!("apk add --no-cache {} >/dev/null && export EINK_SHIM=1 && ", CONTAINER_NATIVE_PKGS.split_whitespace().collect::<Vec<_>>().join(" "))
    } else {
        String::from("apk add --no-cache musl-dev >/dev/null && ")
    };
    let output_rel;
    if native {
        // Static for the tablet's own system; dynamic for the Alpine root
        // (links the same Alpine libraries the root has installed).
        let crt = if t.static_crt { "+crt-static" } else { "-crt-static" };
        // A static build names its target explicitly. Without `--target`, RUSTFLAGS reach the build scripts and
        // proc-macros too, and a musl host cannot build a proc-macro with +crt-static ("cannot produce proc-macro
        // for `bytemuck_derive` ... does not support these crate types"). With it, cargo keeps host artifacts out
        // of the flags, and the target is the container's own triple, so nothing is cross-compiled.
        if t.static_crt {
            cargo.push("--target".into());
            cargo.push(t.triple.into());
            output_rel = format!("{rel}/target/tablet-container/{}/release/{}", t.triple, app.bin);
        } else {
            output_rel = format!("{rel}/target/tablet-container/release/{}", app.bin);
        }
        script += &format!("RUSTFLAGS='-C target-feature={crt}' {}", cargo.join(" "));
    } else {
        cargo.push("--target".into());
        cargo.push(t.triple.into());
        script += &format!(
            "rustup target add {tr} >/dev/null && {lv}=rust-lld RUSTFLAGS='-C target-feature=+crt-static -C link-self-contained=yes' {c}",
            tr = t.triple, lv = crate::buildflags::linker_var(t.triple), c = cargo.join(" "));
        output_rel = format!("{rel}/target/tablet-container/{}/release/{}", t.triple, app.bin);
    }
    let mut argv: Vec<String> = vec![runtime.into(), "run".into(), "--rm".into()];
    if native {
        argv.extend(["--platform".into(), "linux/arm64".into()]);
    }
    argv.extend([
        "-v".into(), format!("{}:/src:Z", repo_root.display()),
        "-v".into(), format!("{CONTAINER_CACHE}:/usr/local/cargo/registry"),
        "-w".into(), workdir,
        "-e".into(), format!("CARGO_TARGET_DIR={target_dir}"),
        t.container_image.clone().unwrap_or_else(|| CONTAINER_IMAGE.into()), "sh".into(), "-c".into(), script,
    ]);
    Ok(BuildPlan {
        argv,
        env: vec![],
        cwd: repo_root.to_path_buf(),
        output: repo_root.join(output_rel.trim_start_matches('/')),
        note: format!("{runtime} with {}{}{}", t.container_image.as_deref().unwrap_or(CONTAINER_IMAGE), if native { " (linux/arm64, native musl)" } else { " (cross, rust-lld)" },
            if app.via_shim { ", eframe via eink-kit's shim" } else { "" }),
        lockfile: workspace_lock(&app.dir, repo_root),
    })
}

/// A host-toolchain build (what `tablet_build_cmd` + `use_toolchain` do).
pub fn host_plan(app: &TabletApp, t: &BuildTarget, tc: &Toolchain, native: bool, inherited_rustflags: Option<&str>, repo_root: &std::path::Path) -> BuildPlan {
    let (mut argv, mut env) = tablet_build_cmd(app, t, inherited_rustflags, native);
    use_toolchain(&mut argv, &mut env, tc);
    if app.via_shim {
        argv.push("--config".into());
        argv.push(shim_config_arg(&repo_root.join(SHIM_REL).to_string_lossy()));
        env.push(("EINK_SHIM".into(), "1".into()));
    }
    let rel = tablet_build_output(&app.bin, t, native);
    let output = app.dir.ancestors().map(|d| d.join(&rel)).find(|p| p.exists()).unwrap_or_else(|| app.dir.join(&rel));
    BuildPlan { argv, env, cwd: app.dir.clone(), output, lockfile: workspace_lock(&app.dir, repo_root),
        note: format!("host toolchain {} ({}){}", tc.cargo.display(), tc.origin, if app.via_shim { ", eframe via eink-kit's shim" } else { "" }) }
}

/// Run `f` with the plan's Cargo.lock saved beforehand and restored after,
/// whatever `f` does (the shim patch rewrites the lock; the operator's
/// checkout must come out exactly as it went in).
pub fn with_lock_preserved<T>(plan: &BuildPlan, f: impl FnOnce() -> T) -> T {
    let saved = plan.lockfile.as_ref().and_then(|p| std::fs::read(p).ok().map(|b| (p.clone(), b)));
    let out = f();
    if let Some((p, bytes)) = saved {
        if std::fs::read(&p).ok().as_deref() != Some(bytes.as_slice()) {
            let _ = std::fs::write(&p, bytes);
        }
    }
    out
}

/// Choose and build the plan for `engine`. Errors explain every engine
/// tried, so "installed but not found" and "no podman" both read plainly.
/// The app as it should be built for a device with `caps`: its default
/// features minus those the hardware cannot use (see
/// `capabilities::select_features`). An app already built with an explicit
/// feature set (`no_default`, e.g. tomescope's `eink` shell) is left alone.
/// Returns the adjusted app and the (feature, reason) pairs dropped.
pub fn app_for_device(app: &TabletApp, caps: Option<&crate::capabilities::Capabilities>) -> (TabletApp, Vec<(String, String)>) {
    let (Some(caps), false) = (caps, app.no_default) else { return (app.clone(), vec![]) };
    let Ok(toml) = std::fs::read_to_string(app.dir.join("Cargo.toml")) else { return (app.clone(), vec![]) };
    let sel = crate::capabilities::select_features(&toml, caps, &app.features);
    let mut a = app.clone();
    if sel.no_default {
        a.features = sel.features;
        a.no_default = true;
    }
    (a, sel.dropped)
}

pub fn plan_build(engine: Engine, repo_root: &std::path::Path, app: &TabletApp, t: &BuildTarget, path_env: &str, home: Option<&str>, cargo_env: Option<&str>) -> Result<BuildPlan, String> {
    plan_build_for(engine, repo_root, app, t, path_env, home, cargo_env, None)
}

/// [`plan_build`] for a surveyed device: features follow its hardware.
#[allow(clippy::too_many_arguments)]
pub fn plan_build_for(engine: Engine, repo_root: &std::path::Path, app: &TabletApp, t: &BuildTarget, path_env: &str, home: Option<&str>, cargo_env: Option<&str>, caps: Option<&crate::capabilities::Capabilities>) -> Result<BuildPlan, String> {
    let (adjusted, dropped) = app_for_device(app, caps);
    let mut plan = plan_build_inner(engine, repo_root, &adjusted, t, path_env, home, cargo_env)?;
    if !dropped.is_empty() {
        let why: Vec<String> = dropped.iter().map(|(f, r)| format!("-{f} ({r})")).collect();
        plan.note = format!("{}; features for this device: {}", plan.note, why.join(", "));
    }
    Ok(plan)
}

fn plan_build_inner(engine: Engine, repo_root: &std::path::Path, app: &TabletApp, t: &BuildTarget, path_env: &str, home: Option<&str>, cargo_env: Option<&str>) -> Result<BuildPlan, String> {
    let native = native_for(t);
    let rustflags = std::env::var("RUSTFLAGS").ok();
    let host = || pick_toolchain(t.triple, native, path_env, home, cargo_env).map(|tc| host_plan(app, t, &tc, native, rustflags.as_deref(), repo_root));
    let container = || match container_runtime(path_env) {
        Some(r) => container_plan(r, repo_root, app, t),
        None => Err("no container runtime found (podman or docker)".into()),
    };
    match engine {
        Engine::Host => host(),
        Engine::Container => container(),
        Engine::Auto => host().or_else(|he| container().map_err(|ce| format!("{he}\nand the container fallback: {ce}"))),
    }
}

// ───────────────────────── AppLoad management ─────────────────────────

/// How an app appears in AppLoad's sidebar (manifest keys from rm-appload
/// `libraryexternals.cpp`; icon is `<dir>/icon.png`).
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AppLoadOptions {
    pub display_name: String,
    /// `disablesWindowedMode`: always fullscreen. Default on: in a window
    /// AppLoad scales the app's full-size framebuffer down, which softens
    /// e-ink text, and eink-kit renders at native panel resolution.
    pub fullscreen: bool,
    pub args: Vec<String>,
    /// Launch inside the Alpine root through alpine-run.
    pub userland: Userland,
}

impl AppLoadOptions {
    pub fn named(name: &str) -> AppLoadOptions {
        AppLoadOptions { display_name: name.to_string(), fullscreen: true, args: vec![], userland: Userland::Static }
    }
}

/// Apps gitmind has put in AppLoad: `<name>\t<display name>` per line, for
/// manifests whose `application` points into gitmind's bin dir. Other apps
/// in AppLoad (KOReader, ...) are not listed and never touched.
pub fn appload_list_cmd(appload_dir: &str) -> Result<String, String> {
    assert_persistent(appload_dir)?;
    Ok(format!(
        "for m in {d}/*/external.manifest.json; do [ -f \"$m\" ] || continue; \
           grep -q '\"{bin}/' \"$m\" || continue; \
           n=$(basename \"$(dirname \"$m\")\"); \
           t=$(sed -n 's/.*\"name\": *\"\\([^\"]*\\)\".*/\\1/p' \"$m\" | head -n1); \
           printf '%s\\t%s\\n' \"$n\" \"$t\"; done; true",
        d = shell_quote(appload_dir),
        bin = format!("{BASE}/bin"),
    ))
}

pub fn parse_appload_list(out: &str) -> Vec<(String, String)> {
    out.lines()
        .filter_map(|l| l.split_once('\t'))
        .filter(|(n, _)| valid_app_name(n))
        .map(|(n, t)| (n.to_string(), t.to_string()))
        .collect()
}

/// Remove an app from AppLoad and delete its binary. Refuses directories
/// whose manifest does not point into gitmind's bin dir, so it can never
/// remove an app it did not install.
pub fn appload_remove_cmd(app: &str, appload_dir: &str) -> Result<String, String> {
    check(app)?;
    assert_persistent(appload_dir)?;
    let dir = format!("{appload_dir}/{app}");
    Ok(format!(
        "m={d}/external.manifest.json; \
         if ! grep -q '\"{bin}/' \"$m\" 2>/dev/null; then echo 'not installed by gitmind; left alone' >&2; exit 1; fi; \
         {stop}; rm -rf {d} {b}; echo removed",
        d = shell_quote(&dir),
        bin = format!("{BASE}/bin"),
        stop = stop_cmd(app)?,
        b = shell_quote(&bin_path(app)),
    ))
}

// ───────────────────────── AppLoad route ─────────────────────────

/// AppLoad's `external.manifest.json` for an eink-kit app. Schema from
/// rm-appload `src/libraryexternals.cpp`: `name` and `application` are
/// required; `qtfb: true` makes AppLoad open a window and pass `QTFB_KEY`;
/// `environment` is passed through, which is how the model and profile reach
/// the app without device detection.
pub fn appload_manifest(opts: &AppLoadOptions, app: &str, model: Option<RmModel>, profile: RefreshProfile) -> String {
    let mut env = serde_json::Map::new();
    env.insert("EINK_BACKEND".into(), "qtfb".into());
    // No EINK_QTFB_MODES: on a Paper Pro, AppLoad answers a refresh-mode
    // request by closing the connection AND the window (every app vanished
    // ~2 s in when this was on). AppLoad's own mode it is.
    // AppLoad-launched apps have no terminal: their stderr goes to the same
    // log `status` tails, so a failure to start is readable from the phone.
    env.insert("EINK_LOG".into(), log_path(app).into());
    env.insert(PROFILE_ENV.into(), profile.as_env().into());
    if let Some(m) = model {
        env.insert("EINK_MODEL".into(), m.env_id().into());
    }
    let v = serde_json::json!({
        "name": opts.display_name,
        "application": match opts.userland {
            Userland::Static => bin_path(app),
            Userland::AlpineRoot => crate::alpine_root::RUNNER.to_string(),
        },
        "qtfb": true,
        "disablesWindowedMode": opts.fullscreen,
        // Inside the root: alpine-run <root> <binary> <args...>. /home is
        // bound into the root at the same path, so the binary is where it is.
        "args": match opts.userland {
            Userland::Static => opts.args.clone(),
            Userland::AlpineRoot => [crate::alpine_root::ROOT.to_string(), bin_path(app)].into_iter().chain(opts.args.iter().cloned()).collect::<Vec<_>>(),
        },
        "environment": env,
    });
    serde_json::to_string_pretty(&v).expect("json")
}

/// Write the manifest into `<appload_dir>/<app>/`. AppLoad lists it after it
/// next reloads its library.
pub fn appload_install_cmd(app: &str, appload_dir: &str, manifest: &str) -> Result<String, String> {
    check(app)?;
    assert_persistent(appload_dir)?;
    let dir = format!("{appload_dir}/{app}");
    // Compare before replacing: AppLoad caches a manifest's environment until
    // xochitl restarts (seen on a Paper Pro: an edited EINK_QTFB_MODES was
    // ignored across many launches until a restart), so the caller must know
    // whether the running AppLoad now disagrees with the file.
    Ok(format!(
        "mkdir -p {d} && f={d}/external.manifest.json && printf '%s\\n' {m} > \"$f.new\" && \
         if [ ! -f \"$f\" ]; then s=manifest-new; elif cmp -s \"$f\" \"$f.new\"; then s=manifest-unchanged; else s=manifest-updated; fi && \
         mv \"$f.new\" \"$f\" && echo $s",
        d = shell_quote(&dir),
        m = shell_quote(manifest),
    ))
}

/// What writing an AppLoad manifest changed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ManifestChange {
    New,
    Updated,
    Unchanged,
}

impl ManifestChange {
    pub fn from_output(out: &str) -> ManifestChange {
        if out.contains("manifest-updated") {
            ManifestChange::Updated
        } else if out.contains("manifest-unchanged") {
            ManifestChange::Unchanged
        } else {
            ManifestChange::New
        }
    }

    /// What the operator needs to know, if anything.
    pub fn note(&self) -> Option<&'static str> {
        match self {
            ManifestChange::Updated => Some("the app's settings changed, and AppLoad keeps the old ones until the tablet's UI restarts (tapping reload is not enough): restart it with `gitmind eink restart-ui <host>` (or Restart UI on the Tablets page)"),
            ManifestChange::New => Some("new in AppLoad: tap reload in AppLoad's sidebar to see it"),
            ManifestChange::Unchanged => None,
        }
    }
}

/// Restart the tablet's UI (xochitl) so AppLoad re-reads every manifest.
/// Closes whatever is open. xovi and AppLoad stay loaded: xovi's systemd
/// drop-in lives in a tmpfs until reboot, and a restart keeps it.
pub const RESTART_UI_CMD: &str = "systemctl restart xochitl && sleep 3 && systemctl is-active xochitl";

/// Push + promote, then either launch (Blight) or register with AppLoad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    Launched(Outcome),
    Registered { manifest_dir: String, change: ManifestChange },
    /// A tool (see [`TOOLS`]): installed, not registered or launched.
    Installed { path: String },
}

/// Tablet TOOLS: installed to gitmind's bin/, never registered with AppLoad
/// or launched (alpine-run is the runner for the Alpine root; launched from
/// AppLoad's sidebar it only printed its usage).
pub const TOOLS: &[&str] = &["alpine-run"];

pub fn is_tool(app: &str) -> bool {
    TOOLS.contains(&app)
}

pub fn deliver(
    remote: &mut dyn Remote,
    route: &Route,
    app: &str,
    local_bin: &str,
    env: &LaunchEnv,
    args: &[&str],
    appload: Option<(&AppLoadOptions, Option<&str>)>,
    on: &mut dyn FnMut(Event),
) -> Result<Delivered, String> {
    if is_tool(app) {
        check(app)?;
        on(Event::Preparing);
        remote.run(&prepare_cmd()).map_err(|e| format!("prepare: {e}"))?;
        on(Event::Pushing { bytes: std::fs::metadata(local_bin).ok().map(|m| m.len()) });
        remote.push(local_bin, &staging_path(app)).map_err(|e| format!("push: {e}"))?;
        on(Event::Installing);
        remote.run(&install_cmd(app)?).map_err(|e| format!("install: {e}"))?;
        return Ok(Delivered::Installed { path: bin_path(app) });
    }
    match route {
        Route::Blight => deploy_and_launch(remote, app, local_bin, env, args, 2, on).map(Delivered::Launched),
        Route::AppLoad { dir } => {
            check(app)?;
            on(Event::Preparing);
            remote.run(&prepare_cmd()).map_err(|e| format!("prepare: {e}"))?;
            on(Event::Pushing { bytes: std::fs::metadata(local_bin).ok().map(|m| m.len()) });
            remote.push(local_bin, &staging_path(app)).map_err(|e| format!("push: {e}"))?;
            on(Event::Installing);
            remote.run(&install_cmd(app)?).map_err(|e| format!("install: {e}"))?;
            let default_opts = AppLoadOptions { args: args.iter().map(|a| a.to_string()).collect(), ..AppLoadOptions::named(app) };
            let (opts, icon) = appload.unwrap_or((&default_opts, None));
            if opts.userland == Userland::AlpineRoot {
                // Its libraries into the root, by what the binary itself
                // names (DT_NEEDED -> so:<lib>, which apk resolves).
                let bytes = std::fs::read(local_bin).map_err(|e| format!("read {local_bin}: {e}"))?;
                let provides = crate::elf_deps::elf_info(&bytes)?.apk_provides();
                if !provides.is_empty() {
                    let names: Vec<&str> = provides.iter().map(String::as_str).collect();
                    remote.run(&crate::alpine_root::add_cmd(&names)?).map_err(|e| format!("installing its libraries into the Alpine root: {e}"))?;
                }
            }
            let manifest = appload_manifest(opts, app, env.model, env.profile);
            let written = remote.run(&appload_install_cmd(app, dir, &manifest)?).map_err(|e| format!("manifest: {e}"))?;
            let change = ManifestChange::from_output(&written);
            if let Some(icon) = icon {
                let target = format!("{dir}/{app}/icon.png");
                assert_persistent(&target)?;
                remote.push(icon, &target).map_err(|e| format!("icon: {e}"))?;
            }
            Ok(Delivered::Registered { manifest_dir: format!("{dir}/{app}"), change })
        }
    }
}

// ───────────────────────── explaining failures ─────────────────────────

/// Turn an error from this module, or an app's log tail, into findings.
///
/// The tablet stack produces a known, finite set of failures, and most of
/// the text that reports them is written in this repo (here and in
/// eink-kit), so they are recognised by their own wording. Each finding
/// quotes the line it came from as a note, so a misrecognition is visible.
/// Unrecognised text yields nothing; callers show it raw.
pub fn diagnose(text: &str) -> Vec<crate::finding::Finding> {
    use crate::finding::{Finding, Severity::*};
    let mut out: Vec<Finding> = Vec::new();
    let has = |needle: &str| text.contains(needle);
    let line_of = |needle: &str| text.lines().find(|l| l.contains(needle)).unwrap_or(needle).trim().to_string();
    let mut add = |f: Finding, needle: &str| {
        let evidence = line_of(needle);
        out.push(f.note(format!("from: {evidence}")));
    };

    // connection and trust
    if has("host key has changed") {
        add(Finding::new(Warning, "tablet-hostkey-changed", "the tablet's SSH host key changed since it was trusted")
            .note("expected after an OS update, which regenerates it; also what an interception looks like")
            .help("if the tablet was just updated and is on your cable or network: re-trust it (`gitmind eink retrust <host>`), then enrol again"),
            "host key has changed");
    }
    if has("only offers an ssh-rsa host key") || has("no matching host key type found") {
        add(Finding::new(Error, "tablet-legacy-hostkey", "the tablet only offers an ssh-rsa host key")
            .note("older reMarkable OS versions; OpenSSH 8.8+ refuses ssh-rsa by default")
            .help("add the lines below to ~/.ssh/config on this phone")
            .fix("Host 10.11.99.1\n  HostKeyAlgorithms +ssh-rsa\n  PubkeyAcceptedKeyTypes +ssh-rsa"),
            "ssh-rsa");
    }
    if has("first contact needs the tablet's SSH password") {
        add(Finding::new(Info, "tablet-needs-password", "first contact needs the tablet's SSH password")
            .help("on the tablet: Settings → General → Help → About → Copyrights and licenses, under General information (username and password)")
            .note("developer mode must be enabled first; enabling it factory-resets the tablet, and OS updates reset the password"),
            "first contact");
    }
    if has("key auth still fails") {
        add(Finding::new(Error, "tablet-key-rejected", "the key was installed but the tablet still refuses it")
            .help("check /home/root/.ssh/authorized_keys on the tablet contains this phone's key"),
            "key auth still fails");
    }
    // "refused" means the tablet ANSWERED: reachable, nothing listening on
    // 22. Lumping it with timeouts said "did not answer", which sent a real
    // debugging session (Paper Pro, developer mode on, Wi-Fi) the wrong way.
    if has("Connection refused") {
        add(Finding::new(Error, "tablet-ssh-not-listening", "the tablet answered but is not running SSH on that address")
            .note("the network path is fine; nothing is listening on port 22 there")
            .note("Paper Pro family over Wi-Fi: SSH over Wi-Fi is OFF by default, even with developer mode on")
            .help("connect over USB once (10.11.99.1) and run `rm-ssh-over-wlan on` (Enable SSH over Wi-Fi on the Tablets page)")
            .help("refused over USB too: developer mode is off (Settings → General → Software → Developer mode; it factory-resets the tablet)"),
            "Connection refused");
    }
    if has("Network unreachable") || has("Network is unreachable") {
        add(Finding::new(Error, "tablet-no-network-path", "this machine has no network path to that address")
            .note("no interface on this machine is on that address's network, so nothing was even sent")
            .help("for 10.11.99.1: the USB network link is not up. Check `ip -br link` for a new interface after plugging in, and `ip -br -4 addr` for a 10.11.99.x address on it")
            .help("on a postmarketOS phone the USB host stack may need the usbnet drivers loaded (and can crash enumerating the tablet: check dmesg for \"HC died\"); a laptop or Wi-Fi avoids it"),
            "unreachable");
    }
    for needle in ["timed out", "No route to host", "Could not resolve", "Name or service not known"] {
        if has(needle) {
            add(Finding::new(Error, "tablet-unreachable", "the tablet did not answer on SSH")
                .help("on a USB cable the tablet is 10.11.99.1; check the cable carries data and the tablet is awake and unlocked")
                .help("over Wi-Fi: check the address (Copyrights and licenses lists it under the password) and that both are on the same network"),
                needle);
            break;
        }
    }
    if has("Permission denied") && !has("key auth still fails") {
        add(Finding::new(Error, "tablet-auth", "the tablet refused the login")
            .help("enrol again: the password from Settings installs this phone's key"),
            "Permission denied");
    }

    // building
    if has("standard library for") && has("is not installed on this machine")
        || has("can't find crate for `core`") || has("can't find crate for `std`")
        || has("target may not be installed")
    {
        let triple = ["aarch64-unknown-linux-musl", "armv7-unknown-linux-musleabihf", "aarch64-unknown-linux-gnu", "armv7-unknown-linux-gnueabihf"]
            .into_iter().find(|t| text.contains(t)).unwrap_or("<target>");
        add(Finding::new(Error, "build-target-missing", format!("this machine has no Rust standard library for {triple}"))
            .note("the tablet is a different CPU; building for it needs that target's std")
            .help("install it (rustup), then build again; or build on an aarch64 postmarketOS phone, where the AppLoad route builds natively")
            .fix(format!("rustup target add {triple}")),
            if has("is not installed on this machine") { "is not installed" } else { "can't find crate" });
    }
    if has("no container runtime found") {
        add(Finding::new(Info, "build-no-container", "no podman or docker on this machine for a container build")
            .help("Fedora: `sudo dnf install podman qemu-user-static`; macOS: install podman, then `podman machine init && podman machine start`"),
            "no container runtime");
    }
    if has("exec format error") {
        add(Finding::new(Error, "build-container-no-emulation", "this machine cannot run the arm64 build container")
            .note("the Paper Pro build runs natively in a linux/arm64 image; on an x86 machine that needs qemu user emulation")
            .help("Fedora: `sudo dnf install qemu-user-static` (registers binfmt), then build again; on Apple Silicon no emulation is needed")
            .fix("sudo dnf install qemu-user-static"),
            "exec format error");
    }
    if has("Cannot connect to Podman") || has("podman machine") && has("not running") {
        add(Finding::new(Error, "build-podman-machine", "podman's VM is not running (macOS)")
            .fix("podman machine start"),
            if has("Cannot connect to Podman") { "Cannot connect to Podman" } else { "not running" });
    }
    if has("no Rust toolchain found") {
        add(Finding::new(Error, "build-no-toolchain", "gitmind found no Rust toolchain to build with")
            .note("a GUI started from the desktop launcher often lacks ~/.cargo/bin on PATH (rustup adds it from the shell profile); gitmind also looks there directly, and at $CARGO")
            .help("install Rust (rustup, or the distro's rust/cargo), or start gitmind with CARGO=/path/to/cargo"),
            "no Rust toolchain found");
    }
    if has("linker `") && has("` not found") {
        add(Finding::new(Error, "build-linker-missing", "the cross linker cargo was told to use is not installed")
            .help("tablet builds fall back to Rust's rust-lld when no musl cross linker is on PATH; an explicit CARGO_TARGET_*_LINKER in your environment overrides that")
            .help("alternatively `cargo install cargo-zigbuild` and build with zig as the linker"),
            "linker `");
    }
    if let Some(l) = text.lines().find(|l| l.trim_start().starts_with("error[E") || l.trim_start().starts_with("error: could not compile")) {
        let l = l.to_string();
        add(Finding::new(Error, "build-compile-error", "the app failed to compile")
            .help("the first error line is quoted below; the full cargo output is in the log above"),
            &l);
    }

    // the binary
    if has("built for x86_64") || (has("built for ") && has("the tablet needs")) {
        add(Finding::new(Error, "tablet-wrong-arch", "that binary is built for a different CPU than the tablet's")
            .help("build it for the tablet: `gitmind eink build <host> <app>` (or the Build button on the Tablets page)"),
            "built for");
    }
    if has("dynamically linked; the tablet has no musl loader") {
        add(Finding::new(Error, "tablet-dynamic-musl", "the binary is dynamically linked and the tablet has no musl loader")
            .note("gitmind's desktop build env makes egui apps dynamic on musl (for winit); tablet builds must not be")
            .help("rebuild through gitmind's tablet build, which forces +crt-static"),
            "dynamically linked");
    }
    if has("not an ELF executable") {
        add(Finding::new(Error, "tablet-not-elf", "that file is not a Linux executable").help("point deploy at the built binary, not a script or archive"), "not an ELF");
    }

    // route and per-boot state
    if has("no display route") {
        add(Finding::new(Error, "tablet-no-route", "nothing on the tablet can put an app on its screen")
            .help("install AppLoad (with xovi) or Oxide via Vellum; reManager can do it from a desktop"),
            "no display route");
    }
    if has("since its last boot") || has("its display server is not running") || has("display server not running") {
        add(Finding::new(Warning, "tablet-needs-activation", "the tablet needs its once-per-boot step")
            .note("xovi (AppLoad) and a Vellum launcher (Oxide) do not start by themselves after a reboot")
            .help("`gitmind eink activate <host>`, or Activate on the Tablets page"),
            if has("since its last boot") { "since its last boot" } else { "not running" });
    }

    // the app's own startup messages (eink-kit)
    if has("no QTFB_KEY") {
        add(Finding::new(Error, "app-not-from-appload", "the app was started outside AppLoad but asked for a qtfb window")
            .note("only AppLoad can open a qtfb window: it creates the key the app connects with")
            .help("open it from AppLoad in the tablet's sidebar"),
            "no QTFB_KEY");
    }
    if has("connect /tmp/qtfb.sock") {
        add(Finding::new(Error, "app-no-appload", "the app could not reach AppLoad's qtfb socket")
            .help("start xovi: `gitmind eink activate <host>`"),
            "qtfb.sock");
    }
    if has("does not support refresh-mode messages") || has("AppLoad dropped the qtfb connection after SET_REFRESH_MODE") {
        add(Finding::new(Error, "app-qtfb-modes-unsupported", "AppLoad closed the app's window when it asked for a faster refresh mode")
            .note("this AppLoad tears down the window along with the connection, so the app cannot recover in place")
            .help("remove EINK_QTFB_MODES from the app's manifest (redeploying with current gitmind does)")
            .fix("sed -i 's/\"EINK_QTFB_MODES\": \"deferred\"/\"EINK_QTFB_MODES\": \"off\"/' /home/root/xovi/exthome/appload/*/external.manifest.json"),
            if has("does not support refresh-mode messages") { "does not support refresh-mode messages" } else { "after SET_REFRESH_MODE" });
    }
    if has("AppLoad closed the qtfb connection") && !has("does not support refresh-mode messages") {
        add(Finding::new(Error, "app-qtfb-closed", "AppLoad closed the app's qtfb connection after it started")
            .note("seen on a Paper Pro when the app sent SET_REFRESH_MODE right after init; eink-kit no longer sends mode messages unless EINK_QTFB_MODES=1")
            .help("rebuild and redeploy with the current eink-kit; if it persists, run with EINK_TRACE=1 and send the app log plus `journalctl -n 100 | grep QTFB` from the tablet"),
            "AppLoad closed the qtfb connection");
    }
    if has("qtfb refused window key") {
        add(Finding::new(Error, "app-qtfb-refused", "AppLoad refused the app's window")
            .help("close the app in AppLoad and open it again from the sidebar"),
            "refused window key");
    }
    if has("qtfb shm is") {
        add(Finding::new(Error, "app-wrong-model", "the window size does not match the tablet model the app assumed")
            .help("`gitmind eink probe <host>` shows the model and how it was identified; redeploy so the manifest carries EINK_MODEL"),
            "qtfb shm is");
    }
    if has("libblight_protocol not found") {
        add(Finding::new(Error, "app-no-blight", "Oxide's display library is not installed where the app looked")
            .help("install Oxide via Vellum, or use the AppLoad route"),
            "libblight_protocol");
    }
    if has("GUESS from architecture") {
        add(Finding::new(Warning, "app-model-guessed", "the app guessed the tablet model from its CPU")
            .help("redeploy from gitmind so the manifest sets EINK_MODEL; if the probe cannot identify it either, report what /sys/devices/soc0/machine says"),
            "GUESS from architecture");
    }
    if text.lines().any(|l| l.contains("EINK_") && (l.contains("want ") || l.contains("unknown"))) {
        let l = text.lines().find(|l| l.contains("EINK_") && (l.contains("want ") || l.contains("unknown"))).unwrap_or("").to_string();
        add(Finding::new(Error, "app-bad-setting", "an EINK_ setting has a value the app does not accept")
            .help("fix or remove it in the manifest / launch environment"),
            &l);
    }
    if has("panicked at") {
        add(Finding::new(Error, "app-panic", "the app panicked")
            .help("the line above is where; send it with the log tail"),
            "panicked at");
    }
    if has("exiting: qtfb socket closed") {
        add(Finding::new(Info, "app-closed", "AppLoad closed the app's window (a normal exit)"), "qtfb socket closed");
    }

    out.sort_by_key(|f| match f.severity { Error => 0, Warning => 1, Info => 2, Ok => 3 });
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn probe(lines: &[&str]) -> Probe {
        parse_probe(&lines.join("\n"))
    }

    #[test]
    fn triple_comes_from_uname_even_without_a_model() {
        let p = probe(&["machine=", "arch=aarch64"]);
        assert_eq!(p.triple, Some("aarch64-unknown-linux-gnu"));
        assert_eq!(p.model, None);
        assert!(p.how.contains("roster"));
        assert_eq!(probe(&["arch=armv7l"]).triple, Some("armv7-unknown-linux-gnueabihf"));
    }

    #[test]
    fn oxides_machine_strings_identify_every_model() {
        for (m, want, arch) in [
            ("reMarkable 1.0", RmModel::Rm1, "armv7l"),
            ("reMarkable Prototype 1", RmModel::Rm1, "armv7l"),
            ("reMarkable 2.0", RmModel::Rm2, "armv7l"),
            ("reMarkable Ferrari", RmModel::PaperPro, "aarch64"),
            ("reMarkable Chiappa", RmModel::PaperProMove, "aarch64"),
            ("reMarkable Tatsu", RmModel::PaperPure, "aarch64"),
        ] {
            let p = probe(&[&format!("machine={m}"), &format!("arch={arch}")]);
            assert_eq!(p.model, Some(want), "{m}");
            assert!(p.how.starts_with(want.label()) && p.how.contains("machine"), "{}", p.how);
        }
    }

    #[test]
    fn the_motd_codename_is_used_when_the_machine_string_is_new() {
        let p = probe(&["machine=reMarkable Something New", "motd=Welcome to ferrari build 3.28", "arch=aarch64"]);
        assert_eq!(p.model, Some(RmModel::PaperPro));
        assert!(p.how.contains("motd"));
    }

    #[test]
    fn an_unknown_codename_is_reported_not_guessed() {
        let p = probe(&["machine=", "motd=  marshmallow  ", "arch=aarch64"]);
        assert_eq!(p.model, None);
        assert_eq!(p.unknown_codename.as_deref(), Some("marshmallow"));
    }

    #[test]
    fn the_stock_hostname_identifies_the_device() {
        let p = probe(&["machine=reMarkable Something New", "hostname=imx8mm-ferrari", "arch=aarch64"]);
        assert_eq!(p.model, Some(RmModel::PaperPro));
        assert!(p.how.contains("hostname"), "{}", p.how);
    }

    #[test]
    fn gitmind_never_unlocks_the_system() {
        // Nothing gitmind sends may remount / or touch the /etc overlay:
        // it has no reason to (it writes only under /home), and doing it
        // wrong loses SSH (the dropbear keys are a bind mount under /etc).
        let mut all = vec![probe_cmd().to_string(), displays_probe_cmd().to_string(), prepare_cmd(),
            activation_cmd(Activation::StartXovi), activation_cmd(Activation::StartLauncher)];
        for app in ["x"] {
            all.push(install_cmd(app).unwrap());
            all.push(stop_cmd(app).unwrap());
            all.push(status_cmd(app, 1).unwrap());
            all.push(launch_cmd(app, &LaunchEnv::new(RefreshProfile::FastMono), &[]).unwrap());
            all.push(appload_install_cmd(app, "/home/root/xovi/exthome/appload", "{}").unwrap());
        }
        for c in all {
            for bad in ["remount", "umount", "mount -t", "mount -o"] {
                assert!(!c.contains(bad), "{bad:?} in {c}");
            }
        }
    }

    #[test]
    fn soc_and_panel_size_identify_an_unlisted_device() {
        assert_eq!(probe(&["soc=i.MX8MM", "arch=aarch64"]).model, Some(RmModel::PaperPro));
        assert_eq!(probe(&["soc=i.MX7D", "arch=armv7l"]).model, Some(RmModel::Rm2));
        // i.MX93 is shared: the panel decides, in either orientation
        assert_eq!(probe(&["soc=i.MX93", "size=1696x954", "arch=aarch64"]).model, Some(RmModel::PaperProMove));
        assert_eq!(probe(&["soc=i.MX93", "size=1404x1872", "arch=aarch64"]).model, Some(RmModel::PaperPure));
        let unsure = probe(&["soc=i.MX93", "arch=aarch64"]);
        assert_eq!(unsure.model, None, "i.MX93 without a panel size is ambiguous");
    }

    #[test]
    fn a_model_contradicting_uname_is_dropped_not_trusted() {
        let p = probe(&["machine=reMarkable 2.0", "arch=aarch64"]);
        assert_eq!(p.model, None);
        assert!(p.how.contains("contradicts"), "{}", p.how);
        assert_eq!(p.triple, Some("aarch64-unknown-linux-gnu"));
    }

    #[test]
    fn the_probe_command_is_valid_sh_and_parses_this_host() {
        let o = std::process::Command::new("sh").arg("-c").arg(probe_cmd()).output().unwrap();
        assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
        let p = parse_probe(&String::from_utf8_lossy(&o.stdout));
        assert!(p.triple.is_some() || std::env::consts::ARCH == "x86_64");
    }

    #[test]
    fn fastest_means_colour_only_where_there_is_colour() {
        assert_eq!(RefreshProfile::fastest(RmModel::PaperPro.panel()), RefreshProfile::FastColor);
        assert_eq!(RefreshProfile::fastest(RmModel::PaperProMove.panel()), RefreshProfile::FastColor);
        assert_eq!(RefreshProfile::fastest(RmModel::PaperPure.panel()), RefreshProfile::FastMono);
        assert_eq!(RefreshProfile::fastest(RmModel::Rm2.panel()), RefreshProfile::FastMono);
        for p in [RefreshProfile::FastColor, RefreshProfile::FastMono, RefreshProfile::Quality] {
            assert_eq!(RefreshProfile::parse(p.as_env()), Some(p));
        }
    }

    #[test]
    fn hostile_app_names_never_reach_a_shell() {
        for bad in ["", "a;rm -rf /", "../x", "App", "a b", "$(x)"] {
            assert!(install_cmd(bad).is_err(), "{bad:?}");
            assert!(launch_cmd(bad, &LaunchEnv::new(RefreshProfile::FastMono), &[]).is_err(), "{bad:?}");
        }
        assert!(valid_app_name("tomescope"));
        assert!(valid_app_name("vidkit-player"));
    }

    #[test]
    fn staging_is_on_the_same_filesystem_as_the_binary() {
        let (s, b) = (staging_path("x"), bin_path("x"));
        assert_eq!(s.rsplit_once('/').unwrap().0, b.rsplit_once('/').unwrap().0);
        assert!(!s.starts_with("/tmp"));
    }

    #[test]
    fn launch_carries_the_profile_and_records_the_pid() {
        let c = launch_cmd("tomescope", &LaunchEnv { profile: RefreshProfile::FastColor, model: Some(RmModel::PaperPro), blight_lib: Some("/home/root/.vellum/lib/libblight_protocol.so.1".into()) }, &["/home/root/a b.epub"]).unwrap();
        assert!(c.contains("tomescope' '/home/root/a b.epub' </dev/null"));
        assert!(c.contains("EINK_PROFILE=fast-color"));
        assert!(c.contains("EINK_BACKEND=blight EINK_MODEL=rmpp EINK_BLIGHT_LIB='/home/root/.vellum/lib/libblight_protocol.so.1'"));
        assert!(c.contains("nohup env"));
        assert!(c.contains("</dev/null"));
        assert!(c.contains("echo $! > '/home/root/.local/share/gitmind/run/tomescope.pid'"));
        // stop runs before start
        assert!(c.find("rm -f").unwrap() < c.find("nohup").unwrap());
    }

    #[test]
    fn stop_only_kills_a_pid_that_is_still_our_binary() {
        let c = stop_cmd("tomescope").unwrap();
        assert!(c.contains("readlink /proc/$p/exe"));
        assert!(c.contains("'/home/root/.local/share/gitmind/bin/tomescope'*)"));
    }

    #[test]
    fn status_parses_running_and_exited() {
        let r = parse_status("running 4242\n---\nhello\n");
        assert_eq!(r.pid, Some(4242));
        assert_eq!(r.log_tail, "hello");
        let s = parse_status("stopped\n---\nblight: service not available\n");
        assert_eq!(s.pid, None);
        assert_eq!(s.log_tail, "blight: service not available");
    }

    /// Alive and not a zombie (an orphan may be reparented to a subreaper
    /// that reaps late, and a zombie still has a /proc entry).
    fn alive(pid: u32) -> bool {
        std::fs::read_to_string(format!("/proc/{pid}/stat"))
            .map(|s| s.rsplit_once(')').map(|(_, r)| !r.trim_start().starts_with('Z')).unwrap_or(false))
            .unwrap_or(false)
    }

    /// The generated shell is real POSIX sh: run it locally against a temp
    /// BASE-shaped tree with a stand-in "app" to check pid/stop/log wiring.
    #[test]
    fn generated_shell_actually_runs_under_sh() {
        let dir = std::env::temp_dir().join(format!("eink_fleet_{}", std::process::id()));
        let base = dir.to_string_lossy().to_string();
        let rebase = |s: String| s.replace(BASE, &base);
        let sh = |cmd: &str| {
            let o = std::process::Command::new("sh").arg("-c").arg(cmd).output().unwrap();
            String::from_utf8_lossy(&o.stdout).to_string()
        };
        sh(&rebase(prepare_cmd()));
        // Stand-in app: a copy of `sleep`, so /proc/<pid>/exe really is our
        // binary path (a #! script would report its interpreter and the stop
        // guard would, correctly, refuse to touch it).
        let sleep = ["/bin/sleep", "/usr/bin/sleep"].into_iter().find(|p| std::path::Path::new(p).exists()).unwrap();
        std::fs::copy(sleep, rebase(staging_path("demo"))).unwrap();
        sh(&rebase(install_cmd("demo").unwrap()));
        assert!(std::path::Path::new(&rebase(bin_path("demo"))).exists());

        let out = sh(&rebase(launch_cmd("demo", &LaunchEnv::new(RefreshProfile::FastColor), &["30"]).unwrap()));
        assert!(out.starts_with("started "), "{out}");
        let st = parse_status(&sh(&rebase(status_cmd("demo", 1).unwrap())));
        let pid = st.pid.expect("running");
        let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
        assert!(environ.split(|b| *b == 0).any(|kv| kv == b"EINK_PROFILE=fast-color"));

        // relaunch replaces the old instance rather than stacking a second one
        sh(&rebase(launch_cmd("demo", &LaunchEnv::new(RefreshProfile::FastMono), &["30"]).unwrap()));
        let st2 = parse_status(&sh(&rebase(status_cmd("demo", 1).unwrap())));
        assert!(st2.pid.is_some());
        assert_ne!(st2.pid, Some(pid));
        assert!(!alive(pid), "old instance survived");

        sh(&rebase(stop_cmd("demo").unwrap()));
        assert_eq!(parse_status(&sh(&rebase(status_cmd("demo", 0).unwrap()))).pid, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    /// Every generated command must EXIT 0 in its normal cases, because
    /// `remote_capture` turns a non-zero exit into an error. (`status` on a
    /// never-launched app used to fail: `tail` of a missing log exits 1.)
    #[test]
    fn commands_exit_zero_on_a_fresh_device() {
        let dir = std::env::temp_dir().join(format!("eink_fresh_{}", std::process::id()));
        let base = dir.to_string_lossy().to_string();
        for cmd in [prepare_cmd(), status_cmd("never", 0).unwrap(), stop_cmd("never").unwrap()] {
            let cmd = cmd.replace(BASE, &base);
            let o = std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
            assert!(o.status.success(), "exit {:?} for {cmd}", o.status.code());
        }
        let o = std::process::Command::new("sh").arg("-c").arg(status_cmd("never", 0).unwrap().replace(BASE, &base)).output().unwrap();
        assert_eq!(parse_status(&String::from_utf8_lossy(&o.stdout)).pid, None);
        let _ = std::fs::remove_dir_all(&dir);
    }

    struct Fake {
        log: Vec<String>,
        status: String,
    }
    impl Remote for Fake {
        fn run(&mut self, cmd: &str) -> Result<String, String> {
            self.log.push(cmd.to_string());
            Ok(if cmd.starts_with("sleep") { self.status.clone() } else { String::new() })
        }
        fn push(&mut self, _l: &str, remote: &str) -> Result<(), String> {
            self.log.push(format!("PUSH {remote}"));
            Ok(())
        }
    }

    #[test]
    fn flow_order_and_early_exit_reporting() {
        let mut f = Fake { log: vec![], status: "stopped\n---\npanic: no display\n".into() };
        let mut ev = vec![];
        let out = deploy_and_launch(&mut f, "tomescope", "/nonexistent", &LaunchEnv::new(RefreshProfile::FastColor), &[], 2, &mut |e| ev.push(e))
            .unwrap();
        assert_eq!(out, Outcome::ExitedEarly { log_tail: "panic: no display".into() });
        assert!(f.log[0].starts_with("mkdir -p"));
        assert_eq!(f.log[1], format!("PUSH {}", staging_path("tomescope")));
        assert!(f.log[2].contains("mv -f"));
        assert!(f.log[3].contains("nohup"));
        assert!(f.log[4].starts_with("sleep 2"));
        assert_eq!(ev.len(), 5);
    }

    struct FakeEnroller {
        log: Vec<String>,
        key_ok: bool,
        key_ok_after_install: bool,
        hostkey_changed: bool,
    }
    impl Enroller for FakeEnroller {
        fn trust_host(&mut self) -> Result<String, String> { self.log.push("trust".into()); Ok("trusted".into()) }
        fn key_check(&mut self) -> KeyCheck {
            self.log.push("key?".into());
            if self.hostkey_changed { KeyCheck::HostKeyChanged } else if self.key_ok { KeyCheck::Works } else { KeyCheck::Fails }
        }
        fn install_key(&mut self, pw: &str) -> Result<String, String> {
            self.log.push(format!("install:{pw}"));
            self.key_ok = self.key_ok_after_install;
            Ok("installed".into())
        }
        fn run(&mut self, cmd: &str) -> Result<String, String> {
            self.log.push("run".into());
            Ok(if cmd.contains("soc0/machine") { "machine=reMarkable Ferrari\narch=aarch64".into() }
               else if cmd.contains("qtfb.sock") { "qtfb=1\nappload_dir=/home/root/xovi/exthome/appload\nglibc=2.39\n".into() }
               else { String::new() })
        }
    }

    #[test]
    fn first_contact_trusts_the_host_before_sending_the_password() {
        let mut f = FakeEnroller { log: vec![], key_ok: false, key_ok_after_install: true, hostkey_changed: false };
        let r = enroll(&mut f, Some("hunter2"), &mut |_| {}).unwrap();
        let trust = f.log.iter().position(|l| l == "trust").unwrap();
        let install = f.log.iter().position(|l| l.starts_with("install")).unwrap();
        assert!(trust < install);
        assert!(r.used_password);
        assert_eq!(r.probe.model, Some(RmModel::PaperPro));
        assert_eq!(r.displays.glibc.as_deref(), Some("2.39"));
        assert_eq!(r.displays.route().unwrap(), Route::AppLoad { dir: "/home/root/xovi/exthome/appload".into() });
    }

    #[test]
    fn re_enrolling_never_asks_for_the_password_or_duplicates_the_key() {
        let mut f = FakeEnroller { log: vec![], key_ok: true, key_ok_after_install: true, hostkey_changed: false };
        let r = enroll(&mut f, None, &mut |_| {}).unwrap();
        assert!(!r.used_password);
        assert!(!f.log.iter().any(|l| l.starts_with("install")));
    }

    #[test]
    fn a_missing_password_says_where_to_find_it() {
        let mut f = FakeEnroller { log: vec![], key_ok: false, key_ok_after_install: true, hostkey_changed: false };
        let e = enroll(&mut f, None, &mut |_| {}).unwrap_err();
        assert!(e.contains("Settings"), "{e}");
    }

    #[test]
    fn a_key_that_still_does_not_work_is_an_error_not_a_silent_success() {
        let mut f = FakeEnroller { log: vec![], key_ok: false, key_ok_after_install: false, hostkey_changed: false };
        assert!(enroll(&mut f, Some("pw"), &mut |_| {}).is_err());
    }

    #[test]
    fn blight_is_preferred_and_missing_everything_is_explained() {
        let both = parse_displays("qtfb=1\nappload_dir=/x\nblight_lib=/opt/lib/libblight_protocol.so.1\nblight_running=1\n");
        assert_eq!(both.route().unwrap(), Route::Blight);
        let installed_not_running = parse_displays("blight_lib=/opt/lib/libblight_protocol.so\n");
        assert!(installed_not_running.route().unwrap_err().contains("not running"));
        assert!(parse_displays("").route().unwrap_err().contains("Vellum"));
    }

    #[test]
    fn appload_manifest_matches_the_schema_and_carries_the_env() {
        let m = appload_manifest(&AppLoadOptions::named("eink demo"), "eink-demo", Some(RmModel::PaperProMove), RefreshProfile::FastColor);
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["name"], "eink demo");
        assert_eq!(v["application"], "/home/root/.local/share/gitmind/bin/eink-demo");
        assert_eq!(v["qtfb"], true);
        assert_eq!(v["disablesWindowedMode"], true, "fullscreen by default");
        assert_eq!(v["environment"]["EINK_MODEL"], "rmppm");
        assert_eq!(v["environment"]["EINK_BACKEND"], "qtfb");
        assert_eq!(v["environment"]["EINK_PROFILE"], "fast-color");
        assert_eq!(v["environment"]["EINK_LOG"], "/home/root/.local/share/gitmind/log/eink-demo.log");
        assert!(v["environment"].get("EINK_QTFB_MODES").is_none(), "no refresh-mode requests: AppLoad closes the window in reply");
    }

    #[test]
    fn the_manifest_command_survives_the_shell_with_quotes_in_it() {
        let m = appload_manifest(&AppLoadOptions::named("it's \"quoted\""), "demo", None, RefreshProfile::Quality);
        let dir = std::env::temp_dir().join(format!("appload_{}", std::process::id()));
        let real = "/home/root/xovi/exthome/appload";
        let cmd = appload_install_cmd("demo", real, &m).unwrap().replace(real, &dir.to_string_lossy());
        let out = std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("manifest-new"), "{}", String::from_utf8_lossy(&out.stderr));
        let written = std::fs::read_to_string(dir.join("demo/external.manifest.json")).unwrap();
        let v: serde_json::Value = serde_json::from_str(&written).unwrap();
        assert_eq!(v["name"], "it's \"quoted\"");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_displays_probe_is_valid_sh_and_emits_only_known_keys() {
        // Whatever this host has installed, the probe must run and every
        // line must be a key the parser knows (no stray output to misread).
        let out = std::process::Command::new("sh").arg("-c").arg(displays_probe_cmd()).output().unwrap();
        assert!(out.status.success(), "{}", String::from_utf8_lossy(&out.stderr));
        for line in String::from_utf8_lossy(&out.stdout).lines() {
            let key = line.split_once('=').map(|(k, _)| k).unwrap_or(line);
            assert!(DISPLAY_KEYS.contains(&key), "probe emitted {line:?}, which parse_displays does not know");
        }
    }

    #[test]
    fn a_changed_host_key_stops_enrolment_before_any_password_is_sent() {
        let mut f = FakeEnroller { log: vec![], key_ok: false, key_ok_after_install: true, hostkey_changed: true };
        let e = enroll(&mut f, Some("pw"), &mut |_| {}).unwrap_err();
        assert!(e.contains("host key has changed") && e.contains("OS update"), "{e}");
        assert!(!f.log.iter().any(|l| l.starts_with("install")), "password must not be sent to a changed key");
    }

    #[test]
    fn every_write_target_is_on_the_persistent_tree() {
        for p in [BASE.to_string(), bin_path("x"), staging_path("x"), pid_path("x"), log_path("x")] {
            assert_persistent(&p).unwrap();
        }
        assert!(assert_persistent("/etc/foo").is_err());
        assert!(assert_persistent("/home/root/../../etc").is_err());
        assert!(assert_persistent("home/root/x").is_err());
        assert!(appload_install_cmd("demo", "/opt/appload", "{}").is_err(), "a probe result outside /home/root is refused");
    }

    #[test]
    fn readiness_after_a_reboot_offers_the_right_activation() {
        // AppLoad installed, xovi not started since boot → start xovi
        let d = parse_displays("appload_dir=/home/root/xovi/exthome/appload\nxovi_installed=1\n");
        assert_eq!(d.readiness().unwrap(), (Route::AppLoad { dir: "/home/root/xovi/exthome/appload".into() }, Some(Activation::StartXovi)));
        // xovi running → ready
        let d = parse_displays("appload_dir=/a\nxovi_installed=1\nqtfb=1\n");
        assert_eq!(d.readiness().unwrap().1, None);
        // Oxide configured but idle beats AppLoad: one step to the better route
        let d = parse_displays("appload_dir=/a\nqtfb=1\nblight_lib=/home/root/.vellum/lib/libblight_protocol.so\nlauncher_current=oxide\nlauncher_active=none\n");
        assert_eq!(d.readiness().unwrap(), (Route::Blight, Some(Activation::StartLauncher)));
        // Oxide running → ready, no step
        let d = parse_displays("blight_lib=/x\nblight_running=1\nlauncher_current=oxide\nlauncher_active=oxide\n");
        assert_eq!(d.readiness().unwrap(), (Route::Blight, None));
        // only Oxide installed, idle
        let d = parse_displays("blight_lib=/x\nlauncher_current=* oxide\n");
        assert_eq!(d.launcher_current.as_deref(), Some("oxide"));
        assert_eq!(d.readiness().unwrap(), (Route::Blight, Some(Activation::StartLauncher)));
    }

    #[test]
    fn activation_commands_are_valid_sh() {
        for a in [Activation::StartXovi, Activation::StartLauncher] {
            let o = std::process::Command::new("sh").arg("-n").arg("-c").arg(activation_cmd(a)).output().unwrap();
            assert!(o.status.success(), "{a:?}: {}", String::from_utf8_lossy(&o.stderr));
        }
    }

    #[test]
    fn the_extended_probe_parses_a_paper_pro_after_reboot() {
        let d = parse_displays("rootfs_ro=1\netc_overlay=1\nxovi_installed=1\nlauncher_current=none\nlauncher_active=\n");
        assert!(d.rootfs_ro && d.etc_overlay && d.xovi_installed);
        assert_eq!(d.launcher_current, None, "\"none\" is no launcher");
    }

    #[test]
    fn the_route_decides_the_build() {
        let appload = Route::AppLoad { dir: "/home/root/xovi/exthome/appload".into() };
        let t = build_target(&appload, Some(RmModel::PaperPro));
        assert_eq!((t.triple, t.static_crt), ("aarch64-unknown-linux-musl", true));
        assert_eq!(build_target(&appload, Some(RmModel::Rm2)).triple, "armv7-unknown-linux-musleabihf");
        let b = build_target(&Route::Blight, Some(RmModel::PaperPro));
        assert_eq!((b.triple, b.static_crt), ("aarch64-unknown-linux-gnu", false));
    }

    #[test]
    fn tablet_builds_force_static_even_for_egui_apps_and_keep_existing_rustflags() {
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let demo = TabletApp { dir: ".".into(), bin: "eink-demo".into(), features: vec![], no_default: false, via_shim: false };
        // a PATH with a musl cross gcc, so the rust-lld fallback stays out of it
        let dir = std::env::temp_dir().join(format!("xgcc_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("aarch64-linux-musl-gcc"), "").unwrap();
        let (argv, env) = tablet_build_cmd_on_path(&demo, &t, Some("-C opt-level=s"), false, &dir.to_string_lossy());
        assert!(argv.windows(2).any(|w| w == ["--bin", "eink-demo"]), "{argv:?}");
        assert!(argv.windows(2).any(|w| w == ["--target", "aarch64-unknown-linux-musl"]), "{argv:?}");
        let rf = env.iter().find(|(k, _)| k == "RUSTFLAGS").map(|(_, v)| v.as_str()).unwrap();
        assert_eq!(rf, "-C opt-level=s -C target-feature=+crt-static");
        assert!(!rf.contains("-crt-static ") && !rf.starts_with("-C target-feature=-crt-static"));
        assert_eq!(tablet_build_output("eink-demo", &t, false), "target/aarch64-unknown-linux-musl/release/eink-demo");
        // on the phone: native, no --target, still static
        let (argv, env) = tablet_build_cmd(&demo, &t, None, true);
        assert!(!argv.iter().any(|a| a == "--target"), "{argv:?}");
        assert_eq!(env, vec![("RUSTFLAGS".to_string(), "-C target-feature=+crt-static".to_string())]);
        assert_eq!(tablet_build_output("eink-demo", &t, true), "target/release/eink-demo");
    }

    /// Minimal ELF images: header + one program header of the given type.
    fn elf(is64: bool, machine: u16, ptype: u32) -> Vec<u8> {
        let mut b = vec![0u8; 256];
        b[..4].copy_from_slice(b"\x7fELF");
        b[4] = if is64 { 2 } else { 1 };
        b[5] = 1;
        b[18..20].copy_from_slice(&machine.to_le_bytes());
        if is64 {
            b[0x20..0x28].copy_from_slice(&64u64.to_le_bytes());
            b[0x36..0x38].copy_from_slice(&56u16.to_le_bytes());
            b[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());
            b[64..68].copy_from_slice(&ptype.to_le_bytes());
        } else {
            b[0x1c..0x20].copy_from_slice(&52u32.to_le_bytes());
            b[0x2a..0x2c].copy_from_slice(&32u16.to_le_bytes());
            b[0x2c..0x2e].copy_from_slice(&1u16.to_le_bytes());
            b[52..56].copy_from_slice(&ptype.to_le_bytes());
        }
        b
    }

    #[test]
    fn binary_check_catches_wrong_arch_and_dynamic_musl() {
        let pp = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        assert!(check_tablet_binary(&elf(true, 183, 1), &pp).is_ok(), "static aarch64");
        let e = check_tablet_binary(&elf(true, 62, 1), &pp).unwrap_err();
        assert!(e.contains("x86_64"), "{e}");
        let e = check_tablet_binary(&elf(true, 183, 3), &pp).unwrap_err();
        assert!(e.contains("musl loader"), "{e}");
        let rm2 = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::Rm2));
        assert!(check_tablet_binary(&elf(false, 40, 1), &rm2).is_ok(), "static 32-bit ARM");
        assert!(check_tablet_binary(&elf(false, 40, 3), &rm2).is_err());
        let blight = build_target(&Route::Blight, Some(RmModel::PaperPro));
        assert!(check_tablet_binary(&elf(true, 183, 3), &blight).is_ok(), "dynamic gnu is right for Blight");
        assert!(check_tablet_binary(b"#!/bin/sh\necho", &pp).is_err());
    }

    #[test]
    fn preflight_refuses_this_hosts_binary_for_a_tablet() {
        let me = std::env::current_exe().unwrap();
        let r = preflight_binary(&me.to_string_lossy(), &Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        if std::env::consts::ARCH != "aarch64" {
            assert!(r.is_err());
        }
        assert!(preflight_binary("/nonexistent/bin", &Route::Blight, None).is_err());
    }

    #[test]
    fn binary_check_on_a_real_host_binary() {
        // this test binary is an x86_64/aarch64 host ELF: must be refused
        // for the tablet unless the host happens to be aarch64
        let me = std::fs::read(std::env::current_exe().unwrap()).unwrap();
        let pp = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        if std::env::consts::ARCH != "aarch64" {
            assert!(check_tablet_binary(&me, &pp).unwrap_err().contains("built for"));
        }
    }

    #[test]
    fn eink_apps_are_found_by_their_dependency() {
        let kit = "[package]\nname = \"eink-kit\"\n[dependencies]\negui = \"0.34\"\n[[bin]]\nname = \"eink-demo\"\npath = \"x\"\n";
        assert_eq!(eink_app_bins(kit), vec![("eink-demo".to_string(), vec![], false)]);
        let app = "[package]\nname = \"reader-eink\"\n[dependencies]\neink-kit = { path = \"../eink-kit\" }\n";
        assert_eq!(eink_app_bins(app), vec![("reader-eink".to_string(), vec![], false)]);
        let other = "[package]\nname = \"hatch\"\n[dependencies]\negui = \"0.34\"\n";
        assert!(eink_app_bins(other).is_empty());
    }

    #[test]
    fn a_feature_gated_port_offers_only_its_tablet_binary() {
        // the tomescope shape
        let t = r#"[package]
name = "tomescope"
[[bin]]
name = "tomescope"
path = "src/main.rs"
required-features = ["desktop"]
[[bin]]
name = "tomescope-eink"
path = "src/bin/tomescope_eink.rs"
required-features = ["eink"]
[[bin]]
name = "shelfctl"
required-features = ["shelfctl"]
[features]
default = ["desktop", "shelfctl"]
desktop = ["dep:eframe"]
eink = ["dep:eink-kit"]
[dependencies]
eframe = { version = "0.34", optional = true }
eink-kit = { path = "../../shared/eink-kit", optional = true }
"#;
        assert_eq!(eink_app_bins(t), vec![("tomescope-eink".to_string(), vec!["eink".to_string()], true)]);
        let app = TabletApp { dir: ".".into(), bin: "tomescope-eink".into(), features: vec!["eink".into()], no_default: true, via_shim: false };
        let target = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let (argv, _) = tablet_build_cmd(&app, &target, None, false);
        for want in ["--no-default-features", "--features", "eink", "--bin", "tomescope-eink"] {
            assert!(argv.iter().any(|a| a == want), "{want} missing from {argv:?}");
        }
    }

    #[test]
    fn discovery_finds_eink_demo_in_this_checkout() {
        // cockpit's crate dir is <root>/apps/gitmind/crates/cockpit
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let apps = discover_tablet_apps(&root);
        assert!(apps.iter().any(|a| a.bin == "eink-demo" && a.dir.ends_with("shared/eink-kit")), "{apps:?}");
        let ts = apps.iter().find(|a| a.bin == "tomescope-eink").expect("tomescope port discovered");
        assert!(ts.no_default && ts.features == ["eink"]);
        assert!(!apps.iter().any(|a| a.bin == "tomescope" || a.bin == "shelfctl"), "desktop bins offered: {apps:?}");
        assert!(apps.iter().any(|a| a.bin == "unendlich-eink" && a.dir.ends_with("games/unendlich/crates/unendlich-eink")), "{apps:?}");
    }

    #[test]
    fn appload_list_and_remove_only_touch_gitmind_apps() {
        let root = std::env::temp_dir().join(format!("apl_{}", std::process::id()));
        let real = "/home/root/xovi/exthome/appload";
        let rebase = |c: String| c.replace(real, &root.to_string_lossy());
        let _ = std::fs::remove_dir_all(&root);
        std::fs::create_dir_all(root.join("eink-demo")).unwrap();
        std::fs::create_dir_all(root.join("koreader")).unwrap();
        let base = root.join("base").to_string_lossy().to_string();
        std::fs::write(root.join("eink-demo/external.manifest.json"),
            appload_manifest(&AppLoadOptions::named("Eink Demo"), "eink-demo", None, RefreshProfile::FastColor).replace(BASE, &base)).unwrap();
        std::fs::write(root.join("koreader/external.manifest.json"),
            "{\"name\": \"KOReader\", \"application\": \"/home/root/koreader/koreader.sh\"}").unwrap();
        let sh = |c: &str| std::process::Command::new("sh").arg("-c").arg(c).output().unwrap();
        let o = sh(&rebase(appload_list_cmd(real).unwrap()).replace(BASE, &base));
        assert!(o.status.success());
        assert_eq!(parse_appload_list(&String::from_utf8_lossy(&o.stdout)), vec![("eink-demo".to_string(), "Eink Demo".to_string())]);
        let o = sh(&rebase(appload_remove_cmd("koreader", real).unwrap()));
        assert!(!o.status.success() && root.join("koreader").exists(), "someone else's app must be left alone");
        let o = sh(&rebase(appload_remove_cmd("eink-demo", real).unwrap()).replace(BASE, &base));
        assert!(String::from_utf8_lossy(&o.stdout).contains("removed"), "{}", String::from_utf8_lossy(&o.stderr));
        assert!(!root.join("eink-demo").exists());
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn diagnose_recognises_this_modules_own_errors() {
        let hk = diagnose(HOSTKEY_CHANGED);
        assert_eq!(hk[0].code, "tablet-hostkey-changed");
        assert!(diagnose(LEGACY_HOSTKEY)[0].fix.as_deref().unwrap().contains("+ssh-rsa"));
        let pp = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let fake_x86 = { let mut b = vec![0u8; 256]; b[..4].copy_from_slice(b"\x7fELF"); b[4] = 2; b[5] = 1; b[18] = 62; b };
        let e = check_tablet_binary(&fake_x86, &pp).unwrap_err();
        assert_eq!(diagnose(&e)[0].code, "tablet-wrong-arch", "{e}");
        let route = parse_displays("").route().unwrap_err();
        assert_eq!(diagnose(&route)[0].code, "tablet-no-route");
        assert!(diagnose("ssh failed: ssh: connect to host 10.11.99.1 port 22: No route to host")[0].code == "tablet-unreachable");
        // the real session: Wi-Fi refused (reachable, SSH-over-Wi-Fi off), USB unreachable (no link)
        let wifi = diagnose("ssh: connect to host 192.168.2.185 port 22: Connection refused");
        assert_eq!(wifi[0].code, "tablet-ssh-not-listening");
        assert!(wifi[0].help[0].contains("rm-ssh-over-wlan on"));
        assert!(!wifi.iter().any(|f| f.code == "tablet-unreachable"), "refused is not 'did not answer'");
        let usb = diagnose("ssh: connect to host 10.11.99.1 port 22: Network unreachable");
        assert_eq!(usb[0].code, "tablet-no-network-path");
    }

    #[test]
    fn the_refresh_mode_window_loss_is_explained_with_the_fix() {
        // the real log line from ambient-learner-egui on the Paper Pro
        let f = diagnose("eink: AppLoad dropped the qtfb connection after SET_REFRESH_MODE; reconnecting without mode messages");
        assert_eq!(f[0].code, "app-qtfb-modes-unsupported");
        assert!(f[0].fix.as_deref().unwrap().contains("\"off\""));
    }

    #[test]
    fn manifest_writes_say_whether_the_running_appload_is_now_stale() {
        let dir = std::env::temp_dir().join(format!("mfst_{}", std::process::id()));
        let base = format!("/home/root/tmp-mfst-{}", std::process::id());
        let run = |m: &str| {
            let cmd = appload_install_cmd("demo", &base, m).unwrap().replace(&base, &dir.to_string_lossy());
            let o = std::process::Command::new("sh").arg("-c").arg(cmd).output().unwrap();
            assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
            ManifestChange::from_output(&String::from_utf8_lossy(&o.stdout))
        };
        assert_eq!(run("{\"a\":1}"), ManifestChange::New);
        assert_eq!(run("{\"a\":1}"), ManifestChange::Unchanged);
        assert_eq!(run("{\"a\":2}"), ManifestChange::Updated);
        assert!(ManifestChange::Updated.note().unwrap().contains("restart"));
        assert!(!dir.join("demo/external.manifest.json.new").exists(), "no temp file left behind");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn tools_are_installed_not_registered() {
        assert!(is_tool("alpine-run") && !is_tool("eink-demo"));
    }

    #[test]
    fn the_paper_pro_qtfb_disconnect_is_explained() {
        let f = diagnose("eink-demo: AppLoad closed the qtfb connection (after 1 message(s) since init, last SET_REFRESH_MODE; this one UPDATE)");
        assert_eq!(f[0].code, "app-qtfb-closed");
    }

    #[test]
    fn diagnose_reads_an_app_log_tail_worst_first_with_evidence() {
        // eink-kit's own wording (engine.rs / device.rs / present/*.rs)
        let tail = "eink: model PaperPro (GUESS from architecture (device tree said \"\"); set EINK_MODEL)\n\
                    eink: EINK_BACKEND=qtfb but no QTFB_KEY: launch from AppLoad\n";
        let f = diagnose(tail);
        let codes: Vec<&str> = f.iter().map(|f| f.code.as_str()).collect();
        assert_eq!(codes, vec!["app-not-from-appload", "app-model-guessed"], "errors first");
        assert!(f[0].notes.last().unwrap().contains("no QTFB_KEY"), "evidence quoted: {:?}", f[0].notes);
        assert!(diagnose("blah\nthread 'main' panicked at src/app.rs:10:5:\nboom").iter().any(|f| f.code == "app-panic"));
        assert!(diagnose("EINK_PROFILE=\"turbo\": want fast-color|fast-mono|quality").iter().any(|f| f.code == "app-bad-setting"));
        assert!(diagnose("completely unrelated text").is_empty(), "unrecognised text yields nothing");
    }

    #[test]
    fn tablet_keys_include_an_ecdsa_key_for_the_rm2() {
        let home = std::env::temp_dir().join(format!("tkeys_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&home);
        std::fs::create_dir_all(&home).unwrap();
        let old = std::env::var("HOME").ok();
        std::env::set_var("HOME", &home);
        let keys = tablet_pubkeys();
        if let Some(o) = old { std::env::set_var("HOME", o); }
        let keys = keys.expect("ssh-keygen available");
        let kinds: Vec<&str> = keys.lines().filter_map(|l| l.split_whitespace().next()).collect();
        assert_eq!(kinds, vec!["ssh-ed25519", "ecdsa-sha2-nistp256"], "{keys}");
        let _ = std::fs::remove_dir_all(&home);
    }

    #[test]
    fn the_user_is_a_default_not_a_constant() {
        assert_eq!(DEFAULT_USER, "root");
        let e = SshEnroller { user: "someone".into(), host: "h".into() };
        assert_eq!(e.user, "someone");
        assert!(parse_displays("wlan_ssh_tool=1\n").wlan_ssh_tool);
    }

    #[test]
    fn a_laptop_without_a_musl_cross_linker_links_with_rust_lld() {
        let app = TabletApp { dir: ".".into(), bin: "eink-demo".into(), features: vec![], no_default: false, via_shim: false };
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let (_, env) = tablet_build_cmd_on_path(&app, &t, None, false, "/nonexistent");
        let get = |k: &str| env.iter().find(|(kk, _)| kk == k).map(|(_, v)| v.clone());
        assert_eq!(get("CARGO_TARGET_AARCH64_UNKNOWN_LINUX_MUSL_LINKER").as_deref(), Some("rust-lld"));
        assert_eq!(get("RUSTFLAGS").as_deref(), Some("-C target-feature=+crt-static -C link-self-contained=yes"));
        // with a real musl cross gcc on PATH, it is used and lld is not forced
        let dir = std::env::temp_dir().join(format!("xlink_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(dir.join("aarch64-linux-musl-gcc"), "").unwrap();
        let (_, env) = tablet_build_cmd_on_path(&app, &t, None, false, &dir.to_string_lossy());
        assert!(env.iter().any(|(k, v)| k.ends_with("_LINKER") && v == "aarch64-linux-musl-gcc"), "{env:?}");
        assert!(!env.iter().any(|(_, v)| v.contains("link-self-contained")));
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn build_failures_explain_themselves() {
        // what the laptop build printed
        let f = diagnose("build failed (exit Some(101)).\nerror[E0463]: can't find crate for `core`\n  = note: the `aarch64-unknown-linux-musl` target may not be installed");
        assert_eq!(f[0].code, "build-target-missing");
        assert_eq!(f[0].fix.as_deref(), Some("rustup target add aarch64-unknown-linux-musl"));
        assert!(diagnose(&missing_target_message("aarch64-unknown-linux-musl"))[0].fix.as_deref().unwrap().contains("rustup target add"));
        assert!(diagnose("error: linker `aarch64-linux-musl-gcc` not found").iter().any(|f| f.code == "build-linker-missing"));
        assert!(diagnose("error[E0308]: mismatched types\n --> src/app.rs:3:5").iter().any(|f| f.code == "build-compile-error"));
    }

    /// A fake toolchain: `rustc --print target-libdir --target T` prints a
    /// dir that exists only for the targets given.
    fn fake_toolchain(root: &std::path::Path, name: &str, targets: &[&str]) -> std::path::PathBuf {
        use std::os::unix::fs::PermissionsExt;
        let bin = root.join(name).join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let lib = root.join(name).join("lib");
        for t in targets { std::fs::create_dir_all(lib.join(t)).unwrap(); }
        let rustc = format!("#!/bin/sh\n[ \"$1 $2 $3\" = \"--print target-libdir --target\" ] && echo {}/$4\n", lib.display());
        std::fs::write(bin.join("rustc"), rustc).unwrap();
        std::fs::write(bin.join("cargo"), "#!/bin/sh\n").unwrap();
        for f in ["rustc", "cargo"] { std::fs::set_permissions(bin.join(f), std::fs::Permissions::from_mode(0o755)).unwrap(); }
        bin
    }

    #[test]
    fn the_toolchain_with_the_target_wins_even_when_it_is_not_first_on_path() {
        // Fedora: /usr/bin (distro rust, no musl target) first on PATH,
        // rustup's ~/.cargo/bin (target installed) not on PATH at all (GUI).
        let root = std::env::temp_dir().join(format!("tc_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let distro = fake_toolchain(&root, "usr", &["x86_64-unknown-linux-gnu"]);
        let home = root.join("home");
        let rustup = fake_toolchain(&home, ".cargo", &["x86_64-unknown-linux-gnu", "aarch64-unknown-linux-musl"]);
        assert!(rustup.ends_with(".cargo/bin"));
        let path = distro.to_string_lossy().to_string();
        let t = pick_toolchain("aarch64-unknown-linux-musl", false, &path, Some(&home.to_string_lossy()), None).unwrap();
        assert_eq!(t.origin, "rustup (~/.cargo/bin)");
        // nobody has it: the error lists what was checked
        let e = pick_toolchain("armv7-unknown-linux-musleabihf", false, &path, Some(&home.to_string_lossy()), None).unwrap_err();
        assert!(e.contains("not installed") && e.contains("rustup (~/.cargo/bin)") && e.contains("PATH ("), "{e}");
        assert_eq!(diagnose(&e)[0].code, "build-target-missing");
        let none = pick_toolchain("aarch64-unknown-linux-musl", false, "/nonexistent", None, None).unwrap_err();
        assert_eq!(diagnose(&none)[0].code, "build-no-toolchain");
        // the build is pointed at that toolchain explicitly
        let mut argv = vec!["cargo".to_string(), "build".into()];
        let mut env = vec![];
        use_toolchain(&mut argv, &mut env, &t);
        assert!(argv[0].ends_with(".cargo/bin/cargo"));
        assert!(env.iter().any(|(k, v)| k == "RUSTC" && v.ends_with(".cargo/bin/rustc")));
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn this_sandboxs_real_toolchain_is_found() {
        let t = pick_toolchain_here("x86_64-unknown-linux-gnu", false);
        if cfg!(target_arch = "x86_64") {
            assert!(t.is_ok(), "{t:?}");
            assert!(pick_toolchain_here("aarch64-unknown-linux-musl", false).is_err(), "not installed here");
        }
    }

    #[test]
    fn a_paper_pro_container_build_is_native_arm64_musl_with_the_repo_mounted() {
        let root = std::path::Path::new("/home/user/mono");
        let app = TabletApp { dir: root.join("apps/tomescope"), bin: "tomescope-eink".into(), features: vec!["eink".into()], no_default: true, via_shim: false };
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let p = container_plan("podman", root, &app, &t).unwrap();
        let a = p.argv.join(" ");
        for want in ["podman run --rm --platform linux/arm64", "-v /home/user/mono:/src:Z", "-w /src/apps/tomescope",
                     "CARGO_TARGET_DIR=/src/apps/tomescope/target/tablet-container", CONTAINER_IMAGE,
                     "+crt-static", "--bin tomescope-eink --no-default-features --features eink", "--target aarch64-unknown-linux-musl"] {
            assert!(a.contains(want), "{want:?} not in {a}");
        }
        assert_eq!(p.output, root.join("apps/tomescope/target/tablet-container/aarch64-unknown-linux-musl/release/tomescope-eink"),
                   "a static build names its target, so proc-macros are built for the host without +crt-static");
        assert_eq!(p.cwd, root);
        // the rM2 cross-compiles with rust-lld instead
        let rm2 = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::Rm2));
        let p = container_plan("docker", root, &app, &rm2).unwrap();
        let a = p.argv.join(" ");
        assert!(!a.contains("--platform") && a.contains("rustup target add armv7-unknown-linux-musleabihf") && a.contains("=rust-lld"), "{a}");
        assert!(p.output.ends_with("armv7-unknown-linux-musleabihf/release/tomescope-eink"));
        // Blight (glibc) is not a container job
        assert!(container_plan("podman", root, &app, &build_target(&Route::Blight, Some(RmModel::PaperPro))).is_err());
    }

    #[test]
    fn auto_falls_back_to_the_container_and_explains_both_when_neither_works() {
        let root = std::env::temp_dir().join(format!("plan_{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&root);
        let bin = root.join("bin");
        std::fs::create_dir_all(&bin).unwrap();
        let app = TabletApp { dir: root.join("shared/eink-kit"), bin: "eink-demo".into(), features: vec![], no_default: false, via_shim: false };
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        // no toolchain, no runtime: both reasons in one error
        let e = plan_build(Engine::Auto, &root, &app, &t, &bin.to_string_lossy(), None, None).unwrap_err();
        assert!(e.contains("no Rust toolchain found") && e.contains("no container runtime"), "{e}");
        let codes: Vec<String> = diagnose(&e).into_iter().map(|f| f.code).collect();
        assert!(codes.contains(&"build-no-toolchain".to_string()) && codes.contains(&"build-no-container".to_string()), "{codes:?}");
        // podman appears: auto uses it
        std::fs::write(bin.join("podman"), "").unwrap();
        let p = plan_build(Engine::Auto, &root, &app, &t, &bin.to_string_lossy(), None, None).unwrap();
        assert!(p.note.starts_with("podman"), "{}", p.note);
        assert_eq!(p.argv[0], "podman");
        let _ = std::fs::remove_dir_all(&root);
    }

    #[test]
    fn container_failures_explain_themselves() {
        let f = diagnose("exec /bin/sh: exec format error");
        assert_eq!(f[0].code, "build-container-no-emulation");
        assert!(diagnose("Error: Cannot connect to Podman. Please verify your connection").iter().any(|f| f.fix.as_deref() == Some("podman machine start")));
        assert_eq!(Engine::parse("podman"), Some(Engine::Container));
    }

    #[test]
    fn ordinary_eframe_apps_are_offered_through_the_shim() {
        let snap = "[package]\nname = \"snapclone-egui\"\n[[bin]]\nname = \"snapclone-egui\"\nrequired-features = [\"app\"]\n[features]\ndefault = [\"app\"]\napp = [\"dep:eframe\"]\n[dependencies]\neframe = { version = \"0.34\", optional = true }\n";
        assert_eq!(eframe_app_bins(snap), vec![(Some("snapclone-egui".to_string()), vec![])], "default feature already enables eframe");
        let photo = "[package]\nname = \"photo\"\n[features]\ndefault = []\napp = [\"dep:eframe\"]\n[dependencies]\neframe = { version = \"0.34\", optional = true }\n";
        assert_eq!(eframe_app_bins(photo), vec![(None, vec!["app".to_string()])], "non-default gate is enabled");
        let dev_only = "[package]\nname = \"x\"\n[dev-dependencies]\neframe = \"0.34\"\n";
        assert!(eframe_app_bins(dev_only).is_empty());
        let shim = "[package]\nname = \"eframe\"\n[dependencies]\neframe = \"0.34\"\n";
        assert!(eframe_app_bins(shim).is_empty());
    }

    #[test]
    fn discovery_offers_the_requested_eframe_apps_in_this_checkout() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..");
        let apps = discover_tablet_apps(&root);
        for want in ["snapclone-egui", "bo-suite-egui", "rusty-board-egui"] {
            let a = apps.iter().find(|a| a.bin == want).unwrap_or_else(|| panic!("{want} not offered: {:?}", apps.iter().map(|a| &a.bin).collect::<Vec<_>>()));
            assert!(a.via_shim, "{want} goes through the shim");
        }
        assert!(apps.iter().any(|a| a.bin == "eink-demo" && !a.via_shim), "eink-kit apps stay native");
        assert!(apps.iter().any(|a| a.bin == "alpine-run" && !a.via_shim), "tablet tools that opt in are offered");
        assert!(!apps.iter().any(|a| a.bin == "tomescope"), "tomescope has an eink shell; its desktop bin is not offered via the shim");
    }

    #[test]
    fn the_lockfile_comes_back_exactly_as_it_was() {
        let dir = std::env::temp_dir().join(format!("lockp_{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let lock = dir.join("Cargo.lock");
        std::fs::write(&lock, "original").unwrap();
        let plan = BuildPlan { argv: vec![], env: vec![], cwd: dir.clone(), output: dir.clone(), note: String::new(), lockfile: Some(lock.clone()) };
        let r = with_lock_preserved(&plan, || { std::fs::write(&lock, "rewritten by the patch").unwrap(); 7 });
        assert_eq!(r, 7);
        assert_eq!(std::fs::read_to_string(&lock).unwrap(), "original");
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn shim_builds_patch_eframe_set_eink_shim_and_track_the_lockfile() {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../../../..").canonicalize().unwrap();
        let app = TabletApp { dir: root.join("apps/snapclone/egui-frontend"), bin: "snapclone-egui".into(), features: vec![], no_default: false, via_shim: true };
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let p = container_plan("podman", &root, &app, &t).unwrap();
        let a = p.argv.join(" ");
        assert!(a.contains("patch.crates-io.eframe.path=\"/src/shared/eink-kit/eframe-shim\""), "{a}");
        assert!(a.contains("export EINK_SHIM=1") && a.contains("openssl-libs-static"), "{a}");
        assert_eq!(p.lockfile.as_deref(), Some(root.join("apps/snapclone/egui-frontend/Cargo.lock").as_path()));
        let tc = Toolchain { cargo: "/x/cargo".into(), rustc: "/x/rustc".into(), origin: "t".into() };
        let h = host_plan(&app, &t, &tc, false, None, &root);
        assert!(h.argv.iter().any(|x| x.contains("eframe-shim")) && h.env.iter().any(|(k, v)| k == "EINK_SHIM" && v == "1"));
    }

    #[test]
    fn a_surveyed_tablet_builds_studio_without_audio_and_says_why() {
        let dir = std::env::temp_dir().join(format!("studioapp_{}", std::process::id()));
        std::fs::create_dir_all(dir.join("src")).unwrap();
        std::fs::write(dir.join("Cargo.toml"), "[package]\nname = \"studio-app\"\n[features]\ndefault = [\"app\", \"audio\", \"midi\"]\napp = [\"dep:eframe\"]\naudio = []\nmidi = []\n").unwrap();
        let app = TabletApp { dir: dir.clone(), bin: "studio-app".into(), features: vec![], no_default: false, via_shim: true };
        let rm = crate::capabilities::parse("arch=aarch64\ncores=4\nmem_mb=1900\nalsa_cards=0\nbluetooth=1\n");
        let (a, dropped) = app_for_device(&app, Some(&rm));
        assert!(a.no_default && a.features == ["app"], "{a:?}");
        assert_eq!(dropped.len(), 2);
        let t = build_target(&Route::AppLoad { dir: "/home/root/x".into() }, Some(RmModel::PaperPro));
        let p = container_plan("podman", &std::env::temp_dir(), &a, &t).unwrap();
        assert!(p.argv.join(" ").contains("--no-default-features --features app"), "{:?}", p.argv);
        // tomescope-style explicit feature sets are left alone
        let explicit = TabletApp { no_default: true, features: vec!["eink".into()], ..app.clone() };
        assert_eq!(app_for_device(&explicit, Some(&rm)).0, explicit);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn an_alpine_root_app_launches_through_the_runner_with_its_binary() {
        let opts = AppLoadOptions { userland: Userland::AlpineRoot, args: vec!["--x".into()], ..AppLoadOptions::named("voiceterm") };
        let v: serde_json::Value = serde_json::from_str(&appload_manifest(&opts, "voiceterm-egui", Some(RmModel::PaperPro), RefreshProfile::FastColor)).unwrap();
        assert_eq!(v["application"], crate::alpine_root::RUNNER);
        let args: Vec<&str> = v["args"].as_array().unwrap().iter().map(|a| a.as_str().unwrap()).collect();
        assert_eq!(args, vec![crate::alpine_root::ROOT, "/home/root/.local/share/gitmind/bin/voiceterm-egui", "--x"]);
        assert_eq!(v["environment"]["EINK_BACKEND"], "qtfb", "the app still talks to AppLoad (qtfb socket bound into the root)");
    }

    #[test]
    fn an_alpine_root_build_uses_the_image_of_the_roots_release() {
        let mut t = build_target_for(&Route::AppLoad { dir: "/x".into() }, Some(RmModel::PaperPro), Userland::AlpineRoot);
        t.container_image = crate::alpine_root::image_for_release("3.22.1");
        let app = TabletApp { dir: "/r/apps/x".into(), bin: "x".into(), features: vec![], no_default: false, via_shim: false };
        let p = container_plan("podman", std::path::Path::new("/r"), &app, &t).unwrap();
        assert!(p.argv.iter().any(|a| a == "docker.io/library/rust:1-alpine3.22"), "{:?}", p.argv);
        assert!(p.note.contains("alpine3.22"));
    }

    #[test]
    fn the_alpine_root_route_builds_dynamic_musl_in_the_arm64_container() {
        let r = Route::AppLoad { dir: "/home/root/x".into() };
        let t = build_target_for(&r, Some(RmModel::PaperPro), Userland::AlpineRoot);
        assert!(!t.static_crt && t.triple == "aarch64-unknown-linux-musl" && t.userland == Userland::AlpineRoot);
        let app = TabletApp { dir: "/r/apps/x".into(), bin: "x".into(), features: vec![], no_default: false, via_shim: false };
        let p = container_plan("podman", std::path::Path::new("/r"), &app, &t).unwrap();
        let a = p.argv.join(" ");
        assert!(a.contains("--platform linux/arm64") && a.contains("-crt-static") && !a.contains("+crt-static"), "{a}");
        let rm2 = build_target_for(&r, Some(RmModel::Rm2), Userland::AlpineRoot);
        assert!(container_plan("podman", std::path::Path::new("/r"), &app, &rm2).unwrap_err().contains("armv7"));
        assert_eq!(build_target_for(&Route::Blight, Some(RmModel::PaperPro), Userland::AlpineRoot).userland, Userland::Static, "Blight is not an AppLoad route");
    }

    #[test]
    fn preflight_accepts_musl_dynamic_for_the_root_and_refuses_glibc() {
        // a minimal aarch64 ELF64 with a PT_INTERP naming the given loader
        let elf = |interp: &str| {
            let mut b = vec![0u8; 0x100];
            b[..4].copy_from_slice(b"\x7fELF"); b[4] = 2; b[5] = 1; b[18] = 183;
            b[0x20..0x28].copy_from_slice(&64u64.to_le_bytes()); // phoff
            b[0x36..0x38].copy_from_slice(&56u16.to_le_bytes()); // phentsize
            b[0x38..0x3a].copy_from_slice(&1u16.to_le_bytes());  // phnum
            b[64..68].copy_from_slice(&3u32.to_le_bytes());      // PT_INTERP
            b[64 + 8..64 + 16].copy_from_slice(&0xc0u64.to_le_bytes());
            b[0xc0..0xc0 + interp.len()].copy_from_slice(interp.as_bytes());
            b
        };
        let t = build_target_for(&Route::AppLoad { dir: "/x".into() }, Some(RmModel::PaperPro), Userland::AlpineRoot);
        assert!(check_tablet_binary(&elf("/lib/ld-musl-aarch64.so.1"), &t).is_ok());
        assert!(check_tablet_binary(&elf("/lib/ld-linux-aarch64.so.1"), &t).unwrap_err().contains("not musl"));
        let static_t = build_target(&Route::AppLoad { dir: "/x".into() }, Some(RmModel::PaperPro));
        assert!(check_tablet_binary(&elf("/lib/ld-musl-aarch64.so.1"), &static_t).is_err(), "the system route still needs static");
    }

    #[test]
    fn deploying_into_the_alpine_root_installs_the_binarys_libraries_then_registers_through_the_runner() {
        let Ok(_) = std::fs::metadata("/bin/ls") else { return };
        let mut r = Fake { log: vec![], status: String::new() };
        let env = LaunchEnv { profile: RefreshProfile::FastColor, model: Some(RmModel::PaperPro), blight_lib: None };
        let opts = AppLoadOptions { userland: Userland::AlpineRoot, ..AppLoadOptions::named("demo") };
        let route = Route::AppLoad { dir: "/home/root/xovi/exthome/appload".into() };
        let d = deliver(&mut r, &route, "demo", "/bin/ls", &env, &[], Some((&opts, None)), &mut |_| {}).unwrap();
        assert!(matches!(d, Delivered::Registered { .. }));
        let apk = r.log.iter().position(|c| c.contains("apk.static") && c.contains(" add so:")).expect("libraries installed into the root");
        let manifest = r.log.iter().position(|c| c.contains("external.manifest.json")).unwrap();
        assert!(apk < manifest, "libraries before the app is offered in AppLoad");
        assert!(r.log[apk].contains("so:libc.so.6"), "named from the binary's own DT_NEEDED: {}", r.log[apk]);
        assert!(r.log[manifest].contains("alpine-run"), "launched through the runner");
    }
}

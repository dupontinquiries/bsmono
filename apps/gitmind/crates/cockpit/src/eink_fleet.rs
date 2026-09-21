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
        crate::deploy::remote_capture(&self.user, &self.host, cmd)
    }
    fn push(&mut self, local: &str, remote: &str) -> Result<(), String> {
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
    "rootfs_ro", "etc_overlay", "xovi_installed", "launcher_current", "launcher_active",
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
            "first contact needs the tablet's SSH password: on the tablet, Settings → Help → \
             Copyrights and licenses (scroll to the end); developer mode may need enabling first"
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

/// The real enroller: `root@host` over the hardened ssh options.
pub struct SshEnroller {
    pub host: String,
}

impl Enroller for SshEnroller {
    fn trust_host(&mut self) -> Result<String, String> {
        crate::deploy::trust_host(&self.host)
    }
    fn key_check(&mut self) -> KeyCheck {
        match crate::deploy::remote_capture("root", &self.host, "true") {
            Ok(_) => KeyCheck::Works,
            Err(e) if crate::deploy::ssh_error_reason(&e) == "hostkey" => KeyCheck::HostKeyChanged,
            Err(e) if crate::deploy::ssh_error_reason(&e) == "legacy-hostkey" => KeyCheck::LegacyHostKey,
            Err(_) => KeyCheck::Fails,
        }
    }
    fn install_key(&mut self, password: &str) -> Result<String, String> {
        crate::deploy::setup_key("root", &self.host, password)
    }
    fn run(&mut self, cmd: &str) -> Result<String, String> {
        crate::deploy::remote_capture("root", &self.host, cmd)
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

// ───────────────────────── AppLoad route ─────────────────────────

/// AppLoad's `external.manifest.json` for an eink-kit app. Schema from
/// rm-appload `src/libraryexternals.cpp`: `name` and `application` are
/// required; `qtfb: true` makes AppLoad open a window and pass `QTFB_KEY`;
/// `environment` is passed through, which is how the model and profile reach
/// the app without device detection.
pub fn appload_manifest(display_name: &str, app: &str, model: Option<RmModel>, profile: RefreshProfile) -> String {
    let mut env = serde_json::Map::new();
    env.insert("EINK_BACKEND".into(), "qtfb".into());
    env.insert(PROFILE_ENV.into(), profile.as_env().into());
    if let Some(m) = model {
        env.insert("EINK_MODEL".into(), m.env_id().into());
    }
    let v = serde_json::json!({
        "name": display_name,
        "application": bin_path(app),
        "qtfb": true,
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
    Ok(format!(
        "mkdir -p {d} && printf '%s\\n' {m} > {d}/external.manifest.json && echo manifest-written",
        d = shell_quote(&dir),
        m = shell_quote(manifest),
    ))
}

/// Push + promote, then either launch (Blight) or register with AppLoad.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Delivered {
    Launched(Outcome),
    Registered { manifest_dir: String },
}

pub fn deliver(
    remote: &mut dyn Remote,
    route: &Route,
    app: &str,
    local_bin: &str,
    env: &LaunchEnv,
    args: &[&str],
    on: &mut dyn FnMut(Event),
) -> Result<Delivered, String> {
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
            let manifest = appload_manifest(app, app, env.model, env.profile);
            remote.run(&appload_install_cmd(app, dir, &manifest)?).map_err(|e| format!("manifest: {e}"))?;
            Ok(Delivered::Registered { manifest_dir: format!("{dir}/{app}") })
        }
    }
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
        let m = appload_manifest("eink demo", "eink-demo", Some(RmModel::PaperProMove), RefreshProfile::FastColor);
        let v: serde_json::Value = serde_json::from_str(&m).unwrap();
        assert_eq!(v["name"], "eink demo");
        assert_eq!(v["application"], "/home/root/.local/share/gitmind/bin/eink-demo");
        assert_eq!(v["qtfb"], true);
        assert_eq!(v["environment"]["EINK_MODEL"], "rmppm");
        assert_eq!(v["environment"]["EINK_BACKEND"], "qtfb");
        assert_eq!(v["environment"]["EINK_PROFILE"], "fast-color");
    }

    #[test]
    fn the_manifest_command_survives_the_shell_with_quotes_in_it() {
        let m = appload_manifest("it's \"quoted\"", "demo", None, RefreshProfile::Quality);
        let dir = std::env::temp_dir().join(format!("appload_{}", std::process::id()));
        let real = "/home/root/xovi/exthome/appload";
        let cmd = appload_install_cmd("demo", real, &m).unwrap().replace(real, &dir.to_string_lossy());
        let out = std::process::Command::new("sh").arg("-c").arg(&cmd).output().unwrap();
        assert!(String::from_utf8_lossy(&out.stdout).contains("manifest-written"));
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
}

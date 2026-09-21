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

/// One shell round-trip that reports what the device is. Each probe is
/// allowed to fail silently; the parser works with whatever came back.
pub fn probe_cmd() -> &'static str {
    "cat /proc/device-tree/model 2>/dev/null; echo; \
     cat /sys/devices/soc0/machine 2>/dev/null; echo; \
     uname -m"
}

/// What a probe established.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Probe {
    /// From `uname -m`, which is reliable. Enough to build for the device
    /// even when the model is unknown.
    pub triple: Option<&'static str>,
    /// Best-effort. `None` means "ask the operator", recorded in the roster
    /// like every other identity the network cannot report.
    pub model: Option<RmModel>,
}

/// Parse [`probe_cmd`] output.
///
/// UNVERIFIED: the model strings matched here are assumptions. The Paper Pro
/// family is matched on the codenames reManager uses for OS images
/// (ferrari / chiappa / tatsu); rM1/rM2 on the human-readable model string.
/// The exact text each device writes to `/proc/device-tree/model` has not
/// been checked on hardware. A miss degrades to `model: None`, never to a
/// wrong model, because every arm is a positive match.
pub fn parse_probe(out: &str) -> Probe {
    let lower = out.to_ascii_lowercase();
    let arch = out.lines().map(str::trim).filter(|l| !l.is_empty()).last().unwrap_or("");
    let triple = match arch {
        "aarch64" | "arm64" => Some("aarch64-unknown-linux-gnu"),
        a if a.starts_with("armv7") => Some("armv7-unknown-linux-gnueabihf"),
        _ => None,
    };
    let model = if lower.contains("ferrari") {
        Some(RmModel::PaperPro)
    } else if lower.contains("chiappa") {
        Some(RmModel::PaperProMove)
    } else if lower.contains("tatsu") {
        Some(RmModel::PaperPure)
    } else if lower.contains("remarkable 2") {
        Some(RmModel::Rm2)
    } else if lower.contains("remarkable 1") || lower.contains("remarkable prototype 1") {
        Some(RmModel::Rm1)
    } else {
        None
    };
    // A model whose triple contradicts uname is a bad match, not a device.
    let model = match (model, triple) {
        (Some(m), Some(t)) if m.triple() != t => None,
        (m, _) => m,
    };
    Probe { triple, model }
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

/// Stop any previous instance, then start detached and print the new pid.
///
/// `nohup env ... BIN &` keeps `$!` equal to the app's pid, because nohup
/// and env both exec. Every fd is redirected so ssh returns immediately
/// instead of waiting on the child's stdout. `args` are passed through
/// individually quoted (e.g. the file `tomescope` should open).
pub fn launch_cmd(app: &str, profile: RefreshProfile, args: &[&str]) -> Result<String, String> {
    let stop = stop_cmd(app)?;
    let args: String = args.iter().map(|a| format!(" {}", shell_quote(a))).collect();
    Ok(format!(
        "{stop}; \
         cd {base} || exit 1; \
         nohup env {env}={val} {bin}{args} </dev/null >{log} 2>&1 & \
         echo $! > {pid}; echo \"started $!\"",
        base = shell_quote(BASE),
        env = PROFILE_ENV,
        val = profile.as_env(),
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
         echo ---; tail -n 20 {log} 2>/dev/null",
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
    profile: RefreshProfile,
    args: &[&str],
    settle_secs: u32,
    on: &mut dyn FnMut(Event),
) -> Result<Outcome, String> {
    check(app)?;
    on(Event::Preparing);
    remote.run(&prepare_cmd()).map_err(|e| format!("prepare: {e}"))?;
    on(Event::Pushing { bytes: std::fs::metadata(local_bin).ok().map(|m| m.len()) });
    remote.push(local_bin, &staging_path(app)).map_err(|e| format!("push: {e}"))?;
    on(Event::Installing);
    remote.run(&install_cmd(app)?).map_err(|e| format!("install: {e}"))?;
    on(Event::Launching { profile });
    remote.run(&launch_cmd(app, profile, args)?).map_err(|e| format!("launch: {e}"))?;
    on(Event::Checking);
    let st = parse_status(&remote.run(&status_cmd(app, settle_secs)?).map_err(|e| format!("status: {e}"))?);
    Ok(match st.pid {
        Some(pid) => Outcome::Running { pid },
        None => Outcome::ExitedEarly { log_tail: st.log_tail },
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn triple_comes_from_uname_even_without_a_model() {
        let p = parse_probe("\n\naarch64\n");
        assert_eq!(p.triple, Some("aarch64-unknown-linux-gnu"));
        assert_eq!(p.model, None);
        assert_eq!(parse_probe("x\n\narmv7l").triple, Some("armv7-unknown-linux-gnueabihf"));
    }

    #[test]
    fn codenames_identify_the_paper_pro_family() {
        assert_eq!(parse_probe("reMarkable Ferrari\n\naarch64").model, Some(RmModel::PaperPro));
        assert_eq!(parse_probe("chiappa\n\naarch64").model, Some(RmModel::PaperProMove));
        assert_eq!(parse_probe("\ntatsu\naarch64").model, Some(RmModel::PaperPure));
        assert_eq!(parse_probe("reMarkable 2.0\n\narmv7l").model, Some(RmModel::Rm2));
    }

    #[test]
    fn a_model_contradicting_uname_is_dropped_not_trusted() {
        // "reMarkable 2" text on an aarch64 kernel is a bad match.
        let p = parse_probe("reMarkable 2.0\n\naarch64");
        assert_eq!(p.model, None);
        assert_eq!(p.triple, Some("aarch64-unknown-linux-gnu"));
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
            assert!(launch_cmd(bad, RefreshProfile::FastMono, &[]).is_err(), "{bad:?}");
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
        let c = launch_cmd("tomescope", RefreshProfile::FastColor, &["/home/root/a b.epub"]).unwrap();
        assert!(c.contains("tomescope' '/home/root/a b.epub' </dev/null"));
        assert!(c.contains("EINK_PROFILE=fast-color"));
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

        let out = sh(&rebase(launch_cmd("demo", RefreshProfile::FastColor, &["30"]).unwrap()));
        assert!(out.starts_with("started "), "{out}");
        let st = parse_status(&sh(&rebase(status_cmd("demo", 1).unwrap())));
        let pid = st.pid.expect("running");
        let environ = std::fs::read(format!("/proc/{pid}/environ")).unwrap();
        assert!(environ.split(|b| *b == 0).any(|kv| kv == b"EINK_PROFILE=fast-color"));

        // relaunch replaces the old instance rather than stacking a second one
        sh(&rebase(launch_cmd("demo", RefreshProfile::FastMono, &["30"]).unwrap()));
        let st2 = parse_status(&sh(&rebase(status_cmd("demo", 1).unwrap())));
        assert!(st2.pid.is_some());
        assert_ne!(st2.pid, Some(pid));
        assert!(!alive(pid), "old instance survived");

        sh(&rebase(stop_cmd("demo").unwrap()));
        assert_eq!(parse_status(&sh(&rebase(status_cmd("demo", 0).unwrap()))).pid, None);
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
        let out = deploy_and_launch(&mut f, "tomescope", "/nonexistent", RefreshProfile::FastColor, &[], 2, &mut |e| ev.push(e))
            .unwrap();
        assert_eq!(out, Outcome::ExitedEarly { log_tail: "panic: no display".into() });
        assert!(f.log[0].starts_with("mkdir -p"));
        assert_eq!(f.log[1], format!("PUSH {}", staging_path("tomescope")));
        assert!(f.log[2].contains("mv -f"));
        assert!(f.log[3].contains("nohup"));
        assert!(f.log[4].starts_with("sleep 2"));
        assert_eq!(ev.len(), 5);
    }
}

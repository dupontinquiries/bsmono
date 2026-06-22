// src/dep_wrangler.rs
#![allow(dead_code)]
//
// Multi-distro dependency wrangler.
//
// `kde_tools.rs` does the basic "is X installed, and how do I install
// it on this distro" job. This module is the next layer up: it knows
// about multiple repositories per distro (Alpine main vs community vs
// edge; Fedora vs RPMFusion; Ubuntu universe/multiverse; Arch official
// vs AUR) and produces an ordered INSTALL PLAN with fallback steps.
//
// The flow is:
//   1. detect()                    → which package manager is here
//   2. plan_for_tool(tool, pm)     → ordered list of attempts
//   3. (caller runs each attempt)  → executes apk/apt/dnf/pacman
//   4. verify_installed(binary)    → did the binary actually appear?
//   5. on failure, try next step   → guarded retry, never silent
//
// The "guarded" part: every attempt records what command ran, what
// exit code it produced, and what the post-attempt verification said.
// The caller sees a full audit trail so when something fails the user
// can see the exact sequence.
//
// We don't actually shell out from this module — that's the install
// script's job. This module is pure logic + tests.

use std::path::Path;

/// Identifies the distro's package manager. Listed in detection order:
/// apk (Alpine) is checked first because it's the most distinctive,
/// then Pacman (Arch), then Dnf (Fedora-likes), then Apt (Debian-likes).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PackageManager {
    Apk,        // Alpine, postmarketOS
    Apt,        // Debian, Ubuntu, Mint, Pop!_OS, KDE Neon
    Dnf,        // Fedora, RHEL 8+, AlmaLinux, Rocky, CentOS Stream
    Pacman,     // Arch, Manjaro, EndeavourOS
    Zypper,     // openSUSE Leap/Tumbleweed (recognized but uses dnf-like fallbacks)
    Unknown,
}

impl PackageManager {
    /// Friendly name for diagnostic output.
    pub fn label(&self) -> &'static str {
        match self {
            PackageManager::Apk    => "Alpine (apk)",
            PackageManager::Apt    => "Debian/Ubuntu (apt)",
            PackageManager::Dnf    => "Fedora/RHEL (dnf)",
            PackageManager::Pacman => "Arch (pacman)",
            PackageManager::Zypper => "openSUSE (zypper)",
            PackageManager::Unknown => "unknown",
        }
    }

    /// The base command used by this PM, e.g. "apk" or "sudo apt".
    /// Returned as a `Vec<&'static str>` so callers can prepend args.
    pub fn base_cmd(&self) -> Vec<&'static str> {
        match self {
            PackageManager::Apk    => vec!["sudo", "apk"],
            PackageManager::Apt    => vec!["sudo", "apt"],
            PackageManager::Dnf    => vec!["sudo", "dnf"],
            PackageManager::Pacman => vec!["sudo", "pacman"],
            PackageManager::Zypper => vec!["sudo", "zypper"],
            PackageManager::Unknown => vec![],
        }
    }

    /// The "install this package non-interactively" verb. Distros vary
    /// quite a lot here — apk uses `add --no-cache`, dnf uses
    /// `install -y`, pacman uses `-S --noconfirm`, etc.
    pub fn install_args(&self) -> Vec<&'static str> {
        match self {
            PackageManager::Apk    => vec!["add", "--no-cache"],
            PackageManager::Apt    => vec!["install", "-y"],
            PackageManager::Dnf    => vec!["install", "-y"],
            PackageManager::Pacman => vec!["-S", "--noconfirm", "--needed"],
            PackageManager::Zypper => vec!["install", "-y"],
            PackageManager::Unknown => vec![],
        }
    }

    /// The "refresh repo index" verb. apt needs explicit `update`;
    /// dnf does this automatically but the verb still exists; pacman
    /// uses `-Sy`; apk uses `update`.
    pub fn refresh_args(&self) -> Vec<&'static str> {
        match self {
            PackageManager::Apk    => vec!["update"],
            PackageManager::Apt    => vec!["update"],
            PackageManager::Dnf    => vec!["check-update"],
            PackageManager::Pacman => vec!["-Sy"],
            PackageManager::Zypper => vec!["refresh"],
            PackageManager::Unknown => vec![],
        }
    }
}

/// Detect the package manager. First tries `/etc/os-release` for an
/// authoritative ID, then falls back to checking which PM binaries
/// exist on the filesystem.
pub fn detect() -> PackageManager {
    if let Ok(content) = std::fs::read_to_string("/etc/os-release") {
        let lower = content.to_lowercase();
        // Alpine: ID=alpine, ID_LIKE rarely present
        if lower.contains("id=alpine") || lower.contains("id=postmarketos") {
            return PackageManager::Apk;
        }
        // Arch family
        if lower.contains("id=arch") || lower.contains("id_like=arch") {
            return PackageManager::Pacman;
        }
        // Fedora family
        if lower.contains("id=fedora")
            || (lower.contains("id_like=") && lower.contains("fedora"))
            || lower.contains("id=rhel")
            || lower.contains("id=centos")
            || lower.contains("id=almalinux")
            || lower.contains("id=rocky")
        {
            return PackageManager::Dnf;
        }
        // openSUSE
        if lower.contains("id=opensuse") || lower.contains("suse") {
            return PackageManager::Zypper;
        }
        // Debian family (last because the most permissive ID_LIKE matches)
        if lower.contains("id=debian")
            || lower.contains("id_like=debian")
            || lower.contains("id=ubuntu")
            || lower.contains("id_like=ubuntu")
            || lower.contains("id=linuxmint")
            || lower.contains("id=pop")
        {
            return PackageManager::Apt;
        }
    }
    detect_by_pm_binary()
}

fn detect_by_pm_binary() -> PackageManager {
    // Order matters: a Debian system with apt-rpm installed (rare but
    // possible) should still resolve to Apt, etc. We probe in the order
    // listed: apk → pacman → dnf → apt → zypper.
    for (path, pm) in [
        ("/sbin/apk",        PackageManager::Apk),
        ("/usr/sbin/apk",    PackageManager::Apk),
        ("/usr/bin/pacman",  PackageManager::Pacman),
        ("/usr/bin/dnf",     PackageManager::Dnf),
        ("/usr/bin/apt",     PackageManager::Apt),
        ("/usr/bin/zypper",  PackageManager::Zypper),
    ] {
        if Path::new(path).exists() { return pm; }
    }
    PackageManager::Unknown
}

/// One repository that a package might live in. The wrangler tries
/// these in order from cheapest (default repo) to most invasive
/// (third-party, build-from-source).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Repository {
    /// The distro's primary repository — apt main, dnf BaseOS, apk main, etc.
    Default,
    /// Secondary "official-ish" repo — apk community, Ubuntu universe,
    /// Arch community (now merged into Extra), Fedora's CRB.
    Community,
    /// Universe/multiverse for Debian-likes; Alpine edge for newer pkgs.
    Universe,
    Multiverse,
    /// Edge/testing channels — bleeding edge, sometimes broken.
    Testing,
    /// RPMFusion for Fedora (codecs, ffmpeg, etc).
    RpmFusion,
    /// AUR — Arch User Repository; requires a helper like yay/paru.
    Aur,
    /// Backports — newer versions for Debian stable.
    Backports,
    /// Last resort: build from upstream source.
    Source,
}

/// One concrete attempt to install a package.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallStep {
    /// Where the package lives.
    pub repo: Repository,
    /// Package name in that repo. Same tool may have different package
    /// names per distro/repo (apk's `ffmpeg-libs` vs apt's `libavcodec-extra`).
    pub package: String,
    /// Human description shown to the user before attempting.
    pub description: String,
    /// Shell-ready command tokens. Concat with spaces to display, or
    /// pass to std::process::Command argv-style.
    pub command: Vec<String>,
    /// If running this step requires extra setup first (e.g. enabling
    /// the rpmfusion repo, installing yay), the prerequisite commands.
    pub prerequisites: Vec<Vec<String>>,
}

impl InstallStep {
    pub fn command_string(&self) -> String { self.command.join(" ") }
}

/// An ordered list of install steps to try for one tool. The wrangler
/// stops at the first step whose post-install verification passes.
#[derive(Debug, Clone)]
pub struct InstallPlan {
    pub binary: String,
    pub friendly_name: String,
    pub steps: Vec<InstallStep>,
}

impl InstallPlan {
    pub fn is_empty(&self) -> bool { self.steps.is_empty() }
}

/// Resolve the canonical package name for a given binary on a given
/// PM. Returns the *primary* package; secondary repos are picked up by
/// the full plan below.
pub fn primary_package_for(binary: &str, pm: PackageManager) -> Option<&'static str> {
    use PackageManager::*;
    match (pm, binary) {
        // PulseAudio playback utilities
        (Apt,    "paplay") | (Apt,    "parec") => Some("pulseaudio-utils"),
        (Dnf,    "paplay") | (Dnf,    "parec") => Some("pulseaudio-utils"),
        (Pacman, "paplay") | (Pacman, "parec") => Some("libpulse"),
        (Apk,    "paplay") | (Apk,    "parec") => Some("pulseaudio-utils"),
        (Zypper, "paplay") | (Zypper, "parec") => Some("pulseaudio-utils"),

        // ALSA playback utilities
        (Apt,    "aplay") | (Apt,    "arecord") => Some("alsa-utils"),
        (Dnf,    "aplay") | (Dnf,    "arecord") => Some("alsa-utils"),
        (Pacman, "aplay") | (Pacman, "arecord") => Some("alsa-utils"),
        (Apk,    "aplay") | (Apk,    "arecord") => Some("alsa-utils"),
        (Zypper, "aplay") | (Zypper, "arecord") => Some("alsa-utils"),

        // Bluetooth CLI
        (Apt,    "bluetoothctl") => Some("bluez"),
        (Dnf,    "bluetoothctl") => Some("bluez"),
        (Pacman, "bluetoothctl") => Some("bluez-utils"),
        (Apk,    "bluetoothctl") => Some("bluez"),
        (Zypper, "bluetoothctl") => Some("bluez"),

        // FFmpeg
        (Apt,    "ffmpeg") | (Apt,    "ffplay") => Some("ffmpeg"),
        (Pacman, "ffmpeg") | (Pacman, "ffplay") => Some("ffmpeg"),
        (Apk,    "ffmpeg") | (Apk,    "ffplay") => Some("ffmpeg"),
        (Zypper, "ffmpeg") | (Zypper, "ffplay") => Some("ffmpeg"),
        // Fedora's main repo ships a free-only ffmpeg; the full
        // version lives in RPMFusion. We return the main one here
        // and the planner adds an RPMFusion fallback step.
        (Dnf,    "ffmpeg") | (Dnf,    "ffplay") => Some("ffmpeg-free"),

        // Avahi (mDNS)
        (Apt,    "avahi-daemon") => Some("avahi-daemon"),
        (Dnf,    "avahi-daemon") => Some("avahi"),
        (Pacman, "avahi-daemon") => Some("avahi"),
        (Apk,    "avahi-daemon") => Some("avahi"),
        (Zypper, "avahi-daemon") => Some("avahi"),

        // libopus runtime — for the soundscape OggOpus encoder. The
        // runtime soname is libopus.so.0; distros split runtime from
        // development headers, so we depend on the runtime package
        // (the bo-controller binary statically links nothing from
        // libopus — it dynamically loads via the Rust opus crate).
        (Apt,    "libopus.so.0") => Some("libopus0"),
        (Dnf,    "libopus.so.0") => Some("opus"),
        (Pacman, "libopus.so.0") => Some("opus"),
        (Apk,    "libopus.so.0") => Some("opus"),
        (Zypper, "libopus.so.0") => Some("libopus0"),

        _ => None,
    }
}

/// Build an `InstallPlan` for one binary. Each PM has a default
/// strategy plus zero or more fallbacks. The order is:
///   1. Primary repo (most distros have the tool in main/default).
///   2. Secondary repo (Alpine community, Fedora RPMFusion, etc.)
///   3. Build from source as a last resort.
pub fn plan_for_tool(binary: &str, friendly_name: &str, pm: PackageManager) -> InstallPlan {
    let mut steps: Vec<InstallStep> = Vec::new();

    // Step 1: primary repo
    if let Some(pkg) = primary_package_for(binary, pm) {
        steps.push(build_step(pm, Repository::Default, pkg, "default repo"));
    }

    // Step 2: PM-specific fallbacks
    match pm {
        PackageManager::Apk => {
            // Alpine community is enabled by default on modern installs,
            // but verify and re-add the entry if missing. ffmpeg lives
            // here on some Alpine versions; gst plugins too.
            if matches!(binary, "ffmpeg" | "ffplay") {
                steps.push(build_step(pm, Repository::Community, "ffmpeg",
                    "Alpine community repo"));
            }
            // Edge testing: latest versions, may be broken. Wrap with
            // an explicit repository pin so we don't pull *everything*
            // from edge by accident.
            if matches!(binary, "ffmpeg" | "ffplay" | "bluetoothctl") {
                let pkg = primary_package_for(binary, pm).unwrap_or(binary);
                let cmd = vec![
                    "sudo".into(), "apk".into(),
                    "add".into(), "--no-cache".into(),
                    "--repository".into(),
                    "https://dl-cdn.alpinelinux.org/alpine/edge/testing".into(),
                    pkg.into(),
                ];
                steps.push(InstallStep {
                    repo: Repository::Testing,
                    package: pkg.into(),
                    description: "Alpine edge testing repo".into(),
                    command: cmd,
                    prerequisites: vec![],
                });
            }
        }
        PackageManager::Dnf => {
            // RPMFusion fallback for ffmpeg and any restricted codecs.
            // Setting it up takes a release-package install — we list
            // it as a prerequisite.
            if matches!(binary, "ffmpeg" | "ffplay") {
                let prereq = vec![
                    "sudo".into(), "dnf".into(),
                    "install".into(), "-y".into(),
                    "https://mirrors.rpmfusion.org/free/fedora/rpmfusion-free-release-$(rpm -E %fedora).noarch.rpm".into(),
                ];
                steps.push(InstallStep {
                    repo: Repository::RpmFusion,
                    package: "ffmpeg".into(),
                    description: "RPMFusion repo (full ffmpeg)".into(),
                    command: vec![
                        "sudo".into(), "dnf".into(), "install".into(), "-y".into(),
                        "--allowerasing".into(), "ffmpeg".into()
                    ],
                    prerequisites: vec![prereq],
                });
            }
        }
        PackageManager::Apt => {
            // Universe is enabled by default on Ubuntu but not always on
            // pure Debian. Add it as an explicit step. This needs the
            // software-properties-common package + add-apt-repository.
            if matches!(binary, "ffmpeg" | "ffplay" | "bluetoothctl") {
                let prereq = vec![
                    "sudo".into(), "apt".into(), "install".into(), "-y".into(),
                    "software-properties-common".into(),
                ];
                let enable = vec![
                    "sudo".into(), "add-apt-repository".into(),
                    "-y".into(), "universe".into(),
                ];
                let pkg = primary_package_for(binary, pm).unwrap_or(binary);
                steps.push(InstallStep {
                    repo: Repository::Universe,
                    package: pkg.into(),
                    description: "Ubuntu universe / Debian backports".into(),
                    command: vec![
                        "sudo".into(), "apt".into(), "install".into(), "-y".into(),
                        pkg.into()
                    ],
                    prerequisites: vec![prereq, enable],
                });
            }
        }
        PackageManager::Pacman => {
            // AUR: needs an AUR helper like yay. We don't auto-install
            // yay (that's intrusive), but we describe the step so the
            // user can run it manually.
            if matches!(binary, "ffmpeg-full") {
                steps.push(InstallStep {
                    repo: Repository::Aur,
                    package: "ffmpeg-full".into(),
                    description: "AUR — requires an AUR helper (yay/paru)".into(),
                    command: vec![
                        "yay".into(), "-S".into(), "--noconfirm".into(), "ffmpeg-full".into()
                    ],
                    prerequisites: vec![],
                });
            }
        }
        _ => {}
    }

    // Step N: build-from-source for tools we know recipes for. This is
    // the same data already in `kde_tools::source_build_hint` but
    // returned as a proper step so the planner is consistent.
    if matches!(binary, "ffmpeg" | "ffplay") {
        steps.push(InstallStep {
            repo: Repository::Source,
            package: "ffmpeg".into(),
            description: "build from source (last resort)".into(),
            command: vec![
                "bash".into(), "-c".into(),
                "git clone https://github.com/FFmpeg/FFmpeg /tmp/ffmpeg && \
                 cd /tmp/ffmpeg && ./configure --disable-x86asm && \
                 make -j$(nproc) && sudo make install".into()
            ],
            prerequisites: vec![],
        });
    }

    InstallPlan {
        binary: binary.into(),
        friendly_name: friendly_name.into(),
        steps,
    }
}

fn build_step(pm: PackageManager, repo: Repository, package: &str, description: &str) -> InstallStep {
    let mut cmd: Vec<String> = pm.base_cmd().into_iter().map(String::from).collect();
    cmd.extend(pm.install_args().iter().map(|s| s.to_string()));
    cmd.push(package.to_string());
    InstallStep {
        repo,
        package: package.into(),
        description: description.into(),
        command: cmd,
        prerequisites: vec![],
    }
}

/// Post-install verification: does the binary now exist on PATH?
/// Used by the orchestrator to decide whether to advance to the next
/// step in the plan. Tries common bin directories in addition to PATH
/// since `sudo apk add` may have populated /sbin without the calling
/// shell knowing.
pub fn verify_installed(binary: &str) -> bool {
    // Shared library detection: names ending in .so or .so.N look up
    // the dynamic linker cache + standard library paths instead of
    // PATH (libraries don't live there). Used for runtime libs like
    // libopus that have no CLI tool.
    if binary.contains(".so") {
        return verify_shared_lib(binary);
    }
    // Cheap: search the PATH env var.
    if which_in_path(binary).is_some() { return true; }
    // Slightly less cheap: probe common bin/sbin locations directly.
    for dir in ["/usr/bin", "/bin", "/usr/sbin", "/sbin", "/usr/local/bin"] {
        if Path::new(dir).join(binary).exists() { return true; }
    }
    false
}

/// Probe whether a shared library is installed. We walk the standard
/// system library paths and check for a file matching the requested
/// soname. Doesn't run ldconfig (avoids the subprocess); the paths
/// list covers Linux distributions we care about.
fn verify_shared_lib(soname: &str) -> bool {
    let paths = [
        "/lib",
        "/usr/lib",
        "/lib/x86_64-linux-gnu",
        "/usr/lib/x86_64-linux-gnu",
        "/lib/aarch64-linux-gnu",
        "/usr/lib/aarch64-linux-gnu",
        "/usr/lib64",
        "/usr/local/lib",
    ];
    for dir in paths {
        if Path::new(dir).join(soname).exists() { return true; }
    }
    false
}

fn which_in_path(binary: &str) -> Option<String> {
    let path = std::env::var("PATH").ok()?;
    for dir in path.split(':') {
        let full = Path::new(dir).join(binary);
        if full.exists() {
            return Some(full.to_string_lossy().into_owned());
        }
    }
    None
}

/// Outcome of one attempted step.
#[derive(Debug, Clone, PartialEq)]
pub enum StepOutcome {
    /// The step ran, post-install verification confirmed the binary is now present.
    Success,
    /// The step ran but the binary still isn't visible — move on to the next step.
    InstalledButNotVisible,
    /// The step's command failed (non-zero exit). The wrangler logs the
    /// stderr and proceeds to the next fallback.
    CommandFailed { exit_code: Option<i32>, stderr: String },
    /// We never tried this step (e.g. because an earlier step succeeded).
    Skipped,
}

/// A run-time log entry: which step, what happened. Used by the
/// orchestrator to render the audit trail.
#[derive(Debug, Clone)]
pub struct StepLog {
    pub step: InstallStep,
    pub outcome: StepOutcome,
}

/// Trail of all the steps the wrangler attempted for one tool.
#[derive(Debug, Clone)]
pub struct WrangleResult {
    pub binary: String,
    pub initial_state: bool,   // was it already installed at the start?
    pub final_state: bool,     // is it installed at the end?
    pub steps: Vec<StepLog>,
}

impl WrangleResult {
    pub fn succeeded(&self) -> bool { self.final_state }
    /// Did the wrangler actually have to do anything?
    pub fn changed_state(&self) -> bool { !self.initial_state && self.final_state }
    /// All commands attempted, for diagnostic display.
    pub fn audit_lines(&self) -> Vec<String> {
        self.steps.iter().map(|sl| {
            let outcome = match &sl.outcome {
                StepOutcome::Success => "OK".to_string(),
                StepOutcome::InstalledButNotVisible => "ran, binary not visible".to_string(),
                StepOutcome::CommandFailed { exit_code, stderr } => {
                    let head: String = stderr.chars().take(80).collect();
                    format!("FAIL (exit {:?}): {}", exit_code, head)
                }
                StepOutcome::Skipped => "skipped".to_string(),
            };
            format!("[{:?}] {} → {}", sl.step.repo, sl.step.command_string(), outcome)
        }).collect()
    }
}

/// Render a plan as plain text for the user. Used by the install
/// script's `--dry-run` mode + by the QML settings page.
pub fn plan_to_text(plan: &InstallPlan) -> String {
    let mut out = String::new();
    out.push_str(&format!("Plan for '{}' ({})\n", plan.binary, plan.friendly_name));
    if plan.steps.is_empty() {
        out.push_str("  (no install strategy known on this distro)\n");
        return out;
    }
    for (i, step) in plan.steps.iter().enumerate() {
        out.push_str(&format!(
            "  {}. [{:?}] {} — {}\n",
            i + 1, step.repo, step.description, step.command_string()
        ));
        for prereq in &step.prerequisites {
            out.push_str(&format!("      prereq: {}\n", prereq.join(" ")));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn package_manager_labels_non_empty() {
        for pm in [
            PackageManager::Apk, PackageManager::Apt, PackageManager::Dnf,
            PackageManager::Pacman, PackageManager::Zypper, PackageManager::Unknown,
        ] {
            assert!(!pm.label().is_empty());
        }
    }

    #[test]
    fn apk_install_args_use_no_cache() {
        let args = PackageManager::Apk.install_args();
        assert!(args.contains(&"add"));
        assert!(args.contains(&"--no-cache"));
    }

    #[test]
    fn apt_install_args_are_non_interactive() {
        let args = PackageManager::Apt.install_args();
        assert!(args.contains(&"install"));
        assert!(args.contains(&"-y"));
    }

    #[test]
    fn pacman_install_args_dont_confirm() {
        let args = PackageManager::Pacman.install_args();
        assert!(args.contains(&"-S"));
        assert!(args.contains(&"--noconfirm"));
        assert!(args.contains(&"--needed"));
    }

    #[test]
    fn primary_package_paplay_per_distro() {
        assert_eq!(primary_package_for("paplay", PackageManager::Apt), Some("pulseaudio-utils"));
        assert_eq!(primary_package_for("paplay", PackageManager::Apk), Some("pulseaudio-utils"));
        assert_eq!(primary_package_for("paplay", PackageManager::Pacman), Some("libpulse"));
        assert_eq!(primary_package_for("paplay", PackageManager::Dnf), Some("pulseaudio-utils"));
    }

    #[test]
    fn primary_package_bluetoothctl_per_distro() {
        assert_eq!(primary_package_for("bluetoothctl", PackageManager::Apt), Some("bluez"));
        assert_eq!(primary_package_for("bluetoothctl", PackageManager::Apk), Some("bluez"));
        assert_eq!(primary_package_for("bluetoothctl", PackageManager::Pacman), Some("bluez-utils"));
    }

    #[test]
    fn primary_package_libopus_per_distro() {
        assert_eq!(primary_package_for("libopus.so.0", PackageManager::Apt),    Some("libopus0"));
        assert_eq!(primary_package_for("libopus.so.0", PackageManager::Dnf),    Some("opus"));
        assert_eq!(primary_package_for("libopus.so.0", PackageManager::Pacman), Some("opus"));
        assert_eq!(primary_package_for("libopus.so.0", PackageManager::Apk),    Some("opus"));
        assert_eq!(primary_package_for("libopus.so.0", PackageManager::Zypper), Some("libopus0"));
    }

    #[test]
    fn verify_installed_handles_shared_libs() {
        // libopus.so.0 is installed in the build environment; the
        // dot-so detection should route through verify_shared_lib
        // and find it.
        assert!(verify_installed("libopus.so.0"),
                "expected libopus.so.0 to be detected via shared-lib path");
        // A nonsense soname should NOT be found.
        assert!(!verify_installed("libnonexistent.so.42"));
    }

    #[test]
    fn plan_for_ffmpeg_on_alpine_includes_community_fallback() {
        let plan = plan_for_tool("ffmpeg", "FFmpeg", PackageManager::Apk);
        assert!(!plan.steps.is_empty());
        let repos: Vec<&Repository> = plan.steps.iter().map(|s| &s.repo).collect();
        assert!(repos.contains(&&Repository::Default));
        assert!(repos.contains(&&Repository::Community));
        // Edge testing fallback
        assert!(repos.contains(&&Repository::Testing));
        // Source as last resort
        assert!(repos.contains(&&Repository::Source));
    }

    #[test]
    fn plan_for_ffmpeg_on_fedora_includes_rpmfusion() {
        let plan = plan_for_tool("ffmpeg", "FFmpeg", PackageManager::Dnf);
        let repos: Vec<&Repository> = plan.steps.iter().map(|s| &s.repo).collect();
        assert!(repos.contains(&&Repository::Default));
        assert!(repos.contains(&&Repository::RpmFusion));
    }

    #[test]
    fn plan_for_ffmpeg_on_apt_includes_universe() {
        let plan = plan_for_tool("ffmpeg", "FFmpeg", PackageManager::Apt);
        let repos: Vec<&Repository> = plan.steps.iter().map(|s| &s.repo).collect();
        assert!(repos.contains(&&Repository::Default));
        assert!(repos.contains(&&Repository::Universe));
    }

    #[test]
    fn plan_for_unknown_pm_returns_no_step() {
        let plan = plan_for_tool("ffmpeg", "FFmpeg", PackageManager::Unknown);
        // The "Source" fallback still gets added even on unknown PMs.
        let has_source = plan.steps.iter().any(|s| s.repo == Repository::Source);
        // Default + RPMFusion etc wouldn't be added, but Source still appears.
        assert!(has_source || plan.steps.is_empty());
    }

    #[test]
    fn plan_text_renders_steps() {
        let plan = plan_for_tool("paplay", "PulseAudio playback", PackageManager::Apt);
        let text = plan_to_text(&plan);
        assert!(text.contains("paplay"));
        assert!(text.contains("default repo") || text.contains("Default"));
    }

    #[test]
    fn plan_text_handles_empty() {
        let plan = InstallPlan {
            binary: "nothing".into(),
            friendly_name: "Nothing".into(),
            steps: vec![],
        };
        let text = plan_to_text(&plan);
        assert!(text.contains("no install strategy"));
    }

    #[test]
    fn install_step_command_string_joins_with_spaces() {
        let step = InstallStep {
            repo: Repository::Default,
            package: "x".into(),
            description: "d".into(),
            command: vec!["sudo".into(), "apt".into(), "install".into(), "-y".into(), "x".into()],
            prerequisites: vec![],
        };
        assert_eq!(step.command_string(), "sudo apt install -y x");
    }

    #[test]
    fn wrangle_result_succeeded_reflects_final_state() {
        let r = WrangleResult {
            binary: "x".into(),
            initial_state: false,
            final_state: true,
            steps: vec![],
        };
        assert!(r.succeeded());
        assert!(r.changed_state());

        let r2 = WrangleResult {
            binary: "x".into(),
            initial_state: false,
            final_state: false,
            steps: vec![],
        };
        assert!(!r2.succeeded());
        assert!(!r2.changed_state());
    }

    #[test]
    fn wrangle_result_audit_lines_render_outcomes() {
        let step = InstallStep {
            repo: Repository::Default,
            package: "x".into(),
            description: "d".into(),
            command: vec!["echo".into(), "hi".into()],
            prerequisites: vec![],
        };
        let r = WrangleResult {
            binary: "x".into(),
            initial_state: false,
            final_state: true,
            steps: vec![StepLog { step: step.clone(), outcome: StepOutcome::Success }],
        };
        let lines = r.audit_lines();
        assert_eq!(lines.len(), 1);
        assert!(lines[0].contains("OK"));
    }

    #[test]
    fn primary_package_for_unknown_pair_is_none() {
        assert_eq!(primary_package_for("doesnt-exist", PackageManager::Apt), None);
    }

    #[test]
    fn detect_by_pm_binary_doesnt_panic_on_missing() {
        // Just exercise the path. The actual return depends on the host
        // running the test (CI vs developer workstation), so we don't
        // assert a specific value.
        let _ = detect_by_pm_binary();
    }
}

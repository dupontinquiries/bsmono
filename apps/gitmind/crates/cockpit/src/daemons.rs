//! daemons — the fleet's background services, in one place.
//!
//! gitmind had no single registry of the daemons (voiced / sharemd / coordd / settingsd), so
//! adding settingsd meant nothing knew to install or start it. This module is the source of
//! truth: each [`Daemon`]'s binary + ipc-kit socket, plus service-unit generation for both init
//! systems in the fleet — **systemd** (Fedora desktop) and **OpenRC** (Alpine / postmarketOS
//! phone) — and an install script that enables them for whichever the target runs.

/// A background service in the fleet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Daemon {
    /// Service + binary name.
    pub name: &'static str,
    /// ipc-kit socket namespace it serves.
    pub socket: &'static str,
    /// One-line description.
    pub description: &'static str,
    /// The core-kit this first-party daemon integrates. This is the developer marker: it's embedded in
    /// the generated unit (see [`FLEET_MARKER`]) so a system scan can auto-discover fleet daemons and
    /// tell them apart from third-party services that carry no integration.
    pub integration: &'static str,
}

/// The unit key that marks a service as a first-party fleet daemon. Every generated unit carries it;
/// a scan greps for it to separate developer daemons (with integrations) from other services.
pub const FLEET_MARKER: &str = "X-Fleet-Integration";

/// Extract the fleet integration (core-kit) from a unit body, if it carries [`FLEET_MARKER`]. Handles
/// both the systemd form `X-Fleet-Integration=voiced-core` and the OpenRC comment form
/// `# X-Fleet-Integration: voiced-core`. Pure — the basis of on-system auto-detection.
pub fn fleet_integration(unit_body: &str) -> Option<String> {
    for line in unit_body.lines() {
        let t = line.trim_start_matches(['#', ' ', '\t']);
        if let Some(rest) = t.strip_prefix(FLEET_MARKER) {
            let v = rest.trim_start_matches([' ', '=', ':']).trim();
            if !v.is_empty() {
                return Some(v.to_string());
            }
        }
    }
    None
}

/// Whether a unit body is a first-party fleet daemon (carries the developer marker).
pub fn is_fleet_daemon(unit_body: &str) -> bool {
    fleet_integration(unit_body).is_some()
}

/// Every daemon gitmind manages. Adding one here is all that's needed for install/start to pick
/// it up.
pub fn all() -> &'static [Daemon] {
    &[
        Daemon { name: "voiced", socket: "voice", description: "voice intent routing (voiced-core: registry + matcher)", integration: "voiced-core" },
        Daemon { name: "sharemd", socket: "share", description: "shared-data hub (datashare-core)", integration: "datashare-core" },
        Daemon { name: "coordd", socket: "coord", description: "coordination arbiter (coord-core)", integration: "coord-core" },
        Daemon { name: "settingsd", socket: "settings", description: "cross-app settings, persisted to ~/.config/gitmind (settings-kit)", integration: "settings-kit" },
        Daemon { name: "logscope-fabricd", socket: "fabric", description: "logscope distributed fanout — broadcaster discovery + remote worker dispatch (fuzz/summarize across devices)", integration: "logscope-fabric" },
    ]
}

/// Look up a daemon by name.
pub fn find(name: &str) -> Option<&'static Daemon> {
    all().iter().find(|d| d.name == name)
}

/// Which init system the target uses.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitSystem {
    /// Fedora desktop, Plasma, most systemd distros — user services.
    Systemd,
    /// Alpine / postmarketOS — OpenRC.
    OpenRc,
}

impl InitSystem {
    pub fn as_str(self) -> &'static str {
        match self {
            InitSystem::Systemd => "systemd",
            InitSystem::OpenRc => "openrc",
        }
    }
    /// Detect from an `os-release`-style body (ID / ID_LIKE). Alpine → OpenRC, else systemd.
    pub fn detect_from_os_release(body: &str) -> InitSystem {
        let b = body.to_lowercase();
        if b.contains("alpine") || b.contains("postmarketos") {
            InitSystem::OpenRc
        } else {
            InitSystem::Systemd
        }
    }
}

/// A systemd **user** unit for a daemon (installed under ~/.config/systemd/user).
pub fn systemd_user_unit(d: &Daemon, exec_path: &str) -> String {
    format!(
        "[Unit]\n\
         Description={desc}\n\
         X-Fleet-Integration={integration}\n\
         After=default.target\n\
         \n\
         [Service]\n\
         ExecStart={exec}\n\
         Restart=on-failure\n\
         RestartSec=2\n\
         \n\
         [Install]\n\
         WantedBy=default.target\n",
        desc = d.description,
        integration = d.integration,
        exec = exec_path,
    )
}

/// An OpenRC service script (installed under /etc/init.d, runs in the background).
pub fn openrc_service(d: &Daemon, exec_path: &str) -> String {
    format!(
        "#!/sbin/openrc-run\n\
         # X-Fleet-Integration: {integration}\n\
         name=\"{name}\"\n\
         description=\"{desc}\"\n\
         command=\"{exec}\"\n\
         command_background=true\n\
         pidfile=\"/run/{name}.pid\"\n\
         output_log=\"/var/log/{name}.log\"\n\
         error_log=\"/var/log/{name}.log\"\n\
         \n\
         depend() {{\n\
         \tneed net\n\
         }}\n",
        name = d.name,
        desc = d.description,
        integration = d.integration,
        exec = exec_path,
    )
}

/// A daemon this fleet knows about, from either source.
///
/// Owned, unlike [`Daemon`], because the manifest-discovered ones cannot be
/// `&'static str`. The static registry stays as the richer description — it
/// carries the socket namespace and the core-kit integration, which the
/// manifest does not know — and discovery fills in the rest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct KnownDaemon {
    pub name: String,
    pub description: String,
    pub integration: String,
    /// The project whose tree it ships from, as the manifest names it.
    pub project: String,
    /// True when it came from [`all`] rather than from a unit file on disk.
    pub registered: bool,
}

/// Service names a project ships, from its `systemd/` directory listing.
///
/// `.socket` files are EXCLUDED. A socket unit is an activation mechanism for
/// a service of the same name, not a second daemon — `sharemd.socket` beside
/// `sharemd.service` is one daemon, and counting it twice would offer the
/// operator two buttons that install the same thing and a `rc-update add
/// sharemd.socket` that OpenRC cannot satisfy at all.
///
/// Pure over a listing so the rule is testable without a filesystem.
pub fn daemon_names_from_units(file_names: &[String]) -> Vec<String> {
    let mut out: Vec<String> = file_names
        .iter()
        .filter_map(|f| f.strip_suffix(".service"))
        .map(|n| n.to_string())
        .collect();
    out.sort();
    out.dedup();
    out
}

/// Merge the static registry with what the manifest's `daemon = true` projects
/// actually ship.
///
/// `discovered` is `(project, service names)` — the caller lists each daemon
/// project's `systemd/` directory, which is the shell around this.
///
/// # Why both sources instead of one
///
/// The registry knows things the filesystem cannot: which core-kit a service
/// integrates and which ipc-kit socket namespace it serves. The filesystem
/// knows things the registry has not been told: `snapclone-bus`,
/// `snapclone-camera-bridge` and `photosync` all ship units and none of them
/// is in [`all`], which is C14_TODO §17's finding — two lists describing one
/// fleet with nothing keeping them in step.
///
/// So the registry wins where it has an entry, and discovery adds the rest
/// rather than replacing it. A discovered daemon gets an empty `integration`,
/// which is honest: it has not declared one, and `is_fleet_daemon` will say so
/// of its hand-written unit too.
pub fn known(discovered: &[(String, Vec<String>)]) -> Vec<KnownDaemon> {
    let mut out: Vec<KnownDaemon> = all()
        .iter()
        .map(|d| KnownDaemon {
            name: d.name.to_string(),
            description: d.description.to_string(),
            integration: d.integration.to_string(),
            project: d.name.to_string(),
            registered: true,
        })
        .collect();
    for (project, services) in discovered {
        for svc in services {
            if out.iter().any(|k| &k.name == svc) {
                continue;
            }
            out.push(KnownDaemon {
                name: svc.clone(),
                description: format!("{svc} (from {project}/systemd)"),
                integration: String::new(),
                project: project.clone(),
                registered: false,
            });
        }
    }
    out
}

/// List the units each daemon project ships, ready for [`known`].
///
/// `projects` is `(id, path)` for every manifest entry with `daemon = true` —
/// `appcatalog::parse_manifest` supplies both fields, so the caller filters
/// rather than this module depending on the catalogue.
///
/// The thin shell around [`daemon_names_from_units`]: a `read_dir` each, and
/// nothing else. A project with no `systemd/` directory yields an empty list
/// rather than an error — `bo-suite` is `daemon = true` with no units checked
/// in, and that is a fact about the project, not a failure of the scan.
pub fn discover_units(root: &std::path::Path, projects: &[(String, String)])
    -> Vec<(String, Vec<String>)>
{
    let mut out = Vec::new();
    for (id, rel) in projects {
        let dir = root.join(rel).join("systemd");
        let names: Vec<String> = match std::fs::read_dir(&dir) {
            Ok(rd) => rd
                .filter_map(|e| e.ok())
                .map(|e| e.file_name().to_string_lossy().into_owned())
                .collect(),
            Err(_) => Vec::new(),
        };
        out.push((id.clone(), daemon_names_from_units(&names)));
    }
    out
}

/// Everything needed to put a daemon on a machine: one file, then some commands.
///
/// Returned as DATA rather than executed, so the same plan drives a local
/// install, an `ssh` one and a dry run — and so the whole of it is testable
/// without a machine to install onto. `deploy.rs` already knows how to run a
/// remote command and push a file; this says what to push and what to run.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct InstallPlan {
    /// Where the unit or init script goes.
    pub unit_path: String,
    pub unit_body: String,
    /// Whether the file needs to be executable — an OpenRC script does, a
    /// systemd unit does not, and getting it wrong fails at boot rather than
    /// at install.
    pub executable: bool,
    /// Run in order, after the file is in place.
    pub commands: Vec<String>,
}

/// Build the install plan for `d`, whose binary will live at `exec_path`.
///
/// # Why generate the unit instead of shipping one
///
/// Because a checked-in unit drifts from the machine it lands on, and today's
/// rusty-board failure is the example: its hand-written unit declared
/// `ReadWritePaths=%h/.config/rusty-board`, nothing created that directory, and
/// systemd refused to set up the mount namespace and died before `ExecStart`
/// with a message naming the directory rather than the cause.
///
/// A generated unit cannot have that problem, because the generator knows what
/// it asked for. It also guarantees `X-Fleet-Integration` is present — the
/// marker `is_fleet_daemon` scans for — and that `ExecStart` matches where the
/// binary actually landed rather than where someone assumed it would.
///
/// # The order of the commands is the part that matters
///
/// Stop, then reload, then enable, then start. Each step is there for a
/// failure:
///
/// * **Stop first** — replacing the binary under a running service leaves the
///   old one in memory holding its socket, so the new one fails to bind and
///   the operator sees a crash loop rather than an upgrade.
/// * **Reload between** — systemd caches unit text. Without it, `start` runs
///   the OLD unit and the change appears not to have happened, which is
///   exactly what "I ran the redeploy script and nothing changed" looks like.
/// * **Enable before start**, so a device that reboots comes back with the
///   daemon running. A service that works until the phone restarts is a bug
///   report a week later with no obvious cause.
pub fn install_plan(d: &Daemon, init: InitSystem, exec_path: &str, home: &str) -> InstallPlan {
    match init {
        InitSystem::Systemd => InstallPlan {
            unit_path: format!("{home}/.config/systemd/user/{}.service", d.name),
            unit_body: systemd_user_unit(d, exec_path),
            executable: false,
            commands: vec![
                // `|| true` on the stop: a first install has nothing to stop,
                // and a non-zero exit there must not abort the install.
                format!("systemctl --user stop {}.service || true", d.name),
                "systemctl --user daemon-reload".to_string(),
                format!("systemctl --user enable {}.service", d.name),
                format!("systemctl --user start {}.service", d.name),
            ],
        },
        InitSystem::OpenRc => InstallPlan {
            unit_path: format!("/etc/init.d/{}", d.name),
            unit_body: openrc_service(d, exec_path),
            // OpenRC EXECUTES the script. A non-executable one fails with
            // "service does not exist", which sends the reader looking for a
            // missing file that is right there.
            executable: true,
            commands: vec![
                format!("rc-service {} stop || true", d.name),
                format!("rc-update add {} default", d.name),
                format!("rc-service {} start", d.name),
            ],
        },
    }
}

/// A lifecycle action on a running daemon.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DaemonAction {
    Start,
    Stop,
    Restart,
}

impl DaemonAction {
    pub fn as_str(self) -> &'static str {
        match self {
            DaemonAction::Start => "start",
            DaemonAction::Stop => "stop",
            DaemonAction::Restart => "restart",
        }
    }
    /// Parse from a UI string; unknown → None.
    pub fn parse(s: &str) -> Option<DaemonAction> {
        match s {
            "start" => Some(DaemonAction::Start),
            "stop" => Some(DaemonAction::Stop),
            "restart" => Some(DaemonAction::Restart),
            _ => None,
        }
    }
}

/// The command to start/stop/restart a daemon now, for the given init system. (`start`, `stop`,
/// and `restart` are all valid verbs for both `systemctl --user` and `rc-service`.)
pub fn control_cmd(d: &Daemon, init: InitSystem, action: DaemonAction) -> String {
    let a = action.as_str();
    match init {
        InitSystem::Systemd => format!("systemctl --user {a} {}.service", d.name),
        InitSystem::OpenRc => format!("rc-service {} {a}", d.name),
    }
}

/// The command to fetch a daemon's most recent `lines` of logs. systemd reads the user journal;
/// OpenRC tails the service's `output_log` (see `openrc_service`), with a friendly note if the
/// daemon predates the log path or logs to syslog instead.
pub fn logs_cmd(d: &Daemon, init: InitSystem, lines: u32) -> String {
    match init {
        InitSystem::Systemd => {
            format!("journalctl --user -u {}.service -n {lines} --no-pager", d.name)
        }
        InitSystem::OpenRc => format!(
            "tail -n {lines} /var/log/{name}.log 2>/dev/null || echo '(no log file yet for {name})'",
            name = d.name
        ),
    }
}

/// The command to *follow* a daemon's logs live (for a streaming view). systemd follows the user
/// journal; OpenRC `tail -F`s the service log (capital -F re-opens across rotation). Starts with a
/// small backlog so the view isn't empty.
pub fn follow_cmd(d: &Daemon, init: InitSystem) -> String {
    match init {
        InitSystem::Systemd => format!("journalctl --user -u {}.service -n 50 -f --no-pager", d.name),
        InitSystem::OpenRc => format!("tail -n 50 -F /var/log/{}.log", d.name),
    }
}

/// Parse a per-daemon status block (as produced by `daemons_status_remote`: one `name: state` per
/// line, where state is systemd's `active`/`inactive` or OpenRC's `started`/`stopped`) into
/// `(daemon, up)` pairs. Pure, so the fleet-health rollup is testable without a network.
pub fn parse_status(output: &str) -> Vec<(String, bool)> {
    output
        .lines()
        .filter_map(|l| {
            let (name, state) = l.split_once(':')?;
            let name = name.trim();
            if name.is_empty() {
                return None;
            }
            let up = matches!(state.trim(), "active" | "started" | "running");
            Some((name.to_string(), up))
        })
        .collect()
}

/// The unit file path a daemon's service lands at, for the given init system.
pub fn unit_path(d: &Daemon, init: InitSystem, home: &str) -> String {
    match init {
        InitSystem::Systemd => format!("{home}/.config/systemd/user/{}.service", d.name),
        InitSystem::OpenRc => format!("/etc/init.d/{}", d.name),
    }
}

/// The command to enable + start a daemon now, for the given init system.
pub fn enable_cmd(d: &Daemon, init: InitSystem) -> String {
    match init {
        InitSystem::Systemd => format!("systemctl --user enable --now {}.service", d.name),
        InitSystem::OpenRc => format!("rc-update add {name} default && rc-service {name} start", name = d.name),
    }
}

/// A shell script that installs + enables every daemon whose binary is present in `bin_dir`,
/// writing the right unit for `init` and enabling it. Skips daemons whose binary is missing so a
/// partial deploy still works (graceful).
pub fn install_daemons_script(bin_dir: &str, home: &str, init: InitSystem) -> String {
    let mut s = String::from("#!/bin/sh\nset -e\n");
    s.push_str(&format!("# install + enable gitmind daemons ({})\n", init.as_str()));
    if init == InitSystem::Systemd {
        s.push_str(&format!("mkdir -p {home}/.config/systemd/user\n"));
    }
    for d in all() {
        let exec = format!("{bin_dir}/{}", d.name);
        let unit = unit_path(d, init, home);
        let body = match init {
            InitSystem::Systemd => systemd_user_unit(d, &exec),
            InitSystem::OpenRc => openrc_service(d, &exec),
        };
        s.push_str(&format!("if [ -x \"{exec}\" ]; then\n"));
        s.push_str(&format!("  cat > \"{unit}\" <<'UNIT'\n{body}UNIT\n"));
        if init == InitSystem::OpenRc {
            s.push_str(&format!("  chmod +x \"{unit}\"\n"));
        }
        s.push_str(&format!("  {}\n", enable_cmd(d, init)));
        s.push_str(&format!("  echo \"  ✓ {} ({})\"\n", d.name, d.socket));
        s.push_str(&format!("else echo \"  – skip {} (binary not installed)\"; fi\n", d.name));
    }
    if init == InitSystem::Systemd {
        s.push_str("systemctl --user daemon-reload || true\n");
    }
    s
}

#[cfg(test)]
mod tests {

    /// The distinction the page was missing: start/stop/restart last until a reboot, and
    /// `enable` is what survives one. Someone who fixed a dead daemon and pocketed the phone
    /// found it dead again, with nothing having suggested there was a difference.
    #[test]
    fn enable_starts_now_and_at_boot_on_both_init_systems() {
        let d = &all()[0];
        let sd = enable_cmd(d, InitSystem::Systemd);
        // ONE command, not enable-then-start: two round trips can half-succeed and leave a
        // daemon enabled and down, which is the state hardest to notice.
        assert!(sd.contains("enable"), "{sd}");
        assert!(sd.contains("--now"), "systemd should start it too: {sd}");

        let rc = enable_cmd(d, InitSystem::OpenRc);
        assert!(rc.contains("rc-update add"), "{rc}");
        assert!(rc.contains("rc-service"), "OpenRC needs the start as well: {rc}");
        assert!(rc.contains("&&"), "the start must be conditional on the add: {rc}");

        // And it is a DIFFERENT command from a plain start, or the button would be a
        // duplicate of one already there.
        assert_ne!(sd, control_cmd(d, InitSystem::Systemd, DaemonAction::Start));
        assert_ne!(rc, control_cmd(d, InitSystem::OpenRc, DaemonAction::Start));
    }
    use super::*;

    #[test]
    fn registry_includes_all_fleet_daemons() {
        let names: Vec<&str> = all().iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["voiced", "sharemd", "coordd", "settingsd", "logscope-fabricd"]);
        assert_eq!(find("settingsd").unwrap().socket, "settings");
        assert_eq!(find("logscope-fabricd").unwrap().socket, "fabric");
        assert!(find("nope").is_none());
    }

    #[test]
    fn init_system_detection() {
        assert_eq!(InitSystem::detect_from_os_release("ID=alpine"), InitSystem::OpenRc);
        assert_eq!(InitSystem::detect_from_os_release("ID=postmarketos\nID_LIKE=alpine"), InitSystem::OpenRc);
        assert_eq!(InitSystem::detect_from_os_release("ID=fedora"), InitSystem::Systemd);
    }

    #[test]
    fn fleet_marker_round_trips_and_detects_third_party() {
        // generated units carry the developer marker → auto-detectable as fleet daemons
        let d = find("voiced").unwrap();
        let sysd = systemd_user_unit(d, "/usr/bin/voiced");
        assert_eq!(fleet_integration(&sysd).as_deref(), Some("voiced-core"));
        assert!(is_fleet_daemon(&sysd));
        let rc = openrc_service(d, "/usr/bin/voiced");
        assert_eq!(fleet_integration(&rc).as_deref(), Some("voiced-core")); // OpenRC comment form
        // a plain third-party unit carries no marker
        let other = "[Unit]\nDescription=some other service\n[Service]\nExecStart=/usr/bin/other\n";
        assert_eq!(fleet_integration(other), None);
        assert!(!is_fleet_daemon(other));
    }

    #[test]
    fn every_fleet_daemon_declares_an_integration() {
        for d in all() {
            assert!(!d.integration.is_empty(), "{} needs an integration", d.name);
        }
    }

    #[test]
    fn systemd_unit_has_execstart_and_install() {
        let d = find("settingsd").unwrap();
        let u = systemd_user_unit(d, "/home/user/.local/bin/settingsd");
        assert!(u.contains("ExecStart=/home/user/.local/bin/settingsd"));
        assert!(u.contains("[Install]") && u.contains("WantedBy=default.target"));
        assert!(u.contains("Restart=on-failure"));
    }

    #[test]
    fn openrc_service_has_command_and_background() {
        let d = find("voiced").unwrap();
        let s = openrc_service(d, "/usr/local/bin/voiced");
        assert!(s.contains("command=\"/usr/local/bin/voiced\""));
        assert!(s.contains("command_background=true"));
        assert!(s.starts_with("#!/sbin/openrc-run"));
    }

    #[test]
    fn install_script_covers_daemons_and_is_graceful() {
        let sh = install_daemons_script("/home/user/.local/bin", "/home/user", InitSystem::Systemd);
        // every daemon referenced
        for d in all() {
            assert!(sh.contains(d.name), "script mentions {}", d.name);
        }
        // graceful: only installs present binaries
        assert!(sh.contains("if [ -x"));
        assert!(sh.contains("skip"));
        // openrc variant chmods the init script
        let rc = install_daemons_script("/usr/local/bin", "/root", InitSystem::OpenRc);
        assert!(rc.contains("chmod +x") && rc.contains("rc-update add"));
    }

    #[test]
    fn control_cmd_is_init_aware() {
        let d = &all()[0];
        assert_eq!(
            control_cmd(d, InitSystem::Systemd, DaemonAction::Restart),
            format!("systemctl --user restart {}.service", d.name)
        );
        assert_eq!(
            control_cmd(d, InitSystem::OpenRc, DaemonAction::Stop),
            format!("rc-service {} stop", d.name)
        );
        // action round-trips through parse/as_str
        for a in [DaemonAction::Start, DaemonAction::Stop, DaemonAction::Restart] {
            assert_eq!(DaemonAction::parse(a.as_str()), Some(a));
        }
        assert_eq!(DaemonAction::parse("frobnicate"), None);
    }

    #[test]
    fn logs_cmd_reads_journal_or_tails_log() {
        let d = &all()[0];
        let sd = logs_cmd(d, InitSystem::Systemd, 200);
        assert!(sd.contains("journalctl --user -u") && sd.contains("-n 200"));
        let rc = logs_cmd(d, InitSystem::OpenRc, 200);
        assert!(rc.contains(&format!("/var/log/{}.log", d.name)) && rc.contains("tail -n 200"));
        // the openrc service now writes to that same path so the tail has something to read
        assert!(openrc_service(d, "/usr/local/bin/x").contains(&format!("output_log=\"/var/log/{}.log\"", d.name)));
    }

    #[test]
    fn follow_cmd_follows_live() {
        let d = &all()[0];
        assert!(follow_cmd(d, InitSystem::Systemd).contains("-f") && follow_cmd(d, InitSystem::Systemd).contains("journalctl --user"));
        assert!(follow_cmd(d, InitSystem::OpenRc).contains("tail -n 50 -F"));
    }

    #[test]
    fn parse_status_handles_both_init_vocabularies() {
        let systemd = "voiced: active\nsharemd: inactive\ncoordd: active\n";
        let got = parse_status(systemd);
        assert_eq!(got, vec![("voiced".into(), true), ("sharemd".into(), false), ("coordd".into(), true)]);
        // openrc vocabulary + stray blank lines
        let openrc = "voiced: started\n\nsettingsd: stopped\n";
        let got = parse_status(openrc);
        assert_eq!(got, vec![("voiced".into(), true), ("settingsd".into(), false)]);
    }
    #[test]
    fn the_plan_reloads_before_it_starts() {
        // systemd caches unit text. Start without a reload runs the OLD unit
        // and the change appears not to have happened — which is exactly what
        // "I ran the redeploy script and nothing changed" looks like from the
        // outside.
        let d = &all()[0];
        let p = install_plan(d, InitSystem::Systemd, "/home/u/.local/bin/voiced", "/home/u");
        let idx = |needle: &str| p.commands.iter().position(|c| c.contains(needle));
        let (stop, reload, enable, start) = (
            idx("stop").expect("no stop"),
            idx("daemon-reload").expect("no reload"),
            idx("enable").expect("no enable"),
            idx("start").expect("no start"),
        );
        assert!(stop < reload, "the binary is replaced under a running service");
        assert!(reload < start, "start would run the cached unit");
        assert!(enable < start, "a reboot would come back without the daemon");
    }

    #[test]
    fn a_first_install_is_not_aborted_by_having_nothing_to_stop() {
        // The stop runs before anything exists. Without the guard a fresh
        // device fails at step one.
        let d = &all()[0];
        let p = install_plan(d, InitSystem::Systemd, "/x", "/home/u");
        assert!(p.commands[0].contains("|| true"), "first command: {}", p.commands[0]);
        let o = install_plan(d, InitSystem::OpenRc, "/x", "/home/u");
        assert!(o.commands[0].contains("|| true"), "first command: {}", o.commands[0]);
    }

    #[test]
    fn the_openrc_script_is_executable_and_the_systemd_unit_is_not() {
        // OpenRC EXECUTES the script; a non-executable one reports "service
        // does not exist", which sends the reader looking for a file that is
        // right there. systemd reads its unit and does not care.
        let d = &all()[0];
        assert!(install_plan(d, InitSystem::OpenRc, "/x", "/h").executable);
        assert!(!install_plan(d, InitSystem::Systemd, "/x", "/h").executable);
    }

    #[test]
    fn every_generated_unit_carries_the_fleet_marker_and_the_real_exec_path() {
        // The marker is what `is_fleet_daemon` scans for, and the exec path is
        // the thing a hand-written unit gets wrong when a binary moves.
        for init in [InitSystem::Systemd, InitSystem::OpenRc] {
            for d in all() {
                let p = install_plan(d, init, "/opt/fleet/bin/thing", "/home/u");
                assert!(is_fleet_daemon(&p.unit_body),
                        "{} on {:?} has no {FLEET_MARKER}", d.name, init.as_str());
                assert_eq!(fleet_integration(&p.unit_body).as_deref(), Some(d.integration));
                assert!(p.unit_body.contains("/opt/fleet/bin/thing"),
                        "{} on {:?} does not reference the exec path", d.name, init.as_str());
            }
        }
    }

    #[test]
    fn the_two_init_systems_write_to_different_places() {
        // A systemd unit under /etc/init.d does nothing, and an OpenRC script
        // in ~/.config/systemd/user does nothing. Both silently.
        let d = &all()[0];
        let s = install_plan(d, InitSystem::Systemd, "/x", "/home/u");
        let o = install_plan(d, InitSystem::OpenRc, "/x", "/home/u");
        assert!(s.unit_path.starts_with("/home/u/.config/systemd/user/"), "{}", s.unit_path);
        assert!(s.unit_path.ends_with(".service"));
        assert!(o.unit_path.starts_with("/etc/init.d/"), "{}", o.unit_path);
        assert_ne!(s.unit_path, o.unit_path);
    }

    #[test]
    fn a_socket_unit_is_not_a_second_daemon() {
        // `sharemd.socket` beside `sharemd.service` is ONE daemon. Counting it
        // twice offers two buttons that install the same thing, and
        // `rc-update add sharemd.socket` is something OpenRC cannot satisfy at
        // all.
        let files: Vec<String> = ["sharemd.service", "sharemd.socket", "voiced.socket", "voiced.service"]
            .iter().map(|s| s.to_string()).collect();
        assert_eq!(daemon_names_from_units(&files), vec!["sharemd", "voiced"]);
        // And a directory of nothing but sockets yields no daemons rather than
        // names with the wrong suffix stripped.
        let only_sockets: Vec<String> = vec!["x.socket".into()];
        assert!(daemon_names_from_units(&only_sockets).is_empty());
        // Duplicates collapse. A directory listed twice — which a caller
        // merging two search paths will do — must not offer the same daemon
        // twice; found by breaking the `dedup` and watching nothing fail.
        let dupes: Vec<String> = vec!["voiced.service".into(), "voiced.service".into()];
        assert_eq!(daemon_names_from_units(&dupes), vec!["voiced"]);
    }

    #[test]
    fn discovery_finds_the_daemons_the_registry_never_knew() {
        // The whole point (C14_TODO §17): `all()` lists five fabric services
        // and knows about none of the three that actually ship unit files.
        let discovered = vec![
            ("snapclone".to_string(),
             vec!["snapclone-bus".to_string(), "snapclone-camera-bridge".to_string()]),
            ("photosync".to_string(), vec!["photosync".to_string()]),
        ];
        let k = known(&discovered);
        let names: Vec<&str> = k.iter().map(|d| d.name.as_str()).collect();
        for want in ["snapclone-bus", "snapclone-camera-bridge", "photosync"] {
            assert!(names.contains(&want), "{want} was not discovered: {names:?}");
        }
        // The registry's own entries survive alongside them.
        assert!(names.contains(&"voiced"));
        assert_eq!(k.len(), all().len() + 3);
    }

    #[test]
    fn the_registry_wins_where_it_has_an_entry() {
        // It knows the socket namespace and the core-kit integration; the
        // filesystem does not. Discovery must not overwrite a described daemon
        // with a bare one.
        let discovered = vec![("voiced".to_string(), vec!["voiced".to_string()])];
        let k = known(&discovered);
        let v: Vec<&KnownDaemon> = k.iter().filter(|d| d.name == "voiced").collect();
        assert_eq!(v.len(), 1, "voiced appeared twice");
        assert!(v[0].registered);
        assert_eq!(v[0].integration, "voiced-core", "the described entry was replaced");
    }

    #[test]
    fn a_discovered_daemon_admits_it_has_no_integration() {
        // Rather than inventing one. `is_fleet_daemon` will say the same of its
        // hand-written unit, so claiming an integration here would make the two
        // disagree.
        let discovered = vec![("snapclone".to_string(), vec!["snapclone-bus".to_string()])];
        let k = known(&discovered);
        let d = k.iter().find(|d| d.name == "snapclone-bus").unwrap();
        assert!(d.integration.is_empty());
        assert!(!d.registered);
        assert_eq!(d.project, "snapclone");
    }

    #[test]
    fn discovery_reads_the_real_tree_and_finds_the_units_that_exist() {
        // Against the real repository, like the manifest test in appcatalog and
        // for the same reason: the value of this function is that it agrees
        // with what is on disk, and a fixture would let the two drift while the
        // test stayed green.
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../.."));
        let projects = vec![
            ("snapclone".to_string(), "apps/snapclone".to_string()),
            ("photosync".to_string(), "system/photosync".to_string()),
            ("voiced".to_string(), "apps/voiced".to_string()),
            // `daemon = true` with NO systemd directory. Must be empty, not an
            // error — a fact about the project, not a failure of the scan.
            ("bo-suite".to_string(), "apps/bo-suite".to_string()),
        ];
        let found = discover_units(root, &projects);
        let of = |id: &str| found.iter().find(|(p, _)| p == id).map(|(_, v)| v.clone()).unwrap();

        assert_eq!(of("snapclone"), vec!["snapclone-bus", "snapclone-camera-bridge"]);
        assert_eq!(of("photosync"), vec!["photosync"]);
        // voiced ships a .socket beside its .service; only one daemon.
        assert_eq!(of("voiced"), vec!["voiced"]);
        assert!(of("bo-suite").is_empty(), "a project with no systemd/ was not empty");

        // A missing project directory is also empty rather than a panic — the
        // manifest can name a path before the tree exists.
        let gone = discover_units(root, &[("ghost".into(), "apps/does-not-exist".into())]);
        assert_eq!(gone, vec![("ghost".to_string(), Vec::<String>::new())]);
    }

    #[test]
    fn the_real_tree_feeds_known_and_the_registry_gap_closes() {
        // End to end over the actual repository: this is the assertion that
        // C14_TODO §17's finding is fixed, and it will fail again if someone
        // adds a daemon project without units or removes the discovery.
        let root = std::path::Path::new(concat!(env!("CARGO_MANIFEST_DIR"), "/../../../.."));
        let projects = vec![
            ("snapclone".to_string(), "apps/snapclone".to_string()),
            ("photosync".to_string(), "system/photosync".to_string()),
        ];
        let k = known(&discover_units(root, &projects));
        let names: Vec<&str> = k.iter().map(|d| d.name.as_str()).collect();
        for want in ["snapclone-bus", "snapclone-camera-bridge", "photosync"] {
            assert!(names.contains(&want), "{want} still invisible: {names:?}");
        }
        assert!(names.contains(&"voiced"), "the registry entries were lost");
    }

}

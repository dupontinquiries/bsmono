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
}

/// Every daemon gitmind manages. Adding one here is all that's needed for install/start to pick
/// it up.
pub fn all() -> &'static [Daemon] {
    &[
        Daemon { name: "voiced", socket: "voice", description: "voice intent routing (voiced-core: registry + matcher)" },
        Daemon { name: "sharemd", socket: "share", description: "shared-data hub (datashare-core)" },
        Daemon { name: "coordd", socket: "coord", description: "coordination arbiter (coord-core)" },
        Daemon { name: "settingsd", socket: "settings", description: "cross-app settings, persisted to ~/.config/gitmind (settings-kit)" },
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
        exec = exec_path,
    )
}

/// An OpenRC service script (installed under /etc/init.d, runs in the background).
pub fn openrc_service(d: &Daemon, exec_path: &str) -> String {
    format!(
        "#!/sbin/openrc-run\n\
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
        exec = exec_path,
    )
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
    use super::*;

    #[test]
    fn registry_includes_all_four_daemons() {
        let names: Vec<&str> = all().iter().map(|d| d.name).collect();
        assert_eq!(names, vec!["voiced", "sharemd", "coordd", "settingsd"]);
        assert_eq!(find("settingsd").unwrap().socket, "settings");
        assert!(find("nope").is_none());
    }

    #[test]
    fn init_system_detection() {
        assert_eq!(InitSystem::detect_from_os_release("ID=alpine"), InitSystem::OpenRc);
        assert_eq!(InitSystem::detect_from_os_release("ID=postmarketos\nID_LIKE=alpine"), InitSystem::OpenRc);
        assert_eq!(InitSystem::detect_from_os_release("ID=fedora"), InitSystem::Systemd);
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
}

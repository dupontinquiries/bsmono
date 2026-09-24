//! `tideshell doctor power-key`: who on this machine consumes the power key.
//!
//! A press can be acted on by several independent readers at once, and the
//! fix is different for each, so the doctor names them instead of guessing:
//!
//! - **processes holding the key's event device open** (`/proc/*/fd` ->
//!   `/dev/input/eventN`): systemd-logind or elogind (button handling),
//!   the compositor (through libinput), acpid, triggerhappy, hkdm, ...
//!   Seeing other users' fds needs root, so run it with sudo for the full
//!   list; the report says how many processes it could not look into.
//! - **the login manager's HandlePowerKey**, from the files that manager
//!   actually reads. systemd-logind reads `*/systemd/logind.conf.d`, elogind
//!   (postmarketOS with OpenRC) reads `*/elogind/logind.conf.d`: a drop-in in
//!   the other one's directory does nothing.
//! - **handle-power-key inhibitors** currently held (D-Bus ListInhibitors).
//! - **cosmic's bindings** for XF86PowerOff (user `custom`, system defaults).
//!
//! The pure half (parsing and the verdict) is tested against a fake root;
//! the IO half only gathers inputs for it.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Reader {
    pub pid: u32,
    pub comm: String,
    pub node: PathBuf,
}

/// Processes with one of `nodes` open, and how many processes could not be
/// inspected (permission). `proc_root` is `/proc` outside tests.
pub fn readers(proc_root: &Path, nodes: &[PathBuf]) -> (Vec<Reader>, usize) {
    let mut out = Vec::new();
    let mut hidden = 0;
    let Ok(rd) = std::fs::read_dir(proc_root) else { return (out, 0) };
    let mut pids: Vec<u32> = rd.flatten().filter_map(|e| e.file_name().to_str()?.parse().ok()).collect();
    pids.sort();
    for pid in pids {
        let dir = proc_root.join(pid.to_string());
        let Ok(fds) = std::fs::read_dir(dir.join("fd")) else {
            hidden += 1;
            continue;
        };
        let comm = std::fs::read_to_string(dir.join("comm")).unwrap_or_default().trim().to_string();
        let mut seen: Vec<PathBuf> = Vec::new();
        for fd in fds.flatten() {
            if let Ok(target) = std::fs::read_link(fd.path()) {
                if nodes.contains(&target) && !seen.contains(&target) {
                    seen.push(target.clone());
                    out.push(Reader { pid, comm: comm.clone(), node: target });
                }
            }
        }
    }
    (out, hidden)
}

/// Every process name, for spotting known key consumers that are running
/// but whose fds were not visible.
pub fn running(proc_root: &Path) -> Vec<String> {
    let mut v: Vec<String> = std::fs::read_dir(proc_root)
        .into_iter()
        .flatten()
        .flatten()
        .filter(|e| e.file_name().to_str().is_some_and(|n| n.bytes().all(|b| b.is_ascii_digit())))
        .filter_map(|e| std::fs::read_to_string(e.path().join("comm")).ok())
        .map(|s| s.trim().to_string())
        .collect();
    v.sort();
    v.dedup();
    v
}

/// Processes known to act on the power key by themselves, with what to do
/// about each. Matched on /proc/<pid>/comm (15 characters at most).
pub const KNOWN: [(&str, &str); 9] = [
    ("systemd-logind", "systemd-logind: HandlePowerKey in /etc/systemd/logind.conf.d/"),
    ("elogind-daemon", "elogind: HandlePowerKey in /etc/elogind/logind.conf.d/"),
    ("elogind", "elogind: HandlePowerKey in /etc/elogind/logind.conf.d/"),
    ("acpid", "acpid: its power-button event handler (/etc/acpi/, e.g. PWRF/00000080 or events/powerbtn)"),
    ("thd", "triggerhappy: a KEY_POWER rule in /etc/triggerhappy/triggers.d/"),
    ("hkdm", "hkdm (postmarketOS hotkey daemon): its config for KEY_POWER"),
    ("cosmic-settings", "cosmic-settings-daemon: check whether it acts on the power button"),
    ("powerdevil", "PowerDevil (Plasma): its power-button action"),
    ("phosh", "phosh: owns the power key in a Phosh session"),
];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Manager {
    Logind,
    Elogind,
}

impl Manager {
    fn dir(self) -> &'static str {
        match self {
            Manager::Logind => "systemd",
            Manager::Elogind => "elogind",
        }
    }
    pub fn name(self) -> &'static str {
        match self {
            Manager::Logind => "systemd-logind",
            Manager::Elogind => "elogind",
        }
    }
    /// Where a drop-in for THIS manager goes.
    pub fn dropin_dir(self) -> String {
        format!("/etc/{}/logind.conf.d", self.dir())
    }
    pub fn from_running(names: &[String]) -> Option<Manager> {
        if names.iter().any(|n| n == "systemd-logind") {
            Some(Manager::Logind)
        } else if names.iter().any(|n| n == "elogind-daemon" || n == "elogind") {
            Some(Manager::Elogind)
        } else {
            None
        }
    }
}

/// The files a manager reads, in the order later ones override earlier:
/// the main file, then drop-ins sorted by file name across
/// /usr/lib, /usr/local/lib, /run, /etc, where a name in an earlier-listed
/// (higher-priority) directory masks the same name elsewhere. This is
/// systemd's conf-files rule; elogind uses the same code under its own
/// directory name.
pub fn config_files(root: &Path, m: Manager) -> Vec<PathBuf> {
    let d = m.dir();
    let mut out = Vec::new();
    // Main file: /etc wins; newer systemd ships the default under /usr/lib.
    for main in [format!("etc/{d}/logind.conf"), format!("usr/lib/{d}/logind.conf")] {
        let p = root.join(&main);
        if p.is_file() {
            out.push(p);
            break;
        }
    }
    let prio = [format!("etc/{d}/logind.conf.d"), format!("run/{d}/logind.conf.d"), format!("usr/local/lib/{d}/logind.conf.d"), format!("usr/lib/{d}/logind.conf.d")];
    let mut by_name: std::collections::BTreeMap<String, PathBuf> = std::collections::BTreeMap::new();
    for dir in prio.iter().rev() {
        for e in std::fs::read_dir(root.join(dir)).into_iter().flatten().flatten() {
            let n = e.file_name().to_string_lossy().into_owned();
            if n.ends_with(".conf") {
                by_name.insert(n, e.path()); // later loop = higher priority dir overwrites
            }
        }
    }
    out.extend(by_name.into_values());
    out
}

/// `HandlePowerKey` as the manager would see it after reading `files`, and
/// the file that set it (None = the compiled-in default, poweroff).
pub fn effective_handle_power_key(files: &[PathBuf]) -> (String, Option<PathBuf>) {
    let mut value = ("poweroff".to_string(), None);
    for f in files {
        let text = std::fs::read_to_string(f).unwrap_or_default();
        let mut in_login = false;
        for line in text.lines().map(str::trim) {
            if line.starts_with('[') {
                in_login = line == "[Login]";
                continue;
            }
            if in_login {
                if let Some(v) = line.strip_prefix("HandlePowerKey=") {
                    value = (v.trim().to_string(), Some(f.clone()));
                }
            }
        }
    }
    value
}

/// cosmic's XF86PowerOff bindings: (file, the binding line).
pub fn cosmic_power_bindings(files: &[PathBuf]) -> Vec<(PathBuf, String)> {
    let mut out = Vec::new();
    for f in files {
        for line in std::fs::read_to_string(f).unwrap_or_default().lines() {
            if line.contains("\"XF86PowerOff\"") {
                out.push((f.clone(), line.trim().to_string()));
            }
        }
    }
    out
}

/// A held inhibitor: (what, who, mode, pid).
pub type Inhibitor = (String, String, String, u32);

pub struct Findings {
    pub devices: Vec<(PathBuf, String)>,
    pub readers: Vec<Reader>,
    pub hidden: usize,
    pub is_root: bool,
    pub running: Vec<String>,
    pub manager: Option<Manager>,
    pub handle: (String, Option<PathBuf>),
    /// Drop-ins with HandlePowerKey sitting in the OTHER manager's directory.
    pub misplaced: Vec<PathBuf>,
    pub inhibitors: Option<Vec<Inhibitor>>,
    pub cosmic_user: Vec<(PathBuf, String)>,
    pub cosmic_system: Vec<(PathBuf, String)>,
}

/// The report: facts first, then what each fact means. Pure.
pub fn report(f: &Findings) -> String {
    let mut s = String::new();
    let mut verdict: Vec<String> = Vec::new();
    s.push_str("power-key devices:\n");
    if f.devices.is_empty() {
        s.push_str("  none found in /sys/class/input (KEY_POWER capability)\n");
    }
    for (node, name) in &f.devices {
        s.push_str(&format!("  {}  {name}\n", node.display()));
    }
    s.push_str("processes reading them:\n");
    for r in &f.readers {
        s.push_str(&format!("  pid {:<7} {:<16} {}\n", r.pid, r.comm, r.node.display()));
    }
    if f.readers.is_empty() {
        s.push_str("  none visible\n");
    }
    if f.hidden > 0 && !f.is_root {
        s.push_str(&format!("  ({} processes could not be inspected: run with sudo to see system daemons)\n", f.hidden));
    }
    for (comm, what) in KNOWN {
        let reads = f.readers.iter().any(|r| r.comm == comm);
        let runs = f.running.iter().any(|n| n == comm);
        if reads || runs {
            s.push_str(&format!("known consumer {}: {what}\n", if reads { "READING THE KEY" } else { "running" }));
            if reads && !matches!(comm, "systemd-logind" | "elogind-daemon" | "elogind") {
                verdict.push(format!("{comm} reads the power key and acts on it by itself -> {what}"));
            }
        }
    }
    let others: Vec<&Reader> = f.readers.iter().filter(|r| !KNOWN.iter().any(|(c, _)| *c == r.comm) && !r.comm.starts_with("cosmic-comp") && r.comm != "tideshell").collect();
    for r in &others {
        verdict.push(format!("{} (pid {}) reads {} and is not one this doctor knows: look at what it does with KEY_POWER", r.comm, r.pid, r.node.display()));
    }
    match f.manager {
        Some(m) => {
            let from = f.handle.1.as_ref().map(|p| p.display().to_string()).unwrap_or_else(|| "compiled-in default".into());
            s.push_str(&format!("login manager: {} -- HandlePowerKey={} (from {from})\n", m.name(), f.handle.0));
            let blocked = f.inhibitors.as_ref().is_some_and(|v| v.iter().any(|(what, _, mode, _)| what.split(':').any(|w| w == "handle-power-key") && mode == "block"));
            if f.handle.0 != "ignore" && !blocked {
                verdict.push(format!(
                    "{} will act on a short press ({}) and nothing blocks it: install 50-tideshell-powerkey.conf into {}/ and reload {}",
                    m.name(),
                    f.handle.0,
                    m.dropin_dir(),
                    m.name()
                ));
            }
            for p in &f.misplaced {
                verdict.push(format!("{} sets HandlePowerKey but is in the other manager's directory; {} never reads it (use {}/)", p.display(), m.name(), m.dropin_dir()));
            }
        }
        None => s.push_str("login manager: none running (no systemd-logind or elogind process)\n"),
    }
    match &f.inhibitors {
        Some(v) => {
            let pk: Vec<&Inhibitor> = v.iter().filter(|(what, ..)| what.split(':').any(|w| w == "handle-power-key")).collect();
            s.push_str(&format!("handle-power-key inhibitors: {}\n", if pk.is_empty() { "none".into() } else { pk.iter().map(|(_, who, mode, pid)| format!("{who} ({mode}, pid {pid})")).collect::<Vec<_>>().join(", ") }));
        }
        None => s.push_str("handle-power-key inhibitors: could not ask the login manager over D-Bus\n"),
    }
    for (p, l) in f.cosmic_user.iter().chain(&f.cosmic_system) {
        s.push_str(&format!("cosmic binding: {}  {l}\n", p.display()));
    }
    let user_routes_to_us = f.cosmic_user.iter().any(|(_, l)| l.contains("ctl power-key"));
    let system_poweroff = f.cosmic_system.iter().any(|(_, l)| l.contains("PowerOff") || l.contains("Spawn") && !l.contains("ctl power-key"));
    if system_poweroff && !user_routes_to_us {
        verdict.push("cosmic's default binds the power key (the shutdown dialog) and your custom shortcuts do not override it: run `tideshell install-keys`".into());
    }
    s.push_str("\nverdict:\n");
    if f.devices.is_empty() {
        s.push_str("  no device reports KEY_POWER here, so there is nothing to diagnose (not the phone, or /sys is not the host's)\n");
        return s;
    }
    if verdict.is_empty() {
        s.push_str("  nothing found that acts on a short press besides the shell.\n");
        if f.hidden > 0 && !f.is_root {
            s.push_str("  (not conclusive without sudo)\n");
        }
    }
    for v in verdict {
        s.push_str(&format!("  - {v}\n"));
    }
    s
}

/// Gather everything from the live system. `bus` is the system bus.
pub fn gather(bus: Option<&zbus::blocking::Connection>, cosmic_config_home: Option<&Path>) -> Findings {
    let root = Path::new("/");
    let devices: Vec<(PathBuf, String)> = crate::keys::discover(Path::new("/sys"), Path::new("/dev/input")).into_iter().filter(|d| d.power).map(|d| (d.node, d.name)).collect();
    let nodes: Vec<PathBuf> = devices.iter().map(|(n, _)| n.clone()).collect();
    let (readers, hidden) = readers(Path::new("/proc"), &nodes);
    let running = running(Path::new("/proc"));
    let manager = Manager::from_running(&running);
    let handle = manager.map(|m| effective_handle_power_key(&config_files(root, m))).unwrap_or(("poweroff".into(), None));
    let misplaced = match manager {
        Some(m) => {
            let other = if m == Manager::Logind { Manager::Elogind } else { Manager::Logind };
            config_files(root, other).into_iter().filter(|p| effective_handle_power_key(std::slice::from_ref(p)).1.is_some()).collect()
        }
        None => vec![],
    };
    let inhibitors = bus.and_then(|b| list_inhibitors(b).ok());
    let cosmic_user = cosmic_config_home.map(|h| cosmic_power_bindings(&[crate::ctl::cosmic_custom_shortcuts(h)])).unwrap_or_default();
    let sys_dir = Path::new("/usr/share/cosmic/com.system76.CosmicSettings.Shortcuts/v1");
    let cosmic_system = cosmic_power_bindings(&[sys_dir.join("defaults"), sys_dir.join("system_actions")]);
    Findings { devices, readers, hidden, is_root: unsafe { libc::geteuid() } == 0, running, manager, handle, misplaced, inhibitors, cosmic_user, cosmic_system }
}

fn list_inhibitors(bus: &zbus::blocking::Connection) -> Result<Vec<Inhibitor>, String> {
    let p = zbus::blocking::Proxy::new(bus, "org.freedesktop.login1", "/org/freedesktop/login1", "org.freedesktop.login1.Manager").map_err(|e| e.to_string())?;
    let reply = p.call_method("ListInhibitors", &()).map_err(|e| e.to_string())?;
    let v: Vec<(String, String, String, String, u32, u32)> = reply.body().deserialize().map_err(|e| e.to_string())?;
    Ok(v.into_iter().map(|(what, who, _why, mode, _uid, pid)| (what, who, mode, pid)).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tmp(tag: &str) -> PathBuf {
        let p = std::env::temp_dir().join(format!("tideshell-pd-{tag}-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&p);
        std::fs::create_dir_all(&p).unwrap();
        p
    }
    fn put(p: PathBuf, s: &str) {
        std::fs::create_dir_all(p.parent().unwrap()).unwrap();
        std::fs::write(p, s).unwrap();
    }

    #[test]
    fn readers_are_found_by_what_their_fds_point_at() {
        let proc_ = tmp("proc");
        let ev0 = PathBuf::from("/dev/input/event0");
        for (pid, comm, target) in [(12, "elogind-daemon", "/dev/input/event0"), (40, "cosmic-comp", "/dev/input/event0"), (41, "cosmic-comp", "/dev/input/event3"), (77, "acpid", "/dev/input/event0")] {
            put(proc_.join(format!("{pid}/comm")), &format!("{comm}\n"));
            std::fs::create_dir_all(proc_.join(format!("{pid}/fd"))).unwrap();
            std::os::unix::fs::symlink(target, proc_.join(format!("{pid}/fd/5"))).unwrap();
            std::os::unix::fs::symlink(target, proc_.join(format!("{pid}/fd/6"))).unwrap(); // two fds, one reader
        }
        put(proc_.join("99/comm"), "hidden\n"); // no fd dir: not inspectable
        let (r, hidden) = readers(&proc_, &[ev0.clone()]);
        let names: Vec<(u32, &str)> = r.iter().map(|x| (x.pid, x.comm.as_str())).collect();
        assert_eq!(names, vec![(12, "elogind-daemon"), (40, "cosmic-comp"), (77, "acpid")]);
        assert_eq!(hidden, 1);
        assert!(running(&proc_).contains(&"acpid".to_string()));
        let _ = std::fs::remove_dir_all(&proc_);
    }

    #[test]
    fn handle_power_key_follows_the_managers_own_files_and_order() {
        let root = tmp("root");
        put(root.join("etc/elogind/logind.conf"), "[Login]\n#HandlePowerKey=poweroff\n");
        // systemd's dir: elogind never reads it.
        put(root.join("etc/systemd/logind.conf.d/50-tideshell-powerkey.conf"), "[Login]\nHandlePowerKey=ignore\n");
        let files = config_files(&root, Manager::Elogind);
        assert_eq!(effective_handle_power_key(&files), ("poweroff".into(), None), "the drop-in in systemd's dir does not count");
        // The same drop-in where elogind looks.
        put(root.join("usr/lib/elogind/logind.conf.d/10-vendor.conf"), "[Login]\nHandlePowerKey=suspend\n");
        put(root.join("etc/elogind/logind.conf.d/50-tideshell-powerkey.conf"), "[Login]\nHandlePowerKey=ignore\n");
        let files = config_files(&root, Manager::Elogind);
        let (v, from) = effective_handle_power_key(&files);
        assert_eq!(v, "ignore", "50- sorts after 10-, so it wins");
        assert!(from.unwrap().ends_with("etc/elogind/logind.conf.d/50-tideshell-powerkey.conf"));
        // A same-named file in /etc masks the one in /usr/lib.
        put(root.join("usr/lib/elogind/logind.conf.d/99-late.conf"), "[Login]\nHandlePowerKey=poweroff\n");
        put(root.join("etc/elogind/logind.conf.d/99-late.conf"), "[Login]\nHandlePowerKey=ignore\n");
        assert_eq!(effective_handle_power_key(&config_files(&root, Manager::Elogind)).0, "ignore");
        let _ = std::fs::remove_dir_all(&root);
        // Keys outside [Login] do not count.
        let r2 = tmp("root2");
        put(r2.join("etc/systemd/logind.conf"), "[Other]\nHandlePowerKey=ignore\n");
        assert_eq!(effective_handle_power_key(&config_files(&r2, Manager::Logind)), ("poweroff".into(), None));
        let _ = std::fs::remove_dir_all(&r2);
    }

    fn base() -> Findings {
        Findings {
            devices: vec![(PathBuf::from("/dev/input/event0"), "pmic_pwrkey".into())],
            readers: vec![],
            hidden: 0,
            is_root: true,
            running: vec![],
            manager: None,
            handle: ("poweroff".into(), None),
            misplaced: vec![],
            inhibitors: Some(vec![]),
            cosmic_user: vec![],
            cosmic_system: vec![],
        }
    }

    #[test]
    fn the_verdict_names_each_consumer_and_its_fix() {
        let mut f = base();
        f.manager = Some(Manager::Elogind);
        f.running = vec!["elogind-daemon".into()];
        f.readers = vec![Reader { pid: 12, comm: "elogind-daemon".into(), node: "/dev/input/event0".into() }, Reader { pid: 77, comm: "acpid".into(), node: "/dev/input/event0".into() }, Reader { pid: 90, comm: "mystery".into(), node: "/dev/input/event0".into() }];
        f.misplaced = vec![PathBuf::from("/etc/systemd/logind.conf.d/50-tideshell-powerkey.conf")];
        let r = report(&f);
        assert!(r.contains("elogind will act on a short press (poweroff) and nothing blocks it: install 50-tideshell-powerkey.conf into /etc/elogind/logind.conf.d/"), "{r}");
        assert!(r.contains("elogind never reads it"), "{r}");
        assert!(r.contains("acpid reads the power key"), "{r}");
        assert!(r.contains("mystery (pid 90) reads /dev/input/event0"), "{r}");
        // A block inhibitor on handle-power-key silences the manager line.
        f.inhibitors = Some(vec![("handle-power-key:handle-lid-switch".into(), "tideshell".into(), "block".into(), 300)]);
        assert!(!report(&f).contains("will act on a short press"));
        // A delay inhibitor does not.
        f.inhibitors = Some(vec![("handle-power-key".into(), "x".into(), "delay".into(), 1)]);
        assert!(report(&f).contains("will act on a short press"));
    }

    #[test]
    fn cosmic_default_binding_is_flagged_until_install_keys_overrides_it() {
        let mut f = base();
        f.cosmic_system = vec![(PathBuf::from("/usr/share/cosmic/.../defaults"), "(modifiers: [], key: \"XF86PowerOff\"): System(PowerOff),".into())];
        assert!(report(&f).contains("run `tideshell install-keys`"));
        f.cosmic_user = vec![(PathBuf::from("~/.config/cosmic/.../custom"), "(modifiers: [], key: \"XF86PowerOff\"): Spawn(\"/usr/bin/tideshell ctl power-key\"),".into())];
        assert!(!report(&f).contains("run `tideshell install-keys`"));
        let mut none = base();
        none.devices.clear();
        assert!(report(&none).contains("nothing to diagnose") && !report(&none).contains("nothing found that acts"), "no device is not an all-clear");
        // Nothing known, as root: says so plainly.
        assert!(report(&base()).contains("nothing found that acts on a short press"));
        let mut nr = base();
        nr.is_root = false;
        nr.hidden = 30;
        let r = report(&nr);
        assert!(r.contains("run with sudo") && r.contains("not conclusive without sudo"));
    }
}

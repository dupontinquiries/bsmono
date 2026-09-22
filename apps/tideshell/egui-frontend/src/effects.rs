//! Actions → effects. One [`Executor`] per environment; the engine never
//! knows which one it is talking to.
//!
//! Every outcome is reported back as an [`Outcome`] and written into the
//! shell trace by the caller, including the ones that are deliberately not
//! wired yet. "Tapped Wi-Fi, nothing happened" should read as
//! `unwired: no NetworkManager backend` on the phone, not as a silent no-op.

use std::path::PathBuf;
use std::process::{Command, Stdio};

use tideshell_core::prefs::ShellPrefs;
use tideshell_core::Action;

use crate::AppTile;

#[derive(Debug, Clone, PartialEq)]
pub enum Outcome {
    Done(String),
    /// Known gap, with the reason. Not an error.
    Unwired(&'static str),
    Failed(String),
}

impl Outcome {
    pub fn describe(&self) -> String {
        match self {
            Outcome::Done(s) => format!("ok: {s}"),
            Outcome::Unwired(s) => format!("unwired: {s}"),
            Outcome::Failed(s) => format!("FAILED: {s}"),
        }
    }
}

pub trait Executor {
    fn run(&mut self, action: &Action) -> Outcome;
}

/// A spawned command whose result is not known yet. `Outcome::Done` from
/// `run` only means STARTED; the real result comes from `System::reap` —
/// e.g. `busctl` exiting 1 because rusty-board is not running, which used
/// to be reported as "ok".
struct Pending {
    label: String,
    child: std::process::Child,
    /// What the shell should undo if this fails.
    on_fail: Option<Undo>,
}

/// A state change the engine made optimistically that a failed command
/// takes back.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Undo {
    /// The keyboard never appeared: the Keyboard mode/tile must go back off.
    KeyboardShown,
}

fn spawn_child(argv: &[&str], keep_stderr: bool) -> Result<std::process::Child, Outcome> {
    Command::new(argv[0])
        .args(&argv[1..])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(if keep_stderr { Stdio::piped() } else { Stdio::null() })
        .spawn()
        .map_err(|e| Outcome::Failed(format!("{}: {e}", argv[0])))
}

/// Missing-binary check for tests (spawn failure is immediate).
#[cfg(test)]
fn spawn(argv: &[&str]) -> Outcome {
    match spawn_child(argv, false) {
        Ok(_) => Outcome::Done("started".into()),
        Err(o) => o,
    }
}

/// The argv that asks rusty-board to show or hide. `busctl` rather than a
/// zbus dependency: it is on every systemd target, adds nothing to the build,
/// and the exact call can be pasted into a shell when debugging.
pub fn keyboard_argv(show: bool) -> [&'static str; 8] {
    [
        "busctl",
        "--user",
        "call",
        "io.rustyboard.Keyboard",
        "/io/rustyboard/Keyboard",
        "io.rustyboard.Keyboard1",
        if show { "Show" } else { "Hide" },
        "",
    ]
}

/// Effects on a real session: hatch for apps, rusty-board for the keyboard,
/// a prefs file. Window management and the EI pointer are the next transports
/// and report `Unwired` until they exist.
pub struct System {
    pub launcher: Option<hatch_core::Launcher>,
    pending: Vec<Pending>,
}

impl System {
    pub fn new() -> System {
        System { launcher: Some(hatch_core::Launcher::new(hatch_core::Config::load())), pending: Vec::new() }
    }
}

impl Default for System {
    fn default() -> Self {
        Self::new()
    }
}

impl System {
    /// Start a command whose exit status matters; its result arrives via `reap`.
    fn start(&mut self, argv: &[&str], long_running: bool) -> Outcome {
        self.start_with(argv, long_running, None)
    }

    fn start_with(&mut self, argv: &[&str], long_running: bool, on_fail: Option<Undo>) -> Outcome {
        // A long-running program (hatch-egui staying resident) is never
        // reaped; only short calls whose exit status IS the result are.
        match spawn_child(argv, !long_running) {
            Ok(child) => {
                let label = argv.iter().filter(|a| !a.is_empty()).cloned().collect::<Vec<_>>().join(" ");
                let msg = format!("started {label} (pid {})", child.id());
                if !long_running {
                    self.pending.push(Pending { label, child, on_fail });
                }
                Outcome::Done(msg)
            }
            Err(o) => o,
        }
    }

    /// Non-blocking: results of short commands that have finished since the
    /// last call. Call once per loop iteration and put them in the trace.
    pub fn reap(&mut self) -> Vec<(Outcome, Option<Undo>)> {
        let mut out = Vec::new();
        self.pending.retain_mut(|p| match p.child.try_wait() {
            Ok(Some(status)) if status.success() => {
                out.push((Outcome::Done(format!("{} succeeded", p.label)), None));
                false
            }
            Ok(Some(status)) => {
                let mut err = String::new();
                if let Some(mut e) = p.child.stderr.take() {
                    use std::io::Read;
                    let _ = e.read_to_string(&mut err);
                }
                let err = err.trim().lines().last().unwrap_or("").to_string();
                out.push((Outcome::Failed(format!("{} exited {}{}", p.label, status.code().map_or("by signal".into(), |c| c.to_string()), if err.is_empty() { String::new() } else { format!(": {err}") })), p.on_fail));
                false
            }
            Ok(None) => true,
            Err(e) => {
                out.push((Outcome::Failed(format!("{}: {e}", p.label)), p.on_fail));
                false
            }
        });
        out
    }
}

impl Executor for System {
    fn run(&mut self, action: &Action) -> Outcome {
        match action {
            Action::LaunchApp(id) => match self.launcher.as_mut() {
                // Through hatch-core so the launch lands in hatch's recents
                // and honours Terminal=/Path= exactly as hatch would.
                Some(l) => match l.activate(id) {
                    Ok(pid) => Outcome::Done(format!("launched {id} (pid {pid})")),
                    Err(e) => Outcome::Failed(format!("launch {id}: {e}")),
                },
                None => Outcome::Failed("no hatch launcher".into()),
            },
            // One instance, poke-file toggle: starts hatch if it is not running.
            // --toggle pokes a running hatch and exits, or BECOMES the
            // resident instance when none runs: treat it as long-running.
            Action::SummonLauncher => self.start(&["hatch-egui", "--toggle"], true),
            Action::ShowKeyboard(on) => {
                let a = keyboard_argv(*on);
                self.start_with(&a[..7], false, on.then_some(Undo::KeyboardShown))
            }
            Action::ShowTrackpad(on) => Outcome::Done(format!("trackpad overlay {}", if *on { "shown" } else { "hidden" })),
            Action::SavePrefs => Outcome::Unwired("caller persists prefs (save_prefs) since the executor does not hold them"),
            Action::HideSystemCursor(on) => match cosmic_config_home() {
                Some(base) => hide_system_cursor_in(&base, *on),
                None => Outcome::Failed("no config home".into()),
            },
            Action::Activate(_) | Action::Close(_) | Action::SetMaximized(..) => {
                Outcome::Unwired("zcosmic_toplevel_manager_v1 requests not in systemcompkit yet")
            }
            Action::Pointer(_) => Outcome::Unwired("EI sender not wired (needs cosmic-comp patch 0002 + reis client)"),
            Action::SetQuick(..) => Outcome::Unwired("no system backend for quick toggles yet"),
            Action::SetSlider(..) => Outcome::Unwired("no brightness/volume backend yet"),
            Action::SetGovernor(_) => Outcome::Unwired("cpu governor: no system backend for this executor"),
            Action::DisplayPower(_) => Outcome::Unwired("display power: needs the layer runtime's OutputPower (see bin/tideshell.rs), not this generic executor"),
        }
    }
}

// ---- system cursor (cosmic-comp) --------------------------------------------

/// `$XDG_CONFIG_HOME` (or `~/.config`): where cosmic-config keeps user values.
pub fn cosmic_config_home() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
}

/// cosmic-comp's `cursor_hide_timeout`: seconds of inactivity before it hides
/// the cursor, live-reloaded from cosmic-config. `Some(0)` hides it right
/// after any motion, which is how the fat cursor replaces it without a
/// compositor patch. (cosmic-comp refuses to hide while the pointer is
/// grabbed, so it shows during a held-button drag.)
pub fn cursor_hide_path(config_home: &std::path::Path) -> PathBuf {
    config_home.join("cosmic/com.system76.CosmicComp/v1/cursor_hide_timeout")
}

/// Where the user's own value is kept while tideshell overrides it. On disk,
/// not in memory: if tideshell dies with the fat cursor on, `Some(0)` would
/// otherwise stay in cosmic-comp's config and the system cursor would stay
/// hidden with nothing left to restore it. `recover_system_cursor` runs at
/// startup and puts it back. Content: `absent`, or `value:` + the old text.
pub fn cursor_saved_path(config_home: &std::path::Path) -> PathBuf {
    config_home.join("tideshell/cursor_hide_timeout.saved")
}

fn write_atomic(path: &std::path::Path, text: &str) -> std::io::Result<()> {
    if let Some(d) = path.parent() {
        std::fs::create_dir_all(d)?;
    }
    let tmp = path.with_extension("tideshell-tmp");
    std::fs::write(&tmp, text)?;
    std::fs::rename(&tmp, path)
}

fn remove_if_present(path: &std::path::Path) -> std::io::Result<()> {
    std::fs::remove_file(path).or_else(|e| if e.kind() == std::io::ErrorKind::NotFound { Ok(()) } else { Err(e) })
}

/// Override (on) or restore (off) cosmic-comp's `cursor_hide_timeout`. The
/// user's own value is saved once, on the first override, and restored
/// exactly — including "no key at all". Written atomically (write + rename),
/// as cosmic-config itself does, so the compositor's watcher never reads
/// half a file.
pub fn hide_system_cursor_in(config_home: &std::path::Path, on: bool) -> Outcome {
    let path = cursor_hide_path(config_home);
    let saved = cursor_saved_path(config_home);
    let r = (|| -> std::io::Result<()> {
        if on {
            if !saved.exists() {
                let orig = match std::fs::read_to_string(&path) {
                    Ok(t) => format!("value:{t}"),
                    Err(_) => "absent".to_string(),
                };
                write_atomic(&saved, &orig)?;
            }
            write_atomic(&path, "Some(0)")
        } else {
            restore_from(&path, &saved)
        }
    })();
    match r {
        Ok(()) => Outcome::Done(format!("system cursor {} ({})", if on { "hidden" } else { "restored" }, path.display())),
        Err(e) => Outcome::Failed(format!("{}: {e}", path.display())),
    }
}

fn restore_from(path: &std::path::Path, saved: &std::path::Path) -> std::io::Result<()> {
    let Ok(orig) = std::fs::read_to_string(saved) else {
        return Ok(()); // never overridden: leave the user's value alone
    };
    match orig.strip_prefix("value:") {
        Some(text) => write_atomic(path, text)?,
        None => remove_if_present(path)?,
    }
    remove_if_present(saved)
}

/// Startup recovery: a previous run left the cursor overridden (it crashed
/// or was killed with the fat cursor on). `None` when there was nothing to do.
pub fn recover_system_cursor(config_home: &std::path::Path) -> Option<Outcome> {
    let saved = cursor_saved_path(config_home);
    saved.exists().then(|| match restore_from(&cursor_hide_path(config_home), &saved) {
        Ok(()) => Outcome::Done("restored system cursor left hidden by a previous run".into()),
        Err(e) => Outcome::Failed(format!("cursor recovery: {e}")),
    })
}

// ---- prefs file ------------------------------------------------------------

/// `$XDG_CONFIG_HOME/tideshell/prefs`, flat `key = value` lines — the same
/// pairs `ShellPrefs::to_kv` hands settingsd, so moving to settingsd later is
/// a transport swap, not a format change.
pub fn prefs_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("tideshell").join("prefs"))
}

pub fn parse_prefs(text: &str) -> (ShellPrefs, Vec<String>) {
    let mut p = ShellPrefs::default();
    let pairs = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with('#'))
        .filter_map(|l| l.split_once('='))
        .map(|(k, v)| (k.trim(), v.trim()));
    let errs = p.apply_kv(pairs);
    (p, errs)
}

pub fn render_prefs(p: &ShellPrefs) -> String {
    let mut s = String::from("# tideshell prefs: shell.* keys, same as settingsd's namespace\n");
    for (k, v) in p.to_kv() {
        s.push_str(&format!("{k} = {v}\n"));
    }
    s
}

pub fn load_prefs() -> (ShellPrefs, Vec<String>) {
    match prefs_path().and_then(|p| std::fs::read_to_string(p).ok()) {
        Some(t) => parse_prefs(&t),
        None => (ShellPrefs::default(), vec![]),
    }
}

pub fn save_prefs(p: &ShellPrefs) -> Outcome {
    let Some(path) = prefs_path() else { return Outcome::Failed("no HOME".into()) };
    if let Some(dir) = path.parent() {
        if let Err(e) = std::fs::create_dir_all(dir) {
            return Outcome::Failed(format!("{}: {e}", dir.display()));
        }
    }
    match std::fs::write(&path, render_prefs(p)) {
        Ok(()) => Outcome::Done(format!("saved {}", path.display())),
        Err(e) => Outcome::Failed(format!("{}: {e}", path.display())),
    }
}

// ---- hatch tiles -------------------------------------------------------------

/// Tiles for the bar's `favorites:N` and `app:<id>` widgets, from hatch's own
/// favorites file, so pinning in hatch pins in the bar. Falls back to hatch's
/// top-ranked entries when there are no favorites yet.
pub fn hatch_tiles(l: &hatch_core::Launcher, max: usize) -> Vec<AppTile> {
    let mut out: Vec<AppTile> = hatch_core::store::load_favorites()
        .iter()
        .filter_map(|id| l.entry(id))
        .map(|e| AppTile { id: e.id.clone(), name: e.name.clone() })
        .collect();
    if out.is_empty() {
        out = l.results().take(max).map(|e| AppTile { id: e.id.clone(), name: e.name.clone() }).collect();
    }
    out.truncate(max);
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use tideshell_core::prefs::Layout;

    #[test]
    fn prefs_file_round_trips_and_tolerates_junk() {
        let mut p = ShellPrefs::default();
        p.layout = Layout::RightBar;
        let text = render_prefs(&p) + "\n# comment\nnot a pair\nshell.hold_ms = 99999\n";
        let (q, errs) = parse_prefs(&text);
        assert_eq!(q.layout, Layout::RightBar);
        assert_eq!(q.hold_ms, p.hold_ms, "out-of-range value rejected, earlier value kept");
        assert_eq!(errs.len(), 1, "{errs:?}");
    }

    #[test]
    fn keyboard_call_names_rusty_boards_interface() {
        let a = keyboard_argv(true);
        assert_eq!(&a[3..7], &["io.rustyboard.Keyboard", "/io/rustyboard/Keyboard", "io.rustyboard.Keyboard1", "Show"]);
        assert_eq!(keyboard_argv(false)[6], "Hide");
    }

    #[test]
    fn system_cursor_override_restores_exactly_what_was_there() {
        let base = std::env::temp_dir().join(format!("tideshell-cursor-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = cursor_hide_path(&base);
        // Case 1: user had their own value.
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Some(5)").unwrap();
        assert!(matches!(hide_system_cursor_in(&base, true), Outcome::Done(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Some(0)");
        // A second "on" must not overwrite the saved original with our own value.
        hide_system_cursor_in(&base, true);
        hide_system_cursor_in(&base, false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Some(5)");
        assert!(!cursor_saved_path(&base).exists(), "sidecar cleared after restore");
        // Case 2: no key at all -> restored to no key.
        std::fs::remove_file(&path).unwrap();
        hide_system_cursor_in(&base, true);
        assert!(path.exists());
        hide_system_cursor_in(&base, false);
        assert!(!path.exists(), "restoring must remove the key we created");
        // Case 3: "off" without a prior "on" leaves the user's value alone.
        std::fs::write(&path, "Some(9)").unwrap();
        hide_system_cursor_in(&base, false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Some(9)");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn a_crashed_run_is_recovered_at_startup() {
        let base = std::env::temp_dir().join(format!("tideshell-cursor-crash-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = cursor_hide_path(&base);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "Some(3)").unwrap();
        hide_system_cursor_in(&base, true);
        // ...process dies here: nothing turns the override off...
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Some(0)");
        // Next start:
        assert!(matches!(recover_system_cursor(&base), Some(Outcome::Done(_))));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "Some(3)");
        assert!(recover_system_cursor(&base).is_none(), "nothing to do the second time");
        let _ = std::fs::remove_dir_all(&base);
    }

    #[test]
    fn short_commands_report_their_real_result() {
        let mut sys = System { launcher: None, pending: Vec::new() };
        assert!(matches!(sys.start(&["true"], false), Outcome::Done(ref s) if s.starts_with("started")));
        assert!(matches!(sys.start_with(&["sh", "-c", "echo nope >&2; exit 3"], false, Some(Undo::KeyboardShown)), Outcome::Done(_)));
        let mut got = Vec::new();
        for _ in 0..200 {
            got.extend(sys.reap());
            if got.len() == 2 {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
        assert_eq!(got.len(), 2, "{got:?}");
        assert!(got.iter().any(|o| matches!(o, (Outcome::Done(s), None) if s == "true succeeded")), "{got:?}");
        assert!(got.iter().any(|o| matches!(o, (Outcome::Failed(s), Some(Undo::KeyboardShown)) if s.contains("exited 3") && s.contains("nope"))), "{got:?}");
        assert!(sys.reap().is_empty(), "each result reported once");
    }

    #[test]
    fn long_running_commands_are_not_tracked() {
        let mut sys = System { launcher: None, pending: Vec::new() };
        sys.start(&["sleep", "5"], true);
        assert!(sys.pending.is_empty());
    }

    #[test]
    fn missing_binary_is_a_visible_failure() {
        assert!(matches!(spawn(&["/nonexistent/tideshell-probe"]), Outcome::Failed(_)));
    }

    #[test]
    fn launch_goes_through_hatch_core() {
        // A launcher with one entry whose Exec is `true`: the launch must
        // succeed through hatch_core::Launcher::activate, proving the wiring
        // (not a reimplementation of Exec parsing here).
        // activate() records a recent under XDG_CONFIG_HOME: keep it out of
        // the real one. Only test in this crate that touches the environment.
        let home = std::env::temp_dir().join(format!("tideshell-launch-{}", std::process::id()));
        std::fs::create_dir_all(&home).unwrap();
        std::env::set_var("XDG_CONFIG_HOME", &home);
        let entry = hatch_core::AppEntry { id: "t".into(), name: "T".into(), exec: "true".into(), ..Default::default() };
        let mut sys = System { launcher: Some(hatch_core::Launcher::with_entries(hatch_core::Config::default(), vec![entry])), pending: Vec::new() };
        match sys.run(&Action::LaunchApp("t".into())) {
            Outcome::Done(s) => assert!(s.starts_with("launched t")),
            o => panic!("{o:?}"),
        }
        assert!(matches!(sys.run(&Action::LaunchApp("missing".into())), Outcome::Failed(_)));
    }
}

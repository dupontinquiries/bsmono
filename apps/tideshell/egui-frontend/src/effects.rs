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
    /// For moving launched apps into systemd --user scopes.
    pub session_bus: Option<zbus::blocking::Connection>,
}

impl System {
    pub fn new() -> System {
        System { launcher: Some(hatch_core::Launcher::new(hatch_core::Config::load())), pending: Vec::new(), session_bus: zbus::blocking::Connection::session().ok() }
    }
}

/// The program that summons hatch. The crate's binary is `hatch-egui`, but the gitmind deploy
/// installs apps under their directory name, so the phone has `hatch` (a symlink to a
/// `hatch@import-…` file) and NO `hatch-egui` — the apps button spawned a name that did not
/// exist and the spawn failure only reached the trace. `override_bin` (HATCH_BIN) wins; then
/// the first of `hatch-egui`, `hatch` found as a file on `path`; else `hatch-egui`, so the
/// failure names the program the shell was built to expect.
pub fn pick_launcher(path: &str, override_bin: Option<&str>) -> String {
    if let Some(b) = override_bin.map(str::trim).filter(|b| !b.is_empty()) { return b.to_string(); }
    for name in ["hatch-egui", "hatch"] {
        if path.split(':').filter(|d| !d.is_empty()).any(|d| std::path::Path::new(d).join(name).is_file()) {
            return name.to_string();
        }
    }
    "hatch-egui".to_string()
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
                    Ok(pid) => {
                        // Into its own cgroup scope, so comfortgov can reclaim
                        // background apps one by one and protect the front one.
                        let scoped = match &self.session_bus {
                            Some(bus) => match move_to_scope(bus, pid, id) {
                                Ok(unit) => format!(", scope {unit}"),
                                Err(e) => format!(", no scope ({e})"),
                            },
                            None => ", no scope (no session bus)".to_string(),
                        };
                        Outcome::Done(format!("launched {id} (pid {pid}){scoped}"))
                    }
                    Err(e) => Outcome::Failed(format!("launch {id}: {e}")),
                },
                None => Outcome::Failed("no hatch launcher".into()),
            },
            // One instance, poke-file toggle: starts hatch if it is not running.
            // --toggle pokes a running hatch and exits, or BECOMES the
            // resident instance when none runs: treat it as long-running.
            Action::SummonLauncher => {
                let prog = pick_launcher(&std::env::var("PATH").unwrap_or_default(), std::env::var("HATCH_BIN").ok().as_deref());
                self.start(&[prog.as_str(), "--toggle"], true)
            }
            Action::ShowKeyboard(on) => {
                let a = keyboard_argv(*on);
                self.start_with(&a[..7], false, on.then_some(Undo::KeyboardShown))
            }
            Action::ShowTrackpad(on) => Outcome::Done(format!("trackpad overlay {}", if *on { "shown" } else { "hidden" })),
            Action::KeyboardCommand(cmd) => match send_keyboard_command(cmd) {
                Ok(()) => Outcome::Done(format!("keyboard: {cmd}")),
                // Not running, or an older build without the socket: say so, do not fail loudly.
                Err(e) => Outcome::Failed(format!("keyboard: {cmd}: {e}")),
            },
            Action::SavePrefs => Outcome::Unwired("caller persists prefs (save_prefs) since the executor does not hold them"),
            Action::HideSystemCursor(on) => match cosmic_config_home() {
                Some(base) => hide_system_cursor_in(&base, *on),
                None => Outcome::Failed("no config home".into()),
            },
            Action::Activate(_) | Action::Close(_) | Action::SetMaximized(..) => {
                Outcome::Unwired("zcosmic_toplevel_manager_v1 requests not in systemcompkit yet")
            }
            Action::Pointer(_) => Outcome::Unwired("EI sender not wired (needs cosmic-comp patch 0002 + reis client)"),
            Action::SetQuick(kind, on) => apply_quick(*kind, *on),
            Action::SetSlider(tideshell_core::control_center::SliderKind::Volume, v) => fade_volume(*v),
            Action::SetSlider(tideshell_core::control_center::SliderKind::Warmth, v) => apply_warmth(*v),
            Action::SetSlider(..) => Outcome::Unwired("brightness is applied by the layer runtime"),
            Action::SetGovernor(_) => Outcome::Unwired("cpu governor: writing scaling_governor needs root; comfortgov (system/comfortgov) is not installed on this phone"),
            Action::DisplayPower(_) => Outcome::Unwired("display power is handled by the layer runtime"),
            Action::Rotate(deg) => rotate_output(*deg),
            Action::SetVoicing(tone) => apply_voicing(*tone),
            Action::AppsLightTheme(on) => match cosmic_config_home() {
                Some(base) => set_apps_light_theme(&base, *on),
                None => Outcome::Failed("no config home".into()),
            },
        }
    }
}

// ---- apps' theme (cosmic) ------------------------------------------------------

/// cosmic's live-reloaded theme mode: `is_dark` under
/// com.system76.CosmicTheme.Mode. COSMIC apps follow it at once; GTK apps
/// through cosmic-settings-daemon's portal setting. Saved to a sidecar
/// and restored exactly (including "no key"), like the cursor override.
pub fn theme_mode_path(config_home: &std::path::Path) -> PathBuf {
    config_home.join("cosmic/com.system76.CosmicTheme.Mode/v1/is_dark")
}

pub fn set_apps_light_theme(config_home: &std::path::Path, light: bool) -> Outcome {
    let path = theme_mode_path(config_home);
    let saved = config_home.join("tideshell/is_dark.saved");
    let r = (|| -> std::io::Result<()> {
        if light {
            if !saved.exists() {
                let orig = match std::fs::read_to_string(&path) {
                    Ok(t) => format!("value:{t}"),
                    Err(_) => "absent".to_string(),
                };
                write_atomic(&saved, &orig)?;
            }
            write_atomic(&path, "false")
        } else {
            restore_from(&path, &saved)
        }
    })();
    match r {
        Ok(()) => Outcome::Done(format!("apps theme {} ({})", if light { "light for sunlight" } else { "restored" }, path.display())),
        Err(e) => Outcome::Failed(format!("{}: {e}", path.display())),
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

// ---- app scopes (systemd --user) ------------------------------------------------

/// A unit name for one launch: `app-tideshell-<id>-<pid>.scope`, with the
/// desktop id reduced to systemd's safe set so comfortgov can read the app
/// id back out of the cgroup's directory name.
pub fn scope_unit_name(desktop_id: &str, pid: u32) -> String {
    format!("app-tideshell-{}-{pid}.scope", sanitize_unit(desktop_id))
}

pub fn sanitize_unit(s: &str) -> String {
    s.chars().map(|c| if c.is_ascii_alphanumeric() || c == '.' || c == '_' { c } else { '_' }).collect()
}

/// systemd1.Manager.StartTransientUnit with the PID, under app.slice.
pub fn move_to_scope(bus: &zbus::blocking::Connection, pid: u32, desktop_id: &str) -> Result<String, String> {
    use zbus::zvariant::Value;
    let unit = scope_unit_name(desktop_id, pid);
    let p = zbus::blocking::Proxy::new(bus, "org.freedesktop.systemd1", "/org/freedesktop/systemd1", "org.freedesktop.systemd1.Manager").map_err(|e| e.to_string())?;
    let props: Vec<(&str, Value)> = vec![("PIDs", Value::from(vec![pid])), ("Slice", Value::from("app.slice")), ("CollectMode", Value::from("inactive-or-failed"))];
    let aux: Vec<(&str, Vec<(&str, Value)>)> = Vec::new();
    p.call_method("StartTransientUnit", &(unit.as_str(), "fail", props, aux)).map_err(|e| format!("StartTransientUnit: {e}"))?;
    Ok(unit)
}

// ---- voicing (PipeWire filter-chain) --------------------------------------------

/// The node id of `name` in `pw-cli ls Node` output (blocks of
/// `\tid N, type ...` followed by indented `key = "value"` lines).
pub fn parse_node_id(listing: &str, name: &str) -> Option<u32> {
    let mut cur: Option<u32> = None;
    for line in listing.lines() {
        let t = line.trim();
        if let Some(rest) = t.strip_prefix("id ") {
            cur = rest.split(',').next().and_then(|n| n.trim().parse().ok());
        } else if t.starts_with("node.name") && t.ends_with(&format!("\"{name}\"")) {
            return cur;
        }
    }
    None
}

pub fn voicing_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("pipewire/pipewire.conf.d/tideshell-voicing.conf"))
}

/// Set the voicing sink's controls; if the sink does not exist yet, install
/// its config (PipeWire picks it up after a restart) and say so.
pub fn apply_voicing(tone: f32) -> Outcome {
    let p = tideshell_core::audio::params(tone);
    let listing = match Command::new("pw-cli").args(["ls", "Node"]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Outcome::Failed(format!("pw-cli ls: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Outcome::Failed(format!("pw-cli: {e} (PipeWire tools not installed)")),
    };
    let Some(id) = parse_node_id(&listing, tideshell_core::audio::SINK_NAME) else {
        return match voicing_config_path() {
            Some(path) if path.exists() => Outcome::Failed(format!("voicing sink not running; config is at {} (restart pipewire: systemctl --user restart pipewire)", path.display())),
            Some(path) => {
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match std::fs::write(&path, tideshell_core::audio::filter_chain_config()) {
                    Ok(()) => Outcome::Failed(format!("voicing sink installed at {}; restart pipewire (systemctl --user restart pipewire) and set it as the default sink", path.display())),
                    Err(e) => Outcome::Failed(format!("cannot write {}: {e}", path.display())),
                }
            }
            None => Outcome::Failed("no config dir for the voicing sink".into()),
        };
    };
    let props = tideshell_core::audio::props_text(&p);
    match Command::new("pw-cli").args(["set-param", &id.to_string(), "Props", &props]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Outcome::Done(format!("voicing {tone:+.2}: hp {:.0} Hz, lp {:.0} Hz, presence {:+.1} dB, trim {:+.1} dB", p.hp_hz, p.lp_hz, p.presence_db, p.trim_db)),
        Ok(o) => Outcome::Failed(format!("pw-cli set-param: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Outcome::Failed(format!("pw-cli: {e}")),
    }
}

/// Where the saved chain lives; the settings app writes it.
pub fn speaker_state_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("tideshell").join("speaker"))
}

/// Apply the saved chain, so the shell comes up sounding the way it was
/// left rather than at defaults until someone opens the settings app.
///
/// Resolved HERE with the shell's own slider positions, which is the reason
/// the file stores choices rather than resolved numbers: a parameter that
/// follows the volume must follow THIS session's volume, not the volume at
/// the moment it was saved.
pub fn apply_saved_speaker(warmth: f32, volume: f32) -> Outcome {
    let Some(path) = speaker_state_path() else { return Outcome::Unwired("no config dir") };
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Outcome::Unwired("no saved speaker chain");
    };
    let (values, bindings) = tideshell_core::chaingraph::decode_state(&text);
    if values.is_empty() && bindings.is_empty() {
        return Outcome::Unwired("saved speaker chain is empty");
    }
    apply_speaker_props(&tideshell_core::chaingraph::resolve(&bindings, &values, warmth, volume))
}

/// Send resolved control/value pairs to the speaker chain.
///
/// One Props message for the whole set: the chain is a graph, and half of a
/// change is a different sound rather than a lesser one.
pub fn apply_speaker_props(pairs: &[(String, f32)]) -> Outcome {
    if pairs.is_empty() {
        return Outcome::Unwired("no speaker props to apply");
    }
    let listing = match Command::new("pw-cli").args(["ls", "Node"]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Outcome::Failed(format!("pw-cli ls: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Outcome::Failed(format!("pw-cli: {e}")),
    };
    let Some(id) = parse_node_id(&listing, tideshell_core::audio::SPEAKER_SINK) else {
        return Outcome::Unwired("speaker chain not running");
    };
    let body: Vec<String> = pairs.iter().map(|(k, v)| format!("\"{k}\" {v:.4}")).collect();
    let props = format!("{{ params = [ {} ] }}", body.join(" "));
    match Command::new("pw-cli").args(["set-param", &id.to_string(), "Props", &props]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Outcome::Done(format!("speaker: {} control(s) set", pairs.len())),
        Ok(o) => Outcome::Failed(format!("pw-cli set-param: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Outcome::Failed(format!("pw-cli: {e}")),
    }
}

/// Put the chain's config in place at startup, so the sink exists without
/// anyone running a script or touching a slider first.
///
/// Writes only when the file is absent or its content has changed, because
/// rewriting it every boot would churn a file PipeWire watches. Says what it
/// did; the caller decides whether that is worth a note.
pub fn ensure_speaker_chain() -> Outcome {
    let Some(path) = speaker_config_path() else {
        return Outcome::Failed("no config dir for the speaker chain".into());
    };
    let want = tideshell_core::audio::speaker_chain_config();
    if std::fs::read_to_string(&path).is_ok_and(|have| have == want) {
        return Outcome::Done(format!("speaker chain config already current at {}", path.display()));
    }
    if let Some(d) = path.parent() {
        let _ = std::fs::create_dir_all(d);
    }
    match std::fs::write(&path, &want) {
        // Not restarting PipeWire here on purpose: killing audio out from
        // under whatever is playing, at login, to install a sink nobody has
        // asked for yet, would be a worse bug than a sink that appears next
        // boot.
        Ok(()) => Outcome::Done(format!("speaker chain written to {}; it appears after `systemctl --user restart pipewire`", path.display())),
        Err(e) => Outcome::Failed(format!("cannot write {}: {e}", path.display())),
    }
}

/// Point the speaker chain's matrix at the screen's current orientation.
///
/// The same rotation that turns the display moves the audio, because what
/// matters is where the listener thinks left is — and on this phone the two
/// speakers are a top earpiece and a bottom main driver, so upright they are
/// not a stereo pair at all and the right answer is mono into both with the
/// bass going where bass can be made (speaker_pose::matrix_for).
///
/// Sent as one Props message: two gains per mixer, plus the earpiece's
/// high-pass corner, so a rotation cannot be seen half-applied.
pub fn apply_speaker_pose(m: &speaker_pose::Matrix, crossover_hz: f32) -> Outcome {
    let listing = match Command::new("pw-cli").args(["ls", "Node"]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Outcome::Failed(format!("pw-cli ls: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Outcome::Failed(format!("pw-cli: {e}")),
    };
    let Some(id) = parse_node_id(&listing, tideshell_core::audio::SPEAKER_SINK) else {
        return Outcome::Unwired("speaker chain not running (set the warmth slider once to install it)");
    };
    let props = speaker_pose_props(m, crossover_hz);
    match Command::new("pw-cli").args(["set-param", &id.to_string(), "Props", &props]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Outcome::Done(format!(
            "speakers: {} (top {:.2}/{:.2}, bottom {:.2}/{:.2})",
            if m.mono { "mono, bass to the main driver" } else { "stereo" },
            m.top.from_left, m.top.from_right, m.bottom.from_left, m.bottom.from_right
        )),
        Ok(o) => Outcome::Failed(format!("pw-cli set-param: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Outcome::Failed(format!("pw-cli: {e}")),
    }
}

/// The Props text for a routing matrix. Gains carry the per-speaker trim,
/// since the mixer is the only place in the graph with a level control.
pub fn speaker_pose_props(m: &speaker_pose::Matrix, crossover_hz: f32) -> String {
    let items: Vec<(String, f32)> = vec![
        ("mixTop:Gain 1".into(), m.top.from_left * m.top.gain),
        ("mixTop:Gain 2".into(), m.top.from_right * m.top.gain),
        ("mixBot:Gain 1".into(), m.bottom.from_left * m.bottom.gain),
        ("mixBot:Gain 2".into(), m.bottom.from_right * m.bottom.gain),
        // The high-pass follows the SPEAKER, so it stays on the earpiece in
        // every orientation; only its corner is configurable.
        ("hpTop:Freq".into(), if m.top.high_pass_hz > 0.0 { m.top.high_pass_hz } else { crossover_hz }),
    ];
    let body: Vec<String> = items.iter().map(|(k, v)| format!("\"{k}\" {v:.4}")).collect();
    format!("{{ params = [ {} ] }}", body.join(" "))
}

/// Where the speaker chain's config lives.
pub fn speaker_config_path() -> Option<PathBuf> {
    std::env::var_os("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
        .map(|c| c.join("pipewire/pipewire.conf.d/tideshell-speaker.conf"))
}

/// Move the speaker chain's controls. If the sink is not running, write the
/// config and say what is missing rather than failing silently — the chain
/// needs plugins that are not builtin (bankstown as LV2, swh-plugins for
/// the compressor and limiter), so "not running" usually means "not
/// installed", and that is worth saying out loud.
pub fn apply_warmth(x: f32) -> Outcome {
    let w = tideshell_core::audio::warmth(x);
    let listing = match Command::new("pw-cli").args(["ls", "Node"]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Outcome::Failed(format!("pw-cli ls: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Outcome::Failed(format!("pw-cli: {e} (PipeWire tools not installed)")),
    };
    let Some(id) = parse_node_id(&listing, tideshell_core::audio::SPEAKER_SINK) else {
        return match speaker_config_path() {
            Some(path) if path.exists() => Outcome::Failed(format!(
                "speaker chain not running; config is at {}. It needs an LV2 bankstown bundle (lv2ls | grep bankstown) and swh-plugins, then: systemctl --user restart pipewire",
                path.display()
            )),
            Some(path) => {
                if let Some(d) = path.parent() {
                    let _ = std::fs::create_dir_all(d);
                }
                match std::fs::write(&path, tideshell_core::audio::speaker_chain_config()) {
                    Ok(()) => Outcome::Failed(format!("speaker chain installed at {}; install bankstown (LV2) and swh-plugins, then restart pipewire", path.display())),
                    Err(e) => Outcome::Failed(format!("cannot write {}: {e}", path.display())),
                }
            }
            None => Outcome::Failed("no config dir for the speaker chain".into()),
        };
    };
    let props = tideshell_core::audio::warmth_props_text(&w);
    match Command::new("pw-cli").args(["set-param", &id.to_string(), "Props", &props]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Outcome::Done(format!(
            "warmth {x:.2}: harmonics {:.2}, compression {:.1}:1, limit {:.0} dB",
            w.bass_amt, w.comp_ratio, w.limit_db
        )),
        Ok(o) => Outcome::Failed(format!("pw-cli set-param: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Outcome::Failed(format!("pw-cli: {e}")),
    }
}

// ---- volume (PipeWire via wpctl) -----------------------------------------------

pub const FADE_STEPS: usize = 6;
pub const FADE_STEP_MS: u64 = 40;

/// `wpctl get-volume` prints "Volume: 0.45" (with " [MUTED]" when muted).
pub fn parse_wpctl_volume(s: &str) -> Option<f32> {
    s.split_whitespace().skip_while(|w| *w != "Volume:").nth(1).and_then(|v| v.parse().ok())
}

/// The levels to set, one per step, ending exactly on `to`. A fade instead
/// of one jump: the volume keys step in 5 % increments and a jump of 5 % is
/// audible as a bump; six 40 ms steps are not.
pub fn fade_steps(from: f32, to: f32, n: usize) -> Vec<f32> {
    (1..=n).map(|i| from + (to - from) * i as f32 / n as f32).map(|v| (v * 1000.0).round() / 1000.0).collect()
}

/// The command-line tools that can set the default sink's volume, tried in this order.
///
/// `wpctl` talks to WirePlumber/PipeWire and was the only one used — but the Fairphone 5 here runs
/// PulseAudio (`postmarketos-base-ui-audio-backend-pulseaudio`; WirePlumber and PipeWire are not
/// running), so every slider and hardware volume key failed at its first call. `pactl` speaks the
/// PulseAudio protocol that PulseAudio AND pipewire-pulse both serve, so it goes first and
/// `wpctl` stays as the fallback for a bare PipeWire session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VolumeTool { Pactl, Wpctl }

const VOLUME_TOOLS: [VolumeTool; 2] = [VolumeTool::Pactl, VolumeTool::Wpctl];

pub fn volume_get_cmd(t: VolumeTool) -> (&'static str, Vec<String>) {
    match t {
        VolumeTool::Pactl => ("pactl", vec!["get-sink-volume".into(), "@DEFAULT_SINK@".into()]),
        VolumeTool::Wpctl => ("wpctl", vec!["get-volume".into(), "@DEFAULT_AUDIO_SINK@".into()]),
    }
}

pub fn volume_set_cmd(t: VolumeTool, level: f32) -> (&'static str, Vec<String>) {
    match t {
        VolumeTool::Pactl => ("pactl", vec!["set-sink-volume".into(), "@DEFAULT_SINK@".into(), format!("{}%", (level.max(0.0) * 100.0).round() as u32)]),
        VolumeTool::Wpctl => ("wpctl", vec!["set-volume".into(), "@DEFAULT_AUDIO_SINK@".into(), format!("{level:.3}")]),
    }
}

/// `pactl get-sink-volume` prints `Volume: front-left: 24904 /  38% / -25.21 dB, front-right: …`;
/// the first percentage on the `Volume:` line, as a fraction.
pub fn parse_pactl_volume(s: &str) -> Option<f32> {
    let line = s.lines().find(|l| l.trim_start().starts_with("Volume:"))?;
    line.split_whitespace().find_map(|w| w.strip_suffix('%')?.parse::<f32>().ok()).map(|p| p / 100.0)
}

pub fn parse_volume(t: VolumeTool, s: &str) -> Option<f32> {
    match t { VolumeTool::Pactl => parse_pactl_volume(s), VolumeTool::Wpctl => parse_wpctl_volume(s) }
}

type RunTool<'a> = &'a dyn Fn(&str, &[String]) -> Result<String, String>;

/// The current volume and the tool that read it: the first of [`VOLUME_TOOLS`] that answers.
pub fn read_volume_with(run: RunTool) -> Result<(VolumeTool, f32), String> {
    let mut errors = Vec::new();
    for t in VOLUME_TOOLS {
        let (prog, args) = volume_get_cmd(t);
        match run(prog, &args) {
            Ok(out) => match parse_volume(t, &out) {
                Some(v) => return Ok((t, v)),
                None => errors.push(format!("{prog}: unreadable output {:?}", out.trim())),
            },
            Err(e) => errors.push(e),
        }
    }
    Err(errors.join("; "))
}

fn run_tool(prog: &str, args: &[String]) -> Result<String, String> {
    let out = Command::new(prog).args(args).stdin(Stdio::null()).output().map_err(|e| format!("{prog}: {e}"))?;
    if out.status.success() {
        Ok(String::from_utf8_lossy(&out.stdout).into_owned())
    } else {
        Err(format!("{prog} {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim()))
    }
}

/// Fade the default sink to `to` (0..1) on a thread; returns at once.
pub fn fade_volume(to: f32) -> Outcome {
    let to = to.clamp(0.0, 1.0);
    let (tool, from) = match read_volume_with(&run_tool) {
        Ok(r) => r,
        Err(e) => return Outcome::Failed(e),
    };
    std::thread::spawn(move || {
        for v in fade_steps(from, to, FADE_STEPS) {
            let (prog, args) = volume_set_cmd(tool, v);
            let _ = run_tool(prog, &args);
            std::thread::sleep(std::time::Duration::from_millis(FADE_STEP_MS));
        }
    });
    Outcome::Done(format!("volume fade {from:.2} -> {to:.2} via {tool:?}"))
}

// ---- the quick toggles: wifi, bluetooth, airplane, mobile data, flashlight ------------

/// The commands behind a system quick tile, or `None` when it is not a system call (shell-owned
/// modes) or has no backend yet. Pure: what to run, not running it.
///
/// Bluetooth goes through `bluetoothctl`, not `rfkill`: `/dev/rfkill` is root-write on the phone, so
/// `rfkill block bluetooth` would fail for the user the shell runs as. NetworkManager's `nmcli radio`
/// needs no privilege. Airplane means EVERY radio off, so it also powers bluetooth down.
pub fn quick_commands(kind: tideshell_core::control_center::QuickKind, on: bool) -> Option<Vec<(&'static str, Vec<String>)>> {
    use tideshell_core::control_center::QuickKind as K;
    let word = |b: bool| if b { "on" } else { "off" }.to_string();
    let nm = |what: &str, state: String| ("nmcli", vec!["radio".to_string(), what.to_string(), state]);
    let bt = |state: String| ("bluetoothctl", vec!["power".to_string(), state]);
    match kind {
        K::Wifi => Some(vec![nm("wifi", word(on))]),
        K::MobileData => Some(vec![nm("wwan", word(on))]),
        K::Bluetooth => Some(vec![bt(word(on))]),
        K::Airplane => Some(vec![nm("all", word(!on)), bt(word(!on))]),
        _ => None,
    }
}

/// The flashlight: the first LED under `base` (`/sys/class/leds`) whose name says flash or torch.
pub fn flashlight_led(base: &std::path::Path) -> Option<PathBuf> {
    let mut leds: Vec<PathBuf> = std::fs::read_dir(base).ok()?.flatten().map(|e| e.path()).collect();
    leds.sort();
    leds.into_iter().find(|p| {
        let n = p.file_name().map(|n| n.to_string_lossy().to_ascii_lowercase()).unwrap_or_default();
        (n.contains("flash") || n.contains("torch")) && p.join("brightness").exists()
    })
}

/// Light or darken the flashlight: `max_brightness` for on, 0 for off. The FP5's `white:flash` LED is
/// writable by the logged-in user, so no privilege is needed.
pub fn set_flashlight(base: &std::path::Path, on: bool) -> Outcome {
    let Some(led) = flashlight_led(base) else { return Outcome::Failed(format!("no flash/torch LED under {}", base.display())) };
    let level = if on {
        match std::fs::read_to_string(led.join("max_brightness")).ok().and_then(|s| s.trim().parse::<u32>().ok()) {
            Some(m) if m > 0 => m,
            _ => return Outcome::Failed(format!("{}: unreadable max_brightness", led.display())),
        }
    } else { 0 };
    match std::fs::write(led.join("brightness"), level.to_string()) {
        Ok(()) => Outcome::Done(format!("{} -> {level}", led.display())),
        Err(e) => Outcome::Failed(format!("{}: {e}", led.join("brightness").display())),
    }
}

fn apply_quick(kind: tideshell_core::control_center::QuickKind, on: bool) -> Outcome {
    use tideshell_core::control_center::QuickKind as K;
    if kind == K::Flashlight { return set_flashlight(std::path::Path::new("/sys/class/leds"), on); }
    let Some(cmds) = quick_commands(kind, on) else {
        return match kind {
            K::DoNotDisturb => Outcome::Unwired("do-not-disturb: cosmic has no switch to drive yet"),
            _ => Outcome::Unwired("this tile is owned by the shell, not a system call"),
        };
    };
    let mut done = Vec::new();
    for (prog, args) in cmds {
        match run_tool(prog, &args) {
            Ok(_) => done.push(format!("{prog} {}", args.join(" "))),
            Err(e) => return Outcome::Failed(e),
        }
    }
    Outcome::Done(done.join("; "))
}

// ---- output rotation (wlr-randr for now) ------------------------------------------

/// The first output's name from `wlr-randr`'s listing (a header line per
/// output: `NAME "description"`; detail lines are indented).
pub fn first_output_name(listing: &str) -> Option<String> {
    listing.lines().find(|l| !l.starts_with(' ') && !l.trim().is_empty()).and_then(|l| l.split_whitespace().next()).map(str::to_string)
}

/// Rotate the (first) output. `wlr-randr` is the transport until the
/// wlr-output-management client lands in the layer runtime.
pub fn rotate_output(deg: u32) -> Outcome {
    let list = match Command::new("wlr-randr").stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => String::from_utf8_lossy(&o.stdout).into_owned(),
        Ok(o) => return Outcome::Failed(format!("wlr-randr: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => return Outcome::Failed(format!("wlr-randr: {e} (apk add wlr-randr)")),
    };
    let Some(name) = first_output_name(&list) else { return Outcome::Failed("wlr-randr listed no output".into()) };
    let transform = match deg % 360 {
        0 => "normal",
        90 => "90",
        180 => "180",
        _ => "270",
    };
    match Command::new("wlr-randr").args(["--output", &name, "--transform", transform]).stdin(Stdio::null()).output() {
        Ok(o) if o.status.success() => Outcome::Done(format!("rotated {name} to {transform}")),
        Ok(o) => Outcome::Failed(format!("wlr-randr --transform: {}", String::from_utf8_lossy(&o.stderr).trim())),
        Err(e) => Outcome::Failed(format!("wlr-randr: {e}")),
    }
}

// ---- prefs file ------------------------------------------------------------

/// `$XDG_CONFIG_HOME/tideshell/prefs`, flat `key = value` lines — the same
/// pairs `ShellPrefs::to_kv` hands settingsd, so moving to settingsd later is
/// a transport swap, not a format change.
/// The fleet settings file settingsd persists (bsettings writes through it):
/// `$XDG_CONFIG_HOME/gitmind/settings.conf`, `k = v` lines.
/// One datagram to rusty-board-egui's control socket.
pub fn send_keyboard_command(cmd: &str) -> Result<(), String> {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").ok_or("XDG_RUNTIME_DIR is not set")?;
    let path = PathBuf::from(dir).join("rusty-board.ctl");
    let sock = std::os::unix::net::UnixDatagram::unbound().map_err(|e| e.to_string())?;
    sock.send_to(cmd.as_bytes(), &path).map_err(|e| format!("{}: {e}", path.display()))?;
    Ok(())
}

pub fn fleet_settings_path() -> Option<PathBuf> {
    let base = std::env::var_os("XDG_CONFIG_HOME").map(PathBuf::from).or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))?;
    Some(base.join("gitmind").join("settings.conf"))
}

/// Sleeping hours from the fleet settings text (defaults: off).
pub fn parse_fleet_sleep(text: &str) -> settings_kit::SleepPrefs {
    let kv: Vec<(String, String)> = text
        .lines()
        .filter_map(|l| {
            let l = l.trim();
            if l.starts_with('#') {
                return None;
            }
            let (k, v) = l.split_once('=')?;
            Some((k.trim().to_string(), v.trim().to_string()))
        })
        .collect();
    settings_kit::Prefs::from_kv(&kv).sleep
}

/// Re-read the fleet sleeping hours if the file changed since `seen`
/// (its mtime). Returns the new value and mtime when it did.
pub fn reload_fleet_sleep(seen: Option<std::time::SystemTime>) -> Option<(settings_kit::SleepPrefs, Option<std::time::SystemTime>)> {
    let p = fleet_settings_path()?;
    let mtime = std::fs::metadata(&p).and_then(|m| m.modified()).ok();
    if mtime.is_some() && mtime == seen {
        return None;
    }
    let text = std::fs::read_to_string(&p).unwrap_or_default();
    Some((parse_fleet_sleep(&text), mtime))
}

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
mod fleet_sleep_tests {
    use super::*;

    #[test]
    fn sleeping_hours_are_read_from_settingsd_text() {
        let s = parse_fleet_sleep("# settingsd\ntheme.palette = Dark\nsleep.hours = 22:45-06:30\nsleep.pulse = true\n");
        assert!(s.enabled && s.pulse && s.start_min == 22 * 60 + 45 && s.end_min == 6 * 60 + 30);
        assert!(!parse_fleet_sleep("").enabled, "no file, no window");
    }
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
    fn apps_theme_flip_restores_exactly() {
        let base = std::env::temp_dir().join(format!("tideshell-theme-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        let path = theme_mode_path(&base);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, "true").unwrap();
        assert!(matches!(set_apps_light_theme(&base, true), Outcome::Done(_)));
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "false");
        set_apps_light_theme(&base, true); // twice: the saved original stays "true"
        set_apps_light_theme(&base, false);
        assert_eq!(std::fs::read_to_string(&path).unwrap(), "true");
        std::fs::remove_file(&path).unwrap();
        set_apps_light_theme(&base, true);
        set_apps_light_theme(&base, false);
        assert!(!path.exists(), "no key before: no key after");
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
        let mut sys = System { launcher: None, pending: Vec::new(), session_bus: None };
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
        let mut sys = System { launcher: None, pending: Vec::new(), session_bus: None };
        sys.start(&["sleep", "5"], true);
        assert!(sys.pending.is_empty());
    }

    #[test]
    fn wlr_randr_output_name_is_the_first_unindented_line() {
        let listing = "DSI-1 \"Unknown\"\n  Make: Unknown\n  Enabled: yes\n  Modes:\n    1224x2700 px, 60.000000 Hz (preferred, current)\n  Position: 0,0\n  Transform: normal\n";
        assert_eq!(first_output_name(listing).as_deref(), Some("DSI-1"));
        assert_eq!(first_output_name("  only indented\n"), None);
    }

    #[test]
    fn scope_names_are_safe_and_carry_the_app_id() {
        assert_eq!(scope_unit_name("org.gnome.Console", 4242), "app-tideshell-org.gnome.Console-4242.scope");
        assert_eq!(scope_unit_name("weird name/with:chars", 1), "app-tideshell-weird_name_with_chars-1.scope");
    }

    /// StartTransientUnit against a mock systemd1 on a private bus.
    #[test]
    fn launched_apps_are_moved_into_their_own_scope() {
        use std::sync::{Arc, Mutex};
        struct Mock(Arc<Mutex<Vec<(String, String, Vec<String>)>>>);
        #[zbus::interface(name = "org.freedesktop.systemd1.Manager")]
        impl Mock {
            fn start_transient_unit(&self, name: String, mode: String, properties: Vec<(String, zbus::zvariant::OwnedValue)>, _aux: Vec<(String, Vec<(String, zbus::zvariant::OwnedValue)>)>) -> zbus::zvariant::OwnedObjectPath {
                let keys = properties.iter().map(|(k, _)| k.clone()).collect();
                self.0.lock().unwrap().push((name, mode, keys));
                zbus::zvariant::OwnedObjectPath::try_from("/org/freedesktop/systemd1/job/1").unwrap()
            }
        }
        let sock = std::env::temp_dir().join(format!("tideshell-sd-{}", std::process::id()));
        let _ = std::fs::remove_file(&sock);
        let addr = format!("unix:path={}", sock.display());
        let Ok(mut daemon) = std::process::Command::new("dbus-daemon").args(["--session", "--nofork", "--nopidfile", &format!("--address={addr}")]).stdout(Stdio::null()).stderr(Stdio::null()).spawn() else {
            eprintln!("dbus-daemon not available: skipping");
            return;
        };
        for _ in 0..50 {
            if sock.exists() {
                break;
            }
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let calls = Arc::new(Mutex::new(Vec::new()));
        let _server = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().name("org.freedesktop.systemd1").unwrap().serve_at("/org/freedesktop/systemd1", Mock(calls.clone())).unwrap().build().unwrap();
        let client = zbus::blocking::connection::Builder::address(addr.as_str()).unwrap().build().unwrap();
        let unit = move_to_scope(&client, 777, "org.gnome.Console").unwrap();
        assert_eq!(unit, "app-tideshell-org.gnome.Console-777.scope");
        let c = calls.lock().unwrap();
        assert_eq!(c.len(), 1);
        assert_eq!((c[0].0.as_str(), c[0].1.as_str()), (unit.as_str(), "fail"));
        assert!(c[0].2.contains(&"PIDs".to_string()) && c[0].2.contains(&"Slice".to_string()), "{:?}", c[0].2);
        let _ = daemon.kill();
    }

    #[test]
    fn pw_cli_node_listing_yields_the_voicing_sink_id() {
        let listing = "\tid 31, type PipeWire:Interface:Node/3\n \t\tobject.serial = \"31\"\n \t\tnode.name = \"alsa_output.platform-sound.stereo-fallback\"\n\tid 47, type PipeWire:Interface:Node/3\n \t\tnode.name = \"tideshell-voicing\"\n \t\tmedia.class = \"Audio/Sink\"\n\tid 48, type PipeWire:Interface:Node/3\n \t\tnode.name = \"tideshell-voicing.out\"\n";
        assert_eq!(parse_node_id(listing, "tideshell-voicing"), Some(47), "the sink, not its .out playback node");
        assert_eq!(parse_node_id(listing, "nope"), None);
    }

    #[test]
    fn volume_fade_math() {
        assert_eq!(parse_wpctl_volume("Volume: 0.45\n"), Some(0.45));
        assert_eq!(parse_wpctl_volume("Volume: 0.60 [MUTED]"), Some(0.6));
        assert_eq!(parse_wpctl_volume("garbage"), None);
        let steps = fade_steps(0.40, 0.45, 6);
        assert_eq!(steps.len(), 6);
        assert!((steps[5] - 0.45).abs() < 1e-6, "ends exactly on the target");
        assert!(steps.windows(2).all(|w| w[1] > w[0]) && steps[0] > 0.40, "monotonic, every step moves");
        assert!(fade_steps(0.5, 0.45, 6).windows(2).all(|w| w[1] < w[0]), "down too");
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
        let mut sys = System { launcher: Some(hatch_core::Launcher::with_entries(hatch_core::Config::default(), vec![entry])), pending: Vec::new(), session_bus: None };
        match sys.run(&Action::LaunchApp("t".into())) {
            Outcome::Done(s) => assert!(s.starts_with("launched t")),
            o => panic!("{o:?}"),
        }
        assert!(matches!(sys.run(&Action::LaunchApp("missing".into())), Outcome::Failed(_)));
    }

    // ---- the apps button must find hatch under the name the phone actually has ----

    fn bin_dir(tag: &str, files: &[&str]) -> String {
        let d = std::env::temp_dir().join(format!("tideshell-launcher-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&d);
        std::fs::create_dir_all(&d).unwrap();
        for f in files { std::fs::write(d.join(f), b"#!/bin/sh\n").unwrap(); }
        d.to_string_lossy().into_owned()
    }

    #[test]
    fn a_phone_with_only_hatch_summons_hatch() {
        // The FP5 as found: ~/.local/bin/hatch, no hatch-egui.
        let d = bin_dir("only-hatch", &["hatch"]);
        assert_eq!(pick_launcher(&format!("/nonexistent:{d}"), None), "hatch");
    }

    #[test]
    fn hatch_egui_is_preferred_when_both_exist() {
        let d = bin_dir("both", &["hatch", "hatch-egui"]);
        assert_eq!(pick_launcher(&d, None), "hatch-egui");
    }

    #[test]
    fn neither_found_falls_back_to_the_name_the_shell_expects() {
        let d = bin_dir("none", &["something-else"]);
        assert_eq!(pick_launcher(&d, None), "hatch-egui");
        assert_eq!(pick_launcher("", None), "hatch-egui");
    }

    #[test]
    fn a_directory_called_hatch_is_not_a_launcher_and_the_override_wins() {
        let d = bin_dir("dir", &[]);
        std::fs::create_dir_all(std::path::Path::new(&d).join("hatch")).unwrap();
        assert_eq!(pick_launcher(&d, None), "hatch-egui", "a directory is not a program");
        let e = bin_dir("ovr", &["hatch", "hatch-egui"]);
        assert_eq!(pick_launcher(&e, Some("/opt/my-hatch")), "/opt/my-hatch");
        assert_eq!(pick_launcher(&e, Some("  ")), "hatch-egui", "a blank override is no override");
    }

    // ---- the sidebar and the volume keys: what was dead, and why ----

    use tideshell_core::control_center::QuickKind;
    use std::cell::RefCell;

    #[test]
    fn pactl_output_from_the_phone_parses_to_a_fraction() {
        // Captured from the FP5's live PulseAudio server.
        let real = "Volume: front-left: 24904 /  38% / -25.21 dB,   front-right: 24904 /  38% / -25.21 dB\n        balance 0.00\n";
        assert!((parse_pactl_volume(real).unwrap() - 0.38).abs() < 1e-6);
        assert_eq!(parse_pactl_volume("Volume: mono: 65536 / 100% / 0.00 dB"), Some(1.0));
        assert!((parse_pactl_volume("Volume: front-left: 78643 / 120% / 4.66 dB").unwrap() - 1.2).abs() < 1e-6, "over 100% is real and must not be clamped away");
        assert_eq!(parse_pactl_volume("Connection failure: Connection refused"), None);
        assert_eq!(parse_pactl_volume(""), None);
        assert_eq!(parse_pactl_volume("balance 50%"), None, "only the Volume: line counts");
    }

    #[test]
    fn the_volume_set_commands_are_what_each_tool_expects() {
        let set = |t, v: f32| { let (p, a) = volume_set_cmd(t, v); format!("{p} {}", a.join(" ")) };
        assert_eq!(set(VolumeTool::Pactl, 0.55), "pactl set-sink-volume @DEFAULT_SINK@ 55%");
        assert_eq!(set(VolumeTool::Pactl, 0.0), "pactl set-sink-volume @DEFAULT_SINK@ 0%");
        assert_eq!(set(VolumeTool::Pactl, 1.0), "pactl set-sink-volume @DEFAULT_SINK@ 100%");
        assert_eq!(set(VolumeTool::Pactl, 0.555), "pactl set-sink-volume @DEFAULT_SINK@ 56%");
        assert_eq!(set(VolumeTool::Pactl, -0.2), "pactl set-sink-volume @DEFAULT_SINK@ 0%", "never a negative percentage");
        assert_eq!(set(VolumeTool::Wpctl, 0.5), "wpctl set-volume @DEFAULT_AUDIO_SINK@ 0.500");
    }

    #[test]
    fn pulseaudio_phone_volume_works_when_wpctl_cannot_connect() {
        // The reported bug: the shell only ever ran wpctl, which needs PipeWire/WirePlumber; this phone
        // runs PulseAudio, so the slider and the hardware volume keys failed at the first call.
        let calls = RefCell::new(Vec::new());
        let run = |prog: &str, _: &[String]| -> Result<String, String> {
            calls.borrow_mut().push(prog.to_string());
            match prog {
                "pactl" => Ok("Volume: front-left: 24904 /  38% / -25.21 dB\n".into()),
                _ => Err("wpctl: Could not connect to PipeWire".into()),
            }
        };
        let (tool, v) = read_volume_with(&run).expect("must read the volume from pactl");
        assert_eq!(tool, VolumeTool::Pactl);
        assert!((v - 0.38).abs() < 1e-6);
        assert_eq!(*calls.borrow(), vec!["pactl".to_string()], "pactl first, and wpctl is never touched when it works");
    }

    #[test]
    fn a_pipewire_only_session_falls_back_to_wpctl_and_a_dead_audio_server_says_why() {
        let pipewire_only = |prog: &str, _: &[String]| -> Result<String, String> {
            if prog == "wpctl" { Ok("Volume: 0.45\n".into()) } else { Err("pactl: not found".into()) }
        };
        assert_eq!(read_volume_with(&pipewire_only), Ok((VolumeTool::Wpctl, 0.45)));
        let unreadable = |prog: &str, _: &[String]| -> Result<String, String> {
            if prog == "pactl" { Ok("garbage".into()) } else { Ok("Volume: 0.2".into()) }
        };
        assert_eq!(read_volume_with(&unreadable).map(|(t, _)| t), Ok(VolumeTool::Wpctl), "unparseable output is a failure of that tool");
        let dead = |prog: &str, _: &[String]| -> Result<String, String> { Err(format!("{prog}: connection refused")) };
        let e = read_volume_with(&dead).unwrap_err();
        assert!(e.contains("pactl") && e.contains("wpctl"), "both failures must be reported: {e}");
    }

    #[test]
    fn the_quick_tiles_map_to_privilege_free_commands() {
        let cmd = |k, on| quick_commands(k, on).map(|v| v.into_iter().map(|(p, a)| format!("{p} {}", a.join(" "))).collect::<Vec<_>>());
        assert_eq!(cmd(QuickKind::Wifi, true), Some(vec!["nmcli radio wifi on".to_string()]));
        assert_eq!(cmd(QuickKind::Wifi, false), Some(vec!["nmcli radio wifi off".to_string()]));
        assert_eq!(cmd(QuickKind::MobileData, false), Some(vec!["nmcli radio wwan off".to_string()]));
        assert_eq!(cmd(QuickKind::Bluetooth, true), Some(vec!["bluetoothctl power on".to_string()]));
        // /dev/rfkill is root-write on the phone: rfkill would fail for the user the shell runs as.
        for on in [true, false] {
            let all = quick_commands(QuickKind::Bluetooth, on).unwrap();
            assert!(all.iter().all(|(p, _)| *p != "rfkill"), "rfkill needs root here");
        }
        // Airplane ON means every radio OFF — the inversion is the whole point of the tile.
        assert_eq!(cmd(QuickKind::Airplane, true), Some(vec!["nmcli radio all off".to_string(), "bluetoothctl power off".to_string()]));
        assert_eq!(cmd(QuickKind::Airplane, false), Some(vec!["nmcli radio all on".to_string(), "bluetoothctl power on".to_string()]));
        for k in [QuickKind::Flashlight, QuickKind::Rotation, QuickKind::DoNotDisturb, QuickKind::Keyboard, QuickKind::CpuGovernor] {
            assert_eq!(quick_commands(k, true), None, "{k:?} is not a command-line toggle");
        }
    }

    fn fake_leds(tag: &str, leds: &[(&str, Option<&str>)]) -> PathBuf {
        let base = std::env::temp_dir().join(format!("tideshell-leds-{}-{tag}", std::process::id()));
        let _ = std::fs::remove_dir_all(&base);
        for (name, max) in leds {
            let d = base.join(name);
            std::fs::create_dir_all(&d).unwrap();
            std::fs::write(d.join("brightness"), "0").unwrap();
            if let Some(m) = max { std::fs::write(d.join("max_brightness"), m).unwrap(); }
        }
        base
    }

    #[test]
    fn the_flashlight_lights_to_max_and_goes_dark_and_ignores_other_leds() {
        // Names as found on the FP5: `mmc2::` (storage activity) and `white:flash`.
        let base = fake_leds("ok", &[("mmc2::", Some("1")), ("white:flash", Some("255"))]);
        assert!(matches!(set_flashlight(&base, true), Outcome::Done(_)));
        assert_eq!(std::fs::read_to_string(base.join("white:flash/brightness")).unwrap(), "255");
        assert_eq!(std::fs::read_to_string(base.join("mmc2::/brightness")).unwrap(), "0", "the storage LED is not a torch");
        assert!(matches!(set_flashlight(&base, false), Outcome::Done(_)));
        assert_eq!(std::fs::read_to_string(base.join("white:flash/brightness")).unwrap(), "0");
    }

    #[test]
    fn a_missing_or_unreadable_flash_led_is_a_failure_with_a_reason_not_a_silent_no_op() {
        let none = fake_leds("none", &[("mmc2::", Some("1"))]);
        assert!(matches!(set_flashlight(&none, true), Outcome::Failed(m) if m.contains("no flash/torch LED")));
        let bad = fake_leds("bad", &[("white:flash", Some("not-a-number"))]);
        assert!(matches!(set_flashlight(&bad, true), Outcome::Failed(m) if m.contains("max_brightness")));
        assert!(matches!(set_flashlight(&bad, false), Outcome::Done(_)), "turning it OFF needs no max_brightness");
        assert!(matches!(set_flashlight(std::path::Path::new("/nonexistent/leds"), true), Outcome::Failed(_)));
        assert_eq!(flashlight_led(&fake_leds("torchname", &[("led:torch_0", Some("1"))])).map(|p| p.file_name().unwrap().to_string_lossy().into_owned()), Some("led:torch_0".into()));
    }
}

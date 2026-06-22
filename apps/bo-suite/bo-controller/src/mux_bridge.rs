// src/mux_bridge.rs
//
// Bridges the Qt GUI to the headless bo-muxd daemon. QML can't open a Unix
// socket directly, so this QObject does it: it speaks the daemon's NDJSON
// control protocol over $XDG_RUNTIME_DIR/bo-mux/control.sock and exposes
// daemon state as Qt properties + actions as Qt methods.
//
// It is intentionally self-contained (re-declares the tiny wire types) so
// the controller crate doesn't need a hard dependency on the bo-mux crate
// to build — the daemon can be installed/updated independently. If you'd
// rather share types, swap these for `bo_mux::api::*`.
//
// Robustness: every daemon call is best-effort. If the socket is absent
// (daemon not installed / not started), calls fail quietly and `daemon_up`
// stays false so the UI shows the install card.

use qmetaobject::*;
use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::process::{Command as ProcCommand, Stdio};
use std::sync::mpsc::{channel, Receiver, TryRecvError};
use std::thread;

/// Internal stream message produced by the background script reader.
/// `Line` carries a single output line (stdout or stderr; merged in
/// arrival order). `Done` is the terminal message and carries the
/// child's exit status. The receiver lives in the MuxBridge and is
/// drained by the QML-driven `poll_setup` method.
enum SetupMsg {
    Line(String),
    Done(bool),
}

/// Version this GUI build ships — compared against the installed daemon's
/// version to drive the "update available" UI. Keep in sync with the
/// bo-mux crate version you bundle.
const SHIPPING_VERSION: &str = "0.1.0";

fn socket_path() -> PathBuf {
    let base = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    base.join("bo-mux/control.sock")
}

fn version_stamp_path() -> PathBuf {
    let base = std::env::var_os("XDG_DATA_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".local/share")))
        .unwrap_or_else(|| PathBuf::from("."));
    base.join("bo-mux/version")
}

/// Send one NDJSON request, return the response line (or None on any error).
///
/// IMPORTANT: this is called from the QML thread (via qt_method →
/// MuxBridge::call → daemon_call). The previous implementation set no
/// read or write timeout, meaning if the daemon ever accepted a
/// connection but didn't reply (deadlock, paused under a debugger,
/// busy doing slow I/O), the GUI would freeze. We set tight timeouts
/// here so a hung daemon manifests as an empty refresh result, NOT a
/// frozen UI. The controller's normal refresh tick will re-poll
/// shortly; transient unavailability is harmless.
///
/// UnixStream::connect itself is essentially instantaneous on Linux
/// (kernel-local, no DNS, no TCP handshake) — the lurking hang risk
/// is the read after sending the request, not the connect. We set a
/// 1-second read+write timeout: long enough for any reasonable
/// command (scene loads may stat files) but short enough that a
/// truly hung daemon doesn't lock up the UI.
fn daemon_call(req: &serde_json::Value) -> Option<serde_json::Value> {
    use std::time::Duration;
    let stream = UnixStream::connect(socket_path()).ok()?;
    // Best-effort timeout setting; if the platform doesn't support
    // these (it does on Linux but be defensive), we silently proceed
    // without — better than failing the call outright.
    let _ = stream.set_read_timeout(Some(Duration::from_millis(1000)));
    let _ = stream.set_write_timeout(Some(Duration::from_millis(1000)));
    let mut w = stream.try_clone().ok()?;
    let mut line = serde_json::to_string(req).ok()?;
    line.push('\n');
    w.write_all(line.as_bytes()).ok()?;
    w.flush().ok()?;
    let want = req.get("id").and_then(|v| v.as_u64()).unwrap_or(0);
    let reader = BufReader::new(stream);
    for l in reader.lines() {
        let l = l.ok()?;
        if l.trim().is_empty() { continue; }
        if let Ok(v) = serde_json::from_str::<serde_json::Value>(&l) {
            // skip event lines (no "id"); match our response id
            if v.get("id").and_then(|x| x.as_u64()) == Some(want) {
                return Some(v);
            }
        }
    }
    None
}

#[derive(QObject, Default)]
pub struct MuxBridge {
    base: qt_base_class!(trait QObject),

    // ── Exposed state ───────────────────────────────────────────────
    daemon_up: qt_property!(bool; NOTIFY changed),
    daemon_status: qt_property!(String; NOTIFY changed),     // installed version or ""
    shipping_version: qt_property!(String; NOTIFY changed),
    scenes_json: qt_property!(String; NOTIFY changed),
    peers_json: qt_property!(String; NOTIFY changed),
    /// Full fabric snapshot for the drag-to-speaker UI: peers (each with
    /// their speakers), the current channel→speaker assignment, and
    /// unclaimed channels. "{}" when there's no mesh. Updated on each
    /// status refresh.
    fabric_json: qt_property!(String; NOTIFY changed),
    /// The loaded scene's layers as [{name, channel}], so the Spaces
    /// page can render draggable layer chips even before playback.
    layers_json: qt_property!(String; NOTIFY changed),
    playing: qt_property!(bool; NOTIFY changed),
    current_scene: qt_property!(String; NOTIFY changed),
    role: qt_property!(String; NOTIFY changed),
    /// True when bo-muxd has entered passive mode — it stopped
    /// spawning new soundscape clips because the controller went
    /// quiet, the user moved audio via KDE picker, BT was toggled
    /// off, or the daemon has been idle too long. The audio
    /// engine still ticks (so timers don't drift); only new
    /// voice_on spawns are suppressed. Any user action in the
    /// controller wakes the daemon back to active mode.
    passive: qt_property!(bool; NOTIFY changed),
    /// Most-recent reason for entering passive mode, suitable for
    /// surfacing in the UI as "Soundscape paused — idle for 12 min".
    /// Empty when active.
    passive_reason: qt_property!(String; NOTIFY changed),
    /// Battery awareness, mirrored from the daemon's Status `power`
    /// object. `on_battery` true means the device is unplugged;
    /// `battery_pct` is 0-100 (or -1 if unknown / no battery). The
    /// controller uses these to show a hint like "Soundscape paused to
    /// save battery (12%)" and to soften the offer to start a scene when
    /// running low on battery. Older daemons omit `power`, so these
    /// default to plugged-in / unknown and never gate anything.
    on_battery: qt_property!(bool; NOTIFY changed),
    battery_pct: qt_property!(i32; NOTIFY changed),
    changed: qt_signal!(),

    // ── Actions ─────────────────────────────────────────────────────
    refresh: qt_method!(fn(&mut self)),
    install: qt_method!(fn(&mut self)),
    update: qt_method!(fn(&mut self)),
    load_scene: qt_method!(fn(&mut self, name: String)),
    play: qt_method!(fn(&mut self)),
    stop: qt_method!(fn(&mut self)),
    start_session: qt_method!(fn(&mut self, layout: String)),
    leave_session: qt_method!(fn(&mut self)),
    /// Pin a soundscape layer's channel to a speaker on the fabric.
    /// Called by the drag-to-speaker gesture in the Spaces page.
    pin_layer: qt_method!(fn(&mut self, channel: String, speaker: String)),
    /// Push a new master gain to bo-muxd (range 0.0..1.5). The
    /// soundscape engine applies this multiplier to every new
    /// voice spawn; existing clips keep their spawn-time level
    /// until they end. Driven by the controller's noise-adaptive
    /// gain curve (see main.rs anc_standby_loudness_ratio binding).
    set_scene_gain: qt_method!(fn(&mut self, value: f64)),

    // ── Setup / diagnostics scripts (GUI-launchable) ────────────────
    // Output of the last script run, line by line (tagged STEP/OK/WARN/...).
    setup_log: qt_property!(String; NOTIFY setup_changed),
    setup_running: qt_property!(bool; NOTIFY setup_changed),
    setup_ok: qt_property!(bool; NOTIFY setup_changed),
    // Doctor results as JSON (the GUI renders a checklist).
    doctor_json: qt_property!(String; NOTIFY setup_changed),
    setup_changed: qt_signal!(),

    run_doctor: qt_method!(fn(&mut self)),
    run_install: qt_method!(fn(&mut self)),
    run_uninstall: qt_method!(fn(&mut self)),
    /// Revert bo-mux's audio side-effects to a clean baseline.
    /// Stops the daemon, reaps orphaned paplay/ffplay/aplay
    /// subprocesses spawned for clip playback, and (with
    /// `--restart-audio`) restarts the user-level pipewire
    /// services. Streams progress to setup_log.
    fix_audio: qt_method!(fn(&mut self)),
    /// Same, but also restarts pipewire / pipewire-pulse. Heavier
    /// hammer for when the lighter version didn't restore audio.
    fix_audio_full: qt_method!(fn(&mut self)),
    /// Drain the streaming-script channel into `setup_log`. QML
    /// calls this on a 200ms Timer while `setup_running` is true;
    /// it appends any pending lines and flips `setup_running` to
    /// false when the child process exits. Cheap when there's
    /// nothing to drain (one Option check, one try_recv, return).
    poll_setup: qt_method!(fn(&mut self)),

    next_id: u64,

    /// Receiver end of the streaming-script channel. None when no
    /// script is running. Drained by poll_setup; replaced on the
    /// next run_script_streaming call.
    setup_rx: Option<Receiver<SetupMsg>>,

    /// What kind of script is currently streaming, so poll_setup
    /// knows how to finalize on Done. Values: "doctor", "install",
    /// "uninstall", "fix_audio", "" (no current script).
    setup_kind: String,
}

impl MuxBridge {
    fn id(&mut self) -> u64 { self.next_id += 1; self.next_id }

    fn call(&mut self, cmd: &str, extra: serde_json::Value) -> Option<serde_json::Value> {
        let id = self.id();
        let mut req = serde_json::json!({ "id": id, "cmd": cmd });
        if let (Some(obj), Some(ex)) = (req.as_object_mut(), extra.as_object()) {
            for (k, v) in ex { obj.insert(k.clone(), v.clone()); }
        }
        daemon_call(&req)
    }

    /// Poll the daemon for status; if it's down, read the version stamp so
    /// we can still tell installed-but-stopped from never-installed.
    fn refresh(&mut self) {
        self.shipping_version = SHIPPING_VERSION.to_string();

        // Installed version from the on-disk stamp (works even if stopped).
        self.daemon_status = std::fs::read_to_string(version_stamp_path())
            .map(|s| s.trim().to_string())
            .unwrap_or_default();

        match self.call("status", serde_json::json!({})) {
            Some(resp) if resp.get("ok").and_then(|v| v.as_bool()) == Some(true) => {
                self.daemon_up = true;
                let data = resp.get("data").cloned().unwrap_or(serde_json::json!({}));
                self.playing = data.get("playing").and_then(|v| v.as_bool()).unwrap_or(false);
                self.current_scene = data.get("scene").and_then(|v| v.as_str()).unwrap_or("").to_string();
                self.role = data.get("role").and_then(|v| v.as_str()).unwrap_or("idle").to_string();
                // Activity-watch state added in the back-off work.
                // Older daemons (pre-back-off-work) omit these
                // fields; treat absence as "active mode" so the
                // controller doesn't render a spurious badge against
                // an older muxd that doesn't know about passive.
                self.passive = data.get("passive")
                    .and_then(|v| v.as_bool()).unwrap_or(false);
                self.passive_reason = data.get("passive_reason")
                    .and_then(|v| v.as_str())
                    .unwrap_or("")
                    .to_string();
                // Power state (added with the battery-saving work). The
                // daemon sends { on_battery, capacity, has_battery }.
                // Absent on older daemons → plugged-in / unknown.
                let power = data.get("power").cloned().unwrap_or(serde_json::json!({}));
                self.on_battery = power.get("on_battery")
                    .and_then(|v| v.as_bool()).unwrap_or(false);
                self.battery_pct = power.get("capacity")
                    .and_then(|v| v.as_u64())
                    .map(|c| c as i32)
                    .unwrap_or(-1);
                // Fabric + layers come embedded in status now (single
                // round-trip). Default to empty JSON when absent (older
                // daemon or single-device with no mesh).
                self.fabric_json = data.get("fabric")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "{}".to_string());
                self.layers_json = data.get("layers")
                    .map(|v| v.to_string())
                    .unwrap_or_else(|| "[]".to_string());
                // pull scene + peer lists too
                if let Some(r) = self.call("list_scenes", serde_json::json!({})) {
                    // The daemon wraps the list as { "scenes": [...] }.
                    // The QML binds scenes_json as a bare array, so unwrap
                    // the inner array here. Fall back to the whole data
                    // blob (older daemons that returned the array directly).
                    if let Some(d) = r.get("data") {
                        let arr = d.get("scenes").cloned().unwrap_or_else(|| d.clone());
                        self.scenes_json = arr.to_string();
                    }
                }
                if let Some(r) = self.call("list_peers", serde_json::json!({})) {
                    if let Some(d) = r.get("data") { self.peers_json = d.to_string(); }
                }
            }
            _ => { self.daemon_up = false; }
        }
        self.changed();
    }

    /// Install/update now go through the GUI-launchable script so there's a
    /// single, observable code path (the script streams tagged progress into
    /// setup_log). The earlier in-Rust deploy() remains as a fallback.
    fn install(&mut self) { self.run_install(); }
    fn update(&mut self)  { self.run_install(); }

    #[allow(dead_code)]
    fn deploy(&mut self, _is_update: bool) {
        // Resolve where this GUI bundles the daemon payload.
        let payload = locate_payload();
        let payload = match payload {
            Some(p) => p,
            None => {
                eprintln!("mux: no bundled daemon payload found (looked beside exe + share dirs)");
                self.changed();
                return;
            }
        };

        let home = std::env::var_os("HOME").map(PathBuf::from).unwrap_or_default();
        let bin_dst = home.join(".local/bin/bo-muxd");
        let unit_dir = home.join(".config/systemd/user");
        let data_dir = home.join(".local/share/bo-mux");
        let _ = std::fs::create_dir_all(bin_dst.parent().unwrap());
        let _ = std::fs::create_dir_all(&unit_dir);
        let _ = std::fs::create_dir_all(&data_dir);

        // Stop running units before swapping (ignore errors).
        let _ = systemctl(&["stop", "bo-mux.service"]);
        let _ = systemctl(&["stop", "bo-mux.socket"]);

        // Copy binary + units (atomic-ish: write then rename).
        let ok_bin = copy_atomic(&payload.join("bo-muxd"), &bin_dst)
            && make_executable(&bin_dst);
        let ok_sock = copy_atomic(&payload.join("bo-mux.socket"), &unit_dir.join("bo-mux.socket"));
        let ok_svc  = copy_atomic(&payload.join("bo-mux.service"), &unit_dir.join("bo-mux.service"));

        if ok_bin && ok_sock && ok_svc {
            let _ = std::fs::write(version_stamp_path(), SHIPPING_VERSION);
            let _ = systemctl(&["daemon-reload"]);
            let _ = systemctl(&["enable", "--now", "bo-mux.socket"]);
            eprintln!("mux: deployed daemon {}", SHIPPING_VERSION);
        } else {
            eprintln!("mux: deploy failed (bin {} sock {} svc {})", ok_bin, ok_sock, ok_svc);
        }
        self.refresh();
    }

    fn load_scene(&mut self, name: String) {
        let _ = self.call("load_scene", serde_json::json!({ "name": name }));
        self.refresh();
    }
    fn play(&mut self) {
        let _ = self.call("play", serde_json::json!({}));
        self.refresh();
    }
    fn stop(&mut self) {
        let _ = self.call("stop", serde_json::json!({}));
        self.refresh();
    }
    fn start_session(&mut self, layout: String) {
        let _ = self.call("start_session", serde_json::json!({ "layout": layout }));
        self.refresh();
    }
    fn leave_session(&mut self) {
        let _ = self.call("leave_session", serde_json::json!({}));
        self.refresh();
    }
    /// Pin a soundscape layer's channel to a speaker on the fabric.
    /// Drag-to-speaker calls this; we refresh afterward so the new
    /// assignment shows up in fabric_json on the next UI read.
    fn pin_layer(&mut self, channel: String, speaker: String) {
        let _ = self.call("pin_layer",
            serde_json::json!({ "channel": channel, "speaker": speaker }));
        self.refresh();
    }

    /// Push a master gain to bo-muxd. Fire-and-forget — no refresh
    /// follows since this is called periodically (~once per second
    /// from main.rs) and the controller already polls status on
    /// its own cadence. Clamps client-side too (defensive: the
    /// daemon clamps as well).
    fn set_scene_gain(&mut self, value: f64) {
        let clamped = value.clamp(0.0, 1.5);
        let _ = self.call("set_scene_gain",
            serde_json::json!({ "value": clamped }));
    }

    // ── Setup / diagnostics ─────────────────────────────────────────

    /// Locate the scripts dir (bundled beside the payload, or in the payload).
    fn scripts_dir(&self) -> Option<PathBuf> {
        // Scripts ship alongside the daemon payload.
        if let Some(p) = locate_payload() {
            // payload may contain scripts/ or the scripts directly
            for cand in [p.join("scripts"), p.clone()] {
                if cand.join("bo-mux-doctor.sh").exists() { return Some(cand); }
            }
        }
        // Fallbacks beside the exe / standard share dir.
        if let Ok(exe) = std::env::current_exe() {
            if let Some(dir) = exe.parent() {
                let c = dir.join("scripts");
                if c.join("bo-mux-doctor.sh").exists() { return Some(c); }
            }
        }
        None
    }

    /// Run a script and capture combined stdout/stderr into setup_log.
    /// Returns whether it exited 0.
    /// Run the doctor with --json and store the parsed result for the UI.
    fn run_doctor(&mut self) {
        // Doctor stays on the streaming path too — its --json output
        // arrives at the end as a single line, parsed in poll_setup
        // when the Done message lands.
        self.run_script_streaming(
            "bo-mux-doctor.sh",
            vec!["--json".into()],
            "doctor",
        );
    }

    fn run_install(&mut self) {
        let payload = locate_payload()
            .map(|p| p.to_string_lossy().to_string())
            .unwrap_or_default();
        let ver = SHIPPING_VERSION.to_string();
        self.run_script_streaming(
            "bo-mux-install.sh",
            vec!["--payload".into(), payload, "--version".into(), ver],
            "install",
        );
    }

    fn run_uninstall(&mut self) {
        self.run_script_streaming(
            "bo-mux-uninstall.sh",
            vec!["--keep-data".into()],
            "uninstall",
        );
    }

    /// User-facing "Fix audio" action. Runs the cleanup script that
    /// stops bo-muxd and reaps orphan audio subprocesses; output
    /// streams to setup_log so the user sees each step land.
    fn fix_audio(&mut self) {
        self.run_script_streaming(
            "bo-mux-fix-audio.sh",
            vec![],
            "fix_audio",
        );
    }

    /// Same as fix_audio but also restarts pipewire/pipewire-pulse.
    fn fix_audio_full(&mut self) {
        self.run_script_streaming(
            "bo-mux-fix-audio.sh",
            vec!["--restart-audio".into()],
            "fix_audio",
        );
    }

    /// Drain the streaming-script channel into setup_log. QML calls
    /// this on a 200ms Timer while setup_running is true. Cheap
    /// when there's nothing to drain.
    ///
    /// We `take()` the receiver to satisfy the borrow checker, then
    /// put it back unless the child has exited (Done message). The
    /// Done branch finalizes per-script state — e.g. parses the
    /// doctor JSON from the trailing log line — and triggers a
    /// refresh() so the daemon-status UI updates atomically.
    fn poll_setup(&mut self) {
        let Some(rx) = self.setup_rx.take() else { return; };
        let mut any_change = false;
        let mut completed = false;
        let mut final_ok = false;
        loop {
            match rx.try_recv() {
                Ok(SetupMsg::Line(s)) => {
                    self.setup_log = format!("{}{}\n", self.setup_log, s);
                    any_change = true;
                }
                Ok(SetupMsg::Done(ok)) => {
                    completed = true;
                    final_ok = ok;
                    break;
                }
                Err(TryRecvError::Empty) => break,
                Err(TryRecvError::Disconnected) => {
                    // Background thread exited without a Done — should
                    // not happen, but treat as failure so we leave
                    // a clean state rather than hanging on
                    // setup_running forever.
                    completed = true;
                    final_ok = false;
                    break;
                }
            }
        }
        if !completed {
            // Still streaming — put rx back for the next poll.
            self.setup_rx = Some(rx);
            if any_change { self.setup_changed(); }
            return;
        }
        // Script completed. Drop the channel and finalize state.
        self.setup_running = false;
        self.setup_ok = final_ok;
        let kind = std::mem::take(&mut self.setup_kind);
        if kind == "doctor" {
            // The doctor's --json output is one trailing JSON object
            // line. Parse it from setup_log so the checklist UI
            // can render.
            let log = self.setup_log.clone();
            let json_line = log.lines().rev()
                .find(|l| l.trim_start().starts_with('{'))
                .unwrap_or("{}");
            self.doctor_json = json_line.to_string();
        }
        self.setup_changed();
        // After install/uninstall/fix_audio the daemon state changed
        // (daemon_status, daemon_up, passive). Refresh so the UI's
        // install banner / passive badge updates immediately.
        self.refresh();
    }

    /// Run a script asynchronously, streaming output to setup_log.
    /// Returns immediately. The QML side polls poll_setup() to drain
    /// output and detect completion.
    ///
    /// Why streaming over the older synchronous run_script:
    ///   1. Install + uninstall scripts take 30-60 s and previously
    ///      froze the Qt event loop. Now the UI stays responsive
    ///      and the user sees STEP/OK lines land in real time.
    ///   2. Fix-audio is fast (~2-3 s) but its per-step "what just
    ///      happened" output IS the value: the user wants to see
    ///      "stopped bo-muxd, reaped 3 orphans, default sink is
    ///      alsa_output.X" rather than a frozen window followed by
    ///      a wall of text.
    ///   3. Failure modes are clearer — if the script hangs, the
    ///      user sees exactly where it stopped instead of an
    ///      indistinguishable frozen UI.
    fn run_script_streaming(&mut self, name: &str, args: Vec<String>, kind: &str) {
        let dir = match self.scripts_dir() {
            Some(d) => d,
            None => {
                self.setup_log = "ERR  scripts dir not found (looked beside payload + exe)".into();
                self.setup_ok = false;
                self.setup_running = false;
                self.setup_changed();
                return;
            }
        };
        let script = dir.join(name);
        if !script.exists() {
            self.setup_log = format!("ERR  script not found: {}", script.display());
            self.setup_ok = false;
            self.setup_running = false;
            self.setup_changed();
            return;
        }

        // Seed the log with the command line so the user sees
        // exactly what we're running.
        self.setup_running = true;
        self.setup_ok = false;
        self.setup_log = format!("$ {} {}\n", name, args.join(" "));
        self.setup_kind = kind.to_string();
        self.setup_changed();

        let (tx, rx) = channel::<SetupMsg>();
        self.setup_rx = Some(rx);

        // Spawn a single background thread that owns the child and
        // both readers. stderr is forwarded to stdout via 2>&1 so
        // we don't need a second reader thread — keeps things
        // simpler and preserves interleaving order.
        thread::spawn(move || {
            // bash -c '<script> "$@" 2>&1' so stderr merges into
            // stdout. We re-shell rather than using shell-redirect
            // wrappers because the script is bash and we want exit
            // status to propagate cleanly.
            let mut cmd = ProcCommand::new("bash");
            cmd.arg(&script)
                .args(&args)
                .stdout(Stdio::piped())
                .stderr(Stdio::piped());
            let mut child = match cmd.spawn() {
                Ok(c) => c,
                Err(e) => {
                    let _ = tx.send(SetupMsg::Line(
                        format!("ERR  failed to launch script: {}", e)));
                    let _ = tx.send(SetupMsg::Done(false));
                    return;
                }
            };
            // Read stdout line-by-line in this thread; spawn a
            // separate one for stderr so a slow stderr reader can't
            // block stdout (or vice-versa). Lines are sent through
            // the same channel — order between stdout and stderr is
            // best-effort but lines from each stream stay ordered
            // within themselves, which is what the user expects
            // when reading the log.
            if let Some(stderr) = child.stderr.take() {
                let tx_err = tx.clone();
                thread::spawn(move || {
                    let reader = BufReader::new(stderr);
                    for line in reader.lines().map_while(Result::ok) {
                        if tx_err.send(SetupMsg::Line(line)).is_err() {
                            // Receiver dropped (poll_setup never
                            // called again or bridge destroyed).
                            return;
                        }
                    }
                });
            }
            if let Some(stdout) = child.stdout.take() {
                let reader = BufReader::new(stdout);
                for line in reader.lines().map_while(Result::ok) {
                    if tx.send(SetupMsg::Line(line)).is_err() {
                        return;
                    }
                }
            }
            // Both readers drained → wait for the process. The
            // stderr-reader thread may still be running for a beat
            // after stdout closed (if it had more to emit); the
            // wait() below blocks until the child fully exits,
            // which guarantees no further lines arrive after Done.
            let ok = child.wait().map(|s| s.success()).unwrap_or(false);
            let _ = tx.send(SetupMsg::Done(ok));
        });
    }
}

/// Find the bundled daemon payload dir (containing bo-muxd + the unit files).
/// Looks beside the running executable, then a couple of install locations.
fn locate_payload() -> Option<PathBuf> {
    let mut candidates: Vec<PathBuf> = Vec::new();
    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            candidates.push(dir.join("bo-mux-payload"));
            candidates.push(dir.to_path_buf());
        }
    }
    if let Some(home) = std::env::var_os("HOME") {
        candidates.push(PathBuf::from(&home).join(".local/share/bo-controller/bo-mux-payload"));
    }
    candidates.push(PathBuf::from("/usr/share/bo-controller/bo-mux-payload"));
    candidates.into_iter().find(|p| p.join("bo-muxd").exists())
}

fn copy_atomic(src: &std::path::Path, dst: &std::path::Path) -> bool {
    if !src.exists() { return false; }
    let tmp = dst.with_extension("tmp-install");
    if std::fs::copy(src, &tmp).is_err() { return false; }
    std::fs::rename(&tmp, dst).is_ok()
}

fn make_executable(path: &std::path::Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    if let Ok(meta) = std::fs::metadata(path) {
        let mut perms = meta.permissions();
        perms.set_mode(0o755);
        return std::fs::set_permissions(path, perms).is_ok();
    }
    false
}

fn systemctl(args: &[&str]) -> bool {
    ProcCommand::new("systemctl").arg("--user").args(args).status()
        .map(|s| s.success()).unwrap_or(false)
}

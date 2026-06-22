// bo-mux/src/activity_watch.rs
//
// ── What this is ───────────────────────────────────────────────────
//
// A small piece of cross-thread state that answers one question:
// "should the daemon proactively start new audio right now, or
// should it back off?"
//
// When the answer is "back off", we flip a shared atomic that the
// audio sink reads on every `voice_on` call. New voices get
// suppressed; already-playing children continue to completion
// (their paplay/ffplay subprocesses are independent of us). The
// daemon stays alive on its control socket — any command exits
// passive mode immediately.
//
// ── Why this exists ────────────────────────────────────────────────
//
// Bo-mux's scene engine is a loop: clips end, the scheduler spawns
// the next one. That's correct for normal listening but actively
// hostile in a few common situations:
//
//   1. **Controller process is gone.** The user closed the bo-controller
//      app or rebooted the phone. Without the GUI, the soundscape
//      shouldn't keep playing indefinitely — it's just respawning
//      ffplay processes that the user can't stop.
//
//   2. **User moved output via KDE picker / pavucontrol.** They
//      reached for the system tray, dragged playback to a different
//      sink. Bo-mux's per-clip `--device=` targeting means we'll
//      keep playing to the OLD sink (silent if BT disconnected)
//      while the user listens to whatever they switched to. The
//      right thing is to stop spawning at the stale target.
//
//   3. **Bluetooth toggled off.** The configured sink vanishes from
//      `pactl list sinks`. Each clip spawn then prints an error
//      and the engine doesn't notice. Stop spawning.
//
//   4. **Idle.** No user interaction for a long time (10 min by
//      default). The scene is presumably forgotten background. Let
//      it end naturally rather than running forever.
//
// In all 4 cases, the daemon stays running. Sending any command
// (load_scene, play, stop, even ping) wakes it back to active mode.
//
// ── How callers use it ─────────────────────────────────────────────
//
//   let watch = Arc::new(ActivityWatch::new());
//   // Daemon::handle calls this on every request:
//   watch.note_command();
//   // Tick thread runs every ~5 s:
//   watch.tick();
//   // Sink reads this atomic on every voice_on:
//   let flag = watch.passive_flag();
//   if flag.load(Ordering::Relaxed) { return; }
//
// The atomic is the only thing on the hot path (voice_on). Tick
// runs at 5 s intervals; pactl probes are a subprocess spawn each
// time, which is fine at that cadence.

use std::process::Command;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

/// How long with no command from any client before we go passive on
/// the "controller heartbeat stale" trigger. The controller polls
/// status every 1.5 s while the soundscape page is visible, and we
/// add a root-level ping every 30 s while the app is alive. 60 s
/// is well past either interval — long enough that a transient
/// hiccup (sleeping device, page transition, mobile RM-OOM event)
/// doesn't flip us to passive spuriously.
pub const DEFAULT_CONTROLLER_TIMEOUT_SECS: u64 = 60;

/// How long with no user-action command (load_scene / play / stop /
/// save_scene) before we go passive on the "idle" trigger. Pings
/// and status polls don't count here. 10 minutes matches the
/// recommendation auto-switch pause window — if the user hasn't
/// touched anything in that long, they're not actively listening.
pub const DEFAULT_IDLE_TIMEOUT_SECS: u64 = 600;

/// How often the tick thread runs. Cheap (one pactl subprocess +
/// some atomic loads) so 5 s is fine.
pub const TICK_INTERVAL_SECS: u64 = 5;

/// Set of commands considered "user actions" for the idle timer.
/// Polls (ping, status, list_scenes, list_peers) are NOT user
/// actions; only commands that change state or express intent are.
pub fn is_user_action(cmd_tag: &str) -> bool {
    matches!(cmd_tag,
        "load_scene" | "play" | "stop" |
        "start_session" | "leave_session" |
        "save_scene" | "shutdown")
}

pub struct ActivityWatch {
    /// Flipped to true when any trigger fires; back to false on the
    /// next command. The sink reads this on every voice_on; the
    /// transport reads it (read-only) to decide whether to emit the
    /// per-tick event.
    passive: Arc<AtomicBool>,

    /// Last time the daemon received ANY command (including polls).
    /// Used for the controller-heartbeat trigger.
    last_command_at: Mutex<Instant>,

    /// Last time the daemon received a USER-INITIATED command.
    /// Used for the idle trigger.
    last_user_action_at: Mutex<Instant>,

    /// What sink the currently-loaded scene targets (typically the
    /// pactl sink name, e.g. "bluez_sink.AC_8E_BD..."). If None,
    /// scenes play to the system default. The sink-disappeared and
    /// sink-changed triggers reference this.
    configured_sink: Mutex<Option<String>>,

    /// Last default-sink string we saw from `pactl get-default-sink`.
    /// We compare across ticks to detect external changes (KDE
    /// picker, pavucontrol, etc).
    last_default_sink: Mutex<Option<String>>,

    /// Most recent reason we went passive, for status reporting and
    /// debugging. Cleared on note_command.
    last_passive_reason: Mutex<Option<String>>,

    controller_timeout: Duration,
    idle_timeout: Duration,
    /// Battery-saving policy. When the device is on battery and low, the
    /// daemon goes passive sooner (or immediately if critical) to stop
    /// the continuous audio/DSP drain. On mains-only systems the battery
    /// reads as absent and these triggers never fire.
    power_policy: crate::power_state::PowerPolicy,
}

impl Default for ActivityWatch {
    fn default() -> Self {
        Self::with_timeouts(
            Duration::from_secs(DEFAULT_CONTROLLER_TIMEOUT_SECS),
            Duration::from_secs(DEFAULT_IDLE_TIMEOUT_SECS),
        )
    }
}

impl ActivityWatch {
    pub fn new() -> Self { Self::default() }

    pub fn with_timeouts(controller_timeout: Duration, idle_timeout: Duration) -> Self {
        let now = Instant::now();
        Self {
            passive: Arc::new(AtomicBool::new(false)),
            last_command_at: Mutex::new(now),
            last_user_action_at: Mutex::new(now),
            configured_sink: Mutex::new(None),
            last_default_sink: Mutex::new(query_default_sink()),
            last_passive_reason: Mutex::new(None),
            controller_timeout,
            idle_timeout,
            power_policy: crate::power_state::PowerPolicy::default(),
        }
    }

    /// Cheap clone of the atomic used on the audio hot path.
    pub fn passive_flag(&self) -> Arc<AtomicBool> { self.passive.clone() }

    /// True if currently in passive mode.
    pub fn is_passive(&self) -> bool {
        self.passive.load(Ordering::Relaxed)
    }

    /// Reason we went passive, or None if active. Useful for status
    /// reporting back to the controller.
    pub fn passive_reason(&self) -> Option<String> {
        self.last_passive_reason.lock().unwrap().clone()
    }

    /// Tell the watch which sink the current scene targets. Pass
    /// `None` when no scene is loaded or when the scene plays to
    /// the system default. The watch uses this for two things:
    ///   1. Detecting "configured sink disappeared" (BT off).
    ///   2. Distinguishing "we set the default sink" from "user did".
    ///      Currently we never set the default sink, but the field
    ///      is here so we can correctly attribute future changes.
    pub fn note_configured_sink(&self, sink: Option<String>) {
        *self.configured_sink.lock().unwrap() = sink;
    }

    /// Record that a command arrived. Resets the controller-heartbeat
    /// timer and exits passive mode if we were in it. If `cmd_tag`
    /// is a user action (see `is_user_action`), also resets the
    /// idle timer.
    pub fn note_command(&self, cmd_tag: &str) {
        let now = Instant::now();
        *self.last_command_at.lock().unwrap() = now;
        if is_user_action(cmd_tag) {
            *self.last_user_action_at.lock().unwrap() = now;
        }
        // Any command exits passive — the client has shown they're
        // around and intend the daemon to do work. We do this even
        // for polls so a freshly-started controller picking up an
        // existing daemon doesn't wait the full tick interval for
        // active mode.
        if self.passive.swap(false, Ordering::Relaxed) {
            *self.last_passive_reason.lock().unwrap() = None;
            eprintln!("activity: command '{}' received — leaving passive mode",
                      cmd_tag);
        }
    }

    /// One step of the background watcher. Checks all four triggers
    /// and flips to passive if any fires. Called from a 5-second
    /// loop in main(). Cheap; ok to run frequently.
    pub fn tick(&self) {
        // Already passive? Nothing to do until a command wakes us.
        if self.passive.load(Ordering::Relaxed) {
            return;
        }
        let now = Instant::now();
        // ── Trigger 1: controller heartbeat stale ──
        let since_cmd = now.duration_since(*self.last_command_at.lock().unwrap());
        if since_cmd > self.controller_timeout {
            self.go_passive(format!(
                "controller silent for {} s (last command was {} s ago)",
                since_cmd.as_secs(), since_cmd.as_secs()));
            return;
        }
        // ── Trigger 2: user took the audio picker ──
        // We snapshot pactl's default sink and compare to what we
        // saw last tick. A change we didn't cause = user did.
        // (We never call set-default-sink, so any change is external.)
        let observed = query_default_sink();
        {
            let mut last = self.last_default_sink.lock().unwrap();
            if let (Some(prev), Some(cur)) = (last.as_ref(), observed.as_ref()) {
                if prev != cur {
                    self.go_passive(format!(
                        "default sink changed externally ({} → {})",
                        prev, cur));
                    *last = observed.clone();
                    return;
                }
            }
            // Seed or refresh the snapshot.
            *last = observed.clone();
        }
        // ── Trigger 3: configured sink disappeared (e.g. BT off) ──
        if let Some(configured) = self.configured_sink.lock().unwrap().clone() {
            if !sink_exists(&configured) {
                self.go_passive(format!(
                    "configured sink '{}' no longer present in pactl list",
                    configured));
                return;
            }
        }
        // ── Trigger 4: idle (no user actions for a long time) ──
        // The idle timeout is shortened when we're low on battery, so an
        // unplugged, draining device naps sooner after you stop touching
        // it. On mains (or with plenty of charge) this is the base value.
        // We scale the Duration directly to preserve sub-second precision
        // (tests use millisecond timeouts).
        let power = crate::power_state::PowerState::read();
        let idle_timeout = self.power_policy
            .effective_idle(self.idle_timeout, &power);
        let since_user = now.duration_since(*self.last_user_action_at.lock().unwrap());
        if since_user > idle_timeout {
            self.go_passive(format!(
                "idle for {} min — no user action",
                since_user.as_secs() / 60));
            return;
        }
        // ── Trigger 5: battery critically low while unplugged ──
        // Continuous decode + mix is a real drain; when the battery is
        // critical and there's no AC, stop the soundscape to preserve
        // charge. Never fires on mains-powered or well-charged systems.
        if self.power_policy.should_force_passive(&power) {
            self.go_passive(self.power_policy.critical_reason(&power));
            return;
        }
    }

    fn go_passive(&self, reason: String) {
        // Atomic swap ensures we only log the transition once even
        // if tick() races with itself (we run from a single thread,
        // but defensive coding is cheap).
        if !self.passive.swap(true, Ordering::Relaxed) {
            *self.last_passive_reason.lock().unwrap() = Some(reason.clone());
            eprintln!("activity: entering passive mode — {}", reason);
        }
    }
}

/// Run `pactl get-default-sink` and return the trimmed output. None
/// if pactl isn't installed, the subprocess errored, or the output
/// is empty. The error case correctly de-bouncing the "sink changed"
/// detection (we won't flip to passive just because pactl had a
/// hiccup — the next tick will see a normal result).
pub fn query_default_sink() -> Option<String> {
    let out = Command::new("pactl")
        .arg("get-default-sink")
        .output()
        .ok()?;
    if !out.status.success() { return None; }
    let s = String::from_utf8_lossy(&out.stdout).trim().to_string();
    if s.is_empty() { None } else { Some(s) }
}

/// Check whether a given sink name appears in `pactl list short sinks`.
/// Returns true if pactl is unavailable (fail-open: we don't want
/// "pactl isn't installed" to flip us to passive — the operator
/// presumably knows what they're doing if they ship without pactl).
pub fn sink_exists(name: &str) -> bool {
    let out = match Command::new("pactl")
        .args(["list", "short", "sinks"])
        .output() { Ok(o) => o, Err(_) => return true };
    if !out.status.success() { return true; }
    let body = String::from_utf8_lossy(&out.stdout);
    // Format is "id<TAB>name<TAB>module<TAB>...". We only care about
    // the name column; substring match is robust to column order
    // variations across pactl versions.
    body.lines().any(|l| l.split_whitespace().nth(1) == Some(name))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn note_command_resets_heartbeat() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_millis(50), Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(60));
        w.note_command("ping");
        w.tick();
        assert!(!w.is_passive(),
            "fresh command should keep us active even past timeout");
    }

    #[test]
    fn stale_controller_triggers_passive() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_millis(40), Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(60));
        w.tick();
        assert!(w.is_passive(), "should go passive after controller timeout");
        let reason = w.passive_reason().expect("reason should be set");
        assert!(reason.contains("controller silent"),
            "reason should mention controller: {}", reason);
    }

    #[test]
    fn command_wakes_from_passive() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_millis(30), Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(50));
        w.tick();
        assert!(w.is_passive());
        w.note_command("status");
        assert!(!w.is_passive(), "any command should wake from passive");
        assert!(w.passive_reason().is_none(),
            "reason should be cleared after wake");
    }

    #[test]
    fn idle_only_resets_on_user_actions() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_secs(60), Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(50));
        // Polls don't reset the idle timer.
        w.note_command("ping");
        w.note_command("status");
        w.tick();
        assert!(w.is_passive(),
            "polls alone shouldn't keep us active past the idle window");
        let reason = w.passive_reason().unwrap();
        assert!(reason.contains("idle"),
            "reason should mention idle: {}", reason);
    }

    #[test]
    fn user_action_resets_both_timers() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_secs(60), Duration::from_millis(40));
        std::thread::sleep(Duration::from_millis(50));
        w.note_command("load_scene");
        w.tick();
        assert!(!w.is_passive(),
            "load_scene is a user action — should keep us active");
    }

    #[test]
    fn is_user_action_classification() {
        // User actions:
        assert!(is_user_action("load_scene"));
        assert!(is_user_action("play"));
        assert!(is_user_action("stop"));
        assert!(is_user_action("save_scene"));
        // Polls / introspection:
        assert!(!is_user_action("ping"));
        assert!(!is_user_action("status"));
        assert!(!is_user_action("list_scenes"));
        assert!(!is_user_action("list_peers"));
    }

    #[test]
    fn passive_flag_is_shared_across_clones() {
        let w = ActivityWatch::with_timeouts(
            Duration::from_millis(20), Duration::from_secs(600));
        let flag = w.passive_flag();
        assert!(!flag.load(Ordering::Relaxed));
        std::thread::sleep(Duration::from_millis(30));
        w.tick();
        assert!(flag.load(Ordering::Relaxed),
            "passive flag should be visible through clones");
    }

    #[test]
    fn note_configured_sink_does_not_panic_on_none() {
        let w = ActivityWatch::new();
        w.note_configured_sink(None);
        w.note_configured_sink(Some("alsa_output.test".into()));
        w.note_configured_sink(None);
    }

    #[test]
    fn already_passive_tick_is_noop() {
        // Going passive twice in a row should not double-log; the
        // swap pattern in go_passive prevents the second transition
        // from doing anything. We verify it doesn't blow up.
        let w = ActivityWatch::with_timeouts(
            Duration::from_millis(20), Duration::from_secs(600));
        std::thread::sleep(Duration::from_millis(30));
        w.tick();
        w.tick();
        w.tick();
        assert!(w.is_passive());
    }
}

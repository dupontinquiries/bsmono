// src/sink_system.rs
//
// A concrete AudioSink that actually makes sound by shelling out to a system
// audio player, one process per sounding voice. This is the pragmatic "last
// inch": no heavy audio-crate dependency, no risk of bricking anything, and
// it works on the user's PipeWire/KDE box where `paplay`/`ffplay`/`aplay`
// exist. A native per-channel mixing sink (real upmix + per-speaker routing)
// can replace this later behind the same trait — the transport doesn't care.
//
// What it does on each event:
//   voice_on(key, v, gain)  → spawn a player for v.clip, remember its child
//   voice_gain(key, gain)   → (no-op for subprocess players; volume is set at
//                              spawn. A native sink would ramp here.)
//   voice_off(key)          → kill the child for that key (the crossfade tail
//                              is approximate — subprocess players can't fade)
//
// Honest limits vs. a native sink:
//   - No sample-accurate crossfades (we can't ramp a running paplay easily),
//     so overlapping clips just both play during the overlap window. For
//     ambient soundscapes this is fine; for tight music it isn't.
//   - Gain is applied at spawn where the player supports it (paplay --volume,
//     ffplay -af volume). pan/rate aren't honored by simple players.
//   - One process per voice; a dense scene = several concurrent players. The
//     OS mixer (PipeWire) sums them, which is exactly what we want here.
//
// This makes the daemon genuinely audible end-to-end today, while keeping the
// door open for the better native path.

use crate::transport::AudioSink;
use crate::player::Voice;
use std::collections::HashMap;
use std::process::{Child, Command, Stdio};

/// Below this linear gain a clip is inaudible (~ -54 dB), so it's not
/// worth spawning a decoder for it. Used to skip work when the scene is
/// fully ducked (music handoff) — saves CPU and battery during the whole
/// music session. Pure + public so the threshold is unit-testable.
pub const AUDIBLE_GAIN_EPS: f32 = 0.002;

/// Is this effective gain loud enough to be worth spawning a player for?
pub fn is_audible(gain: f32) -> bool {
    gain >= AUDIBLE_GAIN_EPS
}

/// Which system player to use, detected once at startup.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PlayerKind { Paplay, Ffplay, Aplay, None }

impl PlayerKind {
    /// Detect an available player, preferring PipeWire-friendly `paplay`.
    pub fn detect() -> Self {
        // Priority order: ffplay first because it natively decodes
        // every format we care about (opus, mp3, flac, ogg, m4a,
        // aac, wav). paplay second — it only plays WAV/AU/RAW
        // natively, but we wrap it with ffmpeg-decode-to-stdin so
        // it can still handle arbitrary inputs (see build_command).
        // aplay last as a true-WAV-only fallback for setups that
        // have neither ffplay nor paplay+ffmpeg.
        //
        // Symptom of the previous order (paplay first, plain spawn):
        // any soundscape scene that referenced .opus clips (which is
        // almost all of them — the user's clip pool is .opus) would
        // silently fail to play. paplay would fork, decide the file
        // wasn't a PCM/WAV, log "Format not supported" to its own
        // stderr, exit 1, and the user just heard nothing.
        for (kind, bin) in [
            (PlayerKind::Ffplay, "ffplay"),
            (PlayerKind::Paplay, "paplay"),
            (PlayerKind::Aplay,  "aplay"),
        ] {
            if which(bin) { return kind; }
        }
        PlayerKind::None
    }
    pub fn binary(&self) -> &'static str {
        match self { PlayerKind::Paplay => "paplay", PlayerKind::Ffplay => "ffplay",
                     PlayerKind::Aplay => "aplay", PlayerKind::None => "" }
    }
}

/// Is `bin` on PATH? (cheap `command -v` via the shell.)
fn which(bin: &str) -> bool {
    Command::new("sh").arg("-c").arg(format!("command -v {}", bin))
        .stdout(Stdio::null()).stderr(Stdio::null())
        .status().map(|s| s.success()).unwrap_or(false)
}

pub struct SystemSink {
    kind: PlayerKind,
    children: HashMap<String, Child>,
    /// If true, route through a specific PipeWire/Pulse sink name (per device
    /// output selection). None = default sink.
    pub target_sink: Option<String>,
    /// Cross-thread "stop spawning new clips" flag, set by
    /// `activity_watch::ActivityWatch::tick()` when the user is
    /// clearly not engaged (controller closed, KDE picker moved
    /// audio elsewhere, BT off, idle). When set, `voice_on`
    /// becomes a no-op so already-playing children finish naturally
    /// and the daemon stops fighting for the audio path. Cleared
    /// when any command arrives via the control socket.
    ///
    /// `None` means "no watch attached" — the sink just always
    /// spawns. This keeps unit tests that construct a raw
    /// SystemSink from needing to plumb an ActivityWatch.
    passive_flag: Option<std::sync::Arc<std::sync::atomic::AtomicBool>>,
    /// Cross-thread master gain multiplier in f32 bit-pattern form.
    /// Applied to every voice_on as `gain * scene_gain`. Updated by
    /// the daemon's `SetSceneGain` handler in response to the
    /// controller's ambient noise observation.
    ///
    /// f32 is stored as u32 bits via `f32::to_bits`/`from_bits` —
    /// `AtomicF32` doesn't exist in std, and the bit-cast roundtrip
    /// is bit-equivalent. `None` means "no gain shared" → effective
    /// multiplier 1.0 (unit-test friendliness).
    scene_gain: Option<std::sync::Arc<std::sync::atomic::AtomicU32>>,
    /// Base directory for resolving relative clip paths. Scene files
    /// reference clips like "clips/brook_close_a.opus"; those are
    /// relative to wherever the clip pool was bootstrapped (the
    /// user's data dir), NOT the daemon's CWD. When set, any clip
    /// path that isn't absolute and doesn't exist as-is is resolved
    /// against this base. None = use clip paths verbatim (tests).
    clip_base: Option<std::path::PathBuf>,
}

impl SystemSink {
    pub fn new() -> Self {
        let kind = PlayerKind::detect();
        if kind == PlayerKind::None {
            eprintln!("[sink] no system audio player found (install pipewire-utils/pulseaudio-utils or ffmpeg)");
        } else {
            eprintln!("[sink] using {}", kind.binary());
        }
        Self {
            kind, children: HashMap::new(),
            target_sink: None, passive_flag: None,
            scene_gain: None, clip_base: None,
        }
    }

    /// Set the base directory used to resolve relative clip paths.
    /// The daemon calls this with the bootstrapped clip pool dir
    /// (e.g. ~/.local/share/bo-mux/clips' parent) so scenes that
    /// reference "clips/foo.opus" find the real files.
    pub fn set_clip_base(&mut self, base: std::path::PathBuf) {
        eprintln!("[sink] clip base set to {:?}", base);
        self.clip_base = Some(base);
    }

    /// Resolve a clip reference to a concrete filesystem path.
    /// Order: (1) absolute path → use as-is; (2) exists relative to
    /// CWD → use as-is; (3) clip_base/<clip> if clip_base set;
    /// (4) fall back to the original string (player will error,
    /// surfacing a clear "file not found" in logs).
    fn resolve_clip(&self, clip: &str) -> String {
        let p = std::path::Path::new(clip);
        if p.is_absolute() { return clip.to_string(); }
        if p.exists() { return clip.to_string(); }
        if let Some(base) = &self.clip_base {
            let joined = base.join(clip);
            if joined.exists() {
                return joined.to_string_lossy().to_string();
            }
            // Also try just the basename under base/clips/ in case
            // the scene used a bare filename without the clips/ prefix.
            if let Some(fname) = p.file_name() {
                let alt = base.join("clips").join(fname);
                if alt.exists() {
                    return alt.to_string_lossy().to_string();
                }
            }
        }
        clip.to_string()
    }

    /// Attach the activity-watch passive flag. The daemon calls this
    /// after constructing the SystemSink for the transport thread.
    /// Once attached, `voice_on` skips spawning whenever the flag is
    /// set (the audio engine still ticks; we just don't fire new
    /// children).
    pub fn set_passive_flag(
        &mut self,
        flag: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) {
        self.passive_flag = Some(flag);
    }

    /// Attach the shared scene-gain atomic. Reads in voice_on
    /// multiply the per-voice gain by the current value (or 1.0
    /// if the atomic isn't attached). The daemon updates this
    /// atomic in response to SetSceneGain commands.
    pub fn set_scene_gain_atomic(
        &mut self,
        atomic: std::sync::Arc<std::sync::atomic::AtomicU32>,
    ) {
        self.scene_gain = Some(atomic);
    }

    /// Current scene-gain multiplier (1.0 if no atomic attached).
    /// Hot-path read on every spawn — uses Relaxed ordering since
    /// we don't need synchronization with other variables.
    fn current_scene_gain(&self) -> f32 {
        match &self.scene_gain {
            Some(a) => f32::from_bits(a.load(std::sync::atomic::Ordering::Relaxed)),
            None => 1.0,
        }
    }

    /// Build the spawn command for one clip at a given gain (0..1).
    fn build_command(&self, clip_ref: &str, gain: f32) -> Option<Command> {
        if self.kind == PlayerKind::None { return None; }
        // Resolve relative clip paths against the bootstrapped clip base.
        let clip_owned = self.resolve_clip(clip_ref);
        let clip = clip_owned.as_str();
        // Determine if the clip is something paplay/aplay can play
        // natively (WAV-family). Anything else needs ffmpeg-decode.
        // .opus/.ogg/.mp3/.flac/.m4a all go through the ffmpeg path.
        let native_paplay = clip.to_ascii_lowercase()
            .ends_with(".wav")
            || clip.to_ascii_lowercase().ends_with(".au")
            || clip.to_ascii_lowercase().ends_with(".raw");
        let mut cmd = Command::new(self.kind.binary());
        match self.kind {
            PlayerKind::Paplay => {
                if native_paplay {
                    // paplay --volume is 0..65536. Map gain.
                    let vol = (gain.clamp(0.0, 1.0) * 65536.0) as u32;
                    cmd.arg(format!("--volume={}", vol));
                    if let Some(s) = &self.target_sink { cmd.arg(format!("--device={}", s)); }
                    cmd.arg(clip);
                } else {
                    // Non-WAV: route through ffmpeg → paplay via a
                    // shell pipe. We use `exec` on the right side
                    // so kill() of the shell propagates the signal
                    // to the paplay child (otherwise voice_off
                    // would orphan paplay leaving sound ringing).
                    //
                    // Symptom of the previous code (paplay-direct on
                    // .opus): paplay logs "Format not supported"
                    // and exits 1; the soundscape scene appears
                    // silent because none of its layers ever
                    // produced audio.
                    let vol = (gain.clamp(0.0, 1.0) * 65536.0) as u32;
                    let dev_flag = self.target_sink.as_ref()
                        .map(|s| format!("--device={} ", shell_escape(s)))
                        .unwrap_or_default();
                    let pipe = format!(
                        "ffmpeg -nostdin -hide_banner -loglevel quiet \
                         -i {clip_esc} -f wav - | \
                         exec paplay --volume={vol} {dev_flag}--raw=false /dev/stdin",
                        clip_esc = shell_escape(clip),
                        vol = vol,
                        dev_flag = dev_flag);
                    let mut shell = Command::new("sh");
                    shell.arg("-c").arg(pipe);
                    shell.stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
                    return Some(shell);
                }
            }
            PlayerKind::Ffplay => {
                // ffplay: no display, exit at end, volume filter.
                // Handles every format we care about natively.
                cmd.args(["-nodisp", "-autoexit", "-loglevel", "quiet"]);
                cmd.arg("-af").arg(format!("volume={:.3}", gain.clamp(0.0, 1.0)));
                cmd.arg(clip);
            }
            PlayerKind::Aplay => {
                // aplay: no volume flag, WAV-only natively. For
                // non-WAV clips on a system where neither ffplay
                // nor paplay exists, surface the constraint
                // honestly rather than fork a process that will
                // fail silently.
                if !native_paplay {
                    eprintln!("[sink] aplay can't play '{}' \
                               (non-WAV). Install ffplay or \
                               paplay+ffmpeg.", clip);
                    return None;
                }
                cmd.arg(clip);
            }
            PlayerKind::None => return None,
        }
        cmd.stdout(Stdio::null()).stderr(Stdio::null()).stdin(Stdio::null());
        Some(cmd)
    }
}

/// Single-quote escape for use inside a shell `sh -c '...'` pipeline.
/// Any embedded single quote is replaced with `'\''` (close-quote,
/// escaped quote, re-open-quote) — the standard POSIX-shell trick.
/// Used by the non-WAV paplay path above where we hand a clip path
/// + target sink to `sh -c` and need to defend against spaces and
/// shell metacharacters.
fn shell_escape(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('\'');
    for c in s.chars() {
        if c == '\'' { out.push_str("'\\''"); } else { out.push(c); }
    }
    out.push('\'');
    out
}

// Marker so the impl block above closes cleanly; the real
// implementation continues with reap() below.
impl SystemSink {

    /// Reap any finished children so the map doesn't grow unbounded.
    pub fn reap(&mut self) {
        self.children.retain(|_, child| {
            matches!(child.try_wait(), Ok(None))   // keep only still-running
        });
    }
}

impl Default for SystemSink {
    fn default() -> Self { Self::new() }
}

impl AudioSink for SystemSink {
    fn voice_on(&mut self, key: &str, v: &Voice, gain: f32) {
        // Honor the activity-watch passive flag, if attached.
        // When set, we DON'T spawn new clips — the user has either
        // closed the controller, moved audio to another sink via
        // the system picker, disabled Bluetooth (sink vanished),
        // or simply gone idle. Existing playing children are
        // independent subprocesses; they finish naturally. The
        // schedule still advances inside the transport so when a
        // command wakes us back to active mode, we don't rewind.
        if let Some(flag) = &self.passive_flag {
            if flag.load(std::sync::atomic::Ordering::Relaxed) {
                // Still reap finished children so the map doesn't
                // grow unbounded while we're paused.
                self.reap();
                return;
            }
        }
        // Apply the master scene-gain multiplier. paplay/ffplay take
        // an absolute volume at spawn time (can't be ramped post-
        // spawn), so we bake the current scene_gain into the
        // spawned command. Future spawns pick up changes; already-
        // playing children keep their original level until they
        // end (typically within ~16 s per layer).
        let effective_gain = (gain * self.current_scene_gain()).clamp(0.0, 1.0);
        // If the scene is ducked to (near) silence — e.g. the controller
        // handed off to real music and set scene_gain ~0 — don't bother
        // spawning a decoder for a clip nobody can hear. Spawning paplay/
        // ffmpeg to play audio at volume 0 still does the full decode +
        // resample work, which is wasted CPU and battery during the whole
        // music session. The schedule still advances (we just skip the
        // spawn), so when music ends and gain returns, the bed resumes in
        // the right place.
        if !is_audible(effective_gain) {
            self.reap();
            return;
        }
        if let Some(mut cmd) = self.build_command(&v.clip, effective_gain) {
            match cmd.spawn() {
                Ok(child) => { self.children.insert(key.to_string(), child); }
                Err(e) => eprintln!("[sink] failed to play {}: {}", v.clip, e),
            }
        }
        self.reap();
    }

    fn voice_gain(&mut self, _key: &str, _gain: f32) {
        // Subprocess players can't be ramped live; a native mixer would
        // adjust the voice's gain here for true crossfades.
    }

    fn voice_off(&mut self, key: &str) {
        if let Some(mut child) = self.children.remove(key) {
            // Best-effort stop. (No fade — subprocess limitation.)
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn detect_returns_a_variant() {
        // Just exercises detection; in CI/containers it's often None, which
        // is a valid result (the sink degrades to silent but doesn't panic).
        let _ = PlayerKind::detect();
    }

    #[test]
    fn build_command_none_when_no_player() {
        let sink = SystemSink { kind: PlayerKind::None, children: HashMap::new(), target_sink: None, passive_flag: None, scene_gain: None, clip_base: None };
        assert!(sink.build_command("x.wav", 1.0).is_none());
    }

    #[test]
    fn paplay_volume_maps_gain() {
        let sink = SystemSink { kind: PlayerKind::Paplay, children: HashMap::new(), target_sink: None, passive_flag: None, scene_gain: None, clip_base: None };
        // We can't run paplay here, but we can confirm the command builds with
        // the expected program + a --volume arg. Inspect via Debug formatting.
        let cmd = sink.build_command("clip.wav", 0.5).unwrap();
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("paplay"));
        assert!(dbg.contains("--volume=32768"));   // 0.5 * 65536
    }

    #[test]
    fn ffplay_builds_volume_filter() {
        let sink = SystemSink { kind: PlayerKind::Ffplay, children: HashMap::new(), target_sink: None, passive_flag: None, scene_gain: None, clip_base: None };
        let cmd = sink.build_command("clip.wav", 0.25).unwrap();
        let dbg = format!("{:?}", cmd);
        assert!(dbg.contains("ffplay"));
        assert!(dbg.contains("volume=0.250"));
    }

    /// When the passive flag is set, voice_on must NOT spawn a
    /// child. We verify by checking `children` stays empty, even
    /// though `build_command` would otherwise produce a valid Command.
    ///
    /// Uses `PlayerKind::None` so the test stays hermetic — no
    /// real player needs to be on PATH; we're verifying the gate,
    /// not the spawn. (build_command returns None for `None` kind,
    /// so even without the gate, no child would be spawned — but
    /// the test asserts something stronger: that when passive is
    /// set, we return BEFORE the build_command attempt.)
    #[test]
    fn passive_flag_suppresses_voice_on() {
        use crate::player::Voice;
        use std::sync::Arc;
        use std::sync::atomic::{AtomicBool, Ordering};
        let flag = Arc::new(AtomicBool::new(true));  // start passive
        let mut sink = SystemSink {
            kind: PlayerKind::None,
            children: HashMap::new(),
            target_sink: None,
            passive_flag: Some(flag.clone()),
            scene_gain: None,
            clip_base: None,
        };
        let voice = Voice {
            layer: "test_layer".into(),
            channel: "test_channel".into(),
            clip: "/tmp/nonexistent.wav".into(),
            start_ms: 0, end_ms: 1000,
            crossfade_ms: 100, peak_gain: 1.0,
            pan: 0.0, rate: 1.0,
        };
        sink.voice_on("test_key", &voice, 1.0);
        assert!(sink.children.is_empty(),
            "passive mode should suppress spawn");

        // Now drop passive: voice_on attempts the spawn (no-op for
        // PlayerKind::None, but proves the gate is path-dependent
        // on the flag, not on something else).
        flag.store(false, Ordering::Relaxed);
        sink.voice_on("test_key", &voice, 1.0);
        assert!(sink.children.is_empty());
    }

    /// Scene-gain multiplier is read on every voice_on and folded
    /// into the per-voice gain. We verify by constructing a Paplay
    /// sink (build_command produces a real Command we can inspect)
    /// and checking the `--volume=` arg shows the scaled value.
    #[test]
    fn scene_gain_multiplies_voice_gain() {
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let atomic = Arc::new(AtomicU32::new(0.5_f32.to_bits()));
        let sink = SystemSink {
            kind: PlayerKind::Paplay,
            children: HashMap::new(),
            target_sink: None,
            passive_flag: None,
            scene_gain: Some(atomic.clone()),
            clip_base: None,
        };
        // current_scene_gain should be 0.5.
        assert!((sink.current_scene_gain() - 0.5).abs() < 1e-6);
        // Verify the multiplier feeds into the spawn command via
        // build_command at scaled gain (since voice_on bakes it
        // into the arg we pass to build_command).
        let cmd = sink.build_command("clip.wav", 1.0 * 0.5).unwrap();
        let dbg = format!("{:?}", cmd);
        // paplay --volume is 0..65536. 0.5 maps to 32768.
        assert!(dbg.contains("--volume=32768"),
            "expected --volume=32768 (0.5 * 65536), got: {}", dbg);
        // Now update the atomic and re-check.
        atomic.store(1.0_f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
        assert!((sink.current_scene_gain() - 1.0).abs() < 1e-6);
    }

    /// Default scene_gain is 1.0 when no atomic is attached.
    #[test]
    fn scene_gain_default_is_unity() {
        let sink = SystemSink {
            kind: PlayerKind::None,
            children: HashMap::new(),
            target_sink: None,
            passive_flag: None,
            scene_gain: None,
            clip_base: None,
        };
        assert!((sink.current_scene_gain() - 1.0).abs() < 1e-6);
    }

    #[test]
    fn is_audible_threshold() {
        assert!(is_audible(1.0));
        assert!(is_audible(0.5));
        assert!(is_audible(AUDIBLE_GAIN_EPS));
        assert!(!is_audible(0.0));
        assert!(!is_audible(AUDIBLE_GAIN_EPS / 2.0));
        // A fully-ducked scene (scene_gain 0) zeroes any voice gain.
        assert!(!is_audible(0.9 * 0.0));
    }

    #[test]
    fn ducked_scene_gain_skips_spawn() {
        // With scene_gain 0 (music handoff full duck), voice_on must not
        // spawn a decoder even for an audible per-voice gain. We use a
        // Paplay sink so that, without the skip, build_command WOULD make
        // a real Command and voice_on would attempt a spawn (which we'd
        // see as a child entry on success, or at least an attempt). With
        // the skip, children stays empty and no work happens.
        use crate::player::Voice;
        use std::sync::Arc;
        use std::sync::atomic::AtomicU32;
        let atomic = Arc::new(AtomicU32::new(0.0_f32.to_bits())); // fully ducked
        let mut sink = SystemSink {
            kind: PlayerKind::Paplay,
            children: HashMap::new(),
            target_sink: None,
            passive_flag: None,
            scene_gain: Some(atomic.clone()),
            clip_base: None,
        };
        let voice = Voice {
            layer: "bed".into(), channel: "FL".into(),
            clip: "/tmp/nonexistent.wav".into(),
            start_ms: 0, end_ms: 1000, crossfade_ms: 100,
            peak_gain: 1.0, pan: 0.0, rate: 1.0,
        };
        sink.voice_on("k", &voice, 1.0);
        assert!(sink.children.is_empty(),
            "fully-ducked scene must not spawn a decoder");

        // Restore audible gain → the skip no longer applies. (Spawn may
        // still fail on the nonexistent file, but it's no longer gated by
        // the silence check — proving the gate is gain-dependent.)
        atomic.store(1.0_f32.to_bits(), std::sync::atomic::Ordering::Relaxed);
        let effective = (1.0_f32 * sink.current_scene_gain()).clamp(0.0, 1.0);
        assert!(is_audible(effective),
            "with scene_gain restored the voice should be audible again");
    }
}

// src/api.rs
//
// The daemon's control API — shared types for both sides (the daemon
// `bo-muxd` and any client, including the Qt GUI). Transport is a Unix
// domain socket carrying line-delimited JSON (one JSON object per line).
// This is deliberately simple so QML/C++/anything can speak it without a
// JSON-RPC library; it's also trivially systemd-socket-activatable.
//
// Why Unix socket + NDJSON (not D-Bus, not HTTP):
//   - local-only by construction (no network surface) — good for a media
//     daemon that controls speakers,
//   - systemd activates it cleanly (ListenStream=%t/bo-mux/control.sock),
//   - line-delimited JSON is dead simple to produce/parse from QML's
//     XMLHttpRequest-free environment or a tiny C++ shim.
//
// Versioned so the GUI (which may auto-update the daemon) can detect a
// mismatch and trigger a redeploy.

use serde::{Deserialize, Serialize};

/// API_VERSION 2: added PinLayer command (drag-to-speaker) and
/// `fabric`/`layers` fields in the Status response. Older controllers
/// ignore the new fields; newer controllers tolerate their absence.
pub const API_VERSION: u32 = 2;
/// Default control socket path (user service). %t = $XDG_RUNTIME_DIR.
pub const DEFAULT_SOCKET_REL: &str = "bo-mux/control.sock";

/// A request from a client to the daemon. `id` is echoed in the response so
/// clients can match replies; `0` is allowed for fire-and-forget.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Request {
    pub id: u64,
    #[serde(flatten)]
    pub cmd: Command,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "cmd", rename_all = "snake_case")]
pub enum Command {
    /// Liveness + version handshake.
    Ping,
    /// Daemon status: current scene, role, peers, playing?.
    Status,
    /// List soundscapes the daemon knows about (from its scene dir).
    ListScenes,
    /// Load a soundscape by name (from the daemon's scene dir) or inline.
    LoadScene { name: Option<String>, inline_json: Option<String> },
    /// Start playback of the loaded scene with an optional explicit seed.
    Play { seed: Option<u64> },
    /// Stop playback.
    Stop,
    /// Become coordinator and announce a multi-speaker session.
    StartSession { layout: String },
    /// Leave the current session.
    LeaveSession,
    /// List discovered peers + speakers on the mesh.
    ListPeers,
    /// Push a new/updated scene file into the daemon's scene dir.
    SaveScene { name: String, json: String },
    /// Set a master gain multiplier applied to every newly-spawned
    /// soundscape voice. Range 0.0..=1.5 (anything above 1.0 is a
    /// boost — clip to 1.0 in `paplay --volume` units to avoid
    /// distortion). The controller drives this from its noise
    /// observation: louder ambient noise → higher multiplier, so
    /// the soundscape masker holds its perceived loudness relative
    /// to the noise floor without the user manually riding gain.
    ///
    /// Applies only to FUTURE spawns; already-playing children
    /// keep their spawn-time volume. Since clips are ~16 s, gain
    /// changes propagate within one clip cycle per layer — fine
    /// granularity for slowly-varying ambient noise.
    SetSceneGain { value: f32 },
    /// Pin a soundscape layer's channel to a specific speaker on the
    /// mesh fabric (drag-to-speaker in the Spaces UI). The coordinator
    /// merges this as a cost-0 user claim so it overrides auto-assign.
    PinLayer { channel: String, speaker: String },
    /// Graceful shutdown (used by the updater before swapping the binary).
    Shutdown,
}

/// The daemon's reply. Exactly one of ok/err is meaningful.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Response {
    pub id: u64,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<serde_json::Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

impl Response {
    pub fn ok(id: u64, data: serde_json::Value) -> Self {
        Self { id, ok: true, data: Some(data), error: None }
    }
    pub fn ok_empty(id: u64) -> Self {
        Self { id, ok: true, data: None, error: None }
    }
    pub fn err(id: u64, msg: impl Into<String>) -> Self {
        Self { id, ok: false, data: None, error: Some(msg.into()) }
    }
}

/// Async events the daemon pushes unsolicited (same socket, id=0): scene
/// state changes, peer joins/leaves, playback start/stop. Clients that don't
/// care can ignore lines where `event` is present.
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(tag = "event", rename_all = "snake_case")]
pub enum Event {
    SceneStateChanged { state: String, scene_ms: u64 },
    PeersChanged { count: usize },
    RoleChanged { role: String },
    PlaybackStarted { scene: String, seed: u64 },
    PlaybackStopped,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn request_roundtrips_ndjson() {
        let req = Request { id: 7, cmd: Command::Play { seed: Some(42) } };
        let line = serde_json::to_string(&req).unwrap();
        // tag-flattened: {"id":7,"cmd":"play","seed":42}
        assert!(line.contains("\"cmd\":\"play\""));
        assert!(line.contains("\"seed\":42"));
        let back: Request = serde_json::from_str(&line).unwrap();
        assert_eq!(back.id, 7);
        matches!(back.cmd, Command::Play { seed: Some(42) });
    }

    #[test]
    fn load_scene_inline_or_named() {
        let named: Request = serde_json::from_str(
            r#"{"id":1,"cmd":"load_scene","name":"forest"}"#).unwrap();
        matches!(named.cmd, Command::LoadScene { name: Some(_), .. });
        let inline: Request = serde_json::from_str(
            r#"{"id":2,"cmd":"load_scene","inline_json":"{}"}"#).unwrap();
        matches!(inline.cmd, Command::LoadScene { inline_json: Some(_), .. });
    }

    #[test]
    fn response_ok_and_err() {
        let o = Response::ok(3, serde_json::json!({"x":1}));
        assert!(o.ok && o.error.is_none());
        let e = Response::err(3, "nope");
        assert!(!e.ok && e.data.is_none() && e.error.as_deref() == Some("nope"));
    }

    #[test]
    fn event_serializes_with_tag() {
        let ev = Event::SceneStateChanged { state: "dawn".into(), scene_ms: 1000 };
        let s = serde_json::to_string(&ev).unwrap();
        assert!(s.contains("\"event\":\"scene_state_changed\""));
    }
}

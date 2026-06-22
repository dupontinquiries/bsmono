// src/device_notify.rs
#![allow(dead_code)]
//
// Push-based device state subscription.
//
// The default control loop polls GetVolume on a timer to keep the UI's
// volume in sync with the actual device value. That's wasteful for two
// reasons:
//
//   1. Most of the time the volume hasn't changed, so the polling
//      request returns immediately and accomplishes nothing.
//   2. When the volume DOES change (e.g. someone presses the dial on
//      the device itself, or another controller adjusts it), there's
//      up to one poll-interval of staleness in the UI.
//
// Modern Mozart firmware exposes a notification endpoint that the
// controller can long-poll: the server holds the request open until
// something changes, then responds with the new state. The controller
// re-issues immediately to get the next change. Net effect:
//
//   * Zero polling traffic when nothing changes.
//   * Sub-50 ms reflection of state changes from any source.
//   * One persistent connection per device instead of one new
//     connection per poll-interval.
//
// This module handles the parsing + endpoint-discovery logic. The
// actual HTTP plumbing lives in the worker thread. We try a small
// list of known endpoint shapes until one returns 200; if all fail,
// the caller falls back to polling.
//
// Endpoints attempted (in order):
//   * /BeoNotify/Notifications              ← newest Mozart firmware
//   * /BeoZone/Zone/Stream/PlayQueue/notif  ← intermediate firmware
//   * /api/getData?path=BeoSound%3A%2Fnotif ← oldest BeoSound
//
// Each shape produces a slightly different JSON envelope. We parse them
// all into the same `DeviceNotification` enum.

use serde_json::Value;

/// Endpoint paths to attempt, in preference order. Each is appended to
/// the device base URL.
pub const CANDIDATE_ENDPOINTS: &[&str] = &[
    "/BeoNotify/Notifications",
    "/BeoZone/Zone/Stream/PlayQueue/notif",
    "/api/getData?path=BeoSound%3A%2Fnotif",
];

/// A device-side state change extracted from a notification payload.
/// More variants get added as new firmware shapes are discovered.
#[derive(Debug, Clone, PartialEq)]
pub enum DeviceNotification {
    /// Volume changed (any source: physical dial, remote, another controller)
    Volume(i32),
    /// Now-playing track / source metadata updated
    NowPlaying { title: String, artist: String, source: String },
    /// Mute toggled
    Mute(bool),
    /// Standby state changed
    Standby(bool),
    /// Source selection changed
    Source(String),
    /// Transport state changed (playing / paused / stopped)
    Transport(String),
    /// Catch-all: parsed JSON but didn't match a known shape. Keeps
    /// the raw value so the worker can log it for future shape work.
    Unknown(Value),
}

/// Parse a raw JSON response body into zero or more notifications.
/// Different firmware shapes produce different envelopes; we try each
/// known one and emit a notification for every match. Empty result means
/// the payload didn't match any known shape (caller should log+ignore).
pub fn parse(json: &Value) -> Vec<DeviceNotification> {
    let mut out = Vec::new();

    // Shape 1: BeoNotify wrapping with `notification` array.
    //   { "notification": [{ "type": "VOLUME", "data": { "speaker": { "level": 35 } } }] }
    if let Some(arr) = json.pointer("/notification").and_then(|v| v.as_array()) {
        for n in arr {
            if let Some(notif) = parse_beonotify_item(n) {
                out.push(notif);
            }
        }
    }

    // Shape 2: BeoZone direct field updates.
    //   { "volume": { "speaker": { "level": 35 } } }
    //   { "primarySource": { "active": "music" } }
    if let Some(v) = json.pointer("/volume/speaker/level").and_then(|v| v.as_i64()) {
        out.push(DeviceNotification::Volume(v as i32));
    }
    if let Some(v) = json.pointer("/volume/speaker/muted").and_then(|v| v.as_bool()) {
        out.push(DeviceNotification::Mute(v));
    }
    if let Some(s) = json.pointer("/primarySource/active").and_then(|v| v.as_str()) {
        out.push(DeviceNotification::Source(s.to_string()));
    }

    // Shape 3: Beosound /api/getData "value" array (same format as our
    // existing GetVolume parser).
    //   [{"i32_": 35, "type": "i32_"}]
    if let Some(arr) = json.as_array() {
        for item in arr {
            if let Some(v) = item.get("i32_").and_then(|v| v.as_i64()) {
                // Pure integer — can only be volume in this context.
                out.push(DeviceNotification::Volume(v as i32));
            }
        }
    }

    // Shape 4: Now-playing metadata bundle.
    //   { "nowPlaying": { "track": { "title": "...", "artist": "...", "source": "..." } } }
    if let Some(np) = json.pointer("/nowPlaying/track") {
        let title = np.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let artist = np.get("artist").and_then(|v| v.as_str()).unwrap_or("").to_string();
        let source = np.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string();
        if !title.is_empty() || !artist.is_empty() {
            out.push(DeviceNotification::NowPlaying { title, artist, source });
        }
    }

    // Shape 5: Standby / transport.
    if let Some(b) = json.pointer("/standby/active").and_then(|v| v.as_bool()) {
        out.push(DeviceNotification::Standby(b));
    }
    if let Some(s) = json.pointer("/playback/state").and_then(|v| v.as_str()) {
        out.push(DeviceNotification::Transport(s.to_string()));
    }

    // If we got nothing but the JSON had content, emit Unknown so the
    // worker can log it. Helps shape discovery on new firmware.
    if out.is_empty() && !json.is_null() {
        out.push(DeviceNotification::Unknown(json.clone()));
    }

    out
}

fn parse_beonotify_item(n: &Value) -> Option<DeviceNotification> {
    let typ = n.get("type").and_then(|v| v.as_str())?;
    let data = n.get("data");
    match typ.to_ascii_uppercase().as_str() {
        "VOLUME" => {
            let lvl = data?
                .pointer("/speaker/level")
                .and_then(|v| v.as_i64())?;
            Some(DeviceNotification::Volume(lvl as i32))
        }
        "MUTE" | "MUTED" => {
            let m = data?.pointer("/muted").and_then(|v| v.as_bool())
                .or_else(|| data?.pointer("/speaker/muted").and_then(|v| v.as_bool()))?;
            Some(DeviceNotification::Mute(m))
        }
        "NOW_PLAYING" | "NOWPLAYING" => {
            let title = data?.get("title").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let artist = data?.get("artist").and_then(|v| v.as_str()).unwrap_or("").to_string();
            let source = data?.get("source").and_then(|v| v.as_str()).unwrap_or("").to_string();
            Some(DeviceNotification::NowPlaying { title, artist, source })
        }
        "SOURCE" => {
            let s = data?.get("active").and_then(|v| v.as_str())?;
            Some(DeviceNotification::Source(s.to_string()))
        }
        "STANDBY" => {
            let b = data?.get("active").and_then(|v| v.as_bool())?;
            Some(DeviceNotification::Standby(b))
        }
        "TRANSPORT" | "PLAYBACK" => {
            let s = data?.get("state").and_then(|v| v.as_str())?;
            Some(DeviceNotification::Transport(s.to_string()))
        }
        _ => None,
    }
}

/// Build the long-poll URL for a given device base + endpoint path.
/// Some endpoints take a `since` parameter (sequence number); the
/// caller passes 0 on first call, then the highest seen.
pub fn long_poll_url(device_base: &str, endpoint: &str, since: u64) -> String {
    let base = device_base.trim_end_matches('/');
    // BeoNotify-style: append ?id=<since>
    if endpoint.contains("BeoNotify") {
        format!("{}{}?id={}&timeout=30", base, endpoint, since)
    } else if endpoint.contains('?') {
        // Endpoint already has query params (e.g. the getData shape).
        format!("{}{}", base, endpoint)
    } else {
        format!("{}{}", base, endpoint)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn parse_empty_object_emits_nothing_useful() {
        let v = json!({});
        let n = parse(&v);
        // Either empty or Unknown(null-ish) — both fine
        assert!(n.is_empty() || matches!(n[0], DeviceNotification::Unknown(_)));
    }

    #[test]
    fn parse_beonotify_volume() {
        let v = json!({
            "notification": [
                { "type": "VOLUME", "data": { "speaker": { "level": 42 } } }
            ]
        });
        let n = parse(&v);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0], DeviceNotification::Volume(42));
    }

    #[test]
    fn parse_beonotify_mute_lowercase() {
        let v = json!({
            "notification": [
                { "type": "mute", "data": { "muted": true } }
            ]
        });
        let n = parse(&v);
        assert_eq!(n.len(), 1);
        assert_eq!(n[0], DeviceNotification::Mute(true));
    }

    #[test]
    fn parse_beozone_direct_volume() {
        let v = json!({ "volume": { "speaker": { "level": 17 } } });
        let n = parse(&v);
        assert!(n.contains(&DeviceNotification::Volume(17)));
    }

    #[test]
    fn parse_beozone_direct_mute() {
        let v = json!({ "volume": { "speaker": { "muted": false } } });
        let n = parse(&v);
        assert!(n.contains(&DeviceNotification::Mute(false)));
    }

    #[test]
    fn parse_beosound_value_array() {
        // The same shape our GetVolume already handles.
        let v = json!([{ "i32_": 25, "type": "i32_" }]);
        let n = parse(&v);
        assert_eq!(n[0], DeviceNotification::Volume(25));
    }

    #[test]
    fn parse_now_playing_full() {
        let v = json!({
            "nowPlaying": { "track": {
                "title": "Faded",
                "artist": "ZHU",
                "source": "spotify"
            }}
        });
        let n = parse(&v);
        assert!(n.contains(&DeviceNotification::NowPlaying {
            title: "Faded".into(),
            artist: "ZHU".into(),
            source: "spotify".into(),
        }));
    }

    #[test]
    fn parse_standby() {
        let v = json!({ "standby": { "active": true } });
        let n = parse(&v);
        assert!(n.contains(&DeviceNotification::Standby(true)));
    }

    #[test]
    fn parse_transport_state() {
        let v = json!({ "playback": { "state": "playing" } });
        let n = parse(&v);
        assert!(n.contains(&DeviceNotification::Transport("playing".into())));
    }

    #[test]
    fn parse_multiple_notifications_in_one_payload() {
        // BeoNotify can batch
        let v = json!({
            "notification": [
                { "type": "VOLUME", "data": { "speaker": { "level": 30 } } },
                { "type": "SOURCE", "data": { "active": "music" } }
            ]
        });
        let n = parse(&v);
        assert_eq!(n.len(), 2);
        assert!(n.contains(&DeviceNotification::Volume(30)));
        assert!(n.contains(&DeviceNotification::Source("music".into())));
    }

    #[test]
    fn parse_unknown_shape_returns_unknown() {
        let v = json!({ "futureFirmwareThing": { "weird": true } });
        let n = parse(&v);
        assert_eq!(n.len(), 1);
        assert!(matches!(n[0], DeviceNotification::Unknown(_)));
    }

    #[test]
    fn long_poll_url_appends_id_for_beonotify() {
        let u = long_poll_url("http://192.168.2.12", "/BeoNotify/Notifications", 5);
        assert!(u.contains("?id=5"));
        assert!(u.contains("timeout"));
    }

    #[test]
    fn long_poll_url_preserves_existing_query() {
        let u = long_poll_url("http://192.168.2.12",
                              "/api/getData?path=BeoSound%3A%2Fnotif", 0);
        assert_eq!(u, "http://192.168.2.12/api/getData?path=BeoSound%3A%2Fnotif");
    }

    #[test]
    fn long_poll_url_trims_trailing_slash() {
        let u = long_poll_url("http://192.168.2.12/", "/BeoNotify/Notifications", 0);
        assert!(!u.contains("//Be"), "double slash in {}", u);
    }

    #[test]
    fn candidate_endpoints_are_distinct() {
        use std::collections::HashSet;
        let set: HashSet<&&str> = CANDIDATE_ENDPOINTS.iter().collect();
        assert_eq!(set.len(), CANDIDATE_ENDPOINTS.len());
    }

    #[test]
    fn parse_handles_null_gracefully() {
        let v = Value::Null;
        let n = parse(&v);
        assert!(n.is_empty(), "null should produce no notifications");
    }
}

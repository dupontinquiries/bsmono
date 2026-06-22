// src/mozart_source.rs
#![allow(dead_code)]
//
// Probe the Beosound device's *active source* via the Mozart REST API.
//
// Why this exists: tapping play on a track only works if the device's
// current source is one that accepts arbitrary URI playback (Network /
// DLNA / Music). When the active source is Spotify, AirPlay,
// Bluetooth, line-in, or TV-input, our DLNA SOAP attempt fails with
// `701 Transition not available` and the user hears silence with no
// indication why. Knowing the source UPFRONT lets us:
//
//   * Show the current source as a chip in the NowPlayingBar.
//   * Pre-flight a play attempt: if the source can't accept URIs,
//     surface a clear toast BEFORE dispatching (no wasted SOAP round
//     trips, no five-minute "is it broken?" debugging session).
//
// We don't know the exact JSON shape Mozart returns — the documented
// API is sparse and varies across firmware versions. So we try a list
// of candidate paths and parse multiple plausible response shapes
// tolerantly. The first one that yields a recognizable source wins.

use std::time::Duration;

/// What the device is currently playing FROM.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceState {
    /// Stable id (e.g. "spotify", "network", "linein", "deezer", "tunein").
    pub id: Option<String>,
    /// Human-readable label (e.g. "Spotify", "Network", "Line-in").
    pub label: Option<String>,
    /// Source family — used to decide whether DLNA URI playback works.
    pub family: SourceFamily,
    /// The candidate path that matched (for diagnostic / sticky use).
    pub via_path: String,
}

/// Family classification. DLNA-capable sources accept arbitrary URI
/// playback via SOAP SetAVTransportURI; the others don't.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceFamily {
    /// Network / DLNA / "Music server" — accepts arbitrary URIs.
    NetworkDlna,
    /// Spotify Connect — separate playback path entirely.
    Spotify,
    /// AirPlay receiver mode.
    AirPlay,
    /// Bluetooth A2DP.
    Bluetooth,
    /// Physical line-in (RCA / 3.5mm / optical).
    LineIn,
    /// TV / HDMI input.
    Tv,
    /// Internet radio (TuneIn or built-in).
    InternetRadio,
    /// Streaming service that isn't Spotify (Deezer, Tidal, etc).
    OtherStreaming,
    /// We saw a source name but can't classify it.
    Unknown,
}

impl SourceFamily {
    /// Can we drive playback via DLNA SetAVTransportURI when in this
    /// source family?
    pub fn accepts_uri_playback(&self) -> bool {
        matches!(self, SourceFamily::NetworkDlna)
    }

    /// Friendly text describing why playback won't work in this source.
    /// Used for toasts. Returns "" when the source DOES accept playback.
    pub fn cant_play_reason(&self) -> &'static str {
        match self {
            SourceFamily::NetworkDlna => "",
            SourceFamily::Spotify =>
                "Device is in Spotify mode — use Spotify Connect to play, \
                 or switch the source to Network on the device.",
            SourceFamily::AirPlay =>
                "Device is in AirPlay mode — send audio from your phone's \
                 AirPlay menu, or switch the source to Network on the device.",
            SourceFamily::Bluetooth =>
                "Device is in Bluetooth mode — pair from your phone, or \
                 switch the source to Network on the device.",
            SourceFamily::LineIn =>
                "Device is in Line-in mode — change source to Network on \
                 the device.",
            SourceFamily::Tv =>
                "Device is in TV mode — change source to Network on the device.",
            SourceFamily::InternetRadio =>
                "Device is playing internet radio — change source to \
                 Network on the device to play your own tracks.",
            SourceFamily::OtherStreaming =>
                "Device is playing from a streaming service — change \
                 source to Network on the device.",
            SourceFamily::Unknown =>
                "Source isn't recognized as DLNA-compatible. Switch the \
                 source to Network on the device.",
        }
    }
}

/// Classify a source id string into a family. The mapping is based on
/// the source identifiers Mozart and ASE-era firmware use.
pub fn classify(id: &str) -> SourceFamily {
    let lower = id.trim().to_ascii_lowercase();
    if lower.is_empty() { return SourceFamily::Unknown; }
    if lower.contains("spotify") { return SourceFamily::Spotify; }
    if lower.contains("airplay") { return SourceFamily::AirPlay; }
    if lower.contains("bluetooth") || lower.starts_with("bt") {
        return SourceFamily::Bluetooth;
    }
    if lower.contains("linein") || lower.contains("line-in")
       || lower.contains("aux") || lower.contains("toslink")
       || lower.contains("optical") {
        return SourceFamily::LineIn;
    }
    if lower.contains("tv") || lower.contains("hdmi") {
        return SourceFamily::Tv;
    }
    if lower.contains("tunein") || lower.contains("radio") {
        return SourceFamily::InternetRadio;
    }
    if lower.contains("deezer") || lower.contains("tidal")
       || lower.contains("qobuz") || lower.contains("amazon")
       || lower.contains("napster") {
        return SourceFamily::OtherStreaming;
    }
    if lower.contains("network") || lower.contains("dlna")
       || lower.contains("music") || lower.contains("media")
       || lower.contains("upnp") || lower.contains("server") {
        return SourceFamily::NetworkDlna;
    }
    SourceFamily::Unknown
}

/// Candidate REST paths we probe in order. Each one is a different
/// possible Mozart shape; the first 200 OK with a parseable body wins.
pub fn candidate_paths() -> &'static [&'static str] {
    &[
        "/api/getData?path=BeoSound%3A%2Fsources%2Factive",
        "/api/getData?path=BeoSound%3A%2FactiveSource",
        "/api/getData?path=BeoSound%3A%2Fsource",
        "/api/getData?path=BeoSound%3A%2Fsources",
        "/api/getData?path=BeoSound%3A%2FplayerStatus",
        "/api/getData?path=BeoZone%3A%2FactiveSource",
        "/BeoZone/Zone/Sources/active",
        "/BeoZone/Zone/Sources",
    ]
}

/// Parse a Mozart-style JSON response into a SourceState. The shape
/// varies across firmware: a few we've observed are:
///
///   * [{"string_": "spotify", "type": "string_"}]                 (setVolume-style)
///   * [{"beoSoundSource": {"id": "spotify", "label": "Spotify"},
///      "type": "beoSoundSource"}]
///   * {"primary": {"id": "deezer", "sourceType": "music"}, "secondary": null}
///   * {"active": {"id": "network", "name": "Network"}}
///   * {"id": "linein", "name": "Line-in"}
///
/// We scan recursively for any object containing both an id/name-ish
/// pair OR a bare string value that classifies as a known source.
/// Returns None when nothing recognizable is found.
pub fn parse(json_text: &str) -> Option<(Option<String>, Option<String>)> {
    let v: serde_json::Value = serde_json::from_str(json_text).ok()?;
    let mut id: Option<String> = None;
    let mut label: Option<String> = None;
    scan(&v, &mut id, &mut label);
    if id.is_none() && label.is_none() { return None; }
    Some((id, label))
}

/// Recursively walk the JSON tree looking for source-like fields.
/// First match wins. Searches for these field names in priority order:
///   id, sourceId, source_id, sourceType  → id
///   label, name, title, displayName       → label
///   string_                               → both (used for bare-string responses)
///
/// When a wrapper-style object is present (a key named primary /
/// active / current), we descend INTO that key FIRST so multi-source
/// payloads like `{primary: {id: "x"}, meta: {id: "junk"}}` resolve
/// to "x" regardless of object iteration order. The wrapper bias
/// matches Mozart's actual shape and is what users intuitively expect.
fn scan(v: &serde_json::Value, id: &mut Option<String>, label: &mut Option<String>) {
    if id.is_some() && label.is_some() { return; }
    match v {
        serde_json::Value::Object(map) => {
            // Priority 1: explicit id/sourceId/sourceType.
            for k in &["id", "sourceId", "source_id", "sourceType"] {
                if id.is_none() {
                    if let Some(serde_json::Value::String(s)) = map.get(*k) {
                        if !s.is_empty() { *id = Some(s.clone()); }
                    }
                }
            }
            // Priority 2: label / name / displayName.
            for k in &["label", "name", "title", "displayName"] {
                if label.is_none() {
                    if let Some(serde_json::Value::String(s)) = map.get(*k) {
                        if !s.is_empty() { *label = Some(s.clone()); }
                    }
                }
            }
            // Priority 3: bare-string field "string_" — used by Mozart
            // for simple-valued getData responses. We treat it as both
            // id AND label (whichever isn't yet set).
            if let Some(serde_json::Value::String(s)) = map.get("string_") {
                if !s.is_empty() {
                    if id.is_none()    { *id    = Some(s.clone()); }
                    if label.is_none() { *label = Some(s.clone()); }
                }
            }
            // Wrapper bias: descend into the canonical "active source"
            // wrapper keys BEFORE iterating arbitrary children. This
            // makes parsing order-independent: when an object has both
            // `primary: {id: ...}` and `meta: {id: ...}`, we pick the
            // primary one even if serde_json iterates `meta` first.
            for wrapper_key in &["primary", "active", "current",
                                 "beoSoundSource", "source"] {
                if id.is_some() && label.is_some() { return; }
                if let Some(inner) = map.get(*wrapper_key) {
                    scan(inner, id, label);
                }
            }
            // Recurse into remaining objects/arrays. Wrapper keys we
            // already visited above are visited again here — that's
            // fine, scan() short-circuits once both fields are set.
            for (_k, val) in map {
                if id.is_some() && label.is_some() { return; }
                scan(val, id, label);
            }
        }
        serde_json::Value::Array(arr) => {
            for item in arr {
                if id.is_some() && label.is_some() { return; }
                scan(item, id, label);
            }
        }
        _ => {}
    }
}

/// HTTP probe: tries the candidate paths against `base_url`, returns
/// the first SourceState that parses. `client.timeout` per request is
/// short (2 s) so the whole probe finishes in under a few seconds
/// even when all paths 404.
pub async fn probe(base_url: &str, client: &reqwest::Client) -> Option<SourceState> {
    let base = base_url.trim_end_matches('/');
    for path in candidate_paths() {
        let url = format!("{}{}", base, path);
        let resp = match client.get(&url)
            .timeout(Duration::from_secs(2))
            .send().await
        {
            Ok(r) => r,
            Err(_) => continue,
        };
        if !resp.status().is_success() { continue; }
        let body = match resp.text().await { Ok(t) => t, Err(_) => continue };
        if let Some((id, label)) = parse(&body) {
            let family = id.as_deref()
                .or(label.as_deref())
                .map(classify)
                .unwrap_or(SourceFamily::Unknown);
            return Some(SourceState {
                id, label, family,
                via_path: path.to_string(),
            });
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classify_known_sources() {
        assert_eq!(classify("spotify"),       SourceFamily::Spotify);
        assert_eq!(classify("Spotify"),       SourceFamily::Spotify);
        assert_eq!(classify("airplay"),       SourceFamily::AirPlay);
        assert_eq!(classify("AirPlay 2"),     SourceFamily::AirPlay);
        assert_eq!(classify("bluetooth"),     SourceFamily::Bluetooth);
        assert_eq!(classify("linein"),        SourceFamily::LineIn);
        assert_eq!(classify("line-in"),       SourceFamily::LineIn);
        assert_eq!(classify("tv"),            SourceFamily::Tv);
        assert_eq!(classify("hdmi"),          SourceFamily::Tv);
        assert_eq!(classify("tunein"),        SourceFamily::InternetRadio);
        assert_eq!(classify("deezer"),        SourceFamily::OtherStreaming);
        assert_eq!(classify("network"),       SourceFamily::NetworkDlna);
        assert_eq!(classify("dlna"),          SourceFamily::NetworkDlna);
        assert_eq!(classify("music"),         SourceFamily::NetworkDlna);
        assert_eq!(classify("media_server"),  SourceFamily::NetworkDlna);
    }

    #[test]
    fn classify_unknown_returns_unknown() {
        assert_eq!(classify(""),              SourceFamily::Unknown);
        assert_eq!(classify("xyz123"),        SourceFamily::Unknown);
    }

    #[test]
    fn classify_accepts_uri_playback() {
        assert!(SourceFamily::NetworkDlna.accepts_uri_playback());
        assert!(!SourceFamily::Spotify.accepts_uri_playback());
        assert!(!SourceFamily::Bluetooth.accepts_uri_playback());
        assert!(!SourceFamily::LineIn.accepts_uri_playback());
        assert!(!SourceFamily::Unknown.accepts_uri_playback());
    }

    #[test]
    fn cant_play_reason_present_for_all_blocking_sources() {
        // Every non-NetworkDlna family must have a non-empty reason
        // string so the toast always has something useful to say.
        for f in [SourceFamily::Spotify, SourceFamily::AirPlay,
                  SourceFamily::Bluetooth, SourceFamily::LineIn,
                  SourceFamily::Tv, SourceFamily::InternetRadio,
                  SourceFamily::OtherStreaming, SourceFamily::Unknown] {
            assert!(!f.cant_play_reason().is_empty(), "missing reason for {:?}", f);
        }
        assert!(SourceFamily::NetworkDlna.cant_play_reason().is_empty());
    }

    #[test]
    fn parse_string_shape_volume_style() {
        // The same shape as setVolume returns — a bare-value array.
        let body = r#"[{"string_":"spotify","type":"string_"}]"#;
        let (id, label) = parse(body).unwrap();
        assert_eq!(id.as_deref(),    Some("spotify"));
        assert_eq!(label.as_deref(), Some("spotify"));
    }

    #[test]
    fn parse_typed_source_shape() {
        let body = r#"[{"beoSoundSource":{"id":"network","label":"Network"},"type":"beoSoundSource"}]"#;
        let (id, label) = parse(body).unwrap();
        assert_eq!(id.as_deref(),    Some("network"));
        assert_eq!(label.as_deref(), Some("Network"));
    }

    #[test]
    fn parse_primary_secondary_shape() {
        let body = r#"{"primary":{"id":"deezer","sourceType":"music","name":"Deezer"},"secondary":null}"#;
        let (id, label) = parse(body).unwrap();
        assert_eq!(id.as_deref(),    Some("deezer"));
        assert_eq!(label.as_deref(), Some("Deezer"));
    }

    #[test]
    fn parse_active_wrapper_shape() {
        let body = r#"{"active":{"id":"linein","name":"Line-in"}}"#;
        let (id, label) = parse(body).unwrap();
        assert_eq!(id.as_deref(),    Some("linein"));
        assert_eq!(label.as_deref(), Some("Line-in"));
    }

    #[test]
    fn parse_flat_shape() {
        let body = r#"{"id":"airplay","name":"AirPlay 2"}"#;
        let (id, label) = parse(body).unwrap();
        assert_eq!(id.as_deref(),    Some("airplay"));
        assert_eq!(label.as_deref(), Some("AirPlay 2"));
    }

    #[test]
    fn parse_returns_none_for_empty_object() {
        assert!(parse("{}").is_none());
    }

    #[test]
    fn parse_returns_none_for_garbage() {
        assert!(parse("not valid json").is_none());
        assert!(parse("[]").is_none());
    }

    #[test]
    fn parse_takes_first_id_at_depth() {
        // When there are multiple id-shaped fields, the depth-first
        // scan should pick the FIRST one (typically the outermost).
        // Mozart sometimes wraps with {payload:{id:"x"}, meta:{id:"y"}}
        // and we want the payload id.
        let body = r#"{"primary":{"id":"network","name":"Network"},"meta":{"id":"ignored"}}"#;
        let (id, _) = parse(body).unwrap();
        assert_eq!(id.as_deref(), Some("network"));
    }

    #[test]
    fn candidate_paths_nonempty_and_url_encoded() {
        let paths = candidate_paths();
        assert!(!paths.is_empty());
        // The BeoSound paths use %3A%2F for ':/' since they're URL-encoded.
        let beosound_paths: Vec<_> = paths.iter()
            .filter(|p| p.contains("BeoSound"))
            .collect();
        assert!(!beosound_paths.is_empty());
        for p in &beosound_paths {
            assert!(p.contains("%3A%2F"),
                "BeoSound path missing URL-encoded colon-slash: {}", p);
        }
    }
}

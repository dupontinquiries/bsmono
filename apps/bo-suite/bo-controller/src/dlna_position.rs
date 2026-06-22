// src/dlna_position.rs
#![allow(dead_code)]
//
// DLNA / UPnP AVTransport playback verification primitives.
//
// Why this exists: when Strategy 6 or 7 in play_uri.rs sends
// SetAVTransportURI + Play to a Mozart device's discovered control
// URL, we get an HTTP 200 back — but that only confirms the SOAP
// layer accepted the action. The device might still be in a state
// where playback won't actually start (paused renderer, source
// mismatch, transient error mid-transition). The standard UPnP way
// to confirm "is playback alive?" is two follow-up SOAP calls:
//
//   GetTransportInfo  → returns CurrentTransportState (PLAYING /
//                       PAUSED_PLAYBACK / STOPPED / TRANSITIONING /
//                       NO_MEDIA_PRESENT)
//
//   GetPositionInfo   → returns Track, TrackDuration, TrackURI,
//                       RelTime (current position), AbsTime, etc.
//
// Both are SOAP calls against the same control URL we already
// discovered. This module provides:
//
//   * The envelope builders for both actions.
//   * Tolerant XML response parsers that don't rely on a full
//     namespace-aware XML parser (UPnP responses are tiny and
//     well-shaped enough that string scanning is fine).
//   * Async wrappers that call them via reqwest.
//
// We do NOT add periodic polling here. That's a deliberate scoping
// call — a polling loop would tie this module to the worker's
// lifecycle and complicate cleanup on device switches. The intended
// use is: after a SetAVTransportURI+Play succeeds, do one
// GetTransportInfo within ~1.5 s to confirm CurrentTransportState
// is PLAYING (or TRANSITIONING, which usually settles to PLAYING).
// If it's STOPPED or NO_MEDIA_PRESENT, the play attempt failed
// despite the 200 — surface that as the real outcome.

use std::time::Duration;

/// Result of a GetTransportInfo call.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TransportInfo {
    /// Current playback state. We parse the standard values and
    /// preserve unknown ones verbatim.
    pub state: TransportState,
    /// Status from the device — usually "OK". Free-form on
    /// vendor-extended firmware.
    pub status: String,
    /// Playback speed multiplier as a string (e.g. "1", "0.5", "2").
    pub speed: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TransportState {
    Playing,
    PausedPlayback,
    Stopped,
    Transitioning,
    NoMediaPresent,
    /// Anything else the device reported (preserved as the raw string).
    Other(String),
}

impl TransportState {
    /// Did the play action actually succeed? PLAYING and
    /// TRANSITIONING both count as success (TRANSITIONING usually
    /// settles to PLAYING within a few hundred ms). STOPPED /
    /// NO_MEDIA_PRESENT / PAUSED indicate the SetAVTransportURI got
    /// processed but the device didn't actually start playback.
    pub fn indicates_playing(&self) -> bool {
        matches!(self, TransportState::Playing | TransportState::Transitioning)
    }

    /// Short human-readable label for log lines + UI.
    pub fn label(&self) -> &str {
        match self {
            TransportState::Playing         => "PLAYING",
            TransportState::PausedPlayback  => "PAUSED",
            TransportState::Stopped         => "STOPPED",
            TransportState::Transitioning   => "TRANSITIONING",
            TransportState::NoMediaPresent  => "NO_MEDIA",
            TransportState::Other(s)        => s.as_str(),
        }
    }
}

/// Parse a CurrentTransportState string into the typed enum.
pub fn parse_transport_state(s: &str) -> TransportState {
    let trimmed = s.trim();
    match trimmed.to_ascii_uppercase().as_str() {
        "PLAYING"           => TransportState::Playing,
        "PAUSED_PLAYBACK"   => TransportState::PausedPlayback,
        "PAUSED"            => TransportState::PausedPlayback,
        "STOPPED"           => TransportState::Stopped,
        "TRANSITIONING"     => TransportState::Transitioning,
        "NO_MEDIA_PRESENT"  => TransportState::NoMediaPresent,
        _ => TransportState::Other(trimmed.to_string()),
    }
}

/// Result of a GetPositionInfo call. All time fields are HH:MM:SS
/// strings (UPnP encodes them that way). The track index is 1-based
/// per the spec; 0 means "no track".
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PositionInfo {
    pub track: u32,
    pub track_duration: String,
    pub track_uri: String,
    pub current_position: String,
    pub abs_time: String,
}

/// Build a GetTransportInfo SOAP envelope. Tiny, hand-built — no XML
/// library dependency.
pub fn build_get_transport_info_envelope() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:GetTransportInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
  </u:GetTransportInfo>
 </s:Body>
</s:Envelope>"#.to_string()
}

/// Build a GetPositionInfo SOAP envelope.
pub fn build_get_position_info_envelope() -> String {
    r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:GetPositionInfo xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
  </u:GetPositionInfo>
 </s:Body>
</s:Envelope>"#.to_string()
}

/// Parse a GetTransportInfoResponse SOAP body. Returns None when the
/// body doesn't contain the expected tags.
pub fn parse_transport_info_response(xml: &str) -> Option<TransportInfo> {
    // Required tag — fail closed if absent.
    let state_str = extract_inner_ci(xml, "CurrentTransportState")?;
    let status = extract_inner_ci(xml, "CurrentTransportStatus")
        .unwrap_or_else(|| "".to_string());
    let speed = extract_inner_ci(xml, "CurrentSpeed")
        .unwrap_or_else(|| "1".to_string());
    Some(TransportInfo {
        state: parse_transport_state(&state_str),
        status,
        speed,
    })
}

/// Parse a GetPositionInfoResponse SOAP body. Returns None when
/// neither RelTime nor AbsTime is present — that's the minimum we
/// need to consider the response useful.
pub fn parse_position_info_response(xml: &str) -> Option<PositionInfo> {
    // The body must contain at least one position-shaped tag to be
    // useful. We probe both since some firmware emits one and not the
    // other.
    let rel = extract_inner_ci(xml, "RelTime");
    let abs = extract_inner_ci(xml, "AbsTime");
    if rel.is_none() && abs.is_none() { return None; }
    let track: u32 = extract_inner_ci(xml, "Track")
        .and_then(|s| s.parse().ok())
        .unwrap_or(0);
    let track_duration = extract_inner_ci(xml, "TrackDuration")
        .unwrap_or_default();
    let track_uri = extract_inner_ci(xml, "TrackURI")
        .unwrap_or_default();
    Some(PositionInfo {
        track,
        track_duration,
        track_uri,
        current_position: rel.unwrap_or_default(),
        abs_time: abs.unwrap_or_default(),
    })
}

/// Parse a UPnP duration string (`HH:MM:SS` or `H:MM:SS.frac`) into
/// total seconds as a float. Used by the future progress-bar UI; for
/// now it just gives us a sanity check on the response shape.
/// Returns None on malformed input. "NOT_IMPLEMENTED" (a common
/// vendor sentinel for "track duration unknown") returns None too.
pub fn parse_upnp_duration(s: &str) -> Option<f64> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.eq_ignore_ascii_case("NOT_IMPLEMENTED") {
        return None;
    }
    let mut parts = trimmed.split(':');
    let h: u64 = parts.next()?.parse().ok()?;
    let m: u64 = parts.next()?.parse().ok()?;
    let s_str = parts.next()?;
    if parts.next().is_some() { return None; }
    let seconds: f64 = s_str.parse().ok()?;
    // Rust's f64::FromStr accepts "NaN", "inf", "-inf" verbatim. A
    // malicious or buggy renderer could send these and we'd hand the
    // caller a non-finite duration that breaks comparisons and arithmetic
    // downstream (NaN propagates silently through progress-bar
    // calculations). Reject here.
    if !seconds.is_finite() { return None; }
    let total = (h as f64) * 3600.0 + (m as f64) * 60.0 + seconds;
    if !total.is_finite() { return None; }
    Some(total)
}

/// Async wrapper: send GetTransportInfo to the given control URL,
/// parse the response into TransportInfo. Short timeout (3s) so a
/// hung renderer doesn't stall the caller's polling loop.
pub async fn get_transport_info(control_url: &str, client: &reqwest::Client)
    -> Option<TransportInfo>
{
    let body = build_get_transport_info_envelope();
    let resp = client.post(control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction",
                "\"urn:schemas-upnp-org:service:AVTransport:1#GetTransportInfo\"")
        .body(body)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() { return None; }
    let text = resp.text().await.ok()?;
    parse_transport_info_response(&text)
}

/// Async wrapper: send GetPositionInfo to the given control URL.
pub async fn get_position_info(control_url: &str, client: &reqwest::Client)
    -> Option<PositionInfo>
{
    let body = build_get_position_info_envelope();
    let resp = client.post(control_url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction",
                "\"urn:schemas-upnp-org:service:AVTransport:1#GetPositionInfo\"")
        .body(body)
        .timeout(Duration::from_secs(3))
        .send()
        .await
        .ok()?;
    if !resp.status().is_success() { return None; }
    let text = resp.text().await.ok()?;
    parse_position_info_response(&text)
}

/// Case-insensitive `<tag>...</tag>` inner-text extractor.  Same
/// shape as the one in play_uri.rs but local to this module to keep
/// it self-contained.
fn extract_inner_ci(xml: &str, tag: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let tag_lower = tag.to_ascii_lowercase();
    let open_needle = format!("<{}", tag_lower);
    let close_needle = format!("</{}>", tag_lower);
    let open_start = lower.find(&open_needle)?;
    let after_open = xml[open_start..].find('>')? + open_start + 1;
    let close_offset = lower[after_open..].find(&close_needle)?;
    let close_start = after_open + close_offset;
    Some(xml[after_open..close_start].trim().to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    const TRANSPORT_PLAYING: &str = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
 <s:Body>
  <u:GetTransportInfoResponse xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <CurrentTransportState>PLAYING</CurrentTransportState>
   <CurrentTransportStatus>OK</CurrentTransportStatus>
   <CurrentSpeed>1</CurrentSpeed>
  </u:GetTransportInfoResponse>
 </s:Body>
</s:Envelope>"#;

    const TRANSPORT_STOPPED: &str = r#"<u:GetTransportInfoResponse>
<CurrentTransportState>STOPPED</CurrentTransportState>
<CurrentTransportStatus>OK</CurrentTransportStatus>
<CurrentSpeed>1</CurrentSpeed>
</u:GetTransportInfoResponse>"#;

    const POSITION_RESPONSE: &str = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
 <s:Body>
  <u:GetPositionInfoResponse xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <Track>1</Track>
   <TrackDuration>00:03:42</TrackDuration>
   <TrackMetaData></TrackMetaData>
   <TrackURI>http://192.168.2.178:8473/t/abc123</TrackURI>
   <RelTime>00:00:17</RelTime>
   <AbsTime>00:00:17</AbsTime>
   <RelCount>1</RelCount>
   <AbsCount>1</AbsCount>
  </u:GetPositionInfoResponse>
 </s:Body>
</s:Envelope>"#;

    #[test]
    fn parse_transport_playing() {
        let info = parse_transport_info_response(TRANSPORT_PLAYING).unwrap();
        assert_eq!(info.state, TransportState::Playing);
        assert_eq!(info.status, "OK");
        assert_eq!(info.speed, "1");
        assert!(info.state.indicates_playing());
    }

    #[test]
    fn parse_transport_stopped() {
        let info = parse_transport_info_response(TRANSPORT_STOPPED).unwrap();
        assert_eq!(info.state, TransportState::Stopped);
        assert!(!info.state.indicates_playing());
    }

    #[test]
    fn parse_transport_missing_state_returns_none() {
        let xml = "<root><CurrentTransportStatus>OK</CurrentTransportStatus></root>";
        assert!(parse_transport_info_response(xml).is_none());
    }

    #[test]
    fn parse_transport_state_handles_all_cases() {
        assert_eq!(parse_transport_state("PLAYING"),         TransportState::Playing);
        assert_eq!(parse_transport_state("playing"),         TransportState::Playing);
        assert_eq!(parse_transport_state("PAUSED_PLAYBACK"), TransportState::PausedPlayback);
        assert_eq!(parse_transport_state("PAUSED"),          TransportState::PausedPlayback);
        assert_eq!(parse_transport_state("STOPPED"),         TransportState::Stopped);
        assert_eq!(parse_transport_state("TRANSITIONING"),   TransportState::Transitioning);
        assert_eq!(parse_transport_state("NO_MEDIA_PRESENT"),TransportState::NoMediaPresent);
        match parse_transport_state("CUSTOM_VENDOR_STATE") {
            TransportState::Other(s) => assert_eq!(s, "CUSTOM_VENDOR_STATE"),
            _ => panic!("expected Other"),
        }
    }

    #[test]
    fn transitioning_counts_as_playing() {
        // Renderers often spend a few hundred ms in TRANSITIONING
        // right after SetAVTransportURI + Play. We treat it as
        // success because it almost always settles to PLAYING.
        assert!(TransportState::Transitioning.indicates_playing());
    }

    #[test]
    fn parse_position_extracts_all_fields() {
        let info = parse_position_info_response(POSITION_RESPONSE).unwrap();
        assert_eq!(info.track, 1);
        assert_eq!(info.track_duration, "00:03:42");
        assert_eq!(info.track_uri, "http://192.168.2.178:8473/t/abc123");
        assert_eq!(info.current_position, "00:00:17");
        assert_eq!(info.abs_time, "00:00:17");
    }

    #[test]
    fn parse_position_only_abstime() {
        // Some firmware skips RelTime; AbsTime alone should still
        // be enough to construct a PositionInfo.
        let xml = "<resp><Track>0</Track><AbsTime>00:01:23</AbsTime></resp>";
        let info = parse_position_info_response(xml).unwrap();
        assert_eq!(info.abs_time, "00:01:23");
        assert_eq!(info.current_position, "");
    }

    #[test]
    fn parse_position_no_time_fields_returns_none() {
        let xml = "<resp><Track>0</Track></resp>";
        assert!(parse_position_info_response(xml).is_none());
    }

    #[test]
    fn parse_upnp_duration_standard() {
        assert_eq!(parse_upnp_duration("00:03:42"), Some(222.0));
        assert_eq!(parse_upnp_duration("01:00:00"), Some(3600.0));
        assert_eq!(parse_upnp_duration("00:00:30.5"), Some(30.5));
    }

    #[test]
    fn parse_upnp_duration_handles_not_implemented() {
        assert_eq!(parse_upnp_duration("NOT_IMPLEMENTED"), None);
        assert_eq!(parse_upnp_duration("not_implemented"), None);
        assert_eq!(parse_upnp_duration(""), None);
    }

    #[test]
    fn parse_upnp_duration_rejects_malformed() {
        assert_eq!(parse_upnp_duration("abc"), None);
        assert_eq!(parse_upnp_duration("00:03"), None);          // missing seconds
        assert_eq!(parse_upnp_duration("00:03:42:99"), None);    // extra component
        assert_eq!(parse_upnp_duration("hh:mm:ss"), None);
    }

    #[test]
    fn parse_upnp_duration__rejects_non_finite_seconds() {
        // Rust's f64::FromStr accepts NaN/inf literals — a malicious
        // or buggy DLNA renderer that sends "00:03:NaN" would otherwise
        // produce a non-finite duration that breaks downstream math.
        assert_eq!(parse_upnp_duration("00:03:NaN"), None);
        assert_eq!(parse_upnp_duration("00:03:inf"), None);
        assert_eq!(parse_upnp_duration("00:03:-inf"), None);
    }

    #[test]
    fn build_envelopes_well_formed() {
        let t = build_get_transport_info_envelope();
        assert!(t.contains("GetTransportInfo"));
        assert!(t.contains("AVTransport:1"));
        assert!(t.contains("<InstanceID>0</InstanceID>"));
        let p = build_get_position_info_envelope();
        assert!(p.contains("GetPositionInfo"));
        assert!(p.contains("AVTransport:1"));
    }

    #[test]
    fn extract_inner_ci_case_insensitive() {
        assert_eq!(extract_inner_ci("<TAG>x</TAG>", "tag"), Some("x".to_string()));
        assert_eq!(extract_inner_ci("<tag>y</tag>", "TAG"), Some("y".to_string()));
    }

    #[test]
    fn transport_state_label() {
        assert_eq!(TransportState::Playing.label(),       "PLAYING");
        assert_eq!(TransportState::PausedPlayback.label(), "PAUSED");
        assert_eq!(TransportState::Stopped.label(),       "STOPPED");
        assert_eq!(TransportState::Transitioning.label(), "TRANSITIONING");
        assert_eq!(TransportState::NoMediaPresent.label(),"NO_MEDIA");
        assert_eq!(TransportState::Other("X".into()).label(), "X");
    }
}

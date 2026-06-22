// src/mozart_source_switch.rs
//
// Attempt to programmatically activate the Network/DLNA source on a
// Mozart-firmware Beosound device so DLNA SOAP playback works.
//
// The motivation: on real hardware (Beosound Stage, Emerge, Level,
// Balance running current Mozart firmware), the device only accepts
// URI playback via the DLNA AVTransport service, and that service
// only gets advertised + responds when the active source is
// "Network" (a.k.a. "DLNA"). When the active source is Spotify
// Connect, AirPlay, Bluetooth, line-in, TV, etc., DLNA stops working
// — the device returns 404 on every play_uri strategy, AND it
// disappears from SSDP M-SEARCH responses.
//
// The user-facing failure mode is: "I pressed play in the controller
// and nothing happened. I have to walk over to the device and push
// the Network button." Solving this means programmatic source
// switching, which Mozart's REST API supports in theory — but the
// exact endpoint is firmware-version dependent and there's no
// authoritative public documentation. We probe several candidates.
//
// ── What we try, in order ─────────────────────────────────────────
//
//   1. GET /BeoZone/Zone/Sources — enumerate available sources, find
//      one whose id classifies as Network/DLNA via mozart_source::
//      classify().
//   2. Once we have a Network source id, try these activation paths
//      in sequence. The first to return a 2xx response wins:
//        a) PUT  /BeoZone/Zone/ActiveSources/primary?id=<id>
//        b) POST /BeoZone/Zone/ActiveSources with JSON body
//           {"primary": "<id>"} or {"primaryExperience": "<id>"}
//        c) POST /BeoZone/Zone/ActiveSources/ (trailing slash variant)
//        d) POST /api/setData?path=BeoSound:/source/active&roles=
//           activate&value=<urlencoded(id)>
//
// Each attempt is logged with method, status, and body snippet so
// the user can see which (if any) the firmware accepts.
//
// ── If nothing works ──────────────────────────────────────────────
//
// We honestly report failure. Some Mozart firmware versions lock
// source switching to physical buttons + the official B&O app
// (which uses an undocumented authenticated API we don't replicate).
// The fallback is the existing user guidance: switch the source on
// the device, then play.

use serde::Serialize;
use std::time::Duration;

/// One activation attempt we made, with its outcome.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchAttempt {
    pub method:   String,
    pub endpoint: String,
    /// HTTP status code, or None if the request didn't get a response.
    pub status:   Option<u16>,
    /// Human-readable note: body snippet, error message, or "ok".
    pub note:     String,
}

/// Full outcome of a switch attempt: which source id we found, what
/// we tried, whether anything worked.
#[derive(Debug, Clone, Serialize)]
pub struct SwitchOutcome {
    pub success: bool,
    /// The endpoint that succeeded (if any).
    pub used_endpoint: Option<String>,
    /// The source id we attempted to activate (Network/DLNA family).
    /// None when we couldn't even enumerate sources or none of the
    /// listed sources classified as Network.
    pub network_source_id: Option<String>,
    /// Full attempt log — surfaced in the UI so the user knows what
    /// happened and we can refine the strategy list over time.
    pub attempts: Vec<SwitchAttempt>,
}

/// Source-list response shape parsing. Mozart's Sources endpoint
/// returns a variety of shapes; we accept any of:
///
///   {"sources": [["id-1", {"sourceType": "..."}], ["id-2", {...}]]}
///   {"sources": [{"id": "id-1", "type": "..."}, ...]}
///   [{"id": "id-1", ...}, ...]
///
/// and extract all id strings recursively.
pub fn extract_source_ids(json_text: &str) -> Vec<String> {
    let v: serde_json::Value = match serde_json::from_str(json_text) {
        Ok(v) => v,
        Err(_) => return Vec::new(),
    };
    let mut out: Vec<String> = Vec::new();
    let mut stack: Vec<&serde_json::Value> = vec![&v];
    while let Some(node) = stack.pop() {
        match node {
            serde_json::Value::Array(arr) => {
                // Old Mozart shape: ["sourceId", {…}] — first element is the id.
                if arr.len() == 2 {
                    if let (Some(serde_json::Value::String(s)),
                            Some(serde_json::Value::Object(_))) =
                        (arr.first(), arr.get(1))
                    {
                        if !s.is_empty() && !out.contains(s) {
                            out.push(s.clone());
                        }
                    }
                }
                for x in arr { stack.push(x); }
            }
            serde_json::Value::Object(obj) => {
                // Standard shape: {"id": "...", ...}
                if let Some(serde_json::Value::String(s)) = obj.get("id") {
                    if !s.is_empty() && !out.contains(s) {
                        out.push(s.clone());
                    }
                }
                for (_, v) in obj { stack.push(v); }
            }
            _ => {}
        }
    }
    out
}

/// From a list of source ids, pick the first one that classifies as
/// Network/DLNA via mozart_source::classify(). Returns None when no
/// such source exists (rare — most Mozart devices have a Network
/// source even when it's not currently active).
pub fn pick_network_source(ids: &[String]) -> Option<String> {
    ids.iter().find(|id| {
        crate::mozart_source::classify(id).accepts_uri_playback()
    }).cloned()
}

/// Drive the full activation attempt against `base_url`. Pure async,
/// no QtState — the controller orchestrates the call from its
/// existing worker thread, just like play_uri.
pub async fn try_activate_network(base_url: &str, client: &reqwest::Client)
    -> SwitchOutcome
{
    let mut attempts: Vec<SwitchAttempt> = Vec::new();

    // ── Step 1: enumerate sources ──────────────────────────────────
    let list_url = format!("{}/BeoZone/Zone/Sources", base_url.trim_end_matches('/'));
    let list_result = client.get(&list_url)
        .timeout(Duration::from_secs(3))
        .send().await;
    let source_ids: Vec<String> = match list_result {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let body = resp.text().await.unwrap_or_default();
            let ids = extract_source_ids(&body);
            attempts.push(SwitchAttempt {
                method:   "GET".into(),
                endpoint: list_url.clone(),
                status:   Some(status),
                note:     if ids.is_empty() {
                    format!("HTTP {} but no source ids parsed: {}",
                            status, body.chars().take(120).collect::<String>())
                } else {
                    format!("found {} source(s): {}", ids.len(), ids.join(", "))
                },
            });
            ids
        }
        Err(e) => {
            attempts.push(SwitchAttempt {
                method:   "GET".into(),
                endpoint: list_url.clone(),
                status:   None,
                note:     format!("request failed: {}", e),
            });
            Vec::new()
        }
    };

    // Pick the network source id from the enumeration.
    let network_id = pick_network_source(&source_ids);

    // If we don't have an id, we still try a few hardcoded ones —
    // some firmware uses canonical "Network" / "dlna" identifiers
    // that aren't in the enumeration but DO accept activation.
    let candidate_ids: Vec<String> = if let Some(ref id) = network_id {
        vec![id.clone()]
    } else {
        // Last-resort guesses. Order: most common → least.
        vec![
            "Network".into(),
            "network".into(),
            "dlna".into(),
            "DLNA".into(),
        ]
    };

    // ── Step 2: try activation strategies ─────────────────────────
    //
    // Strategy registry. Each closure builds a (method, url, body)
    // tuple for the given source id. We try each strategy with each
    // candidate id (cheap — at most ~12 round trips total, each
    // 2s-bounded). First 2xx wins.
    let strategies: &[(&str, fn(&str, &str) -> (String, String, Option<String>))] = &[
        // (a) PUT with query parameter — pattern used by Mozart for
        // many "set value" operations (volume, mute, etc.).
        ("PUT-primary-query", |base, id| (
            "PUT".into(),
            format!("{}/BeoZone/Zone/ActiveSources/primary?id={}",
                    base.trim_end_matches('/'),
                    urlencoding::encode(id)),
            None,
        )),
        // (b) POST JSON to ActiveSources with {"primaryExperience": "..."}
        ("POST-primaryExperience-json", |base, id| (
            "POST".into(),
            format!("{}/BeoZone/Zone/ActiveSources",
                    base.trim_end_matches('/')),
            Some(format!(r#"{{"primaryExperience": "{}"}}"#,
                         id.replace('"', "\\\""))),
        )),
        // (c) POST JSON to ActiveSources with {"primary": "..."}
        ("POST-primary-json", |base, id| (
            "POST".into(),
            format!("{}/BeoZone/Zone/ActiveSources",
                    base.trim_end_matches('/')),
            Some(format!(r#"{{"primary": "{}"}}"#,
                         id.replace('"', "\\\""))),
        )),
        // (d) POST JSON to ActiveSources/ (trailing slash variant)
        ("POST-primary-json-trailing-slash", |base, id| (
            "POST".into(),
            format!("{}/BeoZone/Zone/ActiveSources/",
                    base.trim_end_matches('/')),
            Some(format!(r#"{{"primary": "{}"}}"#,
                         id.replace('"', "\\\""))),
        )),
        // (e) Mozart setData path. Some firmware exposes source
        // activation via the generic setData mechanism rather than
        // BeoZone. The "roles=activate" parameter is the convention
        // for state-changing setData calls.
        ("POST-setData-source-active", |base, id| (
            "POST".into(),
            format!("{}/api/setData?path={}&roles=activate&value={}",
                    base.trim_end_matches('/'),
                    urlencoding::encode("BeoSound:/source/active"),
                    urlencoding::encode(id)),
            None,
        )),
        // (f) PUT to a Sources subresource — another common REST
        // pattern. Less likely to work but cheap to try.
        ("PUT-source-active", |base, id| (
            "PUT".into(),
            format!("{}/BeoZone/Zone/Sources/{}/active",
                    base.trim_end_matches('/'),
                    urlencoding::encode(id)),
            None,
        )),
    ];

    for id in &candidate_ids {
        for (strategy_name, builder) in strategies {
            let (method, url, body) = builder(base_url, id);
            let mut req = match method.as_str() {
                "PUT"  => client.put (&url),
                "POST" => client.post(&url),
                _      => continue,
            };
            req = req.timeout(Duration::from_secs(2));
            if let Some(b) = &body {
                req = req.header("Content-Type", "application/json")
                         .body(b.clone());
            }
            let log_endpoint = format!("[{}] {} {}", strategy_name, method, url);
            let resp = req.send().await;
            match resp {
                Ok(r) => {
                    let status = r.status().as_u16();
                    let body_snip = r.text().await.unwrap_or_default()
                        .chars().take(120).collect::<String>();
                    let ok = (200..300).contains(&status);
                    attempts.push(SwitchAttempt {
                        method:   method.clone(),
                        endpoint: log_endpoint,
                        status:   Some(status),
                        note:     if ok { format!("OK (id={}): {}", id, body_snip) }
                                  else  { format!("(id={}): {}", id, body_snip) },
                    });
                    if ok {
                        // Build the success endpoint string before
                        // moving `url` into the return.
                        let used = format!("[{}] {} {}",
                            strategy_name, method, url);
                        return SwitchOutcome {
                            success: true,
                            used_endpoint: Some(used),
                            network_source_id: Some(id.clone()),
                            attempts,
                        };
                    }
                }
                Err(e) => {
                    attempts.push(SwitchAttempt {
                        method:   method.clone(),
                        endpoint: log_endpoint,
                        status:   None,
                        note:     format!("(id={}): request failed: {}", id, e),
                    });
                }
            }
        }
    }

    SwitchOutcome {
        success: false,
        used_endpoint: None,
        network_source_id: network_id,
        attempts,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extract_source_ids_handles_pair_array_shape() {
        // Old Mozart shape: array of [id, metadata] pairs.
        let json = r#"{"sources": [
            ["radio1.station", {"sourceType": "internetRadio"}],
            ["spotify.endpoint", {"sourceType": "spotify"}],
            ["network", {"sourceType": "music"}]
        ]}"#;
        let ids = extract_source_ids(json);
        assert!(ids.contains(&"radio1.station".to_string()));
        assert!(ids.contains(&"spotify.endpoint".to_string()));
        assert!(ids.contains(&"network".to_string()));
    }

    #[test]
    fn extract_source_ids_handles_object_shape() {
        // Newer shape: array of {id, ...} objects.
        let json = r#"{
            "sources": [
                {"id": "spotify", "name": "Spotify"},
                {"id": "Network", "name": "Network"}
            ]
        }"#;
        let ids = extract_source_ids(json);
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"spotify".to_string()));
        assert!(ids.contains(&"Network".to_string()));
    }

    #[test]
    fn extract_source_ids_handles_nested_shape() {
        // Mixed depth — id can be at any level.
        let json = r#"{"data": {"sources": {"primary": {"id": "linein"}}}}"#;
        let ids = extract_source_ids(json);
        assert_eq!(ids, vec!["linein"]);
    }

    #[test]
    fn extract_source_ids_dedupes() {
        let json = r#"[{"id": "x"}, {"id": "x"}, {"id": "y"}]"#;
        let ids = extract_source_ids(json);
        // Order-independent: both x and y present, no duplicates.
        assert_eq!(ids.len(), 2);
        assert!(ids.contains(&"x".to_string()));
        assert!(ids.contains(&"y".to_string()));
    }

    #[test]
    fn extract_source_ids_returns_empty_for_garbage() {
        assert!(extract_source_ids("not json").is_empty());
        assert!(extract_source_ids("{}").is_empty());
        assert!(extract_source_ids("null").is_empty());
    }

    #[test]
    fn pick_network_source_prefers_dlna_family() {
        let ids = vec![
            "spotify.endpoint".to_string(),
            "linein".to_string(),
            "Network".to_string(),
            "airplay.endpoint".to_string(),
        ];
        // classify("Network") should be NetworkDlna via the lowercased
        // substring match, so pick_network_source returns it.
        let picked = pick_network_source(&ids);
        assert_eq!(picked, Some("Network".to_string()),
            "expected Network to be picked from {:?}", ids);
    }

    #[test]
    fn pick_network_source_returns_none_when_no_dlna() {
        let ids = vec![
            "spotify.endpoint".to_string(),
            "airplay.endpoint".to_string(),
            "bluetooth".to_string(),
        ];
        assert!(pick_network_source(&ids).is_none());
    }

    #[test]
    fn pick_network_source_matches_lowercase_variants() {
        // Mozart firmware varies between "Network", "network", "dlna" etc.
        for id in &["network", "Network", "NETWORK", "dlna", "DLNA"] {
            let v = vec![id.to_string()];
            assert_eq!(pick_network_source(&v), Some(id.to_string()),
                "{} should classify as dlna-capable", id);
        }
    }
}

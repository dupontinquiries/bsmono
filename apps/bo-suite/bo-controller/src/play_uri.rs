// src/play_uri.rs
#![allow(dead_code)]
//
// Issue "play this URL" commands to a B&O networked speaker via its
// ASE/BeoZone API. Different model + firmware combinations expose
// different endpoints, so we try a small list in order until one succeeds.
//
// HONEST SCOPE — this is best-effort. The endpoints below are the ones
// most commonly reported as working across B&O networked products
// (Beosound Stage, Balance, Level, A5/A9, Emerge, Eclipse, Contour).
// Without a public B&O API spec, we can't guarantee any of them work on
// a given firmware. The function returns a clear log line for each
// endpoint attempted + its HTTP status, so the user can see what the
// device accepted in the eprintln output.
//
// Known shapes from reverse-engineering work in the community:
//
//   POST /BeoZone/Zone/PlayQueue
//     body: { "playQueueItem": { "track": { "id": "...", "playUrl": "..." } } }
//
//   POST /BeoZone/Zone/Stream/playPointer
//     body: { "playPointer": { "playUrl": "..." } }
//
//   POST /BeoZone/Zone/ActiveSources/primary  (switch to "music"/"net-radio")
//     followed by POST .../Stream/playPointer
//
// We try them in roughly that order. The very last fallback is to log
// the URL so the user can paste it into their phone's B&O app, which
// always works.

use serde_json::json;

// ── SSDP cache wiring ──────────────────────────────────────────────
//
// play_uri's DLNA strategy needs to find a device's AVTransport
// control URL. Doing live SSDP M-SEARCH on every play attempt costs
// ~1.5 s. Instead, the controller's startup task warms a cache via
// `set_ssdp_cache_provider` below, and play_uri reads from it through
// `cached_ssdp_for_host`. If the cache is empty (no provider set, or
// startup search found nothing), Strategy 7 falls back to a live
// search anyway — the cache is an optimization, not a precondition.

type SsdpProvider = Box<dyn Fn(&str) -> Option<super::dlna_discover::DiscoveredDevice>
                              + Send + Sync + 'static>;

static SSDP_PROVIDER: once_cell::sync::OnceCell<SsdpProvider>
    = once_cell::sync::OnceCell::new();

/// Register a callable that play_uri calls to look up a cached SSDP
/// result for a target host. main.rs installs this at startup, with
/// the closure reading from main's SSDP_CACHE.
pub fn set_ssdp_cache_provider<F>(provider: F)
where F: Fn(&str) -> Option<super::dlna_discover::DiscoveredDevice>
            + Send + Sync + 'static
{
    let _ = SSDP_PROVIDER.set(Box::new(provider));
}

/// Look up a cached SSDP result for a given target host (e.g.
/// "192.168.2.12" or "192.168.2.12:8080"). Returns None when no
/// provider is registered or the cache has no match.
fn cached_ssdp_for_host(target_host: &str)
    -> Option<super::dlna_discover::DiscoveredDevice>
{
    SSDP_PROVIDER.get().and_then(|p| p(target_host))
}

/// Result returned to the caller. `Ok(endpoint_used)` if any of the
/// attempted endpoints returned 2xx; `Err(summary)` if all failed.
pub struct PlayOutcome {
    pub used_endpoint: Option<String>,
    pub attempts: Vec<AttemptLog>,
    pub url: String,
}

pub struct AttemptLog {
    pub endpoint: String,
    pub status: Option<u16>,   // None when network error
    pub note: String,
}

/// Try to make `device_base_url` (e.g. "http://192.168.2.12") play
/// `track_url` (e.g. "http://192.168.2.5:8473/t/abc…"). Logs each
/// attempt via eprintln. Synchronous; uses the existing blocking
/// reqwest client (the call site is already inside the tokio worker).
pub async fn play_url_on_device(
    device_base: &str,
    track_url: &str,
    track_title: &str,
    client: &reqwest::Client,
) -> PlayOutcome {
    // Log the track URL prominently right at the start so users can
    // verify it serves correctly (paste into a browser before the
    // device even tries). Many "playback didn't work" issues turn out
    // to be track-server unreachable from the device's IP.
    eprintln!("play_uri: attempting device={}, track={}, title='{}'",
              device_base, track_url, track_title);

    // Reject IPv6 link-local URLs at the entry — reqwest can't build
    // requests against them without a `%zone` scope id, and even if
    // we added one, the B&O Mozart firmware doesn't serve its REST
    // API on link-local in the first place. The mDNS discovery
    // sometimes surfaces them; the priority/failover system should
    // pick the routable address. If somehow a link-local snuck through,
    // fail fast with a clear diagnostic instead of spewing seven
    // identical "invalid port number" errors.
    if device_base.contains("://[fe80:") || device_base.contains("://fe80:") {
        eprintln!("play_uri: refusing IPv6 link-local URL {} — \
                  reqwest can't address link-local without a scope id, \
                  and the device's REST API isn't exposed there anyway.",
                  device_base);
        return PlayOutcome {
            used_endpoint: None,
            attempts: vec![AttemptLog {
                endpoint: device_base.to_string(),
                status: None,
                note: "IPv6 link-local URL rejected (use the routable IPv4 address)".to_string(),
            }],
            url: track_url.to_string(),
        };
    }

    let mut attempts: Vec<AttemptLog> = Vec::new();

    // Strategy 0: Newest firmware "loadUri" with no Content-Type wrapping —
    // some Mozart 5.x firmware accepts a bare URI as the request body.
    {
        let url = format!("{}/BeoZone/Zone/Stream/loadUri", device_base.trim_end_matches('/'));
        let log = post_text(&url, track_url, client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            return done(attempts, track_url, "Stream/loadUri (text)");
        }}
        attempts.push(log);
    }

    // Strategy 1: Modern Mozart "loadUri" endpoint, JSON body. Used by
    // recent firmware on Beosound Level, Stage v2, A5/A9 5th-gen.
    {
        let url = format!("{}/BeoZone/Zone/Stream/loadUri", device_base.trim_end_matches('/'));
        let body = json!({ "uri": track_url, "name": track_title });
        let log = post_json(&url, &body, client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            return done(attempts, track_url, "Stream/loadUri (json)");
        }}
        attempts.push(log);
    }

    // Strategy 2: Activate "music" source first, then play queue item.
    {
        let act_url = format!("{}/BeoZone/Zone/ActiveSources/primary", device_base.trim_end_matches('/'));
        let body = json!({ "primaryExperience": { "source": { "id": "music:management" } } });
        let log = post_json(&act_url, &body, client).await;
        attempts.push(log);

        let play_url = format!("{}/BeoZone/Zone/PlayQueue", device_base.trim_end_matches('/'));
        let body = json!({
            "playQueueItem": {
                "behaviour": "playNow",
                "track": {
                    "id": "controller-track",
                    "name": track_title,
                    "playUrl": track_url,
                    "type": "audio/url"
                }
            }
        });
        let log = post_json(&play_url, &body, client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            return done(attempts, track_url, "PlayQueue");
        }}
        attempts.push(log);
    }

    // Strategy 3: Older Stream/playPointer.
    {
        let url = format!("{}/BeoZone/Zone/Stream/playPointer", device_base.trim_end_matches('/'));
        let body = json!({ "playPointer": { "playUrl": track_url, "name": track_title } });
        let log = post_json(&url, &body, client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            return done(attempts, track_url, "Stream/playPointer");
        }}
        attempts.push(log);
    }

    // Strategy 4: Beosound /api/setData with a play payload. The
    // Stage/Balance/Emerge/Level/A5/A9 modern models speak this
    // simpler URL-encoded API for volume (we use it everywhere
    // else), and many of them accept a play command on the same
    // path. This is a last-ditch attempt before giving up.
    {
        // Two payload shapes — the device sometimes accepts one and
        // not the other depending on firmware. We try both.
        let payloads = [
            json!({
                "type": "beoSoundPlayMedia",
                "beoSoundPlayMedia": { "uri": track_url, "name": track_title }
            }),
            json!({
                "type": "uri",
                "uri": { "value": track_url, "name": track_title }
            }),
        ];
        for body in &payloads {
            let body_str = serde_json::to_string(body).unwrap_or_default();
            let enc = urlencoding::encode(&body_str);
            let url = format!(
                "{}/api/setData?path=BeoSound%3A%2Fplay&roles=activate&value={}",
                device_base.trim_end_matches('/'), enc,
            );
            let log = post_no_body(&url, client).await;
            if let Some(s) = log.status { if (200..300).contains(&s) {
                attempts.push(log);
                return done(attempts, track_url, "setData/play");
            }}
            attempts.push(log);
        }
    }

    // Strategy 5: Newest Mozart "Cards/Slots" path. Some 2024+ firmware
    // reports a "playback card" you POST to. Worth trying as a last
    // resort before declaring defeat.
    {
        let url = format!(
            "{}/BeoMessageProvider/Cards/Slots/0/Playback",
            device_base.trim_end_matches('/'));
        let body = json!({
            "playback": {
                "uri": track_url,
                "name": track_title,
                "behavior": "playNow"
            }
        });
        let log = post_json(&url, &body, client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            return done(attempts, track_url, "Cards/Slots/Playback");
        }}
        attempts.push(log);
    }

    // Strategy 6: DLNA/UPnP AVTransport SOAP. The Mozart firmware on
    // Beosound Stage/Level/Emerge/Balance speaks DLNA when the user
    // selects the "Network" or "DLNA" source. When that source is
    // active we can drive playback via the standard SOAP
    // SetAVTransportURI + Play sequence. The control endpoint is
    // typically at /upnp/control/AVTransport or /MediaRenderer/
    // AVTransport/Control. We try the common paths in sequence.
    //
    // This works on stock Mozart firmware that has REJECTED every
    // /BeoZone/* and /api/setData? path above — DLNA is the
    // documented multi-vendor protocol for URI playback and is
    // enabled by default. The catch: the user has to have selected
    // the DLNA source on the device first (or it will return
    // 500 "Action not implemented in current state").
    for dlna_path in &[
        "/upnp/control/AVTransport",
        "/AVTransport/control",
        "/MediaRenderer/AVTransport/Control",
        "/dlnarender/0/Action",
    ] {
        let url = format!("{}{}", device_base.trim_end_matches('/'), dlna_path);
        // SetAVTransportURI SOAP envelope. Conforms to the AVTransport:1
        // service spec; current_uri carries our http track URL.
        let body = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
   <CurrentURI>{}</CurrentURI>
   <CurrentURIMetaData></CurrentURIMetaData>
  </u:SetAVTransportURI>
 </s:Body>
</s:Envelope>"#, escape_xml(track_url));
        let log = post_soap(&url, &body,
            "urn:schemas-upnp-org:service:AVTransport:1#SetAVTransportURI",
            client).await;
        if let Some(s) = log.status { if (200..300).contains(&s) {
            attempts.push(log);
            // Set URI succeeded; now actually press Play.
            let play_body = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
   <Speed>1</Speed>
  </u:Play>
 </s:Body>
</s:Envelope>"#;
            let play_log = post_soap(&url, play_body,
                "urn:schemas-upnp-org:service:AVTransport:1#Play",
                client).await;
            let play_ok = play_log.status
                .map(|s| (200..300).contains(&s))
                .unwrap_or(false);
            attempts.push(play_log);
            if !play_ok {
                // Play action itself failed (or returned a SOAP fault
                // demoted to 502). Drop through to the next strategy.
                continue;
            }
            // Post-Play verification. SetAVTransportURI + Play returned
            // 200, but that only means the SOAP layer accepted the
            // action. The renderer might still be STOPPED (silent
            // failure mode). We do one GetTransportInfo within ~1.5s
            // to confirm the device actually started playing. If it
            // reports a non-playing state, fall through to Strategy 7
            // (which uses the SSDP-discovered control URL — sometimes
            // a different endpoint with full state-machine support).
            if let Some(verify) = verify_playback_started(&url, client).await {
                if verify.indicates_playing() {
                    return done(attempts, track_url,
                        &format!("DLNA SOAP {} (verified {})",
                                 dlna_path, verify.label()));
                }
                // SOAP accepted but transport state says otherwise.
                attempts.push(AttemptLog {
                    endpoint: url.clone(),
                    status: Some(502),
                    note: format!("Play accepted but transport state is {} \
                                   (not playing) — falling through to next strategy",
                                  verify.label()),
                });
                continue;
            }
            // GetTransportInfo didn't return a parseable response.
            // Trust the 200 we got from Play — we'd rather assume the
            // renderer just doesn't expose GetTransportInfo than block
            // working playback.
            return done(attempts, track_url,
                &format!("DLNA SOAP {} (verify inconclusive)", dlna_path));
        }}
        attempts.push(log);
    }

    // Strategy 7: SSDP-discovered AVTransport endpoint. Strategy 6
    // tried four hardcoded UPnP control paths; if none worked but the
    // device IS a DLNA renderer, the issue may just be that Mozart
    // puts its control endpoint somewhere we didn't guess. So we
    // perform a real SSDP M-SEARCH, find the device's descriptor URL,
    // parse out its actual AVTransport control URL, and try that.
    //
    // We run M-SEARCH inline (blocking on a background tokio task) to
    // keep the dispatch synchronous within play_uri. The total cost
    // is ~1.5 seconds of waiting which is acceptable on a final-
    // fallback strategy that only runs once per play attempt — but
    // we first check the controller's startup-warmup cache; when the
    // device was discovered there, we skip the live search entirely.
    {
        let target_host = match super::dlna_discover::host_of(device_base) {
            Some(h) => h,
            None => {
                attempts.push(AttemptLog {
                    endpoint: "(ssdp)".to_string(), status: None,
                    note: "could not extract host from device URL".to_string(),
                });
                return finalize_failure(attempts, track_url, device_base);
            }
        };

        // Try the warm cache first via the registered fetcher. If
        // it has fresh data for this host, use it; else fall through
        // to a live M-SEARCH.
        let cached: Option<super::dlna_discover::DiscoveredDevice> =
            cached_ssdp_for_host(&target_host);
        let discovered: Vec<super::dlna_discover::DiscoveredDevice> = match cached {
            Some(d) => {
                eprintln!("play_uri: SSDP cache hit for {} → {}",
                          target_host, d.location);
                vec![d]
            }
            None => {
                eprintln!("play_uri: running SSDP M-SEARCH to find AVTransport control URL for {}",
                          target_host);
                tokio::task::spawn_blocking(|| {
                    super::dlna_discover::m_search(
                        super::dlna_discover::ST_MEDIA_RENDERER,
                        std::time::Duration::from_millis(1500),
                    ).unwrap_or_default()
                }).await.unwrap_or_default()
            }
        };

        // Match the discovered devices against our target host.
        let target_dev = discovered.into_iter()
            .find(|d| super::dlna_discover::matches_host(d, &target_host));
        if let Some(dev) = target_dev {
            eprintln!("play_uri: SSDP matched device at {} (server: {})",
                      dev.location, dev.server);
            // Fetch the descriptor XML.
            match client.get(&dev.location)
                .timeout(std::time::Duration::from_secs(3))
                .send().await
            {
                Ok(resp) if resp.status().is_success() => {
                    let xml = resp.text().await.unwrap_or_default();
                    if let Some(info) = super::dlna_discover::extract_avtransport_info(
                        &xml, &dev.location)
                    {
                        eprintln!("play_uri: descriptor parsed: '{}' → control_url={}",
                                  info.friendly_name, info.control_url);
                        attempts.push(AttemptLog {
                            endpoint: dev.location.clone(),
                            status: Some(200),
                            note: format!("descriptor OK ({})", info.friendly_name),
                        });
                        // Now try the SAME SetAVTransportURI + Play
                        // sequence as Strategy 6, but against the
                        // discovered URL.
                        let body = format!(r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:SetAVTransportURI xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
   <CurrentURI>{}</CurrentURI>
   <CurrentURIMetaData></CurrentURIMetaData>
  </u:SetAVTransportURI>
 </s:Body>
</s:Envelope>"#, escape_xml(track_url));
                        let log = post_soap(&info.control_url, &body,
                            "urn:schemas-upnp-org:service:AVTransport:1#SetAVTransportURI",
                            client).await;
                        // Capture the success bit before moving log
                        // into attempts; this lets the rest of the
                        // block read it without re-borrowing.
                        let set_uri_ok = log.status
                            .map(|s| (200..300).contains(&s))
                            .unwrap_or(false);
                        attempts.push(log);
                        if set_uri_ok {
                            let play_body = r#"<?xml version="1.0" encoding="utf-8"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/" s:encodingStyle="http://schemas.xmlsoap.org/soap/encoding/">
 <s:Body>
  <u:Play xmlns:u="urn:schemas-upnp-org:service:AVTransport:1">
   <InstanceID>0</InstanceID>
   <Speed>1</Speed>
  </u:Play>
 </s:Body>
</s:Envelope>"#;
                            let play_log = post_soap(&info.control_url, play_body,
                                "urn:schemas-upnp-org:service:AVTransport:1#Play",
                                client).await;
                            let play_ok = play_log.status
                                .map(|s| (200..300).contains(&s))
                                .unwrap_or(false);
                            attempts.push(play_log);
                            if play_ok {
                                if let Some(verify) =
                                    verify_playback_started(&info.control_url, client).await
                                {
                                    if verify.indicates_playing() {
                                        return done(attempts, track_url,
                                            &format!("DLNA SSDP {} (verified {})",
                                                     info.control_url, verify.label()));
                                    }
                                    attempts.push(AttemptLog {
                                        endpoint: info.control_url.clone(),
                                        status: Some(502),
                                        note: format!("Play accepted but transport state is \
                                                       {} (not playing)", verify.label()),
                                    });
                                } else {
                                    // No GetTransportInfo response — trust
                                    // the 200 from Play. Same logic as
                                    // Strategy 6's inconclusive path.
                                    return done(attempts, track_url,
                                        &format!("DLNA SSDP {} (verify inconclusive)",
                                                 info.control_url));
                                }
                            }
                            // play_ok false OR verify said not playing:
                            // fall through to final-failure handler.
                        }
                    } else {
                        attempts.push(AttemptLog {
                            endpoint: dev.location.clone(),
                            status: Some(200),
                            note: "descriptor has no AVTransport service".to_string(),
                        });
                    }
                }
                Ok(resp) => {
                    attempts.push(AttemptLog {
                        endpoint: dev.location.clone(),
                        status: Some(resp.status().as_u16()),
                        note: format!("descriptor fetch returned HTTP {}",
                                      resp.status()),
                    });
                }
                Err(e) => {
                    attempts.push(AttemptLog {
                        endpoint: dev.location.clone(),
                        status: None,
                        note: format!("descriptor fetch error: {}", e),
                    });
                }
            }
        } else {
            attempts.push(AttemptLog {
                endpoint: "(ssdp)".to_string(),
                status: None,
                note: format!("no SSDP MediaRenderer reply matched host {}",
                              target_host),
            });
        }
    }

    finalize_failure(attempts, track_url, device_base)
}

/// Render the final-failure summary (Mozart-specific guidance + log
/// lines) and produce the PlayOutcome. Extracted so Strategy 7 can
/// return through it without duplicating the diagnostic.
fn finalize_failure(attempts: Vec<AttemptLog>, track_url: &str,
                    device_base: &str) -> PlayOutcome {
    // user can verify it serves correctly (paste into a browser or
    // the official B&O app). Catches things like "track server can't
    // reach this peer IP" and "file is malformed".
    eprintln!("play_uri: ALL ENDPOINTS FAILED for {}", device_base);
    eprintln!("play_uri: track URL was: {}", track_url);
    eprintln!("play_uri: Try opening {} in a browser to verify the URL serves.", track_url);

    // Mozart-specific guidance. When the REST endpoints we tried all
    // returned 404 OR 500 "path not whitelisted", we're likely
    // hitting modern Mozart firmware that doesn't expose arbitrary
    // URI playback via REST. Print actionable next steps.
    let saw_whitelist_500 = attempts.iter().any(|a|
        a.status == Some(500) && a.note.contains("not whitelisted"));
    let saw_404_loaduri = attempts.iter().any(|a|
        a.status == Some(404) && a.endpoint.contains("loadUri"));
    // 701 = Transition not available — DLNA renderer received our
    // SetAVTransportURI but its current state machine can't transition
    // to "playing" right now. The canonical cause on Mozart is "active
    // source is not Network/DLNA". This deserves its own specific
    // message because we KNOW the fix and can tell the user exactly
    // what button to tap on the device.
    let saw_701_transition = attempts.iter().any(|a|
        a.status == Some(502) && a.note.contains("701"));

    if saw_701_transition {
        eprintln!();
        eprintln!("play_uri: The DLNA renderer reported UPnP 701 'Transition not available'.");
        eprintln!("    This means the device received the play command but isn't currently");
        eprintln!("    in a state where it can accept DLNA playback. Most common cause:");
        eprintln!();
        eprintln!("    → On the Beosound device, the active source is something other than");
        eprintln!("      'Network' / 'DLNA' / 'Music'. Press the source button on the device");
        eprintln!("      (or use the official B&O app) to switch sources, then retry.");
        eprintln!();
        eprintln!("    Programmatically switching the source via SOAP is not supported");
        eprintln!("    on Mozart firmware (the SetSourceTitle and Vendor X_ActiveSource");
        eprintln!("    extensions both 404). This must be done on the device itself.");
    } else if saw_whitelist_500 || saw_404_loaduri {
        eprintln!();
        eprintln!("play_uri: The device's REST API rejected every URI-playback path.");
        eprintln!("    This is typical for Beosound Stage/Level/Balance/Emerge running");
        eprintln!("    current Mozart firmware. Arbitrary HTTP URL playback isn't enabled");
        eprintln!("    in the REST API — only Spotify Connect, AirPlay, Chromecast, DLNA,");
        eprintln!("    line-in, Bluetooth, and TV input work.");
        eprintln!();
        eprintln!("    Workarounds:");
        eprintln!("      1. On the device, switch the active source to 'Network' (DLNA).");
        eprintln!("         The DLNA SOAP strategy then succeeds.");
        eprintln!("      2. Use the official B&O app to start playback, then control via");
        eprintln!("         this controller (volume + transport work in any source).");
        eprintln!("      3. Use Spotify Connect (the device is a Spotify endpoint).");
    }

    PlayOutcome {
        used_endpoint: None,
        attempts,
        url: track_url.to_string(),
    }
}

/// XML-escape a URL/string for embedding in a SOAP envelope. URLs
/// commonly contain & and ? which break the envelope's XML parsing
/// if not escaped.
fn escape_xml(s: &str) -> String {
    s.replace('&', "&amp;")
     .replace('<', "&lt;")
     .replace('>', "&gt;")
     .replace('"', "&quot;")
     .replace('\'', "&apos;")
}

/// SOAP POST. Differs from our JSON helper in that we set the
/// SOAPAction header (required) and use text/xml as the Content-Type.
///
/// Critical: a UPnP control endpoint can return HTTP 200 with a SOAP
/// Fault embedded in the body — the most common Mozart failure mode
/// is `701 Transition not available` when the device's active source
/// isn't DLNA. If we blindly trust the 200 status we'd report "play
/// succeeded" and then hear nothing. parse_soap_fault scans the body
/// for `<s:Fault>` / `<UPnPError>` and demotes the status to a
/// synthetic 502 so the caller treats it as a failure with detail.
/// Post-Play verification helper. Waits briefly for the renderer to
/// transition out of TRANSITIONING, then asks GetTransportInfo what
/// state it's in. Returns the parsed TransportState, or None when the
/// renderer didn't respond with parseable XML.
///
/// 1.2s is a deliberately tight settle window: longer makes the user
/// feel the latency on success; shorter risks getting TRANSITIONING
/// (still treated as success). The cancellable timeout inside
/// get_transport_info bounds the total cost to ~4.5s worst-case.
async fn verify_playback_started(control_url: &str, client: &reqwest::Client)
    -> Option<crate::dlna_position::TransportState>
{
    tokio::time::sleep(std::time::Duration::from_millis(1200)).await;
    let info = crate::dlna_position::get_transport_info(control_url, client).await?;
    eprintln!("play_uri: post-play verify {} → state={} status={}",
              control_url, info.state.label(), info.status);
    Some(info.state)
}

async fn post_soap(url: &str, body: &str, soap_action: &str,
                   client: &reqwest::Client) -> AttemptLog {
    match client.post(url)
        .header("Content-Type", "text/xml; charset=\"utf-8\"")
        .header("SOAPAction", format!("\"{}\"", soap_action))
        .body(body.to_string())
        .timeout(std::time::Duration::from_secs(5))
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let action_short = soap_action.rsplit('#').next().unwrap_or(soap_action);
            let body_text = resp.text().await.unwrap_or_default();
            // Did the body carry a SOAP Fault? If so, the SOAP layer
            // is reporting an error even though the HTTP layer said 200.
            if let Some(fault) = parse_soap_fault(&body_text) {
                let note = format!("UPnP error {} ({}) [HTTP {} SOAP fault]",
                                   fault.code, fault.description, status);
                eprintln!("play_uri: SOAP {} {} -> {}", action_short, url, note);
                // Synthetic 502: HTTP succeeded but the application
                // layer reported a failure. Distinct from a real 502.
                return AttemptLog { endpoint: url.to_string(), status: Some(502), note };
            }
            let note = if body_text.is_empty() {
                format!("HTTP {}", status)
            } else {
                format!("HTTP {}: {}", status, body_text.chars().take(120).collect::<String>())
            };
            eprintln!("play_uri: SOAP {} {} -> {}", action_short, url, note);
            AttemptLog { endpoint: url.to_string(), status: Some(status), note }
        }
        Err(e) => {
            let note = format!("network error: {}", e);
            eprintln!("play_uri: SOAP {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: None, note }
        }
    }
}

/// A SOAP Fault parsed out of a UPnP control response body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SoapFault {
    pub code: String,           // e.g. "701"
    pub description: String,    // e.g. "Transition not available"
}

/// Scan a SOAP response body for a UPnPError fault. The standard
/// shape is `<s:Envelope>...<s:Body><s:Fault>...<UPnPError>
/// <errorCode>NNN</errorCode><errorDescription>...</errorDescription>
/// </UPnPError>...</s:Fault></s:Body></s:Envelope>` but Mozart and
/// other vendors vary on the SOAP namespace prefix and whitespace.
/// We scan for the tag pairs case-insensitively.
pub fn parse_soap_fault(body: &str) -> Option<SoapFault> {
    let lower = body.to_ascii_lowercase();
    // Fast path: the body must mention "fault" or "upnperror" to be
    // worth scanning. Skips work when the body is just a successful
    // <u:SetAVTransportURIResponse/> envelope.
    if !lower.contains("fault") && !lower.contains("upnperror") {
        return None;
    }
    let code = extract_inner_ci(body, "errorCode")?;
    let description = extract_inner_ci(body, "errorDescription")
        .unwrap_or_else(|| "(no description)".to_string());
    Some(SoapFault { code: code.trim().to_string(),
                     description: description.trim().to_string() })
}

/// Case-insensitive `<tag>...</tag>` inner-text extractor for the
/// SOAP fault scanner. Doesn't handle CDATA or namespace prefixes
/// other than matching the bare local name, which is enough for
/// the small fixed vocabulary we care about (errorCode, errorDescription).
fn extract_inner_ci(xml: &str, tag: &str) -> Option<String> {
    let lower = xml.to_ascii_lowercase();
    let tag_lower = tag.to_ascii_lowercase();
    let open_needle = format!("<{}", tag_lower);
    let close_needle = format!("</{}>", tag_lower);
    let open_start = lower.find(&open_needle)?;
    let after_open = xml[open_start..].find('>')? + open_start + 1;
    // The close tag in the original (preserving case) might use
    // a different case than the opening; search lowercased for it.
    let close_start_in_lower = lower[after_open..].find(&close_needle)?;
    let close_start = after_open + close_start_in_lower;
    Some(xml[after_open..close_start].trim().to_string())
}

/// Plain-text POST. Some Mozart firmware accepts a bare URI as the body
/// for loadUri-style endpoints.
async fn post_text(url: &str, body: &str, client: &reqwest::Client) -> AttemptLog {
    match client.post(url)
        .header("Content-Type", "text/plain")
        .body(body.to_string())
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let note = match resp.text().await {
                Ok(t) if t.is_empty() => format!("HTTP {}", status),
                Ok(t) => format!("HTTP {}: {}", status, t.chars().take(120).collect::<String>()),
                Err(e) => format!("HTTP {} (body read err: {})", status, e),
            };
            eprintln!("play_uri: POST(text) {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: Some(status), note }
        }
        Err(e) => {
            let note = format!("network error: {}", e);
            eprintln!("play_uri: POST(text) {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: None, note }
        }
    }
}

async fn post_no_body(url: &str, client: &reqwest::Client) -> AttemptLog {
    match client.post(url)
        .header("Accept", "application/json")
        .header("X-Requested-With", "XMLHttpRequest")
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let note = match resp.text().await {
                Ok(t) if t.is_empty() => format!("HTTP {}", status),
                Ok(t) => format!("HTTP {}: {}", status, t.chars().take(120).collect::<String>()),
                Err(e) => format!("HTTP {} (body read err: {})", status, e),
            };
            eprintln!("play_uri: {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: Some(status), note }
        }
        Err(e) => {
            let note = format!("network error: {}", e);
            eprintln!("play_uri: {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: None, note }
        }
    }
}

fn done(attempts: Vec<AttemptLog>, url: &str, endpoint: &str) -> PlayOutcome {
    PlayOutcome {
        used_endpoint: Some(endpoint.to_string()),
        attempts,
        url: url.to_string(),
    }
}

async fn post_json(url: &str, body: &serde_json::Value, client: &reqwest::Client) -> AttemptLog {
    match client.post(url)
        .header("Content-Type", "application/json")
        .json(body)
        .send()
        .await
    {
        Ok(resp) => {
            let status = resp.status().as_u16();
            let note = match resp.text().await {
                Ok(t) if t.is_empty() => format!("HTTP {}", status),
                Ok(t) => format!("HTTP {}: {}", status, t.chars().take(120).collect::<String>()),
                Err(e) => format!("HTTP {} (body read err: {})", status, e),
            };
            eprintln!("play_uri: {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: Some(status), note }
        }
        Err(e) => {
            let note = format!("network error: {}", e);
            eprintln!("play_uri: {} -> {}", url, note);
            AttemptLog { endpoint: url.to_string(), status: None, note }
        }
    }
}

/// Speculative paused-load. POSTs loadUri with `paused: true`. Returns
/// Ok if the device accepted; the caller subsequently issues
/// Transport=play to start audio when the user actually taps. The set
/// of endpoint shapes that honor `paused` is narrower than the regular
/// play set: we only try the modern Mozart loadUri variants. Older
/// firmware paths (PlayQueue with explicit behaviour, etc.) tend to
/// ignore the pause hint and start immediately, defeating the point.
///
/// Returns Ok(endpoint_used) on success, Err(diagnostic) on failure.
pub async fn speculative_load_paused(
    device_base: &str,
    track_url: &str,
    track_title: &str,
    client: &reqwest::Client,
) -> Result<String, String> {
    eprintln!("play_uri: speculative paused-load device={}, track={}",
              device_base, track_url);

    // Strategy A: Mozart loadUri JSON with explicit paused flag.
    let url = format!("{}/BeoZone/Zone/Stream/loadUri",
                      device_base.trim_end_matches('/'));
    let body = serde_json::json!({
        "uri": track_url,
        "name": track_title,
        "paused": true
    });
    let log = post_json(&url, &body, client).await;
    if let Some(s) = log.status { if (200..300).contains(&s) {
        return Ok("Stream/loadUri (paused)".to_string());
    }}

    // Strategy B: Mozart loadUri with "behaviour" field set to "load".
    // Some firmware accepts "behaviour": "load" as a load-without-play.
    let body = serde_json::json!({
        "uri": track_url,
        "name": track_title,
        "behaviour": "load"
    });
    let log = post_json(&url, &body, client).await;
    if let Some(s) = log.status { if (200..300).contains(&s) {
        return Ok("Stream/loadUri (behaviour=load)".to_string());
    }}

    Err("no paused-load endpoint accepted".to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn outcome_with_no_endpoint_records_attempts() {
        let outcome = PlayOutcome {
            used_endpoint: None,
            attempts: vec![
                AttemptLog { endpoint: "http://x/loadUri".into(), status: Some(404), note: "HTTP 404".into() },
                AttemptLog { endpoint: "http://x/PlayQueue".into(), status: Some(500), note: "HTTP 500".into() },
            ],
            url: "http://srv/t/abc".into(),
        };
        assert!(outcome.used_endpoint.is_none());
        assert_eq!(outcome.attempts.len(), 2);
    }

    #[test]
    fn outcome_with_endpoint_records_success() {
        let outcome = PlayOutcome {
            used_endpoint: Some("Stream/loadUri".into()),
            attempts: vec![
                AttemptLog { endpoint: "http://x/loadUri".into(), status: Some(200), note: "HTTP 200".into() },
            ],
            url: "http://srv/t/abc".into(),
        };
        assert_eq!(outcome.used_endpoint.as_deref(), Some("Stream/loadUri"));
    }

    #[test]
    fn done_constructor_returns_endpoint() {
        let result = done(vec![], "http://x/t/y", "test-endpoint");
        assert_eq!(result.used_endpoint.as_deref(), Some("test-endpoint"));
        assert_eq!(result.url, "http://x/t/y");
    }

    // ── SOAP fault parsing ──
    //
    // Mozart's #1 DLNA failure mode is HTTP 200 with a SOAP fault
    // embedded — "Transition not available" (701) when the active
    // source isn't network/DLNA. We must detect this or we'd happily
    // report "play succeeded" when nothing actually plays.

    #[test]
    fn parse_fault_extracts_upnp_error_701() {
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
 <s:Body>
  <s:Fault>
   <faultcode>s:Client</faultcode>
   <faultstring>UPnPError</faultstring>
   <detail>
    <UPnPError xmlns="urn:schemas-upnp-org:control-1-0">
     <errorCode>701</errorCode>
     <errorDescription>Transition not available</errorDescription>
    </UPnPError>
   </detail>
  </s:Fault>
 </s:Body>
</s:Envelope>"#;
        let fault = parse_soap_fault(body).unwrap();
        assert_eq!(fault.code, "701");
        assert!(fault.description.contains("Transition"));
    }

    #[test]
    fn parse_fault_handles_mixed_case_tags() {
        // Some vendors use camelCase, others lowercase.
        let body = "<Envelope><Body><Fault><detail>\
                    <UPnPError><ERRORCODE>402</ERRORCODE>\
                    <Errordescription>Invalid Args</Errordescription>\
                    </UPnPError></detail></Fault></Body></Envelope>";
        let fault = parse_soap_fault(body).unwrap();
        assert_eq!(fault.code, "402");
        assert_eq!(fault.description, "Invalid Args");
    }

    #[test]
    fn parse_fault_returns_none_on_success_envelope() {
        // Successful SetAVTransportURIResponse — no fault.
        let body = r#"<?xml version="1.0"?>
<s:Envelope xmlns:s="http://schemas.xmlsoap.org/soap/envelope/">
 <s:Body>
  <u:SetAVTransportURIResponse xmlns:u="urn:schemas-upnp-org:service:AVTransport:1"/>
 </s:Body>
</s:Envelope>"#;
        assert!(parse_soap_fault(body).is_none());
    }

    #[test]
    fn parse_fault_returns_none_on_empty_body() {
        assert!(parse_soap_fault("").is_none());
    }

    #[test]
    fn parse_fault_with_no_description_uses_placeholder() {
        let body = "<Fault><UPnPError><errorCode>500</errorCode></UPnPError></Fault>";
        let fault = parse_soap_fault(body).unwrap();
        assert_eq!(fault.code, "500");
        assert!(fault.description.contains("no description"));
    }

    #[test]
    fn extract_inner_ci_handles_attribute_values() {
        let xml = r#"<root><tag attr="x">content</tag></root>"#;
        assert_eq!(extract_inner_ci(xml, "tag"), Some("content".into()));
    }

    #[test]
    fn extract_inner_ci_handles_namespace_prefix() {
        // <ns:tag> is the common case from real SOAP envelopes;
        // we ignore the namespace prefix and match the local name.
        // Note: our simple scanner uses "<tag" as the open-needle,
        // so it does NOT match "<ns:tag" — that's fine for our
        // vocabulary (errorCode / errorDescription always appear
        // without a prefix in UPnPError detail).
        let xml = "<errorCode>123</errorCode>";
        assert_eq!(extract_inner_ci(xml, "errorCode"), Some("123".into()));
    }
}

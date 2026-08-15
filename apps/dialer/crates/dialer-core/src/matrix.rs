//! Matrix backend (feature `matrix`) — a third transport on the same
//! [`Link`] protocol, so a Matrix call rings the identical in-call UI.
//!
//! ## Scope, stated plainly
//!
//! This is **call signalling only**: it signs in, syncs, surfaces incoming
//! `m.call.invite`s as ringing calls, and can decline them with
//! `m.call.hangup`. There is **no media engine** — answering would require a
//! WebRTC stack (ICE/DTLS/SRTP + audio), which is its own project. So v1:
//! incoming Matrix calls ring and can be declined (which stops them ringing
//! on your other devices too); accepting and placing calls are out until a
//! media layer exists. The seam is ready for it: media lands behind
//! `Command::Accept`, the protocol does not change.
//!
//! ## Shape
//!
//! matrix-rust-sdk was rejected for v1 — it drags in tokio and hundreds of
//! crates for what is here four HTTP endpoints. Instead: `ureq` (blocking)
//! and hand-rolled JSON via `serde_json`, in the house visible-loop style.
//! Everything that parses server JSON is a pure function over `&str`
//! (`parse_well_known`, `parse_login`, `parse_sync`) and unit-tested
//! offline; the HTTP calls are one-liners around them.
//!
//! The loop long-polls `/sync` with a 5 s server timeout, draining commands
//! between polls — so a decline can lag up to ~5 s. Deliberate v1 trade
//! (one thread, no channel merging); if that lag ever matters, split sync
//! into a feeder thread merged over an internal enum channel.

use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use crate::link::Event;
use crate::message::{self, Payload};
use crate::types::{CallDirection, CallInfo, CallState};

#[cfg(feature = "matrix")]
use std::collections::HashMap;
#[cfg(feature = "matrix")]
use std::sync::mpsc::{channel, TryRecvError};
#[cfg(feature = "matrix")]
use std::time::Duration;
#[cfg(feature = "matrix")]
use crate::link::{Command, Link};

/// Saved session. Lives next to the call history; contains the access token,
/// so the file is written 0600-equivalent by default umask — same standing
/// as NetworkManager keeping APN passwords.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Creds {
    /// Client-API base URL, e.g. "https://matrix-client.matrix.org".
    pub homeserver: String,
    pub user_id: String,
    pub access_token: String,
    pub device_id: String,
}

impl Creds {
    pub fn path() -> PathBuf {
        crate::history::HistoryStore::default_path()
            .parent()
            .map(|d| d.join("matrix.json"))
            .unwrap_or_else(|| PathBuf::from("matrix.json"))
    }

    pub fn load() -> Option<Creds> {
        let text = std::fs::read_to_string(Self::path()).ok()?;
        serde_json::from_str(&text).ok()
    }

    pub fn save(&self) -> Result<(), String> {
        let path = Self::path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
        }
        std::fs::write(&path, serde_json::to_string_pretty(self).unwrap())
            .map_err(|e| format!("save matrix session: {e}"))
    }

    pub fn forget() {
        let _ = std::fs::remove_file(Self::path());
    }
}

// ---------------------------------------------------------------- pure JSON

/// `.well-known/matrix/client` → the client-API base URL, if the server
/// publishes one.
pub fn parse_well_known(json: &str) -> Option<String> {
    let v: serde_json::Value = serde_json::from_str(json).ok()?;
    let url = v.get("m.homeserver")?.get("base_url")?.as_str()?;
    Some(url.trim_end_matches('/').to_string())
}

/// `/login` response → Creds (homeserver filled in by the caller).
pub fn parse_login(json: &str, homeserver: &str) -> Result<Creds, String> {
    let v: serde_json::Value =
        serde_json::from_str(json).map_err(|e| format!("login response: {e}"))?;
    if let Some(err) = v.get("error").and_then(|e| e.as_str()) {
        return Err(err.to_string());
    }
    let s = |k: &str| {
        v.get(k)
            .and_then(|x| x.as_str())
            .map(str::to_string)
            .ok_or_else(|| format!("login response missing {k}"))
    };
    Ok(Creds {
        homeserver: homeserver.to_string(),
        user_id: s("user_id")?,
        access_token: s("access_token")?,
        device_id: s("device_id").unwrap_or_default(),
    })
}

/// The custom Matrix event type carrying a structured handoff payload. A
/// client without this app still gets the accompanying `m.room.message`
/// fallback (we send both).
pub const PAYLOAD_EVENT_TYPE: &str = "org.ecosystem.dialer.payload";

/// One live invite we may need to hang up on.
#[derive(Debug, Clone, PartialEq)]
pub struct LiveInvite {
    pub room_id: String,
    pub call_id: String,
    /// Echoed back in the hangup: the spec allows integer 0 or string "1".
    pub version: serde_json::Value,
}

/// Our call ids: "matrix/{room_id}/{call_id}". The shell routes on the
/// prefix; both components are needed to send the hangup.
pub fn call_link_id(room_id: &str, call_id: &str) -> String {
    format!("matrix/{room_id}/{call_id}")
}

/// Parse one `/sync` response: next_batch plus the call events we care
/// about. `my_user` filters out our own echoes (our other device declining,
/// our own invites).
pub fn parse_sync(json: &str, my_user: &str) -> Result<(String, Vec<Event>, Vec<LiveInvite>), String> {
    let v: serde_json::Value = serde_json::from_str(json).map_err(|e| format!("sync: {e}"))?;
    let next = v
        .get("next_batch")
        .and_then(|b| b.as_str())
        .ok_or("sync missing next_batch")?
        .to_string();
    let mut events = Vec::new();
    let mut invites = Vec::new();

    let rooms = v.get("rooms").and_then(|r| r.get("join")).and_then(|j| j.as_object());
    let Some(rooms) = rooms else { return Ok((next, events, invites)) };

    for (room_id, room) in rooms {
        let timeline = room
            .get("timeline")
            .and_then(|t| t.get("events"))
            .and_then(|e| e.as_array());
        let Some(timeline) = timeline else { continue };
        for ev in timeline {
            let etype = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
            let sender = ev.get("sender").and_then(|s| s.as_str()).unwrap_or("");
            let content = ev.get("content");
            let call_id = content
                .and_then(|c| c.get("call_id"))
                .and_then(|c| c.as_str())
                .unwrap_or("");
            match etype {
                "m.call.invite" if sender != my_user && !call_id.is_empty() => {
                    let version = content
                        .and_then(|c| c.get("version"))
                        .cloned()
                        .unwrap_or(serde_json::json!(1));
                    invites.push(LiveInvite {
                        room_id: room_id.clone(),
                        call_id: call_id.to_string(),
                        version,
                    });
                    events.push(Event::CallAdded(CallInfo {
                        id: call_link_id(room_id, call_id),
                        number: sender.to_string(), // "@alice:server" is the caller
                        direction: CallDirection::Incoming,
                        state: CallState::RingingIn,
                    }));
                }
                // Answered on another device, hung up by the caller, or
                // rejected elsewhere: all end the ring here.
                "m.call.hangup" | "m.call.answer" | "m.call.reject" if !call_id.is_empty() => {
                    events.push(Event::CallState {
                        id: call_link_id(room_id, call_id),
                        state: CallState::Terminated,
                    });
                }
                // Our structured handoff event (command/file payloads).
                t if t == PAYLOAD_EVENT_TYPE && sender != my_user => {
                    if let Some(json) = content.map(|c| c.to_string()) {
                        if let Ok(payload) = Payload::from_json(&json) {
                            let ts = ev.get("origin_server_ts").and_then(|t| t.as_u64())
                                .map(|ms| ms / 1000).unwrap_or_else(now_secs);
                            events.push(Event::MessageIn {
                                peer: format!("matrix:{sender}"),
                                payload,
                                ts,
                            });
                        }
                    }
                }
                // Plain room messages from others become Text payloads. A
                // message that is itself our SMS-style `run:`/`file:` text is
                // upgraded, so the handoff works cross-transport too.
                "m.room.message" if sender != my_user => {
                    let body = content.and_then(|c| c.get("body")).and_then(|b| b.as_str());
                    if let Some(body) = body {
                        let ts = ev.get("origin_server_ts").and_then(|t| t.as_u64())
                            .map(|ms| ms / 1000).unwrap_or_else(now_secs);
                        events.push(Event::MessageIn {
                            peer: format!("matrix:{sender}"),
                            payload: message::parse_sms(body),
                            ts,
                        });
                    }
                }
                _ => {}
            }
        }
    }
    Ok((next, events, invites))
}

/// From a raw /sync body, map each room's message/handoff senders to that
/// room id, so a reply can target the right room. Pure; unit-tested.
pub fn sender_rooms(sync_json: &str) -> Vec<(String, String)> {
    let mut out = Vec::new();
    let Ok(v) = serde_json::from_str::<serde_json::Value>(sync_json) else { return out };
    let Some(join) = v.get("rooms").and_then(|r| r.get("join")).and_then(|j| j.as_object())
    else { return out };
    for (room_id, room) in join {
        let evs = room.get("timeline").and_then(|t| t.get("events")).and_then(|e| e.as_array());
        let Some(evs) = evs else { continue };
        for ev in evs {
            let t = ev.get("type").and_then(|t| t.as_str()).unwrap_or("");
            if t == "m.room.message" || t == PAYLOAD_EVENT_TYPE {
                if let Some(sender) = ev.get("sender").and_then(|s| s.as_str()) {
                    out.push((sender.to_string(), room_id.clone()));
                }
            }
        }
    }
    out
}

fn now_secs() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

// ------------------------------------------------------------------- HTTP

#[cfg(feature = "matrix")]
fn agent() -> ureq::Agent {
    ureq::AgentBuilder::new()
        .timeout_connect(Duration::from_secs(10))
        // Long-poll: read timeout must exceed the sync timeout below.
        .timeout_read(Duration::from_secs(20))
        .build()
}

/// Resolve a server name to the client-API base URL: `.well-known` first,
/// falling back to `https://{name}` — which is exactly what the spec's
/// discovery section prescribes for a missing well-known.
#[cfg(feature = "matrix")]
pub fn discover(server_name: &str) -> String {
    let name = server_name.trim().trim_end_matches('/');
    let name = name.strip_prefix("https://").unwrap_or(name);
    let url = format!("https://{name}/.well-known/matrix/client");
    if let Ok(resp) = agent().get(&url).call() {
        if let Ok(body) = resp.into_string() {
            if let Some(base) = parse_well_known(&body) {
                return base;
            }
        }
    }
    format!("https://{name}")
}

/// Password login. Blocking — call from a worker thread, never the UI.
#[cfg(feature = "matrix")]
pub fn login(homeserver: &str, user: &str, password: &str) -> Result<Creds, String> {
    let body = serde_json::json!({
        "type": "m.login.password",
        "identifier": { "type": "m.id.user", "user": user },
        "password": password,
        "initial_device_display_name": "dialer",
    });
    let url = format!("{homeserver}/_matrix/client/v3/login");
    let text = match agent().post(&url).send_string(&body.to_string()) {
        Ok(resp) => resp.into_string().map_err(|e| e.to_string())?,
        // 4xx/5xx still carry a JSON error body worth surfacing.
        Err(ureq::Error::Status(_, resp)) => resp.into_string().map_err(|e| e.to_string())?,
        Err(e) => return Err(format!("login: {e}")),
    };
    parse_login(&text, homeserver)
}

/// Send a payload to a room: the structured custom event first (rich
/// clients), then an `m.room.message` text fallback so any Matrix client
/// shows *something*. Two PUTs; the fallback failure is not fatal.
#[cfg(feature = "matrix")]
fn send_matrix(
    ag: &ureq::Agent,
    creds: &Creds,
    auth: &str,
    room: &str,
    payload: &Payload,
    txn: u64,
) -> Result<(), String> {
    let base = &creds.homeserver;
    // 1) custom event
    let url = format!(
        "{base}/_matrix/client/v3/rooms/{room}/send/{}/dialer{txn}",
        PAYLOAD_EVENT_TYPE
    );
    ag.put(&url)
        .set("Authorization", auth)
        .send_string(&payload.to_json())
        .map_err(|e| format!("{e}"))?;
    // 2) human-readable fallback
    let body = serde_json::json!({ "msgtype": "m.text", "body": payload.to_sms() });
    let url = format!("{base}/_matrix/client/v3/rooms/{room}/send/m.room.message/dialerf{txn}");
    let _ = ag.put(&url).set("Authorization", auth).send_string(&body.to_string());
    Ok(())
}

/// Spawn the sync-loop backend. Fails fast (bad token, unreachable server)
/// so the shell can toast instead of running a dead link.
#[cfg(feature = "matrix")]
pub fn spawn(creds: Creds) -> Result<Link, String> {
    // whoami up front: validates the token before we claim to be up.
    let whoami = format!("{}/_matrix/client/v3/account/whoami", creds.homeserver);
    agent()
        .get(&whoami)
        .set("Authorization", &format!("Bearer {}", creds.access_token))
        .call()
        .map_err(|e| format!("matrix session invalid: {e}"))?;

    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (ev_tx, ev_rx) = channel::<Event>();

    std::thread::Builder::new()
        .name("dialer-matrix".into())
        .spawn(move || {
            let _ = ev_tx.send(Event::ModemUp { label: creds.user_id.clone() });
            let ag = agent();
            let auth = format!("Bearer {}", creds.access_token);
            let mut since: Option<String> = None;
            let mut live: HashMap<String, LiveInvite> = HashMap::new(); // link id → invite
            let mut dm_room: HashMap<String, String> = HashMap::new();  // "@user:x" → "!room:x"
            let mut txn = 0u64;

            loop {
                // 1) drain commands (non-blocking; the poll below is the wait)
                loop {
                    match cmd_rx.try_recv() {
                        Ok(Command::Quit) => return,
                        Ok(Command::Hangup(id)) => {
                            let Some(inv) = live.remove(&id) else { continue };
                            txn += 1;
                            let url = format!(
                                "{}/_matrix/client/v3/rooms/{}/send/m.call.hangup/dialer{txn}",
                                creds.homeserver, inv.room_id
                            );
                            let body = serde_json::json!({
                                "call_id": inv.call_id,
                                "version": inv.version,
                                "reason": "user_hangup",
                            });
                            match ag.put(&url).set("Authorization", &auth).send_string(&body.to_string()) {
                                Ok(_) => {
                                    let _ = ev_tx.send(Event::CallState {
                                        id,
                                        state: CallState::Terminated,
                                    });
                                }
                                Err(e) => {
                                    let _ = ev_tx.send(Event::Error(format!("matrix hangup: {e}")));
                                }
                            }
                        }
                        Ok(Command::SendMessage { peer, payload }) => {
                            // Resolve the target room: explicit "matrix/!room"
                            // or a mxid we've exchanged messages with.
                            let room = if let Some(r) = peer.strip_prefix("matrix/") {
                                Some(r.to_string())
                            } else if let Some(mx) = peer.strip_prefix("matrix:") {
                                dm_room.get(mx).cloned()
                            } else {
                                None
                            };
                            let Some(room) = room else {
                                let _ = ev_tx.send(Event::Error(format!(
                                    "no known Matrix room for {peer} (open a chat there first)")));
                                continue;
                            };
                            txn += 1;
                            let ok = send_matrix(&ag, &creds, &auth, &room, &payload, txn);
                            let _ = ev_tx.send(match ok {
                                Ok(()) => Event::MessageSent { peer: peer.clone() },
                                Err(e) => Event::Error(format!("matrix send: {e}")),
                            });
                        }
                        // No media engine yet — see module docs. The shell
                        // hides Accept for matrix calls; this is the backstop.
                        Ok(Command::Accept(_)) => {
                            let _ = ev_tx.send(Event::Error(
                                "matrix calls can't be answered yet (no media engine)".into(),
                            ));
                        }
                        Ok(_) => {} // Dial/Dtmf/ApplyApn/Simulate: not this transport
                        Err(TryRecvError::Empty) => break,
                        Err(TryRecvError::Disconnected) => return,
                    }
                }

                // 2) one long-poll. First sync uses timeout=0 + a filter that
                //    empties the timeline: history must not ring as new calls.
                let url = match &since {
                    None => format!(
                        "{}/_matrix/client/v3/sync?timeout=0&filter=%7B%22room%22%3A%7B%22timeline%22%3A%7B%22limit%22%3A0%7D%7D%7D",
                        creds.homeserver
                    ),
                    Some(s) => format!(
                        "{}/_matrix/client/v3/sync?timeout=5000&since={}",
                        creds.homeserver, s
                    ),
                };
                let body = match ag.get(&url).set("Authorization", &auth).call() {
                    Ok(resp) => match resp.into_string() {
                        Ok(b) => b,
                        Err(e) => {
                            let _ = ev_tx.send(Event::Error(format!("matrix sync read: {e}")));
                            std::thread::sleep(Duration::from_secs(5));
                            continue;
                        }
                    },
                    Err(e) => {
                        let _ = ev_tx.send(Event::ModemDown { detail: format!("matrix: {e}") });
                        std::thread::sleep(Duration::from_secs(10));
                        continue;
                    }
                };
                for (mxid, room) in sender_rooms(&body) {
                    dm_room.insert(mxid, room);
                }
                match parse_sync(&body, &creds.user_id) {
                    Ok((next, events, invites)) => {
                        since = Some(next);
                        for inv in invites {
                            live.insert(call_link_id(&inv.room_id, &inv.call_id), inv);
                        }
                        for ev in events {
                            if let Event::CallState { id, state: CallState::Terminated } = &ev {
                                live.remove(id);
                            }
                            let _ = ev_tx.send(ev);
                        }
                    }
                    Err(e) => {
                        let _ = ev_tx.send(Event::Error(e));
                        std::thread::sleep(Duration::from_secs(5));
                    }
                }
            }
        })
        .map_err(|e| format!("spawn matrix thread: {e}"))?;

    Ok(Link { tx: cmd_tx, rx: ev_rx, kind: "matrix" })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn well_known_yields_trimmed_base_url() {
        let j = r#"{"m.homeserver":{"base_url":"https://matrix-client.example.org/"}}"#;
        assert_eq!(parse_well_known(j).unwrap(), "https://matrix-client.example.org");
        assert!(parse_well_known("{}").is_none());
        assert!(parse_well_known("not json").is_none());
    }

    #[test]
    fn login_parses_creds_and_surfaces_server_errors() {
        let ok = r#"{"user_id":"@a:x.org","access_token":"tok","device_id":"DEV"}"#;
        let c = parse_login(ok, "https://x.org").unwrap();
        assert_eq!((c.user_id.as_str(), c.access_token.as_str(), c.device_id.as_str()),
                   ("@a:x.org", "tok", "DEV"));
        let err = r#"{"errcode":"M_FORBIDDEN","error":"Invalid password"}"#;
        assert_eq!(parse_login(err, "https://x.org").unwrap_err(), "Invalid password");
    }

    const SYNC: &str = r#"{
      "next_batch": "s72595_4483_1934",
      "rooms": { "join": { "!room:x.org": { "timeline": { "events": [
        { "type": "m.room.message", "sender": "@bob:x.org",
          "content": { "body": "hi" } },
        { "type": "m.call.invite", "sender": "@bob:x.org",
          "content": { "call_id": "c123", "version": "1",
                       "offer": { "type": "offer", "sdp": "..." }, "lifetime": 60000 } },
        { "type": "m.call.invite", "sender": "@me:x.org",
          "content": { "call_id": "mine", "version": "1" } }
      ] } } } }
    }"#;

    #[test]
    fn sync_rings_others_invites_but_never_our_own() {
        let (next, events, invites) = parse_sync(SYNC, "@me:x.org").unwrap();
        assert_eq!(next, "s72595_4483_1934");
        assert_eq!(invites.len(), 1);
        assert_eq!(invites[0].call_id, "c123");
        assert_eq!(invites[0].version, serde_json::json!("1"));
        // Exactly one CallAdded (the invite); the "hi" room message also
        // yields a MessageIn, and our own invite is dropped.
        let added: Vec<&Event> = events.iter()
            .filter(|e| matches!(e, Event::CallAdded(_))).collect();
        assert_eq!(added.len(), 1);
        match added[0] {
            Event::CallAdded(info) => {
                assert_eq!(info.id, "matrix/!room:x.org/c123");
                assert_eq!(info.number, "@bob:x.org");
                assert_eq!(info.state, CallState::RingingIn);
                assert_eq!(info.direction, CallDirection::Incoming);
            }
            _ => unreachable!(),
        }
    }

    #[test]
    fn answer_elsewhere_and_hangup_both_stop_the_ring() {
        let j = r#"{ "next_batch": "n", "rooms": { "join": { "!r:x": { "timeline": { "events": [
            { "type": "m.call.answer", "sender": "@me:x.org",
              "content": { "call_id": "c1", "answer": {} } },
            { "type": "m.call.hangup", "sender": "@bob:x.org",
              "content": { "call_id": "c2" } }
        ] } } } } }"#;
        let (_, events, invites) = parse_sync(j, "@me:x.org").unwrap();
        assert!(invites.is_empty());
        let ids: Vec<&str> = events
            .iter()
            .map(|e| match e {
                Event::CallState { id, state: CallState::Terminated } => id.as_str(),
                other => panic!("expected Terminated, got {other:?}"),
            })
            .collect();
        assert_eq!(ids, ["matrix/!r:x/c1", "matrix/!r:x/c2"]);
    }

    #[test]
    fn sender_rooms_indexes_message_senders_by_room() {
        let j = r#"{ "next_batch":"n", "rooms": { "join": {
            "!r1:x": { "timeline": { "events": [
                { "type":"m.room.message", "sender":"@a:x", "content":{"body":"hi"} } ] } },
            "!r2:x": { "timeline": { "events": [
                { "type":"org.ecosystem.dialer.payload", "sender":"@b:x", "content":{"kind":"text","body":"y"} } ] } }
        } } }"#;
        let map = sender_rooms(j);
        assert!(map.contains(&("@a:x".to_string(), "!r1:x".to_string())));
        assert!(map.contains(&("@b:x".to_string(), "!r2:x".to_string())));
    }

    #[test]
    fn inbound_room_message_and_custom_payload_become_messagein() {
        let j = r#"{ "next_batch":"n", "rooms": { "join": { "!r:x": { "timeline": { "events": [
            { "type":"m.room.message", "sender":"@a:x", "origin_server_ts": 5000,
              "content": { "msgtype":"m.text", "body":"run: ls -la" } },
            { "type":"org.ecosystem.dialer.payload", "sender":"@a:x", "origin_server_ts": 6000,
              "content": { "kind":"file_offer", "name":"a.txt", "uri":"mxc://x/a" } }
        ] } } } } }"#;
        let (_, events, _) = parse_sync(j, "@me:x").unwrap();
        // First: the run: text upgraded to a command suggestion.
        match &events[0] {
            Event::MessageIn { peer, payload, ts } => {
                assert_eq!(peer, "matrix:@a:x");
                assert_eq!(*ts, 5); // ms -> s
                assert!(matches!(payload, Payload::CommandSuggestion { .. }));
            }
            other => panic!("expected MessageIn, got {other:?}"),
        }
        match &events[1] {
            Event::MessageIn { payload: Payload::FileOffer { name, .. }, .. } =>
                assert_eq!(name, "a.txt"),
            other => panic!("expected file offer MessageIn, got {other:?}"),
        }
    }

    #[test]
    fn empty_sync_is_fine_and_missing_next_batch_is_not() {
        let (next, events, _) = parse_sync(r#"{"next_batch":"n"}"#, "@me:x").unwrap();
        assert_eq!(next, "n");
        assert!(events.is_empty());
        assert!(parse_sync("{}", "@me:x").is_err());
    }
}

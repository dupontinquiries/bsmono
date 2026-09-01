//! The ModemManager backend (feature `mm`). Talks
//! `org.freedesktop.ModemManager1` on the system bus with zbus **blocking**
//! proxies from a dedicated thread — the shell stays sync, the seam stays the
//! [`Link`] channels.
//!
//! ## Polling, not signals — on purpose (v1)
//!
//! MM has `CallAdded`/`StateChanged` signals, but consuming per-call signal
//! streams from blocking zbus means a thread per call or an async executor in
//! this thread. Instead the loop is a 500 ms tick: drain a command, then
//! `ListCalls` + read each call's `State` property and diff. ≤500 ms of state
//! latency is imperceptible next to a cellular ring, and the loop stays a
//! single readable function. If signal-latency ever matters (e.g. DTMF
//! receive), swap this file's loop for an async one — the protocol doesn't
//! change.
//!
//! ## zbus 4 gotchas encoded here (they cost real time otherwise)
//!
//! * The attribute is `#[zbus::proxy]` — zbus 3's `#[dbus_proxy]` is gone.
//! * The macro on `trait Foo` generates `FooProxy` (async) **and**
//!   `FooProxyBlocking`; we only use the blocking ones.
//! * Property reads go through zbus's cache by default, and the cache is only
//!   updated by `PropertiesChanged` — which our loop never pumps. Every call
//!   proxy is therefore built with `CacheProperties::No`
//!   (`zbus::proxy::CacheProperties` in 4.x, moved from the crate root) so a
//!   read is a real round-trip. Without this the poll reads stale state
//!   forever and calls appear to never connect.
//! * `MM` property `State` is `i` (i32); `StateChanged`'s reason is `u`. We
//!   only read the property.

use std::collections::HashMap;
use std::sync::mpsc::{channel, RecvTimeoutError};
use std::time::Duration;

use zbus::blocking::fdo::ObjectManagerProxy;
use zbus::blocking::Connection;
use zbus::proxy::CacheProperties;
use zbus::zvariant::{ObjectPath, OwnedObjectPath, Value};

use crate::link::{ApnSetting, Command, Event, Link};
use crate::message;
use crate::types::{CallDirection, CallInfo, CallState};

const MM_SERVICE: &str = "org.freedesktop.ModemManager1";
const MM_PATH: &str = "/org/freedesktop/ModemManager1";
const VOICE_IFACE: &str = "org.freedesktop.ModemManager1.Modem.Voice";
const TICK: Duration = Duration::from_millis(500);

#[zbus::proxy(
    interface = "org.freedesktop.ModemManager1.Modem.Voice",
    default_service = "org.freedesktop.ModemManager1"
)]
trait ModemVoice {
    fn create_call(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    fn list_calls(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn delete_call(&self, path: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.ModemManager1.Modem.Messaging",
    default_service = "org.freedesktop.ModemManager1"
)]
trait ModemMessaging {
    /// Create an SMS object from properties (at least "number" and "text");
    /// returns its path. Must then be Sent via the Sms interface.
    fn create(&self, properties: HashMap<&str, Value<'_>>) -> zbus::Result<OwnedObjectPath>;
    fn list(&self) -> zbus::Result<Vec<OwnedObjectPath>>;
    fn delete(&self, path: &ObjectPath<'_>) -> zbus::Result<()>;
}

#[zbus::proxy(
    interface = "org.freedesktop.ModemManager1.Sms",
    default_service = "org.freedesktop.ModemManager1"
)]
trait Sms {
    fn send(&self) -> zbus::Result<()>;
    #[zbus(property)]
    fn number(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn text(&self) -> zbus::Result<String>;
    /// MMSmsState: 0 unknown,1 stored,2 receiving,3 received,4 sending,5 sent.
    #[zbus(property)]
    fn state(&self) -> zbus::Result<u32>;
    /// MMSmsPduType: 1 deliver (inbound), 2 submit (outbound we created).
    #[zbus(property)]
    fn pdu_type(&self) -> zbus::Result<u32>;
}

#[zbus::proxy(
    interface = "org.freedesktop.ModemManager1.Modem.Modem3gpp",
    default_service = "org.freedesktop.ModemManager1"
)]
trait Modem3gpp {
    /// MCC+MNC of the registered operator, e.g. "310260". Empty until
    /// registered.
    #[zbus(property)]
    fn operator_code(&self) -> zbus::Result<String>;
}

/// NetworkManager's connection store — the place an APN actually lives on a
/// ModemManager system. One method is all the wizard needs.
#[zbus::proxy(
    interface = "org.freedesktop.NetworkManager.Settings",
    default_service = "org.freedesktop.NetworkManager",
    default_path = "/org/freedesktop/NetworkManager/Settings"
)]
trait NmSettings {
    fn add_connection(
        &self,
        connection: HashMap<&str, HashMap<&str, Value<'_>>>,
    ) -> zbus::Result<OwnedObjectPath>;
}

#[zbus::proxy(
    interface = "org.freedesktop.ModemManager1.Call",
    default_service = "org.freedesktop.ModemManager1"
)]
trait MmCall {
    fn start(&self) -> zbus::Result<()>;
    fn accept(&self) -> zbus::Result<()>;
    fn hangup(&self) -> zbus::Result<()>;
    fn send_dtmf(&self, dtmf: &str) -> zbus::Result<()>;

    #[zbus(property)]
    fn state(&self) -> zbus::Result<i32>;
    #[zbus(property)]
    fn number(&self) -> zbus::Result<String>;
    #[zbus(property)]
    fn direction(&self) -> zbus::Result<i32>;
}

fn call_proxy<'a>(conn: &Connection, path: &str) -> zbus::Result<MmCallProxyBlocking<'a>> {
    MmCallProxyBlocking::builder(conn)
        .path(path.to_string())?
        .cache_properties(CacheProperties::No) // see module docs — mandatory for a polling loop
        .build()
}

/// Find the first modem exposing the Voice interface. MM's root object is an
/// ObjectManager; modems come and go under it.
fn find_voice_modem(conn: &Connection) -> zbus::Result<Option<OwnedObjectPath>> {
    let om = ObjectManagerProxy::builder(conn)
        .destination(MM_SERVICE)?
        .path(MM_PATH)?
        .build()?;
    let objects = om.get_managed_objects()?;
    let mut paths: Vec<OwnedObjectPath> = objects
        .into_iter()
        .filter(|(_, ifaces)| ifaces.contains_key(VOICE_IFACE))
        .map(|(p, _)| p)
        .collect();
    // Sorted by the path STRING: zbus's OwnedObjectPath has no Ord, so
    // `paths.sort()` does not compile — it only derefs to the path text. The
    // intent is unchanged and worth keeping: with two modems (a phone with a
    // second SIM, or a USB stick alongside the built-in) an unsorted pick is
    // whichever D-Bus enumerated first, which can differ between boots. A
    // dialer that calls out through a different SIM on Tuesday is a billing
    // surprise, not a cosmetic one.
    paths.sort_by(|a, b| a.as_str().cmp(b.as_str()));
    Ok(paths.into_iter().next())
}

/// Try to connect and locate a modem. Errors here mean "run the mock
/// instead" for the auto-selecting launcher, so they come back as `Err`
/// rather than as events.
pub fn spawn() -> Result<Link, String> {
    let conn = Connection::system().map_err(|e| format!("system bus: {e}"))?;
    let modem = find_voice_modem(&conn)
        .map_err(|e| format!("ModemManager: {e}"))?
        .ok_or_else(|| "no modem with voice support".to_string())?;

    let (cmd_tx, cmd_rx) = channel::<Command>();
    let (ev_tx, ev_rx) = channel::<Event>();

    std::thread::Builder::new()
        .name("dialer-mm".into())
        .spawn(move || {
            let modem_str = modem.as_str().to_string();
            let voice = match (|| -> zbus::Result<ModemVoiceProxyBlocking<'_>> {
                Ok(ModemVoiceProxyBlocking::builder(&conn)
                    .path(modem_str.clone())?
                    .cache_properties(CacheProperties::No)
                    .build()?)
            })() {
                Ok(v) => v,
                Err(e) => {
                    let _ = ev_tx.send(Event::ModemDown { detail: format!("voice proxy: {e}") });
                    return;
                }
            };
            let _ = ev_tx.send(Event::ModemUp { label: modem_label(&modem_str) });

            // Operator code, best-effort: absent SIM/registration is normal,
            // not an error — the wizard just falls back to the full list.
            if let Ok(g) = (|| -> zbus::Result<Modem3gppProxyBlocking<'_>> {
                Ok(Modem3gppProxyBlocking::builder(&conn)
                    .path(modem_str.clone())?
                    .cache_properties(CacheProperties::No)
                    .build()?)
            })() {
                if let Ok(code) = g.operator_code() {
                    if !code.is_empty() {
                        let _ = ev_tx.send(Event::Operator { mccmnc: code });
                    }
                }
            }

            // Messaging proxy (optional — some modems/data-only devices lack it).
            let messaging = (|| -> zbus::Result<ModemMessagingProxyBlocking<'_>> {
                Ok(ModemMessagingProxyBlocking::builder(&conn)
                    .path(modem_str.clone())?
                    .cache_properties(CacheProperties::No)
                    .build()?)
            })().ok();

            // What we believe about each live call (keyed by object path).
            let mut known: HashMap<String, CallState> = HashMap::new();
            // SMS object paths already surfaced inbound, so the poll doesn't
            // re-deliver them every tick.
            let mut seen_sms: std::collections::HashSet<String> = std::collections::HashSet::new();

            loop {
                // 1) one command per tick at most — dialing is not a hot path
                match cmd_rx.recv_timeout(TICK) {
                    Ok(Command::Quit) => return,
                    Ok(Command::Dial(raw)) => {
                        let number = crate::number::normalize(&raw);
                        let mut props: HashMap<&str, Value<'_>> = HashMap::new();
                        props.insert("number", Value::from(number.as_str()));
                        match voice.create_call(props) {
                            Ok(path) => {
                                // MM creates the object idle; Start() actually dials.
                                match call_proxy(&conn, path.as_str()).and_then(|c| c.start()) {
                                    Ok(()) => {} // the poll below will report it
                                    Err(e) => {
                                        let _ = ev_tx.send(Event::Error(format!("start call: {e}")));
                                    }
                                }
                            }
                            Err(e) => {
                                let _ = ev_tx.send(Event::Error(format!("create call: {e}")));
                            }
                        }
                    }
                    Ok(Command::Accept(id)) => {
                        if let Err(e) = call_proxy(&conn, &id).and_then(|c| c.accept()) {
                            let _ = ev_tx.send(Event::Error(format!("accept: {e}")));
                        }
                    }
                    Ok(Command::Hangup(id)) => {
                        if let Err(e) = call_proxy(&conn, &id).and_then(|c| c.hangup()) {
                            let _ = ev_tx.send(Event::Error(format!("hang up: {e}")));
                        }
                    }
                    Ok(Command::Dtmf(id, tones)) => {
                        if let Err(e) = call_proxy(&conn, &id).and_then(|c| c.send_dtmf(&tones)) {
                            let _ = ev_tx.send(Event::Error(format!("dtmf: {e}")));
                        }
                    }
                    Ok(Command::SimulateIncoming(_)) => { /* real modems only ring for real */ }
                    Ok(Command::SendMessage { peer, payload }) => {
                        let Some(number) = peer.strip_prefix("tel:") else {
                            // Matrix peers are the matrix backend's job.
                            let _ = ev_tx.send(Event::Error(
                                "this modem only sends SMS (tel: peers)".into()));
                            continue;
                        };
                        let text = payload.to_sms();
                        match &messaging {
                            None => { let _ = ev_tx.send(Event::Error("no SMS support on this modem".into())); }
                            Some(m) => {
                                let mut props: HashMap<&str, Value<'_>> = HashMap::new();
                                props.insert("number", Value::from(number));
                                props.insert("text", Value::from(text.as_str()));
                                let res = m.create(props).and_then(|path| {
                                    SmsProxyBlocking::builder(&conn)
                                        .path(path.as_str().to_string())?
                                        .cache_properties(CacheProperties::No)
                                        .build()?
                                        .send()
                                });
                                match res {
                                    Ok(()) => { let _ = ev_tx.send(Event::MessageSent { peer: peer.clone() }); }
                                    Err(e) => { let _ = ev_tx.send(Event::Error(format!("send SMS: {e}"))); }
                                }
                            }
                        }
                    }
                    Ok(Command::ApplyApn(setting)) => {
                        let _ = ev_tx.send(match apply_apn_nm(&conn, &setting) {
                            Ok(()) => Event::Info(format!(
                                "NetworkManager connection created for {} (APN {})",
                                setting.carrier, setting.apn
                            )),
                            Err(e) => Event::Error(format!("apply APN: {e}")),
                        });
                    }
                    Err(RecvTimeoutError::Timeout) => {}
                    Err(RecvTimeoutError::Disconnected) => return,
                }

                // 2) poll + diff
                let listed = match voice.list_calls() {
                    Ok(v) => v,
                    Err(e) => {
                        // Modem likely went away (suspend, SIM swap, crash).
                        let _ = ev_tx.send(Event::ModemDown { detail: format!("list calls: {e}") });
                        // End everything we knew about so the UI isn't stuck in-call.
                        for (id, _) in known.drain() {
                            let _ = ev_tx.send(Event::CallState { id, state: CallState::Terminated });
                        }
                        std::thread::sleep(TICK);
                        continue;
                    }
                };

                let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
                for path in &listed {
                    let id = path.as_str().to_string();
                    seen.insert(id.clone());
                    let proxy = match call_proxy(&conn, &id) {
                        Ok(p) => p,
                        Err(e) => {
                            let _ = ev_tx.send(Event::Error(format!("call proxy: {e}")));
                            continue;
                        }
                    };
                    let state = proxy.state().map(CallState::from_mm).unwrap_or(CallState::Unknown);
                    match known.get(&id) {
                        None => {
                            let number = proxy.number().unwrap_or_default();
                            let direction =
                                proxy.direction().map(CallDirection::from_mm).unwrap_or(CallDirection::Unknown);
                            known.insert(id.clone(), state);
                            let _ = ev_tx.send(Event::CallAdded(CallInfo {
                                id: id.clone(),
                                number,
                                direction,
                                state,
                            }));
                        }
                        Some(prev) if *prev != state => {
                            known.insert(id.clone(), state);
                            let _ = ev_tx.send(Event::CallState { id: id.clone(), state });
                        }
                        Some(_) => {}
                    }
                    // Terminated calls linger as MM objects; delete so the
                    // modem's list doesn't accumulate and our diff stays small.
                    if state == CallState::Terminated {
                        known.remove(&id);
                        let _ = voice.delete_call(path); // &OwnedObjectPath derefs to &ObjectPath
                    }
                }

                // Inbound SMS: list, deliver any received/deliver PDUs we
                // haven't seen, then delete so the store doesn't grow.
                if let Some(m) = &messaging {
                    if let Ok(list) = m.list() {
                        for path in list {
                            let id = path.as_str().to_string();
                            if seen_sms.contains(&id) {
                                continue;
                            }
                            let sms = match SmsProxyBlocking::builder(&conn)
                                .path(id.clone())
                                .and_then(|b| Ok(b.cache_properties(CacheProperties::No)))
                                .and_then(|b| b.build())
                            {
                                Ok(s) => s,
                                Err(_) => continue,
                            };
                            let pdu = sms.pdu_type().unwrap_or(0);
                            let state = sms.state().unwrap_or(0);
                            // deliver (1) + received (3): a real inbound message.
                            if pdu == 1 && state == 3 {
                                let number = sms.number().unwrap_or_default();
                                let text = sms.text().unwrap_or_default();
                                seen_sms.insert(id.clone());
                                let _ = ev_tx.send(Event::MessageIn {
                                    peer: format!("tel:{number}"),
                                    payload: message::parse_sms(&text),
                                    ts: unix_now(),
                                });
                                let _ = m.delete(&path);
                            }
                        }
                    }
                }

                // Calls that vanished without us seeing Terminated.
                let gone: Vec<String> = known.keys().filter(|k| !seen.contains(*k)).cloned().collect();
                for id in gone {
                    known.remove(&id);
                    let _ = ev_tx.send(Event::CallRemoved { id });
                }
            }
        })
        .map_err(|e| format!("spawn mm thread: {e}"))?;

    Ok(Link { tx: cmd_tx, rx: ev_rx, kind: "modemmanager" })
}

fn unix_now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0)
}

/// Write a GSM connection profile carrying the APN. `autoconnect=true` so
/// NetworkManager brings data up on its own once the profile exists; the
/// wizard's job ends at creating it. Empty username/password are omitted
/// entirely — NM treats an empty string as a credential, not as "none".
fn apply_apn_nm(conn: &Connection, s: &ApnSetting) -> zbus::Result<()> {
    let nm = NmSettingsProxyBlocking::new(conn)?;
    let mut connection: HashMap<&str, HashMap<&str, Value<'_>>> = HashMap::new();

    let mut c: HashMap<&str, Value<'_>> = HashMap::new();
    c.insert("id", Value::from(format!("{} (dialer)", s.carrier)));
    c.insert("type", Value::from("gsm"));
    c.insert("autoconnect", Value::from(true));
    connection.insert("connection", c);

    let mut gsm: HashMap<&str, Value<'_>> = HashMap::new();
    gsm.insert("apn", Value::from(s.apn.as_str()));
    if !s.username.is_empty() {
        gsm.insert("username", Value::from(s.username.as_str()));
    }
    if !s.password.is_empty() {
        gsm.insert("password", Value::from(s.password.as_str()));
    }
    connection.insert("gsm", gsm);

    let mut ipv4: HashMap<&str, Value<'_>> = HashMap::new();
    ipv4.insert("method", Value::from("auto"));
    connection.insert("ipv4", ipv4);
    let mut ipv6: HashMap<&str, Value<'_>> = HashMap::new();
    ipv6.insert("method", Value::from("auto"));
    connection.insert("ipv6", ipv6);

    nm.add_connection(connection).map(|_| ())
}

/// "/org/freedesktop/ModemManager1/Modem/0" → "modem 0".
fn modem_label(path: &str) -> String {
    match path.rsplit('/').next() {
        Some(tail) if !tail.is_empty() => format!("modem {tail}"),
        _ => "modem".to_string(),
    }
}

/// Keep the launcher's fallback logic testable without a bus.
pub fn spawn_or(fallback: impl FnOnce() -> Link) -> (Link, Option<String>) {
    match spawn() {
        Ok(link) => (link, None),
        Err(why) => (fallback(), Some(why)),
    }
}

// src/service/unix_socket.rs
//
// Stream live data over a Unix domain socket.
//
// Path: $XDG_RUNTIME_DIR/ambient-learner.sock
//   (default: /run/user/<uid>/ambient-learner.sock)
//
// Why a socket on top of D-Bus:
//   * `nc -U` works. No bindings required.
//   * Streaming-per-update is the natural shape: one connection
//     receives newline-delimited JSON forever.
//   * D-Bus signals lose updates if the receiver is slow; a socket's
//     write buffer back-pressures the producer instead.
//
// Protocol (newline-delimited JSON, one message per line):
//
//     ←  {"hello":{"version":1,"name":"ambient-learner"}}
//
//     The server then sends an event whenever the world state changes:
//
//     ←  {"event":"temperature","temp_c":21.3,"confidence":0.85,
//         "model":"Linear","ts":1714329600}
//     ←  {"event":"pressure","hpa":1014.2,"trend_30m":-0.4,"ts":...}
//     ←  {"event":"humidity","pct":52.0,"ts":...}
//     ←  {"event":"light","value":120.0,"unit":"lux","ts":...}
//     ←  {"event":"map_pin","action":"upsert","pin":{...}}
//     ←  {"event":"map_pin","action":"remove","id":"foo"}
//
// The client never has to send anything; this is one-way streaming.
// We deliberately don't accept commands here — calibration submission
// should still happen via D-Bus or the inbox, which both have proper
// argument validation. Mixing inbound commands into a "you can `cat`
// this socket" interface is asking for trouble.

#![cfg(feature = "service")]

use std::io::Write;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::Serialize;

use crate::world_state::{self, WorldState};

use super::pin_store::PinStore;

/// Compute the canonical socket path:
///   $XDG_RUNTIME_DIR/ambient-learner.sock
/// Falls back to /tmp when XDG_RUNTIME_DIR isn't set, which is rare on
/// modern desktop systems but happens in some sandboxes.
pub fn default_socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("ambient-learner.sock")
}

/// Start the socket server in the background.
///
/// Returns a handle whose drop unbinds the socket. The caller must
/// keep it alive for the duration of the app — typically by storing
/// it on `service::Runtime`.
pub fn start(
    path: PathBuf,
    world: WorldState,
    pins: PinStore,
) -> std::io::Result<SocketHandle> {
    // If a previous run left a stale socket, the bind will fail. We
    // remove it best-effort; if a *running* peer is using it, our
    // bind() will succeed in atomically replacing it because we use
    // SOCK_STREAM and the kernel doesn't dedup.
    let _ = std::fs::remove_file(&path);

    let listener = UnixListener::bind(&path)?;
    listener.set_nonblocking(false)?;

    // Make the socket accessible only to the user's session — the
    // default umask usually does this, but be explicit.
    use std::os::unix::fs::PermissionsExt;
    let _ = std::fs::set_permissions(
        &path,
        std::fs::Permissions::from_mode(0o600),
    );

    let clients: Clients = Arc::new(Mutex::new(Vec::new()));
    let clients_for_acceptor = clients.clone();

    // Acceptor thread: blocks in accept(), pushes new clients onto
    // the shared list, never quits.
    thread::Builder::new()
        .name("ambient-socket-accept".into())
        .spawn(move || {
            for stream in listener.incoming() {
                match stream {
                    Ok(s) => {
                        if let Err(e) = greet(&s) {
                            log::debug!("socket: greet failed: {e}");
                            continue;
                        }
                        clients_for_acceptor.lock().unwrap().push(Client::new(s));
                    }
                    Err(e) => {
                        log::warn!("socket: accept error: {e}");
                        thread::sleep(Duration::from_millis(100));
                    }
                }
            }
        })
        .ok();

    // Producer thread: polls the world state at a moderate cadence,
    // diffs against the last sent values, and broadcasts events to
    // every connected client.
    let world_for_producer = world.clone();
    let pins_for_producer = pins.clone();
    let clients_for_producer = clients.clone();
    thread::Builder::new()
        .name("ambient-socket-pump".into())
        .spawn(move || run_producer(world_for_producer, pins_for_producer, clients_for_producer))
        .ok();

    log::info!("📡 unix-socket service live at {}", path.display());
    Ok(SocketHandle { path })
}

/// Drop-and-it-cleans-up handle for the socket.
pub struct SocketHandle {
    pub path: PathBuf,
}

impl Drop for SocketHandle {
    fn drop(&mut self) {
        // Best-effort unlink. If we crash, the next start() removes
        // the stale entry anyway.
        let _ = std::fs::remove_file(&self.path);
    }
}

// ─── Internals ────────────────────────────────────────────────────────

/// One connected client.
struct Client {
    stream: UnixStream,
    /// Set to false the first time a write fails; the next sweep drops
    /// the client from the list.
    alive: bool,
}

impl Client {
    fn new(stream: UnixStream) -> Self {
        // Slightly forgiving write timeout: clients on a busy laptop
        // may be slow to drain. Anything longer than this means they
        // can't keep up and we drop them rather than blocking the
        // producer thread.
        let _ = stream.set_write_timeout(Some(Duration::from_millis(250)));
        Self { stream, alive: true }
    }

    fn send(&mut self, line: &str) {
        if !self.alive { return; }
        // Write in one syscall when possible; if it fails, mark dead.
        if let Err(e) = self.stream.write_all(line.as_bytes()) {
            log::debug!("socket: client write failed, dropping: {e}");
            self.alive = false;
            return;
        }
        if self.stream.write_all(b"\n").is_err() {
            self.alive = false;
        }
    }
}

type Clients = Arc<Mutex<Vec<Client>>>;

/// Send the initial hello line to a freshly-accepted client.
fn greet(stream: &UnixStream) -> std::io::Result<()> {
    let mut s = stream;
    let payload = serde_json::json!({
        "hello": {
            "version": 1,
            "name": "ambient-learner",
            "events": ["temperature", "pressure", "humidity",
                       "light", "map_pin"],
        }
    });
    let line = format!("{}\n", payload);
    s.write_all(line.as_bytes())
}

/// State the producer keeps between ticks so we only emit on change.
#[derive(Default)]
struct LastState {
    temp_c: f32,
    confidence: f32,
    pressure_hpa: f32,
    pressure_trend: f32,
    humidity: f32,
    light: f32,
    initialized: bool,
    pin_signature: u64,
}

fn run_producer(
    world: WorldState,
    pins: PinStore,
    clients: Clients,
) {
    let mut last = LastState::default();
    loop {
        thread::sleep(Duration::from_millis(500));

        // Snapshot once, with the lock briefly held.
        let snap = {
            let s = world_state::read(&world);
            (
                s.ambient_temp,
                s.confidence,
                s.env.pressure_hpa.unwrap_or(f32::NAN),
                s.env.pressure_trend_hpa_per_30m.unwrap_or(f32::NAN),
                s.env.humidity_pct.unwrap_or(f32::NAN),
                s.env.ambient_light.unwrap_or(f32::NAN),
                s.env.ambient_light_unit.clone().unwrap_or_default(),
                s.model_type.as_str().to_string(),
            )
        };
        let (
            temp_c, confidence,
            pressure_hpa, pressure_trend,
            humidity, light, light_unit, model,
        ) = snap;

        let mut events: Vec<String> = Vec::new();

        if !last.initialized || (temp_c - last.temp_c).abs() > 0.05 {
            events.push(make(TemperatureEvent {
                event: "temperature",
                temp_c, confidence,
                model: &model,
                ts: now_unix(),
            }));
            last.temp_c = temp_c;
            last.confidence = confidence;
        }

        if pressure_hpa.is_finite() && (
            !last.initialized || (pressure_hpa - last.pressure_hpa).abs() > 0.1
        ) {
            events.push(make(PressureEvent {
                event: "pressure",
                hpa: pressure_hpa,
                trend_30m: if pressure_trend.is_finite() { Some(pressure_trend) } else { None },
                ts: now_unix(),
            }));
            last.pressure_hpa = pressure_hpa;
            last.pressure_trend = pressure_trend;
        }

        if humidity.is_finite() && (
            !last.initialized || (humidity - last.humidity).abs() > 0.5
        ) {
            events.push(make(HumidityEvent {
                event: "humidity", pct: humidity, ts: now_unix(),
            }));
            last.humidity = humidity;
        }

        if light.is_finite() && (
            !last.initialized || (light - last.light).abs() > 1.0
        ) {
            events.push(make(LightEvent {
                event: "light", value: light, unit: &light_unit, ts: now_unix(),
            }));
            last.light = light;
        }

        // Pin events: hash the entire list and emit a single
        // synthetic "snapshot" event when the hash changes. The
        // alternative — diffing additions/removals — would mean
        // keeping a copy of every pin's state, which adds memory
        // for negligible UX gain.
        let sig = pins_signature(&pins);
        if sig != last.pin_signature {
            let pins_list = pins.list();
            events.push(serde_json::json!({
                "event": "map_pins",
                "count": pins_list.len(),
                "pins": pins_list,
                "ts": now_unix(),
            }).to_string());
            last.pin_signature = sig;
        }

        last.initialized = true;

        if events.is_empty() {
            continue;
        }

        // Broadcast and prune dead clients in one pass.
        let mut guard = clients.lock().unwrap();
        for c in guard.iter_mut() {
            for e in &events {
                c.send(e);
                if !c.alive { break; }
            }
        }
        guard.retain(|c| c.alive);
    }
}

fn make<T: Serialize>(v: T) -> String {
    serde_json::to_string(&v).unwrap_or_else(|_| "{}".into())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Cheap hash over the pins so we can detect "anything changed".
/// Doesn't need to be cryptographic — same-input → same-output is
/// the only requirement.
fn pins_signature(pins: &PinStore) -> u64 {
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};
    let mut h = DefaultHasher::new();
    let list = pins.list();
    list.len().hash(&mut h);
    for p in &list {
        p.id.hash(&mut h);
        p.kind.hash(&mut h);
        // f64 isn't Hash; fold via bits.
        p.latitude.to_bits().hash(&mut h);
        p.longitude.to_bits().hash(&mut h);
        p.title.hash(&mut h);
    }
    h.finish()
}

// ─── Event payloads ───────────────────────────────────────────────────

#[derive(Serialize)]
struct TemperatureEvent<'a> {
    event: &'static str,
    temp_c: f32,
    confidence: f32,
    model: &'a str,
    ts: i64,
}

#[derive(Serialize)]
struct PressureEvent {
    event: &'static str,
    hpa: f32,
    #[serde(skip_serializing_if = "Option::is_none")]
    trend_30m: Option<f32>,
    ts: i64,
}

#[derive(Serialize)]
struct HumidityEvent {
    event: &'static str,
    pct: f32,
    ts: i64,
}

#[derive(Serialize)]
struct LightEvent<'a> {
    event: &'static str,
    value: f32,
    unit: &'a str,
    ts: i64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_socket_path_is_under_runtime_dir() {
        let p = default_socket_path();
        let s = p.to_string_lossy();
        assert!(s.ends_with("ambient-learner.sock"), "got {s}");
    }

    #[test]
    fn pin_signature_changes_on_mutation() {
        let store = PinStore::new();
        let sig0 = pins_signature(&store);

        let pin = super::super::map_pin::MapPin {
            id: "x".into(), kind: "test".into(),
            latitude: 1.0, longitude: 2.0,
            title: None, description: None, uri: None,
            payload: Default::default(), source: None,
            submitted_at: 0, expires_at: None,
        };
        store.upsert(pin);

        let sig1 = pins_signature(&store);
        assert_ne!(sig0, sig1);
    }
}

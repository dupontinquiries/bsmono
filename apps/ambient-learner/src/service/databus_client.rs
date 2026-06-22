// src/service/databus_client.rs
//
// Client for the external `snapclone-bus` Unix-socket daemon.
//
// snapclone-bus is a separate, app-agnostic daemon that runs at
// `$XDG_RUNTIME_DIR/snapclone-bus.sock` and shuttles geo-located
// "MapPoint" records between every app that opts in. The protocol is
// newline-delimited JSON; the daemon supports `pub`, `sub`, `list`,
// and `ping` operations.
//
// Our role:
//   * publish: when our world state changes (temperature update,
//     map-pin upsert, etc), forward the relevant points onto the bus
//     so other subscribers see them.
//   * subscribe: import points the bus pushes to us into our local
//     PinStore so they render in the Qt UI alongside locally-submitted
//     pins.
//
// We intentionally do NOT depend on the snapclone-databus Rust crate.
// Their wire protocol is simple enough that a 200-line client keeps us
// loosely coupled and avoids a build-graph cycle if they happen to
// depend on us in turn.
//
// If the bus isn't running, this module logs once at info level and
// quietly does nothing. The rest of our service feature (D-Bus, inbox,
// our own outbound socket) keeps working unaffected.

#![cfg(feature = "service")]

use std::io::{BufRead, BufReader, Write};
use std::os::unix::net::UnixStream;
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::thread;
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::map_pin::MapPin;
use super::pin_store::PinStore;

/// Default socket path. Matches the snapclone-bus daemon convention
/// of `$XDG_RUNTIME_DIR/snapclone-bus.sock`.
pub fn default_socket_path() -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .filter(|p| !p.as_os_str().is_empty())
        .unwrap_or_else(|| PathBuf::from("/tmp"));
    dir.join("snapclone-bus.sock")
}

/// Reverse-DNS identifier we publish ourselves under. The bus uses
/// this in its audit log and "who published this" UI.
pub const SOURCE_APP: &str = "org.ambientlearner";

/// Reconnect backoff: try the bus every N seconds when it's down.
/// Long enough that a missing daemon doesn't spam logs; short enough
/// that the federation lights up promptly when the user starts the
/// daemon mid-session.
const RECONNECT_BACKOFF: Duration = Duration::from_secs(3);

/// Layers we care about. The bus uses a typed `Layer` enum on its
/// side; we send/receive the string form. We listen to PublicSnap,
/// WeatherStation, PublicPhoto, and ShoppingDirectory — these line up
/// 1:1 with the well-known kinds in our own `MapPin`.
const SUBSCRIBED_LAYERS: &[&str] = &[
    "public_snap",
    "weather_station",
    "public_photo",
    "shopping_directory",
];

/// Map our `MapPin.kind` to the bus's `Layer` string.
fn pin_kind_to_layer(kind: &str) -> String {
    match kind {
        "snap"     => "public_snap".to_string(),
        "weather"  => "weather_station".to_string(),
        "photo"    => "public_photo".to_string(),
        "shop"     => "shopping_directory".to_string(),
        "sensor"   => "sensor_reading".to_string(),
        // Anything else passes through as a Custom layer; the bus's
        // server normalizes "custom:..." into the Custom variant.
        other      => format!("custom:{}", other),
    }
}

/// Inverse of `pin_kind_to_layer`.
fn layer_to_pin_kind(layer: &str) -> String {
    match layer {
        "public_snap"         => "snap".into(),
        "weather_station"     => "weather".into(),
        "public_photo"        => "photo".into(),
        "shopping_directory"  => "shop".into(),
        "sensor_reading"      => "sensor".into(),
        s if s.starts_with("custom:") => s[7..].to_string(),
        other                 => other.to_string(),
    }
}

/// Wire-shape for a single MapPoint as the bus expects/sends it.
/// Mirror of their `snapclone_databus::MapPoint` minus a couple of
/// fields we don't use (e.g. internal cursor numbers).
#[derive(Clone, Debug, Serialize, Deserialize)]
struct WirePoint {
    /// Stable id chosen by the publisher. Reused across upserts.
    id: String,
    /// `Layer` string — see `pin_kind_to_layer`.
    layer: String,
    latitude: f64,
    longitude: f64,
    /// Optional accuracy in meters. We don't have this for our pins
    /// today; left as None.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    accuracy_m: Option<f64>,
    /// Reverse-DNS identifier of the publishing app. We send
    /// `org.ambientlearner` for everything we forward.
    source_app: String,
    /// Time-to-live in seconds. 0 means never expires.
    /// We use 1 hour for live readings and 0 for stable map pins.
    #[serde(default)]
    ttl_secs: u32,
    /// Free-form payload. The bus passes this through verbatim.
    #[serde(default)]
    payload: Value,
}

/// Operation envelopes for the bus protocol.
#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum OutgoingOp<'a> {
    /// Subscribe to one or more layers. Server will start streaming
    /// `push` messages for matching points.
    Sub { layers: &'a [&'a str] },
    /// Publish a single point.
    Pub { point: &'a WirePoint },
    /// Liveness probe; server replies with `{"op":"pong"}`.
    Ping,
}

#[derive(Deserialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum IncomingOp {
    /// A point was published by someone (possibly us, possibly not).
    Push { point: WirePoint },
    /// Reply to our `Ping`. We don't actually do anything with it,
    /// but parsing it cleanly lets the loop keep going.
    Pong,
    /// Server-side error message.
    Error { reason: String },
}

/// Public handle the rest of the service holds onto.
///
/// Cloning is cheap and gives a new handle to the same underlying
/// connection state.
#[derive(Clone)]
pub struct DataBusClient {
    inner: Arc<Mutex<Inner>>,
}

struct Inner {
    /// Currently-open writer half. `None` while we're disconnected.
    writer: Option<UnixStream>,
    /// True once we've logged "bus not available" so we don't spam.
    logged_unavailable: bool,
}

impl DataBusClient {
    /// Spawn the client. Returns immediately; the connection happens
    /// in background threads.
    ///
    /// `pin_store` is what we mirror inbound `push` events into so
    /// the Qt UI sees them.
    pub fn start(socket_path: PathBuf, pin_store: PinStore) -> Self {
        let client = DataBusClient {
            inner: Arc::new(Mutex::new(Inner {
                writer: None,
                logged_unavailable: false,
            })),
        };

        // Spawn the read/reconnect loop. Writes happen synchronously
        // from `publish_*` methods on the caller's thread.
        let bg = client.clone();
        thread::Builder::new()
            .name("ambient-databus".into())
            .spawn(move || run_loop(bg, socket_path, pin_store))
            .ok();

        client
    }

    /// Publish a point onto the bus. Returns silently if the bus is
    /// disconnected — the caller never has to know.
    pub fn publish_pin(&self, pin: &MapPin) {
        let point = WirePoint {
            id: pin.id.clone(),
            layer: pin_kind_to_layer(&pin.kind),
            latitude: pin.latitude,
            longitude: pin.longitude,
            accuracy_m: None,
            source_app: SOURCE_APP.to_string(),
            ttl_secs: pin
                .expires_at
                .and_then(|exp| {
                    let now = std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .ok()?
                        .as_secs() as i64;
                    let remaining = exp - now;
                    if remaining > 0 { Some(remaining as u32) } else { None }
                })
                .unwrap_or(0),
            payload: serde_json::to_value(&pin.payload).unwrap_or(Value::Null),
        };
        self.send(OutgoingOp::Pub { point: &point });
    }

    /// Publish a sensor reading (temperature, pressure, etc.).
    /// `lat`/`lon` should come from the user's current location;
    /// callers without coordinates should skip this.
    pub fn publish_sensor_reading(
        &self,
        lat: f64,
        lon: f64,
        payload: Value,
        ttl_secs: u32,
    ) {
        let id = format!("ambient-{}", uuid::Uuid::new_v4());
        let point = WirePoint {
            id,
            layer: "sensor_reading".to_string(),
            latitude: lat,
            longitude: lon,
            accuracy_m: None,
            source_app: SOURCE_APP.to_string(),
            ttl_secs,
            payload,
        };
        self.send(OutgoingOp::Pub { point: &point });
    }

    /// Send any outgoing op. Best-effort; failures mark us
    /// disconnected so the read loop will reconnect.
    fn send<T: Serialize>(&self, op: T) {
        let line = match serde_json::to_string(&op) {
            Ok(s) => s,
            Err(e) => {
                log::debug!("databus: cannot serialize: {e}");
                return;
            }
        };

        let mut guard = self.inner.lock().unwrap();
        let Some(writer) = guard.writer.as_mut() else { return; };
        if writer.write_all(line.as_bytes()).is_err()
            || writer.write_all(b"\n").is_err()
        {
            // Connection broken; the read loop will pick this up and
            // reconnect.
            guard.writer = None;
        }
    }

    fn set_connected(&self, stream: UnixStream) {
        let mut g = self.inner.lock().unwrap();
        g.writer = Some(stream);
        g.logged_unavailable = false;
    }

    fn clear_connected(&self) {
        self.inner.lock().unwrap().writer = None;
    }

    fn log_unavailable_once(&self, err: &std::io::Error) {
        let mut g = self.inner.lock().unwrap();
        if !g.logged_unavailable {
            log::info!("snapclone-bus not reachable ({err}); running standalone");
            g.logged_unavailable = true;
        }
    }
}

/// Periodically publish our world state's temperature / pressure /
/// humidity onto the bus's `sensor_reading` layer. Skips ticks where
/// the user has no current-location coordinates, since a sensor point
/// without lat/lon would just clutter the federation.
pub fn spawn_sensor_publisher(
    client: DataBusClient,
    world: crate::world_state::WorldState,
    interval: Duration,
) {
    thread::Builder::new()
        .name("ambient-databus-sensor".into())
        .spawn(move || {
            loop {
                thread::sleep(interval);

                let snap = {
                    let s = crate::world_state::read(&world);
                    let coords = s.locations
                        .get(s.current_location_idx)
                        .and_then(|l| match (l.latitude, l.longitude) {
                            (Some(lat), Some(lon)) => Some((lat, lon)),
                            _ => None,
                        });
                    let temp = s.ambient_temp;
                    let conf = s.confidence;
                    let pressure = s.env.pressure_hpa;
                    let humidity = s.env.humidity_pct;
                    (coords, temp, conf, pressure, humidity)
                };

                let Some((lat, lon)) = snap.0 else { continue; };
                let (_, temp, conf, pressure, humidity) = snap;

                let mut payload = serde_json::Map::new();
                payload.insert("temperature_c".into(), serde_json::json!(temp));
                payload.insert("confidence".into(), serde_json::json!(conf));
                if let Some(p) = pressure {
                    payload.insert("pressure_hpa".into(), serde_json::json!(p));
                }
                if let Some(h) = humidity {
                    payload.insert("humidity_pct".into(), serde_json::json!(h));
                }
                payload.insert(
                    "ts".into(),
                    serde_json::json!(
                        std::time::SystemTime::now()
                            .duration_since(std::time::UNIX_EPOCH)
                            .map(|d| d.as_secs())
                            .unwrap_or(0)
                    ),
                );

                // 5-minute TTL so stale points evict on their own when
                // the app stops publishing (e.g. user closes us).
                client.publish_sensor_reading(
                    lat, lon,
                    serde_json::Value::Object(payload),
                    300,
                );
            }
        })
        .ok();
}
/// Mirror locally-owned `PinStore` entries onto the federation bus.
///
/// We track per-pin signatures so we only forward when something
/// actually changed. Bursts get collapsed (a pin upserted twice in
/// one tick is forwarded once).
///
/// Pins whose `source` starts with `"databus:"` are skipped — those
/// arrived from the bus in the first place and republishing them
/// would loop.
pub fn spawn_outbound_mirror(
    client: DataBusClient,
    pins: PinStore,
    interval: Duration,
) {
    use std::collections::HashMap;
    use std::collections::hash_map::DefaultHasher;
    use std::hash::{Hash, Hasher};

    thread::Builder::new()
        .name("ambient-databus-mirror".into())
        .spawn(move || {
            // id → last-published signature
            let mut last: HashMap<String, u64> = HashMap::new();

            loop {
                thread::sleep(interval);

                let snapshot = pins.list();

                // Build the new id-set first so we can detect deletions.
                let mut current_ids = std::collections::HashSet::new();

                for p in &snapshot {
                    current_ids.insert(p.id.clone());

                    let from_bus = p.source.as_deref()
                        .unwrap_or("")
                        .starts_with("databus:");
                    if from_bus {
                        continue;
                    }

                    // Hash the pin's user-visible state — same fields
                    // we compare in `pins_equivalent`.
                    let mut h = DefaultHasher::new();
                    p.kind.hash(&mut h);
                    p.latitude.to_bits().hash(&mut h);
                    p.longitude.to_bits().hash(&mut h);
                    p.title.hash(&mut h);
                    p.description.hash(&mut h);
                    p.uri.hash(&mut h);
                    // Payload contributes via its serialized form so
                    // ordering inside the HashMap doesn't matter.
                    if let Ok(s) = serde_json::to_string(&p.payload) {
                        s.hash(&mut h);
                    }
                    p.expires_at.hash(&mut h);
                    let sig = h.finish();

                    if last.get(&p.id).copied() != Some(sig) {
                        client.publish_pin(p);
                        last.insert(p.id.clone(), sig);
                    }
                }

                // Forget signatures for pins that no longer exist
                // locally. (We don't currently send a "remove" op
                // because the bus protocol's sketch from their docs
                // doesn't show one — TTL handles cleanup. If they
                // add a `del` op later, this is where it would slot
                // in.)
                last.retain(|id, _| current_ids.contains(id));
            }
        })
        .ok();
}

/// Background thread: connect, subscribe, read pushes, reconnect.
fn run_loop(client: DataBusClient, socket_path: PathBuf, pin_store: PinStore) {
    loop {
        let stream = match UnixStream::connect(&socket_path) {
            Ok(s) => s,
            Err(e) => {
                client.log_unavailable_once(&e);
                thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };

        log::info!("📡 connected to snapclone-bus at {}", socket_path.display());

        // Hand the *write* half to the client, keep our own clone for
        // reading. UnixStream::try_clone gives us two file descriptors
        // backed by the same underlying socket — same kernel buffer,
        // distinct close behaviour, which is exactly what we want.
        let read_stream = match stream.try_clone() {
            Ok(s) => s,
            Err(e) => {
                log::warn!("databus: try_clone failed: {e}");
                thread::sleep(RECONNECT_BACKOFF);
                continue;
            }
        };
        client.set_connected(stream);

        // First thing we do is subscribe to the layers we care about.
        client.send(OutgoingOp::Sub { layers: SUBSCRIBED_LAYERS });

        // Read until the connection breaks.
        let reader = BufReader::new(read_stream);
        for line in reader.lines() {
            let Ok(line) = line else {
                log::debug!("databus: read error, reconnecting");
                break;
            };
            if line.is_empty() {
                continue;
            }

            match serde_json::from_str::<IncomingOp>(&line) {
                Ok(IncomingOp::Push { point }) => {
                    handle_incoming_point(point, &pin_store);
                }
                Ok(IncomingOp::Pong) => { /* fine */ }
                Ok(IncomingOp::Error { reason }) => {
                    log::warn!("databus server error: {reason}");
                }
                Err(e) => {
                    log::debug!("databus: cannot parse line: {e} ({line})");
                }
            }
        }

        log::info!("snapclone-bus disconnected; will retry");
        client.clear_connected();
        thread::sleep(RECONNECT_BACKOFF);
    }
}

/// Convert an inbound `WirePoint` into a `MapPin` and upsert it. We
/// drop pins published by *us* on the way back in — the bus echoes
/// our own publishes to all subscribers (including us), and we don't
/// want to duplicate-import pins we already know about.
fn handle_incoming_point(point: WirePoint, store: &PinStore) {
    if point.source_app == SOURCE_APP {
        return;
    }

    let payload_map = match &point.payload {
        Value::Object(map) => map.iter().map(|(k, v)| (k.clone(), v.clone())).collect(),
        Value::Null => Default::default(),
        // For non-object payloads, stash the raw value under "value"
        // so the QML side still has something to render.
        other => {
            let mut m = std::collections::HashMap::new();
            m.insert("value".to_string(), other.clone());
            m
        }
    };

    let expires_at = if point.ttl_secs == 0 {
        None
    } else {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0);
        Some(now + point.ttl_secs as i64)
    };

    let pin = MapPin {
        id: format!("databus:{}", point.id),
        kind: layer_to_pin_kind(&point.layer),
        latitude: point.latitude,
        longitude: point.longitude,
        title: None,
        description: None,
        uri: None,
        payload: payload_map,
        source: Some(format!("databus:{}", point.source_app)),
        submitted_at: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0),
        expires_at,
    };

    match pin.validated() {
        Ok(p) => { store.upsert(p); }
        Err(e) => {
            log::debug!("databus: rejecting inbound point: {e}");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn kind_layer_round_trips() {
        for kind in ["snap", "weather", "photo", "shop", "sensor", "zoinks"] {
            let layer = pin_kind_to_layer(kind);
            let back = layer_to_pin_kind(&layer);
            assert_eq!(back, kind, "round-trip failed for {kind}");
        }
    }

    #[test]
    fn custom_kind_passes_through() {
        let layer = pin_kind_to_layer("transit_stop");
        assert_eq!(layer, "custom:transit_stop");
        assert_eq!(layer_to_pin_kind(&layer), "transit_stop");
    }

    #[test]
    fn handles_object_payload() {
        let store = PinStore::new();
        let p = WirePoint {
            id: "x".into(),
            layer: "public_snap".into(),
            latitude: 1.0,
            longitude: 2.0,
            accuracy_m: None,
            source_app: "com.example.someapp".into(),
            ttl_secs: 3600,
            payload: serde_json::json!({"caption": "hi"}),
        };
        handle_incoming_point(p, &store);
        assert_eq!(store.len(), 1);
        let pin = &store.list()[0];
        assert_eq!(pin.kind, "snap");
        assert_eq!(pin.payload["caption"], serde_json::json!("hi"));
    }

    #[test]
    fn ignores_self_publishes() {
        let store = PinStore::new();
        let p = WirePoint {
            id: "x".into(),
            layer: "public_snap".into(),
            latitude: 1.0,
            longitude: 2.0,
            accuracy_m: None,
            source_app: SOURCE_APP.into(),
            ttl_secs: 0,
            payload: Value::Null,
        };
        handle_incoming_point(p, &store);
        assert!(store.is_empty(), "should drop our own echoed publish");
    }
}

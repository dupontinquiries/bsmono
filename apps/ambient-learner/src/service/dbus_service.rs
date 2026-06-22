// src/service/dbus_service.rs
//
// Public D-Bus interface for `org.ambientlearner.Service1`.
//
// We publish on the *session bus* so any user-session app can talk to
// us without polkit or root. The well-known name is:
//
//     bus name:    org.ambientlearner.Service1
//     object path: /org/ambientlearner/Service
//     interface:   org.ambientlearner.Service1
//
// Methods (see SERVICE_API.md for the externally-facing description):
//
//     GetCurrentTemperature() →
//         (d temp_c, d confidence_0_1, s model_type)
//
//     GetTemperatureAt(d lat, d lon) →
//         (d temp_c, d confidence_0_1, t timestamp_unix, b in_range)
//
//     SubmitCalibration(d temp_c, s source) →
//         (b accepted, s reason)
//
//     ListLocations() →
//         a(ssdd)   id, name, lat, lon
//
//     SubmitMapPin(s id, s kind, d lat, d lon, s json_payload) →
//         (b accepted, s reason)
//     RemoveMapPin(s id) →
//         (b accepted, s reason)
//     ListMapPins() →
//         a(ssdds) id, kind, lat, lon, payload_json
//
//     GetCapabilities() →
//         a{sv}    feature flags (e.g. "neural" → true)
//
// Signals:
//
//     TemperatureChanged(d temp_c, d confidence)
//     MapPinsChanged()

#![cfg(feature = "service")]

use std::collections::HashMap;
use std::sync::Arc;

use serde::Deserialize;
use zbus::{
    fdo, interface,
    object_server::SignalContext,
    zvariant::{OwnedValue, Value},
    Connection,
};

use crate::systems::{CalibrationQueue, CalibrationRequest};
use crate::world_state::{self, WorldState};

use super::map_pin::MapPin;
use super::pin_store::PinStore;

pub const SERVICE_NAME: &str = "org.ambientlearner.Service1";
pub const SERVICE_PATH: &str = "/org/ambientlearner/Service";

/// What the service feature exposes to the rest of the app.
///
/// Keep this struct tiny — anything heavier lives behind the `Arc`s
/// inside it (`WorldState`, `PinStore`, etc.). Cloning a `ServiceState`
/// is therefore cheap and safe across threads.
#[derive(Clone)]
pub struct ServiceState {
    pub world: WorldState,
    pub pins: PinStore,
    pub calibration_queue: CalibrationQueue,
}

/// D-Bus interface implementation. The actual public surface — every
/// method here ends up callable from any session-bus client.
struct AmbientService {
    state: ServiceState,
}

#[interface(name = "org.ambientlearner.Service1")]
impl AmbientService {
    /// Read the current ambient temperature and confidence.
    async fn get_current_temperature(&self) -> (f64, f64, String) {
        let s = world_state::read(&self.state.world);
        (
            s.ambient_temp as f64,
            s.confidence as f64,
            s.model_type.as_str().to_string(),
        )
    }

    /// Look up the temperature at a specific lat/lon. We don't yet
    /// have spatial reasoning per location — for now, we return our
    /// global prediction with `in_range = true` only when there's a
    /// stored Location within ~10 km of the query point. A real
    /// implementation would interpolate from per-location calibrations.
    async fn get_temperature_at(&self, lat: f64, lon: f64) -> (f64, f64, u64, bool) {
        let s = world_state::read(&self.state.world);
        let in_range = s.locations.iter().any(|l| match (l.latitude, l.longitude) {
            (Some(la), Some(lo)) => haversine_km(la, lo, lat, lon) < 10.0,
            _ => false,
        });

        let timestamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_secs())
            .unwrap_or(0);

        (
            s.ambient_temp as f64,
            s.confidence as f64,
            timestamp,
            in_range,
        )
    }

    /// Submit a calibration measurement from another app. We accept the
    /// data into the calibration queue; the model worker will pick it
    /// up on its next tick.
    ///
    /// Returns `(accepted, reason)`. If `accepted` is false, `reason`
    /// is a human-readable explanation suitable for the caller's UI.
    async fn submit_calibration(&self, temp_c: f64, source: String) -> (bool, String) {
        if !temp_c.is_finite() || !(-50.0..=70.0).contains(&temp_c) {
            return (false, format!("temperature {temp_c} outside [-50, 70] °C"));
        }
        if source.is_empty() {
            return (false, "source must not be empty".into());
        }
        if source.len() > 64 {
            return (false, "source longer than 64 bytes".into());
        }

        self.state
            .calibration_queue
            .push(CalibrationRequest::new(temp_c as f32).with_source(source));

        (true, String::new())
    }

    /// All saved locations: `(id, name, lat, lon)`. Locations without
    /// coordinates are omitted; clients that want them should look at
    /// the QML UI's location list instead.
    async fn list_locations(&self) -> Vec<(String, String, f64, f64)> {
        let s = world_state::read(&self.state.world);
        s.locations
            .iter()
            .filter_map(|l| match (l.latitude, l.longitude) {
                (Some(lat), Some(lon)) => Some((l.id.clone(), l.name.clone(), lat, lon)),
                _ => None,
            })
            .collect()
    }

    /// Bulk export of recent temperature predictions. Returns up to
    /// `max_count` (capped at 10_000) of the most recent samples as
    /// `(unix_timestamp, temperature_c)` pairs, oldest-first.
    ///
    /// Designed for a network-aware sibling app that wants to upload
    /// our local readings to a remote service or sync with another
    /// device. We don't do that work ourselves — we're local-only.
    async fn get_temperature_history(
        &self,
        max_count: u32,
    ) -> Vec<(u64, f64)> {
        let cap = (max_count as usize).min(10_000);
        let s = world_state::read(&self.state.world);

        // We store `(Instant, f32)`. Convert each Instant to a unix
        // timestamp by anchoring "now" against the system clock.
        // Drift is bounded by how recent the sample is; for a 10-min
        // history buffer this is far more accurate than wall clock
        // sync.
        let now_inst = std::time::Instant::now();
        let now_unix = now_unix_secs();
        s.temperature_history
            .iter()
            .rev()
            .take(cap)
            .rev()
            .map(|(t, temp)| {
                let dt = now_inst.saturating_duration_since(*t).as_secs() as i64;
                let unix = (now_unix - dt).max(0) as u64;
                (unix, *temp as f64)
            })
            .collect()
    }

    /// Bulk export of recent pressure samples, same shape as the
    /// temperature history. Returns an empty vec on systems without
    /// a barometer.
    async fn get_pressure_history(&self, max_count: u32) -> Vec<(u64, f64)> {
        let cap = (max_count as usize).min(10_000);
        let s = world_state::read(&self.state.world);
        let now_inst = std::time::Instant::now();
        let now_unix = now_unix_secs();
        s.pressure_history
            .iter()
            .rev()
            .take(cap)
            .rev()
            .map(|(t, p)| {
                let dt = now_inst.saturating_duration_since(*t).as_secs() as i64;
                let unix = (now_unix - dt).max(0) as u64;
                (unix, *p as f64)
            })
            .collect()
    }

    /// Latest snapshot of every optional environmental sensor.
    /// Returns a JSON-encoded string so the API stays stable as we
    /// add new sensor types — clients parse the JSON object and pick
    /// out the keys they recognize.
    ///
    /// Shape:
    ///   {
    ///     "pressure_hpa": 1014.2,        # may be absent
    ///     "pressure_trend_hpa_30m": -0.4,
    ///     "humidity_pct": 52.1,
    ///     "ambient_light": 120.0,
    ///     "ambient_light_unit": "lux",
    ///     "accel_orientation": "normal",
    ///     "proximity_near": false
    ///   }
    async fn get_environment(&self) -> String {
        let s = world_state::read(&self.state.world);
        serde_json::to_string(&s.env).unwrap_or_else(|_| "{}".to_string())
    }


    /// Submit a map pin. The payload is JSON-encoded so we can accept
    /// arbitrary nested data without committing to a fixed D-Bus
    /// signature. Returns `(accepted, reason)`.
    async fn submit_map_pin(
        &self,
        id: String,
        kind: String,
        lat: f64,
        lon: f64,
        json_payload: String,
        #[zbus(header)] hdr: zbus::message::Header<'_>,
    ) -> (bool, String) {
        // Attribute the submission to its sender for the audit trail.
        // The sender is the D-Bus unique name like ":1.42".
        let source = hdr
            .sender()
            .map(|s| format!("dbus:{}", s))
            .unwrap_or_else(|| "dbus:unknown".to_string());

        let payload: HashMap<String, serde_json::Value> = if json_payload.is_empty() {
            HashMap::new()
        } else {
            match serde_json::from_str::<JsonPayload>(&json_payload) {
                Ok(p) => p.0,
                Err(e) => return (false, format!("payload not valid JSON object: {e}")),
            }
        };

        let pin = MapPin {
            id, kind, latitude: lat, longitude: lon,
            title: None, description: None, uri: None,
            payload, source: Some(source),
            submitted_at: now_unix(),
            expires_at: None,
        };

        match pin.validated() {
            Ok(pin) => {
                self.state.pins.upsert(pin);
                (true, String::new())
            }
            Err(reason) => (false, reason),
        }
    }

    async fn remove_map_pin(&self, id: String) -> (bool, String) {
        if self.state.pins.remove(&id).is_some() {
            (true, String::new())
        } else {
            (false, "no pin with that id".into())
        }
    }

    async fn list_map_pins(&self) -> Vec<(String, String, f64, f64, String)> {
        self.state
            .pins
            .list()
            .into_iter()
            .map(|p| {
                let payload_json = serde_json::to_string(&p.payload).unwrap_or_default();
                (p.id, p.kind, p.latitude, p.longitude, payload_json)
            })
            .collect()
    }

    /// Tell clients which optional features we support. Forward-
    /// compatible: clients should treat absent keys as false.
    async fn get_capabilities(&self) -> fdo::Result<HashMap<String, OwnedValue>> {
        let mut m = HashMap::new();
        for key in &["calibration", "map_pins", "locations", "forecasting"] {
            let v: OwnedValue = Value::from(true)
                .try_into()
                .map_err(|e: zbus::zvariant::Error| {
                    fdo::Error::Failed(format!("encode capability: {e}"))
                })?;
            m.insert((*key).to_string(), v);
        }
        Ok(m)
    }

    /// Emitted whenever the temperature changes by more than the
    /// noise threshold (mirrors the QML refresh logic).
    #[zbus(signal)]
    async fn temperature_changed(
        ctxt: &SignalContext<'_>,
        temp_c: f64,
        confidence: f64,
    ) -> zbus::Result<()>;

    /// Emitted whenever the set of map pins changes.
    #[zbus(signal)]
    async fn map_pins_changed(ctxt: &SignalContext<'_>) -> zbus::Result<()>;
}

/// Tiny wrapper for parsing payloads. We accept either a JSON object
/// or an empty string (treated as an empty object).
#[derive(Deserialize)]
struct JsonPayload(HashMap<String, serde_json::Value>);

/// Bring the service up:
///   1. Connect to the session bus.
///   2. Register the object server at our well-known path.
///   3. Request the well-known name.
///   4. Wire the pin store's change callback to emit `MapPinsChanged`.
///
/// Returns the connection so the caller can keep it alive (dropping
/// the connection unregisters the service).
pub async fn start(state: ServiceState) -> zbus::Result<Connection> {
    let conn = zbus::connection::Builder::session()?
        .name(SERVICE_NAME)?
        .serve_at(SERVICE_PATH, AmbientService { state: state.clone() })?
        .build()
        .await?;

    // Hook up MapPinsChanged so external apps can subscribe to live
    // updates instead of polling ListMapPins().
    let conn_for_pins = conn.clone();
    state.pins.on_change(move || {
        // Best-effort: errors here just mean the signal didn't fire.
        let conn = conn_for_pins.clone();
        let _ = async_io::block_on(async move {
            let path = zbus::zvariant::ObjectPath::try_from(SERVICE_PATH)?;
            let emitter = SignalContext::new(&conn, path)?;
            AmbientService::map_pins_changed(&emitter).await
        });
    });

    log::info!("🛰️  D-Bus service live as {SERVICE_NAME} at {SERVICE_PATH}");
    Ok(conn)
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs() as i64)
        .unwrap_or(0)
}

/// Same as `now_unix` but named to match the call sites in the
/// bulk-export methods. Both shapes are fine; aliasing keeps the
/// reading direction natural in each place.
fn now_unix_secs() -> i64 {
    now_unix()
}

/// Great-circle distance between two lat/lon points, in kilometers.
/// We use the standard haversine formula. Off by ≤0.5% over short
/// distances which is plenty for "is this point near a location?".
fn haversine_km(lat1: f64, lon1: f64, lat2: f64, lon2: f64) -> f64 {
    const EARTH_RADIUS_KM: f64 = 6371.0;
    let dlat = (lat2 - lat1).to_radians();
    let dlon = (lon2 - lon1).to_radians();
    let a = (dlat / 2.0).sin().powi(2)
        + lat1.to_radians().cos() * lat2.to_radians().cos() * (dlon / 2.0).sin().powi(2);
    let c = 2.0 * a.sqrt().atan2((1.0 - a).sqrt());
    EARTH_RADIUS_KM * c
}

/// Background thread that bridges WorldState → D-Bus signal.
///
/// We poll the world state at 1 Hz and emit `TemperatureChanged`
/// whenever the temperature has shifted by more than `eps`. This is
/// the same "only emit on real change" idea we used for QML.
pub fn spawn_temperature_signal_emitter(
    conn: Connection,
    world: WorldState,
) {
    use std::time::Duration;

    std::thread::Builder::new()
        .name("ambient-dbus-signals".into())
        .spawn(move || {
            const EPS: f32 = 0.05;
            let mut last_emitted = f32::NAN;
            loop {
                std::thread::sleep(Duration::from_secs(1));
                let (temp, confidence) = {
                    let s = world_state::read(&world);
                    (s.ambient_temp, s.confidence)
                };
                if (temp - last_emitted).abs() > EPS || last_emitted.is_nan() {
                    let conn2 = conn.clone();
                    let _ = async_io::block_on(async move {
                        let path = zbus::zvariant::ObjectPath::try_from(SERVICE_PATH)?;
                        let emitter = SignalContext::new(&conn2, path)?;
                        AmbientService::temperature_changed(
                            &emitter,
                            temp as f64,
                            confidence as f64,
                        ).await
                    });
                    last_emitted = temp;
                }
            }
        })
        .ok();
}

// We want `fdo` to be visible in scope for any future error wrappers.
// Marking `_` allows us to keep the import even if no method currently
// returns `fdo::Result` — keeping the option open is intentional.
#[allow(dead_code)]
type _FdoMarker = fdo::Error;

/// Wrap an `Arc` for the convenience of callers that want one. Not
/// actually used internally, but exposed so binaries can store the
/// connection without thinking about ownership ceremony.
pub fn into_arc(conn: Connection) -> Arc<Connection> {
    Arc::new(conn)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn haversine_known_pairs() {
        // Paris → London, ~344 km
        let d = haversine_km(48.8566, 2.3522, 51.5074, -0.1278);
        assert!((d - 344.0).abs() < 5.0, "got {d}");

        // Same point → 0
        let d = haversine_km(45.0, -73.0, 45.0, -73.0);
        assert!(d < 0.001);
    }
}

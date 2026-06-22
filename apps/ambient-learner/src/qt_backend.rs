// src/qt_backend.rs
// Qt/QML backend bridge — connects Rust state (WorldState + season) to QML.
//
// Properties fall into four groups:
//   1. Predictions      (temperature, confidence, model_type, ...)
//   2. Sensor health    (sensor_health, sensor_age_seconds, ...)
//   3. Device state     (cpu_load, battery_level, charging)
//   4. Environment      (pressure, humidity, light, tilt, orientation,
//                        proximity, season, day_phase)
//
// All four feed into the QML side, which uses them not only for data
// display but also to drive theme-level effects (gradient direction
// from device tilt, background warmth from ambient light, etc.).

#![cfg(feature = "qt")]

use qmetaobject::*;
use std::ffi::CStr;
use std::sync::{Arc, RwLock};

use crate::seasons::Mood;
use crate::systems::{CalibrationQueue, CalibrationRequest};
use crate::world_state::{self, Location, SystemState, WorldState};

#[derive(QObject, Default)]
pub struct QtBackend {
    base: qt_base_class!(trait QObject),

    // ─── Predictions ─────────────────────────────────────────────────────
    temperature: qt_property!(f32; NOTIFY temperature_changed),
    confidence: qt_property!(f32; NOTIFY confidence_changed),
    model_type: qt_property!(QString; NOTIFY model_type_changed),
    calibration_count: qt_property!(i32; NOTIFY calibration_count_changed),
    linear_calibrations: qt_property!(i32; NOTIFY linear_calibrations_changed),
    neural_calibrations: qt_property!(i32; NOTIFY neural_calibrations_changed),
    pending_calibrations: qt_property!(i32; NOTIFY pending_calibrations_changed),

    // ─── Sensor health ───────────────────────────────────────────────────
    sensor_count: qt_property!(i32; NOTIFY sensor_count_changed),
    sensor_health: qt_property!(QString; NOTIFY sensor_health_changed),
    sensor_age_seconds: qt_property!(i32; NOTIFY sensor_age_seconds_changed),
    sensors_json: qt_property!(QString; NOTIFY sensors_json_changed),

    // ─── Forecast / location ─────────────────────────────────────────────
    location: qt_property!(QString; NOTIFY location_changed),
    location_count: qt_property!(i32; NOTIFY location_count_changed),
    forecast_1h: qt_property!(QString; NOTIFY forecast_1h_changed),
    forecast_24h: qt_property!(QString; NOTIFY forecast_24h_changed),

    // ─── Device state ────────────────────────────────────────────────────
    cpu_load: qt_property!(f32; NOTIFY cpu_load_changed),
    battery_level: qt_property!(f32; NOTIFY battery_level_changed),
    charging: qt_property!(bool; NOTIFY charging_changed),

    // ─── Environment (optional) ─────────────────────────────────────────
    has_pressure: qt_property!(bool; NOTIFY has_pressure_changed),
    pressure_hpa: qt_property!(f32; NOTIFY pressure_hpa_changed),
    pressure_trend_hpa: qt_property!(f32; NOTIFY pressure_trend_hpa_changed),

    has_humidity: qt_property!(bool; NOTIFY has_humidity_changed),
    humidity_pct: qt_property!(f32; NOTIFY humidity_pct_changed),

    has_light: qt_property!(bool; NOTIFY has_light_changed),
    light_level: qt_property!(f32; NOTIFY light_level_changed),
    light_unit: qt_property!(QString; NOTIFY light_unit_changed),
    /// Normalized 0..1 light value useful for gradient strength.
    /// Below 1 lux maps to ~0; bright office light (~500 lux) maps to ~1.
    light_norm: qt_property!(f32; NOTIFY light_norm_changed),

    has_accelerometer: qt_property!(bool; NOTIFY has_accelerometer_changed),
    accel_orientation: qt_property!(QString; NOTIFY accel_orientation_changed),
    accel_tilt: qt_property!(QString; NOTIFY accel_tilt_changed),
    /// X tilt component derived from orientation/tilt strings (–1..1).
    /// Used by QML to bias the temperature card's gradient direction.
    tilt_x: qt_property!(f32; NOTIFY tilt_x_changed),
    tilt_y: qt_property!(f32; NOTIFY tilt_y_changed),

    has_proximity: qt_property!(bool; NOTIFY has_proximity_changed),
    proximity_near: qt_property!(bool; NOTIFY proximity_near_changed),

    // ─── Season / time-of-day ────────────────────────────────────────────
    season: qt_property!(QString; NOTIFY season_changed),
    day_phase: qt_property!(QString; NOTIFY day_phase_changed),

    /// True when `AMBIENT_DEBUG=1` is set in the environment. QML
    /// uses this to gate dev-only pages like the screen-temp grid
    /// inspector. Read once at startup; doesn't change at runtime.
    debug_enabled: qt_property!(bool; NOTIFY debug_enabled_changed),

    // ─── Signals ─────────────────────────────────────────────────────────
    temperature_changed: qt_signal!(),
    confidence_changed: qt_signal!(),
    model_type_changed: qt_signal!(),
    calibration_count_changed: qt_signal!(),
    linear_calibrations_changed: qt_signal!(),
    neural_calibrations_changed: qt_signal!(),
    pending_calibrations_changed: qt_signal!(),

    sensor_count_changed: qt_signal!(),
    sensor_health_changed: qt_signal!(),
    sensor_age_seconds_changed: qt_signal!(),
    sensors_json_changed: qt_signal!(),

    location_changed: qt_signal!(),
    location_count_changed: qt_signal!(),
    forecast_1h_changed: qt_signal!(),
    forecast_24h_changed: qt_signal!(),

    cpu_load_changed: qt_signal!(),
    battery_level_changed: qt_signal!(),
    charging_changed: qt_signal!(),

    has_pressure_changed: qt_signal!(),
    pressure_hpa_changed: qt_signal!(),
    pressure_trend_hpa_changed: qt_signal!(),
    has_humidity_changed: qt_signal!(),
    humidity_pct_changed: qt_signal!(),
    has_light_changed: qt_signal!(),
    light_level_changed: qt_signal!(),
    light_unit_changed: qt_signal!(),
    light_norm_changed: qt_signal!(),
    has_accelerometer_changed: qt_signal!(),
    accel_orientation_changed: qt_signal!(),
    accel_tilt_changed: qt_signal!(),
    tilt_x_changed: qt_signal!(),
    tilt_y_changed: qt_signal!(),
    has_proximity_changed: qt_signal!(),
    proximity_near_changed: qt_signal!(),

    season_changed: qt_signal!(),
    day_phase_changed: qt_signal!(),
    debug_enabled_changed: qt_signal!(),

    // ─── Methods ─────────────────────────────────────────────────────────
    poll_updates: qt_method!(fn poll_updates(&mut self) {
        self.refresh_from_state();
    }),

    calibrate: qt_method!(fn calibrate(&mut self, target_temp: f32) {
        // Default source is "qt-ui" so the audit log clearly
        // attributes user-initiated calibrations to this backend.
        // Callers who want a different attribution should use
        // calibrate_with_source.
        self.enqueue_calibration(target_temp, "qt-ui");
    }),

    calibrate_with_source: qt_method!(fn calibrate_with_source(
        &mut self,
        target_temp: f32,
        source: QString,
    ) {
        let s = source.to_string();
        self.enqueue_calibration(target_temp, &s);
    }),

    get_sensors: qt_method!(fn get_sensors(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);
        serde_json::to_string(&world.sensors)
            .unwrap_or_else(|_| "[]".to_string())
            .into()
    }),

    get_sensor_names: qt_method!(fn get_sensor_names(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);
        serde_json::to_string(&world.sensor_names)
            .unwrap_or_else(|_| "[]".to_string())
            .into()
    }),

    get_temperature_history: qt_method!(fn get_temperature_history(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);

        #[derive(serde::Serialize)]
        struct Pt { t: u64, temp: f32 }

        let now = std::time::Instant::now();
        let pts: Vec<Pt> = world.temperature_history.iter()
            .map(|(t, temp)| Pt {
                t: now.duration_since(*t).as_secs(),
                temp: *temp,
            })
            .collect();
        serde_json::to_string(&pts).unwrap_or_else(|_| "[]".to_string()).into()
    }),

    get_pressure_history: qt_method!(fn get_pressure_history(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);

        #[derive(serde::Serialize)]
        struct Pt { t: u64, p: f32 }

        let now = std::time::Instant::now();
        let pts: Vec<Pt> = world.pressure_history.iter()
            .map(|(t, p)| Pt {
                t: now.duration_since(*t).as_secs(),
                p: *p,
            })
            .collect();
        serde_json::to_string(&pts).unwrap_or_else(|_| "[]".to_string()).into()
    }),

    /// Return recent calibration submissions as JSON. Each entry is
    /// `{ target, mean_sensors, age_seconds, unix_when, source }`.
    ///
    /// `target` is the temperature the calibrator said was correct.
    /// `mean_sensors` is the mean of the sensor readings at that
    /// moment — a coarse proxy for "what the device thought it
    /// was" before this calibration. The two together let the UI
    /// say things like "you said 22.5 °C while the device showed
    /// 24.1 °C; the model adjusted accordingly."
    ///
    /// `source` is the audit-trail attribution: the string the
    /// caller passed via `CalibrationRequest::with_source(...)`.
    /// This is what surfaces "qt-ui" vs "org.example.app" so a
    /// user can tell which app submitted the calibration. Missing
    /// source is rendered as `null` in the JSON; the UI shows
    /// "(unknown)" for those.
    ///
    /// Returned newest-first so the UI doesn't have to reverse.
    get_recent_calibrations: qt_method!(fn get_recent_calibrations(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);

        #[derive(serde::Serialize)]
        struct CalEntry {
            target: f32,
            mean_sensors: f32,
            age_seconds: u64,
            unix_when: i64,
            source: Option<String>,
        }

        let now = std::time::Instant::now();
        // Reverse so newest is first; the in-memory deque grows
        // append-only with oldest at the front.
        let mut entries: Vec<CalEntry> = world
            .calibration_samples
            .iter()
            .rev()
            .map(|s| {
                let mean = if s.sensors.is_empty() {
                    0.0
                } else {
                    let valid: Vec<f32> =
                        s.sensors.iter().copied().filter(|v| v.is_finite()).collect();
                    if valid.is_empty() {
                        0.0
                    } else {
                        valid.iter().sum::<f32>() / valid.len() as f32
                    }
                };
                CalEntry {
                    target: s.target,
                    mean_sensors: mean,
                    age_seconds: now.duration_since(s.when).as_secs(),
                    unix_when: s.unix_when,
                    source: s.source.clone(),
                }
            })
            .collect();

        // Cap the response size at a reasonable number. The deque
        // holds up to 200; UIs typically show 10-20. Returning all
        // 200 wastes bytes; cap at 50 to leave headroom for power
        // users who want to scroll back further.
        entries.truncate(50);

        serde_json::to_string(&entries).unwrap_or_else(|_| "[]".to_string()).into()
    }),

    /// Read the currently-published screen-temperature bitfield and
    /// return it as JSON for the debug page.
    ///
    /// We deliberately read it back through the public `/run` file
    /// rather than reaching into WorldState directly — that way the
    /// debug page also functions as a continuous validation of our
    /// wire format. If the parse fails for any reason (file not yet
    /// written, magic mismatch, body-size mismatch), the error is
    /// surfaced in the returned JSON so the UI can render it
    /// prominently rather than silently showing stale data.
    ///
    /// Returned shape:
    /// ```json
    /// {
    ///   "ok": true,
    ///   "cols": 10, "rows": 16,
    ///   "generation": 42,
    ///   "age_ms": 230,
    ///   "min_c": 19.8, "max_c": 41.2,
    ///   "cells": [21.0, 21.4, ...]    // length = cols * rows
    /// }
    /// ```
    /// On error:
    /// ```json
    /// { "ok": false, "error": "read /run/...: No such file or directory" }
    /// ```
    get_screen_temp_frame: qt_method!(fn get_screen_temp_frame(&self) -> QString {
        match crate::screen_temps::read_current_frame() {
            Ok(frame) => {
                // Min/max for color-scaling. We could compute them in
                // QML but doing it Rust-side keeps the JS simpler and
                // lets us cap them at sane bounds if the frame is
                // empty.
                let (mut min, mut max) = (f32::INFINITY, f32::NEG_INFINITY);
                for &t in frame.temperatures_celsius.iter() {
                    if t.is_finite() {
                        if t < min { min = t; }
                        if t > max { max = t; }
                    }
                }
                if !min.is_finite() { min = 20.0; max = 20.0; }

                let now_ns = std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_nanos() as u64)
                    .unwrap_or(0);
                let age_ms = now_ns.saturating_sub(frame.timestamp_unix_nanos) / 1_000_000;

                #[derive(serde::Serialize)]
                struct Out<'a> {
                    ok: bool,
                    cols: u16,
                    rows: u16,
                    generation: u32,
                    age_ms: u64,
                    min_c: f32,
                    max_c: f32,
                    cells: &'a [f32],
                }
                let out = Out {
                    ok: true,
                    cols: frame.cols,
                    rows: frame.rows,
                    generation: frame.generation,
                    age_ms,
                    min_c: min,
                    max_c: max,
                    cells: &frame.temperatures_celsius,
                };
                serde_json::to_string(&out)
                    .unwrap_or_else(|_| r#"{"ok":false,"error":"serialize failed"}"#.to_string())
                    .into()
            }
            Err(e) => {
                let payload = serde_json::json!({ "ok": false, "error": e });
                payload.to_string().into()
            }
        }
    }),

    get_locations: qt_method!(fn get_locations(&self) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("[]");
        };
        let world = world_state::read(state);

        #[derive(serde::Serialize)]
        struct LocOut<'a> {
            id: &'a str,
            name: &'a str,
            latitude: Option<f64>,
            longitude: Option<f64>,
            current: bool,
        }

        let current_id = world.locations
            .get(world.current_location_idx)
            .map(|l| l.id.as_str())
            .unwrap_or("");

        let out: Vec<LocOut> = world.locations.iter().map(|l| LocOut {
            id: &l.id,
            name: &l.name,
            latitude: l.latitude,
            longitude: l.longitude,
            current: l.id == current_id,
        }).collect();

        serde_json::to_string(&out).unwrap_or_else(|_| "[]".to_string()).into()
    }),

    /// Return every pin published by external apps as a JSON array.
    /// Each element has shape:
    ///   { id, kind, latitude, longitude, title?, description?,
    ///     uri?, payload, source?, submitted_at, expires_at? }
    /// Returns "[]" when the `service` feature is off or no pins
    /// have been submitted yet.
    get_map_pins: qt_method!(fn get_map_pins(&self) -> QString {
        if let Some(provider) = &self.pin_provider {
            return provider().into();
        }
        QString::from("[]")
    }),

    add_location: qt_method!(fn add_location(&mut self, name: QString) -> QString {
        let Some(state) = self.world_state.as_ref() else {
            return QString::from("");
        };
        let name_str = name.to_string();
        if name_str.trim().is_empty() {
            return QString::from("");
        }

        let new_loc = Location::new(name_str);
        let id = new_loc.id.clone();
        let mut world = world_state::write(state);
        world.locations.push(new_loc);
        drop(world);

        self.refresh_from_state();
        id.into()
    }),

    set_current_location: qt_method!(fn set_current_location(&mut self, id: QString) -> bool {
        let Some(state) = self.world_state.as_ref() else { return false };
        let id_str = id.to_string();
        let mut world = world_state::write(state);
        if let Some(idx) = world.locations.iter().position(|l| l.id == id_str) {
            world.current_location_idx = idx;
            world.current_location = world.locations[idx].name.clone();
            drop(world);
            self.refresh_from_state();
            true
        } else {
            false
        }
    }),

    remove_location: qt_method!(fn remove_location(&mut self, id: QString) -> bool {
        let Some(state) = self.world_state.as_ref() else { return false };
        let id_str = id.to_string();
        let mut world = world_state::write(state);
        let Some(idx) = world.locations.iter().position(|l| l.id == id_str) else {
            return false;
        };
        world.locations.remove(idx);
        if world.current_location_idx >= world.locations.len() {
            world.current_location_idx = world.locations.len().saturating_sub(1);
        }
        if let Some(loc) = world.locations.get(world.current_location_idx) {
            world.current_location = loc.name.clone();
        } else {
            world.current_location = "Home".to_string();
        }
        drop(world);
        self.refresh_from_state();
        true
    }),

    format_age: qt_method!(fn format_age(&self, seconds: i32) -> QString {
        let s = seconds.max(0) as u64;
        let out = if s < 1     { "just now".to_string() }
            else if s < 60     { format!("{}s ago", s) }
            else if s < 3600   { format!("{}m ago", s / 60) }
            else               { format!("{}h ago", s / 3600) };
        out.into()
    }),

    // ─── Internal (not exposed to QML) ───────────────────────────────────
    world_state: Option<WorldState>,
    calibration_queue: Option<CalibrationQueue>,
    /// Optional callback that returns the current set of map pins as
    /// JSON. Lets us add map-pin support behind the `service` feature
    /// without making this struct depend on feature-gated types.
    pin_provider: Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>>,

    /// Most recent `last_sensor_update` we observed in WorldState. We
    /// compare against this in `refresh_from_state` to skip the entire
    /// snapshot+emit pass when sensors haven't ticked. Saves ~200µs
    /// of allocations per QML poll on a 4-sensor system.
    last_sensor_seen: Option<std::time::Instant>,
    last_prediction_seen: Option<std::time::Instant>,
}

impl QtBackend {
    /// Attach a shared WorldState and the calibration queue.
    /// Call once before exec().
    /// Attach a shared WorldState and the calibration queue. Optional
    /// `pin_provider` is a callback returning the current map pins as
    /// JSON; pass `None` on builds without the `service` feature.
    pub fn attach(
        &mut self,
        state: WorldState,
        queue: CalibrationQueue,
        pin_provider: Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>>,
    ) {
        self.world_state = Some(state);
        self.calibration_queue = Some(queue);
        self.pin_provider = pin_provider;

        // Read AMBIENT_DEBUG once at attach time. The env var enables
        // debug pages in the QML UI. Truthy values: "1", "true", "yes"
        // (case-insensitive). Anything else, including unset, is false.
        let debug = std::env::var("AMBIENT_DEBUG")
            .ok()
            .map(|v| {
                let lower = v.to_ascii_lowercase();
                matches!(lower.as_str(), "1" | "true" | "yes" | "on")
            })
            .unwrap_or(false);
        self.debug_enabled = debug;
        self.debug_enabled_changed();
        if debug {
            println!("🛠  AMBIENT_DEBUG=1: debug pages enabled in QML UI");
        }

        self.refresh_from_state();
    }

    /// Pull current values out of WorldState into Qt properties.
    fn refresh_from_state(&mut self) {
        // Fast path: if neither the sensor poll nor the model worker
        // has updated WorldState since we last looked, there's nothing
        // for us to do. This skips the snapshot + signal pass entirely
        // on idle ticks, which is most of them — the QML timer fires
        // every 200ms but the sensor system writes every 100ms and the
        // model system every 200ms, so most polls will see at least
        // one update. The savings are real on systems where sensors
        // are slow or absent.
        let (sensor_inst, prediction_inst, force_emit) = match &self.world_state {
            Some(s) => {
                let g = world_state::read(s);
                let force = self.last_sensor_seen.is_none();
                (g.last_sensor_update, g.last_prediction_update, force)
            }
            None => return,
        };
        if !force_emit
            && self.last_sensor_seen == Some(sensor_inst)
            && self.last_prediction_seen == Some(prediction_inst)
        {
            // Nothing changed since last refresh. Still update the
            // pending-calibration count because that flips even when
            // sensors are quiet (someone tapped Calibrate).
            let pending = self
                .calibration_queue
                .as_ref()
                .map(|q| q.len() as i32)
                .unwrap_or(0);
            if self.pending_calibrations != pending {
                self.pending_calibrations = pending;
                self.pending_calibrations_changed();
            }
            return;
        }
        self.last_sensor_seen = Some(sensor_inst);
        self.last_prediction_seen = Some(prediction_inst);

        let snap = match &self.world_state {
            Some(s) => snapshot(&world_state::read(s)),
            None => return,
        };

        let pending = self.calibration_queue.as_ref().map(|q| q.len() as i32).unwrap_or(0);
        let mood = Mood::now_local();

        self.apply_snapshot(snap, pending, mood);
    }

    /// Apply a snapshot and emit signals **only for fields that changed**.
    ///
    /// Why this matters: QML `Connections { onSomethingChanged: ... }`
    /// handlers fire on every signal, regardless of whether the value
    /// is different. Emitting unconditionally causes the Live page's
    /// "pop on temperature change" animation to retrigger 5 times a
    /// second from the polling timer, which looks like the number is
    /// bobbing nervously.
    fn apply_snapshot(&mut self, s: Snapshot, pending: i32, mood: Mood) {
        // For floats we use an epsilon to avoid retriggering on
        // microscopic noise (e.g. the model output drifting by 0.001°C
        // between predictions).
        macro_rules! set_f32 {
            ($field:ident, $val:expr, $notify:ident, $eps:expr) => {{
                if (self.$field - $val).abs() > $eps {
                    self.$field = $val;
                    self.$notify();
                }
            }};
        }
        macro_rules! set_eq {
            ($field:ident, $val:expr, $notify:ident) => {{
                let v = $val;
                if self.$field != v {
                    self.$field = v;
                    self.$notify();
                }
            }};
        }
        macro_rules! set_qstring {
            ($field:ident, $val:expr, $notify:ident) => {{
                let new_q: QString = $val.into();
                if self.$field.to_string() != new_q.to_string() {
                    self.$field = new_q;
                    self.$notify();
                }
            }};
        }

        // Predictions — temperature uses a 0.05°C threshold, confidence
        // uses 1% — these are the values driving animations, so we
        // want the threshold to be larger than typical noise.
        set_f32!(temperature, s.temperature, temperature_changed, 0.05);
        set_f32!(confidence, s.confidence, confidence_changed, 0.01);
        set_qstring!(model_type, s.model_type, model_type_changed);
        set_eq!(calibration_count, s.calibration_count, calibration_count_changed);
        set_eq!(linear_calibrations, s.linear_calibrations, linear_calibrations_changed);
        set_eq!(neural_calibrations, s.neural_calibrations, neural_calibrations_changed);
        set_eq!(pending_calibrations, pending, pending_calibrations_changed);

        // Sensor health
        set_eq!(sensor_count, s.sensor_count, sensor_count_changed);
        set_qstring!(sensor_health, s.sensor_health, sensor_health_changed);
        set_eq!(sensor_age_seconds, s.sensor_age_seconds, sensor_age_seconds_changed);
        set_qstring!(sensors_json, s.sensors_json, sensors_json_changed);

        // Location / forecast
        set_qstring!(location, s.location, location_changed);
        set_eq!(location_count, s.location_count, location_count_changed);
        set_qstring!(forecast_1h, s.forecast_1h, forecast_1h_changed);
        set_qstring!(forecast_24h, s.forecast_24h, forecast_24h_changed);

        // Device — load fluctuates often; treat anything within 1% as same
        set_f32!(cpu_load, s.cpu_load, cpu_load_changed, 0.01);
        set_f32!(battery_level, s.battery_level, battery_level_changed, 0.005);
        set_eq!(charging, s.charging, charging_changed);

        // Environment
        set_eq!(has_pressure, s.has_pressure, has_pressure_changed);
        set_f32!(pressure_hpa, s.pressure_hpa, pressure_hpa_changed, 0.05);
        set_f32!(pressure_trend_hpa, s.pressure_trend_hpa, pressure_trend_hpa_changed, 0.02);
        set_eq!(has_humidity, s.has_humidity, has_humidity_changed);
        set_f32!(humidity_pct, s.humidity_pct, humidity_pct_changed, 0.5);
        set_eq!(has_light, s.has_light, has_light_changed);
        set_f32!(light_level, s.light_level, light_level_changed, 0.5);
        set_qstring!(light_unit, s.light_unit, light_unit_changed);
        set_f32!(light_norm, s.light_norm, light_norm_changed, 0.01);
        set_eq!(has_accelerometer, s.has_accelerometer, has_accelerometer_changed);
        set_qstring!(accel_orientation, s.accel_orientation, accel_orientation_changed);
        set_qstring!(accel_tilt, s.accel_tilt, accel_tilt_changed);
        set_f32!(tilt_x, s.tilt_x, tilt_x_changed, 0.02);
        set_f32!(tilt_y, s.tilt_y, tilt_y_changed, 0.02);
        set_eq!(has_proximity, s.has_proximity, has_proximity_changed);
        set_eq!(proximity_near, s.proximity_near, proximity_near_changed);

        // Season / time-of-day — these change at most every few hours,
        // so equality comparison is essentially free.
        set_qstring!(season, mood.season.as_str().to_string(), season_changed);
        set_qstring!(day_phase, mood.phase.as_str().to_string(), day_phase_changed);
    }

    fn enqueue_calibration(&mut self, target_temp: f32, source: &str) {
        if !target_temp.is_finite() || !(-50.0..=70.0).contains(&target_temp) {
            eprintln!("⚠️  Refusing calibration with bad target {}°C", target_temp);
            return;
        }
        let Some(queue) = &self.calibration_queue else {
            eprintln!("⚠️  Calibration requested but queue is not attached");
            return;
        };
        queue.push(CalibrationRequest::new(target_temp).with_source(source));
        println!("📥 Queued calibration: target={:.2}°C source={}", target_temp, source);

        self.pending_calibrations = queue.len() as i32;
        self.pending_calibrations_changed();
    }
}

/// Plain-data snapshot taken under the read lock so we can drop the
/// guard before mutating Qt fields.
struct Snapshot {
    temperature: f32,
    confidence: f32,
    model_type: String,
    calibration_count: i32,
    linear_calibrations: i32,
    neural_calibrations: i32,
    sensor_count: i32,
    sensor_health: String,
    sensor_age_seconds: i32,
    location: String,
    location_count: i32,
    sensors_json: String,
    forecast_1h: String,
    forecast_24h: String,
    cpu_load: f32,
    battery_level: f32,
    charging: bool,

    has_pressure: bool,
    pressure_hpa: f32,
    pressure_trend_hpa: f32,
    has_humidity: bool,
    humidity_pct: f32,
    has_light: bool,
    light_level: f32,
    light_unit: String,
    light_norm: f32,
    has_accelerometer: bool,
    accel_orientation: String,
    accel_tilt: String,
    tilt_x: f32,
    tilt_y: f32,
    has_proximity: bool,
    proximity_near: bool,
}

fn snapshot(world: &SystemState) -> Snapshot {
    let env = &world.env;

    let (tilt_x, tilt_y) = orientation_to_tilt_xy(
        env.accel_orientation.as_deref(),
        env.accel_tilt.as_deref(),
    );

    Snapshot {
        temperature: world.ambient_temp,
        confidence: world.confidence,
        model_type: world.model_type.as_str().to_string(),
        calibration_count: i32_saturating(world.calibration_count),
        linear_calibrations: i32_saturating(world.linear_calibrations),
        neural_calibrations: i32_saturating(world.neural_calibrations),
        sensor_count: i32_saturating(world.sensor_count),
        sensor_health: world.sensor_health.as_str().to_string(),
        sensor_age_seconds: world.sensor_age().as_secs().min(i32::MAX as u64) as i32,
        location: world.current_location.clone(),
        location_count: i32_saturating(world.locations.len()),
        sensors_json: serde_json::to_string(&world.sensors).unwrap_or_else(|_| "[]".to_string()),
        forecast_1h: world.forecast_1h.clone(),
        forecast_24h: world.forecast_24h.clone(),
        cpu_load: world.device_state.cpu_load,
        battery_level: world.device_state.battery_level,
        charging: world.device_state.charging,

        has_pressure: env.pressure_hpa.is_some(),
        pressure_hpa: env.pressure_hpa.unwrap_or(0.0),
        pressure_trend_hpa: env.pressure_trend_hpa_per_30m.unwrap_or(0.0),
        has_humidity: env.humidity_pct.is_some(),
        humidity_pct: env.humidity_pct.unwrap_or(0.0),
        has_light: env.ambient_light.is_some(),
        light_level: env.ambient_light.unwrap_or(0.0),
        light_unit: env.ambient_light_unit.clone().unwrap_or_default(),
        light_norm: lux_to_normalized(env.ambient_light, env.ambient_light_unit.as_deref()),
        has_accelerometer: env.accel_orientation.is_some(),
        accel_orientation: env.accel_orientation.clone().unwrap_or_default(),
        accel_tilt: env.accel_tilt.clone().unwrap_or_default(),
        tilt_x,
        tilt_y,
        has_proximity: env.proximity_near.is_some(),
        proximity_near: env.proximity_near.unwrap_or(false),
    }
}

fn i32_saturating(n: usize) -> i32 {
    n.try_into().unwrap_or(i32::MAX)
}

/// Map iio-sensor-proxy's orientation/tilt strings to a 2D tilt vector
/// in the range −1..1, suitable for QML gradient/shadow effects.
fn orientation_to_tilt_xy(orientation: Option<&str>, tilt: Option<&str>) -> (f32, f32) {
    // Prefer tilt when present; fall back to orientation.
    let label = tilt.or(orientation).unwrap_or("");
    match label {
        "normal"     => ( 0.0,  0.0),
        "left-up"    => (-0.7,  0.0),
        "right-up"   => ( 0.7,  0.0),
        "bottom-up"  => ( 0.0,  0.7),
        "top-up"     => ( 0.0, -0.7),
        _            => ( 0.0,  0.0),
    }
}

/// Map raw lux to a perceptual 0..1 for UI brightness purposes.
/// Uses a log curve because human perception of brightness is logarithmic
/// (1 lux → near 0, 10 lux → ~0.3, 100 lux → ~0.6, 1000+ lux → 1.0).
fn lux_to_normalized(value: Option<f32>, unit: Option<&str>) -> f32 {
    let Some(v) = value else { return 0.0 };
    if unit == Some("vendor") {
        // Vendor unit is already 0..100.
        return (v / 100.0).clamp(0.0, 1.0);
    }
    if v <= 0.0 {
        return 0.0;
    }
    let scaled = (v.log10() + 1.0) / 4.0; // 0.1 lux → 0, 1000 lux → 1
    scaled.clamp(0.0, 1.0)
}

/// Run the Qt/QML UI. Blocks until the user closes the window.
pub fn run() -> Result<(), String> {
    use std::cell::RefCell;
    println!("🚀 Starting Qt/QML UI...");

    let world: WorldState = Arc::new(RwLock::new(SystemState::new()));

    let poller = crate::ThermalPoller::new();
    let sensor_names = if poller.sensor_names().is_empty() {
        vec!["thermal_zone0".to_string()]
    } else {
        poller.sensor_names()
    };

    {
        let mut s = world_state::write(&world);
        s.sensor_names = sensor_names.clone();
    }

    let model = Arc::new(std::sync::Mutex::new(crate::Model::new(sensor_names.clone())));
    let neural_field = Arc::new(std::sync::Mutex::new(
        crate::temperature_field::TemperatureField::new(sensor_names.len()),
    ));

    // Set up the async DB writer. We try to open the latest
    // existing database; if there isn't one yet, we skip persistence
    // for this run rather than auto-creating (Qt doesn't have a
    // first-run database-creation flow yet; that's GTK's path).
    //
    // `db_writer` is `Option<DbWriter>` — `None` means "this run
    // won't persist sensor readings to disk." The in-memory rolling
    // history in WorldState still works either way.
    let db_writer = match crate::get_latest_database() {
        Some(path) => match crate::db_writer::DbWriter::spawn(path.clone()) {
            Ok(w) => {
                println!("💾 DB writer spawned ({})", path.display());
                Some(w)
            }
            Err(e) => {
                eprintln!("⚠️  DB writer init failed ({}): readings won't persist", e);
                None
            }
        },
        None => {
            println!("📁 No existing database; readings won't persist this run");
            None
        }
    };

    let (_handles, calibration_queue) =
        crate::systems::spawn_all(world.clone(), model, neural_field, db_writer);

    // Try to spin up the iio-sensor-proxy poller on a background thread.
    // No-op if the `dbus` feature is off or the daemon isn't running.
    spawn_iio_proxy_poller(world.clone());

    // Bring up the public service runtime (D-Bus + XDG inbox) so other
    // apps can submit calibrations / map pins. Held in scope for the
    // lifetime of the Qt event loop. No-op if the `service` feature
    // wasn't compiled in.
    let _service_runtime = spawn_service_runtime(world.clone(), calibration_queue.clone());

    qml_register_type::<QtBackend>(
        CStr::from_bytes_with_nul(b"AmbientLearner\0").unwrap(),
        1, 0,
        CStr::from_bytes_with_nul(b"Backend\0").unwrap(),
    );

    let qml_path = find_qml_path()
        .ok_or_else(|| "Could not locate main.qml. Set AMBIENT_QML_DIR to override.".to_string())?;
    println!("📄 Loading QML from: {}", qml_path.display());

    let backend = std::rc::Rc::new(RefCell::new(QtBackend::default()));

    // If the service runtime is up, plumb its pin store into the
    // backend so QML can call `backend.get_map_pins()`.
    let pin_provider = build_pin_provider(&_service_runtime);
    backend.borrow_mut().attach(world.clone(), calibration_queue, pin_provider);

    let pinned = unsafe { QObjectPinned::new(&backend) };
    let mut engine = QmlEngine::new();
    engine.set_object_property("backend".into(), pinned);

    engine.load_file(qml_path.to_string_lossy().as_ref().into());
    engine.exec();

    println!("👋 Qt UI exited");
    Ok(())
}

/// Spin up the iio-sensor-proxy poller if compiled in.
#[cfg(feature = "dbus")]
fn spawn_iio_proxy_poller(world: WorldState) {
    use std::time::Duration;
    crate::iio_proxy::spawn_poller(Duration::from_secs(2), move |reading| {
        let mut s = world_state::write(&world);
        if reading.has_ambient_light {
            s.env.ambient_light = reading.light_level.map(|v| v as f32);
            s.env.ambient_light_unit = reading.light_level_unit;
        }
        if reading.has_accelerometer {
            s.env.accel_orientation = reading.accel_orientation;
            s.env.accel_tilt = reading.accel_tilt;
        }
        if reading.has_proximity {
            s.env.proximity_near = reading.proximity_near;
        }
        s.flags.ui = true;
    });
}

#[cfg(not(feature = "dbus"))]
fn spawn_iio_proxy_poller(_world: WorldState) {
    // Without D-Bus support compiled in, we skip the proxy entirely.
    // The IIO sysfs poller in the environment system still provides
    // pressure / humidity / aux temp.
}

/// Bring up the public service runtime (D-Bus + XDG inbox) so other
/// apps can submit calibrations / map pins. Held in scope for the
/// lifetime of the Qt event loop. No-op if the `service` feature
/// wasn't compiled in.
#[cfg(feature = "service")]
fn spawn_service_runtime(
    world: WorldState,
    queue: crate::systems::CalibrationQueue,
) -> Option<crate::service::Runtime> {
    // We use async_io::block_on, *not* futures::executor::block_on:
    // zbus internally uses async-io for its socket I/O. Mixing
    // executors at startup tends to deadlock the moment zbus tries to
    // poll a fd reactor that the futures executor doesn't drive.
    let started = async_io::block_on(async move {
        crate::service::Runtime::start(world, queue).await
    });
    match started {
        Ok(rt) => Some(rt),
        Err(e) => {
            eprintln!("⚠️  service runtime failed to start: {e}");
            eprintln!("    (the UI still works; only the public API is unavailable)");
            None
        }
    }
}

#[cfg(not(feature = "service"))]
fn spawn_service_runtime(
    _world: WorldState,
    _queue: crate::systems::CalibrationQueue,
) -> Option<()> {
    None
}

/// Build the pin-provider closure for `QtBackend.attach`.
///
/// On `service` builds, returns a closure that reads from the running
/// `Runtime`'s `PinStore` and serializes the current pins as JSON.
/// On non-service builds it returns `None` so the backend's default
/// "[]" fallback is used.
#[cfg(feature = "service")]
fn build_pin_provider(
    runtime: &Option<crate::service::Runtime>,
) -> Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>> {
    let rt = runtime.as_ref()?;
    let pins = rt.pins.clone();
    Some(std::sync::Arc::new(move || {
        let pins = pins.list();
        serde_json::to_string(&pins).unwrap_or_else(|_| "[]".to_string())
    }))
}

#[cfg(not(feature = "service"))]
fn build_pin_provider(
    _runtime: &Option<()>,
) -> Option<std::sync::Arc<dyn Fn() -> String + Send + Sync>> {
    None
}

/// Locate `main.qml` on disk.
fn find_qml_path() -> Option<std::path::PathBuf> {
    use std::path::PathBuf;

    if let Ok(dir) = std::env::var("AMBIENT_QML_DIR") {
        let p = PathBuf::from(dir).join("main.qml");
        if p.exists() {
            return Some(p);
        }
    }

    for candidate in ["src/ui/main.qml", "ui/main.qml"] {
        let p = PathBuf::from(candidate);
        if p.exists() {
            return Some(p);
        }
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(exe_dir) = exe.parent() {
            for rel in ["qml/main.qml", "../share/ambient-learner/qml/main.qml"] {
                let p = exe_dir.join(rel);
                if p.exists() {
                    return Some(p);
                }
            }
        }
    }

    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn lux_normalization_curve() {
        // Pitch black → 0
        assert_eq!(lux_to_normalized(Some(0.0), Some("lux")), 0.0);
        // Brightly lit → 1
        assert!(lux_to_normalized(Some(10000.0), Some("lux")) > 0.99);
        // Dim room (around 50 lux) sits in the middle of the curve.
        let mid = lux_to_normalized(Some(50.0), Some("lux"));
        assert!(mid > 0.4 && mid < 0.7, "got {}", mid);
    }

    #[test]
    fn lux_vendor_units_are_already_percent() {
        assert_eq!(lux_to_normalized(Some(50.0), Some("vendor")), 0.5);
        assert_eq!(lux_to_normalized(Some(100.0), Some("vendor")), 1.0);
        assert_eq!(lux_to_normalized(Some(150.0), Some("vendor")), 1.0); // clamped
    }

    #[test]
    fn tilt_mapping_handles_known_orientations() {
        assert_eq!(orientation_to_tilt_xy(Some("normal"), None), (0.0, 0.0));
        let (x, _) = orientation_to_tilt_xy(Some("left-up"), None);
        assert!(x < 0.0);
        let (x, _) = orientation_to_tilt_xy(Some("right-up"), None);
        assert!(x > 0.0);
        // Unknown orientations fall back to (0,0)
        assert_eq!(orientation_to_tilt_xy(Some("zoinks"), None), (0.0, 0.0));
    }
}

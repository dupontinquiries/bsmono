// src/iio_proxy.rs
//
// Talk to iio-sensor-proxy over the system D-Bus.
//
// The proxy lives at:
//     bus name:    net.hadess.SensorProxy
//     object path: /net/hadess/SensorProxy
//     interface:   net.hadess.SensorProxy
//
// Properties we read:
//     HasAccelerometer (b)
//     AccelerometerOrientation (s)   "normal" | "left-up" | "right-up"
//                                    | "bottom-up" | "undefined"
//     AccelerometerTilt (s)          (newer versions only)
//     HasAmbientLight (b)
//     LightLevelUnit (s)             "lux" | "vendor"
//     LightLevel (d)
//     HasProximity (b)
//     ProximityNear (b)
//
// Methods we call:
//     ClaimAccelerometer / ReleaseAccelerometer
//     ClaimLight         / ReleaseLight
//     ClaimProximity     / ReleaseProximity
//
// We "claim" sensors at startup and never release them — the OS frees
// them when our process exits. The polite alternative is to claim only
// while the relevant UI page is visible, but the simpler approach is
// fine for an app whose whole purpose is reading environmental data.
//
// This module is feature-gated under `dbus`. When that feature isn't
// enabled, every call returns `None` / does nothing.

#![cfg(feature = "dbus")]

use std::time::Duration;

use zbus::blocking::{Connection, Proxy};

const SERVICE: &str = "net.hadess.SensorProxy";
const PATH: &str = "/net/hadess/SensorProxy";
const IFACE: &str = "net.hadess.SensorProxy";

/// A blocking client that talks to iio-sensor-proxy.
///
/// All methods are best-effort: if the daemon isn't running or the
/// system doesn't have the relevant sensor, you get `None`.
pub struct IioProxyClient {
    conn: Connection,
}

impl IioProxyClient {
    /// Connect to the system bus. Returns an error if D-Bus itself
    /// is unreachable, but *does not* fail just because iio-sensor-proxy
    /// isn't installed — that's discovered later via `is_available()`.
    pub fn new() -> zbus::Result<Self> {
        let conn = Connection::system()?;
        Ok(Self { conn })
    }

    /// Is the iio-sensor-proxy daemon running on the bus?
    pub fn is_available(&self) -> bool {
        self.proxy().is_ok()
    }

    fn proxy(&self) -> zbus::Result<Proxy<'_>> {
        Proxy::new(&self.conn, SERVICE, PATH, IFACE)
    }

    /// Try to claim every supported sensor. Each call is independent;
    /// failures are logged but never bubbled up — we want partial
    /// success ("we got light but not accelerometer") to be normal.
    pub fn claim_all(&self) {
        for method in ["ClaimAccelerometer", "ClaimLight", "ClaimProximity"] {
            self.try_call(method);
        }
    }

    /// Release everything. Callers who care can run this in a Drop impl.
    pub fn release_all(&self) {
        for method in ["ReleaseAccelerometer", "ReleaseLight", "ReleaseProximity"] {
            self.try_call(method);
        }
    }

    fn try_call(&self, method: &str) {
        let Ok(p) = self.proxy() else { return };
        if let Err(e) = p.call_method(method, &()) {
            // Most failures here are "this hardware doesn't have that sensor",
            // which is fine. Log at debug level only.
            log::debug!("iio-sensor-proxy {method} unavailable: {e}");
        }
    }

    /// Read every property in one shot.
    /// Returns `None` if the daemon isn't on the bus.
    pub fn read(&self) -> Option<IioProxyReading> {
        let p = self.proxy().ok()?;

        Some(IioProxyReading {
            has_accelerometer: get_bool(&p, "HasAccelerometer"),
            accel_orientation: get_string(&p, "AccelerometerOrientation"),
            accel_tilt: get_string(&p, "AccelerometerTilt"),
            has_ambient_light: get_bool(&p, "HasAmbientLight"),
            light_level: get_f64(&p, "LightLevel"),
            light_level_unit: get_string(&p, "LightLevelUnit"),
            has_proximity: get_bool(&p, "HasProximity"),
            // `proximity_near` is intentionally `Option<bool>`: a true
            // "we don't know" needs to be distinct from a positive
            // "the sensor reports far away". The bool getter folds
            // both into false; use the option getter instead.
            proximity_near: get_optional_bool(&p, "ProximityNear"),
        })
    }
}

/// One snapshot of every iio-sensor-proxy reading we can extract.
#[derive(Debug, Clone, Default)]
pub struct IioProxyReading {
    pub has_accelerometer: bool,
    pub accel_orientation: Option<String>,
    pub accel_tilt: Option<String>,
    pub has_ambient_light: bool,
    pub light_level: Option<f64>,
    pub light_level_unit: Option<String>,
    pub has_proximity: bool,
    pub proximity_near: Option<bool>,
}

fn get_bool(p: &Proxy<'_>, name: &str) -> bool {
    p.get_property::<bool>(name).unwrap_or(false)
}

fn get_optional_bool(p: &Proxy<'_>, name: &str) -> Option<bool> {
    p.get_property::<bool>(name).ok()
}

fn get_string(p: &Proxy<'_>, name: &str) -> Option<String> {
    p.get_property::<String>(name).ok()
}

fn get_f64(p: &Proxy<'_>, name: &str) -> Option<f64> {
    p.get_property::<f64>(name).ok()
}

/// Convenience: spawn a thread that polls the proxy every `interval`
/// and calls `on_reading` with each new value. Returns immediately.
///
/// The thread exits silently if connecting to the bus fails, so the
/// rest of the app doesn't have to care whether iio-sensor-proxy is
/// actually installed.
pub fn spawn_poller<F>(interval: Duration, on_reading: F)
where
    F: Fn(IioProxyReading) + Send + 'static,
{
    std::thread::Builder::new()
        .name("iio-sensor-proxy".into())
        .spawn(move || {
            let Ok(client) = IioProxyClient::new() else {
                log::info!("iio-sensor-proxy: system bus unreachable, skipping");
                return;
            };
            if !client.is_available() {
                log::info!("iio-sensor-proxy: daemon not on bus, skipping");
                return;
            }
            client.claim_all();

            loop {
                if let Some(reading) = client.read() {
                    on_reading(reading);
                }
                std::thread::sleep(interval);
            }
        })
        .ok();
}

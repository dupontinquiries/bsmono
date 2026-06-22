// src/environment.rs
//
// Read environmental sensors from the Linux kernel:
//   * IIO (`/sys/bus/iio/devices/iio:deviceN/in_<measurement>_input`)
//       - Barometric pressure (kPa, converted to hPa for display)
//       - Relative humidity (%)
//       - Auxiliary temperature (°C)  — separate from the thermal_zone CPU sensors
//   * iio-sensor-proxy via D-Bus (`net.hadess.SensorProxy`):
//       - Ambient light (lux or vendor %)
//       - Accelerometer orientation (string)
//       - Accelerometer tilt (string)
//       - Proximity (bool)
//
// Why two paths:
//   - IIO sysfs is the lowest common denominator. It works on phones
//     (PinePhone, Librem 5), Raspberry Pi, embedded boards.
//   - iio-sensor-proxy is required for accelerometer/light because the
//     raw sysfs interface needs continuous claiming and per-device
//     scaling. The proxy already does that work on the desktop.
//
// All reads are *best-effort*. Missing files / disconnected D-Bus
// just produce `None`. Nothing here panics.

use std::fs;
use std::path::{Path, PathBuf};

/// A single snapshot of every environmental sensor we managed to read.
///
/// `None` means "we didn't find this sensor on this system right now".
/// Callers should treat the absence of a value as "no sensor", not as
/// "the value is zero".
#[derive(Clone, Debug, Default, serde::Serialize, serde::Deserialize)]
pub struct EnvSnapshot {
    /// Barometric pressure in hectopascals (hPa / mbar).
    /// Sea-level reference is ~1013.25 hPa.
    pub pressure_hpa: Option<f32>,

    /// Recent pressure delta over the last ~30 minutes, in hPa.
    /// Positive = rising (clear weather), negative = falling (storm).
    /// Computed by the runtime, not read from a sensor directly.
    pub pressure_trend_hpa_per_30m: Option<f32>,

    /// Relative humidity in percent (0–100).
    pub humidity_pct: Option<f32>,

    /// Auxiliary temperature from a BME280-style sensor, in °C.
    /// Distinct from the device's CPU thermal zones — this is usually
    /// a more honest "ambient" reading when present.
    pub aux_temp_c: Option<f32>,

    /// Ambient light. The unit varies — see `ambient_light_unit`.
    pub ambient_light: Option<f32>,
    pub ambient_light_unit: Option<String>,

    /// Accelerometer orientation: "normal", "left-up", "right-up",
    /// "bottom-up", or "undefined".
    pub accel_orientation: Option<String>,
    /// Accelerometer tilt (similar string set to orientation).
    pub accel_tilt: Option<String>,

    /// Proximity sensor binary state.
    pub proximity_near: Option<bool>,
}

impl EnvSnapshot {
    /// Read every sensor we can find. Best-effort; missing sensors
    /// leave fields as `None`.
    pub fn read_all() -> Self {
        let mut snap = Self::default();
        Self::populate_from_iio_sysfs(&mut snap);
        // iio-sensor-proxy reads happen in a separate task because
        // they're asynchronous and need a long-lived D-Bus connection.
        snap
    }

    fn populate_from_iio_sysfs(snap: &mut Self) {
        let Ok(devices) = enumerate_iio_devices() else { return };

        for dev in devices {
            // Each device may expose any subset of pressure/humidity/temp.
            // We just try every channel and merge into the snapshot.
            if let Some(p) = read_iio_channel(&dev, "in_pressure_input") {
                // IIO units are kilopascal; convert to hPa.
                snap.pressure_hpa = Some(p * 10.0);
            }
            if let Some(h) = read_iio_channel(&dev, "in_humidityrelative_input") {
                // Already in percent.
                snap.humidity_pct = Some(h);
            }
            if let Some(t_milli) = read_iio_channel(&dev, "in_temp_input") {
                // IIO temp is millidegrees Celsius. The aux temp is only
                // useful if it's a BME280-style sensor, not the CPU. We
                // distinguish heuristically: aux temp readings sit in
                // 10–50°C; CPU thermal-zone readings are usually 30–90°C
                // and appear under /sys/class/thermal, not under
                // /sys/bus/iio/devices in the bare sysfs interface.
                snap.aux_temp_c = Some(t_milli / 1000.0);
            }
        }
    }
}

/// Enumerate /sys/bus/iio/devices/iio:deviceN paths.
fn enumerate_iio_devices() -> std::io::Result<Vec<PathBuf>> {
    let base = Path::new("/sys/bus/iio/devices");
    if !base.exists() {
        return Ok(Vec::new());
    }
    let mut out = Vec::new();
    for entry in fs::read_dir(base)? {
        let entry = entry?;
        let name = entry.file_name();
        let Some(name) = name.to_str() else { continue };
        if name.starts_with("iio:device") {
            out.push(entry.path());
        }
    }
    Ok(out)
}

/// Read `<device>/<channel>` and parse it as f32.
/// Returns `None` if the file is absent or unparseable.
fn read_iio_channel(device: &Path, channel: &str) -> Option<f32> {
    let path = device.join(channel);
    let s = fs::read_to_string(&path).ok()?;
    s.trim().parse::<f32>().ok()
}

/// Compute pressure trend from a small history buffer.
///
/// The caller maintains a buffer of (timestamp, pressure) pairs.
/// We return `Some(delta)` only when the buffer spans at least 5 min
/// of data, otherwise `None` (avoids reporting noise as a trend).
pub fn pressure_trend_hpa(history: &[(std::time::Instant, f32)]) -> Option<f32> {
    if history.len() < 4 {
        return None;
    }
    let oldest = history.first()?;
    let newest = history.last()?;
    let span = newest.0.duration_since(oldest.0);
    if span < std::time::Duration::from_secs(5 * 60) {
        return None;
    }

    // Normalize to a "per 30 minutes" delta so the number is intuitive
    // (meteorologists quote "3 hPa/3hr" type figures; this is similar).
    let raw_delta = newest.1 - oldest.1;
    let scale = 30.0 * 60.0 / span.as_secs_f32();
    Some(raw_delta * scale)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::{Duration, Instant};

    #[test]
    fn pressure_trend_needs_enough_span() {
        let now = Instant::now();
        let short = vec![
            (now - Duration::from_secs(10), 1013.0),
            (now,                            1014.0),
        ];
        assert!(pressure_trend_hpa(&short).is_none());
    }

    #[test]
    fn pressure_trend_normalizes_to_30min() {
        let now = Instant::now();
        // 1 hPa rise over 10 minutes → 3 hPa per 30 minutes.
        let hist = vec![
            (now - Duration::from_secs(10 * 60), 1013.0),
            (now - Duration::from_secs( 5 * 60), 1013.5),
            (now - Duration::from_secs( 1 * 60), 1013.9),
            (now,                                1014.0),
        ];
        let trend = pressure_trend_hpa(&hist).expect("should compute");
        assert!((trend - 3.0).abs() < 0.5, "got {}", trend);
    }

    #[test]
    fn read_missing_channel_returns_none() {
        let p = std::path::PathBuf::from("/nonexistent/iio:device0");
        assert!(read_iio_channel(&p, "in_pressure_input").is_none());
    }

    #[test]
    fn snapshot_default_is_all_none() {
        let s = EnvSnapshot::default();
        assert!(s.pressure_hpa.is_none());
        assert!(s.humidity_pct.is_none());
        assert!(s.aux_temp_c.is_none());
        assert!(s.ambient_light.is_none());
        assert!(s.accel_orientation.is_none());
    }

    #[test]
    fn read_all_doesnt_panic() {
        // Whether or not the test machine has IIO sensors, this must
        // never panic.
        let _ = EnvSnapshot::read_all();
    }
}

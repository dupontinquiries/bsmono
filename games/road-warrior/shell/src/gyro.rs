//! Gyro input via iio-sensor-proxy over D-Bus (net.hadess.SensorProxy).
//!
//! Partially verified: this compiles cleanly as part of `shell` and has run,
//! repeatedly, in a sandbox with no D-Bus session — confirming the failure
//! path works as designed (`connection failed → log it → GyroTilt stays at
//! 0.0 → keyboard fallback still drives flips`), not a guess. What's NOT
//! verified: an actual connection to a live iio-sensor-proxy instance, real
//! property values, or whether the X-axis choice below matches how a device
//! is actually held. Before relying on this, run
//! `busctl --system introspect net.hadess.SensorProxy /net/hadess/SensorProxy`
//! on the target device to confirm property names match your iio-sensor-proxy
//! version; the interface has been stable for years but was never checked
//! against a real instance this session.

use std::sync::atomic::{AtomicI32, Ordering};
use std::sync::Arc;
use zbus::dbus_proxy;

#[dbus_proxy(
    interface = "net.hadess.SensorProxy",
    default_service = "net.hadess.SensorProxy",
    default_path = "/net/hadess/SensorProxy"
)]
trait SensorProxy {
    fn claim_accelerometer(&self) -> zbus::Result<()>;
    fn release_accelerometer(&self) -> zbus::Result<()>;

    #[dbus_proxy(property)]
    fn accelerometer_x(&self) -> zbus::Result<f64>;
    #[dbus_proxy(property)]
    fn accelerometer_y(&self) -> zbus::Result<f64>;
    #[dbus_proxy(property)]
    fn has_accelerometer(&self) -> zbus::Result<bool>;
}

/// Tilt shared with the game loop, fixed-point as milli-units so it's Sync
/// without a mutex. Range roughly [-1000, 1000] representing device roll.
#[derive(Clone)]
pub struct GyroTilt(Arc<AtomicI32>);

impl GyroTilt {
    pub fn new() -> Self {
        Self(Arc::new(AtomicI32::new(0)))
    }

    pub fn get(&self) -> f32 {
        self.0.load(Ordering::Relaxed) as f32 / 1000.0
    }

    fn set(&self, value: f32) {
        self.0.store((value.clamp(-1.0, 1.0) * 1000.0) as i32, Ordering::Relaxed);
    }
}

/// Spawns a background task that claims the accelerometer and polls it,
/// writing device roll into `tilt`. Poll-based rather than signal-based
/// because AccelerometerX/Y are plain D-Bus properties, not push signals —
/// the proxy would need PropertiesChanged subscription for push updates,
/// which is a reasonable follow-up if 20Hz polling feels laggy on-device.
pub async fn run(tilt: GyroTilt) -> zbus::Result<()> {
    let connection = zbus::Connection::system().await?;
    let proxy = SensorProxyProxy::new(&connection).await?;

    if !proxy.has_accelerometer().await.unwrap_or(false) {
        eprintln!("gyro: no accelerometer reported by iio-sensor-proxy, tilt input disabled");
        return Ok(());
    }

    proxy.claim_accelerometer().await?;

    loop {
        match proxy.accelerometer_x().await {
            // AccelerometerX is roughly in [-1, 1] g already; this maps
            // device roll directly to flip-control tilt. Sign/axis choice
            // (X vs Y) depends on how the phone is mounted for landscape
            // play — verify orientation on-device and flip if flips go backwards.
            Ok(x) => tilt.set(x as f32),
            Err(e) => eprintln!("gyro: read failed: {e}"),
        }
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
    }
}

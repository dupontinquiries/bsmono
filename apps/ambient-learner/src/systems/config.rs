// src/systems/config.rs
// Centralized configuration for the system worker threads.
//
// Numbers that previously lived as magic literals scattered across
// sensor_system.rs and model_system.rs now live here, where they
// can be tuned in one place and documented.

use std::time::Duration;

/// How often the sensor worker polls thermal zones.
///
/// 100 ms gives a 10 Hz update rate which is more than enough for
/// thermal sensors (which typically update at 1–4 Hz internally) and
/// keeps CPU usage negligible.
pub const SENSOR_POLL_INTERVAL: Duration = Duration::from_millis(100);

/// Tick the sensor sleep loop at this rate while waiting for the
/// next poll. Smaller = more responsive shutdown, but more wakeups.
///
/// **Currently unused.** The sensor loop now sleeps once for the
/// remaining time until the next poll deadline. Kept here as a
/// reference value; future shutdown signaling may want to use it
/// to wake the worker more frequently.
#[allow(dead_code)]
pub const SENSOR_SLEEP_TICK: Duration = Duration::from_millis(10);

/// Recreate the ThermalPoller after this many consecutive failed polls.
/// Helps recover from a sensor disappearing (e.g. ACPI hotplug, suspend/resume).
pub const SENSOR_MAX_FAILURES: u32 = 10;

/// Maximum size of the rolling sensor history buffer.
/// At 10 Hz, 300 samples = 30 seconds of recent readings.
pub const SENSOR_HISTORY_CAPACITY: usize = 300;

/// How often the model worker checks whether predictions need to refresh.
pub const MODEL_TICK: Duration = Duration::from_millis(200);

/// Maximum size of the rolling temperature history buffer.
/// At one prediction per sensor update (10 Hz max, but driven by
/// model dirty flag so usually slower), 1000 samples covers the recent past.
pub const TEMP_HISTORY_CAPACITY: usize = 1000;

/// Minimum calibrations before the neural field is trusted enough to
/// take over from the linear model.
pub const NEURAL_FIELD_THRESHOLD: usize = 5;

/// Confidence value used when the neural field is making a prediction.
/// Once we have a calibration count → confidence mapping, this becomes
/// `f32::min(0.5 + 0.05 * (calibrations as f32), 0.95)` or similar.
pub const NEURAL_FIELD_CONFIDENCE: f32 = 0.85;

/// Confidence value when the linear model has been calibrated at least once.
pub const LINEAR_CALIBRATED_CONFIDENCE: f32 = 0.65;

/// Confidence value when the linear model has zero calibrations
/// (essentially returning the raw sensor average).
pub const LINEAR_UNCALIBRATED_CONFIDENCE: f32 = 0.30;

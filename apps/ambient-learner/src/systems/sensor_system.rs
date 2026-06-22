// src/systems/sensor_system.rs
// Sensor polling worker thread
//
// Responsibilities:
//   - Poll the thermal sensors at a fixed cadence
//   - Update WorldState.sensors / .device_state / .sensor_health
//   - Maintain a rolling history buffer
//   - Recover automatically when the underlying poller starts failing
//   - Mark the model dirty so the model worker picks up new readings

use std::time::{Duration, Instant};
use std::thread;
use std::thread::JoinHandle;

use crate::ThermalPoller;
use crate::db_writer::DbWriter;
use crate::world_state::{self, DeviceState, SensorHealth, WorldState};

use super::config::{
    SENSOR_HISTORY_CAPACITY, SENSOR_MAX_FAILURES, SENSOR_POLL_INTERVAL,
};

/// How often to persist a reading to the database. The sensor
/// system polls every 100 ms; we don't need that many DB writes
/// — disk is happy with one row per minute, the in-memory rolling
/// history in WorldState covers everything finer-grained than
/// that. Sub-sampling here keeps the DB compact (a phone left on
/// for a year accumulates 525K rows at this rate, ~50 MB).
const DB_WRITE_INTERVAL: Duration = Duration::from_secs(60);

/// Spawn the sensor system worker thread.
///
/// The thread name (`"ambient-sensor"`) shows up in panic messages and
/// debugger output, which makes diagnosing thread-related issues much
/// easier than the default unnamed threads.
///
/// If `db_writer` is `Some`, the worker persists one reading per
/// `DB_WRITE_INTERVAL` (default 60s) to SQLite via the async batched
/// writer. If `None`, readings stay in-memory only — useful for
/// tests and `--headless` modes that don't want disk side effects.
pub fn spawn(
    world: WorldState,
    db_writer: Option<DbWriter>,
    shutdown: crate::systems::ShutdownSignal,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("ambient-sensor".to_string())
        .spawn(move || run(world, db_writer, shutdown))
        .expect("failed to spawn sensor thread")
}

fn run(
    world: WorldState,
    db_writer: Option<DbWriter>,
    shutdown: crate::systems::ShutdownSignal,
) {
    println!("🌡️  Sensor system started");

    let mut poller = ThermalPoller::new();
    let mut last_poll = Instant::now() - SENSOR_POLL_INTERVAL;
    let mut consecutive_failures = 0u32;

    // Track when we last persisted a reading. The clock is advanced
    // only on successful persistence — a busy DB writer that drops
    // a row doesn't reset the timer, so the next tick will try
    // again.
    let mut last_db_write: Option<Instant> = None;

    // If the poller has zero sensors, mark health as Unavailable up-front
    // so the UI doesn't have to wait for a failed poll to figure it out.
    if poller.sensor_names().is_empty() {
        let mut state = world_state::write(&world);
        state.sensor_health = SensorHealth::Unavailable;
    }

    loop {
        if shutdown.is_requested() {
            println!("🌡️  Sensor system shutting down");
            return;
        }

        // Sleep precisely until the next poll deadline, then poll.
        //
        // A previous version slept in fixed `SENSOR_SLEEP_TICK` chunks
        // and re-checked the clock — that wakes up 10× per poll just
        // to check elapsed time. The clean version sleeps once for
        // exactly the time remaining, which uses the kernel's high-
        // resolution timer wheel and burns less power on a laptop.
        let elapsed = last_poll.elapsed();
        if elapsed < SENSOR_POLL_INTERVAL {
            thread::sleep(SENSOR_POLL_INTERVAL - elapsed);
            // After waking, fall through to poll. Don't `continue`
            // here — that would reread `elapsed` (now ~0 because of
            // the sleep we just did) and skip the poll entirely.
        }
        last_poll = Instant::now();

        match poller.poll() {
            Some(readings) => {
                let was_unhealthy = consecutive_failures > 0;
                consecutive_failures = 0;

                // We need a few pieces of state for the optional DB
                // write below: the readings themselves, plus the
                // current location id (for the foreign key). We
                // gather them under the write lock so the lock is
                // held only as long as it takes to update the live
                // fields.
                let location_id_for_db = {
                    let mut state = world_state::write(&world);
                    state.sensor_count = readings.len();
                    state.sensor_history.push_back((Instant::now(), readings.clone()));
                    while state.sensor_history.len() > SENSOR_HISTORY_CAPACITY {
                        state.sensor_history.pop_front();
                    }
                    state.sensors = readings.clone();
                    state.device_state = read_device_state();
                    state.last_sensor_update = Instant::now();
                    state.sensor_health = SensorHealth::Ok;
                    state.flags.model = true;

                    // Read the location once so we don't have to
                    // re-acquire the lock for the DB write below.
                    state
                        .locations
                        .get(state.current_location_idx)
                        .map(|l| l.id.clone())
                };

                // Optionally persist. We sub-sample to DB_WRITE_INTERVAL
                // because 100ms polls would generate 36k rows per
                // hour — far more than is useful for a long-term
                // history.
                if let Some(writer) = db_writer.as_ref() {
                    let due = last_db_write
                        .map(|t| t.elapsed() >= DB_WRITE_INTERVAL)
                        .unwrap_or(true);
                    if due {
                        let device = world_state::read(&world).device_state.clone();
                        let reading = crate::SensorReading {
                            time: std::time::SystemTime::now()
                                .duration_since(std::time::UNIX_EPOCH)
                                .map(|d| d.as_secs())
                                .unwrap_or(0)
                                .to_string(),
                            sensors: readings,
                            cpu_load: device.cpu_load,
                            // ALS readings are TODO-pluggable; we
                            // store 0 here rather than pretending
                            // we have data. Downstream readers can
                            // distinguish "no light sensor" from
                            // "light sensor reads 0 lux" via the
                            // separate env channel.
                            als_lux: 0,
                            charging: device.charging,
                            // Screen brightness 0..1 → 0..255 byte.
                            screen: (device.screen_brightness * 255.0)
                                .round()
                                .clamp(0.0, 255.0) as u8,
                        };
                        let ok = writer.submit_reading(
                            &reading,
                            location_id_for_db.as_deref(),
                        );
                        if ok {
                            last_db_write = Some(Instant::now());
                        }
                        // If `ok` is false the queue was full — don't
                        // advance the timer, so the next tick will
                        // try again. This back-pressure pattern means
                        // a DB stall doesn't silently drop readings
                        // forever; we'll resume as soon as the writer
                        // catches up.
                    }
                }

                if was_unhealthy {
                    println!("✅ Sensor poll recovered");
                }
            }
            None => {
                consecutive_failures += 1;

                // Update health: any failure → Degraded; sustained failure
                // means we'll recreate the poller and stay Unavailable
                // until the next success.
                {
                    let mut state = world_state::write(&world);
                    state.sensor_health = if consecutive_failures > SENSOR_MAX_FAILURES {
                        SensorHealth::Unavailable
                    } else {
                        SensorHealth::Degraded
                    };
                }

                if consecutive_failures > SENSOR_MAX_FAILURES {
                    eprintln!(
                        "⚠️  {} consecutive sensor failures, recreating poller",
                        consecutive_failures
                    );
                    poller = ThermalPoller::new();
                    consecutive_failures = 0;
                }
            }
        }
    }
}

/// Read the current device state. Delegates to the canonical
/// implementation in `main.rs` which reads `/proc` and `/sys`.
fn read_device_state() -> DeviceState {
    crate::get_device_state()
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::{Arc, RwLock};
    use std::time::Duration;
    use crate::world_state::SystemState;

    #[test]
    fn test_device_state_in_range() {
        let state = read_device_state();
        assert!((0.0..=1.0).contains(&state.cpu_load));
        assert!((0.0..=1.0).contains(&state.battery_level));
        assert!((0.0..=1.0).contains(&state.screen_brightness));
    }

    #[test]
    fn test_sensor_system_spawn() {
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        let shutdown = crate::systems::ShutdownSignal::new();
        let _handle = spawn(world.clone(), None, shutdown.clone());
        // Don't leave the thread running past the test; request
        // shutdown immediately. The worker will exit within one
        // poll interval.
        shutdown.request();

        // Let the worker run briefly. It will either succeed and mark
        // health Ok, or fail and mark Unavailable / Degraded — but it
        // should not leave health as Unknown after this.
        thread::sleep(Duration::from_millis(500));

        let state = world_state::read(&world);
        assert_ne!(
            state.sensor_health,
            SensorHealth::Unknown,
            "worker should have updated sensor_health after running"
        );
    }
}

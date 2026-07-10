// src/systems/mod.rs
// System worker threads
//
// Each system runs independently and coordinates through `WorldState`:
//   - Sensor system     : polls thermal sensors at a fixed cadence
//   - Model system      : drains the calibration queue, runs ML predictions
//   - Environment system: polls IIO sensors (barometer, humidity, light)
//   - Screen-temp system: reconstructs a 2D screen heat map and publishes it
//                         to $XDG_RUNTIME_DIR for other apps to read

pub mod sensor_system;
pub mod model_system;
pub mod environment_system;
pub mod screen_temp_system;
pub mod calibration_queue;
pub mod config;

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::thread::JoinHandle;
use crate::world_state::WorldState;
use crate::Model;
use crate::temperature_field::TemperatureField;

pub use calibration_queue::{CalibrationQueue, CalibrationRequest};

/// Cooperative-shutdown signal shared with every worker thread.
///
/// Cloning is cheap (it's an `Arc<AtomicBool>` inside). Workers
/// check `is_requested()` once per tick. When `request()` is called
/// — typically from `SystemHandles::drop` — each worker sees the
/// flag on its next loop iteration and exits its run function.
///
/// We deliberately use `Relaxed` ordering: the flag is monotonic
/// (once true, always true), and we don't need any happens-before
/// relationship between the request and the workers' subsequent
/// reads. The cost of a `Relaxed` atomic load per tick is one
/// nanosecond; cost of `Acquire` would be the same here because
/// there's no synchronized data being published with the flag.
#[derive(Clone)]
pub struct ShutdownSignal {
    flag: Arc<AtomicBool>,
}

impl ShutdownSignal {
    pub fn new() -> Self {
        Self { flag: Arc::new(AtomicBool::new(false)) }
    }

    /// Set the flag. Subsequent `is_requested()` calls from any
    /// thread will return `true`.
    pub fn request(&self) {
        self.flag.store(true, Ordering::Relaxed);
    }

    /// Check whether shutdown has been requested. Workers call
    /// this once per loop iteration.
    pub fn is_requested(&self) -> bool {
        self.flag.load(Ordering::Relaxed)
    }
}

impl Default for ShutdownSignal {
    fn default() -> Self { Self::new() }
}

/// Handles for all spawned system threads.
///
/// Holding this struct keeps the workers reachable and the
/// shutdown signal alive. On `Drop`, we set the shutdown signal
/// so workers exit their loops cleanly on their next tick.
///
/// **We do not join the threads on drop.** Joining in `Drop` would
/// block whoever drops us, sometimes for hundreds of milliseconds
/// if a worker is sleeping in a syscall. For our use cases (process
/// exit) the OS reaps abandoned threads cheaply; for "restart the
/// worker pool" use cases (which we don't have yet) the caller
/// should join explicitly via `take_handles()` if they need to
/// wait for completion.
///
/// The screen-temp handle is `Option` because spawning that worker
/// can legitimately fail (no `XDG_RUNTIME_DIR`, read-only sandbox,
/// etc.) and we want the rest of the app to keep working when it
/// does. The other systems are pure in-process and don't fail.
pub struct SystemHandles {
    pub sensor: Option<JoinHandle<()>>,
    pub model: Option<JoinHandle<()>>,
    pub environment: Option<JoinHandle<()>>,
    pub screen_temp: Option<JoinHandle<()>>,
    /// The shutdown signal. Public so external code can request a
    /// shutdown without dropping the whole handle struct (useful
    /// during graceful-exit handlers).
    pub shutdown: ShutdownSignal,
}

impl SystemHandles {
    /// Request shutdown and return the join handles. Caller can
    /// `join()` them to wait for clean exit.
    ///
    /// Use this when you want to wait for the workers to finish
    /// (graceful-exit handler, test teardown). Use plain `drop()`
    /// when you don't care to wait (typical process exit).
    pub fn shutdown_and_join(mut self) {
        self.shutdown.request();
        // take() the handles out of their Options — can't move fields out of a Drop type directly.
        for h in [self.sensor.take(), self.model.take(), self.environment.take(), self.screen_temp.take()] {
            if let Some(h) = h {
                let _ = h.join();
            }
        }
    }
}

impl Drop for SystemHandles {
    fn drop(&mut self) {
        // Cooperative: just flip the flag. Workers will notice
        // within one tick (≤ 200ms for any of them) and exit.
        // We do NOT join here — that would block the dropper,
        // possibly indefinitely if a worker is in a slow syscall.
        // Use `shutdown_and_join` if you need to wait.
        self.shutdown.request();
    }
}

/// Spawn all worker threads and return their handles plus the shared
/// `CalibrationQueue` the UI uses to feed calibrations to the model
/// worker.
///
/// The optional `db_writer` is plumbed into the sensor worker so
/// readings get persisted to SQLite asynchronously. It's optional
/// for two reasons:
///   * Tests spawn workers without a writer (no DB to write to).
///   * Some CLI modes (`--headless` for export-only) skip DB
///     creation entirely.
/// When `None`, the sensor worker still runs — it just doesn't
/// persist. The in-memory rolling history in WorldState is the
/// authoritative recent record either way.
pub fn spawn_all(
    world: WorldState,
    linear_model: Arc<Mutex<Model>>,
    neural_field: Arc<Mutex<TemperatureField>>,
    db_writer: Option<crate::db_writer::DbWriter>,
) -> (SystemHandles, CalibrationQueue) {
    println!("🚀 Spawning all system workers...");

    let shutdown = ShutdownSignal::new();
    let calibration_queue = CalibrationQueue::new();

    let sensor = sensor_system::spawn(
        world.clone(), db_writer, shutdown.clone(),
    );
    let model = model_system::spawn(
        world.clone(),
        linear_model,
        neural_field,
        calibration_queue.clone(),
        shutdown.clone(),
    );
    let environment = environment_system::spawn(world.clone(), shutdown.clone());
    let screen_temp = screen_temp_system::spawn(world.clone(), shutdown.clone());

    println!("✅ All systems spawned");

    (
        SystemHandles { sensor: Some(sensor), model: Some(model), environment: Some(environment), screen_temp, shutdown },
        calibration_queue,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn shutdown_signal_starts_unset() {
        let s = ShutdownSignal::new();
        assert!(!s.is_requested());
    }

    #[test]
    fn shutdown_signal_propagates_across_clones() {
        // The signal is shared via Arc, so requesting on one
        // clone must be visible on another. This is the load-
        // bearing property: workers each hold their own clone
        // and the requester (typically SystemHandles::drop) has
        // yet another.
        let a = ShutdownSignal::new();
        let b = a.clone();
        assert!(!b.is_requested());
        a.request();
        assert!(b.is_requested());
    }

    #[test]
    fn shutdown_signal_request_is_monotonic() {
        // No `clear()` exists. Once shutdown is requested it
        // stays requested. This matches what workers expect: if
        // they see `true` once they exit, full stop; they don't
        // need to re-check whether the signal got un-requested.
        let s = ShutdownSignal::new();
        s.request();
        assert!(s.is_requested());
        // Second request is a no-op.
        s.request();
        assert!(s.is_requested());
    }

    #[test]
    fn shutdown_signal_is_default_constructable() {
        // The `Default` impl matters because `spawn_all` and
        // tests both want "no work to set up" construction.
        let s: ShutdownSignal = Default::default();
        assert!(!s.is_requested());
    }
}

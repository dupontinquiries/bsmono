// src/systems/environment_system.rs
//
// Poll the environmental IIO sensors (pressure, humidity, aux temp)
// at a slow cadence and update WorldState.env.
//
// Why slow:
//   - Pressure / humidity don't change quickly. Polling once every
//     30 seconds is plenty.
//   - These reads are file-system reads under /sys/bus/iio, which
//     are essentially free, but most barometers are oversampled and
//     responding to a "fast" poll is wasted effort.
//
// We also maintain `pressure_history` (a 60-minute rolling buffer)
// and recompute the "trend" field every cycle so the UI can color
// the forecast card based on rising/falling pressure.

use std::thread;
use std::thread::JoinHandle;
use std::time::{Duration, Instant};

use crate::environment::{pressure_trend_hpa, EnvSnapshot};
use crate::world_state::{self, WorldState};

const POLL_INTERVAL: Duration = Duration::from_secs(30);
const PRESSURE_HISTORY_CAPACITY: usize = 120;
const PRESSURE_HISTORY_MAX_AGE: Duration = Duration::from_secs(60 * 60);

/// Spawn the environment sensor poller.
pub fn spawn(
    world: WorldState,
    shutdown: crate::systems::ShutdownSignal,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("ambient-env".to_string())
        .spawn(move || run(world, shutdown))
        .expect("failed to spawn environment thread")
}

fn run(world: WorldState, shutdown: crate::systems::ShutdownSignal) {
    println!("🌍 Environment system started");

    loop {
        if shutdown.is_requested() {
            println!("🌍 Environment system shutting down");
            return;
        }
        let snap = EnvSnapshot::read_all();
        update_world(&world, snap);

        // POLL_INTERVAL is long (~30s). Splitting the sleep into
        // short chunks lets shutdown surface within ~1 second
        // instead of up to 30. The poll cadence is unchanged
        // because we still wait the full POLL_INTERVAL of total
        // sleep before the next read.
        let chunk = std::time::Duration::from_secs(1);
        let mut remaining = POLL_INTERVAL;
        while remaining > std::time::Duration::ZERO {
            if shutdown.is_requested() {
                return;
            }
            let this = remaining.min(chunk);
            thread::sleep(this);
            remaining = remaining.saturating_sub(this);
        }
    }
}

fn update_world(world: &WorldState, mut snap: EnvSnapshot) {
    let now = Instant::now();
    let mut state = world_state::write(world);

    // Maintain the rolling pressure history.
    if let Some(p) = snap.pressure_hpa {
        state.pressure_history.push_back((now, p));
        // Drop entries older than the configured max age.
        while let Some((t, _)) = state.pressure_history.front() {
            if now.duration_since(*t) > PRESSURE_HISTORY_MAX_AGE {
                state.pressure_history.pop_front();
            } else {
                break;
            }
        }
        while state.pressure_history.len() > PRESSURE_HISTORY_CAPACITY {
            state.pressure_history.pop_front();
        }
    }

    // Compute the trend from the (now-updated) history.
    let history: Vec<(Instant, f32)> = state.pressure_history.iter().copied().collect();
    snap.pressure_trend_hpa_per_30m = pressure_trend_hpa(&history);

    state.env = snap;
    state.flags.ui = true;
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_state::SystemState;
    use std::sync::{Arc, RwLock};

    #[test]
    fn update_world_preserves_history() {
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));

        // First reading: no trend yet (need 5 minutes of data).
        let snap1 = EnvSnapshot {
            pressure_hpa: Some(1013.0),
            ..Default::default()
        };
        update_world(&world, snap1);
        {
            let s = world.read().unwrap();
            assert_eq!(s.pressure_history.len(), 1);
            assert!(s.env.pressure_trend_hpa_per_30m.is_none());
        }

        // Second reading: history has 2 entries.
        let snap2 = EnvSnapshot {
            pressure_hpa: Some(1013.5),
            ..Default::default()
        };
        update_world(&world, snap2);
        {
            let s = world.read().unwrap();
            assert_eq!(s.pressure_history.len(), 2);
        }
    }
}

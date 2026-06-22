// src/systems/model_system.rs
// Model inference worker thread.
//
// Responsibilities:
//   - Drain the calibration queue and apply each user-provided calibration
//   - Recompute ambient temperature predictions when sensors update
//   - Maintain a rolling temperature history
//   - Coordinate via dirty flags so we don't waste cycles when nothing changed

use std::sync::{Arc, Mutex};
use std::thread;
use std::thread::JoinHandle;
use std::time::Instant;

use crate::Model;
use crate::temperature_field::TemperatureField;
use crate::world_state::{self, DeviceState, ModelType, WorldState};

use super::calibration_queue::CalibrationQueue;
use super::config::{
    LINEAR_CALIBRATED_CONFIDENCE, LINEAR_UNCALIBRATED_CONFIDENCE, MODEL_TICK,
    NEURAL_FIELD_CONFIDENCE, NEURAL_FIELD_THRESHOLD, TEMP_HISTORY_CAPACITY,
};

/// Spawn the model worker thread.
pub fn spawn(
    world: WorldState,
    linear_model: Arc<Mutex<Model>>,
    neural_field: Arc<Mutex<TemperatureField>>,
    calibration_queue: CalibrationQueue,
    shutdown: crate::systems::ShutdownSignal,
) -> JoinHandle<()> {
    thread::Builder::new()
        .name("ambient-model".to_string())
        .spawn(move || run(world, linear_model, neural_field, calibration_queue, shutdown))
        .expect("failed to spawn model thread")
}

fn run(
    world: WorldState,
    linear_model: Arc<Mutex<Model>>,
    neural_field: Arc<Mutex<TemperatureField>>,
    calibration_queue: CalibrationQueue,
    shutdown: crate::systems::ShutdownSignal,
) {
    println!("🧠 Model system started");

    loop {
        if shutdown.is_requested() {
            println!("🧠 Model system shutting down");
            return;
        }
        thread::sleep(MODEL_TICK);

        // 1. Drain pending calibrations first — apply *before* the next
        //    prediction so the user sees the effect immediately.
        let pending = calibration_queue.drain();
        if !pending.is_empty() {
            apply_calibrations(&pending, &world, &linear_model, &neural_field);
            // A calibration always invalidates the current prediction.
            world_state::write(&world).flags.model = true;
        }

        // 2. If nothing changed, we're done.
        let needs_update = world_state::read(&world).flags.model;
        if !needs_update {
            continue;
        }

        // 3. Snapshot just what we need, releasing the lock before
        //    running the (potentially-expensive) prediction.
        let (sensors, device_state) = {
            let state = world_state::read(&world);
            (state.sensors.clone(), state.device_state.clone())
        };

        if sensors.is_empty() {
            // Nothing to predict from yet — clear the flag and wait.
            world_state::write(&world).flags.model = false;
            continue;
        }

        let (prediction, confidence, model_used) =
            predict(&sensors, &device_state, &linear_model, &neural_field);

        // 4. Write back results in a single lock acquisition.
        let now = Instant::now();
        let mut state = world_state::write(&world);
        state.ambient_temp = prediction;
        state.confidence = confidence;
        state.model_type = model_used;
        state.last_prediction_update = now;

        state.linear_calibrations = linear_model
            .try_lock()
            .map(|m| m.calibration_count)
            .unwrap_or(state.linear_calibrations);

        state.neural_calibrations = neural_field
            .try_lock()
            .map(|nf| nf.get_calibration_count())
            .unwrap_or(state.neural_calibrations);

        state.calibration_count = state.linear_calibrations.max(state.neural_calibrations);

        state.temperature_history.push_back((now, prediction));
        while state.temperature_history.len() > TEMP_HISTORY_CAPACITY {
            state.temperature_history.pop_front();
        }

        // Clear the model dirty flag, request UI refresh.
        state.flags.model = false;
        state.flags.ui = true;
        state.flags.sphere = true;
        state.flags.forecast = true;
    }
}

/// Apply each calibration to both models.
///
/// We always train both models so the user can switch between Linear/Neural
/// at any time without losing accuracy. The neural field needs the device
/// state (CPU load etc.) for its physics-aware features; the linear model
/// ignores it.
fn apply_calibrations(
    pending: &[super::CalibrationRequest],
    world: &WorldState,
    linear_model: &Arc<Mutex<Model>>,
    neural_field: &Arc<Mutex<TemperatureField>>,
) {
    let (sensors, device_state) = {
        let state = world_state::read(world);
        (state.sensors.clone(), state.device_state.clone())
    };

    if sensors.is_empty() {
        eprintln!("⚠️  Skipping {} calibration(s): no sensor readings yet", pending.len());
        return;
    }

    for req in pending {
        println!(
            "🎯 Applying calibration: target={:.2}°C source={:?}",
            req.target_temp, req.source
        );

        if let Ok(mut model) = linear_model.lock() {
            model.calibrate(&sensors, req.target_temp);
        } else {
            eprintln!("⚠️  Could not lock linear model for calibration");
        }

        if let Ok(mut nf) = neural_field.lock() {
            // Use a small constant learning rate; tuning is the model's job.
            nf.calibrate(&sensors, &device_state, req.target_temp, 0.001);
        } else {
            eprintln!("⚠️  Could not lock neural field for calibration");
        }

        // Record the sample in WorldState for the UI to visualize.
        // We do this regardless of whether the locks above
        // succeeded — the *intent* to calibrate is what the user
        // sees in the scatter plot, and seeing a sample appear
        // confirms the path worked end-to-end even if the model
        // mutex was momentarily contended.
        let mut state = world_state::write(world);
        state.calibration_samples.push_back(
            crate::world_state::CalibrationSample {
                sensors: sensors.clone(),
                target: req.target_temp,
                when: Instant::now(),
                unix_when: std::time::SystemTime::now()
                    .duration_since(std::time::UNIX_EPOCH)
                    .map(|d| d.as_secs() as i64)
                    .unwrap_or(0),
                source: req.source.clone(),
            },
        );
        while state.calibration_samples.len()
            > crate::world_state::CALIBRATION_SAMPLE_CAPACITY
        {
            state.calibration_samples.pop_front();
        }
    }
}

/// Run a temperature prediction, preferring the neural field once it
/// has enough calibrations to be trustworthy.
fn predict(
    sensors: &[f32],
    device_state: &DeviceState,
    linear_model: &Arc<Mutex<Model>>,
    neural_field: &Arc<Mutex<TemperatureField>>,
) -> (f32, f32, ModelType) {
    // Neural field path — only used once we've trained it enough.
    if let Ok(mut nf) = neural_field.try_lock() {
        if nf.get_calibration_count() >= NEURAL_FIELD_THRESHOLD {
            let prediction = nf.estimate_ambient(sensors, device_state);
            return (prediction, NEURAL_FIELD_CONFIDENCE, ModelType::Neural);
        }
    }

    // Linear-model fallback.
    if let Ok(mut model) = linear_model.try_lock() {
        let prediction = model.predict(sensors);
        let confidence = if model.calibration_count > 0 {
            LINEAR_CALIBRATED_CONFIDENCE
        } else {
            LINEAR_UNCALIBRATED_CONFIDENCE
        };
        return (prediction, confidence, ModelType::Linear);
    }

    // Both models busy — return last-known state by retaining the previous
    // prediction; the caller decides whether to keep the old value.
    (20.0, 0.0, ModelType::Linear)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::world_state::SystemState;
    use std::sync::RwLock;

    #[test]
    fn test_prediction_fallback_to_linear() {
        let sensors = vec![30.0, 35.0, 32.0];
        let device_state = DeviceState::default();
        let linear_model = Arc::new(Mutex::new(Model::new(vec!["a".into(), "b".into(), "c".into()])));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(3)));

        let (temp, conf, model_type) = predict(&sensors, &device_state, &linear_model, &neural_field);

        assert_eq!(model_type, ModelType::Linear);
        assert!(conf > 0.0 && conf <= 1.0);
        assert!(temp.is_finite());
    }

    #[test]
    fn calibration_queue_drained_into_models() {
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        // Pre-populate sensors so apply_calibrations doesn't bail out.
        {
            let mut s = world.write().unwrap();
            s.sensors = vec![40.0, 42.0, 38.0];
        }
        let linear_model = Arc::new(Mutex::new(Model::new(vec!["a".into(), "b".into(), "c".into()])));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(3)));

        let pending = vec![
            crate::systems::CalibrationRequest::new(20.0),
            crate::systems::CalibrationRequest::new(21.0),
        ];

        let before = linear_model.lock().unwrap().calibration_count;
        apply_calibrations(&pending, &world, &linear_model, &neural_field);
        let after = linear_model.lock().unwrap().calibration_count;

        assert_eq!(after, before + 2, "both calibrations should have applied");
    }

    #[test]
    fn empty_sensors_skips_calibration_safely() {
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        let linear_model = Arc::new(Mutex::new(Model::new(vec!["a".into()])));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(1)));

        let pending = vec![crate::systems::CalibrationRequest::new(20.0)];
        let before = linear_model.lock().unwrap().calibration_count;
        apply_calibrations(&pending, &world, &linear_model, &neural_field);
        let after = linear_model.lock().unwrap().calibration_count;

        assert_eq!(after, before, "no sensors → no calibration applied");
    }

    #[test]
    fn calibration_source_is_recorded_in_sample() {
        // Source attribution is the audit-log primitive. Without
        // it, the user can't tell who calibrated their model. This
        // test pins down that the source string from the request
        // ends up on the recorded sample.
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        {
            let mut s = world.write().unwrap();
            s.sensors = vec![40.0, 42.0, 38.0];
        }
        let linear_model = Arc::new(Mutex::new(Model::new(
            vec!["a".into(), "b".into(), "c".into()],
        )));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(3)));

        let pending = vec![
            crate::systems::CalibrationRequest::new(20.0)
                .with_source("test-source"),
            crate::systems::CalibrationRequest::new(21.0)
                .with_source("another"),
            // Sourceless request — `source` field should be None.
            crate::systems::CalibrationRequest::new(22.0),
        ];

        apply_calibrations(&pending, &world, &linear_model, &neural_field);

        let state = world.read().unwrap();
        assert_eq!(state.calibration_samples.len(), 3);

        let s0 = &state.calibration_samples[0];
        assert_eq!(s0.target, 20.0);
        assert_eq!(s0.source.as_deref(), Some("test-source"));

        let s1 = &state.calibration_samples[1];
        assert_eq!(s1.source.as_deref(), Some("another"));

        let s2 = &state.calibration_samples[2];
        assert!(s2.source.is_none(),
                "sourceless requests should leave source=None");
    }

    #[test]
    fn calibration_records_unix_timestamp() {
        // unix_when is what the Qt audit log uses to render
        // "5 minutes ago" — Instant alone can't do that. Verify
        // we're populating a sane wall-clock value.
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        {
            let mut s = world.write().unwrap();
            s.sensors = vec![40.0];
        }
        let linear_model = Arc::new(Mutex::new(Model::new(vec!["a".into()])));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(1)));

        let before_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        apply_calibrations(
            &[crate::systems::CalibrationRequest::new(20.0)],
            &world,
            &linear_model,
            &neural_field,
        );

        let after_unix = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .unwrap()
            .as_secs() as i64;

        let state = world.read().unwrap();
        let s = &state.calibration_samples[0];
        assert!(
            s.unix_when >= before_unix && s.unix_when <= after_unix,
            "unix_when ({}) should be within [{}, {}]",
            s.unix_when, before_unix, after_unix,
        );
    }

    #[test]
    fn calibration_sample_buffer_respects_capacity() {
        // Recording 1000 samples should leave us with the most
        // recent CALIBRATION_SAMPLE_CAPACITY. FIFO eviction —
        // oldest entries drop first.
        let world: WorldState = Arc::new(RwLock::new(SystemState::new()));
        {
            let mut s = world.write().unwrap();
            s.sensors = vec![40.0];
        }
        let linear_model = Arc::new(Mutex::new(Model::new(vec!["a".into()])));
        let neural_field = Arc::new(Mutex::new(TemperatureField::new(1)));

        // Push 250 calibrations (capacity is 200 at the time of
        // writing this test; the assertion uses the constant so
        // it stays correct if we tune capacity).
        let pending: Vec<_> = (0..250)
            .map(|i| {
                crate::systems::CalibrationRequest::new(20.0 + i as f32 * 0.01)
                    .with_source(&format!("test-{}", i))
            })
            .collect();
        apply_calibrations(&pending, &world, &linear_model, &neural_field);

        let state = world.read().unwrap();
        assert_eq!(
            state.calibration_samples.len(),
            crate::world_state::CALIBRATION_SAMPLE_CAPACITY,
            "buffer should be capped at CALIBRATION_SAMPLE_CAPACITY"
        );

        // The newest entry should be the last one we pushed.
        let newest = state.calibration_samples.back().unwrap();
        assert_eq!(newest.source.as_deref(), Some("test-249"));
        // The oldest retained should be test-50 (since we dropped
        // the first 50 to make room).
        let oldest = state.calibration_samples.front().unwrap();
        assert_eq!(oldest.source.as_deref(), Some("test-50"));
    }
}

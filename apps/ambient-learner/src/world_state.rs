// src/world_state.rs
// Central application state - single source of truth
// This pattern is used in robotics/autonomous systems for managing
// continuous sensor streams + ML inference + UI updates

use std::sync::{Arc, RwLock, RwLockReadGuard, RwLockWriteGuard};
use std::collections::VecDeque;
use std::time::Instant;

/// Health of the sensor subsystem at a glance.
///
/// Surfaced to the UI so the user can tell why predictions might be
/// stale or missing.
#[derive(Clone, Copy, PartialEq, Debug, Default)]
pub enum SensorHealth {
    /// We haven't tried to poll yet.
    #[default]
    Unknown,
    /// Last poll succeeded — sensors are working.
    Ok,
    /// We tried but couldn't open / read the sensors.
    /// Usually means another process holds the thermal device, or the
    /// /sys/class/thermal tree is empty (non-Linux, sandboxed, etc.)
    Unavailable,
    /// We were polling fine but something started failing recently.
    /// The sensor worker recreates the poller after several failures.
    Degraded,
}

impl SensorHealth {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Unknown => "unknown",
            Self::Ok => "ok",
            Self::Unavailable => "unavailable",
            Self::Degraded => "degraded",
        }
    }

    pub fn is_ok(&self) -> bool {
        matches!(self, Self::Ok)
    }
}

/// Central application state shared across all systems
/// 
/// Uses RwLock for performance:
/// - Multiple readers (UI, sphere, database) can read concurrently
/// - Single writer (sensor thread) gets exclusive access
/// - Much faster than Mutex for read-heavy workloads
pub struct SystemState {
    // === Sensor Data ===
    pub sensors: Vec<f32>,                    // Current sensor readings
    pub sensor_count: usize,                  // Number of active sensors
    pub sensor_names: Vec<String>,            // Sensor identifiers
    pub sensor_health: SensorHealth,          // Last-poll status
    pub device_state: DeviceState,            // CPU, battery, screen
    pub last_sensor_update: Instant,
    
    // === Predictions ===
    pub ambient_temp: f32,                    // Current prediction
    pub confidence: f32,                      // Model confidence
    pub model_type: ModelType,                // Linear or Neural
    pub last_prediction_update: Instant,
    
    // === Calibration ===
    pub calibration_count: usize,             // Total calibrations
    pub linear_calibrations: usize,
    pub neural_calibrations: usize,
    /// Bounded list of recent calibration *samples* — each entry
    /// is a `(sensor_readings, target_temp, when)` triple. The
    /// linear/neural calibration counts above are aggregates;
    /// this is the raw input data, kept for visualization. The
    /// list is cleared on app start (we don't reload past
    /// calibrations from the DB into this buffer) and grows up to
    /// `CALIBRATION_SAMPLE_CAPACITY` before discarding the
    /// oldest.
    pub calibration_samples: VecDeque<CalibrationSample>,
    
    // === History ===
    pub sensor_history: VecDeque<(Instant, Vec<f32>)>,
    pub temperature_history: VecDeque<(Instant, f32)>,
    
    // === 3D Sphere Heat Field ===
    pub sphere_samples: Vec<SpherePoint>,     // Spatial temperature samples
    pub sphere_center_temp: f32,              // Center (ambient)
    pub sphere_min_temp: f32,                 // For color mapping
    pub sphere_max_temp: f32,                 // For color mapping
    
    // === Locations ===
    pub current_location: String,
    pub locations: Vec<Location>,
    pub current_location_idx: usize,
    
    // === Forecast ===
    pub forecast_1h: String,
    pub forecast_24h: String,

    // === Environment (optional sensors) ===
    /// Latest snapshot of optional environmental sensors (barometer,
    /// humidity, light, accelerometer). All fields are `Option` and
    /// will be `None` on systems without those sensors.
    pub env: crate::environment::EnvSnapshot,
    /// Rolling buffer of (timestamp, pressure_hpa) for trend analysis.
    pub pressure_history: VecDeque<(Instant, f32)>,
    
    // === Dirty Flags (performance optimization) ===
    /// Dirty flags prevent unnecessary computation
    /// - Only recalculate when needed
    /// - Battery-friendly
    /// - Clean coordination between systems
    pub flags: DirtyFlags,
}

/// Dirty flags for change tracking
/// 
/// Each system marks what changed so other systems know what to update.
/// This prevents expensive recomputations when nothing changed.
#[derive(Default, Clone, Copy)]
pub struct DirtyFlags {
    pub sensors: bool,       // Sensor thread updated sensors
    pub model: bool,         // Model needs recalculation
    pub sphere: bool,        // Sphere needs resampling
    pub ui: bool,            // UI needs refresh
    pub forecast: bool,      // Forecast needs update
    pub database: bool,      // Database needs write
}

/// Single point in 3D heat field
#[derive(Clone, Copy, Debug)]
pub struct SpherePoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub temperature: f32,
}

/// Which model is currently being used
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ModelType {
    Linear,
    Neural,
}

impl ModelType {
    pub fn as_str(&self) -> &'static str {
        match self {
            ModelType::Linear => "Linear",
            ModelType::Neural => "Neural",
        }
    }
}

/// Device hardware state
#[derive(Clone, Default, Debug, serde::Serialize, serde::Deserialize)]
pub struct DeviceState {
    pub cpu_load: f32,
    pub battery_level: f32,
    pub charging: bool,
    pub screen_brightness: f32,
}

/// One calibration event recorded by the model worker. We keep a
/// rolling window of these in `SystemState.calibration_samples`
/// for the UI to visualize — for example, the Iced "Chart" tab's
/// scatter plot of (predicted, target) pairs, or the Qt model
/// page's "recent calibrations" card.
///
/// `sensors` is a snapshot of the per-sensor readings at the
/// moment of calibration. `target` is the temperature the user
/// (or upstream caller) said was the correct ambient. `when` is
/// monotonic; do NOT use it for cross-process comparisons —
/// `unix_when` is the wall-clock equivalent for that purpose and
/// for "N minutes ago"-style display.
///
/// `source` is the audit-trail attribution: the string the caller
/// gave for `CalibrationRequest::with_source("...")`. It identifies
/// which UI or external app submitted the calibration. Possible
/// values include `"qt-ui"`, `"iced-ui"`, `"gtk-ui"`, or any
/// arbitrary string from a D-Bus or XDG-inbox caller. This is
/// what lets a user see "an unfamiliar app calibrated my model"
/// and undo it if they want.
#[derive(Clone, Debug)]
pub struct CalibrationSample {
    pub sensors: Vec<f32>,
    pub target: f32,
    pub when: Instant,
    pub unix_when: i64,
    pub source: Option<String>,
}

/// Maximum number of recent calibration samples we keep in the
/// in-memory buffer. 200 is enough to plot a meaningful scatter
/// for the model debugging UI while staying well below the memory
/// footprint where we'd care. Older samples are dropped FIFO.
pub const CALIBRATION_SAMPLE_CAPACITY: usize = 200;

/// Location information
#[derive(Clone, Debug)]
pub struct Location {
    pub id: String,
    pub name: String,
    pub latitude: Option<f64>,
    pub longitude: Option<f64>,
}

impl Location {
    /// Build a new Location with a fresh UUID id and no coordinates.
    pub fn new(name: impl Into<String>) -> Self {
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            name: name.into(),
            latitude: None,
            longitude: None,
        }
    }

    /// Attach coordinates to a Location (builder-style).
    pub fn with_coords(mut self, lat: f64, lon: f64) -> Self {
        self.latitude = Some(lat);
        self.longitude = Some(lon);
        self
    }
}

/// Type alias for the shared world state
pub type WorldState = Arc<RwLock<SystemState>>;

/// Read the world state, recovering automatically from a poisoned lock.
///
/// Lock poisoning means a previous thread panicked while holding the
/// write lock — but the *data* is almost always still consistent
/// (we don't tend to leave half-written invariants between fields),
/// so it's safer to keep going than to crash the whole UI.
pub fn read(state: &WorldState) -> RwLockReadGuard<'_, SystemState> {
    match state.read() {
        Ok(g) => g,
        Err(poisoned) => {
            eprintln!("⚠️  WorldState read lock poisoned; recovering");
            poisoned.into_inner()
        }
    }
}

/// Acquire a write lock on the world state, recovering from poisoning.
pub fn write(state: &WorldState) -> RwLockWriteGuard<'_, SystemState> {
    match state.write() {
        Ok(g) => g,
        Err(poisoned) => {
            eprintln!("⚠️  WorldState write lock poisoned; recovering");
            poisoned.into_inner()
        }
    }
}

impl SystemState {
    /// Create new system state with default values
    pub fn new() -> Self {
        let now = Instant::now();
        Self {
            sensors: Vec::new(),
            sensor_count: 0,
            sensor_names: Vec::new(),
            sensor_health: SensorHealth::Unknown,
            device_state: DeviceState::default(),
            last_sensor_update: now,
            
            ambient_temp: 20.0,
            confidence: 0.0,
            model_type: ModelType::Linear,
            last_prediction_update: now,
            
            calibration_count: 0,
            linear_calibrations: 0,
            neural_calibrations: 0,
            calibration_samples: VecDeque::with_capacity(CALIBRATION_SAMPLE_CAPACITY),
            
            sensor_history: VecDeque::with_capacity(300),
            temperature_history: VecDeque::with_capacity(1000),
            
            sphere_samples: Vec::new(),
            sphere_center_temp: 20.0,
            sphere_min_temp: 20.0,
            sphere_max_temp: 30.0,
            
            current_location: "Home".to_string(),
            locations: Vec::new(),
            current_location_idx: 0,
            
            forecast_1h: String::new(),
            forecast_24h: String::new(),

            env: crate::environment::EnvSnapshot::default(),
            pressure_history: VecDeque::with_capacity(120),
            
            flags: DirtyFlags::default(),
        }
    }
    
    /// Mark all UI elements as needing update
    pub fn mark_ui_dirty(&mut self) {
        self.flags.ui = true;
    }
    
    /// Clear all dirty flags
    pub fn clear_all_dirty(&mut self) {
        self.flags = DirtyFlags::default();
    }

    /// How long since the most recent successful sensor poll.
    /// Useful for "stale data" indicators.
    pub fn sensor_age(&self) -> std::time::Duration {
        self.last_sensor_update.elapsed()
    }

    /// `true` if the prediction was updated within the last 5 seconds.
    pub fn prediction_is_fresh(&self) -> bool {
        self.last_prediction_update.elapsed() < std::time::Duration::from_secs(5)
    }
}

impl Default for SystemState {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_system_state_creation() {
        let state = SystemState::new();
        assert_eq!(state.ambient_temp, 20.0);
        assert_eq!(state.model_type, ModelType::Linear);
        assert_eq!(state.sensor_health, SensorHealth::Unknown);
    }
    
    #[test]
    fn test_dirty_flags() {
        let mut state = SystemState::new();
        assert!(!state.flags.ui);
        
        state.mark_ui_dirty();
        assert!(state.flags.ui);
        
        state.clear_all_dirty();
        assert!(!state.flags.ui);
    }
    
    #[test]
    fn test_model_type_string() {
        assert_eq!(ModelType::Linear.as_str(), "Linear");
        assert_eq!(ModelType::Neural.as_str(), "Neural");
    }

    #[test]
    fn test_sensor_health_strings() {
        assert_eq!(SensorHealth::Unknown.as_str(), "unknown");
        assert_eq!(SensorHealth::Ok.as_str(), "ok");
        assert_eq!(SensorHealth::Unavailable.as_str(), "unavailable");
        assert_eq!(SensorHealth::Degraded.as_str(), "degraded");
        assert!(SensorHealth::Ok.is_ok());
        assert!(!SensorHealth::Unknown.is_ok());
        assert!(!SensorHealth::Degraded.is_ok());
    }

    #[test]
    fn test_location_builder() {
        let loc = Location::new("Office").with_coords(45.5, -73.5);
        assert_eq!(loc.name, "Office");
        assert_eq!(loc.latitude, Some(45.5));
        assert_eq!(loc.longitude, Some(-73.5));
        assert!(!loc.id.is_empty());
    }

    #[test]
    fn test_poison_resistant_locks() {
        // Spawn a thread that panics while holding the write lock,
        // then verify the helpers still return usable guards.
        let state: WorldState = Arc::new(RwLock::new(SystemState::new()));
        let s2 = state.clone();

        let _ = std::thread::spawn(move || {
            let mut g = s2.write().unwrap();
            g.ambient_temp = 99.9;
            panic!("intentional panic to poison the lock");
        }).join();

        // The lock is poisoned, but the helpers recover and the data
        // we set is still there.
        let g = read(&state);
        assert_eq!(g.ambient_temp, 99.9);
    }

    #[test]
    fn test_sensor_age_starts_small() {
        let state = SystemState::new();
        assert!(state.sensor_age() < std::time::Duration::from_secs(1));
    }
}

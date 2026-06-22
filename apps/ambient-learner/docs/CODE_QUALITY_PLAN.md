# Code Quality & Architecture Improvements

## Current Issues to Address

### 1. Magic Numbers
- Constants scattered throughout code
- Hard-coded values in functions
- No centralized configuration

### 2. Error Handling
- Many `.unwrap()` calls that could panic
- Silent error swallowing with `let _ = `
- No error propagation strategy

### 3. Code Organization
- 2000+ line single file
- Mixed concerns (UI, DB, model, sensors)
- No separation of concerns

### 4. Documentation
- Missing function documentation
- No module-level docs
- Unclear parameter meanings

### 5. Type Safety
- Raw strings for IDs
- Magic string matching
- No newtype patterns

### 6. Duplication
- Repeated lock patterns
- Similar error handling blocks
- Copy-pasted UI creation

## Refactoring Plan

### Phase 1: Extract Constants (15 min)
```rust
// Create config module
mod config {
    pub const PASSIVE_INTERVAL_MS: u64 = 1000;
    pub const ACTIVE_INTERVAL_MS: u64 = 100;
    pub const ACTIVE_BURST_COUNT: usize = 20;
    pub const HISTORY_SECONDS: u64 = 15;
    pub const TEMP_MIN: f32 = -40.0;
    pub const TEMP_MAX: f32 = 40.0;
    pub const LEARNING_RATE: f32 = 0.05;
    pub const DATA_DIR: &str = "weather_data";
    
    // UI Constants
    pub mod ui {
        pub const WINDOW_WIDTH: i32 = 400;
        pub const WINDOW_HEIGHT: i32 = 700;
        pub const UI_UPDATE_MS: u64 = 100;
        pub const PAGE_MARGIN: i32 = 20;
    }
    
    // Database Constants
    pub mod db {
        pub const RETRY_ATTEMPTS: u32 = 3;
        pub const RETRY_DELAY_MS: u64 = 50;
        pub const WAL_CACHE_SIZE: i32 = 10000;
    }
    
    // Chart Constants
    pub mod chart {
        pub const NETWORK_WIDTH: u32 = 800;
        pub const NETWORK_HEIGHT: u32 = 1200;
        pub const HISTORY_WIDTH: u32 = 800;
        pub const HISTORY_HEIGHT: u32 = 600;
    }
}
```

### Phase 2: Strong Types (20 min)
```rust
// Replace raw strings with types
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
pub struct LocationId(String);

impl LocationId {
    pub fn new() -> Self {
        Self(uuid::Uuid::new_v4().to_string())
    }
    
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[derive(Debug, Clone)]
pub struct Temperature(f32);

impl Temperature {
    pub fn from_celsius(value: f32) -> Result<Self, &'static str> {
        if value < TEMP_MIN || value > TEMP_MAX {
            Err("Temperature out of valid range")
        } else {
            Ok(Self(value))
        }
    }
    
    pub fn celsius(&self) -> f32 {
        self.0
    }
}
```

### Phase 3: Error Types (15 min)
```rust
// Custom error types instead of Box<dyn Error>
#[derive(Debug)]
pub enum AppError {
    Database(rusqlite::Error),
    Io(std::io::Error),
    Serialization(serde_json::Error),
    InvalidTemperature(f32),
    SensorAccessDenied,
    ModelNotFound,
}

impl std::fmt::Display for AppError {
    fn fmt(&self, f: &mut std::fmt::Formatter) -> std::fmt::Result {
        match self {
            AppError::Database(e) => write!(f, "Database error: {}", e),
            AppError::Io(e) => write!(f, "IO error: {}", e),
            AppError::Serialization(e) => write!(f, "Serialization error: {}", e),
            AppError::InvalidTemperature(t) => write!(f, "Invalid temperature: {}", t),
            AppError::SensorAccessDenied => write!(f, "Cannot access thermal sensors"),
            AppError::ModelNotFound => write!(f, "Model file not found"),
        }
    }
}

impl std::error::Error for AppError {}

pub type Result<T> = std::result::Result<T, AppError>;
```

### Phase 4: Helper Functions (20 min)
```rust
// Lock helpers to reduce duplication
trait LockExt<T> {
    fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R;
    
    fn with_lock_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R;
}

impl<T> LockExt<T> for Arc<Mutex<T>> {
    fn with_lock<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&T) -> R,
    {
        let guard = self.lock().unwrap();
        f(&*guard)
    }
    
    fn with_lock_mut<F, R>(&self, f: F) -> R
    where
        F: FnOnce(&mut T) -> R,
    {
        let mut guard = self.lock().unwrap();
        f(&mut *guard)
    }
}

// Usage:
model.with_lock_mut(|m| {
    m.calibrate(&readings, target);
    m.prune_weak_sensors();
});
```

### Phase 5: Module Structure (30 min)
```
src/
├── main.rs           # App entry, UI setup only
├── config.rs         # All constants
├── error.rs          # Error types
├── model/
│   ├── mod.rs        # Model trait
│   ├── thermal.rs    # ThermalModel implementation
│   └── stats.rs      # SensorStat
├── database/
│   ├── mod.rs        # Database trait
│   ├── sqlite.rs     # SQLite implementation
│   └── migrations.rs # Schema migrations
├── sensors/
│   ├── mod.rs        # Sensor trait
│   └── thermal.rs    # ThermalPoller
├── location/
│   ├── mod.rs        # Location types
│   └── detection.rs  # Auto-detection
└── ui/
    ├── mod.rs        # UI builder
    ├── pages/
    │   ├── live.rs   # Live temperature page
    │   ├── model.rs  # Model visualization
    │   ├── history.rs # History chart
    │   └── locations.rs # Locations management
    └── widgets/
        ├── calibration_dialog.rs
        └── location_menu.rs
```

### Phase 6: Documentation (15 min)
```rust
/// Represents a thermal prediction model that learns from sensor data.
///
/// The model uses a weighted average of thermal sensor readings to predict
/// ambient temperature. It automatically adjusts sensor weights based on
/// calibration data and prunes unreliable sensors.
///
/// # Examples
///
/// ```
/// let poller = ThermalPoller::new();
/// let mut model = Model::new(poller.sensor_names());
/// 
/// // Calibrate with known temperature
/// model.calibrate(&sensor_readings, 22.5);
/// 
/// // Make predictions
/// let prediction = model.predict(&new_readings);
/// ```
pub struct Model {
    /// Statistical information for each sensor
    stats: Vec<SensorStat>,
    /// Model bias term
    bias: f32,
    /// Number of calibration points used
    calibration_count: usize,
    /// When the model was created
    created_at: String,
    /// Hash of last saved state (for change detection)
    #[serde(skip)]
    last_saved_hash: u64,
}

impl Model {
    /// Creates a new model with the given sensor names.
    ///
    /// # Arguments
    ///
    /// * `sensor_names` - Vector of thermal sensor identifiers
    ///
    /// # Returns
    ///
    /// A new `Model` instance with zero-initialized statistics
    pub fn new(sensor_names: Vec<String>) -> Self {
        // ...
    }
}
```

### Phase 7: Cleanup Patterns (20 min)

**Replace `.unwrap()` with proper error handling:**
```rust
// Before:
let m = model.lock().unwrap();

// After:
let m = model.lock()
    .map_err(|e| AppError::LockPoisoned(format!("{}", e)))?;
```

**Replace `let _ = ` with explicit error handling:**
```rust
// Before:
let _ = save_locations(&locations);

// After:
if let Err(e) = save_locations(&locations) {
    eprintln!("⚠️  Failed to save locations: {}", e);
}
```

**Extract repeated patterns:**
```rust
// Helper for database operations with retry
fn db_with_retry<F, T>(mut operation: F) -> Result<T>
where
    F: FnMut() -> Result<T>,
{
    for attempt in 0..DB_RETRY_ATTEMPTS {
        match operation() {
            Ok(result) => return Ok(result),
            Err(AppError::Database(rusqlite::Error::SqliteFailure(err, _))) 
                if err.code == rusqlite::ErrorCode::DatabaseBusy => 
            {
                if attempt < DB_RETRY_ATTEMPTS - 1 {
                    eprintln!("Database busy, retrying... (attempt {})", attempt + 1);
                    thread::sleep(Duration::from_millis(DB_RETRY_DELAY_MS));
                    continue;
                }
            }
            Err(e) => return Err(e),
        }
    }
    Err(AppError::MaxRetriesExceeded)
}
```

### Phase 8: Performance Annotations (10 min)
```rust
/// # Performance
///
/// This operation acquires a lock on the model and may block if another
/// thread is performing a calibration. Typical execution time: <1ms.
///
/// # Panics
///
/// Panics if the model lock is poisoned (should never happen in practice).
pub fn predict(&self, readings: &[f32]) -> f32 {
    // ...
}

/// # Performance
///
/// This is a blocking I/O operation that writes to disk. On mobile storage,
/// this can take 50-200ms. Should be called from a background thread.
///
/// # Errors
///
/// Returns `AppError::Io` if the file cannot be written.
pub fn save(&self, path: &Path) -> Result<()> {
    // ...
}
```

## Implementation Order

1. ✅ Extract config constants (easy win)
2. ✅ Add function documentation (easy win)
3. ✅ Replace unwrap() in critical paths
4. ✅ Create error types
5. ⏳ Extract helper functions
6. ⏳ Split into modules (if file >2000 lines)
7. ⏳ Add strong types
8. ⏳ Performance annotations

## Code Quality Metrics

### Before:
- Lines: 2009
- Functions: ~30
- Modules: 1
- Documentation: Minimal
- Error handling: Inconsistent
- Unwraps: ~50+
- Magic numbers: ~20+

### Target:
- Lines: <500 per file
- Functions: <50 per module
- Modules: 8-10
- Documentation: All public items
- Error handling: Consistent Result<T>
- Unwraps: <10 (only in tests)
- Magic numbers: 0 (all in config)

## Testing Additions

```rust
#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn test_temperature_validation() {
        assert!(Temperature::from_celsius(20.0).is_ok());
        assert!(Temperature::from_celsius(-50.0).is_err());
        assert!(Temperature::from_celsius(100.0).is_err());
    }
    
    #[test]
    fn test_model_predict_empty() {
        let model = Model::new(vec![]);
        assert_eq!(model.predict(&[]), 20.0); // Default temp
    }
    
    #[test]
    fn test_location_id_uniqueness() {
        let id1 = LocationId::new();
        let id2 = LocationId::new();
        assert_ne!(id1, id2);
    }
}
```

## Linting & Formatting

Add to project:
```toml
# .cargo/config.toml
[target.x86_64-unknown-linux-gnu]
rustflags = [
    "-W", "missing-docs",
    "-W", "unused-must-use",
    "-W", "rust-2018-idioms",
]
```

Run before commit:
```bash
cargo fmt
cargo clippy -- -D warnings
cargo test
```

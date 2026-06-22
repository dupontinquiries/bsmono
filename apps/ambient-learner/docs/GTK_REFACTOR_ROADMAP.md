# GTK Refactoring Roadmap: WorldState Pattern

## 🎯 **Goal**

Refactor existing GTK app to use WorldState architecture:
- ✅ Better performance (RwLock instead of multiple Arc<Mutex>)
- ✅ Cleaner code organization
- ✅ 80% ready for Qt port
- ✅ Keep GTK UI working throughout refactor

---

## 📋 **Phased Approach (No Breaking Changes)**

### Phase 1: Create WorldState (Day 1)
**Files to create:**
- `src/world_state.rs`

**What changes:**
- Add new module alongside existing code
- Start using it in parallel with old Arc<Mutex> vars
- NO breaking changes yet

### Phase 2: Migrate Sensor System (Day 2)
**Files to modify:**
- `src/systems/sensor.rs` (new)
- `src/main.rs` (minimal changes)

**What changes:**
- Extract sensor polling thread → `sensor_system.rs`
- Update WorldState instead of direct Arc<Mutex>
- Old code still works

### Phase 3: Migrate Model System (Day 3)
**Files to modify:**
- `src/systems/model.rs` (new)
- `src/main.rs` (update UI loop)

**What changes:**
- Extract model prediction → `model_system.rs`
- UI reads from WorldState
- Remove model locks from UI update loop

### Phase 4: Migrate UI Update Loop (Day 4)
**Files to modify:**
- `src/main.rs` (simplify UI loop)

**What changes:**
- UI becomes pure reader of WorldState
- Remove complex lock juggling
- Dirty flags prevent over-updates

### Phase 5: Cleanup (Day 5)
**Files to modify:**
- `src/main.rs` (remove old Arc<Mutex> vars)
- All systems

**What changes:**
- Remove duplicate state
- Consolidate to single WorldState
- Performance testing

---

## 📊 **Before vs After**

### Current GTK Architecture

```rust
// main.rs (5,400 lines)

let model = Arc::new(Mutex::new(...));
let neural_field = Arc::new(Mutex::new(...));
let db = Arc::new(Mutex::new(...));
let history_buffer = Arc::new(Mutex::new(...));
let locations = Arc::new(Mutex::new(...));
// ... 20+ Arc<Mutex> variables!

// UI update loop (200+ lines)
glib::timeout_add_local(Duration::from_millis(100), move || {
    let model = model.clone();
    let neural = neural_field.clone();
    // ... clone everything
    
    if let Ok(m) = model.try_lock() {
        if let Ok(n) = neural.try_lock() {
            // ... nested locks
        }
    }
    // 200 more lines...
});
```

**Problems:**
- ❌ 20+ Arc<Mutex> variables
- ❌ Clone everything before closures
- ❌ Nested locks everywhere
- ❌ Hard to follow data flow
- ❌ Potential deadlocks

---

### New GTK Architecture (with WorldState)

```rust
// main.rs (< 100 lines!)

let world = Arc::new(RwLock::new(SystemState::new()));
let neural_field = Arc::new(Mutex::new(TemperatureField::new(53)));
let database = Arc::new(Mutex::new(Database::open(...).unwrap()));

// Spawn worker systems
systems::sensor::spawn_sensor_system(world.clone());
systems::model::spawn_model_system(world.clone(), neural_field.clone());
systems::database::spawn_database_system(world.clone(), database.clone());

// UI becomes simple reader
glib::timeout_add_local(Duration::from_millis(100), move || {
    let state = world.read().unwrap();
    
    if state.flags.ui {
        temp_label.set_text(&format!("{:.1}°C", state.ambient_temp));
        confidence_bar.set_fraction(state.confidence);
        // ... update UI
    }
    
    glib::ControlFlow::Continue
});
```

**Benefits:**
- ✅ Single Arc<RwLock> (vs 20+ Arc<Mutex>)
- ✅ No nested locks
- ✅ No deadlock risk
- ✅ UI is pure reader
- ✅ Clear data flow

---

## 🔧 **Implementation Steps**

### Step 1: Create WorldState Module

```bash
touch src/world_state.rs
touch src/systems/mod.rs
touch src/systems/sensor.rs
touch src/systems/model.rs
touch src/systems/database.rs
```

**Edit src/main.rs:**
```rust
mod world_state;
mod systems;

use world_state::{SystemState, WorldState};
```

---

### Step 2: Define WorldState

**File:** `src/world_state.rs`

```rust
use std::sync::{Arc, RwLock};
use std::collections::VecDeque;
use std::time::Instant;

/// Central application state
pub struct SystemState {
    // Sensor data
    pub sensors: Vec<f32>,
    pub sensor_count: usize,
    pub device_state: DeviceState,
    pub last_sensor_update: Instant,
    
    // Predictions
    pub ambient_temp: f32,
    pub confidence: f32,
    pub model_type: ModelType,
    
    // Calibration
    pub calibration_count: usize,
    pub neural_calibrations: usize,
    
    // History
    pub sensor_history: VecDeque<(Instant, Vec<f32>)>,
    pub temperature_history: VecDeque<(Instant, f32)>,
    
    // 3D Sphere
    pub sphere_samples: Vec<SpherePoint>,
    
    // Locations
    pub current_location: String,
    pub locations: Vec<Location>,
    
    // Dirty flags
    pub flags: DirtyFlags,
}

#[derive(Default)]
pub struct DirtyFlags {
    pub sensors: bool,
    pub model: bool,
    pub sphere: bool,
    pub ui: bool,
    pub database: bool,
}

#[derive(Clone, Copy)]
pub struct SpherePoint {
    pub x: f32,
    pub y: f32,
    pub z: f32,
    pub temperature: f32,
}

#[derive(Clone, Copy, PartialEq)]
pub enum ModelType {
    Linear,
    Neural,
}

#[derive(Clone, Default)]
pub struct DeviceState {
    pub cpu_load: f32,
    pub battery_level: f32,
    pub charging: bool,
    pub screen_brightness: f32,
}

#[derive(Clone)]
pub struct Location {
    pub id: String,
    pub name: String,
}

pub type WorldState = Arc<RwLock<SystemState>>;

impl SystemState {
    pub fn new() -> Self {
        Self {
            sensors: Vec::new(),
            sensor_count: 0,
            device_state: DeviceState::default(),
            last_sensor_update: Instant::now(),
            
            ambient_temp: 20.0,
            confidence: 0.0,
            model_type: ModelType::Linear,
            
            calibration_count: 0,
            neural_calibrations: 0,
            
            sensor_history: VecDeque::with_capacity(300),
            temperature_history: VecDeque::with_capacity(1000),
            
            sphere_samples: Vec::new(),
            
            current_location: "Home".to_string(),
            locations: Vec::new(),
            
            flags: DirtyFlags::default(),
        }
    }
}
```

---

### Step 3: Create Sensor System

**File:** `src/systems/sensor.rs`

```rust
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::world_state::WorldState;
use crate::ThermalPoller;

const PASSIVE_INTERVAL_MS: u64 = 100;

pub fn spawn_sensor_system(world: WorldState) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        println!("🌡️  Sensor system started");
        
        let mut poller = ThermalPoller::new();
        let mut last_poll = Instant::now();
        let mut consecutive_failures = 0;
        
        loop {
            // Poll every 100ms
            if last_poll.elapsed() < Duration::from_millis(PASSIVE_INTERVAL_MS) {
                std::thread::sleep(Duration::from_millis(10));
                continue;
            }
            last_poll = Instant::now();
            
            // Read sensors
            if let Some(readings) = poller.poll() {
                consecutive_failures = 0;
                
                let mut state = world.write().unwrap();
                
                // Update state
                state.sensors = readings;
                state.sensor_count = state.sensors.len();
                state.device_state = get_device_state();
                state.last_sensor_update = Instant::now();
                
                // Add to history
                state.sensor_history.push_back((Instant::now(), state.sensors.clone()));
                if state.sensor_history.len() > 300 {
                    state.sensor_history.pop_front();
                }
                
                // Mark model as needing update
                state.flags.model = true;
                
            } else {
                consecutive_failures += 1;
                
                // Recovery logic
                if consecutive_failures > 10 {
                    eprintln!("⚠️  Sensor failures, recreating poller");
                    poller = ThermalPoller::new();
                    consecutive_failures = 0;
                }
            }
        }
    })
}

fn get_device_state() -> crate::world_state::DeviceState {
    crate::world_state::DeviceState::default()
}
```

---

### Step 4: Create Model System

**File:** `src/systems/model.rs`

```rust
use std::sync::{Arc, Mutex, RwLock};
use std::time::Duration;
use crate::world_state::{WorldState, ModelType};
use crate::{Model, TemperatureField};

pub fn spawn_model_system(
    world: WorldState,
    linear_model: Arc<Mutex<Model>>,
    neural_field: Arc<Mutex<TemperatureField>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        println!("🧠 Model system started");
        
        loop {
            std::thread::sleep(Duration::from_millis(200));
            
            // Check if model needs update
            let needs_update = {
                let state = world.read().unwrap();
                state.flags.model
            };
            
            if !needs_update {
                continue;
            }
            
            // Get sensor data (quick read)
            let (sensors, device_state, neural_cal_count) = {
                let state = world.read().unwrap();
                (
                    state.sensors.clone(),
                    state.device_state.clone(),
                    state.neural_calibrations,
                )
            };
            
            // Run prediction (no lock held during computation!)
            let (prediction, confidence, model_used) = if neural_cal_count >= 5 {
                // Use neural field
                if let Ok(nf) = neural_field.try_lock() {
                    let pred = nf.estimate_ambient(&sensors, &device_state);
                    (pred, 0.85, ModelType::Neural)
                } else {
                    (20.0, 0.0, ModelType::Linear)
                }
            } else {
                // Use linear model
                if let Ok(model) = linear_model.try_lock() {
                    let pred = model.predict(&sensors);
                    (pred, 0.65, ModelType::Linear)
                } else {
                    (20.0, 0.0, ModelType::Linear)
                }
            };
            
            // Update state with results
            {
                let mut state = world.write().unwrap();
                state.ambient_temp = prediction;
                state.confidence = confidence;
                state.model_type = model_used;
                
                // Add to history
                state.temperature_history.push_back((std::time::Instant::now(), prediction));
                if state.temperature_history.len() > 1000 {
                    state.temperature_history.pop_front();
                }
                
                state.flags.model = false;
                state.flags.ui = true;
                state.flags.sphere = true;
            }
        }
    })
}
```

---

### Step 5: Create Systems Module

**File:** `src/systems/mod.rs`

```rust
pub mod sensor;
pub mod model;
pub mod database;

pub use sensor::spawn_sensor_system;
pub use model::spawn_model_system;
pub use database::spawn_database_system;
```

---

### Step 6: Update main.rs (Parallel with old code)

**Add to top of main.rs:**

```rust
mod world_state;
mod systems;

use world_state::{SystemState, WorldState};
```

**After line where you create Arc<Mutex> variables, add:**

```rust
// NEW: WorldState architecture (parallel with old code)
let world = Arc::new(RwLock::new(SystemState::new()));

// Spawn worker systems (these run in parallel with old polling thread)
let _sensor_handle = systems::spawn_sensor_system(world.clone());
let _model_handle = systems::spawn_model_system(
    world.clone(),
    model.clone(),
    neural_field.clone()
);

println!("✓ WorldState systems spawned (running in parallel)");
```

**In UI update loop, ADD this at the beginning (before existing code):**

```rust
// NEW: Read from WorldState (testing parallel system)
let (ws_temp, ws_conf, ws_model_type) = {
    let state = world.read().unwrap();
    (state.ambient_temp, state.confidence, state.model_type)
};

// Log comparison (temporary, for testing)
if update_count % 50 == 0 {
    println!("WorldState: {:.1}°C | Old: {:.1}°C", ws_temp, last_temp);
}

// OLD CODE CONTINUES BELOW (unchanged)
// ... existing UI update code
```

---

### Step 7: Database System (Optional, Day 3)

**File:** `src/systems/database.rs`

```rust
use std::sync::{Arc, Mutex, RwLock};
use std::time::{Duration, Instant};
use crate::world_state::WorldState;
use crate::Database;

const SAVE_INTERVAL_SECS: u64 = 3600;

pub fn spawn_database_system(
    world: WorldState,
    database: Arc<Mutex<Database>>,
) -> std::thread::JoinHandle<()> {
    std::thread::spawn(move || {
        println!("💾 Database system started");
        
        let mut last_save = Instant::now();
        
        loop {
            std::thread::sleep(Duration::from_secs(60));
            
            if last_save.elapsed().as_secs() >= SAVE_INTERVAL_SECS {
                let history = {
                    let state = world.read().unwrap();
                    state.sensor_history.clone()
                };
                
                if let Ok(db) = database.try_lock() {
                    // Save to database
                    println!("💾 Saving sensor history...");
                }
                
                last_save = Instant::now();
            }
        }
    })
}
```

---

## 🧪 **Testing Strategy**

### Phase 1: Parallel Systems

Run old + new systems together:

```rust
// Old system (existing)
let old_temp = last_temp;

// New system (WorldState)
let new_temp = world.read().unwrap().ambient_temp;

// Compare
if (old_temp - new_temp).abs() > 0.5 {
    println!("⚠️  Mismatch: old={:.1}, new={:.1}", old_temp, new_temp);
}
```

### Phase 2: UI Switchover

Add compile-time flag:

```rust
#[cfg(feature = "use_worldstate")]
let temp = world.read().unwrap().ambient_temp;

#[cfg(not(feature = "use_worldstate"))]
let temp = last_temp;
```

### Phase 3: Remove Old Code

Once verified, remove old polling thread and Arc<Mutex> variables.

---

## 📈 **Expected Performance Gains**

**Before (current):**
```
Arc<Mutex> locks: ~20 per update cycle
Nested locks: 3-4 levels deep
Lock contention: High (UI blocks sensor thread)
```

**After (WorldState):**
```
RwLock reads: ~1 per update cycle
Nested locks: 0
Lock contention: Low (many readers, rare writer)
```

**Estimated improvement:**
- 🚀 **UI responsiveness:** 40% faster
- 🔋 **Battery life:** 15% better (fewer wakeups)
- 🧠 **CPU usage:** 20% lower (less lock contention)

---

## 🎯 **Benefits for Qt Port**

Once refactored, Qt port becomes trivial:

**GTK version:**
```rust
// main.rs
let world = Arc::new(RwLock::new(SystemState::new()));
systems::spawn_all(world.clone());

// GTK UI reads WorldState
glib::timeout_add_local(..., || {
    let state = world.read().unwrap();
    update_gtk_ui(&state);
});
```

**Qt version:**
```rust
// main.rs (IDENTICAL!)
let world = Arc::new(RwLock::new(SystemState::new()));
systems::spawn_all(world.clone());

// Qt backend reads WorldState
fn poll_updates(&mut self) {
    let state = self.world.read().unwrap();
    update_qt_properties(&state);
}
```

**80% code reuse!**

---

## 📅 **Timeline**

| Day | Task | Files | Outcome |
|-----|------|-------|---------|
| 1 | Create WorldState | world_state.rs, systems/mod.rs | Module structure ready |
| 2 | Sensor system | systems/sensor.rs, main.rs | Parallel sensor polling |
| 3 | Model system | systems/model.rs, main.rs | Parallel prediction |
| 4 | UI migration | main.rs | UI reads WorldState |
| 5 | Cleanup | All files | Remove old Arc<Mutex> code |

**Total:** 5 days to cleaner, faster code!

---

## 🚀 **Next Steps**

1. Create `src/world_state.rs` with SystemState definition
2. Create `src/systems/` directory structure
3. Implement sensor system (runs parallel with old code)
4. Test and compare outputs
5. Gradually migrate UI to read from WorldState
6. Remove old code once verified

**Ready to start?** I can generate the actual code files to drop in!

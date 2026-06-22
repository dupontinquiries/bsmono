# WorldState Integration Checklist

## ✅ Phase 1: Foundation (COMPLETED)

- [x] Create `src/world_state.rs` module
- [x] Create `src/systems/sensor_system.rs`
- [x] Create `src/systems/model_system.rs`
- [x] Create `src/systems/mod.rs` coordinator
- [x] Write documentation
- [x] Create Qt scaffold (bonus)

**Status:** ✅ Ready to integrate into main.rs

---

## 📝 Phase 2: Integration (NEXT)

### Step 1: Add Module Declarations

**File:** `src/main.rs` (after line 63)

```rust
mod world_state;  // Central state management
mod systems;      // Worker thread systems
```

**Test:**
```bash
cargo check
# Should compile without errors
```

- [ ] Modules compile
- [ ] No errors

---

### Step 2: Add Use Statements

**File:** `src/main.rs` (after line 18)

```rust
use std::sync::RwLock;
use world_state::{WorldState, SystemState, ModelType};
```

**Test:**
```bash
cargo check
# Should still compile
```

- [ ] Imports resolve
- [ ] No warnings

---

### Step 3: Create WorldState at Startup

**File:** `src/main.rs` (around line 3000, after model/neural_field creation)

**Find this code:**
```rust
let neural_field = Arc::new(Mutex::new(neural_field_loaded));
```

**Add after it:**
```rust
// Create WorldState for new architecture
println!("Creating WorldState...");
let world = Arc::new(RwLock::new(SystemState::new()));

// Initialize world state with sensor info
{
    let mut state = world.write().unwrap();
    state.sensor_names = sensor_names.clone();
    state.sensor_count = sensor_names.len();
}
println!("✓ WorldState initialized");

// Spawn worker systems (sensor polling + model inference)
println!("Spawning worker systems...");
let _system_handles = systems::spawn_all(
    world.clone(),
    model.clone(),
    neural_field.clone(),
);
```

**Test:**
```bash
cargo build --release
./target/release/ambient-learner

# Expected output:
Creating WorldState...
✓ WorldState initialized
Spawning worker systems...
🚀 Spawning all system workers...
🌡️  Sensor system started
🧠 Model system started
✅ All systems spawned
```

- [ ] Build succeeds
- [ ] Worker threads spawn
- [ ] No crashes

---

### Step 4: Update UI Loop (Read from WorldState)

**File:** `src/main.rs` (around line 4100, in UI update loop)

**Find this code:**
```rust
glib::timeout_add_local(Duration::from_millis(100), move || {
    update_count += 1;
```

**Add BEFORE the closure:**
```rust
// Clone world for UI thread
let world_for_ui = world.clone();
```

**Inside the closure, REPLACE sensor polling with WorldState reading:**

**OLD (comment out for now):**
```rust
// if let Some(readings) = poller.poll() {
//     ... old sensor polling code
// }
```

**NEW (add this):**
```rust
// Read from WorldState (populated by worker threads)
let (temp, conf, model_str, calibrations, active) = {
    if let Ok(state) = world_for_ui.read() {
        (
            state.ambient_temp,
            state.confidence,
            state.model_type.as_str().to_string(),
            state.calibration_count,
            state.sensor_count
        )
    } else {
        (20.0, 0.0, "Unknown".to_string(), 0, 0)
    }
};

// Use these values for UI updates (existing code stays same)
temp_label.set_text(&format!("{:.1}°C", temp));
// ... rest of UI updates
```

**Test:**
```bash
cargo build --release
./target/release/ambient-learner

# Check:
- Temperature updates every ~1s
- Model type shows "Linear" or "Neural"
- No UI lag
```

- [ ] UI displays temperature
- [ ] Values update smoothly
- [ ] No lag or freezing

---

### Step 5: Remove Old Sensor Polling

**File:** `src/main.rs` (in UI loop)

**Delete or comment out:**
- Old `ThermalPoller::new()` in UI thread
- Old sensor polling in UI loop
- Old model prediction in UI loop

**Keep:**
- UI label updates
- Event handlers
- Everything else

**Test:**
```bash
cargo build --release
./target/release/ambient-learner

# Verify:
- Still works identically
- Temperature updates
- Calibration works
- No regressions
```

- [ ] App works identically
- [ ] No old polling code left
- [ ] Cleaner UI loop

---

## 🧪 Phase 3: Verification

### Functional Tests

- [ ] App starts without errors
- [ ] Temperature displays and updates
- [ ] Sensor count shows correctly
- [ ] Model type switches (Linear → Neural at ≥5 calibrations)
- [ ] Calibration button works
- [ ] Database logs data
- [ ] Sphere updates (Page 2)
- [ ] History displays (Page 3)
- [ ] Sync works (Page 5)

### Performance Tests

- [ ] UI feels smooth (60 FPS)
- [ ] No UI lag during ML computation
- [ ] CPU usage lower or same
- [ ] Battery life same or better
- [ ] No memory leaks

### Code Quality

- [ ] `cargo check` passes
- [ ] `cargo test` passes
- [ ] No new warnings
- [ ] Code is more organized
- [ ] main.rs is shorter

---

## 🚀 Phase 4: Future Systems

Once core integration works, add more systems:

### Sphere System

```rust
// src/systems/sphere_system.rs
pub fn spawn(world: WorldState, neural_field: Arc<Mutex<TemperatureField>>) {
    thread::spawn(move || {
        loop {
            if flags.sphere {
                // Sample 3D heat field
                // Update WorldState
            }
        }
    });
}
```

- [ ] Create sphere_system.rs
- [ ] Wire into systems::spawn_all()
- [ ] Test 3D visualization

### Database System

```rust
// src/systems/database_system.rs
pub fn spawn(world: WorldState, db: Arc<Mutex<Database>>) {
    thread::spawn(move || {
        loop {
            sleep(Duration::from_secs(3600)); // Hourly
            // Save history to database
        }
    });
}
```

- [ ] Create database_system.rs
- [ ] Wire into systems::spawn_all()
- [ ] Test hourly saves

### Forecast System

```rust
// src/systems/forecast_system.rs
pub fn spawn(world: WorldState, forecast_model: Arc<Mutex<ForecastModel>>) {
    thread::spawn(move || {
        loop {
            if flags.forecast {
                // Update forecast
            }
        }
    });
}
```

- [ ] Create forecast_system.rs
- [ ] Wire into systems::spawn_all()
- [ ] Test predictions

---

## 📊 Success Metrics

### Before Integration:
```
main.rs: 5,137 lines
UI loop: ~120ms
FPS: ~8 FPS
Testability: Hard
```

### After Integration:
```
main.rs: ~1,000 lines (target)
UI loop: ~2ms
FPS: 60 FPS
Testability: Easy
```

---

## 🎯 Qt Integration (Bonus)

Once GTK refactor is stable:

### Step 1: Share Core Modules

```bash
cd ambient-learner-qt/src
ln -s ../../ambient-learner/src/world_state.rs
ln -s ../../ambient-learner/src/systems
```

- [ ] Symlink world_state.rs
- [ ] Symlink systems/
- [ ] Qt project compiles

### Step 2: Wire Qt Backend

```rust
// ambient-learner-qt/src/main.rs
let world = Arc::new(RwLock::new(SystemState::new()));
systems::spawn_all(world.clone(), ...);
```

- [ ] Spawn systems in Qt version
- [ ] Qt UI shows real data
- [ ] Works on Windows/macOS

---

## 🎉 Done!

When all checkboxes are checked:
- ✅ GTK app uses WorldState architecture
- ✅ Performance improved
- ✅ Code better organized
- ✅ Ready for Qt port
- ✅ Future-proof

**Current Status:** Ready to start Step 1!

**Next Action:** Add module declarations to main.rs

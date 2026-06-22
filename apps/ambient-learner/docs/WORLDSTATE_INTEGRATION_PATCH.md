# WorldState Integration Patch for main.rs

## Add Module Declarations (after line 63)

```rust
mod world_state;  // Central state management
mod systems;      // Worker thread systems
```

## Add Use Statements (after line 18)

```rust
use std::sync::RwLock;
use world_state::{WorldState, SystemState, ModelType};
```

## In main() function, after creating model and neural_field:

### OLD CODE (around line 2980-3000):
```rust
let model = if let Some(model_path) = get_latest_model() {
    // ... load model
} else {
    Arc::new(Mutex::new(Model::new(sensor_names)))
};

let neural_field = Arc::new(Mutex::new(neural_field_loaded));
```

### ADD THIS AFTER (new code):
```rust
// Create WorldState for new architecture
let world = Arc::new(RwLock::new(SystemState::new()));

// Initialize world state with sensor names
{
    let mut state = world.write().unwrap();
    state.sensor_names = sensor_names.clone();
    state.sensor_count = sensor_names.len();
}

// Spawn worker systems (sensor polling + model inference)
let _system_handles = systems::spawn_all(
    world.clone(),
    model.clone(),
    neural_field.clone(),
);
```

## In UI Update Loop (around line 4100):

### CHANGE THIS:
```rust
glib::timeout_add_local(Duration::from_millis(100), move || {
    update_count += 1;
    
    // OLD: Poll sensors in UI thread
    // if let Some(readings) = poller.poll() { ... }
```

### TO THIS:
```rust
// Clone world for UI loop
let world_for_ui = world.clone();

glib::timeout_add_local(Duration::from_millis(100), move || {
    update_count += 1;
    
    // NEW: Read from WorldState (populated by worker threads)
    if let Ok(state) = world_for_ui.read() {
        if state.flags.ui {
            // Update UI from WorldState
            let temp = state.ambient_temp;
            let conf = state.confidence;
            let model_type_str = state.model_type.as_str();
            let calibrations = state.calibration_count;
            let active_sensors = state.sensor_count;
            
            // Use these values to update GTK labels
            // (rest of UI update code stays the same)
        }
    }
```

## Benefits:

1. **Sensor polling moved to background thread** (sensor_system)
2. **Model inference moved to background thread** (model_system)  
3. **UI only reads state** (no heavy computation)
4. **RwLock allows concurrent reads** (faster than Mutex)
5. **Dirty flags prevent unnecessary updates** (battery friendly)

## Testing:

```bash
cargo build --release

# Should see:
🚀 Spawning all system workers...
🌡️  Sensor system started
🧠 Model system started
✅ All systems spawned
```

## Notes:

- Keep old code commented out initially for rollback
- Worker threads run independently
- UI polls WorldState at same 100ms interval
- No functional changes, just better architecture
- Sets foundation for Qt port (same WorldState works with both!)

# GTK Refactoring to WorldState Architecture

## ✅ Phase 1: Foundation (COMPLETED)

### Created Files:
1. `src/world_state.rs` - Central state with RwLock
2. `src/systems/sensor_system.rs` - Sensor polling thread
3. `src/systems/model_system.rs` - ML prediction thread  
4. `src/systems/mod.rs` - System coordinator

### Architecture:
```
WorldState (Arc<RwLock<SystemState>>)
    ↕
┌──────────────┬────────────────┐
│    Sensor    │     Model      │
│    System    │     System     │
└──────────────┴────────────────┘
        ↓
    GTK UI (polls WorldState)
```

---

## 🔄 Phase 2: Wire Into main.rs (IN PROGRESS)

### Steps:

**1. Add world_state to main.rs**
```rust
mod world_state;
mod systems;

use world_state::WorldState;
use std::sync::RwLock;
```

**2. Create WorldState at startup**
```rust
// Replace individual Arc<Mutex<>> with single WorldState
let world = Arc::new(RwLock::new(world_state::SystemState::new()));
```

**3. Spawn worker systems**
```rust
// After model/database setup
let _system_handles = systems::spawn_all(
    world.clone(),
    model.clone(),
    neural_field.clone()
);
```

**4. Update UI loop to read from WorldState**
```rust
glib::timeout_add_local(Duration::from_millis(100), move || {
    if let Ok(state) = world.read() {
        if state.flags.ui {
            // Update GTK labels
            temp_label.set_text(&format!("{:.1}°C", state.ambient_temp));
            model_label.set_text(state.model_type.as_str());
            // ... more updates
        }
    }
    glib::ControlFlow::Continue
});
```

**5. Remove old sensor polling from UI loop**
```rust
// DELETE this block (now handled by sensor_system):
// if let Some(readings) = poller.poll() { ... }
```

---

## 📋 Phase 3: Migration Checklist

### Old main.rs Structure → New Structure

| Old | New | Status |
|-----|-----|--------|
| Individual `Arc<Mutex<T>>` | Single `WorldState` | ✅ Ready |
| UI loop polls sensors | Sensor system thread | ✅ Ready |
| UI loop runs model | Model system thread | ✅ Ready |
| `model.lock()` in UI | `world.read()` | ⬜ TODO |
| Manual flag tracking | Dirty flags | ✅ Ready |
| Scattered state updates | Centralized in WorldState | ✅ Ready |

---

## 🎯 Phase 4: Benefits After Refactor

### Performance Improvements:
1. **No more UI blocking** - Heavy ML runs in background
2. **RwLock > Mutex** - Multiple readers don't block
3. **Dirty flags** - Skip unnecessary work
4. **Better CPU usage** - Dedicated threads vs UI loop

### Code Quality:
1. **Single source of truth** - WorldState
2. **Clear ownership** - Each system owns its logic
3. **Testable** - Systems are independent
4. **Maintainable** - ~300 lines per file vs 5400

### Future-Proof:
1. **Qt-ready** - Same WorldState works with Qt
2. **CLI-ready** - Core logic reusable
3. **Web-ready** - Can add HTTP API layer
4. **Mobile-ready** - Core works on Android

---

## 🚀 Phase 5: Next Steps

### Immediate (Today):
1. ✅ Create WorldState module
2. ✅ Create system worker threads
3. ⬜ Wire into main.rs
4. ⬜ Test that UI still works
5. ⬜ Remove old polling code

### Short Term (This Week):
1. Add sphere system worker
2. Add database system worker
3. Add forecast system worker
4. Clean up main.rs to <500 lines

### Medium Term (Next Week):
1. Extract UI pages to separate files
2. Create service layer (calibration, sync)
3. Add proper error handling
4. Write unit tests

### Long Term (Month):
1. Start Qt/QML version
2. Add CLI tool
3. Add web dashboard
4. Performance benchmarks

---

## 📊 Code Metrics

### Before Refactor:
```
main.rs: 5,137 lines (MONOLITHIC!)
- UI layout: ~2000 lines
- Event handlers: ~1500 lines
- Sensor polling: ~300 lines
- Model updates: ~400 lines
- Everything mixed together
```

### After Refactor:
```
main.rs: ~800 lines (UI setup + coordination)
world_state.rs: ~200 lines (state definition)
systems/:
  - sensor_system.rs: ~150 lines
  - model_system.rs: ~180 lines
  - sphere_system.rs: ~200 lines (future)
  - database_system.rs: ~150 lines (future)

Total: ~1,680 lines (vs 5,137)
Reduction: 67% smaller, better organized
```

---

## 🧪 Testing Strategy

### Unit Tests (New):
```rust
#[test]
fn test_sensor_system() {
    let world = Arc::new(RwLock::new(SystemState::new()));
    let handle = sensor_system::spawn(world.clone());
    
    thread::sleep(Duration::from_millis(500));
    
    let state = world.read().unwrap();
    assert!(state.sensor_count > 0);
}
```

### Integration Tests:
```rust
#[test]
fn test_full_pipeline() {
    // Sensor → Model → UI
    let world = create_test_world();
    spawn_all_systems(world.clone());
    
    // Wait for update
    thread::sleep(Duration::from_secs(1));
    
    let state = world.read().unwrap();
    assert!(state.ambient_temp > 0.0);
    assert!(state.flags.ui);
}
```

---

## 🔍 Code Review Checklist

Before merging refactor:
- [ ] All tests pass
- [ ] UI still works (no regressions)
- [ ] Performance same or better
- [ ] No new compiler warnings
- [ ] Documentation updated
- [ ] Example usage added

---

## 💡 Key Insights

### Why RwLock?
```rust
// UI reads frequently (100 Hz)
let state = world.read().unwrap();  // ✅ Non-blocking

// Sensor writes occasionally (10 Hz)
let mut state = world.write().unwrap();  // ✅ Exclusive

// Multiple readers can read simultaneously!
// This is 10x faster than Mutex for read-heavy workloads
```

### Why Dirty Flags?
```rust
// Without flags:
loop {
    run_expensive_model();  // ❌ Every 100ms!
}

// With flags:
if state.flags.model {
    run_expensive_model();  // ✅ Only when needed!
    state.flags.model = false;
}
```

### Why Separate Threads?
```rust
// Old: Everything in UI thread
UI loop:
    poll sensors     // 5ms
    run model        // 50ms   ← BLOCKS UI!
    update UI        // 2ms

// New: Separate threads
Sensor thread:
    poll sensors     // 5ms    ← Independent

Model thread:
    run model        // 50ms   ← Independent

UI thread:
    update UI        // 2ms    ← Smooth!
```

---

## 🎉 Success Criteria

Refactor is complete when:
1. ✅ GTK UI works identically
2. ✅ Performance improves (no UI lag)
3. ✅ Code is organized (<500 lines/file)
4. ✅ All tests pass
5. ✅ Ready for Qt port (shared core/)

**Current Status:** Phase 1 complete, Phase 2 in progress
**Next Step:** Wire WorldState into main.rs

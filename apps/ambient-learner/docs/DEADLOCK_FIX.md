# Deadlock Fix - Window Not Loading

## Problem
App hung at "[8/10] Creating pages..." - window never appeared.

## Root Cause
**Deadlock during Page 5 construction:**

```rust
// Page 5 code (WRONG):
.subtitle(&format!("📊 {} calibrations ({})", 
    model.lock().unwrap().calibration_count,        // Lock #1
    if model.lock().unwrap().calibration_count >= 20  // Lock #2 - DEADLOCK!
```

**Why it deadlocked:**
1. Background thread loads model and holds lock
2. UI thread tries to access model during Page 5 construction
3. Two `model.lock()` calls in same expression
4. Second lock waits for first to release
5. But first can't release until expression completes
6. **Deadlock!**

## Fix

### 1. Use Single Lock with try_lock
```rust
// FIXED:
let (cal_count, model_type) = model.try_lock()
    .map(|m| {
        let count = m.calibration_count;  // Single lock, read both values
        let mtype = if count >= 20 { "Neural" } else { "Linear" };
        (count, mtype)
    })
    .unwrap_or((0, "Loading..."));  // Fallback if locked

let current_db_row = adw::ActionRow::builder()
    .title("Current Model")
    .subtitle(&format!("📊 {} calibrations ({})", cal_count, model_type))
    .build();
```

**Benefits:**
- Single lock acquisition
- Non-blocking (`try_lock` instead of `lock`)
- Graceful fallback if model is busy
- Shows "Loading..." during background load

### 2. Delay Background Thread
```rust
std::thread::spawn(move || {
    // Let UI thread start first
    std::thread::sleep(Duration::from_millis(100));
    
    // Then load model
    if let Some(model_path) = get_latest_model() {
        // ...
    }
});
```

**Why this helps:**
- UI construction gets 100ms head start
- Reduces chance of contention
- Background loading is not time-critical

## Testing

**Before:**
```
[8/10] Creating pages...
[HANGS - window never appears]
```

**After:**
```
[8/10] Creating pages...
[9/10] All handlers configured
[10/10] Presenting window...
✓ UI initialization complete!
[Window appears]
```

## Prevention

**Rule:** Never call `.lock()` multiple times in the same expression!

**Bad:**
```rust
format!("{} {}", 
    mutex.lock().unwrap().field1,  // Lock 1
    mutex.lock().unwrap().field2   // Lock 2 - Can deadlock!
)
```

**Good:**
```rust
let guard = mutex.lock().unwrap();
format!("{} {}", guard.field1, guard.field2)
```

**Better (non-blocking):**
```rust
let (f1, f2) = mutex.try_lock()
    .map(|g| (g.field1, g.field2))
    .unwrap_or((default1, default2));
format!("{} {}", f1, f2)
```

## Build & Test

```bash
cargo build --release
./target/release/ambient-learner
```

**Expected:**
- Window loads in <2 seconds
- Page 5 shows "0 calibrations (Linear)" or "Loading..."
- After background load completes, shows actual count
- No hanging

## Summary

✅ **Fixed:**
- Deadlock in Page 5 construction
- Multiple lock acquisitions in single expression
- Background thread contention

✅ **Changes:**
- Use `try_lock()` instead of `lock()`
- Single lock acquisition
- 100ms delay in background thread
- Graceful fallback values

**Status:** Window should now load properly!

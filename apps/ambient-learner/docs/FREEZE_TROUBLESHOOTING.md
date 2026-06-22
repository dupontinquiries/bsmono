# UI Freeze Troubleshooting Guide

## Symptoms

1. **UI sometimes doesn't load** - App starts but window never appears
2. **App freezes after calibration** - After clicking calibrate and entering data, UI becomes unresponsive

## Root Causes & Fixes

### Issue 1: UI Doesn't Load

**Most Common Causes:**

#### A. Exception During Startup
**Check console output:**
```bash
./target/release/ambient-learner 2>&1 | tee startup.log
```

**Look for:**
- Panic messages
- "thread panicked at ..."
- Error messages between startup markers

**Debug markers added:**
```
[1/10] Loading locations...
[2/10] Starting background cleanup...
[3/10] Loading model...
[4/10] Loading database...
[5/10] Initializing state...
[6/10] Starting sensor polling thread...
[7/10] Building UI window...
[8/10] Creating pages...
[9/10] All handlers configured
[10/10] Presenting window...
✓ UI initialization complete!
```

**If it stops before [10/10]:**
- Check the last marker reached
- Look for error right after that marker
- Common issues at each stage:
  - [1/10]: locations.json corrupt
  - [3/10]: model_*.json corrupt
  - [4/10]: sensors_*.db corrupt or locked
  - [6/10]: sensor access issues
  - [7/10]: GTK/libadwaita not properly installed

#### B. Corrupt Data Files
```bash
cd ~/.local/share/ambient-learner/

# Check locations.json
cat locations.json | jq '.'
# If error: rm locations.json

# Check latest model
ls -lt model_*.json | head -1
# If corrupt: rm model_latest.json (or the bad one)

# Check database
sqlite3 sensors_*.db "SELECT COUNT(*) FROM readings;"
# If error: rm sensors_*.db
```

#### C. Lock Contention on Startup
**Symptoms:**
- Hangs at [4/10] or [6/10]
- No error messages

**Fix:**
```bash
# Kill any running instances
killall -9 ambient-learner

# Check for stale locks
lsof | grep ambient-learner

# Remove lock files if any
rm ~/.local/share/ambient-learner/*.lock 2>/dev/null

# Try again
./target/release/ambient-learner
```

### Issue 2: Freeze After Calibration

**Cause:** Lock contention between calibration thread and UI update thread

**Symptoms:**
- Dialog closes
- Console shows "Calibration: Starting burst readings..."
- Then freezes - no more output
- UI becomes unresponsive

**Debug Output:**
```
Calibration: Starting burst readings...
Calibration: Got 50 burst readings
Calibration: Applying to model...
✓ Model saved after calibration: ...
Calibration: Saving to database...
✓ Calibration saved to database
✓ Calibration complete!
```

**If freeze happens:**

1. **Between "Starting" and "Got X readings"**
   - Sensor access deadlock
   - Fix: Reduce ACTIVE_BURST_COUNT from 50 to 10

2. **Between "Got X readings" and "Applying to model"**
   - History buffer lock contention
   - Rare - shouldn't happen

3. **After "Applying to model", before "Model saved"**
   - Model lock held too long during save
   - **Fixed** in latest code by releasing lock before save

4. **After "Model saved", before "Calibration saved"**
   - Database lock contention
   - **Fixed** in latest code with better lock handling

**Workarounds:**

**Immediate (stop freeze):**
```bash
# Don't kill -9, let it finish
# Wait 5-10 seconds
# If still frozen, kill and restart:
killall ambient-learner
./target/release/ambient-learner
```

**Permanent fixes (in code):**

1. **Reduce burst count** (if sensor access is slow):
```rust
const ACTIVE_BURST_COUNT: usize = 10;  // Was 50
```

2. **Increase UI update interval** (reduce lock contention):
```rust
glib::timeout_add_local(Duration::from_millis(200), ...);  // Was 100
```

3. **Add timeout to locks**:
```rust
// Instead of:
let mut m = model.lock().unwrap();

// Use:
match model.try_lock() {
    Ok(mut m) => {
        // Do work
    }
    Err(_) => {
        eprintln!("Model lock busy, skipping update");
        return;
    }
}
```

## Diagnostic Commands

### Check if app is frozen or just slow:
```bash
# In one terminal:
./target/release/ambient-learner

# In another:
watch -n 1 'ps aux | grep ambient-learner'

# Look for:
# - CPU usage (should be <5%)
# - If stuck at 100% CPU = infinite loop
# - If stuck at 0% CPU = waiting on lock
```

### Get thread backtrace during freeze:
```bash
# Find PID
pidof ambient-learner

# Attach gdb
sudo gdb -p <PID>

# Get backtrace
thread apply all bt

# Look for:
# - pthread_mutex_lock (waiting on lock)
# - parking_lot (Rust Mutex)
# - Multiple threads waiting on same lock = deadlock
```

### Check for deadlock pattern:
```
Thread 1: waiting on model.lock() - already held by Thread 2
Thread 2: waiting on db.lock() - already held by Thread 1
= DEADLOCK
```

## Prevention

### Best Practices:

1. **Always release locks ASAP:**
```rust
// BAD:
let m = model.lock().unwrap();
do_expensive_operation();
m.save();  // Still holding lock!

// GOOD:
{
    let m = model.lock().unwrap();
    do_quick_operation();
} // Lock released here
do_expensive_operation();
```

2. **Never nest locks:**
```rust
// BAD:
let m = model.lock().unwrap();
let d = db.lock().unwrap();  // DANGER: can deadlock

// GOOD:
let data = {
    let m = model.lock().unwrap();
    extract_data(&m)
}; // Release model lock
let d = db.lock().unwrap();
d.save(data);
```

3. **Use try_lock when possible:**
```rust
match model.try_lock() {
    Ok(m) => { /* do work */ }
    Err(_) => { /* skip this update */ }
}
```

## Recovery Steps

### If UI won't load:

1. **Clean data directory:**
```bash
mv ~/.local/share/ambient-learner ~/.local/share/ambient-learner.backup
./target/release/ambient-learner
```

2. **Reinstall dependencies:**
```bash
sudo apt install --reinstall libadwaita-1-0 libgtk-4-1
```

3. **Rebuild from scratch:**
```bash
cargo clean
cargo build --release
```

### If freezes during calibration:

1. **Let it finish** (wait 10 seconds)
2. **Kill and restart** if still frozen after 10s
3. **Reduce burst count** in code
4. **Add debug output** to find exact freeze point

## Testing for Freeze Resistance

### Stress test:
```bash
# Rapid calibrations (test lock handling)
for i in {1..10}; do
    echo "Calibration $i"
    # Manual: Click calibrate, enter 20+$i
    sleep 2
done
```

### Concurrent operations:
```bash
# While app running:
# 1. Switch locations rapidly
# 2. Navigate pages quickly
# 3. Trigger calibration
# 4. Export CSV
# All at same time - should not freeze
```

## Fixed in Latest Code

✅ Model lock released before save (calibration)
✅ DB operations outside model lock
✅ Debug output tracks progress
✅ Better error handling in all locks
✅ Pulse state updated on main thread only

## Still Experiencing Freezes?

**Collect this info:**

1. **Startup log:**
```bash
./target/release/ambient-learner 2>&1 | tee startup.log
# Include full output
```

2. **Freeze backtrace:**
```bash
sudo gdb -p $(pidof ambient-learner)
thread apply all bt > backtrace.txt
```

3. **System info:**
```bash
uname -a
lsb_release -a
pkg-config --modversion gtk4 libadwaita-1
```

4. **Data state:**
```bash
ls -lah ~/.local/share/ambient-learner/
```

Then report with all above files.

## Quick Reference

| Symptom | Likely Cause | Quick Fix |
|---------|--------------|-----------|
| Hangs at [1/10] | Corrupt locations.json | `rm locations.json` |
| Hangs at [3/10] | Corrupt model | `rm model_*.json` |
| Hangs at [4/10] | Locked database | `killall ambient-learner` |
| Hangs at [6/10] | Sensor access issue | Check dmesg, permissions |
| Freeze after calibrate | Lock contention | Wait 10s or kill/restart |
| Never shows window | GTK issue | Check `gtk4-demo` works |
| CPU at 100% | Infinite loop | Kill, report bug |
| CPU at 0% frozen | Deadlock | Kill, check backtrace |

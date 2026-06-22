# Startup Freeze Troubleshooting Guide

## Symptoms

App freezes on startup and doesn't always reach the UI.

## Potential Causes

### 1. Blocking Operations on Main Thread

**What to check**:
```bash
# Run with debug output
RUST_BACKTRACE=1 ./ambient-learner 2>&1 | tee startup.log
```

Look for:
- Where it stops printing
- Last message before freeze

**Common culprits**:
- Model loading from disk
- Database opening
- Thermal sensor file access
- Directory scanning

### 2. Thermal Sensor Access Issues

**Check sensor availability**:
```bash
ls -la /sys/class/thermal/thermal_zone*/temp
cat /sys/class/thermal/thermal_zone*/temp
```

**If permission denied**:
```bash
sudo chmod 644 /sys/class/thermal/thermal_zone*/temp
```

**If sensors don't exist**:
The app will create a model with 0 sensors, which should still work.

### 3. File Lock Conflicts

**Check for existing instances**:
```bash
ps aux | grep ambient-learner
killall ambient-learner
```

**Check for locked database**:
```bash
lsof ~/.local/share/ambient-learner/*.db
```

### 4. Directory Permissions

**Check data directory**:
```bash
ls -ld ~/.local/share/ambient-learner/
ls -la ~/.local/share/ambient-learner/
```

**Fix permissions**:
```bash
chmod 755 ~/.local/share/ambient-learner/
chmod 644 ~/.local/share/ambient-learner/*
```

### 5. GTK/Libadwaita Issues

**Check GTK installation**:
```bash
pkg-config --modversion gtk4
pkg-config --modversion libadwaita-1
```

**Test basic GTK**:
```bash
# If this freezes, it's a GTK issue not app issue
gtk4-demo
```

## Debug Build

Add debug prints to identify where it freezes:

```rust
fn build_ui(app: &adw::Application) {
    eprintln!("DEBUG: Starting build_ui");
    
    eprintln!("DEBUG: Loading model...");
    let model = ...;
    eprintln!("DEBUG: Model loaded");
    
    eprintln!("DEBUG: Loading database...");
    let db = ...;
    eprintln!("DEBUG: Database loaded");
    
    eprintln!("DEBUG: Starting polling thread...");
    thread::spawn(move || {
        eprintln!("DEBUG: Polling thread started");
        ...
    });
    eprintln!("DEBUG: Polling thread spawned");
    
    eprintln!("DEBUG: Creating window...");
    let window = ...;
    eprintln!("DEBUG: Window created");
    
    eprintln!("DEBUG: Presenting window...");
    window.present();
    eprintln!("DEBUG: Window presented");
}
```

## Quick Fixes

### Fix 1: Clean Start

```bash
# Backup data
cp -r ~/.local/share/ambient-learner ~/.local/share/ambient-learner.backup

# Remove everything
rm -rf ~/.local/share/ambient-learner

# Try again
./ambient-learner
```

### Fix 2: Skip Model Loading

If model loading is slow, you can temporarily disable it:

```rust
// Comment out model loading, always create fresh
let model = {
    let poller = ThermalPoller::new();
    Arc::new(Mutex::new(Model::new(poller.sensor_names())))
};
```

### Fix 3: Run Without Sensors

If thermal sensors are the issue:

```rust
impl ThermalPoller {
    fn new() -> Self {
        // Disable sensor access for testing
        Self {
            files: Vec::new(),  // Empty - no sensors
            buffer: vec![0; 32],
            locked: false,
        }
    }
}
```

### Fix 4: Disable Database

Test if database is causing the freeze:

```rust
let db = Arc::new(Mutex::new(Database::new(&get_timestamp()).unwrap()));
// Don't try to load existing database
```

## Fixes Applied in Latest Code

### Non-Blocking History Updates

Changed from synchronous to asynchronous:
```rust
// OLD - blocks UI thread
let daily_data = {
    let all_dbs = list_all_databases();
    // ... database operations ...
};

// NEW - runs on idle
glib::idle_add_local_once(move || {
    let daily_data = {
        let all_dbs = list_all_databases();
        // ... database operations ...
    };
    // Update UI here
});
```

### Simplified Closure Handling

Removed `Rc<RefCell<>>` wrappers that could cause immediate execution:
```rust
// OLD - could execute immediately
let update_history = Rc::new(RefCell::new(update_history));
(update_history.borrow())();

// NEW - simple closure
let update_history = update_history.clone();
update_history();
```

## Testing After Rebuild

After applying fixes:

```bash
cargo clean
cargo build --release

# Test with verbose output
./target/release/ambient-learner 2>&1 | tee test.log

# Should see:
# === Ambient Learner v0.2.0 ===
# Data directory: /home/user/.local/share/ambient-learner
# [either "Found existing model" or "No existing model found"]
# [Window should appear within 2 seconds]
```

## Performance Expectations

On modern hardware:
- **Model loading**: < 50ms
- **Database opening**: < 100ms  
- **Window creation**: < 500ms
- **Total startup**: < 1 second

If startup takes > 3 seconds, something is wrong.

## Common Error Messages

### "Failed to load model"
- Model file corrupt
- JSON parse error
- → Fix: Delete model files, let app recreate

### "Failed to open existing database"
- Database locked by another process
- Database file corrupt
- → Fix: Delete database, let app recreate

### "Sensors Unavailable"
- Normal if no thermal sensors exist
- App should still work for calibration

## Still Freezing?

If app still freezes after all fixes:

1. **Get a backtrace**:
```bash
gdb ./target/release/ambient-learner
run
# Wait for freeze
# Press Ctrl+C
bt
```

2. **Check system logs**:
```bash
journalctl --user -xe | grep ambient
dmesg | tail -50
```

3. **Try minimal GTK app**:
```bash
# Test if it's GTK or your code
gtk4-demo
```

4. **Report issue** with:
- Output of `./ambient-learner 2>&1`
- OS and desktop environment
- GTK/Libadwaita versions
- Whether `gtk4-demo` works

## Rollback

If you need to go back to a working version:

```bash
git checkout <previous-commit>
cargo build --release
```

Or use the backup:
```bash
mv ~/.local/share/ambient-learner.backup ~/.local/share/ambient-learner
```

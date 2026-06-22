# Emergency Fix - Disabled Signal Handlers & WiFi

## Problem
App still shows "failed to register: timeout was reached" and window doesn't appear.

## Emergency Actions Taken

### 1. Disabled signal-hook Dependency
```toml
# Cargo.toml
# signal-hook = "0.3"  # DISABLED
```

**Why:** signal-hook's signal registration might be timing out

### 2. Commented Out Signal Handler Code
All signal handler setup code is now commented out in build_ui()

### 3. Disabled WiFi Detection
All WiFi detection code is now commented out

**Why:** zbus D-Bus connection might be timing out

### 4. Added Extensive Debug Output
```
[DEBUG] Step 1: Starting build_ui
[DEBUG] Step 2: Signal handlers DISABLED
[DEBUG] WiFi detection DISABLED
[DEBUG] Step 4: Loading CSS
[DEBUG] Page 1 added
[DEBUG] Page 2 added
...
```

---

## Build & Test

```bash
# Clean build
cargo clean
cargo build --release 2>&1 | tee build.log

# Run with full output
./target/release/ambient-learner 2>&1 | tee run.log
```

**Watch the debug output carefully. Where does it stop?**

---

## Expected Output (if this fixes it)

```
=== Ambient Learner v0.2.0 ===
[DEBUG] Step 1: Starting build_ui
[DEBUG] Step 2: Signal handlers DISABLED for testing
[DEBUG] WiFi detection DISABLED for testing
[DEBUG] Step 4: Loading CSS
✓ Loaded liquid glass CSS styles
[1/10] Loading locations (async)...
...
[DEBUG] Page 1 added
[DEBUG] Page 2 added
[DEBUG] Page 3 added
[DEBUG] Page 4 added
[DEBUG] Page 5 added
[DEBUG] Starting swipe gesture setup
[9/10] All handlers configured
[10/10] Presenting window...
✓ UI initialization complete!
```

**If you see all these debug messages and the window appears:**
→ The problem was signal-hook or WiFi detection

**If it still hangs:**
→ Tell me the LAST debug message you see

---

## If This Works

We can re-enable features one by one:

1. **First:** Get window working (current goal)
2. **Then:** Add back WiFi with better error handling
3. **Finally:** Add back signal handlers if needed

---

## Alternative: Check System Issues

If still hanging, the timeout might be system-level:

```bash
# Check if dbus is running
ps aux | grep dbus

# Check if NetworkManager is running  
systemctl status NetworkManager

# Check for hung processes
ps aux | grep ambient-learner

# Kill any hung instances
pkill -9 ambient-learner
```

---

## Build Now

```bash
cargo build --release
```

Should compile without signal-hook dependency.

Then run and watch for debug output!

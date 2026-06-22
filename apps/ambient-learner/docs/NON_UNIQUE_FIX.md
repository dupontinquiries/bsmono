# Application Registration Fix - NON_UNIQUE Flag

## Problem
```
failed to register: timeout was reached
```

## Root Cause

**GTK Application Registration Issue**

The error is from GTK/GLib, not our code. It happens when:
1. Application tries to register with D-Bus using `application_id`
2. D-Bus registration times out (slow system, another instance, D-Bus issues)
3. App fails to start

**Why it happens:**
```rust
// Default behavior:
let app = adw::Application::builder()
    .application_id("org.localweather.App")  // Tries to register with D-Bus
    .build();
    
// D-Bus says: "timeout registering this ID"
// App fails to start
```

---

## Solution: NON_UNIQUE Flag

**Allow multiple instances, skip D-Bus registration:**

```rust
let app = adw::Application::builder()
    .application_id("org.localweather.App")
    .flags(gtk4::gio::ApplicationFlags::NON_UNIQUE)  // ← FIX
    .build();
```

**What NON_UNIQUE does:**
- Allows multiple app instances
- Skips D-Bus unique registration
- No timeout possible
- App always starts

---

## Trade-offs

**With UNIQUE (default):**
- ✅ Only one instance can run
- ✅ New instance activates existing window
- ❌ Can fail with D-Bus timeout
- ❌ Blocks if D-Bus slow

**With NON_UNIQUE (our fix):**
- ✅ Always starts, no timeout
- ✅ Works without D-Bus
- ✅ Works in containers/restricted environments
- ⚠️ Multiple instances allowed (usually fine for this app)

---

## Testing

```bash
cargo build --release
./target/release/ambient-learner
```

**Expected:**
```
Starting application...
=== Ambient Learner v0.2.0 ===
[DEBUG] Step 1: Starting build_ui
...
[10/10] Presenting window...
✓ UI initialization complete!
[WINDOW APPEARS]
```

**No more "failed to register" error!**

---

## Why This Is The Right Fix

**For this app:**
- Temperature monitoring app (not a system service)
- Multiple instances OK (different locations, testing, etc.)
- Should always start, even in degraded environments
- D-Bus registration not critical

**Alternative (if you need unique instances):**
- Handle registration errors gracefully
- Fall back to NON_UNIQUE on timeout
- Add retry logic

---

## Verification

**Test 1: Single instance**
```bash
./target/release/ambient-learner
# Should start immediately
```

**Test 2: Multiple instances**
```bash
./target/release/ambient-learner &
./target/release/ambient-learner &
# Both should start (two windows)
```

**Test 3: No D-Bus**
```bash
# Even without D-Bus, should work
DBUS_SESSION_BUS_ADDRESS="" ./target/release/ambient-learner
```

---

## Summary

✅ **Fixed:** D-Bus registration timeout
✅ **Method:** NON_UNIQUE flag
✅ **Result:** App always starts
✅ **Trade-off:** Multiple instances allowed (acceptable)

🎉 **Build and test - window should appear!**

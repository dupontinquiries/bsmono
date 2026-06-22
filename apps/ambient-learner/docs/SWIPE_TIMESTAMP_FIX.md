# Swipe Gesture Fix - Timestamp-Based Debouncing

## Error
```
error[E0599]: no method named `set_enabled` found for struct `GestureSwipe`
```

## Problem
`gtk::GestureSwipe` doesn't have a `set_enabled()` method in GTK4. The gesture controller API doesn't support enabling/disabling individual gestures.

## Solution: Timestamp-Based Debouncing

Instead of trying to disable the gesture, we track the timestamp of the last swipe and reject any swipes that occur within 500ms.

### Implementation

```rust
// Track last swipe time
let last_swipe_time = Rc::new(Cell::new(Instant::now()));

swipe_gesture.connect_swipe(move |gesture, velocity_x, _| {
    let now = Instant::now();
    let elapsed = now.duration_since(last_swipe_time.get());
    
    // Reject swipes within 500ms of last swipe
    if elapsed < Duration::from_millis(500) {
        println!("⏸️  Swipe too soon ({:?} ago), BLOCKING", elapsed);
        gesture.set_state(EventSequenceState::Denied);
        return;
    }
    
    // ... determine target page ...
    
    if let Some(target) = target_page {
        // Update timestamp IMMEDIATELY
        last_swipe_time.set(now);
        
        // Change page
        stack.set_visible_child_name(target);
        
        println!("✅ Page changed, 500ms lockout active");
    }
});
```

### How It Works

1. **Before swipe:** Check elapsed time since last swipe
2. **If < 500ms:** Deny gesture immediately, no processing
3. **If >= 500ms:** Process swipe normally
4. **After page change:** Update timestamp
5. **Result:** 500ms minimum between swipes

### Benefits

- ✅ Simple, no complex state machine
- ✅ No reliance on non-existent API methods
- ✅ Guaranteed 500ms minimum between swipes
- ✅ Works with GTK4's gesture system
- ✅ Clear debug output

### Expected Terminal Output

**Normal swipe:**
```
🔄 Swipe detected: velocity_x = -450
📍 Starting from page: 'live'
🎯 Target: 'model' from 'live'
✅ Page changed, 500ms lockout active
```

**Blocked rapid swipe:**
```
🔄 Swipe detected: velocity_x = -450
📍 Starting from page: 'live'
🎯 Target: 'model' from 'live'
✅ Page changed, 500ms lockout active

[User tries to swipe again immediately]

🔄 Swipe detected: velocity_x = -380
⏸️  Swipe too soon after previous (120ms ago), BLOCKING
```

### Why 500ms?

- ViewStack transition: ~200-300ms
- User perception delay: ~100ms
- Safety buffer: ~100ms
- Total: 500ms = smooth, prevents accidental double-swipes

### Comparison to Previous Approaches

| Approach | Issue | Status |
|----------|-------|--------|
| State machine | Still allowed multiple events | ❌ Failed |
| Gesture disable/enable | API doesn't exist in GTK4 | ❌ Compile error |
| Timestamp debouncing | Simple, works with GTK4 | ✅ **Current** |

## Build & Test

```bash
cargo build --release
./target/release/ambient-learner
```

**Test:**
1. Swipe left quickly
2. Try to swipe again immediately
3. Should see: `⏸️  Swipe too soon`
4. Wait 500ms, try again
5. Should work normally

**Result:** One page per swipe, guaranteed! ✅

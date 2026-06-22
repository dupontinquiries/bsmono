# Delimiter Error Fix

## Error
```
error: unexpected closing delimiter: `}`
    --> src/main.rs:2588:5
```

## Cause
Duplicate closing braces and statements were left in after the str_replace edit:

```rust
        } else {
            gesture.set_state(gtk::EventSequenceState::Denied);
        }
    });
            gesture.set_state(gtk::EventSequenceState::Denied);  // ← DUPLICATE
        }                                                        // ← DUPLICATE
    });                                                          // ← DUPLICATE
```

## Fix
Removed lines 2586-2588:
- Duplicate `gesture.set_state(gtk::EventSequenceState::Denied);`
- Duplicate `}`
- Duplicate `});`

## Result
Clean swipe gesture closure:

```rust
swipe_gesture.connect_swipe(move |gesture, velocity_x, _velocity_y| {
    // ... swipe logic ...
    
    if let Some(target) = target_page {
        // ... handle swipe ...
        gesture.set_state(gtk::EventSequenceState::Claimed);
    } else {
        gesture.set_state(gtk::EventSequenceState::Denied);
    }
});  // ← Single closing

swipeable_stack.add_controller(swipe_gesture);
```

## Status
✅ Fixed - Should compile now!

## Build
```bash
cargo build --release
./target/release/ambient-learner
```

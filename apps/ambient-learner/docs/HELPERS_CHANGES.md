# Exact Changes for helpers.rs

## Location: Line 16-19 in `src/helpers.rs`

### FIND THIS:
```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
```

### REPLACE WITH:
```rust
use std::collections::hash_map::DefaultHasher;
use std::hash::{Hash, Hasher};
use std::sync::{Arc, Mutex};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

// GTK-specific imports
#[cfg(feature = "gtk")]
use libadwaita as adw;
#[cfg(feature = "gtk")]
use glib;
```

---

## That's It!

Just add these 5 lines after line 19:
```rust

// GTK-specific imports
#[cfg(feature = "gtk")]
use libadwaita as adw;
#[cfg(feature = "gtk")]
use glib;
```

The rest of helpers.rs stays the same. The functions `show_toast()` and `try_lock_or_continue()` already have `#[cfg(feature = "gtk")]` on them at lines 1006 and 1040.

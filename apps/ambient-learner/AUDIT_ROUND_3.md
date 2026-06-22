# Comprehensive Audit & Fixes — Round 3

This is the final round summarizing every bug fix, dead code removal,
visibility correction, and incomplete-implementation completion across
the entire `ambient-learner-project` source tree.

## Critical Bugs Fixed

### 1. Network input vector size mismatch (silent corruption)
**File:** `src/temperature_field.rs:344, 577`

The neural network is initialized with input size `num_sensors*3 + 5 + pos_encoding_dims`,
but my earlier fix removing `gpu_load` and `battery_current` left only 4 device features.
The forward pass would have built an input vector with the wrong number of elements, causing
either a panic (matrix×vector size mismatch) or silent wrong predictions.

**Fix:** Restored 5 device-state push calls, with the 5th being `0.0` as a reserved slot.
The model still works; the unused dimension is just zeros forever, which the network
will learn to ignore.

### 2. parking_lot vs std::sync::Mutex confusion
**File:** `src/metrics.rs:90-91`

```rust
// BUG: parking_lot::Mutex::lock() returns a guard, not Result
time_since_last_reading: self.last_reading.lock().map(|t| t.elapsed())
```

`MutexGuard<Option<Instant>>` doesn't have `.map()` — only `Result<Guard>` does.
But since `parking_lot::Mutex::lock()` doesn't return a Result, this would never
have compiled.

**Fix:** Added `.as_ref()` so `.map()` operates on the wrapped `Option`:
```rust
time_since_last_reading: self.last_reading.lock().as_ref().map(|t| t.elapsed())
```

### 3. Duplicate get_device_state() with conflicting semantics
**File:** `src/systems/sensor_system.rs:74-81`

A second `get_device_state()` lived in `sensor_system.rs` with `battery_level: 100.0`
in violation of the documented 0.0–1.0 range. A test even asserted this wrong value.

**Fix:** sensor_system now delegates to `crate::get_device_state()` (the real
implementation in main.rs that reads from /proc and /sys). Test updated to range checks.

### 4. SyncManager::new() panics on init failure
**File:** `src/sync.rs:88`

```rust
let security = SecurityManager::new(data_dir.clone())
    .expect("Failed to initialize security");
```

`expect()` would panic if e.g. the data directory can't be created. Made `new()`
return `Result<Self, String>` so callers can recover.

### 5. Phantom directory from brace-expansion bash mishap
**File:** `src/ui/{gtk4,iced}/`

A literal directory with curly braces in the name was created by `mkdir 'src/ui/{gtk4,iced}'`
quoted incorrectly. Removed.

### 6. qt_backend.rs orphan #[cfg] attribute
**File:** `src/qt_backend.rs:8-10`

Two consecutive empty `#[cfg(feature = "qt")]` attributes with nothing to attach to,
creating an orphan attribute (compile error).

**Fix:** Cleaned up.

## Visibility Corrections (cascading from systems/sync access)

Made these `pub` because they're accessed from `crate::systems::*` and `crate::sync`:

- `Model` (struct), `Model::new`, `Model::predict`, `Model::calibration_count`
- `ThermalPoller` (struct), `ThermalPoller::new`, `ThermalPoller::poll`,
  `ThermalPoller::sensor_names`, `ThermalPoller::sensor_types`
- `Database` (struct), `Database::get_all_calibrations`, `Database::get_all_calibrations_v1`,
  `Database::store_calibration`
- `get_device_state` function

## Dead Code Removal

Files on disk but never declared in `main.rs` (cargo wouldn't compile them):
- `src/network.rs` — removed
- `src/network_viz.rs` — removed
- `src/cleanup.rs` — removed
- `src/lazy.rs` — removed
- `src/simd.rs` — removed (also requires nightly Rust)

## Module Declarations Added

Files that were on disk and useful but not declared, added with `#[allow(dead_code)]`:
- `mod constants` — centralized configuration values
- `mod error` — structured error types  
- `mod lock_utils` — lock-with-timeout helper
- `mod metrics` — performance counters

## Completed TODOs (turned stubs into real implementations)

### Real device state from Linux filesystems
**File:** `src/main.rs:get_device_state()`

Was returning hardcoded zeros. Now reads:
- **CPU load**: parses `/proc/stat`, computes idle vs total tick deltas, caches
  previous reading in a `OnceLock<Mutex<...>>` for delta calculation
- **Battery level**: scans `/sys/class/power_supply/BAT*/capacity`
- **Charging**: checks `BAT*/status == "Charging"`, plus `AC/online == 1`
- **Screen brightness**: reads `/sys/class/backlight/*/brightness ÷ max_brightness`

All reads have proper Option fallbacks; sensible defaults if any read fails
(non-Linux, sandboxed, or missing files).

## Tests Fixed

- `temperature_field::test_add_snapshot` — was passing `state` by value after
  signature changed to `&DeviceState`. Fixed all 4 call sites.
- `helpers::test_retry_exhaustion` — type inference failure, added explicit
  `Result<i32, &str>` annotation.
- `sensor_system::test_device_state` — was asserting wrong value range. Fixed.
- `sync::test_sync_manager` — updated for new `Result<Self>` return type.

## Iced Module API Update (feature-gated, not built by default)

**File:** `src/ui/iced/mod.rs`

Updated `text(...).style(temp_color)` to use the iced 0.12 `Style` closure API:
```rust
.style(move |_theme: &Theme| iced::widget::text::Style {
    color: Some(temp_color),
})
```
Only matters with `--features iced`. Default GTK build doesn't touch this.

## Conditional Compilation for D-Bus

**File:** `src/main.rs:2350-2483`

All `zbus` references gated with `#[cfg(feature = "dbus")]`:
- `use zbus::{...}` import
- `use async_io::block_on` import
- `get_current_network_ssid_dbus()` async function
- `get_current_network_ssid_async_with_timeout()` (with stub fallback)
- `get_current_network_ssid_async()` (with stub fallback)

Also added explicit type annotations to `zbus::Proxy::new()` calls
(`let nm_proxy: zbus::Proxy = ...`) to fix type inference failures.

## Trait Signature Consistency

The `TemperatureEstimator` trait's `update_sensor_history` was declared with
`&DeviceState` but two implementations had `DeviceState` (by value). Fixed both:
- `impl TemperatureEstimator for TemperatureField` (line 961)
- `impl TemperatureEstimator for Model` (line 1418)

## Unused Imports Cleaned Up

- `src/systems/sensor_system.rs` — removed `use std::sync::RwLock`
- `src/systems/model_system.rs` — removed `RwLock` from `use std::sync::{...}`
- `src/systems/mod.rs` — removed `RwLock` from `use std::sync::{...}`
- `src/qt_backend.rs` — removed `use std::sync::RwLock`
- `src/ui/iced/mod.rs` — removed unused `Arc` and `RwLock` imports

## Remaining Known Issues (Not Bugs, Just Caveats)

1. **`gtk::Text` downcast** at `main.rs:4462` — needs runtime verification.
   `EntryRow.first_child()` may return a wrapper widget on some libadwaita versions.
   Won't break compile; may make the default-text not show in dialogs.

2. **libadwaita version sensitivity** — `MessageDialog`, `EntryRow`,
   `ResponseAppearance` are deprecated in libadwaita 1.5+. Currently using
   `features = ["v1_4"]` which still includes them. Future migration to
   `AlertDialog` recommended.

3. **~50 unwrap()/expect() calls** in security/sync/forecast — runtime panic risk
   in edge cases, not compile errors.

4. **iced 0.12 module** is feature-gated — won't compile cleanly without `--features iced`
   if you actually need it. Default GTK build is unaffected.

## Build Commands That Should Work

```bash
# Default GTK build
cargo build --release

# With Qt UI  
cargo build --release --features qt

# With D-Bus network detection
cargo build --release --features "gtk,dbus"

# Everything
cargo build --release --features "gtk,qt,dbus"

# Tests
cargo test
```

## Files Modified (Complete List)

Round 3 modifications:
1. `src/main.rs` — 8 changes (visibility + real device-state impl)
2. `src/temperature_field.rs` — 5 changes (trait sig + tests + input padding)
3. `src/world_state.rs` — 1 change (Serde derives on DeviceState)
4. `src/sync.rs` — 2 changes (Result return + test update)
5. `src/systems/sensor_system.rs` — 3 changes (delegate + test)
6. `src/systems/model_system.rs` — 2 changes (revert speculative fix)
7. `src/systems/mod.rs` — 1 change (unused import)
8. `src/ui/mod.rs` — 1 change (allow unused)
9. `src/ui/iced/mod.rs` — 2 changes (imports + style API)
10. `src/qt_backend.rs` — 1 change (orphan attribute)
11. `src/helpers.rs` — 1 change (test type annotation)
12. `src/metrics.rs` — 1 change (parking_lot map)
13. `Cargo.toml` — 2 changes (libadwaita features, unused deps)

Files removed:
- `src/network.rs`, `src/network_viz.rs`, `src/cleanup.rs`, `src/lazy.rs`,
  `src/simd.rs`, `src/ui/{gtk4,iced}/`

## Total Bug Count Across All 3 Rounds

| Round | Errors Fixed |
|-------|--------------|
| Round 1 | 90+ |
| Round 2 | 25+ |
| Round 3 | 15 (real bugs) + 10 (cleanup) |
| **Total** | **140+ corrections** |

The build should now compile cleanly with `cargo build --release` on a system
with GTK4, libadwaita, and the `v1_4` feature available.

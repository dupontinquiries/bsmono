# SnapClone Rust Modules

Pure-Rust subcrates for SnapClone. Designed as reusable submodules so any
future UI mockup or fork can use the same business logic and protocol without
touching the Qt layer.

## Layer architecture

```
snapclone-app  (main binary — wires Qt UI + core)
      │
      ├── snapclone-qt-ui  (future: Qt implements ui-traits)
      │         │
      └── snapclone-ui-traits ──────── STABLE BOUNDARY ────────┐
                │                                               │
          snapclone-core                              Any future UI:
          (AppCore, events,                           egui / Slint /
           commands, models)                          web / terminal
                │
     ┌──────────┼──────────────┐
snapclone-  snapclone-  snapclone-  snapclone-
 storage    identity     nat         proto
  (SQLite)  (sc-ID)    (STUN/punch) (wire fmt)
```

## Crates

| Crate                   | C++ equivalent              | Status                   |
|-------------------------|-----------------------------|--------------------------|
| `snapclone-identity`    | AppController identity      | ✅ complete              |
| `snapclone-nat`         | NatTraversal.cpp            | ✅ stun + cgnat + punch  |
| `snapclone-proto`       | SnapRouter.cpp              | ✅ frame + chunk + sync  |
| `snapclone-storage`     | StorageManager.cpp          | ✅ contacts + snaps      |
| `snapclone-core`        | AppController (logic only)  | ✅ events + commands     |
| `snapclone-dht`         | GossipDiscovery + Jami      | 🔧 scaffold              |
| `snapclone-ui-traits`   | MainWindow + SwipeNavigator | ✅ traits + GestureRecognizer |

## Building

```bash
cd rust/
cargo build --release     # all crates
cargo test --workspace    # 20+ unit tests
```

## Key design decisions

### `snapclone-ui-traits` is the stable boundary
Any UI can implement `SnapApp`, `TabBar`, `GestureHost`, `ContactList`.
Core and storage changes don't require UI changes, and vice versa.

### `GestureRecognizer` is a pure Rust state machine
Ported from SwipeNavigator.cpp. Takes `GestureEvent` (touch / wheel),
returns `GestureOutcome` (Dragging / PageChanged / BounceBack).
A Qt backend calls `recognizer.process(event)` and drives animations from
the result. An egui backend does the same. Zero framework coupling.

### `GestureConfig` exposes all tunable parameters
`lock_threshold_px`, `commit_threshold_px`, `flick_threshold_px_ms`,
`wheel_idle_ms`, `direction_ratio` — all the constants that were buried in
SwipeNavigator.hpp are now in one struct you can tweak per-platform.

### DELETE journal + synchronous=FULL in snapclone-storage
Eliminates the WAL replay data-loss bug. Every contact save is fsynced.

### `AppEvent` / `AppCommand` are the complete API
The UI only ever calls `AppCore::dispatch(AppCommand)` and handles
`AppEvent`. It never calls into NetworkManager, StorageManager, etc.
This is the same layering as the C++ AppController but enforced by types.

## Adding a new UI backend (e.g. egui)

1. Create `snapclone-egui` crate
2. `impl TabBar for EguiTabBar { ... }`
3. `impl GestureHost for EguiGestureHost { ... }` (or use `GestureRecognizer` directly)
4. `impl ContactList for EguiContactList { ... }`
5. `impl SnapApp for EguiSnapApp { ... }`
6. In your `update()`: `app.poll_events(&core); app.render();`

No changes to `snapclone-core`, `snapclone-storage`, or `snapclone-nat` required.

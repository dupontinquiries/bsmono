# Orientation for a fresh session

This file is the single best place to start if you're (Claude or a
new contributor) sitting down with this codebase for the first time.

## What this project is in one paragraph

`ambient-learner` is a **local-only**, sensor-fusion + ML temperature
prediction app written in Rust. It reads `/sys/class/thermal` and
`/sys/class/hwmon`, runs a linear-regression and neural-field model
over those readings to estimate true ambient temperature, and
exposes its data to other apps on the system through three Linux
IPC channels (D-Bus, XDG inbox, Unix socket). It also publishes a
2D screen-temperature bitfield to `/run` for use cases like
rain/glove touch-input correction. It does **not** make network
requests — that's the job of a separate "network-aware sibling" app
that integrates over the local IPC.

Primary target: phones running mobile Linux (Mobian, postmarketOS,
Plasma Mobile, Phosh). Also runs on regular Linux desktops.

## Critical things to know upfront

1. **Local-only.** No TCP, no HTTP, no DNS in our process. Any
   internet-facing functionality lives in some other user-installed
   app. Don't be tempted to add `reqwest` or `tokio::net::TcpListener`.

2. **Three UI backends, each plays to its strengths.** GTK is
   the desktop-native default (HeaderBar + Stack, libadwaita
   conventions). Qt/QML is the primary mobile target — rich
   animations, mood theming, swipe nav, Canvas-painted seasonal
   backgrounds. Iced leans into Elm-architecture composability
   and Canvas-as-pure-function: dense data viz (sparklines,
   multi-line strip charts) plus keyboard-first navigation.
   Touch is first-class on all three. The user picks via
   `--ui {gtk,iced,qt}`. See ADR-015 for the framing.

3. **WorldState is the shared state.** `Arc<RwLock<SystemState>>`,
   accessed via `world_state::read()` and `world_state::write()`
   helpers that do poisoned-lock recovery. Worker threads write to
   it; UI threads read. Lock contention is real on hot paths;
   minimize hold time.

4. **Worker thread pattern.** `src/systems/` has four workers
   (sensor, model, environment, screen-temp). They're spawned by
   `systems::spawn_all` and run forever. No graceful-shutdown
   plumbing today; we expect the process to exit cleanly.

5. **SQLite via rusqlite 0.31** with `bundled` and `backup` features.
   WAL mode, 2s busy timeout. Writes are batched through
   `db_writer::DbWriter` which runs its own thread with an mpsc
   sync_channel for back-pressure.

6. **Public IPC is a stable contract.** D-Bus name
   `org.ambientlearner.Service1`, `/run/user/<uid>/ambient-learner.sock`,
   `~/.local/share/ambient-learner/inbox/`, and the screen-temp
   bitfield format are documented in `SERVICE_API.md` and
   `SCREEN_TEMPS.md`. Don't change those without a version bump.

## File-by-file mental map

```
src/
├── main.rs                       6000+ line monolith — needs splitting
│   ├── ThermalPoller             /sys/class/thermal poller + unit detection
│   ├── Database                  SQLite open/init/backup, prepared statements
│   ├── Model                     linear regression
│   └── GTK UI code               huge inline build_ui() function
├── world_state.rs                shared state struct + lock helpers
├── temperature_field.rs          neural field (NN over sensor readings)
├── qt_backend.rs                 Qt/QML backend (QObject macro madness)
├── ui/
│   ├── main.qml, *Page.qml       Qt UI views
│   ├── Theme.js                  shared theming / animation tokens
│   └── iced/mod.rs               Iced UI (less maintained)
├── systems/                      worker thread loops
│   ├── sensor_system.rs
│   ├── model_system.rs
│   ├── environment_system.rs
│   ├── screen_temp_system.rs
│   └── calibration_queue.rs
├── service/                      public IPC (#[cfg(feature = "service")])
│   ├── dbus_service.rs           org.ambientlearner.Service1
│   ├── unix_socket.rs            $XDG_RUNTIME_DIR/ambient-learner.sock
│   ├── inbox.rs                  $XDG_DATA_HOME/ambient-learner/inbox/
│   ├── map_pin.rs                MapPin struct, validation
│   ├── pin_store.rs              thread-safe pin store + expiry sweeper
│   └── databus_client.rs         snapclone-bus federation (best-effort)
├── screen_temps.rs               2D bitfield, kernel, atomic publish
├── screen_temp_hints.rs          consumer-driven resolution requests
├── db_writer.rs                  async batched SQLite writer
├── iio_proxy.rs                  iio-sensor-proxy D-Bus client
├── environment.rs                IIO sysfs poller (pressure, humidity, light)
├── seasons.rs                    season + time-of-day "mood"
├── forecast.rs                   per-location temperature forecasting
├── temperature_field.rs          neural field implementation
├── sphere.rs                     3D sphere viz (Qt)
├── sync.rs                       NFC sync (legacy)
├── security.rs                   crypto for sync payloads
└── dbmaint.rs                    vacuum / pruning
```

## Conventions worth following

- **Comments explain *why*, not what.** The code shows what; future
  Claude (you) reads the comment for context that isn't in the code.
- **No emoji-noise commits.** Decisions and rationale > emoji.
- **Threshold-based emission everywhere.** UI properties only emit
  signals when the value actually changed beyond a noise floor.
  Saves rebuilds and avoids spurious animation retriggers.
- **`#[cfg(feature = ...)]` per file or function**, not per
  expression. Crate-level feature flags compose; if it's confusing
  to reason about, split it into two helper functions.
- **`unwrap()` is acceptable in tests, almost never in production
  code.** Use `.unwrap_or(...)` or `Result` propagation.
- **`println!` for user-visible startup messages**, `log::info!` /
  `log::warn!` for runtime events.

## How features are added

Walk through a feature change in order:

1. **Decide if it crosses the public IPC contract.** If yes, update
   `SERVICE_API.md` or `SCREEN_TEMPS.md` first; that's the source
   of truth for downstream consumers.
2. **If it touches WorldState, plan the lock-hold time.** Reads are
   cheap. Writes need to minimize the critical section. Holding the
   write lock across a SQLite call is a bug.
3. **If it spawns a thread, give it a `name`.** `thread::Builder::new()
   .name("ambient-foo".into()).spawn(...)`. Helps debugging.
4. **Tests in the same file as the impl when possible.** Module-level
   `#[cfg(test)] mod tests { ... }`. Integration tests are rare.
5. **Update `04_TODO.md` if you leave anything unfinished.**

## What's been deliberately not built

These have come up and we've decided *not* to add them. Don't
re-litigate without a strong reason:

- **A TCP/HTTP server.** Networking lives in the sibling app.
- **A REST API.** D-Bus + Unix socket are sufficient.
- **Auto-trust gates on inbound calibrations.** Audit trail
  (`source` field) is sufficient for v1; first-contact prompt
  deferred until we have a real moderation need.
- **Temporal super-resolution / Kalman filtering.** Per-cell EMA
  smoothing is enough; deep filtering is the consumer's job.
- **A daemon mode that runs without a UI.** The app is a desktop
  app, not a service. `--headless` exists but is for testing.

## Things that have surprised us

- **SQLite `run_to_completion(-1, ...)` panics.** Use `step(-1)` in
  a loop instead. The `-1` shortcut for "all pages" works for
  `step()` but not `run_to_completion()`.
- **`SignalEmitter` in zbus 4 is called `SignalContext`.** Renamed
  in 5.0.
- **`use iced::...` doesn't resolve** because the Cargo.toml renames
  the crate to `iced_dep` (avoid colliding with our `mod iced`).
  Top of `iced/mod.rs` aliases it back.
- **Mobile thermal sensors sometimes report microdegrees** instead
  of millidegrees. `normalize_thermal_reading()` tries three
  divisors and picks the one that lands in the physical range.
- **`gtk::prelude::WidgetExt` isn't dyn-compatible** because of
  `PartialEq` supertrait. Use `&[&gtk::Widget]` + `.upcast_ref()`.

## Build commands

```bash
# Default (GTK)
cargo build --release

# Qt mobile target
cargo build --release --features "qt service"

# Everything
cargo build --release --features full

# Run tests
cargo test --features "qt service"

# CLI options
./target/release/ambient-learner --help
./target/release/ambient-learner --ui qt
./target/release/ambient-learner --import-db /path/to/other.db
./target/release/ambient-learner --export-db /tmp/backup.db
AMBIENT_DEBUG=1 ./target/release/ambient-learner --ui qt
```

## When in doubt

- **Check live code.** This doc and any other in `docs/` can be stale.
- **Read `03_DESIGN_DECISIONS.md`** if you're tempted to make an
  architectural change.
- **Update this file** if you discover something important that
  isn't here. The whole point is that this file stays useful.

# Architecture

## High-level diagram

```
┌─────────────────────────────────────────────────────────────────────┐
│                        ambient-learner process                       │
│                                                                      │
│  ┌──────────────────┐  ┌──────────────────┐  ┌──────────────────┐  │
│  │ Sensor system    │  │ Model system     │  │ Environment sys  │  │
│  │ /sys/class/      │  │ Linear + neural  │  │ IIO: pressure,   │  │
│  │ thermal poller   │  │ field            │  │ humidity, light  │  │
│  │ 100ms tick       │  │ 200ms tick       │  │ 30s tick         │  │
│  └────────┬─────────┘  └────────┬─────────┘  └────────┬─────────┘  │
│           │                     │                      │            │
│           └─────────────────────┼──────────────────────┘            │
│                                 ▼                                    │
│                    ┌────────────────────────────┐                   │
│                    │   WorldState               │                   │
│                    │   Arc<RwLock<...>>         │                   │
│                    │   shared state             │                   │
│                    └────────────┬───────────────┘                   │
│                                 │                                    │
│           ┌─────────────────────┼──────────────────────────┐        │
│           ▼                     ▼                          ▼        │
│  ┌─────────────────┐  ┌─────────────────┐  ┌─────────────────┐    │
│  │ UI backend      │  │ DB writer       │  │ Screen-temp pub │    │
│  │ GTK / Qt / Iced │  │ batched async   │  │ 4Hz to /run     │    │
│  │ 200ms refresh   │  │ 250ms / 256 row │  │ adaptive res    │    │
│  └─────────────────┘  └────────┬────────┘  └────────┬────────┘    │
│                                 │                    │             │
│                                 ▼                    ▼             │
│                          ┌──────────────┐    ┌──────────────┐    │
│                          │ SQLite WAL   │    │ tmpfs binary │    │
│                          │ sensors_*.db │    │ + meta JSON  │    │
│                          └──────────────┘    └──────────────┘    │
│                                                                    │
│  ┌────────────────────────────────────────────────────────────┐   │
│  │ Service feature (#[cfg(feature = "service")])              │   │
│  │ ┌──────────────┐  ┌──────────────┐  ┌──────────────┐      │   │
│  │ │ D-Bus        │  │ XDG inbox    │  │ Unix socket  │      │   │
│  │ │ session bus  │  │ inotify      │  │ stream out   │      │   │
│  │ └──────────────┘  └──────────────┘  └──────────────┘      │   │
│  │ ┌──────────────────────────────────────────────────┐      │   │
│  │ │ snapclone-bus federation client (opt-in)         │      │   │
│  │ └──────────────────────────────────────────────────┘      │   │
│  └────────────────────────────────────────────────────────────┘   │
└─────────────────────────────────────────────────────────────────────┘
                                  │
                                  ▼  (only by user-installed sibling apps)
                            ┌──────────────┐
                            │ The internet │
                            └──────────────┘
```

## Module dependency rules

- `world_state.rs` is the only module everyone depends on. It owns
  the shared state struct and the lock helpers.
- `systems/` workers read/write WorldState and nothing else from
  the rest of the code. They don't touch the UI or the database
  directly.
- `db_writer.rs` is the only module that writes to the database
  from a hot path. The synchronous `Database::store_*` methods on
  `main.rs::Database` still exist as a fallback for durability-
  critical writes (calibrations) and for the GTK code that hasn't
  been migrated.
- `service/` modules are all behind `#[cfg(feature = "service")]`.
  They don't touch the UI; they expose WorldState read-only via
  `dbus_service::start()`.
- `screen_temps.rs` is feature-free and pure (no IO except writing
  the published file). `systems/screen_temp_system.rs` wraps it
  with the worker loop.

## Key invariants

These are things the code assumes are true. Breaking them is a bug.

1. **WorldState is always lockable.** `world_state::read()` and
   `write()` recover from poisoned locks. Don't bypass them.
2. **Sensor count is stable per session.** `WorldState.sensor_names`
   is set once at startup and never changes. Calibration data,
   models, etc. are all sized to that count. Adding hot-plug
   support is a follow-up; today it would crash on resize.
3. **Database connection is owned by one writer thread.** The
   `db_writer::DbWriter` has its own connection. Other connections
   in the codebase are read-only. Two writers on the same DB cause
   `SQLITE_BUSY` errors that the retry loop usually masks but
   shouldn't be relied on.
4. **The screen-temp bitfield is atomic.** Reader sees either the
   previous frame intact or the new one intact, never torn. Achieved
   via write-temp-then-rename.
5. **D-Bus methods are non-blocking.** They snapshot state under the
   read lock and return. They do not block on DB I/O. If a method
   needs DB data, it must read from the in-memory cache.

## Thread inventory

| Thread name | Spawned by | What it does |
|---|---|---|
| `ambient-sensor` | `sensor_system::spawn` | Polls thermal sensors at 100ms |
| `ambient-model` | `model_system::spawn` | Runs linear + neural at 200ms, drains calibration queue |
| `ambient-env` | `environment_system::spawn` | Polls IIO sensors at 30s |
| `ambient-screen-temp` | `screen_temp_system::spawn` | Publishes bitfield at 4Hz |
| `ambient-db-writer` | `db_writer::DbWriter::spawn` | Drains write queue, batches into transactions |
| `ambient-pin-sweeper` | `pin_store::start_expiry_sweeper` | Drops expired pins every 60s |
| `ambient-dbus-signals` | `dbus_service::spawn_temperature_signal_emitter` | Emits D-Bus signals on temperature change |
| `ambient-socket-accept` | `unix_socket::start` | Accepts new socket clients |
| `ambient-socket-pump` | `unix_socket::start` | Broadcasts events to connected clients |
| `ambient-databus` | `databus_client::start` | Connects to snapclone-bus, reads pushes |
| `ambient-databus-mirror` | `spawn_outbound_mirror` | Forwards local pin changes to snapclone-bus |
| `ambient-databus-sensor` | `spawn_sensor_publisher` | Publishes our reading to snapclone-bus every 60s |
| QML / GTK main | UI backend | UI event loop (one thread) |

## Data flow for a temperature read

```
User calls backend.get_current_temperature() in QML
    ↓
QtBackend::get_current_temperature
    ↓
world_state::read(&world)
    ↓
SystemState.ambient_temp (f32)
    ↓
return to QML
```

That's it — five frames of stack between QML invocation and the
returned scalar. The model already computed the value on its
worker tick; the UI just reads from cache.

## Data flow for a calibration submit

```
User taps Calibrate in QML, enters 22.5
    ↓
QtBackend::submit_calibration(22.5)
    ↓
CalibrationQueue::push(CalibrationRequest::new(22.5))
    ↓
[next tick of model worker]
    ↓
model_system::run() drains queue
    ↓
Model::add_sample(sensors, 22.5)
    ↓
Model::train() — gradient descent step
    ↓
db_writer.submit_calibration(sensors, 22.5, location_id)
    ↓
[batched flush, eventually]
    ↓
SQLite INSERT INTO calibrations (...)
```

The model updates the next tick. The DB write is async and may
land seconds later in a batch. Both completions are visible to
the UI via WorldState fields that the model worker updates.

## Feature flag composition

```
[default]      = gtk
[gtk]          = gtk4 + libadwaita + glib
[qt]           = qmetaobject + qttypes
[iced]         = iced_dep (renamed from iced to avoid module collision)
[dbus]         = zbus 4.0
[service]      = dbus + notify  (= D-Bus + XDG inbox + Unix socket)
[full]         = gtk + qt + dbus + service
```

`service` implies `dbus`; you can have `dbus` without `service`
(for the iio_proxy client alone).

## Files that need attention

- **`src/main.rs`** is 6000+ lines. Splitting it is a real
  refactor: the GTK UI code, the `Database` impl, and the
  `Model` impl all want to be in their own modules.
- **The GTK UI** has the `build_ui()` mega-function. Extracting
  per-page builders would help.
- **`qt_backend.rs`** is OK but the macro-derived Qt properties
  are stretching the limits of what `qmetaobject` can comfortably
  express. Adding more properties is starting to slow compile time.

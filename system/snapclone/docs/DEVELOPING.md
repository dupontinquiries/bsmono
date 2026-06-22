# Developing SnapClone

A practical guide for getting from "I want to fix something" to "I shipped
a fix that doesn't break anything else." For *what we are building* see
[VISION.md](VISION.md); for *current priorities* see [TODO.md](TODO.md).

---

## Build & run

```bash
# Clean rebuild — do this when CMake or generated headers may have changed
rm -rf build && cmake -B build && cmake --build build

# Incremental — fast (~30s on desktop, ~15min on Fairphone 5)
cmake --build build

# Run
./build/src/SnapClone -l ./log.txt    # log to file
./build/src/SnapClone                 # log to stderr
```

For mobile (postmarketOS), build natively on the device. Cross-compiling
is technically possible but the toolchain churn has historically eaten
more time than the recompile cost.

---

## Source layout

```
include/          public headers (Q_OBJECT classes for AUTOMOC)
  core/           Types, AppController, SnapRouter, identity
  network/        NetworkManager, transports, NAT, discovery
  storage/        StorageManager (SQLite)
  ui/             Widgets, dialogs, pages
src/              implementation
rust/             AES-256-GCM crypto crate (cxx-rs bridge)
camera-bridge/    helper binary for camera handoff on some distros
docs/             you are here
```

Each public header has an implementation in the matching `src/` path.
Q_OBJECT classes need their header listed in the relevant CMakeLists.txt
under their target's `SOURCES` so AUTOMOC sees them.

---

## Namespaces

| Directory | Namespace |
|-|-|
| `src/core/`, `include/core/` | `SnapClone::Core` |
| `src/network/`, `include/network/` | `SnapClone::Network` |
| `src/storage/`, `include/storage/` | `SnapClone::Storage` |
| `src/ui/`, `include/ui/` | `SnapClone::UI` |
| `src/ui/video/`, `include/ui/video/` | `SnapClone::UI::Video` |
| `src/ui/camera/`, `include/ui/camera/` | `SnapClone::UI::Camera` |
| `src/ui/wayfinder/`, `include/ui/wayfinder/` | `SnapClone::UI` (no sub-namespace) |

When adding a new file: open with `namespace SnapClone::X { ... } // namespace`.

---

## Logging categories

Set up with `Q_LOGGING_CATEGORY(name, "snapclone.X")` near the top of
each `.cpp` file. Common categories:

| Category | What it covers |
|-|-|
| `snapclone.app` | AppController orchestration |
| `snapclone.network` | NetworkManager, transports |
| `snapclone.nat` | STUN, hole punching |
| `snapclone.udpchan` | SCUD UDP channel |
| `snapclone.dht` | OpenDHT operations |
| `snapclone.gossip` | LAN multicast discovery |
| `snapclone.router` | SnapRouter (snap delivery) |
| `snapclone.storage` | SQLite, migrations |
| `snapclone.camera` | CameraWidget orchestration |
| `snapclone.camera.qt` / `.gst` | Backend specifics |
| `snapclone.video.recorder` | gst-launch recording |
| `snapclone.video.shutter` | Hold-detect shutter button |
| `snapclone.video.transcode` | Re-encode to AV1/VP9/VP8 webm |
| `snapclone.ux` | UxGuide milestones / hints |
| `snapclone.ui` | General UI events |
| `snapclone.dialog` | MobileDialog |
| `snapclone.upnp` | Port mapping |
| `snapclone.firewall` | firewalld/portal |
| `snapclone.location` | GPS / location |
| `snapclone.session` | Login lifecycle |
| `snapclone.identity` | Stable ID load/save |

Filter at runtime: `QT_LOGGING_RULES="snapclone.nat=true" ./SnapClone`.

---

## Architecture invariants

These are the rules that have *cost us debugging time before* and we
keep relearning. Read these before touching the relevant subsystems.

### Q_LOGGING_CATEGORY placement

The macro **must** come before any anonymous-namespace function that
references it. Cost-of-not-doing: undefined reference at link time
that shows up as a confusing template error.

### gst-launch named muxers

Named elements like `webmmux name=mux` must be **declared before they
are referenced** (`! mux.`). gst-launch parses left-to-right.

### CameraWidget vs CameraBackend

`CameraWidget` is a pure UI shell. Backend-specific logic
(`v4l2src` direct, Qt camera, GstLaunchBackend) lives in
`Camera::CameraBackend` subclasses. Don't reach across.

### MediaType enum byte values

These are **wire-protocol stable**. Never renumber. New types append.

### AppController PIMPL

All AppController members live in `d->` (the `Impl` struct in
`src/core/AppController_p.hpp`). When adding new state, put it in the
PIMPL — *not* in the AppController class itself.

### Touch routing

Custom widgets that want touches:
1. `setAttribute(Qt::WA_AcceptTouchEvents)` on construction
2. Override `event()` and translate `QEvent::TouchBegin/Update/End`
   to mouse events yourself OR call the base class.

OR be a `QAbstractButton` — those are automatically allowed through
`SwipeNavigator`'s interactive-widget check.

If a touch isn't reaching your widget, the issue is probably
SwipeNavigator's gesture-capture. Check the interactive-widget
allowlist around `SwipeNavigator.cpp:349` and `:529`.

### `m_proc` lifecycle (any QProcess wrapper)

The pattern that consistently doesn't crash:

```cpp
~MyClass() {
    if (m_proc) {
        m_proc->QObject::disconnect();   // 1. drop all signal/slot
        m_proc->kill();                  // 2. async kill
        m_proc->deleteLater();           // 3. Qt event-loop cleanup
        m_proc = nullptr;
    }
}
```

DO NOT use `waitForFinished()` in destructors, hideEvents, or any
context that could be on a signal-handler stack. `waitForFinished`
runs a *nested event loop* which re-enters everything in flight.
That's the source of multiple historical crashes.

The ONE exception: `VideoRecorder::stop()` does use `waitForFinished(3000)`
because we need graceful EOS so matroskamux finalizes the file. That's
called only from explicit user-initiated stop, not from teardown chains.

### Lambda captures + receiver lifetime

Lambdas connected via `connect(sender, signal, receiver, [...](){...})`
should generally:
- Capture `this` only when the receiver is the class itself
- Use `QPointer<T>` when capturing a pointer to a sibling widget that
  might outlive the connection
- For one-shot timers that need to outlive their parent, prefer
  `QMetaObject::invokeMethod(... QueuedConnection)` to a fresh stack

### Q_OBJECT classes and AUTOMOC

Every header with a `Q_OBJECT` macro must be listed in its target's
CMakeLists.txt `SOURCES` (not just headers in `target_include_directories`).
Symptom of missing entry: linker complains about undefined
`vtable for X` or `staticMetaObject`.

### Wire format compatibility

`buildV3` is the live transport wire builder. It currently writes
`kWireVersion4` (videoDurationMs added). v3 readers will reject this
version. Both endpoints must upgrade together. There's no v3-tolerant
path; we chose simplicity over compatibility because the user runs
the same binary on both ends.

If you change wire format: bump `kWireVersion4` → `5`, add the new
field at the END of the existing fields (before the F_SYNC/F_DATA
tail), and add a `if (version >= kWireVersion5)` guard in
`handleV3Message` to read it conditionally.

### Schema migrations

`StorageManager.cpp` has a list of migrations indexed by version
number. Adding a new migration:

1. Append `{ N, QStringLiteral("ALTER TABLE ... ;") }` to the
   `kMigrations` list (don't reuse N).
2. Run them ordered, atomically per version, with the version
   recorded in `_version`. Each runs once; existing DBs auto-upgrade.
3. **Never modify an existing migration's SQL** — that would silently
   skip on already-upgraded DBs. Add a new migration instead.

---

## Common gotchas

### "Compiles on my machine, fails on phone"

Usually:
- A gstreamer element you have isn't on the phone (e.g.
  `gst-plugins-rs` for some Rust-implemented elements).
  Probe with `gst-inspect-1.0 <element>`.
- A header is in a Qt module not built on the phone (less common).
- `#ifdef __linux__` is fine; `#ifdef __unix__` is fine; `#ifdef
  Q_OS_LINUX` requires `<QtGlobal>`.

### "Mobile crashes silently with no error"

Often a synchronous-emit re-entry issue (signal handler enters a
nested event loop while a dialog/animation is mid-flight). Defer
with `QMetaObject::invokeMethod(... QueuedConnection)` instead of
`emit signal(...)` if you're emitting from inside another handler
that has visible UI consequences.

### "Logs spam from one address"

Almost always a self-feedback loop — we sent something to an address
we reach as ourselves (loopback or our own LAN IP) and replied to
the reply. Guard at top of the receiver with
`AddressClass::isLoopback()` and at the sender entry point. Both
sides — defence in depth.

### "App appears connected but messages don't deliver"

Probably idle-TCP-drop. NAT/firewall hardware silently closes the
TCP socket after 3-5 minutes of no traffic. WebSocket keepalive
sends a ping every 30s now. If you see this, check
`WebSocketTransport::onConnected` — the keepalive timer should
start AND be cleared on disconnect.

---

## Testing

There's no comprehensive test suite yet. The development loop is
"build, run on two devices, send snaps, check logs." For specific
subsystems:

- Storage: `tests/StorageTests.cpp` (if present — historically
  excluded from default build)
- Wire format: `tests/SnapRouterTests.cpp` (if present)

When adding tests:
- Pure functions (e.g. `AddressClassification` predicates) → easy,
  add a small QtTest case
- Stateful systems → harder; the actual integration tests are the
  manual two-device sessions

---

## Where to ask "why was it done this way"

In rough order of likelihood-to-have-the-answer:

1. The comment block above the suspicious code (we keep extensive
   inline rationale)
2. [ARCHITECTURE.md](../ARCHITECTURE.md) — ADRs for big decisions
3. [docs/SignalRegistry.md](SignalRegistry.md) — what signals exist
   and who listens
4. [docs/DATABUS.md](DATABUS.md) — internal event bus design
5. Recent transcripts in `/mnt/transcripts/` if developing in the
   Claude-assisted workflow
6. The git log
7. Ask the next person you talk to about it

If you find yourself wondering "why didn't they just do X" for more
than a minute, write up your finding in
[TODO.md](TODO.md) under 🧪 — it's probably worth a closer look.

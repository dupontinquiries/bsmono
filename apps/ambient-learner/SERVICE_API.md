# Ambient Learner — Local Service API

`ambient-learner` is a **local-only** sensor-fusion and prediction app. We
don't open TCP sockets, make HTTP requests, or do DNS. Anything that
needs to reach the network — weather APIs, remote map services, cloud
sync — runs in some **other app** on your system. That app is the
"network-aware sibling"; we're the local data hub it talks to.

This document describes the local IPC surface other apps can use to:

- **Read from us** — current temperature, prediction, calibration history,
  pressure / humidity / light readings, locations.
- **Push into us** — calibrations from a thermometer app, map pins from a
  weather/photo/shop sync, observations from a sensor module.

Three local channels are exposed for direct use, plus we participate
in the **snapclone-bus federation** if its daemon is running:

| Channel | Direction | Best for | Format |
|---|---|---|---|
| **D-Bus** session bus | both | Typed RPC, capability discovery, signals | binary D-Bus |
| **XDG inbox** files | inbound | Shell scripts, post-hooks, cron jobs | JSON files |
| **Unix socket** | outbound | Streaming "every update" outbound | newline-JSON |
| **snapclone-bus** | both | Multi-app federation with peers | newline-JSON |

You can use any combination. The first three are us *exposing*
endpoints; the fourth is us *connecting to* a daemon that some
other app on your system runs (see "Federation" below).

## D-Bus interface: `org.ambientlearner.Service1`

Service: **session bus** (not system), well-known name
`org.ambientlearner.Service1`, object path `/org/ambientlearner/Service`,
interface `org.ambientlearner.Service1`.

### Outbound (other apps read from us)

| Method | Returns | Description |
|---|---|---|
| `GetCurrentTemperature` | `(d temp_c, d confidence_0_1, s model_type)` | Latest prediction |
| `GetTemperatureAt(d lat, d lon)` | `(d temp_c, d confidence, t timestamp_unix, b in_range)` | Lookup at coordinates |
| `GetTemperatureHistory(u max_count)` | `a(td)` `(unix_ts, temp_c)` pairs | Recent samples for sync/upload |
| `GetPressureHistory(u max_count)` | `a(td)` `(unix_ts, hpa)` pairs | Recent barometer readings |
| `GetEnvironment` | `s` JSON snapshot | Pressure / humidity / light / tilt |
| `ListLocations` | `a(ssdd)` | Saved locations |
| `ListMapPins` | `a(ssdds)` | Currently active pins |
| `GetCapabilities` | `a{sv}` | Feature flags |

### Inbound (other apps push into us)

| Method | Returns | Description |
|---|---|---|
| `SubmitCalibration(d temp_c, s source)` | `(b accepted, s reason)` | Train the model |
| `SubmitMapPin(s id, s kind, d lat, d lon, s json_payload)` | `(b accepted, s reason)` | Add a pin |
| `RemoveMapPin(s id)` | `(b accepted, s reason)` | Drop a pin |

### Signals

| Signal | Args | Fires when |
|---|---|---|
| `TemperatureChanged` | `(d temp_c, d confidence)` | Temperature shifted by ≥ 0.05 °C |
| `MapPinsChanged` | — | A pin was added, modified, or removed |

### Examples

#### Python — pull temperature, push calibration

```python
from gi.repository import Gio, GLib

bus = Gio.bus_get_sync(Gio.BusType.SESSION, None)
proxy = Gio.DBusProxy.new_sync(
    bus, Gio.DBusProxyFlags.NONE, None,
    "org.ambientlearner.Service1",
    "/org/ambientlearner/Service",
    "org.ambientlearner.Service1",
    None,
)

# Read
temp, conf, model = proxy.GetCurrentTemperature()
print(f"{temp:.1f}°C ({model}, {conf:.0%})")

# Write
ok, reason = proxy.SubmitCalibration("(ds)", 22.5, "my-sync-app")
```

#### Bash — bulk export for upload

```bash
gdbus call --session \
    --dest org.ambientlearner.Service1 \
    --object-path /org/ambientlearner/Service \
    --method org.ambientlearner.Service1.GetTemperatureHistory \
    1000
```

## XDG inbox — file-based submissions

For apps that don't want a D-Bus binding, drop a JSON file into:

```
$XDG_DATA_HOME/ambient-learner/inbox/pins/<id>.json
# default: ~/.local/share/ambient-learner/inbox/pins/<id>.json
```

We watch the directory with `inotify` and ingest each file. The filename
(minus `.json`) becomes the pin id if the file's `id` field is empty.

### File format

```json
{
  "id": "weather-cache-paris",
  "kind": "weather",
  "latitude": 48.8566,
  "longitude": 2.3522,

  "title": "Paris forecast",
  "description": "Cached from network-app",
  "uri": "https://example.com/source",
  "payload": {
    "temperature_c": 18.5,
    "humidity_pct": 64,
    "summary": "partly cloudy"
  },
  "expires_at": 1893456000
}
```

Required: `id`, `kind`, `latitude`, `longitude`. Everything else
optional. `expires_at` is a Unix timestamp; we drop the pin when it
passes.

### Conventions for `kind`

Free-form, but these get UI decoration:

- `weather` — observation or forecast (publisher: a weather sync app)
- `photo` / `snap` / `image` — image with a location
- `shop` / `shopping` / `listing` — commerce listing
- `directory` / `places` — directory entry

Anything else falls under `other` and renders as a generic pin.

### Examples

```bash
# Network-aware weather app caches a forecast for us
cat > ~/.local/share/ambient-learner/inbox/pins/wx-paris.json <<'EOF'
{
  "kind": "weather",
  "latitude": 48.8566,
  "longitude": 2.3522,
  "payload": {"temperature_c": 18.5, "summary": "partly cloudy"},
  "expires_at": 1893456000
}
EOF

# Photo organizer post-hook publishes a snap
cat > ~/.local/share/ambient-learner/inbox/pins/$(uuidgen).json <<'EOF'
{
  "kind": "snap",
  "latitude": 45.5017,
  "longitude": -73.5673,
  "title": "Mont Royal sunset",
  "uri": "file:///home/me/Pictures/sunset.jpg"
}
EOF
```

## Unix socket — streaming outbound

For apps that want a continuous live feed, connect to:

```
$XDG_RUNTIME_DIR/ambient-learner.sock
# default: /run/user/<uid>/ambient-learner.sock
```

The socket emits **newline-delimited JSON**. One message per line. The
client doesn't need to send anything — this is one-way streaming
outbound. (Inbound submissions still go through D-Bus or the inbox.)

### Protocol

On connect we send a hello frame:

```json
{"hello":{"version":1,"name":"ambient-learner","events":[...]}}
```

Then events as they happen:

```json
{"event":"temperature","temp_c":21.3,"confidence":0.85,"model":"Linear","ts":1714329600}
{"event":"pressure","hpa":1014.2,"trend_30m":-0.4,"ts":1714329630}
{"event":"humidity","pct":52.0,"ts":1714329660}
{"event":"light","value":120.0,"unit":"lux","ts":1714329690}
{"event":"map_pins","count":3,"pins":[...],"ts":1714329720}
```

We only emit on real change (using the same noise thresholds as the
QML side: 0.05 °C, 0.1 hPa, 0.5 % humidity, 1 lux). Slow consumers get
back-pressured by the kernel write buffer; if a client can't keep up
for >250ms, we drop it and the broadcast continues without them.

### Examples

```bash
# Tail every event live
nc -U /run/user/$UID/ambient-learner.sock

# Filter for just temperature changes
nc -U /run/user/$UID/ambient-learner.sock | \
    grep '"event":"temperature"' | \
    jq -r '"\(.temp_c)°C @\(.ts)"'
```

```python
import socket, json, os

s = socket.socket(socket.AF_UNIX)
s.connect(f"/run/user/{os.getuid()}/ambient-learner.sock")

with s.makefile("r") as f:
    hello = json.loads(f.readline())
    print("connected:", hello)
    for line in f:
        evt = json.loads(line)
        if evt.get("event") == "temperature":
            push_to_remote_service(evt["temp_c"])  # network-aware app
```

## Why these three and nothing else

We deliberately did **not** build:

- ❌ A TCP / HTTP server in our process. We don't do networking — the
  whole point of the split is to keep the trust boundary small.
- ❌ A REST API. Use D-Bus or the socket.
- ❌ Cross-user data sharing. Per-user-session only.
- ❌ Auto-trust gates. Every D-Bus method records the caller's unique
  name in the pin's `source` field for an audit trail; a future
  "first contact" prompt can replay the log.

## Permissions and isolation

- **Session bus only**, never system bus. No polkit, no root, no
  cross-user access.
- The Unix socket is created with mode `0600` so only your user can
  read it.
- The XDG inbox lives in your home directory; standard filesystem
  permissions apply.
- Pin payloads stay in memory; the only on-disk artifact is the inbox
  file the publisher created.

## Federation: snapclone-bus

If a `snapclone-bus` daemon is running on your system at
`$XDG_RUNTIME_DIR/snapclone-bus.sock`, we automatically:

- **Subscribe** to the `public_snap`, `weather_station`, `public_photo`,
  and `shopping_directory` layers. Every point another app publishes
  to those layers is imported into our pin store as a `MapPin` (with
  `source` prefixed `databus:`) and shows up in our QML map.
- **Publish** every locally-owned pin onto the bus under the
  corresponding layer, so peers see what we render.
- **Publish** our live sensor reading (`sensor_reading` layer, 5-minute
  TTL) when we have current-location coordinates set, so a federated
  weather panel sees temperature/humidity/pressure from all peers.

Loop prevention: pins arriving from the bus carry a `source` like
`databus:com.example.someapp`, and we don't republish those — that
prevents a point from echoing forever between us and the daemon.

If snapclone-bus isn't running, none of this happens — we log once
at startup ("snapclone-bus not reachable; running standalone") and
all our other channels work normally.

### Layer ↔ kind mapping

| Bus layer | Our `kind` | Notes |
|---|---|---|
| `public_snap` | `snap` | |
| `weather_station` | `weather` | |
| `public_photo` | `photo` | |
| `shopping_directory` | `shop` | |
| `sensor_reading` | `sensor` | We publish ours periodically |
| `custom:<x>` | `<x>` | Any other kind round-trips through Custom |

## Building with the service feature

```bash
cargo build --release --features "qt service"
./target/release/ambient-learner --ui qt
```

You should see startup messages:

```
🛰️  D-Bus service live as org.ambientlearner.Service1 at /org/ambientlearner/Service
📂 inbox watcher live at /home/you/.local/share/ambient-learner/inbox/pins
📡 unix-socket service live at /run/user/1000/ambient-learner.sock
📡 connected to snapclone-bus at /run/user/1000/snapclone-bus.sock      # if running
```

(Or `snapclone-bus not reachable; running standalone` if no peer daemon.)

That's all four channels up. The network-aware sibling app can now:

- Subscribe to `TemperatureChanged` over D-Bus or `nc -U` the socket
  to stream readings outbound.
- Call `GetTemperatureHistory(1000)` to bulk-export for upload.
- Drop a `weather` pin into the inbox when remote forecasts arrive.
- Call `SubmitCalibration(22.3, "my-app")` when a user records an
  observation.

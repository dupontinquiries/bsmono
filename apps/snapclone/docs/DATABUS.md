# SnapClone Data Hub — Integration Guide

## The model

SnapClone is a **local data hub**. It never makes outbound HTTP requests.
The internet-aware app on your system does all networking; SnapClone
provides the local rendezvous point through three IPC channels.

```
┌───────────────────────────────────────────────────────────────┐
│  Your weather app / photo syncer / map fetcher / any app      │
│  (makes its own HTTP/TLS connections — SnapClone doesn't)     │
└─────────────────────┬─────────────────────────────────────────┘
                      │  Three local IPC channels
         ┌────────────┼──────────────────────────┐
         │            │                          │
   ┌─────▼──────┐ ┌───▼──────────┐ ┌────────────▼────────┐
   │  D-Bus     │ │ XDG inbox    │ │ Stream socket        │
   │ Channel A  │ │ Channel B    │ │ Channel C            │
   │            │ │              │ │                      │
   │ Typed RPC  │ │ Drop a .json │ │ nc -U sock           │
   │ Signals    │ │ No libs      │ │ → live JSON forever  │
   │ Discovery  │ │ Shell-ready  │ │ One-way outbound     │
   └────────────┘ └──────────────┘ └──────────────────────┘
```

---

## Channel A: D-Bus

**Service:** `org.snapclone.DataHub` on the session bus
**Object:** `/org/snapclone/DataHub`
**Interface:** `org.snapclone.DataHub1`

### Methods (your app calls SnapClone)

| Method | Returns | Purpose |
|--------|---------|---------|
| `GetCurrentLocation()` | JSON string | Our last known GPS/IP fix |
| `GetLayerPoints(layer, limit)` | JSON string | Snapshot of in-TTL points |
| `GetPublicSnaps(bbox, limit)` | JSON string | Public snaps in bounding box |
| `SubmitMapPoint(json)` | string (id) | Push a point from remote source |
| `GetCapabilities()` | string array | Which layers have live data |
| `GetSnapHistory(limit)` | JSON string | Recent public snaps |

### Signals (SnapClone tells your app)

| Signal | Args | Fires when |
|--------|------|------------|
| `LocationChanged` | lat, lon, acc, ssid | Our GPS/IP fix updates |
| `SnapPublished` | snap_id, sender_id | A public snap is shared |
| `MapPointAdded` | layer, id | Any new point arrives |

### Python example

```python
import dbus, json

bus   = dbus.SessionBus()
hub   = bus.get_object("org.snapclone.DataHub", "/org/snapclone/DataHub")
iface = dbus.Interface(hub, "org.snapclone.DataHub1")

# Get our location
loc = json.loads(iface.GetCurrentLocation())
print(f"We are at {loc['lat']}, {loc['lon']}")

# Push a weather forecast you fetched from the internet
iface.SubmitMapPoint(json.dumps({
    "layer": "weather_station",
    "lat": 45.5017, "lon": -73.5673,
    "source_app": "com.myforecastapp",
    "ttl_secs": 3600,
    "payload": {"temp_c": 18.0, "description": "Rain expected at 3pm"}
}))

# Subscribe to location updates → upload to your server
def on_location(lat, lon, acc, ssid):
    my_server.upload(lat=lat, lon=lon)

hub.connect_to_signal("LocationChanged", on_location,
                       dbus_interface="org.snapclone.DataHub1")
```

### Shell example (gdbus)

```bash
# Query current location
gdbus call --session \
  --dest org.snapclone.DataHub \
  --object-path /org/snapclone/DataHub \
  --method org.snapclone.DataHub1.GetCurrentLocation

# Submit a pin (no Python needed)
gdbus call --session \
  --dest org.snapclone.DataHub \
  --object-path /org/snapclone/DataHub \
  --method org.snapclone.DataHub1.SubmitMapPoint \
  '{"layer":"weather_station","lat":45.5,"lon":-73.5,"source_app":"myapp",\
    "ttl_secs":300,"payload":{"temp_c":21}}'
```

---

## Channel B: XDG inbox folder

**Path:** `~/.local/share/snapclone/inbox/`
**Processed:** `~/.local/share/snapclone/inbox/processed/`

Drop any `.json` file here. SnapClone picks it up within 2 seconds,
inserts it into the point store (so it appears on the PollinatorPage radar),
and moves the file to `processed/`. Broken files go to `processed/broken/`.

**No library needed.** Shell scripts, cron jobs, gpsd postscripts, anything.
The files survive SnapClone restarts — they're processed when it next starts.

### Shell example

```bash
#!/bin/sh
# weather-submit.sh — run from cron or after fetching a forecast
INBOX="$HOME/.local/share/snapclone/inbox"
cat > "$INBOX/weather-$(date +%s).json" << EOF
{
  "layer": "weather_station",
  "lat": 45.5017,
  "lon": -73.5673,
  "source_app": "my-weather-script",
  "ttl_secs": 1800,
  "payload": {
    "temp_c": 21.4,
    "humidity_pct": 62,
    "description": "Partly cloudy"
  }
}
EOF
```

### gpsd → SnapClone bridge (10 lines)

```bash
#!/bin/sh
INBOX="$HOME/.local/share/snapclone/inbox"
gpspipe -w | while IFS= read -r line; do
  lat=$(echo "$line" | jq -r '.lat // empty' 2>/dev/null)
  lon=$(echo "$line" | jq -r '.lon // empty' 2>/dev/null)
  [ -z "$lat" ] && continue
  printf '{"layer":"sensor_reading","lat":%s,"lon":%s,"source_app":"gpsd",\
    "ttl_secs":60,"payload":{"sensor_type":"gps","value":0,"unit":"fix"}}\n' \
    "$lat" "$lon" > "$INBOX/gps-$(date +%s%N).json"
done
```

### Python — no D-Bus needed

```python
import json, os, time, pathlib

INBOX = pathlib.Path.home() / ".local/share/snapclone/inbox"
INBOX.mkdir(parents=True, exist_ok=True)

def submit(layer, lat, lon, payload, ttl=300, app="myapp"):
    point = {"layer": layer, "lat": lat, "lon": lon,
             "source_app": app, "ttl_secs": ttl, "payload": payload}
    fname = INBOX / f"{layer}-{int(time.time()*1000)}.json"
    fname.write_text(json.dumps(point))

submit("weather_station", 45.5017, -73.5673,
       {"temp_c": 21.4, "description": "Partly cloudy"})
```

---

## Channel C: Outbound stream socket

**Path:** `$XDG_RUNTIME_DIR/snapclone-data.sock` (e.g. `/run/user/1000/snapclone-data.sock`)
**Mode:** 0600 (owner-only)
**Direction:** ONE-WAY OUTBOUND. SnapClone writes; you read.

Connect and receive a continuous stream of newline-delimited JSON events.
Slow consumers get 250ms before being dropped — the stream continues for others.

### Events

```jsonc
// Hello frame (on connect)
{"event":"hello","version":"1","events":["location","snap_published","weather","map_point","ping"]}

// Our GPS/IP location updated
{"event":"location","lat":45.50,"lon":-73.56,"accuracy_m":15,"ssid":"MyWifi","ts":"..."}

// A public snap was shared
{"event":"snap_published","snap_id":"...","sender_id":"sc-aabbccdd","ts":"..."}

// A new map point arrived (from any source, any layer)
{"event":"map_point","layer":"weather_station","id":"...","lat":45.5,"lon":-73.5,
 "source_app":"myapp","payload":{"temp_c":21.4},"ts":"..."}
```

### Shell — live location upload

```bash
# Upload every location fix to your server
nc -U /run/user/1000/snapclone-data.sock | \
  jq -c 'select(.event == "location")' | \
  while IFS= read -r line; do
    curl -sS -X POST https://myserver.example.com/location \
      -H "Content-Type: application/json" \
      -d "$line"
  done
```

### Python — stream consumer

```python
import socket, json

sock = socket.socket(socket.AF_UNIX, socket.SOCK_STREAM)
sock.connect("/run/user/1000/snapclone-data.sock")

buf = b""
while True:
    buf += sock.recv(4096)
    while b"\n" in buf:
        line, buf = buf.split(b"\n", 1)
        event = json.loads(line)
        if event["event"] == "location":
            upload_to_server(event["lat"], event["lon"])
        elif event["event"] == "snap_published":
            notify_followers(event["snap_id"])
```

### Rust — async consumer

```rust
use snapclone_databus::outbound::stream_socket_path;
use tokio::{io::{AsyncBufReadExt, BufReader}, net::UnixStream};

let stream = UnixStream::connect(stream_socket_path()).await?;
let mut lines = BufReader::new(stream).lines();
while let Some(line) = lines.next_line().await? {
    let event: serde_json::Value = serde_json::from_str(&line)?;
    println!("{:?}", event["event"]);
}
```

---

## Supported layers

| Layer id | Payload fields | Suggested TTL |
|---|---|---|
| `public_snap` | sender_id, content_hash, caption, media_type | 300s (snap duration) |
| `weather_station` | temp_c, humidity_pct, wind_kmh, description | 1800s |
| `public_photo` | url, caption, author | 86400s |
| `shopping_directory` | name, category, open_now, hours, url | 3600s |
| `sensor_reading` | sensor_type, value, unit, quality | 60s |
| `custom_*` | anything | your choice |

---

## Key principle

**SnapClone does not phone home.** All data flowing through these channels
was put there by a local app on your device. The network-aware sibling app
decides what to fetch from the internet and what to upload; SnapClone is
purely a local hub and display layer.

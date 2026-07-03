# Location Privacy Design

## What is stored and why

SnapClone optionally attaches GPS coordinates to outgoing snaps. The goal is two things:

- **Snap geotagging**: the recipient can see approximately where a snap was taken.
- **Last-known location**: if a peer sent you a snap recently, you can see roughly where they were at the time — useful for "are they nearby?" without requiring live tracking.

## What is NOT done

- No background location polling while the app is closed.
- No location sharing except as a side effect of sending a snap (user-initiated).
- No centralised server receives coordinates. They travel only peer-to-peer, encrypted.
- No history. The DB keeps **one row per peer**, overwritten on each new snap. There is no timeline.

## Data model

```
peer_locations (peer_id TEXT PK, lat REAL, lon REAL, accuracy_m REAL,
                ssid TEXT, recorded_at INTEGER ms, source TEXT)
```

- `source` is `'gps'` (own fix, written on location update) or `'snap'` (peer's fix, extracted from a received snap).
- Entries older than **5 minutes** are treated as unavailable by the application (`loadPeerLocation(maxAgeMs=300000)`).
- `purgeStaleLocations()` runs on startup and removes anything older than 10 minutes.

## Precision

Accuracy is rounded to the nearest **100 metres** before storage, so the DB never contains a precise fix. On the wire, coordinates are encoded as `float32` (~11 m precision at mid-latitudes), which is further degraded by the 100 m rounding.

## Wire format

Snap v3 adds an optional location block after the duration field:

```
hasLoc: uint8  (0 = absent, 1 = present)
[lat:  float32 BE]
[lon:  float32 BE]
[acc:  float32 BE]  -- metres
```

Old clients (v1/v2) receive snaps without this block and ignore it gracefully. New clients receiving old snaps treat location as absent.

## User control

Location is attached only when `LocationManager::hasFreshFix()` returns true — i.e. only if the device has obtained a GPS fix in the last 5 minutes. If location is unavailable or the device has no `QGeoPositionInfoSource`, snaps are sent without coordinates.

A future settings toggle (`Share location in snaps`) can gate the `snap.locLat` attachment in `MainWindow` without any protocol change.

## Threat model

| Threat | Mitigation |
|---|---|
| Passive network observer | Snap payload is AES-256-GCM encrypted; coordinates are inside the ciphertext |
| Compromised device / forensics | 5-minute TTL + 100 m rounding limits historical value; no movement history |
| Recipient storing coordinates indefinitely | Out of scope — same as any other snap content the recipient chooses to save |
| Peer inferring your location without your consent | Coordinates are only present if you send a snap; you choose when to send |

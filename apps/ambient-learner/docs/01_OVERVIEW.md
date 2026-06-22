# Overview

## What

`ambient-learner` is a Linux desktop app — primarily targeted at
mobile Linux phones — that:

1. **Polls all thermal sensors on the device** (CPU, GPU, battery,
   modem, charger, etc.).
2. **Predicts true ambient (room) temperature** from those readings
   using a learned model. The model is local; no cloud calls.
3. **Exposes its data over local IPC** to other apps on the device.
4. **Publishes a 2D screen-temperature heat map** to `/run` for use
   cases like rain/glove touch correction.

It also has features for:
- Per-location forecasting (small NN per saved place)
- Pressure / humidity / light from IIO sensors
- Calibration: user enters a reference temperature, model adjusts
- NFC-based device-to-device sync (legacy, partially working)
- Federation with a separate `snapclone-bus` daemon when present

## What it isn't

- **A weather app.** Weather APIs are network-only; we don't fetch.
  A separate "weather sync" app could pull forecasts and drop them
  into our XDG inbox, where they'd appear on our map.
- **A cloud service client.** No accounts, no API keys.
- **A daemon.** It's a desktop app with a UI. `--headless` exists
  for tests but isn't a supported production mode.
- **A precision instrument.** Sensor temperatures are read from
  Linux sysfs, which itself reads from on-chip junction sensors
  intended for thermal throttling, not metrology. We do our best
  with calibration; absolute accuracy is ±2°C in good conditions.

## Why

Phones know their internal temperature very well (the thermal
governor depends on it) but apps don't have access to a "what's
the room like" reading. A user with a phone in their hand has a
remarkably accurate thermometer that nobody uses.

The neural field model learns the relationship between the device's
internal sensors and an externally-known true temperature (provided
by the user during calibration). Once trained, it estimates ambient
without further calibration.

## Use cases

| Use case | Channel |
|---|---|
| Glance at room temp from another app | D-Bus `GetCurrentTemperature` |
| Live temperature feed to a chat app | Unix socket newline-JSON |
| Network app pushes forecast for our map | XDG inbox JSON file |
| Touch-input filter for rain/gloves | Screen-temp bitfield in `/run` |
| Cross-device data sync | NFC sync, or via a sibling sync app |

## Primary target

Mobile Linux phones (PinePhone, PinePhone Pro, Librem 5, FxTec Pro1,
and similar) running Mobian, postmarketOS, Plasma Mobile, or Phosh.
Qt/QML UI tested on these; GTK UI works on desktop and on Phosh.

## Status

Reasonably stable for the core features. Public IPC is contract-versioned
and we won't break the protocol without a major bump. The screen-temp
bitfield is well-defined enough for external consumers but has
device-specific tuning that's still being figured out.

See `04_TODO.md` for what's open.

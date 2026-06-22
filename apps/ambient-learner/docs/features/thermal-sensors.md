# Thermal sensors

## Where readings come from

Two sysfs trees are scanned at startup:

```
/sys/class/thermal/thermal_zone*/temp     # standard kernel thermal zones
/sys/class/hwmon/hwmon*/temp*_input        # hardware monitors (CPU, etc.)
```

Each file is opened once and held open for the life of the process.
On every poll tick, we `seek(0)` and `read()` to get the current
value — kernel sysfs supports this pattern for cheap re-reads.

## Sensor type classification

The `type` sibling file is read once at startup to classify each
sensor:

| Type string contains | Classified as |
|---|---|
| `cpu`, `x86_pkg_temp`, `coretemp`, `k10temp` | CPU |
| `gpu`, `amdgpu`, `nvidia` | GPU |
| `battery`, `bat` | Battery |
| `acpi` | Ambient |
| anything else | Unknown |

This classification is used by the screen-temp layout to map
sensors to physical positions, and by the UI to color-code
readings.

## Unit detection — the 900 °C problem

The Linux sysfs convention says thermal values are in **millidegrees
Celsius** (`1 °C = 1000`). On desktop and most laptops, this is what
you get.

**Mobile vendor kernels are inconsistent.** Some SoCs report in
microdegrees (`1 °C = 1_000_000`), some in raw degrees (`1 °C = 1`).
A naïve `value / 1000` gives:

- 900 °C (microdegrees mis-read as millidegrees)
- 0.025 °C (raw degrees mis-read as millidegrees)

Both are clearly wrong. The 900 °C case is what surfaced on the
53-sensor mobile test.

### How we handle it

`normalize_thermal_reading(raw: i64) -> f32` (in `main.rs`) tries
three candidate divisors and picks the first whose result lands in
the physical silicon range (−40 °C to +125 °C):

```rust
let candidates = [
    raw as f32 / 1000.0,         // millidegrees (standard)
    raw as f32 / 1_000_000.0,    // microdegrees (some vendor kernels)
    raw as f32,                  // raw degrees (rare)
];
```

If none fit, we clamp to the boundary rather than feed a garbage
value into the ML model.

### Known limitation

A value like `65` is ambiguous: 0.065 °C (millidegrees) is in range
and wins, but the sensor might actually mean 65 °C (raw). We pick
the conservative interpretation because:

1. A sleeping sensor reading ~0 °C is plausible.
2. A "raw degrees" sensor at exactly 65 is also plausible but
   would have been clear from context if we had any.

The alternative (always pick the larger interpretation) would
trigger thermal throttling responses to nonsense readings. The
current choice is the less-bad failure mode.

## Polling cadence

The sensor worker runs at 100 ms intervals. The loop sleeps until
the next deadline, then polls — it does *not* wake every 10 ms to
check the clock (which an earlier version did). Power impact on a
phone is meaningful: a 100 ms loop with one wakeup is ~10× cheaper
than the same loop with ten.

## Stuck-sensor handling

`ThermalPoller::try_lock()` runs once at startup and confirms each
sensor returns *some* value. If a sensor goes quiet later, the
poll loop returns `None` from that tick and the model worker
treats the previous reading as stale. After `SENSOR_MAX_FAILURES`
consecutive failures the sensor is marked `SensorHealth::Unavailable`
and the UI shows a warning badge.

## Hot-plug

**Not supported.** Sensor count is fixed at startup. If a sensor
disappears (rare — it'd take a hardware removal) the poller will
just keep getting `EBADF` or similar. If a new sensor *appears*
(e.g., a USB-C charger that announces its own thermal zone), we
won't see it without a restart.

Adding hot-plug would require resizing the model, history buffer,
and `WorldState.sensors` array atomically. Not trivial; not on the
near-term TODO.

## Per-device tuning

Different phones have different sensor populations:

| Device | Approx. sensor count |
|---|---|
| PinePhone | ~8 |
| PinePhone Pro | ~12 |
| Librem 5 | ~15 |
| OnePlus 6 (postmarketOS) | ~24 |
| Recent vendor kernels | 40-80 |

The screen-temp layout (`ScreenLayout::generic_phone()`) is tuned
for ~10 sensors. Devices at the high end have many sensors clustered
in one area (e.g., 8 sensors all on the SoC die) — their layout
config should reflect that for sharper reconstructions, but the
generic config still works.

A real per-device config is on the TODO; today the generic config
is good enough that no individual phone is broken by its absence.

## Files

- `src/main.rs::ThermalPoller` — the poller itself
- `src/main.rs::normalize_thermal_reading` — unit detection
- `src/systems/sensor_system.rs` — the worker loop
- `src/world_state.rs::SensorHealth` — health enum

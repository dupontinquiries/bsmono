# HANDOFF — r336-weather bin wiring (PROPOSE, do not apply in-sandbox)

**WID:** `r336-weather` · **Pure deliverable:** `world::weather` (additive — the global
weather state machine). Per SHARED_RULES §2 the worker does not touch the bin/overlay; the
coordinator wires this on-device.

## What the model gives the bin
- `GlobalWeather { current: WeatherKind, intensity01, ticks_remaining, phase }`
- `GlobalWeather::new(seed, temp01, humidity01) -> GlobalWeather` — opening spell.
- `GlobalWeather::step(self, dt, seed, temp01, humidity01) -> GlobalWeather` — advance.
- `precip_for(&GlobalWeather, temp01) -> Precip { rain01, snow01 }`.

All pure/deterministic; the bin supplies `dt`, the world `seed`, and the local climate.

## Proposed tick (session/world update, not per-frame)
Weather is a slow, world-wide regime — step it on the sim tick (or a fraction of it), not the
render loop. Transitions are infrequent (spell durations are hundreds of ticks), so this is cheap.

```rust
// once, when the world loads:
let (temp01, humidity01) = climate_at_world_focus();   // see below
self.weather = world::weather::GlobalWeather::new(self.world_seed, temp01, humidity01);

// each sim tick (guard on the existing enable gate):
if world::weather::weather_enabled() {
    let (temp01, humidity01) = climate_at_world_focus();
    self.weather = self.weather.step(dt_ticks, self.world_seed, temp01, humidity01);
}
```

`climate_at_world_focus()` = the temperature/humidity for the world (or the player's region)
from `world::biome_temp_map` — the same `[0,1]` climate scale `snow_fraction` already uses.
A single global sample is fine for a *global* regime; per-region variation stays in the field
model (`sample`).

## Proposed sky / particle hookup
```rust
let p = world::weather::precip_for(&self.weather, temp01);
// p.rain01 / p.snow01 ∈ [0,1] drive the precipitation particle spawn rates
// (reuse the WeatherState::liquid_rain / snow_amount consumers already in the renderer).
spawn_rain(p.rain01);
spawn_snow(p.snow01);

// intensity + kind drive sky dimming / cloud cover / audio:
let dim = self.weather.intensity01;                 // 0 clear .. 1 storm
match self.weather.current {
    WeatherKind::Storm => { /* darkest sky, wind up, thunder via existing lightning_flash */ }
    WeatherKind::Rain | WeatherKind::Snow => { /* overcast + precip */ }
    WeatherKind::Fog => { /* raise fog density (ties to T-fog) */ }
    WeatherKind::Overcast => { /* grey sky, no precip */ }
    WeatherKind::Clear => { /* full sky */ }
}
```

## Relationship to the existing field model (`sample`)
`sample(time, humidity, temperature)` (continuous noise) still works for fine local detail.
`GlobalWeather` sets the **world-wide regime**; the coordinator decides the blend. Recommended:
`GlobalWeather` is authoritative for the regime/precip amount, and the field helpers
(`cloud_cover`, `fog`, `wind`, `lightning_flash`) layer local texture on top — drive lightning
from `precip_for(..).rain01` so strikes only happen in rain/storm spells.

## Apply-kind note
Toggling/transitioning weather is **world-state**, not a render-pipeline change — it needs no
swapchain/pipeline rebuild. Update particle emitters + sky uniforms live. (Flagged per the
`apply_kind` convention from r330-menus.)

## Tuning knobs (coordinator)
`SNOW_TEMP_LO/HI`, `RAIN_HUMIDITY_LO/HI`, `STORM_HUMIDITY_LO/HI`, and the per-kind
`duration_band`/`intensity_band` tables in `world/weather.rs` (under `// r336-weather`) are the
dials for how often/long each kind appears. Durations are in caller ticks — pick tick = second
or frame on wiring.

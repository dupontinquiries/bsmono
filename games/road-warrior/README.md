# Road Warrior

A from-scratch Rust reimplementation of a 2014-era side-view combat racer
(the one with the Behemoth, Pillager, six-wheeled flip specialist, and a
fast-but-hard-to-flip red sedan). Not a port — the original is closed-source
and off the store — this is a new engine built to match the feel: drive,
launch off ramps, flip mid-air for nitro, cross gaps, fight bosses at the end
of each level.

Targets mobile Linux (Fairphone 5, PinePhone, postmarketOS/Asahi) and desktop
Linux, using gyro tilt for flip control on-device and keyboard as a desktop
fallback.

## Project structure

- **`core/`** — headless game engine. Physics, terrain, boss AI, level data,
  save/progression, campaign sequencing. Pure Rust, no rendering or windowing
  dependencies, fully unit-tested (`cargo test -p road_warrior_core`). Every
  file in this project's history that's been genuinely load-bearing has lived
  here; it's the part that's cheap to verify and safe to build on.
- **`shell/`** — the actual game: eframe/egui + wgpu for rendering, zbus for
  gyro input via `iio-sensor-proxy`. This is where GPU pipeline setup, input
  handling, and asset loading live.

The split exists because physics/AI/save-logic and rendering/input have
completely different testing needs — one can run headless in CI or a
sandbox with no display, the other needs a real GPU and window. Neither
should have to know about the other's internals; `core` has zero rendering
dependencies by design.

## Building

```
cargo build --release -p shell
```

Needs a current stable Rust toolchain (1.85+ — current `eframe`/`wgpu`/`zbus`
require edition2024). See `BUILDING.md` for what's actually been verified,
what broke on first compile and why, and troubleshooting if you hit
something new.

`Cargo.lock` is committed on purpose: it pins the exact dependency versions
already built and run this session. Delete it only if you want newer patches
and are prepared to re-verify.

## Running

```
cargo run --release -p shell
```

Requires a Wayland or X11 session and a Vulkan-capable GPU (or a software
Vulkan implementation like lavapipe for testing without a GPU). For gyro
input, `iio-sensor-proxy` needs to be running on the target device.

### Controls

| Input | Action |
|---|---|
| `←` / `→` | Throttle (grounded) / flip control (airborne); on the title, pick a car |
| `Space` | Jump |
| `N` | Activate nitro (requires banked nitro from a clean landing) |
| `Enter` | Start (title), resume (pause), continue (campaign complete) |
| `Esc` | Pause; on the pause screen, resume |
| `M` | Sound on/off |
| Gyro tilt (on-device) | Flip control while airborne, in addition to keyboard |

Left/Right does double duty because a vehicle can't be both grounded and
airborne — throttle has no effect in the air, tilt has no effect on the
ground, so there's no actual conflict.

Sound is synthesized (no assets): an engine drone that follows speed and
throttle, a voice per moment (launch, landing, hit, ram, nitro, dry tank) and
two stings (a run lost, a level done). It needs an audio output and, on
Linux, ALSA headers to build (`libasound2-dev`); `cargo build
--no-default-features -p shell` is the silent binary with no audio
dependency, and a machine with no output device runs silent. The voices have
not been heard by the session that wrote them (`shell/src/audio.rs`).

### Debug env vars (dev-only, compiled out of release builds)

- `RW_DEBUG_WEAK_BOSS=1` — every boss spawns with 10 HP instead of its real
  health pool, so the level-complete → unlock → save → next-level chain can
  be exercised without grinding a 1000+ HP boss down over a dozen ram passes.
- `RW_DEBUG_LOW_HP=1` — vehicle starts at 1 HP, so the respawn/retry-level
  flow triggers on the first hit instead of waiting for organic damage.

Both are gated behind `cfg!(debug_assertions)` — setting them against a
`--release` build has no effect, by construction, not just convention.

## Current status

Built and verified this session against real rendered output (Xvfb +
lavapipe software Vulkan, screenshotted at each step), not just logic:
physics, terrain-following, all three levels, boss combat, ramming,
jump/flip, nitro, camera, save/load, car unlocks, best times, full campaign
completion, and death/respawn all work end-to-end. 64 unit tests across the
workspace, all passing (`cargo test --workspace`).

**Known gaps, not yet addressed:**
- Real gyro input has never been tested on actual hardware — only that the
  keyboard fallback works and that the D-Bus code compiles. See `gyro.rs`
  for what's unverified about the `net.hadess.SensorProxy` interface.
- Car and boss art is a placeholder (colored rectangles drawn procedurally
  in `scene.wgsl`) — no sprites yet.
- ~~No campaign-complete screen; finishing the last level just shows a toast.~~
  (c21: a title with the car select, a pause screen and a campaign-complete
  screen -- `shell/src/flow.rs`.)
- No death penalty beyond retrying the current level — worth deciding if
  that's the right convention (vs. lives, or restarting the campaign).

## Levels

Levels are data (`shell/assets/*.json`), not code — `LevelData` in
`core/src/level.rs` deserializes directly into the same `Track`/`Boss` types
the physics engine runs on, so a new level is a JSON file, not a rebuild.
`Campaign` (`core/src/campaign.rs`) sequences a list of them and tracks
progression.

# Sensor controls + games consolidation — status & plan

## Done (verified)
- **shared/sensor-kit** (4 tests): device tilt/orientation → an impulse-based 2D control vector, the
  model proven in gyro-ball. `parse_monitor_sensor` (iio-sensor-proxy stream on postmarketOS/
  Fairphone), `TiltControl` (apply events → per-frame `tick` folds impulses into velocity under
  friction — lighter while a tilt is held, heavier when neutral). Reuses `touchkit::Vec2` (no second
  vector type). Source-agnostic: a macOS CoreMotion / Android SensorManager bridge feeds the same
  events. **This is the shared "sensor controls for all games".**
- **games/gyroball**: the gyro-ball prototype landed (Python, monitor-sensor tilt-maze + LAN).

## Decisions needed before I proceed
1. **Game duplicates (the "move to games/").** apps/ and games/ hold *diverged* versions:
   `apps/unendlich` (29M) vs `games/unendlich` (40M); `apps/cybertron` (5.7M) vs `games/blackstar`
   (94M, the archive rename). These are NOT the same tree — a blind move would lose work. Which is
   canonical per game (keep apps/, keep games/, or merge)? Then I consolidate under games/.
2. **macOS "fixup" — need the specifics.** What's failing (build? window/surface? sensors? Metal
   backend?) and ideally the error. The Rust games are wgpu (cross-platform); the likely spots are
   the Metal surface config, `raw-window-handle` on macOS, and the sensor bridge (CoreMotion vs the
   Linux monitor-sensor path).

## Fairphone/Plasma Mobile stutter — root cause + fix path
gpu-kit already defaults to `Present::Vsync` → `Fifo` (the present mode that lets the compositor pace
you instead of fighting it). But the games don't use gpu-kit — they configure wgpu directly, almost
certainly on Mailbox/Immediate. **Fix:** route each game's surface present-mode through
`gpu_kit::Policy::default().pick_present_mode(surface_caps.present_modes)` so it resolves to Fifo on
Wayland. If stutter persists under Fifo, it's redraw-on-timer fighting the frame callback → drive
redraws from the compositor's frame callback / winit `RedrawRequested`, not a free-running loop. Both
are per-game edits and need on-device confirmation (can't repro headless).

## Sequence
1. ✅ sensor-kit (shared tilt/gyro control)
2. ✅ land gyroball
3. wire sensor-kit + touchkit into each game's input (unified controls) — per game
4. route each game's present mode through gpu-kit Vsync (stutter) — per game, on-device
5. consolidate games under games/ — after the duplicate decision
6. macOS fixups — after specifics
7. port gyroball Python → a Rust game on sensor-kit (optional; the prototype runs as-is)

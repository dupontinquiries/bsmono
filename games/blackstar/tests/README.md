# Test & tuning harnesses

Unit tests and simulation harnesses, all registered with ctest and runnable via
`../run_harnesses.sh` from the repo root.

## Simulation / tuning harnesses (headless, metric-reporting)
- **test_movesim** - movement tuning harness. Re-runs the SHARED movement core
  (`exengine/move.h`, the same code the game uses) across scenarios and prints
  metrics: drift sweep (materials x vehicles), flight ceiling/terminal speed,
  jump apex/range, 0-200u speed table, obstacle navigation, crossfire damage
  avoidance, and group spread. Use it to tune feel/material/vehicle constants and
  see the effect immediately, without a window.
- **harness_step** - terrain stepping/collision harness; verifies box-step
  traversal stays physically smooth across generated courses.

## Unit tests
transform, netcode, gameplay, element, menu, vfx, gait, weapon, encounter,
pickup, custom, mapgen, waves, class, feel, material, and more - one per module.

Run everything: `./run_harnesses.sh`

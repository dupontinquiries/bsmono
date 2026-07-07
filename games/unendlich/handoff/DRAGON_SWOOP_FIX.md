# Why dragons fly too low & don't swoop — and the fix (r625)

## Root cause (both complaints are one bug)
`sim::dragon_mode::decide` computes the dive flag as:
```rust
dive: in_air && s.target_below && matches!(s.activity, Activity::Alert),
```
It only stoops when the prey is **already below** the dragon, and `target_altitude`
is a fixed cruise height that is **never raised for hunting** (defaults to 0.0 in
`DragonBrain`, and the bin sets a low cruise). So:
- dragons cruise low → prey is at/above their level → `target_below` is false →
- the dive never fires → they "don't swoop."

There is no *climb-above-the-prey-then-dive* behaviour — the raptor stoop.

## Fix: `sim::flight_phase` (this build)
A reusable flight FSM (Ground/Takeoff/Climb/Cruise/Soar/Glide/**Approach**/**Stoop**/
Descend) that, when `hunting` and the prey is within `STOOP_RANGE` (60 m):
- **Approach**: if not yet above the prey, seek `prey_altitude + STOOP_HEIGHT`
  (25 m) — i.e. **climb higher** to set up the stoop.
- **Stoop**: once `above_prey && aligned`, dive (`dive = true`) toward the prey.

It returns `FlightIntent { phase, target_altitude, dive }`. It's generic —
dragons and any flying fauna use it.

## Wiring into `dragon_brain` (bin/engine)
`dragon_brain::think` currently passes a static `target_altitude` and gets `dive`
from `dragon_mode`. Feed it through `flight_phase` when there's a prey target:
```rust
use unendlich_engine::sim::flight_phase::{self, FlightSenses};

let fi = flight_phase::update(&FlightSenses {
    grounded:       !in_air,
    wants_fly:      mode_step.air.wants_fly,
    airspeed:       st.air.airspeed,
    stall_speed:    dragon_flight::stall_speed(&fp),
    altitude:       st.air.altitude,
    cruise_altitude: s.target_altitude.max(BASE_CRUISE), // give a real default!
    energy01:       s.stamina01,
    updraft:        s.updraft,
    hunting:        s.has_prey,
    prey_altitude:  s.prey_altitude,
    prey_range:     s.prey_range,
    above_prey:     st.air.altitude > s.prey_altitude + 4.0,
    aligned:        s.prey_horizontal_dist < 6.0,
});
// use fi.target_altitude for the climb/altitude controller,
// and fi.dive to trigger the dive/`sim::dive` power-dive.
```
Key changes that fix the symptoms:
1. **Give a real cruise altitude** — set `DragonBrain.target_altitude`'s default
   (and the bin's) to something like 40–80 m instead of 0, so dragons don't skim
   the ground.
2. **Route `target_altitude` through `flight_phase`** so it *rises to prey +
   STOOP_HEIGHT* during a hunt (the dragon gains height to stoop).
3. **Take `dive` from `flight_phase`** (Approach→Stoop) instead of the
   below-only check — now the dragon climbs, positions overhead, then swoops.
4. On `Stoop`, drive the descent with `sim::dive` (`dive_terminal`/`dive_step`)
   for the fast folded-wing plunge.

## Other fauna
Flying fauna (the bird/bat body plans) can call `flight_phase::update` the same
way for perch→takeoff→cruise→soar and, for raptor species, approach→stoop hunts.
Pair with `sim::wing_loading` (stall speed / regime) and `sim::dragon_energy`
(the energy reserve that forces Descend).

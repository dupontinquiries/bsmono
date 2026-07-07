# Flight-mode FSM (r675) — sim::flight_modes

Pure core from `04_Creatures_and_Dragons/flying_animal_behaviour.docx` §2.1: the
seven flight modes (powered flapping, bounding, undulating, thermal/ridge/dynamic
soaring, stooping) + a plain glide, with the doc's wind thresholds
(W≥0.10 thermal / 0.20 ridge / 0.50 dynamic) and relative energy costs
(bounding 0.85, undulating 0.70, soaring ~0.05, stoop 0.0). `wing_loading` gives
the size envelope; this is the per-tick behavioural choice on top of it.

API: `select_mode(&Env, &Caps, energy01, covering_distance) -> FlightMode`,
`best_soar`, `FlightMode::{power_factor, is_soaring}`, and `Caps::{small_bird,
generic, soarer}`. 6 tests green standalone.

## Wiring seam (NOT wired yet — needs the flight-state code mapped)
A flying `Roamer` (`flying`/`flight_phase` fields) would, each tick:
1. build `Env` — `wind_w` from the weather cell wind (`w.cell.wind`, see the
   `[weather]` log site), `thermal`/`ridge` from updraft detection (updrafts
   already "visibly form over fires", bin ~1187), `committed_dive` from the
   dragon's dive/hunt state, `descending_to_target` from its approach state;
2. pick `Caps` by `CreatureKind` (Dragon → `soarer`, small fauna → `small_bird`);
3. `select_mode(&env, &caps, vitality01, covering_distance)`;
4. apply: multiply the flight energy drain by `mode.power_factor()`, and when
   `mode.is_soaring()` allow altitude gain (feed the existing flight_phase/soar
   path) instead of costing energy.

Storing the chosen `FlightMode` on the Roamer (new field) lets the renderer pick
the wing animation (flap vs. fixed-wing glide vs. folded stoop). This is a
gameplay-affecting integration into the dragon flight cycle, so it wants a
careful pass + playtest rather than a blind edit.

# blackstar-rs — working notes

Running log from an extended session. Organized as: what's fixed and verified,
what's found but not fixed (with exact repro), and design proposals not yet
built. Update this file rather than losing findings to chat history.

Build/test baseline throughout: `cargo check`/`cargo test --bin skydribble`
under `--no-default-features`, `--features audio`, and default. 1256 tests
throughout. Warning count started at 335 (pre-existing, unrelated to this
session) and sits at 337 as of the last tooling addition — the +2 are
`align::diagnose`/`diagnose_unmatched`, dev-tool-only functions not called
from the shipped binary, same category as the pre-existing 335.

---

## Fixed and verified this session

- **Fold cinematics** (`camera.rs`, `sound.rs`, `main.rs::advance_xform`):
  director's-camera mode (`CamMode::Cinematic`) during a fold, arc speed
  tied to `content::Chassis::fold_spin` (previously dead data), servo-sweep
  + clank sound cues, spark bursts tinted from the chassis's own most
  saturated body-prim color. Tests: 5 new in `camera.rs`, 1 in `xform.rs`.
- **`foldaudit::SAMPLES` was 24 (~18 Hz) against real ~60 Hz playback** —
  manufactured false "pop" faults on smooth, fast, ordinary motion. Fixed
  to 120. Proven with a measurement (constant 4.54 u/s velocity across a
  16x sample-density range) and a regression test
  (`constant_velocity_motion_does_not_read_as_a_pop`) that fails on the old
  value and passes on the new one.
- **617 chassis pose files had stale/placeholder orientation** —
  `assets/bots/*/orient.json` mostly still carried the pre-fix placeholder
  (`up=[0,1,0]`, `source:"foldview"`) from before commit `b6929d9a`, which
  only touched 424 of what's now 1454 pose files. Re-ran the game's own
  `skydribble --orient sweep --full-only --commit` (only high-confidence
  guesses, deliberately skipping the 493 low-confidence ones). This is
  almost certainly the direct cause of the "funky orientation and ground
  placement" the person reported — confirmed via `hardboil`'s walk poses
  being diagnosed `head-hint: FLIP` with the tool explicitly reporting
  "buries the feet."
- **`skeleton::skeletonize`'s core-selection fallback** picked the single
  biggest prim by point count as "the core" when no prim's centroid was
  close enough to the body's mass-weighted middle — which can select the
  very oversized offending appendage as "core" and swallow everything
  else into it, reporting zero limbs. Fixed to pick the prim closest to
  the mass-weighted middle instead. Reproduced with a minimal synthetic
  case (25 small debris prims + 1 large off-center prim → 0 limbs before,
  22 after) and verified against all 14 existing `skeleton.rs` tests.
- **Hover/hovertank locomotion never declared** (this session, reported
  bug from the base code): `set_car_model`'s `declared` mode-override only
  fired Hover for the five hand-authored hovercars (`Model::hover.is_some()`)
  — never for any chassis whose fleet.json category is `hoverer`/
  `hovertank`. Flight already had the correct treatment
  (`traits.flies()`); hover didn't have the mirror-image check even
  though `Traits::floats()` already existed. Added `Chassis::floats`
  (parallel to the existing `Chassis::flight`), wired it into `declared`
  the same way. Every category-labelled hoverer/hovertank should now
  reliably get real hover physics instead of gambling on the geometry
  detector's untuned skirt heuristic.
- **Correction to an earlier claim in this session:** I said `compact.rs`/
  fold panel compaction was "confirmed dead code, never activated." That
  was wrong — I read a comment describing a past bug without reading far
  enough down the same function to see the fix immediately below it.
  `main.rs` (~line 3013) does set `m.compaction = Some(traits.compaction())`
  live, using real populated per-chassis category data. Correcting the
  record here since I stated the wrong thing as fact earlier.

- **Half of `chassis_traits::Traits` has no live gameplay effect.**
  Checked every field for a real consumer, not just an existing method:
  `lift`→flight and `hover`→hover mode (both live), `armour`→fold
  compaction style (live, see correction above). But `traction` has zero
  callers anywhere except its own unit test (`grip()` is never invoked
  from the game); `mass_bias` has zero callers anywhere; `agility` has
  exactly one real consumer (`guidance.rs`'s evasive "jink" maneuver) but
  it's only reachable through an offline `--guidance` CLI analysis tool,
  never from live play. A tank's high traction and a racer's low one
  currently make no difference to actual handling beyond whatever
  geometry-derived grip `driveline.rs` computes independently.
- **Found and fixed one real regression from the orientation fix itself,
  and caught a false alarm about a much bigger one before reporting it.**
  After the 617-file orientation commit, `refusal_audit --sweep-all`
  showed 7 unify-refused chassis instead of the pre-fix 6. Verified via
  `git stash` (comparing the exact same sweep against the old and new
  asset state) that `tank_lynx_6x6` was the one new regression: its 5
  fold poses went from sharing one (wrong) placeholder orientation to
  getting wildly different independent per-pose guesses — the heuristic
  solver has no cross-pose consistency check, and evaluates each pose
  file in isolation. Reverted just that one chassis's `orient.json`
  files. Then, worried this might be widespread, wrote a script checking
  every one of the 617 changed chassis for differing per-pose up-axis —
  it flagged 165 of 172 chassis with fold poses. That number is **not a
  real problem**: a pose's *native* export axis is expected to vary
  pose-to-pose when poses were authored/exported separately, and the
  solver's entire job is correcting each one into a shared frame — the
  real ground-truth check is whether `align::unify` still succeeds after
  that correction, which is the unify-refused count already re-measured
  above and now matches the pre-fix baseline exactly (248 faulty / 6
  unify-refused) apart from the one already-fixed case. Recording this
  here mainly as a warning to future-me and anyone else: a per-pose
  axis-consistency check on raw orientation *guesses* is not the same
  thing as a correctness check, and I almost reported the wrong number
  as a crisis before checking what it actually meant.
- Fixed a mangled duplicated doc comment above `advance_xform` (cosmetic).
- Corrected two of my own overclaims mid-session: `actuator.rs` is
  superseded/redundant, not a real gap in flight/drive feel (flight/drive
  already have dedicated, tested implementations in `lift.rs`/
  `driveline.rs`); the crotch-corridor silhouette check flip-flopped
  between two of my own fix attempts and is flagged unreliable rather than
  presented as fact.

## New diagnostic tooling (checked into `examples/`)

- `refusal_audit.rs` — runs the *real* fold pipeline (`load_raw_at` +
  `align::unify` + `Morph::build` + `foldaudit::audit`, matching
  `main.rs::build_morph_for_active` exactly) against any chassis or the
  whole roster. `--sweep-all` for a roster pass.
- `sample_density_check.rs` — the tool that proved the `SAMPLES` bug: worst
  per-vertex velocity at rising sample density, to distinguish "smooth
  motion sampled too coarsely" from a real discontinuity.
- `silhouette_audit.rs` — measures the *final* robot pose (not fold
  motion) for overhang beyond the core body and crotch-corridor clutter.
  Overhang check is stable and cross-validated. Crotch check has a known
  false-positive mode on non-biped hulls (documented in-file) and should
  be treated as a lead, not a verdict.
- `trace_compact.rs` — traces `skeleton::skeletonize`/`compact` against a
  real chassis, prim by prim: which prims got claimed as limbs, what tuck
  vector (if any) a named prim got. This is how the skeleton bug above was
  found and the fix verified.

## Real, roster-wide data now available (re-run any time; not static)

- Fold self-intersection ranking — cleanest: `hardboil`, `quickfire`,
  `ironforge`. Worst: `pyrachnid`, `trenchmaster`, `bot_lucifer`,
  `spider_bot` (correlates with limb count, which is reassuring — not
  noise).
- Silhouette overhang — `starlord` (oversized Wing), `void` (oversized
  Wheel + Thruster) flagged with exact prim index/position/size, and
  independently corroborated by the fold self-intersection data for the
  same two chassis.
- 6 chassis fail even the topology check (`align::unify` refuses):
  worth a `refusal_audit.rs --sweep-all` re-run to get current names.

---

## Found, NOT fixed — precise repro, needs either a fix attempt with
## fresh eyes or actual visual/playtest feedback before touching further

- **`skeleton::skeletonize`'s CORE_R (0.42) is still too permissive** on
  real chassis even after the fallback fix above. Confirmed on `starlord`:
  the oversized Wing (prim 34) passes the *primary* `d < CORE_R` check
  directly (23 of 37 prims do), becoming part of "core" itself rather
  than ever being evaluated as a limb — a third, distinct failure mode
  from both the original doc-comment scenario and the fallback bug fixed
  above. A prim can be centroid-central AND still reach far in one
  direction; the current check only looks at the first property. Repro:
  `cargo run --example trace_compact -- assets/bots starlord 34`.
- Added `align::diagnose()` and `align::diagnose_unmatched()` — read-only
  companions to `align::unify()` that report match percentage and the
  exact unmatched (pose, prim) pairs even on refusal (`unify` itself just
  returns `None`, discarding all of this). Used them on the 6
  permanently-refused chassis. **Correcting my own first read of this
  data within the same session**: I initially called
  `animal_bot_runningwind` (49%) and `raidboss_tremor` (48%)
  "near-misses... one or two prim-pairs away." The per-prim breakdown
  shows that's the wrong mental model — for `animal_bot_runningwind`,
  poses 0→1 and 1→2 each have ~48-52 of ~55 prims failing to match
  (near-total mismatch), while 2→3 and 3→4 have only 6-7. That's not
  scattered near-misses; it's the signature of poses 1 and 2 specifically
  being UV-authored differently from poses 0, 3, and 4 — the aggregate
  percentage landing just under 50% is an average of "almost nothing
  matches here" and "almost everything matches there," not "everything's
  slightly off." A real fix would mean re-unwrapping poses 1-2 to agree
  with their neighbors, not a small nudge — a harder, more specific
  problem than my first read suggested. `constructor_heavy_spider_animated`,
  `hoverers_shellstike`, and `jet_hull` remain substantially mismatched
  overall (30-35%), but even they aren't uniform once broken down the
  same way: `constructor_heavy_spider_animated`'s pose 3 is a near-total
  outlier (virtually every prim fails to match) while its pose 4 is
  comparatively clean — a specific wrong mesh/export is a plausible
  cause. `hoverers_shellstike` is the mirror image: pose 4 (the final/
  robot pose) is the near-total outlier, not an early pose. `jet_hull`
  alone is genuinely scattered across all four transitions with no
  standout pose — the one case here that actually matches "generally
  incompatible" rather than "one bad pose."
  Did not touch `MIN_MATCHED` (the 50% threshold) — one global number
  for all 254 chassis, can't verify a change wouldn't let through some
  other chassis that should legitimately fall back to stepping.
- Same tool, same command shape, works for any other chassis —
  `bot_t4b_tank_tread_mech_vehicle_form`, `animal_bot_runningwind`, and
  `pyrachnid` are the only chassis where `silhouette_audit`'s crotch check
  currently reads as plausibly real rather than a non-biped false
  positive; worth tracing with `trace_compact` the same way `starlord`
  was.
- **`orientsim.rs`'s per-pose orientation solver has no cross-pose
  consistency check** — it evaluates each pose file of a fold strip
  completely independently, with no verification that the poses still
  agree with each other (via `align::unify`) after correction. This is
  what caused the `tank_lynx_6x6` regression above, and could recur on
  any future `--commit` run against any chassis. A real fix would run
  `align::unify` per-chassis as a post-check inside the sweep itself and
  refuse to write a chassis's fix if it would newly break unification
  compared to the pre-write state — turning a silent regression into a
  loud one at commit time instead of requiring a manual `git stash` diff
  to find.
- **Only 2 of 34 tank-named chassis are detected as tracked at all**
  (`turret::find`'s `treads.len() >= 2` check). Traced to a precise,
  reproducible cause, not scattered noise: `TREAD_LOW` (0.10) checks a
  candidate prim's top against the body's own vertical half-extent
  (`half.y`), and a genuinely low-profile vehicle — which is what a real
  tank silhouette *is* — has a small `half.y` by design. That makes the
  10% threshold nearly impossible to satisfy for any prim, including
  ones that are otherwise textbook tread shapes. Measured directly on
  `tank` (the plain reference chassis, checked across all 9 of its pose
  files): several prims pass `long` (0.72-1.00, needs >0.55) and `flat`
  cleanly, but every one fails `low` at 0.73-1.00 against a needed <0.10.
  The check is set up to fail hardest on exactly the shapes most
  realistically built like a real tank. Confirmed this isn't a fluke of
  one chassis or one pose — checked with a new tool
  (`examples/check_treads.rs`) against every tank-named chassis in the
  roster, all poses. Real gameplay consequence: `driveline::Shape.tracked`
  (which should give a tank tank-like handling) is false for the large
  majority of tanks, which get generic wheeled-car handling instead.
  Same category of bug as the hover fix earlier — an untuned geometry
  heuristic quietly failing on real assets — but this one is a threshold
  miscalibration inside a single check, not a missing category override,
  and I have not attempted a fix: `TREAD_LOW` interacts with body
  proportions in a way I'd want to check against several very differently
  -shaped tanks (turreted vs. flat-deck, tall vs. squat) before proposing
  a number, exactly the guess-a-threshold trap flagged elsewhere in this
  file. `examples/check_treads.rs` reproduces this on demand.
- **Clarifying what's actually spinning on a tank right now**, since it
  looks like tread motion but isn't: `fittings.rollers` (round, low,
  outboard prims — road wheels/sprockets/idlers) is a *separate* system
  from `fittings.treads` (the flat belt), is correctly wired and
  vehicle-mode-gated (verified earlier this session), and does spin.
  `tank`'s own driving pose has 3 rollers detected. The tread *belt*
  itself has zero animation consumer anywhere in the codebase — checked
  every use of `.treads`, all inside `turret.rs`'s own detection/tests —
  so what reads as "the treads sort of spin" is the round running-gear
  pieces turning while the flat track segment between them stays static.
  Not a conflict between the two systems (roller detection explicitly
  excludes tread-tagged prims); just that only one half of "a tank's
  track assembly" was ever built. A belt animation (UV-scroll or segment
  cycling) is genuinely new work, and is blocked on the `TREAD_LOW`
  detection bug above first — right now there's usually nothing in the
  `treads` list to animate in the first place.
- **Weapon turret aiming has no elevation, only yaw** — `main.rs` (~line
  8020) explicitly documents this as deliberate, not an oversight: "pitching
  a turret needs the mantlet's own axis, which is a second detection and
  not this one." The yaw-tracking itself is solid (correctly pivots about
  the detected ring, not the turret's own mass-centroid, tracks the
  camera's look direction). Relevant to "do the weapons work" — they fire
  correctly (verified earlier, 23 passing tests) but a turret-mounted
  weapon on a chassis with a real elevation range (most tanks) can't aim
  up or down, only side to side. A real fix needs `turret::find` extended
  to also detect a mantlet/elevation axis, which is genuinely a second
  detection problem, not a quick addition to the existing one.
- **Real, roster-wide ground-placement numbers exist, with an honest gap
  in what they mean.** `skydribble --orient sweep`'s per-pose diagnostic
  (the same "seat: ... buries the feet"/"floats it" lines that first
  pointed at the orientation bug) is printed for the whole roster
  regardless of `--low-confidence-only` — confirmed by diffing the two
  sweeps' output, identical except a 4-line summary difference.
  Aggregated: of 1454 total pose diagnostics, 869 (60%) report the body
  floating above ground clearance, 299 (21%) report feet buried, and 286
  (20%) are correctly seated. **What I could not determine**: the tool
  prints no chassis name or pose-file label next to these lines, so
  there's no way from this output alone to tell how many of the 869/299
  are the strip's actual endpoint poses (the ones a player ever sees
  independently placed on the ground) versus mid-fold poses, which
  aren't independently grounded in real gameplay — they're just
  interpolated through between two already-placed ends, so a mid-fold
  pose reporting "floats it" may be completely expected and harmless.
  Reporting the raw 60% figure without that context would be the same
  kind of overclaim already caught twice this session (the axis-
  consistency false alarm, the "near-miss" mischaracterization) — so
  treat this as a real number that needs one more filtering pass
  (tag each diagnostic line with its source file, likely a small change
  to `orientsim.rs`'s `sweep` function itself) before acting on it, not
  as "60% of the roster has a ground-placement bug."
- **Resolved the labeling gap from last pass, with a real, defensible
  number instead of a raw aggregate.** Added a one-line, purely additive
  change to `orientsim.rs`'s existing `seat:` diagnostic — printing the
  pose's own path alongside it — verified against the full 1256-test
  suite (no behavior change, print-only). That let me group by chassis
  and separate strip endpoints (the poses actually placed on the ground
  in real play — `f00` and the strip's last pose) from mid-fold poses
  (interpolated between two already-placed ends, never independently
  grounded, so "floats it" there is often expected and harmless — not
  the 60% crisis the unfiltered aggregate implied).
  **Endpoint poses**: 508 total — 183 float (36%), 81 buried (16%), 244
  correctly seated (48%). **Mid-fold poses**: 762 total — 590 float
  (77%), mostly the expected/harmless case. So the real, actionable
  number is **52% of strip-endpoint poses have a genuine ground-
  placement issue** — still substantial and worth a real fix pass, but
  meaningfully lower than the naive aggregate and now something I can
  stand behind rather than a number I'd have had to walk back later.
- **One more correction on top of the correction above — the real number
  is 33%, not 52%.** The `seat:` diagnostic has zero hover-awareness: it
  flags "floats it" from raw geometry alone, with no exception for
  chassis whose category (`hoverer`/`hovertank`/`flyer`/`flyerf`/`heli`/
  `airship`/`spaceship`) means floating is the *correct*, designed
  behavior, not a bug. Cross-referencing the 508 endpoint poses against
  `fleet.json`'s own category field: 95 of the 183 raw "floats it" cases
  belong to chassis that are supposed to float. Excluding those: 88
  ground-vehicle chassis genuinely float when they shouldn't, 81 bury
  their feet (no category excuses burying — this count is unaffected),
  244 correctly seated. **Real endpoint ground-placement bug rate: 33%**
  (169 of 508), not the 52% before this correction or the 60% before
  that one. Three successive corrections on the same number in two
  turns is itself worth noting: each pass looked done until the next
  obvious question ("does this account for X") turned up something that
  moved the number meaningfully. Worth checking whether there's a
  fourth correction before anyone treats 33% as final — I stopped here
  because I'd run out of category-level explanations I could think to
  check, not because I'm confident there isn't one.
- Left the 493 low-confidence orientation guesses (`orient sweep`,
  without `--full-only`) untouched. They need either a closer per-chassis
  pass or visual confirmation before committing — a bad low-confidence
  guess could replace a placeholder that happened to already be right.

---

## Design proposals — NOT implemented, deliberately

Control-feel and mode-switching decisions need playtesting more than any
other work in this session; implementing blind risks the same
guess-a-threshold trap already hit twice on `foldaudit`/`skeleton`. These
are scoped for whoever picks them up (me, next session, or a person with
hands on a controller).

### Jets: split "fly forward" from "nose down"

**Confirmed exact mechanism, not just a reported feel issue.** In
`main.rs`'s `controls()` (~line 5477):

```rust
throttle: (fwd - rev + ps.throttle + t_thr).clamp(-1.0, 1.0),
pitch: (rev - fwd - ps.stick_y + t_air.map(|v| -v.y).unwrap_or(0.0)).clamp(-1.0, 1.0),
```

`pitch` is derived directly from the same `fwd`/`rev` keyboard inputs as
`throttle`, just negated. This is **deliberate and correct** for ground
vehicles doing a Rocket-League-style aerial trick (holding forward
pitches the nose down to angle into a hit — `ps.stick_y`/`t_air` are
added on top for players with an actual analog stick, so gamepad/touch
already has more nuance). The same formula is reused unconditionally for
`Flight` mode, where a jet needs throttle and pitch to be independent —
that coupling is exactly what reads as "forward and nose-down are the
same button."

**I attempted the obvious fix (zero the `fwd - rev` term when
`self.car.flight` is true) and reverted it**: keyboard has no dedicated
pitch binding at all (checked `input.rs` — only `RollL`/`RollR` exist;
`Action::Transform` etc. don't cover pitch), so zeroing that term leaves
a keyboard flyer with *no way to pitch whatsoever*. That's a worse
regression than the original complaint.

**What actually needs deciding before this can be fixed:** either (a) add
dedicated keyboard pitch-up/pitch-down bindings (a real keybinding/UI
change, `input.rs::Action` + the settings menu + default keymap), or (b)
give Flight mode its own control scheme entirely where, e.g., `SteerL`/
`SteerR` become roll and a currently-unused pair of keys becomes pitch —
which is really the same design question as the VTOL/autoboost mode
proposal below, since both are asking "what does the jet's control
scheme look like," and should probably be answered together rather than
patched twice.

### Jets: single-tap Transform cycles VTOL/hover vs. fast-fly/autoboost

Proposal: tapping `Action::Transform` while already in Flight mode (rather
than triggering vehicle<->robot) cycles between two flight sub-modes:

- **VTOL/hover** — low speed, high control authority, for navigation and
  precision (landing, threading terrain, dogfighting maneuvers).
- **Fast-fly/autoboost** — high speed, committed trajectory, boost fuel
  regenerates continuously or doesn't deplete (distinct from the existing
  hovercar two-tier thrust reserve in `physics::Hover`, which is a
  different mechanic for a different mode — this would need its own
  reserve concept for flight, not reuse of `Hover::base_tank`).

Anchoring: `Action::Transform` is already a real input
(`input.rs::Action::Transform`); the fold system (`xform.rs`) owns what
happens on that button today. This proposal needs a *second* meaning for
the same button conditional on already being in Flight mode without a
fold in progress — needs a design decision on precedence (does it ever
conflict with an actual robot-fold request for a flying chassis?) before
implementation, not just wiring.

### Tanks: siege / roll / bot as a three-way cycle

Same button, same precedence question as above, extended to a third
state. `content::CHASSIS`/`chassis_traits::Traits` already model tank
armor/traction; a "siege" mode would presumably trade mobility for grip
or damage resistance — needs the actual gameplay numbers designed, not
just the state machine.

### Racers: weapon pop-out as a cosmetic transform-lite

Softer proposal already narrowed down in the person's own message: a
transformers-style partial reconfiguration (weapons extend/retract) as
character expression more than a mechanical system, OR tied to
configurable spaceship upgrades. Lowest technical risk of the four
proposals here — it's closer to `skeleton::fold_pack`/`compact`'s existing
partial-fold machinery than a new physics mode. Worth prototyping first
if picking one of these four to actually build.

### Wire `traction` and `mass_bias` into real handling

Lower risk than the control-scheme proposals above — this doesn't touch
input or camera, only `driveline::derive`'s existing formula. But it's
still a real game-balance decision (how much should traction scale grip
relative to what geometry already contributes? does mass_bias shift the
centre of mass used in `driveline::Shape`, or something else?), so it's
listed here rather than attempted blind. `driveline::Shape` would need
two new fields, and `chassis_traits::Traits::grip()` already exists as a
candidate formula — it's just never been plugged in and its tuning has
never been checked against a real chassis' feel.

### Raid bosses: bigger, less uniformly transformable, JSON-driven mechanics

**The JSON-externalization part is now DONE, built this session.**
`raidcon::BOSSES` (the 19-boss const array) has a new, fully additive
JSON loading path: `assets/raids/bosses.json` now exists as the real,
editable, checked-in content file — bootstrapped from the exact 19
compiled entries, verified byte-for-byte identical to the const via a
new test. `raidcon::all_bosses()` loads it once (cached), falling back
to the compiled `BOSSES` on any failure — missing file, malformed JSON,
an unknown `mech`/`element` name, a missing field — deliberately
all-or-nothing so a partially-loaded roster can't happen. `raidcon::boss()`,
`raidflow::RaidFlow::begin`, and `raidgen::plan_all` all now go through
this path; the compiled const stays untouched and is still what the
tests that want deterministic fixed data reference directly.

No external crate — a small hand-rolled JSON reader (`raidcon::json`,
~150 lines) matching this codebase's existing convention of hand-parsing
simple JSON rather than pulling in serde for one file. Not a general
parser (no `\u` unicode escapes, no exponent notation) — deliberately
minimal, scoped to exactly what a boss roster needs.

Verified: 3 new tests (round-trip fidelity against every field of every
boss including relic arrays, missing/malformed-file fallback, the parser
in isolation), plus the existing `nineteen_plans_of_three_phases` test
now genuinely exercises the new path end-to-end (`plan_all()` →
`all_bosses()` → the JSON file) rather than just the old const. Full
suite: 1259/1259 (was 1256; +3 new). All three feature-flag
configurations still build clean.

**What this does NOT do, kept deliberately out of scope**: doesn't
change the `Boss` struct's actual field types, doesn't add any new
mechanic types or gameplay behavior, doesn't touch the still-unconfirmed
question of whether these 19 narrative bosses reach a physical chassis
in a live fight. This was purely "make the existing rich mechanic data
editable without a recompile," which is what was actually asked for.

The rest of the original investigation stands as written below —
already real, already player-selectable boss chassis at 2.6x scale, the
still-unconfirmed narrative-to-chassis link, the not-yet-implemented
transform-exemption flag, and the logscope/aperture AI vision as a
later-stage goal.

**Already real (unchanged from before):** 9 chassis are flagged `"boss": true` at 2.6x scale via
a per-chassis `chassis.json` (`hardboil`, `pyrachnid`, `starlord`,
`hussar_tank`, `judgementor`, `primitive_mantis_mech`,
`scorpion_commander_...`, `strider_tank`) — data-driven, not hardcoded.
They're already player-selectable in the garage/catalog (which
deliberately spawns minion-scale chassis around a boss preview "for
scale" — real evidence bosses are meant to be played, not just fought).
`raidcon.rs` separately carries 19 handcrafted narrative bosses, each
with a distinct `Mech` type (`Purge`/`Ground`/`Air`/`Sync` — genuinely
different combat mechanics, not just flavor), element, home arena, and
real lore text.

**The concrete gap:** that 19-boss table is a Rust `const` array, not
JSON. Tuning or adding a boss mechanic today means a code change and
recompile. Externalizing `raidcon::BOSSES` to a data file is the direct,
scoped version of "handcrafted mechanics in the raids json" — a real,
bounded refactor (define a schema, a loader, keep the const as a
compiled-in fallback/default set), not a vague ask.

**Resolved, definitively — the earlier "not confirmed" note above was
too narrow a question.** Traced this all the way through this session:
`boss.id` is never used anywhere to select a 3D model (only as a hash
seed and a save-progress key — checked every use of `.id` across
`raidflow.rs`/`raidgen.rs`). `EncSpec`/`PhaseSpec` (the actual encounter-
planning data a `RaidPlan` produces) carry no model reference at all —
just position, an abstract `EncKind` (`Anchor`/`Standard`), a mech type,
and a minimum unit count. That `EncKind::Anchor` comment references
"the C engine" directly, which led to checking `games/blackstar` — a
**separate, real C implementation of this same game**
(`engine_src/game.c`, `engine_src/exengine/raid_plan.h`) that the Rust
`Mech`/`EncSpec` enums are numerically kept compatible with. The actual
boss/champion spawn logic lives there, and it's a **much smaller,
different, hardcoded set**: `g_champion_kind` is 0-5 — `Cryotek`,
`Blackstar`, `Jhiaxus`, `The Watch`, `Unicron`. Only "Jhiaxus" overlaps
the Rust 19-boss table by name (`jhiaxus_omega`, "Devastator Jhiaxus")
at all.

**So the real picture**: `raidcon.rs`'s 19-boss table (now the JSON
file) is newer, richer, Rust-side-only content — genuinely well
authored, but currently disconnected from the C engine's actual,
already-shipping champion-spawn mechanism, not just missing a lookup
table to wire it up. The two systems don't even have the same roster
size or names. Before doing ANY more work on "which chassis a boss
looks like," the real open question is architectural: is `blackstar-rs`
meant to fully replace the C engine's raid system (in which case the
19-boss table needs its own spawn logic built from scratch, since there
is nothing to hook into), or does the 19-boss content eventually need
to reconcile with the 5-6 existing C-engine champions (in which case
much of `raidcon.rs`'s roster is speculative/future content, not a
near-term integration target)? That's a real product decision, not
something I should guess at by picking one and building toward it.

**Not implemented at all:** anything that exempts a boss from the
standard vehicle<->robot fold. Every chassis, boss or not, goes through
the same `xform`/`Morph` pipeline today — "not always transformable in
the same sense as bots" has no code path yet. Would need a per-chassis
or per-boss flag the fold system actually checks (a natural
`chassis.json` field, `"transforms": false`, mirroring how `scale`/
`boss` already work) plus deciding what such a boss does INSTEAD of
folding — stay one form, or a different, non-`xform` presentation
entirely.

**The logscope/aperture/wgpu-engine vision (AI-authored raids):**
architecturally coherent, not just aspirational — `apps/logscope`
(elsewhere in this monorepo) contains a real `gpu-gemm` wgpu→Vulkan
inference crate, and `apps/aperture`'s recent streaming-backend work is
a real precedent for a live-generation pipeline. But this is a
multi-project integration effort spanning three currently-independent
apps, clearly a later-stage goal — the JSON-externalization work above
is the concrete first step that would actually make raid content
edit-without-recompile, which is presumably a prerequisite for anything
generating that content automatically later.

### Spaceship work

Explicitly asked to continue. Investigated this session — real, honest
answer:

**Solid, tested, done:** interior walking/rooms/doors (`interior.rs`,
`airlock.rs`, 27 tests, real collision wiring into `world.rs`'s SDF), and
the ship-level travel state machine (`ship.rs`, 15 tests) — board, plot a
course, cruise, descend, arrive.

**Does not exist at all:** a player-piloted spaceship. Every instance of
a ship moving through space in this codebase is either a scripted,
deterministic flyover (`dropship.rs` — its own doc comment: "canned...
nobody owns a live flying ship yet") used for raid insertion, or the
`Cruise` phase of the travel state machine, which is an `.interior()`
phase — the player walks the ship's rooms as a passenger while transit
happens automatically, never controls it directly. `BigBody` (the large-
ship physics body) has full position/rotation/collision/damage
simulation but zero input handling anywhere; `main.rs` never references
it. Confirmed by reading `dropship.rs`, `ship.rs`'s `Phase` enum, and
`bigbody.rs`'s public API in full.

This is a real fork, not a bug: if "keep working on the spaceship" means
finishing what's there, that's mostly done. If it means being able to
fly one as a vehicle, that's a genuinely new feature — new input scheme,
new camera behavior at that scale/mass, new physics tuning for something
much bigger and heavier than a car or jet — and belongs in the same
"needs a design decision before implementation" bucket as the jet/tank/
racer control proposals above, not something to improvise blind.

---

## Process notes for whoever continues this

- This session hit the same failure shape three times: guess a numeric
  threshold, discover via a second check that it's wrong, guess again.
  Each time, the fix that stuck was the one backed by an actual
  measurement or a reproducible synthetic case, not a hand-picked number.
  Keep doing that — build the repro before proposing the constant.
- Prefer extending the existing `examples/*_audit.rs`/`trace_*.rs`
  harnesses over inventing new ones; they already match the real loading
  pipeline (`load_raw_at` + `align::unify`), which took several wrong
  turns to get right the first time.
- None of this was tested in an actual running/rendered build this
  session (no GPU here). Everything above is verified by compiling,
  passing tests, and reproducible CLI tool output — real signal, but not
  a substitute for someone actually looking at and playing the game.

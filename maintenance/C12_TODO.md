# C12_TODO — open threads from the c12 session

Same convention as `C9_TODO.md` / `C10_TODO.md` / `C11_TODO.md`: every item
says how far it actually got. None of those files are superseded; C11-2a was
edited in place this session because it turned out not to be a real finding.

The c12 session installed the offline 1.96.0 toolchain, merged two bundles
(c9-006g studio, c5blackstar-008 blackstar-rs), and corrected two pieces of
inherited sandbox lore. What follows is what it found and did not close.

---

## 1. blackstar-rs: 13 tests fail, and they failed on arrival

The c5blackstar-008 merge is committed and the crate now builds every target
for the first time on this line (`cargo ailite test --no-default-features`:
**1200 passed, 13 failed**). All 13 are in the imported work.

**The attribution matters, so here is the evidence rather than the claim.**
The test binary did not compile on the incoming branch (three separate
breakages, fixed in `35ad4fd77`), so no test in the crate had run there. Main
had not touched any of the failing modules since the fork point at
`695fc3b73` — the only files both sides changed are README.md, orientsim.rs
and physics.rs. And `skeleton`, `turret` and `driveline` name no `crate::`
sibling at all: they are glam-only, so nothing this line did can reach them.

- **C12-1a — skeleton, three failures.** `a_vehicle_decomposes_the_same_way_a_body_does`
  ("the wings did not read as limbs: core 1 prims, 1 limbs (reach 1.20), 2
  orphans joined"), `compaction_halves_the_overhang_without_detaching_it`
  ("the wing barely moved: 1.2005669 -> 1.2005669" — the compaction is a
  no-op), `vehicle_mode_packs_the_limbs_inside_the_hull` ("prim 1 still pokes
  out by -0.55/5.35 after containment"). The middle one looks like the root:
  if `compact` moves nothing, the other two follow. `read`, not diagnosed.
- **C12-1b — turret: `the_whole_turret_follows_the_barrel`** — "the hull was
  swept into the turret: fittings: 4 turret prims (ring 0.00,0.10,0.23), 2
  tread prims". The ring test is admitting hull prims. `read`.
- **C12-1c — driveline, two failures.** `the_reference_car_is_left_alone`
  ("a hatchback weighs 2.574 t") and `wing_loading_is_actual_aerodynamics`
  ("the big wing stalls faster" — both configs report an identical
  `stall_ms` of 57.400238, so wing area is not reaching the stall term).
  The identical number across two very different masses is the lead.
- **C12-1d — the ship-interior trio.** `hullform::the_hull_contains_every_room_it_was_fitted_to`
  ("len 40: room bridge is outside the hull it was fitted to"),
  `interior::the_hull_between_rooms_is_still_solid` ("the ship is mostly
  corridor: 1484 open, 244 solid"), `aboard::the_camera_probe_finds_open_volume`
  + `aboard::the_hull_is_solid_where_the_walls_are` ("the hull corner is
  walkable — the ship is a shell with no walls"). These four are one story:
  the exterior fitted in `6a29d2708` does not enclose what it was fitted to.
  Highest-value of the group — it is the feature the commit series is named
  for.
- **C12-1e — `shots::a_locked_blast_catches_a_crossing_mark`** ("the missile
  spent itself without arriving") and
  **`model::a_flung_panel_no_longer_sets_the_ride_height`** ("debris still
  owns the ride height: 18"). The second is the same bug family as
  `2cdd88a14` (bounds measured over debris, not over the body) — that commit
  fixed it for bounds and orientation; the ride height still reads debris.

Run them with `CARGO_TARGET_DIR=/tmp/bd-bs maintenance/tools/cargo-ailite test
--no-default-features` from `games/blackstar-rs`. Default features
(audio/analog) now BUILD here too — `libasound2-dev` is apt-installable
(§3, corrected this session) — but were not run.

## 2. voiceprint-kit duplicates voiceterm, with nothing keeping them in step

The c9-006g merge brought `shared/voiceprint-kit`, whose `fingerprint` and
`compute_mfcc` are byte-identical to `apps/voiceterm/src/acoustic_index.rs`
and `phonetic.rs` apart from `pub(crate)` → `pub`. Verified by diffing the
function bodies, not by reading the commit subject.

That identity is the whole point — studio compares against the same
fingerprint voiceterm computes — and nothing enforces it. If either side
drifts, cross-app comparison silently stops meaning anything, and the failure
looks like "the match rate got worse" rather than an error.

- **C12-2a** — the real fix is voiceterm depending on the kit, which touches
  the AI01→AI02 file format and `N_MFCC`'s visibility; not a merge-integration
  change.
- **C12-2b** — the cheap guard is a `maintenance/tools` checker comparing
  normalised function bodies across the two files and FAILING if either side
  is missing (§5b: a scan that finds nothing must not pass).

## 3. Inherited lore that was wrong, both corrected in place

- **C11-2a withdrawn** — there was never a stale lock; `lock_check.py` with a
  relative root could not recognise workspace members. Fixed in `dd56a20a0`.
  **Consequence: C10-1a's feature-sweep FAIL on `unendlich-engine` has lost
  its explanation and is undiagnosed again.** Do not start from the lock.
- **CLAUDE.md §3's missing-deps list** — `libasound2-dev`, `libopus-dev`,
  `cmake` and `mesa-vulkan-drivers` all install from apt here (`f31e894a5`).
  A fresh sandbox has NO Vulkan ICD at all, which C11_TODO §21 assumes it
  has; install `mesa-vulkan-drivers` before reaching for lavapipe.

## 4. Not reached

C11's §21 perf hunt (the submit-cause counting around `generate_kv_stream`
that this session was oriented to do), the whole hive/queen thread (§19–20),
and C11-2b's seven orphaned .rs files.

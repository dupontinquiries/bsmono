# foldview

Watch a blackstar frame fold between its two lives — robot at `t=0`, vehicle at
`t=1` — using the fixes from `TRANSFORM_OVERHAUL.md`, wired to pixels for the
first time:

1. **Rigid motion only.** No scale lerp, no `crimp`. Every frame of every part
   is provably a rotation + translation of the rest mesh — the test suite runs
   `model_verify::verify_pair` on real geometry at five timeline points.
2. **Per-joint 3D axes** via `motion::axis_angle`.
3. **Scheduled choreography** via `motion::schedule` — parents before children,
   widest swings first, parts *hold still* between their windows.
4. **Per-chassis performance**: `fold_spin` / `xform_dur` / proportions come
   from `content/chassis.json`, and the easing `Profile` (Fluid / Combat /
   Anger) gives the same choreography a different mood.

The identity map follows `content/bots.json` `transform_map_reference`:
chest→hull wedge, head→cab, pauldrons→fenders, arms→front wheels,
thighs→rear wheels, shins→exhausts.

## Run

Headless (works anywhere, no GPU):

```
cargo run --release -- --data ../../engine_src/data --content ../../content \
    --variant watch --chassis raptor --frames 8 --out /tmp/fold
cargo run --release -- ... --sheet --t 0.63 --out /tmp/all_chassis.png
```

Interactive:

```
cargo run --release --features gpu -- --data ../../engine_src/data \
    --content ../../content --gpu
```

Space plays/reverses at the chassis' `xform_dur`. `[` `]` cycle chassis,
`1`–`3` cycle variants, drag orbits, Esc quits.

## Tests

`cargo test` loads the real `p*_*.iqm` part meshes and content JSONs and
asserts: rest pose is exact identity; every part at every sampled `t` is a
rigid motion with correct winding (their determinant check catches mirrored
parts); no child's window starts before its parent's.

## Toolchain note

Developed and tested against rustc 1.75 in a sandbox where only Ubuntu's
archived toolchain was reachable; the committed `Cargo.lock` pins a few
transitive crates (`bytemuck`, `proc-macro-crate`, `indexmap`) below releases
that need newer cargo. On current stable Rust none of the pins are necessary —
`cargo update` freely. The `gpu` feature was type-checked here against winit's
x11 configuration (the wayland build-dep tree needs rustc ≥1.79) and ships
with winit's default features; the crate's own code is platform-neutral.

## Known limits, honestly

The vehicle-mode silhouette is as good as procedural parts get — the fold is
*right* (rigid, sequenced, verifiable) but the shell is generated primitives.
Detail comes from step 4 of the overhaul: slotting imported geometry into these
same joints via the anatomy pass. This viewer is the harness that work renders
into.

## Imported vehicles — full transformations

The category imports (`flyer`, `tank`, `hoverer`, `hovertank`, `racer`) were
flattened to one submesh by the pipeline, but they are unwelded — wheels,
turrets, guns and wings are still separate shells topologically. `veh.rs`
welds by position, splits into connected islands (`model_verify::weld` +
`components`), classifies the islands into an anatomy (hull / arms / legs /
wings / barrel / head), and hangs each island on a `motion::Joint`. Vehicle is
the rest pose; the robot is where the schedule carries it. Same `Bot` type,
same renderer, same rigidity proof — two imported vehicles are in the test
suite alongside the p-part bots.

```
foldview --data engine_src/data --census tank            # how each model reads
foldview --data engine_src/data --veh engine_src/data/imported_racer_grit_aka_rocket_car.iqm --frames 8 --out /tmp/grit
foldview --data engine_src/data --vehposter flyer --pick a.iqm,b.iqm --out poster.png
```

The skeleton is voxel-fitted per model: the vehicle is solid-voxelized, the
robot height comes from the material actually available (H = cbrt(V / 0.047),
clamped to the body span), the torso band centers on the voxel mass, limbs are
sized to the fore/aft span that exists, and every component welds to the
bind-pose bone the majority of its vertices sit nearest — then rides that
bone's articulation (root pitch, pi shoulder folds, bending knees and elbows).

Honest limits: a handful of models are welded into one solid island
(`akimbo`, `echotron`, `reverser`, `batwing`, `tempest`) and cannot fold
until they're split; anatomy is heuristic, so a wheel occasionally enlists as
the wrong limb; and pieces may interpenetrate mid-fold — clearance is authored
work, not derived.

## Interiors, archetypes, and hand-tuning

Every robot now grows original generated hardware: knuckled fists, heel-and-
toe feet, a visor on the elected head, a reactor core whose disc spins through
the fold, rib slats, a hip gyro ring, and knee/shoulder pistons whose rods
stroke as the joints move. Which extras appear comes from the interior
archetype — brawler (thick bones, exhaust stacks), raptor (light truss,
X-braces), sprinter (calf pistons, low core), signals (antenna mast, module
bay behind a hatch that slides open mid-fold) — defaulted per category and
overridable per model in `foldview_tuning.json`, alongside pitch, stance,
palette, and head-election overrides. The tuning file is meant to be edited
by hand and by eye.

Round two: Rule 20 is enforced in the scheduler (abduction expands early,
armor snugs close late, the finale — head reveal, hatch, six clenching
fingers — lands after 0.78, spinning parts never stop), shoulders abduct so
the pi fold sweeps around the torso, and each archetype gained a signature:
brawler knuckle spikes, raptor calf winglets, sprinter heel wheels that keep
turning, signals radar dish. Vertebrae, cable runs, pauldrons, a face around
the visor, and a sternum keel fill the frame out. 27 models hand-tuned in
foldview_tuning.json.

The fold now breaks apart first and reassembles later: every armor piece and
chunk gets an early radial burst (0.03-0.30) and a late pull (0.55-0.95)
whose vector sums with the burst to land exactly on its robot station — the
exploded-view beat, with the skeleton and organs holding form inside the
cloud. Per-category amplitude (racers pop, tanks shrug) with a `burst` knob
in tuning. QA gates cohesion at t=0 and t=1 only; mid-fold blobs are the
feature now.

## FVX: the extended mesh format

IQM stores what a game renders; FVX stores what a transformation needs. Some
source meshes (several flyers especially) ship with cracked surfaces — open
boundary edges along seams — which shred component detection and cohesion.
Segmentation now runs a seam-repair pass (boundary vertices snap to nearby
boundary vertices before components form), and `--bake <model.iqm|category>`
writes the repaired segmentation to a `model.fvx` sidecar that `build` loads
in place of live segmentation. Version 1 stores repaired pieces; the header
reserves flags for baked orientation frames and per-piece role hints, so
hand-repairs can eventually be stored rather than recomputed. Long pieces
that refused to fold are now carved into fourths along the body axis so each
segment takes its own bone and motion.

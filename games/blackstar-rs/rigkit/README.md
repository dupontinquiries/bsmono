# rigkit -- vehicle-to-robot transforms

Builds the transform viewers (a self-contained HTML page per vehicle) from a
source model: orion, the Falcon GT, a seeker-jet flyer, and Folly, a hover car
that levitates into a floating mage.

    python3 build_viewer.py                    # orion  -> out/rig-viewer.html
    VEHICLE=falcon python3 build_viewer.py     # also: flyer, hover

Run from this directory. `rigpaths.py` resolves everything: `inputs/` (the source
models), `out/` (built pages and caches, not committed), and the repo's own
`tools/forge`. Builds are reproducible: the same inputs give byte-identical pages.

- `vehicles/` -- one kit per vehicle: a catalogue of its islands, panel cuts, and
  a dress module (fates, seats, reshapes, hinges, weapons, the transform mode).
  `hover_direct.json` holds decisions made by nearest known part (see partnet).
- `partnet/` -- the part-decision experiment: networks vs nearest known part,
  scored leave-one-vehicle-out; `decide.py` decides a new vehicle's parts.
- `harness/` -- headless checks on built pages: boots a page's own script in node
  with the real three.js (`npm install` here first).
- `rigview/` -- renders a page's frames on a real GL pipeline for capture.
- `packx/` -- extracts `bots.pack`.

The robot-form motion core is `games/shared/boneforge` (Rust); the engine drives
it through `src/rigmotion.rs`.

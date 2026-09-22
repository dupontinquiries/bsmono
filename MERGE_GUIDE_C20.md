# Merge guide — C20 (on top of c19unified-003-fleet)

For whoever -- human or another Claude -- merges this session's work into a copy
of the repo. The base is `c19unified-003-fleet.bundle`, whose `main` is
`aa5482537`. Everything here is a fast-forward from it.

## Apply

    git clone c19unified-003-fleet.bundle c19 && cd c19     # or an existing checkout at aa5482537
    git fetch /path/to/c19-C20.bundle main:c20
    git merge --ff-only c20

If your `main` has moved past `aa5482537`, merge rather than fast-forward. The
likely conflict points are `manifest/projects.toml` (one appended entry),
`TODO.md` (one inserted paragraph), and `games/blackstar-rs/src/main.rs`
(three small insertions around `loco` / `loco_pose`).

## Commits, in order

- `7bae4e4fe` forge: generated vehicles and bots for blackstar-rs, as named-node GLB
- `a2e8568a9` unendlich: barrier-free subgroup variant of chunk_cull, and a SIMD-width audit
- `4b5fcfab4` bsweb: a CLI and a Fairphone GUI for the parts web, with an undo journal
- `7c431b962` uikit-egui: two deprecated Context::style calls, and the migration note
- `3bb74dcfd` blackstar-rs: group the catalogue -- the mechanism was there, it was starved
- `83d0c2a04` blackstar-rs: forge chassis as loadable fold strips
- `471d8f32a` blackstar-rs: default to the generated roster, and a hand-written default car
- `7a53b5ea1` blackstar-demo: a small testable slice, and rename the binary to blackstar
- `80d1c91a8` blackstar-demo: geometry and a renderer that runs in a test
- `2de651118` blackstar-rs: drive a 22-bone rig from the gait machine
- `936ec4af4` blackstar-rs/rigkit: the vehicle-to-robot transform pipeline
- `f1e8c8adb` tools: gitsweep, chunkup, chatgrep and coursekit
- `28a3499a4` tomescope: a demo book with a designed cover
- `b1f2fc7d4` blackstar-rs/tools/models: the model-pipeline scripts
- `922905f4a` games/speedrace: speedrace-core, a headless race engine
- `941f7ca57` docs: the Pulsar reconciliation, and unendlich's rejected alternatives
- `fb14d861e` TODO: C20 open threads, and the investigations that changed no code

## How each part was checked

- **blackstar-rs rig integration** -- `rigmotion`'s 5 tests pass under
  `maintenance/tools/cargo-ailite test --no-default-features --bins rigmotion`.
  boneforge's 39 tests pass on their own (`cd games/shared/boneforge && cargo test`).
  The vocabulary census had been broken by this session's own `3bb74dcfd` (an
  `IP_MARKS` list in `main.rs`); the list moved to `vocab.rs`, the file the
  census exempts, and the counts are back to the base values. An earlier note
  said the census already failed at the base -- that was measured after
  `3bb74dcfd` and was wrong.
- **rigkit** -- builds all four viewers from inside the repo (`python3
  build_viewer.py`, `VEHICLE=falcon|flyer|hover`). Two consecutive builds are
  byte-identical, and the decoded content matches the pages built before the
  move. The orion island cache is rebuilt and matches the original island by
  island. Its node harness needs `npm install` in `rigkit/harness` (three.js 0.128).
- **speedrace** -- 24 tests pass.
- **Python tools** -- all compile. They were not re-run on real data here.
- **Licensed marks** -- everything added was audited for the franchise names the
  repo polices, plus the character names that came up in this work. None
  remain, except in `tools/chunkup/exclude-ip.txt`, which is the filter for
  them (the same exemption `vocab.rs` has).

## Not in this bundle, on purpose

- The built viewer pages and GPU captures: build products of `rigkit`.
- The per-island pickle: rebuilt on first use.
- voiceterm / aperture / tomescope code: those apps were investigated and not
  changed. The findings are in `maintenance/C20_TODO.md` and each app's TODO.

## Watch for

- On a disk-capped machine (~19 GB), full debug builds of blackstar-rs fail by
  running out of disk; use `cargo-ailite`.
- `cargo test` without `--bins --tests` fails to compile the `rigpose`
  example: `src/rig.rs:96` was already corrupted at the base.

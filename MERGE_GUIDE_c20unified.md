# Merge guide — c20unified

For whoever -- human or another Claude -- merges this line into a copy of the
repo. The detail of every change is in its commit message; the index, what was
verified and what is open, is `maintenance/C20UNIFIED_TODO.md`. This file is
how to apply it and what to watch.

Head: `beb59b21b` plus the commit that updates this guide (it was `8cc21f04f` at
`c20unified-023`; since then four more bundles were merged in, below). In the bundle, `main`,
`integrate` and `tideshell` all point at it; `remarkable` is `c96958c5d`
(already merged in).

## What this line already contains

Each of these tips is an ancestor of the head (checked with
`git merge-base --is-ancestor`):

| tip | what |
|---|---|
| `6b03cd4af` | c19unified-042 `main` |
| `1c10de7ec` | c19unified-042 `tideshell` (where this session started) |
| `c96958c5d` | c19unified-036-remarkable |
| `97c722c04` | c19unified-039-hive `main` |
| `1ff18963f` | c19unified-039-hive (the "hive" commit) |
| `4c10042c3` | tideshell session, c20unified-009 `c20-merge` |
| `ba4a64aeb` | tideshell session, c20unified-010 `c20-merge` |
| `5db19cf18` | clawbound c20unified-024 `main` (with `twinfang-pass` `c44478e35`, `clawbound-jungle` `1871f3225`) |
| `64cf7f3a2` | vidkit-c20-on-0d5c2a36f |
| `88c05b3df` | unendlich-dragons (c20unified-020 partial) |
| `4ce40d38c` | tideshell session, c20unified-011 `c20-merge` |

## Apply

If your checkout's `main` is at one of those tips (or behind one of them):

    git fetch /path/to/c20unified-NNN.bundle integrate:c20unified
    git merge --ff-only c20unified

If your `main` has moved past them, merge (not rebase: four merge commits and
their recorded conflict resolutions are part of this line):

    git fetch /path/to/c20unified-NNN.bundle integrate:c20unified
    git merge c20unified

The last four came in as incremental bundles on `0d5c2a36f`; their merges
(`9ece7f372`, `8a5574fcc`, `a0f907fc0`, `3d620722f`) record what each brought,
the one conflict (tideshell's loop) and the tests run. They exposed two things
fixed right after: `shared/ambient-palette` missing from tideshell's
`pack-src.sh` (`162acb640`) and 31 lockfiles vidkit's branch left stale
(`beb59b21b` -- `cargo --locked` failed on that branch alone). Re-running the
section-7 checkers then found one unpinned text field from vidkit, pinned in the
commit after those.

## Where a merge is likely to conflict

Files this session's own commits changed, outside a single app, where another
line is most likely to have edited too:

- `CLAUDE.md` -- only the orientation paragraph pointing at the newest TODOs
  (now `C20UNIFIED_TODO`, then `C20`, `C19`, `C18`, `C14`). Keep both sides'
  pointers.
- `docs/agents/INDEX.md` -- three list entries added under "Other projects".
- `manifest/projects.toml` -- the `fao-codec-egui` notes string only.
- `maintenance/tools/lock_check.py`, `orphan_check.py`, `fleet_build.sh` -- see
  "Stricter checkers" below; if the other side changed them too, keep both
  behaviours.
- `maintenance/C14_TODO.md` -- one STATUS paragraph added to section 14.
- `shared/crypto-kit/src/lib.rs` (+ `random_bytes` / `random_hex`),
  `shared/depot-auth/*`, `shared/cosmic-levers/*` (new crate),
  `shared/uikit/crates/uikit-egui/src/icons.rs` (`renders()` now asks epaint's
  glyph table).
- Lockfiles: `apps/{bo-suite,cadscope,depot,gitmind}/Cargo.lock`,
  `apps/snapclone/egui-frontend/Cargo.lock`, `apps/tideshell/settings/Cargo.lock`,
  `shared/{cosmic-levers,depot-auth}/Cargo.lock`. **Do not hand-merge a lock.**
  Take either side, then in that directory run `cargo metadata --format-version 1
  >/dev/null` (a minimal resolve: adds what is missing, moves nothing) and
  confirm with `cargo metadata --locked --format-version 1 >/dev/null`.
- `games/unendlich/assets/shaders/chunk_generation.comp.spv` and `.spv.hash`:
  generated. If `chunk_generation.comp` conflicts, resolve the SOURCE, then let
  the crate's `build.rs` regenerate both (needs `glslc` or `glslangValidator`);
  never merge the binary.

Inside apps, the most-changed files: `apps/tideshell/deploy/DEPLOY-FP5.md`,
`apps/tideshell/egui-frontend/src/bin/tideshell.rs`,
`games/unendlich/crates/unendlich-engine/src/hero_forge.rs`,
`.../bin/unendlich_fullgame.rs`, `apps/fao-codec/egui-frontend/src/{transcode,lib,encode_page}.rs`,
`apps/gitmind/crates/cockpit/src/depot_client.rs`, `apps/depot/crates/depot-core/src/clients.rs`,
`apps/snapclone/egui-frontend/src/{voice_codec,voice_bridge,host}.rs` and its binary.

## Behaviour changes another line should know about

- **snapclone voice wire**: a new packet kind 4 (Opus: flags, u32 seq, payload).
  A build without it drops kind 4 -- it hears silence from a newer peer, not
  noise. **snapclone-egui now needs libopus to build** (default feature `opus`;
  `--no-default-features --features app` builds without it, sending raw).
- **unendlich heroes**: every hero's drawn proportions change (height is the
  body's, not props' or pose's); items are in the anatomical RIGHT hand (they
  were in the left, in third person too); the first-person camera sits in the
  head; capes / the cloak / the scarf are cloth in the game. HeroPose gained
  `fp_carry01` and `cloth_garments`, and HeroAnchors is no longer `Copy`: any
  other line that builds a HeroPose field by field will not compile until it
  sets both (the game does; see `044f4147e`).
- **unendlich build**: the engine's default features compile again; if your
  line added egui uses to `render/world_renderer.rs`, gate them on `ui-overlay`
  or the default build breaks again.
- **depot / gitmind**: paired clients sign write requests (`Depot-Sig`, scopes
  in `depot_auth::scope_for`); gitmind's Fleet page runs a join server on port
  8099 for phones with only a browser and curl.
- **Stricter checkers**: `lock_check` now reads workspace MEMBERS (it missed
  their dependencies), so after the merge it may report stale locks the other
  line did not know it had -- fix them as above, they are real (`cargo
  --locked` fails on them). `orphan_check` counts `include!` and
  examples/tests/benches mounts; `fleet_build` classifies whisper-rs-sys /
  audiopus_sys build-script failures as SKIP (missing tools).

## After merging: verify

The checks this line passes, and what to expect:

    for c in lock_check orphan_check archive_check aarch64_flags_check eframe_backend_check \
             voiceprint_parity_check board_gestures_parity_check cfg_feature_check \
             aperture_cmdstrip_check textedit_id_check font_install_check; do
      python3 maintenance/tools/$c.py; done
    # expected failures, pre-existing and documented:
    #   orphan_check: gpu-gemm kernels.rs, snapclone camera-bridge shared_frames.rs (x2)
    #   cfg_feature_check: rusty-board llm-swipe-ffi

Per project (on a small machine, `CARGO_BUILD_JOBS=1`; unendlich's test crate
needs ~3 GB of RAM):

| project | command | this line's result |
|---|---|---|
| unendlich engine | `cargo test -p unendlich-engine --no-default-features --lib` | 8850 passed |
| unendlich engine | `cargo test -p unendlich-engine --lib` (defaults) | 9232 passed, 2 failed -- exactly the 2 expected below (measured at the head) |
| unendlich all targets | `cargo check -p unendlich-engine --all-targets` and again with `--features ui-overlay` | compile |
| snapclone-egui | `cargo test` in `apps/snapclone/egui-frontend` | 1746 + 3 passed |
| snapclone-audio | `cargo test -p snapclone-audio --features opus` in `apps/snapclone/rust` | 107 passed |
| fao-codec-egui | `cargo test --release --lib` in `apps/fao-codec/egui-frontend` | 186 passed |
| tideshell | `cargo test --release --lib` in `apps/tideshell/egui-frontend` | 116 passed |
| gitmind cockpit | `cargo test -p cockpit` in `apps/gitmind` | 563 passed |
| unendlich + dragons | `cargo test -p unendlich-engine --lib --no-default-features --features audio` | 8990 passed (after the dragons merge) |
| tideshell (after its c20unified-011 merge) | lib; `crates/tideshell-core`; `settings` | 119; 269; 23 passed |
| clawbound-battleground | `cargo test --lib`; `--test game`; `--test shaders` | 381 + 3 pre-existing failures (named in its note); 22; 2 |
| vidkit | `vidkit-core --lib`; `vidkit-egui --lib --features glow-upload`; player `--no-default-features --features app,https,alsa,bscale` | 114; 168; 195 + 3 ignored |
| uikit-egui | `cargo test -p uikit-egui --lib` in `shared/uikit` | 551 + the pre-existing callout failure |

Expected failures (documented, not regressions):
`mesh_tri_regression::sample_grid_worst_case_stays_under_budget` (a real
foliage-cost finding awaiting a decision), `mesher_binary::cross_tufts_are_eight_vertices_and_double_sided`
(binary mesher, off by default), and in uikit-egui
`callout::body_text_stays_readable_on_every_tinted_surface_in_every_palette`
(pre-existing, slate/Error 2.93:1).

## Not verified anywhere

Nothing on the phone; nothing rendered on a GPU; no real snapclone call. The
list, per item, is in `maintenance/C20UNIFIED_TODO.md`.

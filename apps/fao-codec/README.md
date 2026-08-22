# fao-codec

Workspace: `fao_codec_core`, `fao_codec_lib`, `fao_codec_v1`, `fao_codec_v2`,
`fao_app`, `fao_cli`, `egui-frontend`.

## Verification (2026-08-19)

Had no build-status record anywhere. Ran the workspace (excluding
`egui-frontend`) for the first time with rustc 1.96 and found a real,
fixable compile break, plus a set of pre-existing runtime test failures
this same break had been hiding.

**Fixed**: `fao_codec_lib::v8_reconstruction_codec::MeshIntegrator::
integrate` gained three parameters (`source_yuv: &[u8]`, `source_w: usize`,
`source_h: usize`) for real color sampling at some point, and three of its
own tests (`empty_depth_map_produces_no_mesh`, `flat_depth_produces_mesh_
grid`, `discontinuity_creates_no_triangle`) were never updated to match —
a hard `E0061` compile error, so this crate's entire test binary could not
build at all. Fixed by passing `&[], 0, 0` at each of the three call sites
— `integrate`'s own logic already treats an empty slice / zero dimensions
as "no source frame available" and skips color sampling entirely (its own
code checks `source_yuv.is_empty() || source_w == 0 || source_h == 0`), so
this is exactly the documented no-color-data path, not a stand-in for real
data these geometry-only tests never needed.

**Found, NOT fixed** — because the crate's test binary couldn't compile at
all before the fix above, these 8 failures had never actually been run and
observed:

- `library::tests::add_and_match`, `::manifest_persists_across_open`,
  `::quick_stats_matches_open_at_small_scale` — a temp-dir-backed model
  library (add/match/persist). Could be a real regression or a sandbox-
  specific filesystem quirk (permissions, tmp cleanup ordering); not
  distinguished here.
- `residual::tests::small_constant_offset` — a lossy residual encode/decode
  round-trip asserts mean reconstruction error `< 5.0` for a uniform
  +10 offset; failed, meaning either the codec's quality regressed or the
  threshold is now wrong.
- `v8_reconstruction_codec::feature_tests::fast_finds_synthetic_corner`,
  `::feature_track_pass_assigns_stable_ids`, `::lk_tracks_pure_translation`
  — FAST corner detection and Lucas-Kanade optical flow tracking on
  synthetic test images; all three failed.
- `v8_reconstruction_codec::densify_tests::anchor_cadence_correct` — an
  anchor-frame cadence check for the densification pass.

None of these were touched. They span real computer-vision/codec algorithm
correctness (corner detection, optical flow, lossy compression quality) —
verifying a fix would need either domain expertise in the specific
algorithms or a way to visually inspect the reconstructed output, neither
of which this pass had. `cargo test -p fao-codec-lib` after the integrate()
fix: 65 passed, 8 failed (up from a hard compile failure that hid all 73).

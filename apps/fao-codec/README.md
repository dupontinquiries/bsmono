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
- `v8_reconstruction_codec::feature_tests::fast_finds_synthetic_corner` —
  **root-caused, not just observed**: the test builds a 32×32 image with a
  16×16 dark square notched out of a bright field (a right-angle "step"
  corner) and expects `FastDetector::detect` to fire near the tip. Worked
  through the actual math: this detector's `arc_score` requires 9+ of the
  16 circle points (radius 3) to agree in polarity — more than half the
  circle. At the exact corner tip, the dark quadrant subtends only a ~90°
  wedge of that circle (~4 of 16 points), and points just inside either
  straight edge only ever get 2 of the 4 quick-reject cardinal probes to
  cross the threshold (the other 2 cardinals point further into the same
  region, away from the boundary) — geometrically, a single right-angle
  step notch can never satisfy a "same polarity over half the circle"
  test at any point near its tip. This isn't what FAST is generally weak
  at in practice (real image corners usually aren't pure right-angle
  steps against a flat field) — it means either the test's synthetic
  geometry doesn't actually exercise what this implementation can detect,
  or the 9-of-16 threshold is stricter than intended for this use case.
  Either fix is a design call (change the test's corner shape, or loosen
  the algorithm's arc threshold) — not a bug to patch blind.
- `v8_reconstruction_codec::feature_tests::feature_track_pass_assigns_
  stable_ids` ("frame 0 should produce features") — NOT the same root
  cause as the FAST test above (this one runs `FeatureTrackPass::process`
  over a diagonal-stripe gradient image, not a step-corner), so whatever
  is stopping it from producing features here needs its own look.
- `v8_reconstruction_codec::feature_tests::lk_tracks_pure_translation` —
  independent of both of the above: seeds fixed points directly (no
  `FastDetector` involved) and tracks a synthetic +2px x-shift via
  `LkTracker`; the tracker reports `dx≈0.0005` instead of `≈2.0` — the
  optical flow essentially isn't moving the estimate at all. A real
  Lucas-Kanade implementation issue (iteration not converging, a sign/
  Jacobian error, or similar) worth a dedicated look, distinct from the
  corner-detection question above.
- `v8_reconstruction_codec::densify_tests::anchor_cadence_correct` — an
  anchor-frame cadence check for the densification pass. Not investigated
  in the same depth as the three above.

None of these were touched. They span real computer-vision/codec algorithm
correctness (corner detection, optical flow, lossy compression quality) —
verifying a fix would need either domain expertise in the specific
algorithms or a way to visually inspect the reconstructed output, neither
of which this pass had. `cargo test -p fao-codec-lib` after the integrate()
fix: 65 passed, 8 failed (up from a hard compile failure that hid all 73).

**Also fixed** — 4 doctest compile failures across `fao_codec_core::dct`
and `fao_codec_lib::library_db`: unlabeled indented text/fenced blocks in
module doc comments (a DCT pseudocode formula, a directory-layout diagram,
two binary record-format layouts) were being parsed as real Rust code by
rustdoc, which they never were. Fixed by marking them ` ```text ` (the
same convention `fao_codec_lib::library.rs`'s equivalent blocks already
correctly used one file over) — rustdoc then correctly excludes them from
doctest execution entirely rather than trying to compile prose as Rust.
Full workspace `cargo test --no-fail-fast` (excluding `egui-frontend`,
not attempted this pass): every crate green except the 8 pre-existing
`fao-codec-lib` failures above.

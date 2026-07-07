# MANIFEST — r330-world

**WID:** `r330-world`  **Section:** T-world (surface read from generated voxels)
**Base:** `unendlich-r330b-mp-net_channels-and-dispatch-VERIFIED`
**Grounds:** `NEXT_SESSION_GUIDE.md §4a` (highest-value bug), fix direction (a).

## Files touched
| File | Change |
|---|---|
| `crates/unendlich-engine/src/sim/surface_scan.rs` | **NEW** — pure surface-read core + tests |
| `crates/unendlich-engine/src/sim/mod.rs` | **+1 line** — `pub mod surface_scan;` (after `surface_cache`) |

No other files touched. No bin/shader/GPU edits. No dependencies added. No public
signatures changed elsewhere.

## API added (all pure, panic-free, alloc-free in the scan paths; saturating Y math)
- `trait Voxel { fn is_solid(&self) -> bool; }`
- `fn top_solid_y<V: Voxel>(column: &[V], y_min: i32) -> Option<i32>`
  — the brief's signature; top-down first-solid scan of a contiguous column.
- `fn top_solid_y_by<V, F: Fn(&V)->bool>(column: &[V], y_min, is_solid) -> Option<i32>`
  — predicate form; bin passes `|b: &BlockId| b.is_solid()` (no trait impl / no shared edit).
- `fn top_solid_y_in_chunk<V, F>(blocks, x, z, chunk_size, y_base, is_solid) -> Option<i32>`
  — alloc-free in-place scan of the flat chunk buffer (stride `x + z*cs + y*cs*cs`);
    Y-depth derived from `blocks.len()/cs²`; out-of-range/short-buffer → `None`.

## Verify (ran in-sandbox)
- **Standalone gate (documented floor, same as `net_channels`):**
  `rustc --edition 2021 --test surface_scan.rs -o /tmp/t && /tmp/t`
  → **10 passed / 0 failed**.
- **Warning-clean:** `rustc --edition 2021 --crate-type lib surface_scan.rs` → exit 0, **0 warnings**.
- **Patch applies:** `git apply --check -p1 CHANGES-r330-world.patch` → **CLEAN** on the base tree;
  applied for real, re-gated the applied file → **10/10**.
- Purity audit: no `unwrap`/`expect`/`panic!`/IO/RNG/clock in non-test code (only total
  `unwrap_or`). Module is `crate::`-free → gates standalone.
- **Toolchain caveat (honesty):** sandbox had rustc **1.75** (apt), not the spec's **1.96**.
  Module uses only long-stable APIs, so the result is identical; flagged rather than hidden.

### Tests (10)
finds_first_solid_from_the_top · all_air_is_none (+empty) · solid_at_the_very_top (+single)
· fluids_and_plants_above_ground_are_skipped · **divergence_harness_follows_voxels_not_the_fp64_prediction**
(the §4a bug reproduced: true voxel top 67 ≠ FP64 prediction 70, scan follows the voxels)
· strided_chunk_scan_matches_contiguous_scan · strided_scan_rejects_bad_coords_without_panicking
· air_pocket_returns_the_upper_lip_not_the_cave_floor · predicate_and_trait_forms_agree_and_are_deterministic
· y_min_offset_is_applied_to_the_world_y

## Handoff (bin wiring — proposed, NOT applied)
See `HANDOFF_world_surface.md`. Summary: switch placement call-sites
(`sim/spawn.rs:72,169`, `chunk_manager.rs` `pick_spawn`@1836, `world_prober.rs:91`, and the
coordinator-owned bin foliage/mob/spawn sites) from `surface_y_at` (FP64 prediction) to
`top_solid_y_in_chunk` on the loaded chunk's voxels; reuse `sim::surface_cache::SurfaceCache`
to memo per tick; keep prediction only as the not-yet-loaded fallback. Generation-time flora
(`cpu_generator.rs:763`) and the player's initial spawn need the loaded-chunk precondition
handled — flagged in the handoff.

## Honesty
- **VERIFIED** (logic executed: 10/10 standalone + patch applied + re-gated). Pure data +
  scan; **no** GPU/visual/bin claims — those are operator-validated on the FP32 path.
- Did **not** run a full-workspace `cargo build` (cold link on the 7-crate tree overruns a
  turn; the standalone gate is the documented substitute for a dependency-free `sim::`
  module). The only shared-file touch is the single `pub mod` declaration.
- A real bug in my first cut (`top_solid_y_in_chunk` assumed a cubic chunk, so Y-depth was
  wrong for non-cubic buffers) was caught by the standalone gate and fixed (depth now
  derived from buffer length). Reporting it rather than quietly squashing it.

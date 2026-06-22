#!/usr/bin/env bash
# sanity.sh — curated fast-feedback subset of the test suite.
#
# Use this as the "spam" loop while iterating on bug fixes. Full suite
# (788 tests) takes too long for tight feedback. This subset compiles
# the same crate, so the recompile cost amortizes across runs.
#
# Usage:
#   ./scripts/sanity.sh           # debug, fastest recompile
#   RELEASE=1 ./scripts/sanity.sh # release, slower compile, faster run
#
# Exits non-zero on any failure. Pipe through `tail -20` for terse output.

set -euo pipefail

MODE_FLAG=""
if [[ "${RELEASE:-0}" == "1" ]]; then
  MODE_FLAG="--release"
fi

# 44 tests, --exact so substring collisions don't matter.
TESTS=(
  # ── stale_lod_key predicate (covers the prior ship's refactor) ──
  stale_lod_key_evicts_other_lods_same_coord
  stale_lod_key_keeps_same_lod_same_coord
  stale_lod_key_keeps_other_coords
  stale_lod_key_filter_yields_correct_set

  # ── meshing pool core lifecycle ──
  pool_processes_a_request
  pool_processes_multiple_requests
  pool_rejects_when_full
  drop_does_not_hang_when_result_channel_full

  # ── mesh buffer pool (file we just modified — keep full coverage) ──
  fresh_pool_is_empty
  alloc_returns_offsets_within_buffers
  alloc_rounds_up_to_alignment
  free_releases_spans
  double_free_rejected
  stale_version_handle_rejected
  exhaustion_returns_err
  realistic_workload_fits
  churn_pattern_no_unbounded_growth

  # ── bounded_copy_dst_range (THIS ship — barrier scope math) ──
  bounded_copy_dst_range_empty_returns_none
  bounded_copy_dst_range_single_copy_trivial
  bounded_copy_dst_range_multi_contiguous
  bounded_copy_dst_range_multi_scattered
  bounded_copy_dst_range_overlapping_copies_handled

  # ── gpu-decorate parity (THIS ship — grass tufts on GPU meshes) ──
  gpu_decorate_appends_tuft_geometry_to_base_mesh

  # ── seasons foundation (THIS ship — temporal climate axis) ──
  year_phase_wraps_to_unit_interval
  summer_is_warmer_than_winter
  temperature_delta_is_bounded
  season_landmarks_map_correctly

  # ── weather model (THIS ship — rain + lightning state) ──
  precipitation_is_bounded
  no_rain_in_dry_air
  no_lightning_without_heavy_rain
  lightning_strikes_during_a_downpour
  snow_fraction_goes_cold_to_warm
  no_lightning_in_a_snowstorm

  # ── water field foundation (THIS ship — heightfield + settle) ──
  settle_conserves_volume_on_flat_ground
  water_spreads_from_a_spike_on_flat_ground
  water_flows_downhill
  depth_stays_non_negative
  seed_from_chunk_counts_fluid_per_column
  seed_then_settle_conserves_seeded_volume

  # ── GPU meshing CPU ref (output invariants — regression guard) ──
  empty_octree_emits_no_geometry
  single_isolated_block_emits_six_quads
  invariant_checker_accepts_empty_mesh
  neighbor_lookup_eliminates_seam_face
  neighbor_lookup_symmetric
)

echo "sanity: ${#TESTS[@]} tests, mode=${MODE_FLAG:-debug}"

cargo test --lib -p unendlich-engine --features render,gpu ${MODE_FLAG} -- \
  --exact "${TESTS[@]}"

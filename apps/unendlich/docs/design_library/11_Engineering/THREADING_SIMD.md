# Threading + NEON/SIMD plan (r191, operator-directed)
STAGE 1 (DONE r191): rayon — Phase-1 worldgen columns parallel (map ->
serial scatter; deterministic). Foundation dep landed.
STAGE 2: tick sharding — 2x2x2 chunk-REGION checkerboard (8 phases): all
regions of one parity tick in parallel (no neighbor races: handlers read
neighbors via snapshot, write own region only), phases rotate per tick.
This is also where the operator's 2x2x2 same-conditions FSM batching
lives: per-region equivalence classes (stage, moisture-key) feed the r191
memo so a region computes each class ONCE. Requires: TickContext split
into per-region views + merge of TickEdits in region order (determinism
preserved by fixed phase order).
STAGE 3: mesher + apply overlap — apply batches build on a worker,
swap-in on main (the Arc architecture already supports it).
STAGE 4: NEON/SIMD (Asahi M2, aarch64): density_from_surface inner loop
over y as 4-lane f64x2 pairs via `wide` crate (portable; NEON codegen on
aarch64); fbm octave loops unrolled to multiply-accumulate chains;
audio StemGroup sum + saturation as i16x8. Candidates chosen by harness
wall_ms attribution. All stages land behind the harness: digests must be
byte-identical before/after each stage.

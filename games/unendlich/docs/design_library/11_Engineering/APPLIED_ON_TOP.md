# Engineering docs applied on top of the design library (r202)

The operator shipped the organized Unendlich Design Library (52 docs,
June 2026) as the canonical source-of-truth structure. It is copied here
under docs/design_library/ verbatim.

These additional engineering docs were authored DURING the wiring work
(they post-date the library snapshot and were noted as "slightly stale
compared to your work"). They are the implementation-side companions to
the design docs and live here in 11_Engineering:

- FASTMATH_ANALYSIS.md — measured verdict: scalar fastmath is a net loss
  on M2/modern hw; rejected with benchmark evidence. Parity surface
  (worldgen) has no transcendentals so it was never a parity risk.
- PARALLELISM_BY_OPERATION.md — operation-gated threading (P pure-map /
  C clone-apply / S serial) where type signatures make mis-classification
  a compile error. Supersedes the area-gated sketch.
- SCHEDULER_NOTES.md — rollback-netcode scheduling: input-mailbox-as-clock,
  bounded-rollback confirmation horizon, player-first priority lanes,
  deterministic fetch order.
- THREADING_SIMD.md, tick_authority_regions.md — threading stages + the
  tick-authority region model the multiplayer cadence uses.
- STRUCTURES_creature/render/worldgen.md — module-structure maps used for
  the wiring census.

RECONCILIATION: where a library doc and an extracted .md in docs/design/
cover the same topic, the library doc is canonical for DESIGN intent; the
docs/design/*.extracted.md copies are the older working extractions kept
only because the wiring tooling (doc_linker.py, METRICS.md parity scores)
references them by path. No design content was lost — the library is a
superset of the extractions plus the newer SPEC/META tiers.

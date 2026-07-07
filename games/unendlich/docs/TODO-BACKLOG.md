# TODO / FIXME backlog — triage

The 10 in-tree TODOs are mostly *deferred design*, not quick mechanical fixes.
Triaged here so "knock out TODOs" is an informed choice, not busywork. None were
mechanically safe to close blind (no build loop), so this is the analysis input.

| # | location | what | effort | risk blind | value | call |
|---|----------|------|--------|-----------|-------|------|
| 1 | `water_field.rs:47` | module is doc-only scaffold ("coming in a follow-up") | large | n/a | feature | own session; design first |
| 2 | `chunk_manager.rs:50` | add a coord conversion helper "when needed" | small | low | tidy | do opportunistically when a call site needs it |
| 3 | `weathering.rs:15` | evolve blocks toward env-driven equilibrium | medium | med | feature | needs the weathering tick wired first |
| 4 | `blocks.rs:85–88` | migrate `is_solid` → narrower predicates (`blocks_collision`, `blocks_light`, `produces_face_against`, `is_meshable_volume`) | large | **high** | debt | staged refactor + per-call-site review; touches mesher/light. NOT blind |
| 5 | `biomes.rs:112` | add `MapleLog`/`MapleLeaves` blocks | small | **high** | content | block-enum change feeds shaders/meshers; append variants only, verify discriminants, recompile shaders. Build loop |
| 6 | `cpu_generator.rs:482` | Lava-block "hook in the bin when implemented" | medium | med | feature | depends on a bin-side system that doesn't exist yet |
| 7 | `gpu_meshing_octree.rs:504` | freshness TODO "filed" | n/a | — | note | informational; no action |

## Genuinely safe + small (candidates for a build-loop session, in order)
1. **#2** — add the coord helper the moment a real call site wants it (avoids a
   speculative API). Trivial, low risk.
2. **#5 MapleLog/MapleLeaves** — only if appended at the END of the `BlockId` enum
   (so existing discriminants — which index into shader/mesher tables — don't
   shift), with the shader block tables + `blocks.rs` predicates updated in the
   same brick, then a shader recompile (`UNENDLICH_STRICT_SHADERS=1` to verify no
   stale `.spv`). This is the cleanest "content" win but MUST go through the build
   loop because of the cross-shader coupling.

## Deliberately deferred (need a design pass / their own brick)
- **#4 is_solid migration** is the highest-debt item but also the most dangerous
  to do blind: every mesher/light/collision call site has to be re-pointed at the
  right narrower predicate, and a wrong pick silently changes meshing or lighting.
  Worth a dedicated session that adds the predicates first (no call-site changes),
  then migrates call sites in small reviewed batches.
- **#1 water_field**, **#3 weathering**, **#6 lava hook** are features whose
  prerequisites (ticks/systems) aren't wired; design before code.

## Recommendation
Pair these with the build loop. The two quick wins (#2, #5) are ~1 brick each with
testing; the rest are design-first. Nothing here is a safe blind edit, which is
why this turn shipped the stripes guard + overlay FIF-ring (both contained) and
left these triaged rather than churned.

---

## BACKLOG (R27, user-requested): GPU memory partitioner — multiple mesh/chunk regions + queue sectioning
**Status:** design-first investigation, deferred. Logged for "later / singleplayer perf."

**The idea (user):** look into multiple mesh and chunk regions in the GPU memory
partitioner for local singleplayer, and/or perf optimization via queue-organization
sectioning.

**Why it's worth doing (evidence from the R27 M2 runs):**
- Today the GPU pools are each ONE flat region: `mesh_vert` (3072MB cap), `mesh_idx`
  (2048MB cap), and the octree pool (`UNENDLICH_OCTREE_POOL_MB`, run at 2048→8192MB).
  Allocation/eviction/upload all operate on these single arenas.
- Mesh pools sit nearly empty in steady state (vert ~54MB, idx ~63MB used) — the cap
  isn't the constraint; locality and churn are. far-LOD meshing is the volume cost
  (n≈40k @ ~5.6ms each), and the near tail spikes hard (max 510–880ms single mesh on
  the critical path). The pain is tail/churn, not capacity.
- On M2 unified memory, `OCTREE_POOL_MB=8192` reserves 8GB of the shared pool to hold
  ~574MB actually used — wasteful; a partitioned pool would size regions to need.

**Sketch of the work (for whoever picks it up):**
1. **Region-partitioned pools.** Split each GPU arena into sub-regions keyed by LOD
   tier (near/mid/far/vfar) and/or spatial sector (a coarse grid of chunk-columns),
   each with its own free-list/ring. Wins: per-region eviction (drop a whole far sector
   without scanning the global map — relates to the T10 evict work), bounded
   fragmentation, and right-sized reservations instead of one 8GB arena.
2. **Queue organization / sectioning.** Section the mesh-request + upload queues by the
   same region key so work batches spatially/by-tier (improves cache locality, lets the
   slack scheduler drain one sector at a time, and pairs with BIASED_SHELL_WALK +
   predicted_mesh_ahead which already produce motion-ordered candidates).
3. **Singleplayer angle.** With one local player there's exactly one hot sector (around
   the player) + a motion-predicted next sector; the partitioner could keep those two
   regions "warm" and demote the rest, instead of treating all loaded chunks uniformly.

**Cross-refs:** T10 (per-LOD evict, already in — the per-region evict generalizes it),
PERF-LEVERS.md (upload budget/slack scheduler), prefetch_predict (sector prediction),
mesh_buffer_pool deferrable lane (per-region defer budgets would slot in here).
**Not a blind edit** — touches allocator + queue plumbing; design + bench first.

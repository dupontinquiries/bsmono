# r340 SHARED-LIB OVERLAP MATRIX + CONSOLIDATION BACKLOG (coordinator)

7 threads returned. MERGED the 5 NEW shared-lib modules (the extractions); the large CALL-SITE
SWEEPS are recorded here as a dedup'd backlog (a careful targeted pass, not mass-executed this round
— half-done sweeps across 800+ sites are unverifiable without a full build).

## MERGED shared modules (gated @1.96)
| Module | Thread | Tests | Notes |
|---|---|---|---|
| `world::field_ops` | sl-fields | 9/9 | clamp01 (branch, **preserves -0.0**) + saturate (max/min, normalizes -0.0). TWO forms kept — must NOT be merged (field RESPECT). +task_lanes parallel field op (bonus a). |
| `sim::net::codec` | sl-net | 9/9 | Reader/Writer + check_header/read_str/read_vec + encode_into (alloc-free). |
| `render::mesh_prims` | sl-mesh | 7/7 | canonical face table/FACE_NORMALS/quad_corners/quad_aabb + quad_aabbs_parallel (bonus a, parity). |
| `world::worldgen_noise` | sl-worldgen | 7/7 | delegates to `util::hash_util` (no splitmix re-impl) + task_lanes parallel gen (bonus a). |
| `render::gpu_pack` | sl-gpu | 8/8 | SlotPool recycler + pack_tiles/unpack_tiles (lossless, bonus b). |

## OVERLAP RESOLUTIONS (≥2 threads proposed the same candidate)
- **splitmix64 / unit hash** — sl-math C1 (71 files), sl-worldgen, sl-batch. CANONICAL: `util::hash_util`
  (`mix64`/`mix64_unit`/`mix64_2`, exists). worldgen_noise already delegates. ⚠ Some sites are RNG-
  *steppers* (`&mut state`), not fixed-seed hashes — those need a stream helper, NOT a blind mix64 swap.
- **clamp / saturate** — sl-math C2 (mathx), sl-fields C1 (field_ops). RESOLUTION: KEEP BOTH — `mathx`
  hosts general clamp; `field_ops` owns the **-0.0-preserving branch** clamp01 for field paths. Do NOT
  cross-route (merging flips the sign of a zero in a field — proven divergence on `-0.0`).
- **Aabb struct** — sl-mesh C3 (flagged, NOT extracted), likely math/gpu/worldgen. CANONICAL: `util::bbox`
  (exists). Route here; no new module.
- **GPU SlotPool / buffer pool** — sl-gpu C2 (EXTRACTED), sl-mesh (proposed cross-cluster). CANONICAL:
  `render::gpu_pack::SlotPool`.
- **pack_tiles/unpack_tiles** — `render::gpu_pack` (new) DUPLICATES `render::q8::pack_tiles` (r336-neural).
  Output-identical (only param-name + an independent `let` reordered). CANONICAL: `gpu_pack`. BACKLOG:
  delegate q8 → gpu_pack (preserves q8 byte-parity; deferred to keep the neural oracle untouched now).

## CALL-SITE SWEEP BACKLOG (dedup'd; targeted future pass, NOT executed)
1. splitmix64 → `hash_util`: ~71 files (full local copies + `fn unit` reimpls; mind the stepper shape).
2. scalar clamp/lerp/smoothstep → `mathx`: 743 inline saturate idioms + 101 private helper defs.
3. `batch_*` map boilerplate → `batch_kernel::map_collect`: 34 of 36 fold 1:1 (8 already folded r330).
4. greedy face-merge → `mesh_prims::greedy_merge_mask` (sl-mesh C2, proposed; different mask types).
5. GPU host-buffer staging → `render::gpu_buffer` (sl-gpu C1, unsafe/ash glue — bin-side, propose only).
6. q8::pack_tiles → gpu_pack (above).
Each sweep is mechanical but large; do as its own gated pass (per-file rustc/grep + the operator build).

## RESPECT/parity verification (all PASS)
- field_ops: 20k-fuzz, each clamp form bit-matches its originals; -0.0 distinction preserved.
- codec: byte-identical frames (the 5 per-module wire tests still pass verbatim).
- worldgen_noise: delegates to hash_util; seeded determinism + serial==parallel parity.
- mesh_prims/field_ops/worldgen_noise bonuses: task_lanes serial==parallel parity (rayon path is
  task_lanes' own tested concern; kernel logic gated serially here).
- gpu_pack: pack/unpack lossless round-trip; SlotPool preserves existing alloc semantics.
- No field flattened, no wire/quantize bytes changed, no block-grid assumption introduced.

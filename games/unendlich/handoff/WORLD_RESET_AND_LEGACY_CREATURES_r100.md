# r100 — merged-worlds fix + legacy-dino diagnosis

## (1) Merged worlds on Create — FIXED
CAUSE: launch called chunk_manager.rebuild_for_seed (clears the MANAGER's
chunks/octrees) but the RENDERER kept its uploaded GPU meshes for the old titlescreen
world, so the new seed's world streamed in ON TOP → "merged based on surface".
FIX: new WorldRenderer::clear_all_chunk_meshes() (gate-verified under --features
render) drops every uploaded mesh + drains the parked cache (deferred GPU-buffer
destroy via the normal path). The bin Launch handler now calls it right after
rebuild_for_seed. Verify on-device: Create a seed → the titlescreen terrain should be
GONE, only the new world present.
CAVEAT (follow-up if needed): mesh results that were already in-flight in the mesher
pool when reset happened can briefly upload old geometry until the new chunk replaces
them — a transient flicker, not the persistent merge. If it's visible, add a world
"epoch" counter: stamp requests with the epoch, discard mesh results whose epoch is
stale on drain.

## (2) "Big legacy dinos still showing up" — diagnosis + fix path
NOT the ambient ecosystem (habitat_weight(Dinosaur)=0, tests enforce it). The live
creatures are built by `creature_design::EggSpec::chosen(arche).hatch()` →
`spawn::from_blueprint` (and `spawn::spawn` for ecology kinds) — the LEGACY procedural
body system. UNENDLICH_SPAWN_ARCHE=dragon forces that picker's "dragon" archetype,
whose body still reads as a tall legacy creature. The good `sim::dragon_forge` model
(the one tuned via the collages) is NOT wired as the live creature mesh.

FIX PATH (the real one — render/spawn integration, compile-on-end):
- Each live creature already carries a `genetics::Genome` (bin line ~1735). Bridge it:
  `dragon_forge::build(&forge_genome_from(creature_genome), res)` → `Vec<ForgeBox>` →
  feed the entity/mob renderer instead of the legacy blueprint mesh.
- This is precisely where the dispatched creature-from-image threads (C1–C3) land:
  they PRODUCE the forge Genome/traits; this wiring makes the forge the LIVE body.
- Until wired, the legacy bodies render. Stopgap if they must go sooner: gate the
  SPAWN_ARCHE observation spawns off, or lower their spawn weight, in the bin.

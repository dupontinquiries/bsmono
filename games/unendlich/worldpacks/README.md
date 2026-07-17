# World packs

A world pack customises how a world **generates** — it never ships a map.

Unendlich terrain is an infinite, deterministic function of a seed; rollback
netcode, region digests, and `world_authority` command sourcing all depend on
every peer regenerating identical ground from `(seed, ruleset)`. So a pack is
**parameters and procedural directives only** — biome definitions (the same
JSON `assets/biomes/…` already hot-loads), global climate offsets, and
seed-anchored field brushes (`field_brush.rs` vocabulary). The validator in
`world/worldpack.rs` actively rejects any document smuggling voxel/heightmap
payloads — including for server sub-lobbies, which are just `(seed, pack)`
pairs, never saved maps.

- `ashen_reach.json` — the reference pack: lowered seas, two new biomes, a
  guaranteed humid pocket at spawn, and seed-scattered oases + heat blooms.

## The LLM terrain endpoint

`world/terrain_llm.rs` serves the same schema over HTTP so a model (or any
tool) can propose terrain changes at runtime:

    GET  /schema           machine-readable op format + the no-fixed-maps policy
    GET  /packs            staged packs by lobby
    POST /terrain?lobby=L  a world-pack JSON; 200 staged / 422 with problems

The endpoint only **stages** validated packs; the sim thread claims them with
`PackStore::take_pending` at a generation boundary and feeds
`worldpack::resolve_brushes` into the field interaction layer — the same paths
elemental items already exercise. Network thread never touches world state.

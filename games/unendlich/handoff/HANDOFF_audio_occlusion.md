# HANDOFF — wiring `sim::audio_occlusion` (WID r332-sound)

**Status:** PROPOSAL ONLY. The pure core (`sim::audio_occlusion`) is done +
verified (8/8). The block-counting raymarch and the filter drive touch the
audio/bin path, which a worker must not wire (SHARED_RULES §1–§2) —
coordinator/operator apply on device.

## What's pure (this module) vs glue (the bin)
- **Pure (here):** given a solid-block count per stem, produce a gain multiplier
  (`occlusion_gain`), a full occluded mix (`occluded_mix_gains`), and a low-pass
  cutoff hint (`muffle_cutoff_hz`). All finite/clamped, deterministic.
- **Glue (bin):** count solid voxels on the listener→source segment (a voxel
  raymarch), and feed the gain to the mixer + the cutoff to a filter.

## Proposed call-sites

### 1. Raymarch the solid-block count (bin, per stem)
Where the mixer updates per frame (the `audio`-feature playback shell behind
`handoff/AUDIO_SPATIAL_MIXER.md`), for each stem compute the count of solid
voxels the straight listener→source segment passes through, using the chunk
voxel grid the renderer already has:

```rust
// pseudo — bin glue, uses the existing world voxel query (e.g. chunk_manager block_at)
fn solid_blocks_between(world: &World, a: [f32;3], b: [f32;3]) -> u32 {
    // DDA/Amanatides-Woo voxel traversal a→b; count cells where block is solid.
    // step ~1 block; cap the march at the falloff distance (cost bound).
    let mut n = 0u32;
    for cell in voxel_line(a, b) {              // existing DDA helper or a small one
        if world.block_at(cell).is_solid() { n = n.saturating_add(1); }
    }
    n
}
let occ: Vec<u32> = stems.iter().map(|s| solid_blocks_between(world, listener, s.pos)).collect();
```

Perf: cap the march length (e.g. at the mixer falloff) and only re-march every N
frames or when the listener moves a cell — occlusion changes slowly. The march is
the only added world-read; keep it off the hot per-sample path (it's per-stem,
per-frame, not per-sample).

### 2. Apply gain (replace the mix call)
```rust
// was: let gains = sim::spatial_audio::mix_gains(listener, stems, falloff);
let gains = sim::audio_occlusion::occluded_mix_gains(listener, stems, falloff, &occ);
// or, per-material attenuation:
// let gains = sim::audio_occlusion::occluded_mix_gains_db(listener, stems, falloff, &occ, per_block_db);
```
Drop-in: with `occ` all-zero this is bit-identical to `mix_gains`, so wiring it
can't regress the unoccluded mix.

### 3. (Optional) drive a low-pass for spectral muffle
Occlusion eats highs, not just level. If the playback shell has a per-stem
biquad/one-pole low-pass, set its cutoff from the hint:
```rust
let cutoff_hz = sim::audio_occlusion::muffle_cutoff_hz(occ[i]); // 20kHz open → 250Hz floor
```
If no filter exists yet, the gain drop alone is already an audible improvement;
the cutoff is the next refinement.

## Tunables (consts in the module, override per material on the bin side)
- `DEFAULT_PER_BLOCK_DB = 3.0` — attenuation per solid block.
- `OCCLUSION_FLOOR = 0.05` — a wall muffles, never perfectly silences.
- `MUFFLE_SURVIVE_PER_BLOCK = 0.6`, `MUFFLE_FLOOR_HZ = 250.0`.
A denser material (stone vs foliage) → higher `per_block_db` via `_db` variant.

## Verify after wiring (on device)
- Stand a sound source behind a wall: the stem's level drops and (with the
  filter) sounds muffled; step into the open doorway and it returns to the
  distance-only gain.
- With no wall between listener and any stem, the mix is unchanged from before
  (all-zero occlusion ⇒ `occluded_mix_gains == mix_gains`).

# Bin patches — show the forge dragon, real place/spawn commands

These are the only changes that live in the binary (`crates/unendlich-engine/src/bin/unendlich_world.rs`),
which can't be compiled in the sandbox. The lib functions they call are all built and tested.

## 1. Make `UNENDLICH_SPAWN_ARCHE=dragon` show the NEW forge dragon (not the old blueprint)

This is why you saw the old, tan/"orange" model: the SPAWN_ARCHE branch builds a `creature_design`
blueprint, which discards the genome and sets no element tint. Route it through the new genome body
(which carries the forge voxel model + element tint).

Around line ~1716, replace:

```rust
let (kind, mut cr) = if let Some(arche) = spawn_arche {
    let bp = unendlich_engine::creature_design::EggSpec::chosen(arche, wseed).hatch();
    (CreatureKind::Dragon, unendlich_engine::spawn::from_blueprint(&bp, s.pos))
} else {
    (s.kind, unendlich_engine::spawn::spawn(s.kind, s.pos))
};
```

with:

```rust
let (kind, mut cr) = if let Some(arche) = spawn_arche {
    if matches!(arche, unendlich_engine::creature_design::Arche::Dragon) {
        // NEW: the forge voxel dragon, element-tinted, varied by seed.
        (CreatureKind::Dragon, unendlich_engine::spawn::spawn_dragon_arche(wseed, s.pos))
    } else {
        let bp = unendlich_engine::creature_design::EggSpec::chosen(arche, wseed).hatch();
        (CreatureKind::Dragon, unendlich_engine::spawn::from_blueprint(&bp, s.pos))
    }
} else {
    (s.kind, unendlich_engine::spawn::spawn(s.kind, s.pos))
};
```

Now your overspawn command shows the new dragons. (Normal ecology spawns already route through
`spawn::spawn(Dragon)`, which also builds the forge model, so dragons show up everywhere now.)

For a guaranteed green "vine" dragon, use `spawn::spawn_dragon_element(wseed, 0, s.pos)` (0 = Wood).
Elements: 0 Wood(green) · 1 Water(blue) · 2 Air(pale) · 3 Dust(tan) · 4 Fire(red) · 5 Storm(violet).

## 2. Replace the hard-coded `Stone` place with real block / spawn-egg commands

Around line ~2360 the place branch hard-codes `BlockId::Stone` and ignores the held item. The lib's
`resolve_use` (and `resolve_use_seeded` for trait eggs) already does the right thing. Replace the
`else { /* Place: ... Stone ... */ }` body with:

```rust
} else {
    let target = hit_face.neighbor(hit_cell);
    let [x, y, z] = target;
    // The currently selected hotbar item (wire to your inventory state):
    let held: unendlich_engine::item::ItemKind = local_player.selected_item();
    use unendlich_engine::spawn::{resolve_use_seeded, UseAction};
    // a fresh per-use seed + the chosen egg element from the inventory UI:
    let egg_seed = wseed_for_use(x, y, z);   // any deterministic seed source
    let egg_element = local_player.selected_egg_element();  // 0..5, default 0=Wood
    match resolve_use_seeded(held, target, egg_seed, egg_element) {
        UseAction::PlaceBlock { cell: [bx, by, bz], block } => {
            // (keep the existing player-AABB suffocation guard around this)
            if chunk_manager.set_block(bx, by, bz, block) {
                last_block_action = Instant::now();
            }
        }
        UseAction::SpawnCreature(mut cr) => {
            cr.body.position[1] = cr.grounded_y(cr.body.position[1]);
            // register as a roamer exactly like the ecology spawn block (~line 1735):
            register_roamer(cr, egg_seed, &mut roamers);
            last_block_action = Instant::now();
        }
        UseAction::Nothing => {}
    }
}
```

`resolve_use(held, target)` (no seed) also works if you don't want trait selection yet — a dragon egg
then spawns a position-seeded forge dragon.

## 3. More spawn eggs + inventory trait selection

The lib primitives are ready:
- `dragon_archetype::Genome::from_seed_element(seed, element_idx)` — breeds true to an element.
- `spawn::spawn_dragon_element(seed, element_idx, pos)` — the body for a trait egg.
- `spawn::resolve_use_seeded(held, cell, seed, element_idx)` — the use action for a trait egg.

Inventory/UI work (bin): the SpawnEggs tab should let the player pick an element (and optionally roll a
seed); store `selected_egg_element` on the player and pass it into `resolve_use_seeded`. If you want the
element baked into the item itself (so different eggs stack separately), add a variant like
`ItemKind::DragonEgg { element: u8, seed: u64 }` and handle it in `resolve_use_seeded`.

## 4. Gate the OLD dragons to "beta dragons"

Production dragons are now the forge voxel model (`spawn::spawn`, `spawn_dragon_arche`,
`spawn_dragon_element`). The old box-anatomy dragon is kept only behind a beta gate:
`spawn::spawn_beta_dragon(pos)` (anatomy boxes, no forge → the old silhouette).

Wherever a dragon is spawned in the bin, gate the old path behind a flag, e.g.:

```rust
let beta = std::env::var("UNENDLICH_BETA_DRAGONS").is_ok()
    || matches!(spawn_arche, Some(a) if a == /* your beta arche */);
let cr = if beta {
    unendlich_engine::spawn::spawn_beta_dragon(s.pos)   // old box look
} else {
    unendlich_engine::spawn::spawn_dragon_arche(wseed, s.pos)   // forge dragon
};
```

If you saw both old and new dragons at once, that's the blueprint path (#1) still firing for some
spawns — once #1 routes `Arche::Dragon` to `spawn_dragon_arche`, only `spawn_beta_dragon` produces the
old look, and only when the beta flag is set.

## Notes
- **Resolution = 256.** `spawn::DRAGON_FORGE_RES = 256` (the forge slider caps at 140 but 256 looked
  near-perfect when overridden). At 256 a dragon is ~24k raw surface voxels; the `dragon_forge` Z-run
  merge collapses those to ~12.5k boxes. Voxelizing is cheap (built once per dragon at spawn, tens of ms).
- **Overspawn / distance LOD.** 12.5k boxes per dragon is fine for a handful but heavy for a big herd.
  Use `spawn::spawn_dragon_with_genome_res(pos, g, res)` to dial resolution down for dense spawns
  (e.g. 64–128 far from camera), and/or add a distance-based res in the roamer render loop. A fuller
  multi-axis greedy mesh (vs. the current Z-only merge) is the next reduction if needed.
- Capabilities (fly/see/eat), hitboxes and combat still come from the dragon's anatomy; only the *visuals*
  switch to the forge voxels.

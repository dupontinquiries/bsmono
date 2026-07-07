# BIN PATCHES — r594: ragdoll on death/knockout

The pure bridge is shipped + tested in `sim::ragdoll`:
`from_creature(&Creature, dt)` and `from_creature_with_impulse(&Creature,
impulse, dt)` build a `(Vec<Node>, Vec<Stick>)` star skeleton (trunk + one stick
per limb), seeded with the body's velocity (+ any knockback) so a creature killed
mid-run tumbles. Drive it each frame with `ragdoll::step(&mut nodes, &sticks,
ground_y, dt, iterations)` and draw each part at its node position. 12/12 module
tests pass; `from_limbs` is verified standalone, the `from_creature*` wrappers
are trivial field maps (integration-verify on your `cargo build`).

This doc is the **bin wiring** (can't compile in-sandbox). Tagged
**integration-verify**.

## 1. A place to keep tumbling corpses

Add a small owned list beside `roamers` (bin-local; no engine change):

```rust
struct Corpse {
    nodes:  Vec<unendlich_engine::sim::ragdoll::Node>,
    sticks: Vec<unendlich_engine::sim::ragdoll::Stick>,
    parts:  Vec<unendlich_engine::creature::Limb>, // for rendering each node
    tint:   [f32; 3],
    ttl:    f32,      // seconds left before despawn
    ground_y: f32,
}
let mut corpses: Vec<Corpse> = Vec::new();
```

## 2. Spawn the ragdoll at the death site

In the `hp <= 0` handler (`bin/unendlich_fullgame.rs`, ~line 6033) **before**
`roamers.swap_remove(ri)`:

```rust
// r594: spawn a ragdoll from the slain creature so it collapses instead of
// vanishing. Use the strike direction as knockback (fall back to zero).
let cr = &roamers[ri].creature;
let knock = strike_dir.map(|d| [d[0] * 4.0, 1.5, d[2] * 4.0]).unwrap_or([0.0, 0.0, 0.0]);
let (nodes, sticks) =
    unendlich_engine::sim::ragdoll::from_creature_with_impulse(cr, knock, dt);
let ground_y = surface_at(cr.body.position[0], cr.body.position[2])
    .map(|t| (t + 1) as f32)
    .unwrap_or(cr.body.position[1]);
corpses.push(Corpse {
    nodes, sticks,
    parts: cr.limbs.clone(),
    tint:  cr.tint,
    ttl:   8.0,
    ground_y,
});
// (existing loot roll + swap_remove follow unchanged)
```

`strike_dir` is whatever unit direction the killing blow used (the bending strike
already has it; for a generic hit use `attacker_pos → victim_pos` normalised, or
just pass `[0,0,0]` and it tumbles from its own momentum).

## 3. Tick + expire (once per frame, near the roamer update)

```rust
for c in &mut corpses {
    unendlich_engine::sim::ragdoll::step(&mut c.nodes, &c.sticks, c.ground_y, dt, 6);
    c.ttl -= dt;
}
corpses.retain(|c| c.ttl > 0.0);
```

`iterations = 6` is a good default; drop to 4 if a mass die-off ever shows up in
the frame budget (`corpses.len()` is naturally small with the TTL).

## 4. Render each node

Node 0 is the trunk; nodes `1..` line up with `parts[0..]` (same order as
`cr.limbs`). Draw each part's box centred at its node position, tinted by
`c.tint`. Skeleton is positions-only (no per-node orientation yet — parts render
axis-aligned as they collapse, which reads fine for a heap; oriented parts are a
later polish that would carry a basis per node).

## Verify (integration tier)
- `cargo test -p unendlich-engine --features render --lib sim::ragdoll` — the
  module's 12 tests (ground rest, stick restore, self-collision, and the new
  `from_limbs` topology/yaw/impulse/settle cases).
- In-game: kill a roamer mid-run — it should tumble in the direction it was
  moving (plus knockback), collapse into a heap on the surface, and despawn
  after the TTL. A creature killed while standing should just crumple in place.

## Follow-ups (not blocking)
- **Per-node orientation** for parts that should keep their long axis along the
  bone (neck, tail) — carry a basis and derive it from the stick direction.
- **Fall damage → ragdoll**: `sim::fall` already models impact; a hard landing
  could spawn a brief knockout ragdoll (recover if hp > 0) instead of a death one.
- **Knockout vs death**: `from_creature` works for both; a knockout just means
  you keep the creature and swap the ragdoll back to an upright pose on recovery.

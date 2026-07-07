# Characters, armor & animation

This note covers the figure/animation systems: the customisable armored mannequin, the animated enemy bodies,
the third-person player body, and the movement feel (vault, death).

## The mannequin (`src/character.rs`)
`build_mannequin_static(&ArmorSet)` assembles a low-poly armored figure from boxes, tubes and spheres: an
under-suit body plus per-slot armor pieces (head/chest/shoulders/arms/legs/boots), each with a `PieceStyle`
(None/Standard/Heavy/Recon/Eva/Spartan/Myth) and an `Accent` colour. The `ArmorSet` is serialisable
(`to_text`/`apply_kv`) so a loadout round-trips through the cosmetics DB.

### Surface texturing
Every armour and bodysuit piece carries a `Material` so it samples the shared texture atlas instead of reading
as flat vertex-coloured plastic — the single biggest lift to how the character looks. The bodysuit (torso,
limbs, pelvis) uses `Cloth` for a fine technical-fabric weave; armour plates use a plate material chosen by
style via `plate_material()` — `HexPlate` (hard tactical tread) for Standard/Spartan/Recon/Eva, `RustPlate`
(thick weathered battle plate) for Heavy/Myth. `part_box_mat` / `push_mat` attach the material at build time,
soften the vertical tint (the material already supplies the surface variation), and skip emissive pieces
(visor, crystalline crests, seam lights) so their glow still reads. Helmet and pauldron blocks, which have many
per-style variants, get the material via a post-pass over the pieces they just pushed. Because the atlas is UV-
mapped (not world-space projected), the texture stays locked to the surface as the figure animates and turns.
The armory/loadout preview binds the atlas, so the equipped character shows its real textures in-app.

### Pose system
Rather than a full skeletal rig (overkill for a faceted figure), `Pose` drives a handful of whole-mesh,
height-weighted vertex transforms that read convincingly as life. `build_mannequin_posed(set, pose)` applies
them; `build_mannequin(set)` is the rest pose.

- `Pose::rest()` — the neutral static build (identical to the un-posed mesh; regression-tested).
- `Pose::idle(t)` — breathing rise/fall, slow weight-shift sway, idle head-glances and a gentle arm sway,
  layered at incommensurate frequencies so the loop never reads as an obvious repeat. Feet stay planted.
- `Pose::moving(t, speed01, crouch01)` — a locomotion pose: legs swing fore/aft (opposite sides) through a
  stride whose cadence rises with speed, arms counter-swing, the body bobs at 2× cadence and leans into the
  run; `crouch01` sinks the whole figure (most at the hips) with the feet held so the knees read as bent.

The armory preview animates the equipped set with `Pose::idle` on the UI clock. The third-person player body
(`game.rs`) now uses `Pose::moving` driven by the player's real horizontal speed and crouch, so the figure
walks/runs/crouch-walks in step with play instead of standing frozen — matching the animated enemy bodies.

## Enemy bodies (`src/enemy.rs`)
`build_mesh_tinted` builds each enemy as a rigged, armored soldier: a walk cycle (alternating leg swing with
knee-bend and foot-lift, arm counter-swing with a hinged forearm, torso bob and weight list), segmented armor
(thigh/knee/shin plates, chest plate with collar, angled pauldrons, gauntlets, belt), and per-kind silhouette
details (Screecher blade crests, DeathMech horns + reactor core, etc.). Gait scales by AI state, so a chasing
enemy takes a full stride while an idle one only breathes. Team re-tint shifts the body toward the faction hue
while leaving the armor mostly neutral metal.

### Hunt creatures (`build_creature_body`)
The exotic hunt predators — the **Storm Kraken** and the **Widow** — are *not* the humanoid soldier with parts
bolted on (an earlier version was, and it read as a person in a costume). Only these two divert: when
`hunter_creature` is `Kraken` or `Widow`, `build_mesh_tinted` bails out early to `build_creature_body`, which
builds each as its own anatomy from scratch. The four **humanoid horrors** (Revenant, Stalker, Brute, Wraith)
deliberately keep the human rig (scaled up ~1.22×), so they fall through to the normal humanoid build — the gate
must *not* match all creatures, or those four would render as an empty (invisible) mesh. A regression test,
`every_hunt_creature_builds_visible_geometry`, locks this in.

The two exotic bodies:
- **Storm Kraken** — a floating pointed mantle (a stretched teardrop) with glowing eyes and socket rings,
  charge-lit ridge fins up the crown, and eight three-segment tentacles hanging and lashing below on the walk
  phase, each capped with a pulsing charged node. It hovers (no legs). Death sinks and fades rather than
  toppling (a topple looks wrong on a radial body).
- **Widow** — a low, wide spider: a fat squashed abdomen with the hourglass mark, a smaller cephalothorax, a
  fanged head carrying a cluster of eight glinting eyes, and eight long two-segment legs that arch high off the
  body and plant wide on the ground, scuttling in a staggered gait.

### Spawn & death
- **Spawn-in**: the body rises from below the floor with a bright glow that fades as it settles.
- **Death topple**: for ~0.6s after dying the whole body rotates forward about the feet and falls to the
  ground (ease-in-out, with a slight sideways keel so it doesn't drop like a rigid plank), then lies flat while
  the corpse slowly sinks and fades. This replaced the old "sink straight into the floor" effect, which read as
  melting rather than dying. The corpse lingers through the topple + sink before being cleared.

## Movement feel (`src/player.rs`)
The player has gravity/jump, crouch, sprint, slide, dive, and **vault/mantle**. Vault detection looks just
ahead at foot level; if a ledge blocks the body and its top is within climbing reach (a real step-up, not a
kerb or a sheer wall) with clear standing room and headroom, the body eases up and onto it.

The vault now has camera feel so it reads as physical effort rather than a rigid slide:
- `eye()` adds a **heave** — the head lags low through the first half of the climb, then surges over the lip
  with a slight overshoot — plus a small lateral bob toward the leading hand.
- `view_lean()` returns a **pitch-down** (look toward the ledge you're pulling onto) and a slight roll toward
  the reaching hand; `view_cam()` folds the pitch into the render camera. (Roll is computed but not yet applied
  — `FpsCam` has no roll axis; adding one would ripple through the whole projection path.)
All of it resolves cleanly to the neutral camera the instant the mantle ends (regression-tested).

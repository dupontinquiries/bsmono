# BIG PASS PLAN (r545+) — character models, real dragons, survival

Orientation pass for the operator's three asks. Grounded in code as of r544.
Sequenced low-risk/visible first → device/shader/bigger last. Each roll compiles
+ ships independently.

## What already exists (so we build, not rebuild)
- **Heroes**: hero_forge.rs — 5 element humanoids, walk cycle (r539), menu mesh
  preview (r541), build sliders (r538), element bending override (r534). Avatar
  fade/pinch/zoom/bob/shake all done (r535–r542).
- **Dragons**: dragon_forge.rs (ForgeModel from Genome, WingPose {Spread/Folded/
  Beating/Flared/Aggressive}, skin()/skin_with_flex()); spawn_dragon_arche; deep
  wild-dragon AI (dragon_brain/hunt/flight/thirst/air/locomotion/mode/activity);
  Arche::Dragon renders the forge dragon as the player avatar; bond_tame (Feed/
  Gift→trust, tamed follow). Roamer dragons already track `airborne` + wing buffet.
- **Survival**: Vitality (energy/hunger, drain→starvation→death+respawn), thermal
  /hypothermia (warmth, wet chill, fire, shelter), fall_impact damage, combat
  (crit_roll + armor + weapon durability 4000–4007), eating (raw/cooked meat +
  vegetables), cooking at fire, taming. **Mostly logged, not shown on a HUD.**
- **Unwired assets found (RADAR)**: pose.rs::{walking,animate}, creature_design::
  {gaze_forward,resting_gaze,jaw_hinge}, mob.rs::{can_fly,strike}.

## The bundled pass

### Roll 1 — SURVIVAL HUD + hero polish (pure-Rust, visible, low risk)
- **C1 Survival HUD**: an egui Area (sibling of the hotbar, world_renderer ~4710)
  showing vitality (hunger) + thermal as bars, fed from a new renderer field the
  bin sets each frame. Surfaces the depth that already exists. *Biggest legibility
  win in the game right now.*
- **A1 Hero held item**: render the equipped tool/weapon as a small box in the
  third-person hero's hand (hero_forge gains an optional held-item color/size),
  swinging on use (reuse swing_timer). Heroes currently hold nothing in 3rd person.
- **A2 Hero idle**: subtle breathing/sway when speed≈0 so the hero isn't frozen
  (small sine on torso/arms blended as gait→0).

### Roll 2 — REAL DRAGON FLIGHT (the "real dragons" core)
- **B1 Playable dragon flaps**: drive the dragon avatar's WingPose + gait from the
  player's vertical/horizontal velocity (Beating when ascending, Flared/Spread
  gliding, Folded on ground) — same pattern as the hero gait. Makes flying *feel*
  like a dragon. (self_avatar already builds the forge dragon; just feed anim.)
- **B3 Dragon density cap** (operator note r441): per-region spawn budget so heavy
  seeds don't swarm. Pure-Rust in the spawn director.

### Roll 3 — SURVIVAL PILLARS: thirst + stamina
- **C2 Thirst/hydration**: a hydration reserve paralleling Vitality; drains over
  time, refilled by drinking water (right-click water/empty-hand). HUD bar (C1).
- **C3 Stamina + sprint**: sprint key drains a stamina bar; gates sprint/jump;
  regenerates at rest. Ties into the bob/movement feel. HUD bar.

### Roll 4 — ELEMENT GAMEPLAY (ties heroes + dragons to ELEMENTS.md fields)
- **B2 Dragon breath**: as a dragon, the attack/bend key emits an elemental cone
  that writes into the element/heat field (fire warms+ignites-dry, water douses,
  etc.) per the "element = field brush" model. Visible particles + real field push.
- **A3 Hero elemental aura**: a small hand aura/glow matching the hero's bend
  element; the existing bend cast already uses the character element.

### Roll 5+ — DEVICE / BIGGER (stage with operator)
- **B4 Dragon model fixes** (TODO.md): spike-as-feet joint binding, wing-fan
  radius, dorsal glow — visual, needs device A/B.
- **A4 Hero faithful pass**: per-hero detail closer to the forge JS; optional 2D
  reliefs in the menu.
- **B5 Mount/ride a tamed dragon**: bond_tame → mount → fly. Bigger systems work.
- **C4/C5 Combat feedback + night danger**: hit flashes/knockback/mob health;
  hostile night spawns via the spawn director.

## Sequencing rationale
Front-load the **survival HUD** (turns invisible depth into a playable loop) and
**dragon flight** (the headline "real dragon" feel) because they're high-impact,
pure-Rust, and compile-verifiable. New survival pillars (thirst/stamina) plug into
the HUD from Roll 1. Element gameplay comes after the meters exist to react to it.
Device/shader and large-systems work (mounts, night raids, model fixes) come last,
staged with the operator since they need on-hardware verification.

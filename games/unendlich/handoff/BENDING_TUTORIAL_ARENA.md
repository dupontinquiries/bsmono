# Bending Tutorial — bounded practice arena (r661)

## What happens now
Menu → Singleplayer → **Bending Tutorial** launches a fixed-seed survival world
(`TUTORIAL_SEED = 0xBE57_1233`, name "Bending Tutorial") and, ~90 frames after the
spawn chunks load, stamps a **bounded arena** at spawn:
- a `(2·HALF+1)`-square stone-brick floor (`HALF=11` → 23×23) on a bedrock
  underlayer,
- a `WALL_H=5`-high perimeter wall (bedrock base + brick), so the player is
  contained (can't jump a 5-high wall in survival),
- the interior air column cleared so surrounding terrain never intrudes,
- **seven element pedestals** in a ring, each a 2-high cobblestone pillar topped
  with a marker block: Earth→Stone, Water→Ice, Fire→Magma, Air→Glass,
  Storm→Crystal, Brine→Quartz, Information→GlowCrystal,
- a gold dais at the centre where the player starts.

## Where it lives
- `sim/tutorial_arena.rs` — **pure** geometry (`arena_blocks([px,sy,pz]) ->
  Vec<([i32;3], BlockId)>`), 4 tests green standalone. Later list entries override
  earlier ones at the same cell (dais/pedestals over floor/air), so apply in order.
- Bin: pick 9 (Tutorial) sets `fg_pending_load = Some(("Bending Tutorial",
  TUTORIAL_SEED, true))` + arms `fg_tutorial_arena_pending`; a one-shot block near
  the landmark placement stamps the arena via `chunk_manager.set_block`.

## Not yet (follow-ups)
- The arena is scenery — the actual bending *lesson* (the `tutorial_ladder` 7-beat
  ladder from r645) isn't driven yet; the pedestals are visual anchors for it.
- Element pedestals aren't interactive (no "stand here to practise element X").
- The world is a normal survival world apart from the stamped arena (infinite
  terrain still exists outside the walls; the walls just bound the play space).

## r666 — arena trainers
Two "trainer" creatures (Lauscher + Spaeher — the watchful observer kinds) now
spawn inside the arena when it's stamped, ~4 blocks out from centre on opposite
sides, for the player to practise bending near. They're built through a new shared
`build_roamer(kind, pos, wseed)` helper factored out of the H-key spawn (so genome/
senses/turn-character/ecology are identical to a normally-spawned creature) — the
H-key spawn now calls the same helper, so there's one construction path.

Follow-up: the trainers are ordinary wild roamers (they wander/flee). A real
"trainer" behaviour (stand at a pedestal, demo an element, react to the player's
bending) would need a dedicated tutorial-actor mode — not wired yet.

## r679 — direct launch (no wizard) + enclosed hub + doc alignment
Playtest: "the bending arena still opens a normal world AND shows the create/seed
page — should be a fully different unique tutorial area." Two root causes fixed:

1. **Seed-page flash** — r663 launched by opening the create wizard and walking it
   (fg_create_spree), which drew the seed page for several frames. Replaced with a
   DIRECT path: new `AppInput::StartTutorial(seed)` → `MenuController::start_tutorial`
   builds a fixed `WorldSpec { "Bending Tutorial", seed, Creative }` and drives the
   outer FSM MainMenu → Loading (mirroring `confirm()`'s `flow.apply(Title(Select(
   NewGame)))`) WITHOUT ever creating the wizard. No wizard = no seed page. pick=9 in
   the bin now emits `StartTutorial(TUTORIAL_SEED)` + arms the arena stamp; the old
   fg_tutorial_pending/wizard-spree block and its flag are removed.

2. **"Open world" feel** — the arena was open-topped, so the surrounding procedural
   terrain was visible over the walls. `tutorial_arena` now adds a **solid roof** at
   `sy + WALL_H + 1` over the whole 23×23 (4×4 GlowCrystal lattice for light, brick
   otherwise), so the hub is a fully enclosed interior chamber.

### Still not a true flat/void world (documented limitation)
The world is still procedurally generated — the enclosed hub sits *inside* real
terrain; you just can't see out. A genuinely self-contained flat/void tutorial
world (nothing beyond the hub) needs a **flat-terrain generator mode**, which does
not exist (`WorldGenerator` is always procedural via `WorldModel`). That's the real
next step for full doc fidelity. Per `01_World_and_Environment/tutorial_system.docx`
the hub should ultimately be: a central **Resonance Font crystal** ringed by **seven
element zones** (Earth/Water/Fire/Air/Storm/Brine/Information), each with a wizard-
teacher NPC and an element-material "display screen", running the beat sequence
(Arrival → Element sense → First voice shape → First technique → Choir note → First
harmonisation → What comes next). Current hub has the 7 element pedestals + a centre
dais; Font crystal, per-zone screens, wizard NPCs, and the beat script are still to
build (the r666 trainers are 2 generic roamers, not per-zone teachers).

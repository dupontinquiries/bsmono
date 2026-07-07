# r663 — tutorial launch fix + character defaults; roadmap for the rest

## Fixed this rev
### Bending Tutorial now actually enters the world
r661 launched via `fg_pending_load`, which rebuilt terrain but never drove the App
FSM — so `fg_app.state()` stayed `MainMenu`, `menu_open` stayed true, and it bounced
back to the title (the "clicks through, cursor vanishes, back to menu, no arena"
symptom; the code even warns this bypass "bounced back to the menu"). Now the
Tutorial button opens the create wizard, and the bin pins the tutorial seed/name +
`GameMode::Survival` and one-click launches through the SAME path Create uses
(`fg_create_spree`), which drives MainMenu → Loading → InGame properly. The arena
then stamps at spawn. Survival keeps the 5-high walls containing the player.

### Character archetype defaults (the "start" on character work)
`hero_forge::default_params(HeroKind)` + `height_range(HeroKind)` — pure, tested
per-hero proportions so each archetype has a distinct silhouette:
- Windwalker (Air): tall, lean, long-limbed (2.0)
- Stormweaver (Air): tall, broad-shouldered (1.95)
- Heliographer (Fire): balanced, sturdy (1.85)
- Gravitybender (Earth): short, heavy, powerful (1.75, bulk 1.35)
- Tidecaller (Water): shorter, slighter (1.70)

**Wiring TODO (not done — would desync model vs physics if done at the build site):**
apply `default_params(kind)` at CHARACTER CREATION, i.e. when a new character's name
resolves to a `HeroKind`, set `character.build = default_params(kind)` so the model
AND the derived stature (collider/mass/eye-height, from `active_character.build`)
agree. The build site at unendlich_fullgame.rs ~11838 only feeds the model, so
overriding there alone would make a Windwalker *look* 2.0 but *move* like 1.8.

## Remaining asks — plan
1. **Item models (only colours):** slots draw `rect_filled(display_color)` — there's
   no per-item icon/mesh. Options: (a) a small procedural voxel-preview render per
   ItemKind (reuse the entity/voxel renderer to bake tiny icons), or (b) a glyph/
   shape per item CATEGORY over the colour (quick win). (b) is the cheap first step.
2. **Creative hotbar inconsistency + "drag":** the creative catalog HOTBAR row
   (creative_hotbar) and the bottom in-game hotbar show different things, which reads
   as "empty/old/inconsistent". r657 made it CLICK-based (drag never worked in egui
   here). Next: make the creative catalog hotbar mirror the REAL hotbar slots so
   there's one source of truth, and keep click-to-assign.
3. **More hero types:** the JS character-forge has more archetypes than the 5 ported
   `HeroKind`s — port the remaining builders (skeleton + buildBody colour maps) as new
   variants, each with a `default_params`/`height_range` entry.
4. **Tiny arena "1–2 chunks" + trainers:** current arena is a 23×23 walled room
   stamped in an open world. To make it a self-contained 1–2 chunk space, either a
   void/flat tutorial worldgen or a taller perimeter + roof. Trainers = spawn a couple
   of creatures at the pedestals (reuse the H-key Roamer construction).

## r664 — hero default builds WIRED
`presets()` (player_character.rs) now post-processes each preset: if its name
resolves to a `HeroKind`, its `build` is set to `hero_forge::default_params(kind)`.
So the 5 hero archetypes now render (and move) with distinct silhouettes:
- Windwalker 2.00 tall/lean, Stormweaver 1.95 broad, Heliographer 1.85 balanced,
  Gravitybender 1.75 heavy (bulk 1.35), Tidecaller 1.70 slight.
Because the collider / eye-height / mass all read `build`, look and physics agree.
Non-hero presets (Wanderer, Strider, …) keep the generic 1.8 build. The editable
CUSTOM slot is untouched (player-controlled).

### Still open (unchanged priority)
- **More hero types:** needs the JS character-forge source (not in this tree) to
  port skeletons/colour-maps faithfully, OR invent new archetypes with their own
  `default_params`/`height_range`. Say which and I'll add them.
- **height_range per hero in the customizer slider:** `hero_forge::height_range`
  exists but the character customizer's height slider isn't clamped by it yet.
- **Creative hotbar consistency:** two hotbars (panel `creative_hotbar` row vs the
  always-on bottom HUD hotbar) draw from different sources → "empty/inconsistent".
  Fix = one source of truth; needs a careful pass over both render sites.
- **Item icons** (colours only today) and **tiny 1–2 chunk arena + trainers**.

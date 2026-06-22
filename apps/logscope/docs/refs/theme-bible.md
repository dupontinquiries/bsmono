# Logscope — Environmental Theme Bible

A worldbuilding + design reference for the Logscope "living archive" UI. It collects every
scene description from our sessions, folds in the environmental concept-art breakdowns, and
synthesizes the additional data (dimensions, palettes, shader stacks, UI-mappings, audio,
presence cues, render tokens) needed to build each scene consistently in three.js **and**
to port it to the egui app later.

Measurements are worldbuilding estimates for shader/environment work, not architectural specs.

---

## 0. Design Thesis (applies to every scene)

The emotional weight of these references comes from **architecture, not widgets**. Five rules:

1. **Enclosure over void.** The UI never floats in empty space. It lives *inside* something —
   a ring of books, a dome, a cave, a colonnade. Walls give scale, direction, orientation:
   "I am in a place," not "where am I?"
2. **One dominant centerpiece.** Tree / dome / helix / fountain / orrery. It occupies ~10–20%
   of the volume but commands ~80% of attention because every sightline, fog gradient, and
   light vector funnels toward it. Everything else exists *relative* to it.
3. **A source of transcendence above.** Skylight / oculus / canopy gap. Light always arrives
   from beyond the visible system. In UI terms the ceiling = global context / latent space /
   long-term memory: visible but unreachable, patterns moving overhead.
4. **Concentric stone-and-water composition.** Center platform (current conversation) → shallow
   reflective water (memory) → stone terraces (branches) → archive walls (stored conversations)
   → dome above (global context).
5. **80% atmosphere, 20% object.** Volume, not furniture. The empty air — fog, shafts,
   reflections, droplets — is itself a design element.

**Presence without characters.** The space feels *inhabited yesterday, not occupied now*:
a chair pulled out, a lantern still warm, a page turning itself, ripples crossing still water,
a leaf drifting down, dust disturbed by something unseen. Evidence, never an NPC.

**Sunlight as the primary UI layer.** Instead of color/icons/panels, the interface speaks through
light, shadow, fog, reflection, motion. The active branch is simply the one a moving shaft is
currently illuminating; a forgotten conversation exists only in reflections beneath the water.

---

## Shared Design Language — "UI grown from nature"

The magic isn't UI placed *on top of* nature; the UI appears to have **grown out of** natural
phenomena. The user can't always tell where architecture ends and interface begins.

| Element | Behavior |
|---|---|
| **Stone buttons** | Smooth river stones half-emerged from water; size = importance (10cm command → 60cm). Material per role: black basalt = command, pale limestone = memory, red sandstone = dangerous op. Roughness 0.7 at rest → 0.15 on hover. |
| **Carved text** | Labels engraved 2–5mm into rock; near-invisible at some angles, perfectly readable when light rakes across — ancient inscription. |
| **Glowing veins** | Some text is mineral veins (quartz in granite) glowing softly from beneath the surface; the glow bleeds into nearby fog, lights water ripples, reflects off leaves. |
| **Gravitational air-ripple** | Important nodes distort space within a 1–2m radius — heat-haze / faint lensing. Dust curves around them, fog bends, leaves orbit lazily. Significance without an icon. |
| **Fog as information density** | Low fog = simple areas, dense fog = complex regions. Distant branches resolve gradually: silhouette → shape → detail. Knowledge *emerges* rather than appearing. |
| **Floating droplets** | Suspended individual droplets (1/5/20mm) drift and act as tiny lenses; you glimpse reflected text, upside-down shelves, miniature trees inside them. |
| **Reflective stone floor** | A microscopically thin water layer — not a mirror, not a puddle, *a memory*. Tree/fog/node reflections, distorted by slow movement. The floor appears alive. |
| **Paper interfaces** | Cotton-rag / vellum pages emerge from shelves, edges curled, ink absorbed not printed. Some hover and turn themselves; bookmarks drift like leaves. |
| **Leaves as displays** | Sunlight makes individual canopy leaves translucent; veins illuminate, words form across ~30 leaves, the wind moves them, the sentence dances and reforms. Or light projects *through* leaves to cast citation-text onto stone. |
| **Graph as floating ecology** | Nodes = stones / flowers / crystals / lanterns. Connections = roots / water currents / light paths. The graph feels *discovered*, not drawn. |

### Concentric spatial model (as built)
```
            oculus / skylight  (global context, overhead, unreachable)
                    │  volumetric shaft
   ┌──────────── archive wall (stored conversations, textured skin) ────────────┐
   │      outer terrace ring (branches)                                          │
   │           reflective memory water (spontaneous ripples = presence)          │
   │                central stone platform (current conversation)                │
   │                     CENTERPIECE  +  graph nodes orbiting                    │
   └─────────────────────────────────────────────────────────────────────────────┘
```

### Token architecture (three orthogonal data axes → flat token bag → widgets)
```
SCENE  (palette / world identity, "which place")   ─┐
SKIN   (widget material, "how chrome looks")        ├─►  resolve() ─► tok ─► widgets read tok only
VARIANT(atmosphere grading, "what mood")           ─┘
```
Adding a scene/skin/variant = one data row; widget code never changes (the anti-rot seam).

---

# Scene Chapters

Each scene lists: identity, emotional function, estimated dimensions, enclosure, centerpiece,
palette, lighting rig, material/shader stack, atmosphere, **Logscope UI mapping** (how the app
surfaces manifest in-world), presence cues, synthesized audio, motion, and the **render tokens
as actually built** (for the egui port).

---

## 1 · FOLDOUT  (Folding@Home)

- **Identity:** A biology-inspired computation chamber. The name is *protein folding* — the
  centerpiece is a **double helix**, not standing stones. (Stones removed.)
- **Emotional function:** Active computation, life-as-process, quiet awe at molecular machinery.
- **Estimated dimensions:** chamber Ø 30–40 m · helix height 8–12 m · backbone radius 1.5 m.
- **Enclosure:** dark stratified-stone rotunda (cliff-like walls), faint blue rune/lightning glow
  threading the rock; the room reads as a deep vault around the helix.
- **Centerpiece:** rotating DNA/protein **double helix** — two smooth tube backbones on
  Catmull-Rom curves, base-pair rungs, amino-acid bead spheres in 4 residue colors, biological
  motes drifting around it; soft blue + teal glow.
- **Palette:** sky `#030210→#0d0620`, accent gold `#d4a847`, particles `#4466ff`,
  helix strands `#3a78ff`/`#33ccff`, residues `#ff5566 #55ddaa #ffcc44 #66aaff`.
- **Lighting rig:** key electric-blue `#4466ff` from above (i 3.0), warm gold fill `#d4a847`,
  violet rim `#8844ff`; god-ray tint cool blue `(.25,.4,1.0)`.
- **Material/shader:** matte stratified stone walls (mode 1), emissive helix, additive motes.
- **Atmosphere:** stars on, no volumetric fog; lightning filaments flicker in the wall shader.
- **Logscope UI mapping:** conversation **nodes = amino-acid beads** threading the helix; an
  active "fold" = the strand segment currently lit; *Coordinator* prompting modes visualized as
  parallel strands folding in sync. Context cards = residue tooltips.
- **Presence cues:** motes that pause and re-disperse as if stirred; a faint pulse running the
  helix like a heartbeat.
- **Audio (synth):** sub-bass hum, occasional crystalline "tick" as a base pair locks, distant
  cavern drip.
- **Motion:** helix slow Y-rotation (~0.35 rad/s), motes vertical drift, glow heartbeat.
- **Render tokens (built):** `fog[#04020e,6,28]` · `amb[#141028,0.7]` · key `#4466ff@3.0 (0,6,3)` ·
  fill `#d4a847@1.0` · rim `#8844ff@1.6` · panel `{rough .28, metal .80, env 2.0}` ·
  floor `#08060e r.04` · style `stone` · panel-opacity `.9` · wall `#1a1430→#2a2150` strata ·
  shaft `(.4,.5,1.0)` oc `#6688ff`.

---

## 2 · GROVE LIBRARY

- **Identity:** A shaded **circular library grove** — an ancient tree growing through the middle
  of a round, balconied archive. *Best on Atmosphere 10 · Golden Hour.*
- **Emotional function:** Reverence, accumulated knowledge, living memory. The civilization was
  built *around* this tree.
- **Estimated dimensions:** library Ø 80–120 m · tree height 20–30 m (stout) · canopy Ø 30–40 m ·
  trunk Ø 3–5 m · bookshelves 10–15 m · upper walkway 8–12 m.
- **Enclosure:** continuous ring of **ancient, tightly-packed books**, a balcony, a curved wall,
  no visible exterior. A containment vessel that turns "tree" into "archive / history / reverence."
- **Centerpiece:** stout, **leaning** tree; **triplet fractal branching** (trunk → 3 → 3),
  thick mossy trunk, hanging vines with leaf tufts, bushy leaves crawling the base, a **two-tier
  canopy of individual leaf cards** with a rounded crown so **dappled light** passes through gaps.
  Node-labels are engraved on select leaves and flutter; white doves implied.
- **Palette:** warm browns — sky `#140d05→#3a2810`, accent amber `#c8923c`, leaves
  `#3e8a22`/emissive `#1e5210`, bark `#3a2410`, moss `#3a6e22`.
- **Lighting rig:** warm sun key `#ffd886` (i 2.9), green-gold fill `#8a6a32`, warm rim `#ffe09a`;
  god-ray warm gold `(1.0,.88,.45)`. Dappled canopy filtering + ground bounce (green from leaves,
  warm from books).
- **Material/shader:** **wood-grain panel skin** (UI panels carved from wood); leaf cards are
  alpha-tested double-sided planes sharing one texture; bookshelf wall is a curved cylinder with a
  generated book-spine texture.
- **Atmosphere:** light golden volumetric haze `(.5,.42,.2)`; dust shafts; a single leaf drifts
  down and resets.
- **Logscope UI mapping:** **leaves = graph nodes / displays** (text dances across them in wind);
  bookshelf ring = stored conversations / source repositories; quick-action buttons are **carved
  wooden buttons with leather corner caps and indigo-dyed borders**. Branches = conversation
  branches; sunlight reveal lights the active turn.
- **Presence cues:** drifting leaf, fluttering canopy, doves crossing, dust in the shafts.
- **Audio (synth):** birdsong, leaves rustling, distant page turns, wood creak.
- **Motion:** leaves flutter individually (additive on base rotation), gentle whole-tree sway,
  leaf drift, dappled light movement.
- **Render tokens (built):** `fog[#140d06,6,24]` · `amb[#2a1e0e,1.3]` · key `#ffd886@2.9 (2,7,3)` ·
  fill `#8a6a32@1.1` · rim `#ffe09a@1.1` · panel `{rough .55, metal .20, env .7}` ·
  floor `#140e06 r.85` · style `wood` · panel-opacity `.84` · wall `#2a1a0c→#4a2f14` bookshelf ·
  shaft `(1.0,.88,.45)` oc `#ffd070` · vol `(.5,.42,.2)`.

---

## 3 · MISTFALL  *(working name — see Naming Proposals)*

- **Identity:** A misty, low-gravity **floating-island waterfall** place — geometric stone rocks
  suspended over still reflective water, heavy mist. Glass-fronted interface.
- **Emotional function:** Suspension, calm, slow discovery; gravity feels negotiable.
- **Estimated dimensions:** open chamber Ø 40–80 m · island Ø 5–12 m · mist layer 1–3 m thick ·
  waterfall drop 8–15 m.
- **Enclosure:** partial — drifting stone terraces and a far fog wall rather than hard walls;
  the mist itself is the boundary.
- **Centerpiece:** cluster of **floating faceted stone islands** (icosahedral boulders with mossy
  caps) at varied heights, a translucent **waterfall sheet** with falling spray, a **reflective
  pool** at the base.
- **Palette:** teal/green — sky `#020c08→#041a12`, accent `#00ffaa`, rock `#4a5560`,
  moss `#2e5530`, water `#2a4555`.
- **Lighting rig:** key aqua `#00ffbb` (i 2.2), blue fill `#0088cc`, pale rim `#88ffdd`;
  god-ray green-teal `(.1,1.0,.7)`.
- **Material/shader:** **frosted-glass panel skin** (background shows through); physical-glass
  waterfall + pool with transmission; layered soft-sprite mist.
- **Atmosphere:** **heavy volumetric mist** `(.05,.85,.55)`; contact fog hugging the water 0.5–2 m;
  reflections double scene depth.
- **Logscope UI mapping:** each **floating island = a conversation branch** you hop between; the
  waterfall = streaming tokens; the reflective pool surfaces older/forgotten branches as
  reflections. Mist density = local complexity.
- **Presence cues:** islands bob slowly, mist breathes, ripples cross the pool.
- **Audio (synth):** waterfall white-noise bed, dripping, low reverberant air, distant stone groan.
- **Motion:** islands bob + slow yaw, waterfall spray loop, mist drift, pool shimmer.
- **Render tokens (built):** `fog[#030e0a,6,26]` · `amb[#041810,1.6]` · key `#00ffbb@2.2 (-1,6,2)` ·
  fill `#0088cc@1.1` · rim `#88ffdd@.8` · panel `{rough .15, metal .88, env 2.6}` ·
  floor `#030e08 r.06` · style `glass` · panel-opacity `.66` · wall `#0a2026→#103a30` strata ·
  shaft `(.2,.9,.8)` oc `#66ffcc` · vol `(.05,.85,.55)`.

---

## 4 · MARBLE LIBRARY  *(Reading Room)*

- **Identity:** A baroque classical **reading room** under a great oculus dome — marble, gold,
  sunbeams, papers on desks.
- **Emotional function:** Scale, mystery, orientation, scholarly hush. The dome is a sun; the room
  orbits it.
- **Estimated dimensions:** room Ø 45–60 m · wall 12–15 m · dome Ø 10–15 m at 18–22 m height ·
  bookcases 7–9 m · central floor Ø 20–25 m · arches 4–6 m wide, 12–15 m tall.
- **Enclosure:** concentric rings — oculus → arches (structural ribs, no hard corners) → gallery →
  book walls → desks → floor. Everything points to center.
- **Centerpiece:** a pair of **Corinthian marble columns** with entablature and gold flame-bowls;
  reinforced by the baroque hall behind (rows of fluted columns, packed shelves, coffered gold
  ceiling, central arched window).
- **Palette:** warm stone/gold — sky `#0e0b05→#281e08`, accent gold `#ddaa44`, marble `#d8cca8`,
  gold `#c8922a`.
- **Lighting rig:** bright warm key `#fff0c0` (i 3.8) from the oculus, gold fill `#cc9944`,
  amber rim `#aa7722`; god-ray warm `(1.0,.88,.55)`; 15–25 visible dust shafts.
- **Material/shader:** **marble panel skin** (dark stone + gold veining); near-mirror polished
  floor (r .03); SSGI-style warm bounce.
- **Atmosphere:** very low haze, just enough to catch shafts; heavy AO under furniture.
- **Logscope UI mapping:** desks = open documents / files; the oculus = global context overhead;
  shelves = archives; reading the active turn = the desk currently in a sunbeam.
- **Presence cues:** a chair pulled out, an open book, motes in the shafts, a page turning itself.
- **Audio (synth):** cavernous room tone, distant footsteps, paper rustle, a far-off cough echo.
- **Motion:** slow dust in shafts, flame-bowl flicker, subtle shaft drift.
- **Render tokens (built):** `fog[#1a1508,9,34]` · `amb[#281e08,1.0]` · key `#fff0c0@3.8 (0,10,5)` ·
  fill `#cc9944@1.5` · rim `#aa7722@1.2` · panel `{rough .04, metal .93, env 3.4}` ·
  floor `#18140a r.03` · style `marble` · panel-opacity `.9` · wall `#20180c→#4a3a1e` bookshelf ·
  shaft `(1.0,.9,.55)` oc `#ffe090`.

---

## 5 · BEACH LIBRARY

- **Identity:** The **Huntington Beach Library** mushroom fountains beside a sunset ocean —
  concrete column fountains with wide bowl tops, a sand foreground, waves.
- **Emotional function:** Warmth, openness, golden-hour nostalgia, public-civic calm.
- **Estimated dimensions:** terrace Ø ~20 m · fountain columns 2–3 m · bowls Ø 1.5–2 m ·
  pool basin Ø ~6 m.
- **Enclosure:** semi-open — a circular terraced basin and balustrade rather than full walls;
  the ocean horizon is the far boundary.
- **Centerpiece:** three **mushroom fountains** at staggered heights (fluted concrete columns,
  wide shallow bowls, water surfaces, rim drips + center jets), a circular pool basin with inner
  step ring and perimeter plants.
- **Palette:** sunset + sea — sky `#050810→#12243a`, accent cyan `#33ccee`, concrete `#4e4438`,
  water `#3d7a9a`, sun-orange rim `#ff7722`.
- **Lighting rig:** warm sun key `#ffe090` (i 3.5) low from the side, cool sea fill `#33aacc`,
  orange sunset rim `#ff7722`; god-ray warm `(1.0,.72,.28)` — the most dramatic shafts.
- **Material/shader:** frosted-glass panel skin; rough concrete + physical water; **background
  shader** paints sunset sky, ocean band with animated waves + sun glitter, and a wet-sand
  foreground with foam waterline.
- **Atmosphere:** stars faintly on (dusk), sea spray, warm haze.
- **Logscope UI mapping:** fountains = active "streams" (each conversation flows over a bowl);
  the pool = shared memory; sand foreground = scratch space; horizon = the unknown / new topics.
- **Presence cues:** water drips and jets, gulls implied, ripples in the pool, a towel/book left
  on the rim.
- **Audio (synth):** surf, fountain trickle, distant gulls, wind.
- **Motion:** rim drips falling + resetting, animated jets, ocean waves, sun glitter.
- **Render tokens (built):** `fog[#080f1a,8,26]` · `amb[#0e1828,1.2]` · key `#ffe090@3.5 (5,9,4)` ·
  fill `#33aacc@1.0` · rim `#ff7722@1.1` · panel `{rough .10, metal .82, env 2.8}` ·
  floor `#0a1620 r.05` · style `glass` · panel-opacity `.74` · wall `#142028→#244050` strata ·
  shaft `(1.0,.72,.28)` oc `#ffcc88`.

---

## 6 · OBSERVATORY

- **Identity:** A **bright, airy white-domed observatory hall** — pale marble, sunlit, a blue
  marbled oculus, white arches, warm wood reading tables. (Not dark/space — daytime and clean.)
- **Emotional function:** Clarity, optimism, expansive calm; the brightest scene.
- **Estimated dimensions:** hall Ø ~50 m · dome oculus Ø 10–15 m · arches tall and rounded ·
  reading tables along the floor.
- **Enclosure:** white ribbed arches and arched window niches encircling the hall; floor of light
  marble; wood tables.
- **Centerpiece:** an **orrery** — nested counter-rotating metal rings with planets and a glowing
  core, beneath the bright oculus.
- **Palette:** pale + bright — sky `#aec4de→#e8eef4`, accent indigo `#4466bb`, oculus blue glass,
  warm wood floor `#bcc6d4`.
- **Lighting rig:** bright warm sun key `#fff2dc` (i 2.6), cool sky fill `#bcd2ec`, pale rim
  `#cfe0f5`; high ambient 2.6; god-ray near-white `(1.0,.95,.82)`. Stars off (it's day).
- **Material/shader:** frosted-glass panel skin reads as dark UI on bright ground; light marble
  floor (r .12); bright-dome background shader (white arches, blue marbled oculus, wood tables).
- **Atmosphere:** airy, low fog far (22–60), gentle sun wash from the oculus.
- **Logscope UI mapping:** orrery rings = nested context scopes / orbiting branches; the oculus =
  global embeddings overhead; wood tables = working set / open files; bright = "everything legible."
- **Presence cues:** a chair turned, an open ledger, slow ring rotation, sun creeping across the floor.
- **Audio (synth):** bright airy room tone, soft mechanism ticks from the orrery, distant quiet voices.
- **Motion:** orrery rings counter-rotate, planets orbit, sun wash drifts.
- **Render tokens (built):** `fog[#d4dEeC,22,60]` · `amb[#dfe8f2,2.6]` · key `#fff2dc@2.6 (1,12,4)` ·
  fill `#bcd2ec@1.3` · rim `#cfe0f5@.7` · panel `{rough .12, metal .82, env 1.8}` ·
  floor `#bcc6d4 r.12` · style `glass` · panel-opacity `.8` · wall `#8a96a8→#c4cedc` (bright) ·
  shaft `(1.0,.96,.85)` oc `#fffff0`.

---

## 7 · ATLANTIS

- **Identity:** An **underwater dome** archive — a glass dome on a thick matte-stone drum, an
  **interior balcony for air-breathers**, blue-obsidian floor with diffused caustics, deep-ocean
  pressure and weight.
- **Emotional function:** Pressure, depth, sanctuary under immensity; protected silence.
- **Estimated dimensions:** dome Ø ~20 m · interior gallery ring · stepped platform Ø ~30 m ·
  ocean depth implied beyond the glass.
- **Enclosure:** the **dome itself** — heavy meridian/latitude stone ribs over darker, less
  transmissive glass (deeper-ocean weight), a thick double-walled drum, an exterior colonnade,
  and a three-step circular platform.
- **Centerpiece:** the built-out **dome** (glass cap + oculus crown + ribbing) sitting on the drum,
  with an **interior gallery walkway** (inward-facing balustrade) where air-breathers stand.
- **Palette:** teal/abyss — sky `#01060a→#021018`, accent `#00ffcc`, glass `#012028`,
  stone `#123842`/`#1b5560`, **blue-obsidian floor `#06141f`**.
- **Lighting rig:** key teal `#00ffcc` (i 2.8), blue fill `#0066ff`, aqua rim `#00eeaa`;
  god-ray teal `(.0,.95,.75)`.
- **Material/shader:** frosted-glass panel skin (most transparent, pop .6); **matte rough stone**
  (rough .62–.7, low emissive so shine sits *on* the stone, not eaten by glow); **blue-stained
  obsidian floor** at roughness .14 (diffused reflection, not mirror) with an animated
  **caustic overlay** (ocean-light filaments + soft sun-ray streak, edge-faded).
- **Atmosphere:** strong volumetric water `(.0,.8,.65)`; rising bubbles; fish silhouettes; caustics
  dancing across the floor.
- **Logscope UI mapping:** the dome interior = the safe "air" workspace; fish/bubbles = ambient
  background activity; the glass = the boundary between current context (inside) and the vast
  ocean of latent/global knowledge (outside); the gallery = navigation between branches.
- **Presence cues:** bubbles rising, fish passing, caustics shifting, a lantern glow inside the dome.
- **Audio (synth):** muffled deep-ocean pressure hum, sonar pings, bubble streams, distant whale call.
- **Motion:** bubbles rise + reset, fish swim and turn, caustics animate, glass shimmer.
- **Render tokens (built):** `fog[#020c14,6,28]` · `amb[#031416,2.2]` · key `#00ffcc@2.8 (0,7,3)` ·
  fill `#0066ff@1.8` · rim `#00eeaa@1.4` · panel `{rough .05, metal 1.0, env 4.2}` ·
  floor `#06141f r.14` (blue obsidian + caustic overlay) · style `glass` · panel-opacity `.6` ·
  wall `#041820→#0a3340` (smooth/glass mode) · shaft `(.1,.9,.8)` oc `#00ffcc` · vol `(.0,.8,.65)`.

---

# Atmosphere Variants (global grading layer — orthogonal to scene)

Multiplied on top of any scene; instantly comparable via the dropdown.

| # | Name | Character | Key params (exp / amb / fogN / fogF / rays / drop / shaft / vignette / tint) |
|---|------|-----------|---|
| 1 | Clear | neutral baseline | 1.25 / 1.0 / 1.0 / 1.0 / 1.0 / 1.0 / 1.0 / .38 / none |
| 2 | Hazy Archive | dusty warm haze | 1.15 / 1.1 / .7 / .6 / 1.1 / 1.3 / 1.1 / .45 / warm .18 |
| 3 | Cathedral | dramatic cool god-shafts | 1.30 / .8 / .9 / .85 / 1.9 / 1.2 / 1.7 / .55 / blue .14 |
| 4 | Candlelit | dim intimate amber | 1.02 / .6 / .8 / .7 / 1.2 / .8 / 1.2 / .62 / amber .26 |
| 5 | Misted Veil | dreamy low-contrast | 1.20 / 1.2 / .55 / .5 / 1.0 / 1.6 / 1.0 / .42 / teal .20 |
| 6 | Crisp Noon | bright clear minimal | 1.45 / 1.15 / 1.4 / 1.6 / .7 / .7 / .8 / .28 / white .05 |
| 7 | Dreamy Bloom | glowy rosy | 1.50 / 1.1 / .9 / .9 / 1.4 / 1.4 / 1.3 / .40 / pink .18 |
| 8 | Ink & Shadow | dark high-contrast | 1.00 / .55 / 1.0 / 1.1 / 1.1 / .9 / 1.1 / .70 / dark .22 |
| 9 | Abyssal | murky deep fog | 1.08 / .7 / .5 / .45 / .9 / 1.6 / 1.0 / .58 / teal .24 |
| 10 | Golden Hour | warm long rays | 1.35 / 1.0 / .95 / 1.0 / 1.7 / 1.2 / 1.5 / .44 / orange .22 |

*Locked favorite so far: Grove on Golden Hour (10).*

---

# Chrome Skins (widget-material layer — mirrors egui `theme::Skin`)

Field schema is shared with the egui end so a skin is portable line-for-line:
`radius, fill_rest, fill_hover, fill_down, fill_disabled, accent, grain, bevel_lit,
bevel_shadow, sheen, sheen_speed, vein[rest,hover], engrave_shadow, engrave_lip`.

| Skin | Feel | radius | grain | bevel(lit/shd) | sheen/spd | vein | engrave(shd/lip) |
|---|---|---|---|---|---|---|---|
| **Carved** | default stone, glowing veins | 6 | .18 | .30/.55 | .35/1.0 | .40→1.80 | .55/.30 |
| **Matte** | dead matte, glow killed | 4 | .26 | .12/.45 | .04/.4 | .12→.40 | .62/.18 |
| **Glass** | high sheen, bright lip | 10 | .05 | .55/.30 | .85/1.8 | .30→1.10 | .35/.55 |
| **High-relief** | deep bevel + engrave | 3 | .22 | .65/.80 | .50/1.2 | .55→2.30 | .80/.70 |

Swaps **crossfade** (`lerpSkin(a,b,t)` over ~0.45s); face textures repaint to target.
*Skin affinity suggestion:* Carved → Foldout/Marble; Glass → Mistfall/Beach/Atlantis;
Matte → Observatory; High-relief → Grove (wooden carving) & Atlantis stone.

---

# Naming Proposals (split the cave-family scenes)

The two cave-ish references are distinct scenes and should not share a name:

1. **Mistfall** (current) — the misty, low-gravity floating-island **waterfall** place.
   Candidate names: *Mistfall*, *Driftfen*, *The Hollow Tide*, *Nullgrave* (low-gravity), *Suiren*.
2. **The Lost Program** — the *ancient-alien-technology found inside a cave* scene (bioluminescent
   crystal cavern, arch, glowing glyphs). Candidate names: *The Lost Program*, *Substrate*,
   *The Deep Index*, *Glyphwell*, *Cold Archive*.

Suggested split: keep **Mistfall** for floating islands/waterfall; add **The Lost Program** as an
8th scene for the crystalline alien-tech cavern (emerald crystal pillars, bioluminescent river,
ancient glyph walls, teal glow) — its own tab, palette, and centerpiece.

---

# Appendix · Token field reference (for the egui port)

**Scene token (`SC[i]`):** `label, skyA, skyB, fog[color,near,far], amb[color,intensity],
key{c,p[x,y,z],i}, fill{…}, rim{…}, panel{c,r(ough),m(etal),e(nv)}, acc(cent), pt(particle),
godray{c[r,g,b],src[x,y,z],i}, stars(bool), vol[r,g,b]|null, floorCol, floorR, style, pop(opacity)`.

**Architecture token (`ARCH[i]`):** `wA,wB (wall tint pair), wM (wall mode: 0 bookshelf / 1 strata
/ 2 smooth), water, plat(form), terr(ace), shaft[r,g,b], sOp (shaft opacity), oc (oculus color)`.

**Variant token:** `exp, amb, fogN, fogF, rays, drop, shaft, vig, tint, tintA`.

**Skin token:** see Chrome Skins above (egui `theme::Skin` parity).

**Resolve pipeline:** `tok = resolve(sceneA, sceneB, blend, variant)` each frame; widgets read only
`tok.*`. Skin resolved separately via `lerpSkin`. This is the stable seam — add data rows freely,
widget/styler code never changes.

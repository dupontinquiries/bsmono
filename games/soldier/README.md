# EARLY-FPS

A small, complete first-person shooter written in Rust with a **modern-military
(Black Ops-era) feel** — regenerating health, aim-down-sights, sprint, magazine
reloads, hitmarkers, a kill feed, and a clean tactical HUD — but rendered the
old-school way: a **software renderer** that rasterizes the whole world into a
single `egui` mesh every frame. No GPU shaders, no image textures, no game engine.
Just triangles, a depth sort, hemispherical lighting, distance fog, and a
cinematic colour grade.

Procedurally generated arenas across **eight biomes** with real **terrain
elevation** (high ground, sunken lanes, ramps, rolling ground) and a **signature
landmark** at each map's heart, a four-gun loadout (assault rifle, SMG, shotgun,
sniper), hitscan combat, billboard particles, seek-and-shoot enemies, scorestreaks,
ammo resupply, drifting ambient haze, and escalating waves.

```
cargo run --release --features app
```

![urban](screenshots/preview_urban.png)
![military](screenshots/preview_military.png)

*(Headless render snapshots — generate your own with `cargo run --example preview --release`.)*

---

## Controls

Movement defaults to the **Dvorak** layout: `.` `o` `e` `u` sit in the same
inverted-T your fingers expect from WASD, just shifted to where those physical
keys land on Dvorak. Everything is rebindable in Settings, with a one-click
QWERTY preset.

| Input              | Action                          |
|--------------------|---------------------------------|
| `.` `o` `e` `u`    | Move (forward / left / back / right) |
| Mouse              | Look                            |
| Left click         | Fire                            |
| Right click        | Aim down sights (zoom, tighter spread) |
| `Shift` / `;`      | Sprint                          |
| `q`                | Crouch (hold)                   |
| `j`                | Combat dive                     |
| `p`                | Reload                          |
| `G`                | Deploy UAV (when earned)        |
| `H`                | Deploy Sentry Gun (when earned) |
| `F`                | Call in Missile Strike (when earned) |
| `T`                | Throw frag grenade              |
| `1` `2`            | Primary / sidearm (from your loadout) |
| Mouse wheel        | Cycle weapons                   |
| `Space`            | Jump                            |
| `Esc`              | Open menu / pause (releases the mouse) |

From the pause menu, **Settings** exposes mouse sensitivity, invert-look, field
of view, a minimap toggle, and fully rebindable movement keys (click a key, then
press the new one). Press `Esc` again — or hit Resume — to drop back into the
fight. Clear every enemy to advance to the next, larger wave.

Health **regenerates** after a few seconds out of fire (a red blood vignette
creeps in at the edges when you're low and fades as you recover), so there are no
health packs — only **ammo crates** scattered through the level and dropped by
kills to keep your reserve topped up. Each weapon has a magazine and a reserve
pool; reload with `R` (or it auto-reloads when the mag runs dry). Hits register a
**hitmarker**, kills post to the **kill feed** and bump your **score** and
**streak**. Take fire and a **directional damage arrow** flicks toward whoever
shot you.

**Scorestreaks** build from your score each life. Hit the first threshold and
`Q` calls in a **UAV** that marks every enemy on your HUD for a few seconds; the
next unlocks a **Sentry Gun** (`C`) — a deployable tripod turret that tracks and
fires on nearby enemies until its ammo or timer runs out; and the highest drops a
**Missile Strike** (`F`) that explodes across the whole enemy force. All three
reset when you go down, so spend them well.

Gunplay has a **recoil pattern** per weapon — the view climbs and wanders sideways
the longer you hold the trigger, so a sustained spray has to be controlled — and a
**reload animation** dips the weapon as it swaps mags.

Enemies come in four flavours, and the mix gets nastier in later waves: **grunts**
(baseline), **brutes** (slow, tanky, hit hard), **rushers** (fast, fragile, charge
into your face), and **marksmen** (slow, accurate, high-damage from range — they
back away to keep their distance, so you have to push them). Each has a distinct
silhouette and colour so you can read the threat at a glance.

---

## Building

The project is split so the **game and renderer depend only on `egui`** (pure
Rust). The native window — eframe's winit + glow stack — lives behind the
optional `app` feature.

Play it (needs a display):

```
cargo run --release --features app
```

Run the headless test suite (no display, no `app` feature — pure compute):

```
cargo test
```

If you build without `--features app`, the binary just prints these
instructions; everything except the window still compiles.

---

## How the two references shaped this

**Reference 1 — the egui software-3D renderer.** This is the load-bearing idea.
Its renderer takes a list of meshes, projects each vertex to the screen by hand,
shades it against a sun, sorts triangles back-to-front (painter's algorithm),
and hands egui one big vertex buffer. `early-fps` keeps that exact pipeline:
see `mesh.rs` (the `Mesh3` type, hemispherical `Light::sun`, box/panel/sphere/
tube/billboard primitives), `camera.rs` (the by-hand perspective projection with
near-plane handling), and `render.rs` (shading, near-clip, depth sort, fog, and
the conversion into an `egui::epaint::Mesh`).

**Reference 2 — the GLM-5 "port shooter to Rust" chat.** This supplied the
*game* architecture: a `shooter_map`-style grid level generator, billboard
particles built on the CPU so they always face the camera, a hitscan combat
module returning a single best hit, and a weapon table with per-shot spread and
a muzzle offset out in front of the eye. Those map onto `level.rs`,
`particles.rs`, `combat.rs`, and `weapons.rs` here.

The two converge on one technique: **everything in the world emits
`Vec<Mesh3>`**, and the renderer paints all of it the same way — geometry,
enemies, particles, tracers, and the gun viewmodel alike.

---

## Architecture

```
            input ─► player ─► game.step() ─► world state
                                   │
                  level + enemies + particles + tracers + viewmodel
                                   │
                          each emits Vec<Mesh3>
                                   │
                    render(): project ► shade ► fog ►
                       near-clip ► depth-sort ► egui::Mesh
                                   │
                         ui.painter().add(mesh)  +  HUD on top
```

Frame budget stays in the low thousands of triangles, so the CPU software path
runs comfortably — exactly the trade the period games made.

### Modules

| File           | Responsibility                                                    |
|----------------|-------------------------------------------------------------------|
| `math.rs`      | `Vec3` / `Vec2`, the small vector algebra everything uses          |
| `mesh.rs`      | `Mesh3`, lighting, and geometry primitives + ported logscope organic assets (recursive trees, leaf clusters, light shafts, stepping stones) |
| `camera.rs`    | FPS camera: yaw/pitch basis and hand-rolled perspective projection |
| `render.rs`    | Software rasterizer → `egui` mesh (shade, fog, alpha, cinematic grade, near-clip, depth sort) |
| `level.rs`     | Seeded generator: per-biome layouts (rooms / streets / bays / clearings), a terrain height field (tiers, ramps, noise), a signature landmark, lit fixtures, beams, skylights, cover props, pits + meshes |
| `weapons.rs`   | Arsenal (AR / SMG / shotgun / sniper): magazines, reserve, reloads, ADS, recoil pattern |
| `combat.rs`    | Hitscan raycast vs. walls (DDA), crates (AABB), and enemies (sphere) |
| `player.rs`    | Movement, sprint, ADS, gravity, collision slide, regenerating health, reloads  |
| `enemy.rs`     | Four enemy types (grunt, brute, rusher, marksman): detect, chase/kite, line-of-sight, shoot; low-poly bodies |
| `loadout.rs`   | Perks + the player's chosen kit (primary weapon, sidearm, three perk slots) and their effect modifiers |
| `cosmetics.rs` | Reticles, gun skins, and armor frames (visual + light stat tradeoffs) |
| `creatures.rs` | The Hunt mode: playable horror creatures + game-mode enum |
| `profile.rs`   | Persistent K/D metaprofile: kills/deaths, K/D, skill score, Bronze→Diamond tier |
| `db.rs`        | File-based save backend (profile + loadout) under the user's config dir |
| `particles.rs` | CPU billboard particles: muzzle flash, sparks, blood, smoke, shells, plasma |
| `pickups.rs`   | Hovering, spinning ammo crates: scatter, enemy drops, collection |
| `game.rs`      | Ties it together: stepping, firing, recoil, scorestreaks, sentries, waves, tracers, the gun viewmodel |
| `app.rs`       | eframe shell: mouse grab, fixed-timestep loop, HUD, minimap, pause menu + settings page *(feature `app`)* |

### Notes on the look

The world is built the way the logscope reference builds its procedural scenes —
no image textures, only per-vertex colour and light:

- **`tint_up`** colours upward-facing vertices one way and side-facing another, so
  floors and prop tops read as lit-from-above (snow on the ice rocks, bright crate
  lids).
- **`tint_height`** runs a dark-to-light gradient up the walls, faking ambient
  occlusion: grimy and shadowed at the floor, brighter toward the lights.
- **Emissive fixtures** actually motivate the lighting — glowing wall trim strips,
  hanging ceiling lamps on a grid, inlaid floor seams, ceiling beams overhead, and
  translucent skylight panels on the open-sky biomes.
- **Translucency**: the renderer carries per-vertex alpha through shading and paints
  back-to-front, so logscope's `set_alpha` glass/mist/skylights blend correctly.
- **Faceted geometry**: chunky displaced-sphere `rock`s, stacked `cairn` barriers,
  and (on the lava world) a small emissive `crystal` lava shard stand in for plain
  crates depending on biome.
- **Ambient haze** drifts through the air, tinted and animated per biome — rising
  embers on the lava map, falling snow on the ice map, fireflies in the jungle,
  faint dust elsewhere.
- **Ammo crates** bob, spin, and glow on floor pads (`disc` + `rotate_y_about` + the
  per-mesh emissive term).
- A **hemispherical sun** (cool sky above, warm bounce below) plus a 1.65× exposure
  boost gives a punchy lit base, then a **cinematic colour grade** (desaturation,
  teal shadows, warm highlights, an S-curve) pushes it toward a modern-shooter look;
  distance fog tuned to the map hides the far clip.
- A 3D gun viewmodel that rises to centre when you aim, drops to the hip when you
  sprint, and kicks with recoil; screen shake and a red hit-flash on damage.

**Biomes.** Each seed picks one of eleven themes — Industrial, Bunker, Warehouse,
Urban, Military, Volcanic (war-torn dusk), Ice (snowfield), a night Jungle, two
logscope-style organic groves (Canopy, Mistfall), and the Toxic Mire (an acid-green
swamp whose sludge burns) — each with its own palette, fog, sun colour, ambient
level, and cover-prop vocabulary (crates and hazard barrels indoors, faceted rock or
mossy boulders outdoors, lava-veined stone on the volcanic one). Practical ceiling lights and structural trim replace
the old neon strips.

**The Hunt.** An asymmetric mode with two sides. As the **Predator** you play a
horror creature (Revenant, Stalker, Brute, or Wraith — each trading speed for reach)
with a lethal claw strike instead of a gun, while the enemies become unarmed prey
that flee, break line of sight, and hide. As a **Survivor** you flip it: you're one
of the hunted — a fragile, unarmed human stalked by an AI creature alongside three
AI survivors — and must stay alive and out of sight until dawn while the creature
picks the group off. Pick your side and creature from the main menu, choose a
hunting ground, and either run them all down or outlast the night.

**Layouts.** The floor plan itself changes by biome — not just the palette. Four
carvers feed a shared pipeline (walls, terrain, landmark, props): an **interior
compound** of BSP rooms and corridors (Industrial, Bunker, Military); a **street
grid** of avenues and cross-streets between solid building blocks around a central
plaza (Urban); **open bays** with rows of container stacks forming aisles and
sightlines (Warehouse); and **organic clearings** — rough, noisy-edged open areas
linked by wide natural paths (Volcanic, Ice, Jungle). Every layout is verified
connected from spawn by a flood-fill test across all biomes and seeds.

**Terrain & landmarks.** The floor is a real height field: rooms sit on discrete
elevation tiers, the corridors between them ramp smoothly (relaxation), and the
outdoor biomes get rolling value-noise ground — so maps have high ground, sunken
lanes, and sightlines instead of one flat plane. Movement, gravity, and the AI all
follow the surface. At the centre of every map stands a **signature landmark**,
and each biome family gets its own silhouette: a **reactor tower** (interior
compounds), a **watchtower** you move under (Military), a **comms mast** with a
dish and aviation light (Urban), a **crashed dropship** with engine embers
(Volcanic), a **frozen monolith** of translucent ice crystals (Ice), and a
**ruined totem arch** you walk through (Jungle). Each anchors sightlines and, as a
blocked footprint, shapes how you move through the central area.

To eyeball the renderer without launching the window, `cargo run --example preview
--release` writes PNG snapshots of several biomes using the exact same `render()`
the game uses.

Each launch reseeds from the clock, so the arena layout, biome, cover, and enemy
placement differ every time.

# Echelon Art Direction — "worn machines under colored light"

Target: **SC2-fidelity forms, modern colored lighting & shadows, SC1 grit.**
Dark and specular, never glossy. Everything below is already encoded in
`assets/data/tilesets/*.ron`; the CPU look-dev renderer
(`echelon-runner preview <map> out.png`) is the ground truth any GPU pass
must match before adding features.

## The three laws

1. **Value discipline.** Terrain albedo luminance lives in 0.10–0.30
   (linear). Nothing on the ground exceeds ~0.5 except specular hits and
   emissives. Team colors are saturated but value-capped so the light rig
   — not the albedo — carries readability.
2. **Specular, not gloss.** Broad worn highlights: Blinn exponent 10–16
   (≈ GGX perceptual roughness 0.62–0.78), intensity 18–26%. Reads as wet
   basalt / oiled steel / damp stone. Never a tight plastic ping on
   terrain; units may go one notch tighter (`spec_pow + 8`).
3. **Grit everywhere.** Two-octave value-noise grime multiplied into
   albedo at 55–75% strength, plus micro height noise so the light
   *catches* the dirt. Cliff faces get strata banding and a contact-
   occlusion shadow band (0.45 floor) under every ledge — the SC1 smell.

## The light rig (per tileset)

Key `sun` (colored, ~1.35× intensity) + complementary `ambient` fill +
low grazing `rim` from the anti-key direction (squared falloff — it only
kisses silhouettes). The three shipped rigs:

| tileset       | key                | ambient           | rim            | mood |
|---------------|--------------------|-------------------|----------------|------|
| ashfall       | ember 255,150,84   | slate 36,44,58    | magma 120,44,30| volcanic night |
| verdigris     | cyan 168,214,235   | rust 52,38,30     | pale green     | cold ruin, warm bounce |
| relic_wastes  | gold 255,226,178   | indigo 30,32,52   | violet         | harsh dusk desert |

Resource emissives are constant across biomes (identity > mood): alloy
teal `60,255,214`, flux violet `176,108,255`, additive with soft falloff
and mild local tonemap so glows bloom without clipping.

## Bevy translation (M1.5 target)

Go 3D-lite, top-down orthographic — Bevy's 2D has no lighting, and this
look *is* lighting:

- **Terrain**: heightfield mesh from `GameMap` (same bilinear-over-tile-
  centers + micro noise as `png.rs`), vertex-colored from the tileset
  albedos (or a 4-way splat later). `StandardMaterial { base_color:
  albedo, perceptual_roughness: 0.62–0.78, metallic: 0.05, reflectance:
  0.35 }`. Multiply a tiling 512² grime texture (authored from the same
  value-noise) into base color at `grit`% strength.
- **Lights**: one `DirectionalLight { color: sun, illuminance: ~11_000,
  shadows_enabled: true }` aimed along `sun_dir`; `AmbientLight { color:
  ambient, brightness: ~140 }`; rim = a second shadowless directional at
  ~8% from the anti-key grazing angle (cheap and matches the reference).
- **Post**: `Tonemapping::TonyMcMapface` (keeps the darks rich),
  bloom with high threshold so only emissives bloom, optional very light
  film grain. No SSAO needed at first — the baked contact band in the
  terrain colors covers it.
- **Units (SC2 fidelity)**: silhouette-first meshes ~1–4k tris, one atlas
  material per race, roughness ~0.55, team color via mask channel,
  emissive accents used sparingly (weapon tips, vents). Until meshes
  exist, the client's flat quads should at least take the tileset's
  terrain tint + a fake N·L bevel like the look-dev renderer's slabs.
- **Shadows** are the "modern" tell: one crisp colored-key shadow, soft
  ambient. Keep cascade count low (top-down camera = tiny depth range).

## The vertical model (rendering must sell it)

Nine quantized levels: terrain L0–L5, air A0–A2 (levels 6–8). Units carry
`depth` (massive ground and capital ships fill two levels; townhalls
three, poking structure tops into low air). **Bridges are first-class**: a
tile can hold a walkable deck above walkable ground, with under-passage
gated by clearance (`terrain + unit_depth <= deck`). Rendering duties:
each level steps albedo toward the `high` pair and gains a touch of key
light; ramps read as their own worn material; chasms drop into 20% depth
fog; decks are flat lit slabs with plank banding, dark rail lips, and a
drop shadow cast onto whatever passes beneath — that shadow is what makes
"under the bridge" legible at a glance. Specular is slope-gated: flats
keep 15% broken sparkle, lips/cliffs/ramps catch the sheen. Air layers
render as altitude: A0 close over the deck line, capitals (A1–A2, depth
2) larger, slower shadows — their ground shadow offset is the tell for
which band they occupy.

## Models & animation (the CADscope pipeline)

Unit meshes come from `apps/cadscope` Mark II: per-race construction
grammars over four tagged parts (hull, plate, glow-emissive, dark),
deterministic per unit name, palettes matched to the faction consoles.
The same source drives four outputs: OBJ+MTL for the Bevy client,
roster sheets for silhouette review, `spin` turntable frames, and the
8-facing sprite atlas (`assets/gfx/echelon-atlas.*`) that the match
renderer blits — facing derived from the live path or held target, team
read from an underlay ring, altitude from the flyer shadow offset, plus
procedural animation the sim pays for free: hover bob phased per entity
and muzzle flashes on the exact tick a weapon cycles. Animated output is
APNG assembled by `tools/apng.py` (crop / posterize / step keep battle
films shippable); `--film DIR from frames every` on the runner dumps
the frames.

## Map flow language (why these maps look different from ladder SC2)

- **Confluence** (verdigris): high-ground mains with one ramp, naturals
  below — but both attack paths *converge* on one shared central plateau
  holding contested flux, instead of SC2's safe-third ring rotation.
  Fighting for the middle is the economy.
- **Cauldron** (ashfall): king-of-the-hill. One rich four-ramp plateau in
  the center, corner mains, void ribs that forbid straight rushes.
  Holding the cauldron grants interior lines; sieging it is the game.

Maps are two aligned char layers in RON (`height`: `. ^ / #`, `things`:
`a f 1 2`) generated by `tools/gen_maps.py` — edit geometry there, rerun,
`echelon-runner preview <map> out.png` to iterate on look and flow in one
loop. Player-prototyped SC2 maps convert via `tools/sc2map_extract.py`
(starts + resources + dimensions land automatically; cliff layers get
hand-quantized onto the height layer).

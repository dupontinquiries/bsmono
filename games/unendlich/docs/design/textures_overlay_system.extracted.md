
Textures & Overlay System
16×16 Natural Surface Tiles · Environmental Overlay Pipeline · Property-Driven Blending
All base tiles and overlays shown at 8× scale (128×128). Original resolution: 16×16 pixels. Tiles generated programmatically — this document is both spec and render output.

1.  Texturing Philosophy
All surface textures are 16×16 pixel art — a constraint that forces every pixel to carry meaning. Natural surfaces follow consistent palette logic: a base colour, a shadow colour (15–25% darker), a highlight colour (15–25% lighter), and 1–2 variation colours for texture complexity. Tiles are designed to tile seamlessly on all four edges.

Overlays are a separate semi-transparent layer system applied dynamically on top of base surfaces based on environmental property readings (H, R, W, accumulation). A stone tile in a wet cave is different from the same stone tile in a desert — the geometry does not change, but the moisture, erosion, and weathering overlays blend over it and shift it into something that reads correctly for the context.

Layer
Resolution
Purpose
Base Surface
16×16 opaque PNG
The geological or biological identity of the surface — what the material is
Environmental Overlay
16×16 RGBA PNG
The current condition of that surface — how the environment has affected it
Blend Result
16×16 composite
Alpha-composite of all active overlays applied to the base, in stacking order


2.  Base Surface Tile Catalogue
Every tile is shown at 8× scale (128×128 display pixels from a 16×16 source). Palette values are given in the notes beneath each group.

2.1  Geological Surfaces


Stone
Grey, crack lines

Cobblestone
Irregular block faces

Gravel
Mixed pebbles

Volcanic Rock
Dark red-grey

  Geological Palette Notes
  Stone:      128,128,128 base  |  148,148,148 highlight  |  88,88,88 crack
  Cobble:     120,120,120 face  |  160,160,160 highlight  |  70,70,70 mortar gap
  Gravel:     100,100,100 base  |  155,155,155 light pebble  |  60,60,60 deep gap
  Volcanic:   80,55,55 red-grey base  |  115,85,80 ridge  |  40,25,25 fissure

2.2  Soil, Sand and Organic Ground


Grass
Mixed greens, dirt base

Dirt
Warm brown mix

Sand
Warm tan, ripple lines

Mycelium
Purple-tinged fibrous soil

  Soil Palette Notes
  Grass:  106,158,64 base  |  125,184,73 blade tip  |  62,108,34 depth  |  122,82,48 dirt (rows 14–15)
  Dirt:   122,82,48 base  |  140,95,56 light  |  90,55,30 deep shadow
  Sand:   210,185,115 base  |  240,215,145 ripple crest  |  170,145,80 trough
  Mycelium: 110,70,110 purple base  |  150,100,150 fibre highlight  |  155,120,80 soil patch

2.3  Cold Climate Surfaces


Snow
White, blue-shadow base

Ice
Crystalline blue-white

Moss Stone
Grey + green moss patches

Wet Cave Stone
Dark grey + moss veins

  Cold and Moisture Palette Notes
  Snow:  255,255,255 peak  |  220,225,232 mid  |  160,175,195 compressed base (bottom rows)
  Ice:   220,235,255 surface  |  140,170,215 mid depth  |  100,130,180 deep crevice
         Crystal pattern: diagonal offset geometry suggesting facet planes.
  Moss Stone:  110,110,110 stone  |  75,120,60 moss highlight  |  55,95,42 moss shadow
  Cave Stone:  90,90,95 base  |  50,85,70 moss pocket  |  55,57,62 wet depression

2.4  Wood, Leaf and Water Surfaces


Oak Bark
Warm brown, vertical grain

Pine Bark
Darker, rougher grain

Leaf Canopy
Stippled greens

Water Surface
Diagonal ripple, deep blue


Desert Rock
Orange-tan, heat cracks

Gravel Path
(repeat for comparison)

Snow
(compare cold surfaces)

Ice
(compare cold surfaces)

  Wood, Leaf, Water Palette Notes
  Oak Bark:   92,62,42 base  |  112,78,52 ridge highlight  |  55,36,24 groove shadow
              Vertical grain: alternating light-base-dark columns at ~2–3px width.
  Pine Bark:  60,40,28 base (darker than oak)  |  95,65,45 ridge  |  35,22,14 crack
  Leaves:     60,100,40 mass  |  95,150,65 highlight tip  |  30,55,18 deep shadow
              Stippled: no two adjacent pixels (including diagonal) share same value.
  Water:      45,80,170 deep  |  65,110,200 mid  |  100,160,230 ripple crest
              Diagonal ripple: 45° lines, 4px bright then 4px mid-deep repeating.
  Desert Rock: 190,145,85 face  |  230,190,125 highlight  |  130,90,45 heat crack



3.  Overlay Tile Catalogue
Overlay tiles use the full RGBA channel — pixels range from fully transparent (α = 0) to semi-opaque (α = 180). The base surface shows through at all points. Overlays are shown below at 8× scale against a white background so their pixel pattern is visible despite partial transparency.


Moisture
Blue dots, α 0–180

Frost
Crystal growth, α 80–180

Heat / Arid
Red-orange cracks, α 70–150

Erosion
Brown pitting, α 90–160


Mud
Brown film, α 80–150




Overlay
Alpha range
Pixel pattern intent
Moisture
0 – 180
Sparse droplet matrix. Denser and higher-alpha as H increases. Zero-alpha pixels = dry surface beneath.
Frost
80 – 180
Dendritic crystal growth branching from corner inward. Alpha peaks at crystal centres. Asymmetric for directional character.
Heat / Arid
70 – 150
Primary crack (full row width) + branch cracks (partial). High-alpha at crack centres, sparse shimmer dots between cracks.
Erosion
90 – 160
Isolated pit pixels with high alpha surrounded by transparent surface. Bottom rows denser — simulates accumulated wear.
Mud
80 – 150
Horizontal film streaks. Thicker at bottom rows (gravity flow). Thinner near top (surface drying).


4.  Blended Demonstrations
Left: base surface. Centre: isolated overlay. Right: alpha-composited result. The right column is what the game engine renders.

Base Surface
Overlay Tile
Blended Result

Stone

Moisture

Wet Stone

Sand

Heat

Cracked Desert

Grass

Frost

Frosted Grass

Cobblestone

Erosion

Weathered Cobble

Dirt

Mud

Muddy Ground

Oak Bark

Moisture

Rain-Soaked Bark

Desert Rock

Erosion

Wind-Worn Desert

Snow

Moisture

Wet Melting Snow


5.  Environmental Property → Overlay Mapping
Each overlay is driven by one or more environmental property values from the Resonance Bending system (H, R, W, P, L, S, B). The table below gives exact activation thresholds and intensity scaling formulas.

Overlay
Primary
Threshold
Intensity formula
Moisture
H
H > 0.35
intensity = clamp((H − 0.35) / 0.45, 0.0, 1.0). At H=0.35: 10% alpha. At H=0.80: 100%. Also activates at 100% during any active rain event regardless of H.
Frost
R
R < 0.10
intensity = clamp((0.10 − R) / 0.10, 0.0, 1.0). Full crystal density at R=0. Crystal count and branch length also scale with intensity.
Heat / Arid
R
R > 0.60
intensity = clamp((R − 0.60) / 0.40, 0.0, 1.0). Hairline cracks at R=0.60. Full deep cracking and shimmer at R=1.0. Crack width scales with intensity.
Erosion
Time
Age > 30 days
Cumulative accumulation, not instantaneous. Accrues 2% per in-game day when H > 0.4 AND W > 0.2 simultaneously. Resets to 0 if block is replaced.
Mud
H + W
H > 0.6 AND W > 0.3
intensity = clamp((H−0.6)*0.8 + (W−0.3)*0.4, 0.0, 1.0). Soil-category surfaces only. Does not apply to stone, wood, or water tiles.

5.1  Overlay Application Order (Bottom to Top)
  Stacking order — applied sequentially, each blended onto running composite
  1. EROSION     — applied first (closest to base material, represents physical wear)
  2. MOISTURE    — water settles into erosion pits and surface depressions
  3. MUD         — mud film forms where moisture and wind combine on soil surfaces
  4. HEAT        — heat-crack pattern overlaid on geological surfaces
  5. FROST       — crystals form on top of all other layers (surface-most)

  Note: HEAT and FROST are physically mutually exclusive but not code-enforced.
  Transitional states (e.g. volcanic surface at altitude during cold snap) will show
  both simultaneously and look unusual — which is the correct visual outcome.

5.2  Surface Category Restrictions
Surface
Moisture
Frost
Heat
Erosion
Mud
Notes
Stone / Geological
✓
✓
✓
✓
✗
No mud — no organic binder
Soil (grass, dirt)
✓
✓
Partial
✗
✓
Heat causes soil crack separately
Sand / Desert
✗
✗
✓
✓
✗
Too dry for moisture, frost, mud
Wood / Bark
✓
✓
✗
✓
✗
Wood chars rather than cracks
Water Surface
✗
✓ (freeze)
✗
✗
✗
Frost converts water to ice tile
Snow / Ice
✓
✗
✗
✗
✗
Already frozen — no further frost
Volcanic Rock
✗
✗
✓
✓
✗
Always-hot — no moisture or frost
Mycelium
✓
✗
✗
✗
✗
Biological — no frost or heat


6.  Complete Reference Grid
All base tiles at 6× scale (96×96) for side-by-side comparison. Three rows of six tiles each.


Grass


Dirt


Stone


Cobble


Sand


Gravel


Snow


Ice


Oak Bark


Pine Bark


Leaves


Water


Cave Stone


Moss Stone


Desert


Volcanic


Mycelium


Wet Grass


6.2  All Overlay Tiles at 6× Scale

Moisture
α 0–180

Frost
α 80–180

Heat/Arid
α 70–150

Erosion
α 90–160

Mud
α 80–150


7.  Implementation Technical Specification
7.1  PNG Format
  Tile file specification
  Format:      PNG-32 (RGBA) — 8 bits per channel, 4 channels
  Dimensions:  16 × 16 pixels (all base and overlay tiles)
  Colour space: sRGB (gamma-corrected display)
  Alpha:        Pre-multiplied for overlay tiles; fully opaque (255) for base tiles
  Filter:       None (filter type 0 per scanline) — optimal for small solid-colour tiles
  Compression:  zlib level 6

  Naming convention:
    Base tile:    surface_<name>.png            (e.g. surface_stone.png)
    Overlay tile: overlay_<name>.png            (e.g. overlay_moisture.png)
    Variant:      surface_<name>_<variant>.png  (e.g. surface_grass_autumn.png)

7.2  GPU Blend Equation
  GLSL fragment shader — overlay application
  // Porter-Duff 'over' composite for a single overlay:
  vec4 base     = texture(base_sampler,    uv);
  vec4 ov       = texture(overlay_sampler, uv);
  float a       = ov.a * overlay_intensity;  // intensity from property system
  vec4 result   = vec4(mix(base.rgb, ov.rgb, a), 1.0);

  // For N stacked overlays (applied in order: erosion, moisture, mud, heat, frost):
  vec4 acc = base;
  for (int i = 0; i < NUM_OVERLAYS; i++) {
      if (overlay_intensity[i] > 0.001) {
          vec4 ov_i = texture(overlay_sampler[i], uv);
          float a_i = ov_i.a * overlay_intensity[i];
          acc.rgb   = mix(acc.rgb, ov_i.rgb, a_i);
      }
  }
  FragColor = acc;

7.3  CPU Property-to-Intensity Mapping
  C++ / Java pseudo-code — runs once per second per surface zone
  float computeOverlayIntensity(EnvProps p, SurfaceAccum accum) {
      float moisture = clamp((p.H - 0.35f) / 0.45f, 0.0f, 1.0f);
      float frost    = clamp((0.10f - p.R) / 0.10f, 0.0f, 1.0f);
      float heat     = clamp((p.R  - 0.60f) / 0.40f, 0.0f, 1.0f);
      float erosion  = min(accum.erosionDays / 50.0f, 1.0f);
      float mud = 0.0f;
      if (p.H > 0.6f && p.W > 0.3f)
          mud = clamp((p.H-0.6f)*0.8f + (p.W-0.3f)*0.4f, 0.0f, 1.0f);
      // Push to surface material uniform buffer
      uniforms.intensities = {moisture, frost, heat, erosion, mud};
  }
  // Erosion accumulation (tick once per in-game day):
  if (p.H > 0.4f && p.W > 0.2f)
      accum.erosionDays = min(accum.erosionDays + 1, 50);

7.4  Seasonal Palette Variants
Surface
Spring
Summer
Autumn
Winter
Trigger condition
Grass
Yellow-green
Deep green
Amber-green
Frost-bleached
L crossing below 0.5 for autumn; R < 0.1 for winter frost-bleach
Leaves
Pale new-flush
Deep green
Orange-crimson
Bare branch tile
L decline past 0.5 triggers autumn palette swap
Dirt
Wet dark brown
Standard warm
Dry lighter
Frost-grey
H drives seasonal wet/dry shift; R < 0.05 adds frost overlay
Water
Clear blue-green
Deep blue
Slightly dark
→ Ice tile
R < 0.05 replaces water tile with ice tile entirely
Sand
Standard
Standard
Standard
+ frost overlay
Sand palette is stable; only frost overlay applies in cold


End of Textures & Overlay System Document
16×16 tiles rendered in-document via pure-JS PNG encoder. No external image assets required — tiles are procedurally generated from palette + pattern data.

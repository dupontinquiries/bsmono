
Mob & Dragon Viewer
UI Design v2 — Ultra Rendering
Dragon Scale Shaders · Wing Membrane Translucency · Bioluminescence · Genome Strip Depth · FSM Floating Planes · Habitat Ultra-Environment
v2 adds the rendering layer: the creature is a natural history specimen under controlled museum light, with depth of field isolating the inspection region, and the habitat environment behind it soft enough to read as atmosphere rather than backdrop.

1.  Rendering Philosophy — The Creature as Natural History Specimen
The best natural history museum displays put the specimen in a diorama — a controlled environment with specific lighting designed to reveal the animal's structure, not just show that it exists. The background is accurate to the habitat but rendered soft enough to not compete with the specimen itself. Light is directional and reveals surface detail. The specimen is sharp; the world behind it is not.

This is the rendering model for the mob viewer. The creature is the specimen. The habitat environment is the diorama background — present, accurate, and soft. The lighting reveals scale geometry, membrane translucency, and material character. The depth of field tracks the player's inspection — hovering a body region pulls that region into sharp focus while the rest of the creature falls softly out.

  Rendering priorities — v2 additions to v1 structure
  P1: Dragon scale geometry at close zoom is the highest-priority rendering asset.
      Individual scales must show: geometric relief via POM, inter-scale cavity
      shadowing via SSAO, and material variation across the body.

  P2: Wing membrane translucency is the most cinematically compelling material.
      Light through the wing should show the blood vessel network and the
      structural bone as a darker internal silhouette.

  P3: The habitat environment is soft background — never competing with the creature.
      Bokeh blur at 28–40px. The environment communicates habitat, not geography.

  P4: Bioluminescent creatures produce real light in the viewer.
      A cave mycelium network emits light that falls on the environment around it.
      A glowing dragon eye illuminates nearby surfaces.

  P5: The genome strip and FSM graph have virtual depth.
      They exist in 3D space at different focal distances, reinforcing their
      relationship to the creature and to each other.




  🌿  Habitat Environment — Per-Creature Background Shaders

2.  Habitat Ultra-Environment
Each creature is displayed in a background environment that matches both its primary element and its habitat biome. The environment is rendered at 120% of the panel resolution then downsampled — this supersampling sharpens the caustic patterns and particle details before the DOF blur is applied, producing a more visually rich background even after blurring.

2.1  Habitat Layer Composition
The background is composited from three layers, rendered front-to-back:

Layer 1 (deepest): the elemental void shader from the bending menu system, running at full quality. This establishes the element identity — Storm dragon has crackling atmospheric depth, Water creature has abyssal pressure darkness.
Layer 2 (mid): habitat-specific geometry approximation. Not actual terrain — a soft parallax-mapped surface suggestion. A cave habitat has dripping stone texture visible in the deep soft focus. A coastal habitat has a horizon line where ocean meets sky. A forest habitat has layered canopy depth. These are rendered as parallax-mapped planes, not 3D geometry.
Layer 3 (closest to creature): ambient particles and atmospheric volume specific to the habitat. These particles exist at a virtual depth between the habitat background and the creature, giving the scene layered depth rather than a flat backdrop.

Creature type
Habitat background
Atmospheric layer
Fire dragon
Volcanic rift: magma glow from below, dark stone walls. Heat shimmer on midground.
Ember particles, volcanic ash drift, heat haze ripple distortion.
Air dragon
High altitude: cloud layer far below, clear blue above. Thin horizon line.
Drifting high-altitude ice crystals, atmospheric lens flare from sun position.
Water dragon
Deep ocean: pressure blue-black, distant bioluminescent drift, thermocline shimmer.
Ascending bubble columns, suspended particulate, teal bioluminescent motes.
Storm dragon
Thunderhead interior: dark grey cloud texture, internal arc flash, pressure dark.
Arc tracers, charged ion glow, rain streak in background layer.
Cave creature
Cave chamber: stone ceiling with stalactites in soft parallax, darkness.
Water drip particles, cave mycelium glow patches, dust mote drift.
Forest creature
Forest interior: layered canopy depth, god-ray shafts from above.
Leaf debris drift, pollen particle float, ambient dapple light shift.
Lauscher
Cave/forest interface: dark entry with forest light bleeding from behind.
Dust mote at the light-dark boundary, very sparse — the Lauscher's environment is minimal.

2.2  God-Ray Shafts — Volumetric Light in Habitat
For forest and cave habitats, volumetric light shafts (god-rays) pass through the background and illuminate particles in the atmospheric layer. These are rendered as a screen-space radial blur from the light source position, modulated by an occlusion mask derived from the habitat geometry layer:

  GLSL — volumetric god-ray for forest/cave habitat backgrounds
// ── Volumetric god-ray shader (screen-space radial blur) ──────────────────
// For forest and cave habitat backgrounds.

uniform sampler2D uOcclusionMask;  // 1.0 = light visible, 0.0 = occluded
uniform vec2      uLightSourceUV;  // position of sun/opening in screen space
uniform float     uDecay;          // 0.96 — each step dims the ray
uniform float     uExposure;       // 0.15 — overall ray brightness
uniform float     uDensity;        // 1.0 — how many steps sample the mask
uniform int       uNumSamples;     // 80 — quality

vec4 godRay(vec2 uv) {
    vec2  delta  = (uv - uLightSourceUV) * (1.0 / float(uNumSamples)) * uDensity;
    vec2  coord  = uv;
    float illum  = 1.0;
    vec4  colour = vec4(0.0);

    for (int i = 0; i < uNumSamples; i++) {
        coord -= delta;
        vec4 sample = texture(uOcclusionMask, coord) * illum;
        colour     += sample;
        illum      *= uDecay;
    }
    return colour * uExposure;
}

void main() {
    // Composite: scene + god-ray additive blend
    vec4 scene = texture(uSceneColor, vUV);
    vec4 rays  = godRay(vUV);
    gl_FragColor = scene + rays * uRayColor;  // element-matched ray colour
}



  🐉  Dragon Scale Shaders — POM, SSAO, Material Variation

3.  Dragon Scale Shaders
Dragon scale surfaces are the most complex material in the viewer. A dragon's body is not uniform — scale density, size, and material vary across body regions. Belly scales are lighter, smoother, and more iridescent than dorsal scales. Wing attachment scales are fused and overlapping. The jaw and brow have individual enlarged plates. Each region has distinct PBR parameters and a distinct parallax depth.

3.1  Scale PBR Material — Parallax Occlusion Mapping
  GLSL — dragon scale PBR with parallax occlusion and cavity SSAO
// ── Dragon scale PBR with parallax occlusion mapping ─────────────────────
// Each scale region has: albedo, roughness, metalness, normal, and depth maps.
// The depth map drives POM — apparent scale height from geometry.

uniform sampler2D uAlbedo;     // base colour with region-based tinting
uniform sampler2D uRoughness;  // smooth at scale tips, rough in inter-scale gaps
uniform sampler2D uMetalness;  // metallic sheen at scale tips (element-dependent)
uniform sampler2D uNormalMap;  // scale edge normals, inter-scale cavity normals
uniform sampler2D uDepthMap;   // scale profile depth — POM source
uniform sampler2D uSSAOMask;   // pre-computed cavity SSAO for inter-scale gaps

uniform float  uScaleHeight;   // 0.04–0.12 — POM depth scale
uniform float  uBodyRegion;    // 0.0=dorsal, 0.5=flank, 1.0=belly — blends params

vec2 parallaxUV(vec2 uv, vec3 viewDir) {
    // Steep parallax — 8 steps for close zoom, 4 at distance
    int   steps  = 8;
    float step_h = 1.0 / float(steps);
    float curr_h = 0.0;
    vec2  curr_uv = uv;
    vec2  delta   = viewDir.xy / viewDir.z * uScaleHeight / float(steps);
    float depth   = texture(uDepthMap, curr_uv).r;
    for (int i = 0; i < steps; i++) {
        if (curr_h >= depth) break;
        curr_h  += step_h;
        curr_uv -= delta;
        depth    = texture(uDepthMap, curr_uv).r;
    }
    return curr_uv;
}

void main() {
    vec2  pUV   = parallaxUV(vUV, normalize(vViewDirTangent));
    vec3  alb   = texture(uAlbedo, pUV).rgb;
    float rough = texture(uRoughness, pUV).r;
    float metal = texture(uMetalness, pUV).r;
    vec3  norm  = normalFromMap(uNormalMap, pUV);
    float ao    = texture(uSSAOMask, pUV).r;  // cavity darkening in gaps

    // Body region blend: belly is lighter, smoother, more iridescent
    alb   = mix(alb, alb * 1.35, uBodyRegion);     // belly is lighter
    rough = mix(rough, rough * 0.60, uBodyRegion);  // belly is smoother
    metal = mix(metal, metal * 1.80, uBodyRegion);  // belly has sheen

    // Inter-scale cavity SSAO
    vec3 lit = PBR_shade(alb, rough, metal, norm, ao);
    gl_FragColor = vec4(lit, 1.0);
}

3.2  Scale Material Variation by Element
Attunement
Scale material character
PBR parameters and special effects
Earth
Rough stone-like scales, matte, thick overlapping
Roughness: 0.85–0.95. Metalness: 0.0. Scale tips have mineral-dust accumulation darkening the albedo. POM depth: 0.10 (deep, heavy scale relief). Cavity SSAO is strong — deep inter-scale shadows.
Fire
Swept, heat-resistant, slight metallic lustre at tips
Roughness: 0.55–0.75. Metalness: 0.1–0.2. Tips have heat-blued specular shift (toward blue-grey at glancing angles). Scale underside: warmer albedo from internal heat. Heat glow from spine ridge: emissive 0.05.
Water
Smooth, overlapping, slightly translucent at edges
Roughness: 0.25–0.45. Metalness: 0.3–0.5. Iridescent micro-surface: thin-film interference coating at scale edge (rainbow shift at glancing angles). Belly: near-mirror smoothness. POM depth: 0.04 (shallow — scales are smooth).
Storm
Bifurcated scales with gap for arc passage, slightly raised
Roughness: 0.40–0.60. Metalness: 0.4 (conductive). Gap between scale sections glows faintly at arc discharge points (emissive 0.12). Electrostatic effect: fine hair/spines at scale edge slightly raised — driven by vertex shader displacement.
Air
Lightest scales, partially transparent, high-aspect thin
Roughness: 0.30–0.50. Metalness: 0.15. Scale edges are rendered with alpha fade — the scale thins toward its tip and becomes translucent. This requires front-and-back rendering of the scale geometry.
Brine
Crystalline mineral deposits visible between scales, dense
Roughness: 0.65–0.80. Metalness: 0.1. Brine crystal deposit visible in the POM depth in inter-scale gaps. Crystal deposits are higher albedo (white-purple) than scale surface. Cavity SSAO inverted at crystal locations.



  🦋  Wing Membrane — Translucency, Blood Vessels, Stress Visualisation

4.  Wing Membrane Ultra-Rendering
The wing membrane is the most cinematically distinctive surface on a dragon. Thin enough to be translucent when backlit — a flying dragon passing between the viewer and a bright light source reveals the blood vessel network as a dark branching pattern through the membrane. In the viewer, the habitat lighting is arranged to produce this translucency when the creature is in a wing-spread pose.

4.1  Dual-Sided Translucent Material
  GLSL — dragon wing membrane: dual-sided, transmission, vessel and bone maps
// ── Wing membrane: dual-sided, translucent, SSS + back-light transmission ──
// Rendered twice: front-face and back-face with different lighting models.

// Forward scattering transmission — light from behind shines through thin areas
vec3 backLightTransmission(vec3 lightDir, vec3 normal, vec3 albedo,
                            float thickness) {
    // Light passing through the membrane: more through thin areas
    float transmission = (1.0 - thickness) * 0.85;
    // Forward scatter angle: strongest when light is directly behind
    float fwd_scatter  = pow(max(0.0, dot(normalize(-lightDir), normalize(normal))), 2.0);
    vec3  transmitted  = albedo * uBackLightColor * transmission * fwd_scatter;
    return transmitted;
}

// Membrane thickness map: 0.0 = very thin (near-transparent), 1.0 = opaque
// Thinnest: between membrane panels, near outer edge
// Thickest: along structural bones, near wing root

void membrane_frag_main() {
    float thickness  = texture(uThicknessMap, vUV).r;
    vec3  albedo     = texture(uAlbedoMap,    vUV).rgb;
    vec3  normal     = normalFromMap(uNormalMap, vUV);

    // Blood vessel network: darker branching pattern from vessel albedo map
    float vessel     = texture(uVesselMap, vUV).r;  // 0=open, 1=vessel
    vec3  vessel_col = vec3(0.55, 0.12, 0.10);      // dark red-brown blood
    albedo = mix(albedo, vessel_col, vessel * 0.65);

    // Front face: standard PBR with low roughness (smooth membrane)
    vec3 front = PBR_shade(albedo, 0.35, 0.05, normal, 1.0);

    // Back-light transmission for backlit membrane
    vec3 back  = backLightTransmission(uMainLightDir, normal, albedo, thickness);

    // Bone structure: visible as silhouette from behind (darker + opaque)
    float bone = texture(uBoneMap, vUV).r;
    vec3  bone_col  = albedo * 0.40;  // bone is darker than membrane
    float bone_opac = mix(0.0, 1.0, bone);

    // Composite: front + back-transmission + bone opacity
    vec3 result = front + back * (1.0 - bone_opac);
    result = mix(result, bone_col, bone_opac * 0.6);

    gl_FragColor = vec4(result, mix(0.75, 1.0, thickness + bone_opac));
}

4.2  Wing Stress Visualisation — Active During Flight Poses
When the viewer displays the dragon in a flight or stoop pose, the wing membrane receives an additional stress visualisation layer. Wing stress from aerodynamic loading is calculated from the pose (bank angle, load factor) and displayed as a colour gradient on the membrane:

Low stress (cruise, level flight): membrane colour unchanged. The natural translucency is the only effect.
Medium stress (banking 40°, climbing): a faint amber gradient appears at the membrane root and along the leading edge — the highest tension zones. The amber is subtle (alpha 0.15) — it tints rather than covers.
High stress (stoop, hard bank > 60°): the leading edge shows a pronounced strain gradient from amber (root) through orange-red (mid-span). The outermost membrane edge shows the maximum stress as a near-red tint. Blood vessels in high-stress zones appear more prominent — the increased blood pressure makes them thicker in the vessel map.
Structural bone highlight: at high stress, the structural bones that support the membrane emit a faint amber-gold edge glow (emissive 0.08) — the bones are working.
The stress colour never covers the translucency — the light-through-membrane effect remains visible through the stress layer at all times.


  ✨  Bioluminescence — Volumetric Emission, Light Bleed, Cave Materials

5.  Bioluminescent Material Rendering
Bioluminescent creatures — cave mycelium, bioluminescent marine organisms, glowing cave fauna — emit real light in the viewer. The emission is volumetric, not flat: the light bleeds into the surrounding atmosphere and onto adjacent geometry. A cave mycelium network illuminates the stone surface it grows on. A bioluminescent dragon's eye illuminates the nearby cheek scales.

5.1  Volumetric Emission Rendering
  GLSL — volumetric bioluminescence: falloff, biological pulse, accumulation
// ── Volumetric bioluminescent emission — light bleeds into adjacent geometry ──
// Applied to cave mycelium, marine organisms, glowing creature features.

struct BioLightSource {
    vec3  position;      // world-space emission point (or surface point)
    vec3  colour;        // emission colour
    float intensity;     // 0.0–1.0 — biological emission intensity
    float radius;        // falloff radius in world units
    float flicker_rate;  // Hz — biological pulse frequency (0 = steady)
    float phase;         // per-source random phase offset
}

// Falloff: biological emission decays faster than a point light
// because the organism's emission geometry is small and diffuse.
// Using inverse-square with a linear component for the soft near-field.
float bio_falloff(float dist, float radius) {
    float r = dist / radius;
    return max(0.0, 1.0 - r * r) * max(0.0, 1.0 / (1.0 + dist * dist * 0.5));
}

// Biological pulse — slow sinusoidal variation with slight asymmetry
float bio_pulse(float time, float rate, float phase) {
    if (rate < 0.01) return 1.0;  // steady — no pulse
    float t = sin(time * rate * 6.283 + phase);
    // Asymmetric: quick rise, slow decay (biological charge-discharge)
    return smoothstep(-1.0, 1.0, t * 0.6 + 0.4);
}

vec3 accumulateBioLight(vec3 fragPos, vec3 fragNormal, BioLightSource[32] sources) {
    vec3 total = vec3(0.0);
    for (int i = 0; i < 32; i++) {
        vec3  dir   = sources[i].position - fragPos;
        float dist  = length(dir);
        float fall  = bio_falloff(dist, sources[i].radius);
        float pulse = bio_pulse(uTime, sources[i].flicker_rate, sources[i].phase);
        float ndotl = max(0.0, dot(fragNormal, normalize(dir)));
        total += sources[i].colour * sources[i].intensity * fall * pulse * ndotl;
    }
    return total;
}

5.2  Cave Mycelium — Full Material Stack
The cave mycelium material shown in the mob viewer uses three layered techniques: the texture generator's noise-based albedo (from the blocks manifest v2.2), volumetric bioluminescent emission, and screen-space backlight scatter for the hyphal threads:

  Cave mycelium material layers
  LAYER 1 — BASE SURFACE (albedo from texture generator):
    Dark substrate (8, 14, 10) RGB. Hyphal thread network from FBM threshold,
    green-teal (22, 188, 120). Rendered as the base PBR surface.

  LAYER 2 — EMISSION (volumetric glow from hyphal threads):
    Each hyphal thread segment is a bio-light source at low intensity (0.08–0.15).
    Pulse rate: 0.15 Hz — a slow, barely-visible biological rhythm.
    The accumulated light from all threads illuminates the cave stone beneath.
    Visible effect: the mycelium glows and the stone it grows on is softly green-lit.

  LAYER 3 — BACKLIGHT SCATTER (makes threads visible against dark stone):
    When the habitat lighting comes from an angle, the threads are backlit.
    A thin-surface SSS pass makes the hyphae appear to glow from within
    even on their dark sides — biologically accurate.

  BONUS — SPORULATION EFFECT (on breath / disturbance):
    When the creature near the mycelium moves, spore particles emit from
    dense mycelium areas. Each spore is a small bioluminescent particle
    that drifts upward and fades over 3–6 seconds.




  🧬  Genome Strip — 3D Elevation and Gene Inspection Close-up

6.  Genome Visualisation — 3D Depth and Close-up
The chromosome strip from v1 is a flat 2D bar chart. In v2 it is a 3D object: each gene region is a raised column above the strip surface, with height proportional to the gene value. The columns are rendered with a physically-correct material and lit by the habitat lighting. The strip surface itself is rendered as a slightly reflective dark material — biological sequencing glass.

6.1  Strip Geometry and Material
Strip surface: a flat horizontal rectangle, 40% of panel width × 8% of panel height. Material: dark glossy glass (roughness 0.15, metalness 0.0) with a faint internal grid from a procedural texture. Slight cube-map reflections from the habitat environment above it.
Gene columns: 3D rectangular columns rising from the strip surface. Height: gene value normalised to 0.0–1.0 mapped to 0–20px column height. Material: element-matched translucent glass with the gene's characteristic colour — the column is backlit from below by the strip's internal light.
Column illumination: the strip has an internal emissive surface at the base. Each column is lit from below — the column's underside glows, the top face reflects the habitat light. This creates a lamp-table effect where the genetic data appears self-illuminated.
Hover interaction: hovering a column causes it to rise an additional 8px (animated, 150ms). The column brightens. The surrounding columns dim slightly. The creature in the viewport highlights the corresponding body feature with a soft accent glow.

6.2  Gene Close-up — Camera Push to Strip
Double-tapping a gene column pushes the camera from the standard panel position to a close-up of that gene column. At this zoom level, the column surface shows micro-text annotation — the specific parameter values are rendered as visible surface printing on the column face:

  Gene close-up mode — depth of field at strip level
  CAMERA TRANSITION (220ms):
    Camera pushes toward the strip, focusing on the selected column.
    DOF shifts: strip surface is now focal plane.
    Other columns: 8px blur (slightly behind focal plane).
    Habitat background: 45px blur — fully soft.
    Creature model: 18px blur — present but not competing.

  CLOSE-UP VIEW:
    The selected column fills approximately 20% of the panel width.
    Column face shows printed annotation:
      'WING MORPHOLOGY'
      'aspect_ratio: 10.2'
      'membrane_area: 28.4 m²'
      'fold_complexity: 2'
    The text is the same size as normal panel text — at this zoom level,
    the column is large enough to hold readable text.

  BIOLOGICAL EFFECT (only in ultra mode):
    The column's internal backlight pulses once at 0.8 Hz during close-up.
    This communicates that the gene is 'active' — being read.
    Pulse colour shifts toward the gene's phenotypic expression colour.



  🔄  FSM Graph — Floating Node Planes with Virtual Depth

7.  FSM Graph — Floating Node Planes
The FSM state graph from v1 is a 2D directed graph in the panel. In v2 it is rendered as a series of floating planes at different virtual depths, matching the hierarchy of the FSM. Resting states float closest to the viewer (shallowest depth). Hunting and combat states float furthest away (deepest depth). The depth gradient visually communicates the escalation from calm to active behaviour.

7.1  Node Depth Assignment and Material
Resting / idle states (GROUNDED_REST, LISTEN): virtual depth 0.2. Sharp. Node material: dark glass with element accent colour fill, emissive at 0.04 — just barely glowing.
Alert / patrol states (ALERT_GROUNDED, ORIENT, SHADOW, HUNTING_PATROL): virtual depth 0.8. 4–6px blur. Node material: slightly brighter element colour, emissive at 0.08. Edges from these nodes animate in a slow dash pattern — the creature is actively considering transitions.
Active / committed states (STOOP, COMBAT_AIR, STOOP_SETUP, OBSERVE): virtual depth 1.8. 10–14px blur. These exist in the background of the graph — aggressive and distant. Node material: warm element colour, higher emissive, edges from these nodes animate faster.
Active state (the creature's current state): always pulled forward to virtual depth 0.0 regardless of its position in the hierarchy. It is the sharpest object in the FSM panel. A pulsing glow extends 10px beyond the node boundary.
Edges between nodes: rendered as slightly glowing line objects, not flat lines. They have a small amount of emissive bloom — visible as a soft halo in ultra mode. Edge width scales with transition probability.

7.2  Memory Entry Spatial Placement
Memory entry annotation chips (from v1) are given physical position relative to the FSM nodes they annotate. In ultra mode they float at a depth slightly in front of their associated node — closer to the viewer, sharper, more immediately readable:

Memory chips are rendered as frosted glass capsules — the same material as the UI panels — with white text at native resolution and a faint element-tinted backdrop blur. They are at virtual depth 0.05: the sharpest elements in the FSM panel after the active state.
Connection lines between a memory chip and its associated FSM node: thin emissive lines with a slow pulse. The pulse travels from the node to the chip at 0.5Hz — communicating that the memory is actively associated with that state.
In Live Feed mode, new memory entry chips appear with a brief resolve animation — they arrive blurred and sharpen over 200ms, as though the memory has just been formed and is still crystallising.


  🔬  Body Region Close-up — Extreme DOF, Pose-Specific Detail

8.  Body Region Inspection — Extreme DOF and Micro-Detail
The creature viewport supports close-up inspection of any body region through double-tap on the rotating model. In close-up mode the camera pushes to the surface of the selected region, DOF narrows to approximately f/1.4, and tessellation increases to the maximum subdivision level. The selected region fills the viewport; the rest of the creature is soft background.

8.1  Close-up Camera Behaviour
  Close-up mode — full sequence
  DOUBLE-TAP a body region on the rotating creature model.

  TRANSITION (280ms, smoothstep):
    Camera: orbital position → surface of selected region, 0.3 units away.
    DOF: current aperture → f/1.4 equivalent.
    Tessellation: current level → maximum (4 subdivisions).
    The rest of the creature: defocuses smoothly over 280ms.
    Habitat background: blurs to 50px (maximum).

  CLOSE-UP VIEW:
    Selected region fills approximately 80% of viewport.
    Micro-detail normal map: visible at full resolution.
    POM depth: scale relief is deep and readable.
    SSAO: inter-scale cavity shadowing is prominent.
    Material wear: individual scratches and edge dulling visible.

  ORBIT IN CLOSE-UP:
    Drag rotates the camera around the close-up point.
    Orbit speed: 1/3 of standard orbit (slow, precise).
    The selected region stays centred; orbit reveals different angles.

  EXIT:
    Single tap anywhere outside the highlighted region.
    Camera returns over 220ms (faster than entry — snap back feel).
    DOF returns to standard aperture. Tessellation reduces.

8.2  Micro-Detail per Body Region
Body region
Micro-detail at close-up
Ultra-exclusive effects
Dorsal scales
Scale geometric relief via POM. Inter-scale cavity shadows. Scale tip specular.
Micro-crystalline deposits visible between scales (for Earth/Brine element). Individual scale edge wear from combat history.
Wing membrane
Blood vessel network. Translucency with backlight. Bone silhouette.
Stress colour gradient from load-bearing bones outward. Spore emission if mycelium-touched.
Eye
Cube-map corneal reflection. Pupil geometry. Limbal ring.
Dynamic pupil dilation (responds to L value of habitat light). Iridescent iris micro-structure at extreme close-up.
Jaw / brow plates
Enlarged individual plate geometry. Raised edges.
Battle scarring procedural map from play history. Element discharge marks at jaw tip.
Foot / claws
Claw surface texture. Keratin layering.
Substrate material trace: soil residue for Earth dragon, salt residue for Brine, soot for Fire.
Belly
Smooth scale surface. Near-mirror specular.
Thin-film iridescence (rainbow shift at glancing angles). More pronounced SSS — belly skin is thinnest.


  ⚙  Performance Scaling

9.  Performance Scaling
Feature
Ultra
High / Standard
Scale POM depth
8-step steep parallax
High: 4-step. Standard: normal map only, no parallax.
Wing membrane
Dual-sided translucency, transmission, vessel map
High: single-sided, opacity mask for thin areas. Standard: flat membrane texture with fake translucency baked.
Wing stress VFX
Full colour gradient, bone glow
High: amber edge tint only. Standard: not shown.
Bioluminescence
32 volumetric bio-light sources, pulse, falloff
High: 8 sources, simplified pulse. Standard: emissive texture only, no dynamic light cast.
Genome strip
3D columns, backlit, reflective strip surface
High: 3D columns, flat strip surface. Standard: 2D bar chart.
FSM floating planes
Full 3D, emissive edges, depth-sorted bokeh
High: 3D nodes, flat edges. Standard: 2D graph.
God-ray background
80-sample radial blur per ray
High: 40-sample. Standard: pre-rendered god-ray texture.
SSS on creature
3-pass screen-space, per-element profile
High: 2-pass. Standard: pre-baked albedo approximation.
Tessellation
Adaptive 4px edge, 4 levels
High: 2 levels. Standard: base mesh only.



End of Mob & Dragon Viewer UI v2 — Ultra Rendering Specification.
Companion to: Mob & Dragon Viewer UI v1 (creature, genome, FSM, physics, voice, memory, live feed). All structural decisions from v1 are retained. v2 specifies the rendering layer. Shares DOF system, frosted glass panels, LUT grading, and element void shaders with Bending Character Menu UI v2.

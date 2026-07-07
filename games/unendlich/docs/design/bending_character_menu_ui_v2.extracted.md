
Bending Character Menu
UI Design v2 — Ultra Rendering
Depth of Field · Minimalist Frosted Panels · Close-up Tessellation · PBR Material Shaders · Per-element Colour Grading
v2 adds the rendering specification on top of v1's structural specification. Every layout and interaction decision from v1 is retained. What changes is how everything is rendered: the character is a cinema subject, the background dissolves into soft light, and the panels are glass rather than cards.

1.  Rendering Philosophy — The Menu as Cinematic Frame
A character menu is a portrait session. The subject — the player's character — deserves the same depth of field, the same quality of light, the same attention to surface detail that a portrait photographer would give. The background is present but intentionally soft. The subject is sharp, three-dimensional, and lit for maximum material legibility. The UI chrome is thin enough to be transparent — literally transparent, in the case of the frosted glass panels.

Three rendering principles govern everything in v2:

The character or creature is always the sharpest element in the frame. Every other element — background void, skill tree nodes, panel chrome — exists at a greater virtual depth and receives proportionally more blur.
Panels are not surfaces. They are atmospheres. A panel is a region where text and icons exist against a blurred, tinted version of the element behind them. No heavy backgrounds. No high-contrast borders. The panel is legible because the text is sharp, not because the background is opaque.
Transitions are optical, not digital. When navigating between panels, elements don't slide or fade — they defocus into blur, then the new content resolves from blur into sharpness. This matches how attention actually moves — focus shifts before content fully changes.



  📷  Depth of Field — Dynamic Focus with Bokeh

2.  Depth of Field System
The depth of field system treats the menu as a camera with a physical aperture. The character's face is the default focus plane. All scene depth is calculated relative to this plane, and the circle of confusion (CoC) radius at each pixel drives the bokeh blur applied to that pixel. The blur kernel is a hexagonal bokeh matching a real lens aperture, not a simple Gaussian — the hexagonal shape is immediately readable as a camera effect rather than a post-process artifact.

2.1  Focus Depth Table — What Is Sharp at Each UI State
UI state
Focus point
Background blur radius (pixels at 1080p)
Default — character in viewport
Character face
Background void: 28–32px bokeh. Panel chrome: 8px. Distant particles: 40px.
Hovering a skill tree node
Hovered node
Character: 4px. Other nodes (same depth): 2px. Nodes further in tree: 10–18px. Background: 35px.
Equipment inspection — body slot hover
Hovered equipment slot on character
Adjacent equipment: 6px. Character face: 8px. Background: 40px. Panel: 4px.
Equipment close-up mode (double-tap slot)
Equipment piece surface
Everything else: 22–40px. Ultra-shallow depth — the piece is the only sharp element.
Discipline tile hover
Hovered discipline tile
Other tiles: 8px. Character: 6px. Background: 20px (partially sharpened by tile proximity).
Voice panel active
Harmonic spectrum bars
Character: 8px. Background: 36px. Other panels: 6px.

2.2  Circle of Confusion Calculation
  GLSL — hexagonal bokeh depth of field post-process
// ── Depth of Field — Circle of Confusion and Bokeh Blur ──────────────────
// Runs as a post-process pass after the full scene is composited.

uniform sampler2D uColorBuffer;   // full scene render
uniform sampler2D uDepthBuffer;   // linear scene depth 0..1
uniform float     uFocalDepth;    // depth of focus plane (world units)
uniform float     uFocalLength;   // mm equivalent, default 85.0
uniform float     uFStop;         // aperture: 1.8 = cinematic, 5.6 = deep
uniform float     uSensorWidth;   // 35mm equiv: 36.0

// Convert depth to blur radius in pixels
float cocRadius(float depthWorld) {
    float f  = uFocalLength;
    float Fs = uFStop;
    float s  = uFocalDepth;
    // Thin lens CoC formula
    float coc = abs((f / Fs) * (s - depthWorld) / (s * (depthWorld - f / 1000.0)));
    // Convert mm to pixels at current resolution
    return coc * (uResolution.x / uSensorWidth) * 0.5;
}

// Hexagonal bokeh kernel — 7 samples per ring, 3 rings
const int   RINGS       = 3;
const float RING_SCALE  = 1.0;
const vec2  HEX_OFFSETS[6] = vec2[](
    vec2(1.0, 0.0), vec2(0.5, 0.866), vec2(-0.5, 0.866),
    vec2(-1.0, 0.0), vec2(-0.5, -0.866), vec2(0.5, -0.866)
);

vec4 bokehBlur(sampler2D tex, vec2 uv, float radius) {
    if (radius < 0.5) return texture(tex, uv);  // sharp — no blur needed
    vec4 sum = texture(tex, uv); float weight = 1.0;
    for (int ring = 1; ring <= RINGS; ring++) {
        float r = radius * float(ring) / float(RINGS);
        for (int h = 0; h < 6; h++) {
            vec2 offset = HEX_OFFSETS[h] * r / uResolution;
            sum += texture(tex, uv + offset);
            weight += 1.0;
        }
    }
    return sum / weight;
}

void main() {
    float depth  = texture(uDepthBuffer, vUV).r;
    float radius = cocRadius(depth * uFarPlane);  // world-space depth
    // Focus depth lerp: smooth transition when focus plane changes
    radius = mix(uPrevRadius, radius, uTransitionAlpha);
    gl_FragColor = bokehBlur(uColorBuffer, vUV, clamp(radius, 0.0, 40.0));
}

2.3  Dynamic Focus Transition
When the focus point changes — hovering a new node, opening a new panel — the focal depth does not jump. It lerps over 180ms using a smoothstep curve. This prevents the jarring pop of an instantaneous focus shift and reads as the camera physically refocusing. A fast hover (< 120ms on any element) does not trigger a focus shift — the transition only commits when the hover is sustained.

Focus shift speed: 180ms smoothstep for standard navigation. 280ms for entering close-up mode (camera also pushes in). 80ms for returning from close-up (camera snaps back slightly faster than it pushed).
During a focus shift, both the outgoing and incoming depths are rendered and weighted by the smoothstep value — the world doesn't stop while the camera refocuses.
The character's face is always the fallback focus point. If the player moves away from all interactive elements, the focus returns to face at 400ms.


  💎  Character Ultra-Rendering — Tessellation, SSS, Cloth, Eyes

3.  Character Ultra-Rendering
The character model in the menu is rendered at a quality tier unavailable during active play — full tessellation subdivision, screen-space subsurface scattering for skin and membrane materials, real-time cloth simulation for robes and capes, and high-resolution PBR material maps. This is the player's best view of their character. It should be the standard they imagine their character meeting during play.

3.1  Tessellation and Micro-Detail
The base character mesh is a mid-poly model (12,000–18,000 triangles). In the menu, GPU tessellation subdivides this mesh based on camera distance and screen-space edge length, targeting a maximum screen-space triangle edge of 4 pixels. At default view distance this produces approximately 3–4 subdivision levels. At close-up inspection zoom (camera pushed to a body region), the tessellation produces fine geometric detail that the micro-detail normal map then drives.

Surface region
Micro-detail at close-up zoom
Bending cloth / robe
Individual weave pattern visible as actual geometric variation. Warp and weft directions produce anisotropic specular reflection — cloth catches light differently along and across the weave. Thread-level detail at extreme zoom.
Skin / exposed body
Pore-scale surface detail from micro-detail normal map. Subsurface scatter produces the characteristic red-shift at ear edges, finger webbing, and nose — places where skin is thin enough for light to transmit.
Element-touched skin
Bending-active body regions show element influence: Earth benders have stone-dust impregnation in skin creases; Water benders have a slight surface tension sheen; Storm benders have micro-hair charged and standing slightly raised; Information benders show faint prismatic under-skin capillary shimmer.
Equipment metal / crystal
Scratches from actual use: procedural wear map applied at close zoom shows impact marks and edge dulling consistent with the character's play history. Element-appropriate oxidation: copper equipment greens near Water discipline; iron equipment is orange-traced near Fire.
Dragon-tamer bonding marks
If the player has a tamed dragon with mature owner voice mesh, faint harmonic resonance markings appear at contact points (wrists, chest) — these are the physical signature of the voice coupling. Visible only at close-up zoom.

3.2  Subsurface Scattering — Skin and Membrane Materials
Subsurface scattering (SSS) approximated using a screen-space separated Gaussian diffusion profile. The profile has three passes at three different radii (1px, 3px, 8px) with element-specific colour tinting for the deep scatter:

  GLSL — screen-space SSS with per-element scatter profile
// ── Screen-space SSS approximation — three-pass Gaussian blur per channel ──
// Applied after the albedo pass, before specular compositing.
// Profile: Jensen dipole approximation with per-element tinting.

struct SSSProfile {
    vec3 scatter_r;  // scatter weight at radius: (weight_1px, weight_3px, weight_8px)
    vec3 scatter_g;  // per channel — R scatters deeper than G, G deeper than B
    vec3 scatter_b;
    vec3 tint;       // element-specific deep scatter colour shift
};

// Earth bender: warm ochre deep scatter (mineral deposit in skin)
const SSSProfile EARTH_PROFILE = SSSProfile(
    vec3(0.22, 0.40, 0.38), vec3(0.18, 0.44, 0.38), vec3(0.12, 0.28, 0.60),
    vec3(1.05, 0.95, 0.80)  // warm ochre tint on deep scatter
);

// Water bender: cool blue-green deep scatter (surface tension sheen)
const SSSProfile WATER_PROFILE = SSSProfile(
    vec3(0.20, 0.38, 0.42), vec3(0.16, 0.42, 0.42), vec3(0.10, 0.26, 0.64),
    vec3(0.92, 1.02, 1.10)  // cool tint
);

// Storm bender: slight blue-white scatter (ionised surface layer)
const SSSProfile STORM_PROFILE = SSSProfile(
    vec3(0.18, 0.38, 0.44), vec3(0.14, 0.40, 0.46), vec3(0.08, 0.22, 0.70),
    vec3(0.95, 0.98, 1.08)
);

vec3 applySSS(sampler2D irradiance, vec2 uv, SSSProfile profile) {
    vec3 s1 = gaussianBlur(irradiance, uv, 1.0);  // thin scatter
    vec3 s3 = gaussianBlur(irradiance, uv, 3.0);  // mid scatter
    vec3 s8 = gaussianBlur(irradiance, uv, 8.0);  // deep scatter × tint
    return vec3(
        dot(vec3(s1.r,s3.r,s8.r*profile.tint.r), profile.scatter_r),
        dot(vec3(s1.g,s3.g,s8.g*profile.tint.g), profile.scatter_g),
        dot(vec3(s1.b,s3.b,s8.b*profile.tint.b), profile.scatter_b)
    );
}

3.3  Cloth Simulation — Robes and Bending Gear
Bending robes and capes are simulated with a position-based dynamics (PBD) cloth solver. In the menu — unlike active play where a simplified proxy is used — the full cloth mesh is simulated at 60hz with gravity, self-collision, and element-appropriate response:

Earth bending cloth: dense, heavy fabric. Settles quickly, holds shape, minimal flutter. Drape is deep and the folds are large.
Air bending cloth: lightweight, high-float silk equivalent. Perpetual gentle flutter even with zero input — the character's own elemental W field is a continuous force input to the cloth simulation.
Water bending cloth: flowing, mid-weight. Settles with visible wave motion, takes a long time to fully damp. The bottom hem continues oscillating after the character settles.
Storm bending cloth: medium weight with a conductive material layer that makes it stand slightly outward from the body — electrostatic repulsion from the character's own charge. Occasional sharp 'snap' as the cloth discharges.
The pose strip in the viewport includes cloth state — when the player selects a stance demonstration, the cloth begins simulating from its current drape state and responds to the character's movement. Cloth does not teleport to a preset drape.

3.4  Eye Detail — Pupil, Reflection, and Element Response
Eyes are the single highest-return detail investment. At close zoom, eyes immediately communicate whether the model is alive or a mannequin. The character's eyes in the menu receive:

Cube-map reflections: the elemental void's colour and particle field reflect in the corneal surface. An Earth bender's eye reflects warm amber fault lines. A Storm bender's eye reflects the arc discharge. The reflection uses a real-time cube capture updated once per second (not per frame — the reflection lags slightly, which is correct for eyes).
Dynamic pupil dilation: the pupil size responds to the current environment's L value. High-L environments (Air void — bright, near-white) produce contracted pupils. Low-L environments (Storm void — dark, deep blue-purple) produce dilated pupils. The dilation lerps over 0.8 seconds — the actual physiological response speed.
Limbal ring: a distinct dark ring at the iris-sclera boundary. Sharpens at close zoom using a separate high-resolution texture. The ring colour shifts slightly toward the element's accent colour.
Specular highlight: two specular highlights per eye — a large soft one from the dominant light source and a small sharp one from a secondary fill. The highlights are not baked; they come from the actual scene lights.



  🪟  Minimalist Panels — Frosted Glass, Backdrop Blur, Zero Chrome

4.  Minimalist Frosted Panel Design
Panels are not cards, not boxes, not containers. They are regions of atmospheric transparency where text and icons are legible because they are sharp against a soft, tinted version of the world behind them. The background-blur is a Gaussian applied to the composited scene before the panel's text is drawn on top. The panel has no background fill, no border, no shadow — only the backdrop blur creates the sense of a surface.

4.1  Frosted Glass Implementation
  GLSL — frosted glass panel backdrop: blur, desaturate, element tint
// ── Frosted glass panel backdrop — applied per panel region ───────────────
// Renders a blurred, tinted version of the scene behind the panel.
// The blur radius is proportional to the panel's virtual depth.

uniform sampler2D uSceneColor;   // full composited scene before UI
uniform float     uBlurRadius;   // pixels — set per panel based on depth
uniform vec3      uTint;         // element-matched tint (very low alpha)
uniform float     uTintAlpha;    // 0.08–0.18 — barely there
uniform float     uSaturation;   // 0.85 — slightly desaturate behind panel

// 13-tap Gaussian kernel (fixed for performance; weights pre-computed)
const float GAUSS_WEIGHTS[7] = float[](
    0.2270, 0.1945, 0.1216, 0.0540, 0.0162, 0.0044, 0.0007
);

vec4 gaussianBlurH(sampler2D tex, vec2 uv, float radius) {
    vec2 texel = 1.0 / uResolution;
    vec4 sum   = texture(tex, uv) * GAUSS_WEIGHTS[0];
    for (int i = 1; i < 7; i++) {
        vec2 off = vec2(radius * float(i) * texel.x, 0.0);
        sum += (texture(tex, uv + off) + texture(tex, uv - off)) * GAUSS_WEIGHTS[i];
    }
    return sum;
}

void panel_backdrop_main() {
    // Two-pass Gaussian (H then V applied separately for performance)
    vec4 blurred = gaussianBlurH(uSceneColor, vUV, uBlurRadius);
    // blurred is the scene behind this panel region, softened

    // Slight desaturation
    float lum = dot(blurred.rgb, vec3(0.299, 0.587, 0.114));
    blurred.rgb = mix(vec3(lum), blurred.rgb, uSaturation);

    // Element tint at very low alpha — barely perceptible but present
    blurred.rgb = mix(blurred.rgb, uTint, uTintAlpha);

    gl_FragColor = blurred;
    // Text and icons are drawn on top of this output as a separate pass.
}

4.2  Panel Blur Depth Table
Blur radius scales with the panel's virtual depth — how far 'behind' the character it appears to sit. Deeper panels are more aggressively blurred, maintaining the illusion that they exist further from the focal plane:

Panel
Blur radius (1080p px)
Visual effect and rationale
Void background (deepest)
40–48px
Fully soft — detailed elemental motion is preserved (you can see the arc tracers / caustics), but no sharp detail. This is the cinematic background bokeh.
Secondary panel (far)
20–28px
Readable but soft. This panel is not in focus — it is the other panel when the primary is selected.
Primary panel (near)
10–14px
Slightly soft. The panel exists at virtual depth behind the character, but is close enough for comfortable reading.
Active element (hovered)
6–8px
Barely blurred — the hovered element is nearly in focus alongside the character. This communicates interactability.
Character model (foreground)
0–2px
Sharp. The character is on the focal plane. Any blur is only from temporal AA.
Tab bar (closest to camera)
6–8px
Slightly soft backdrop but tab labels and icons are always sharp — they are overlaid at native resolution.

4.3  Typography and Icon Rendering on Glass
Text and icons sit on top of the blurred backdrop and must remain perfectly sharp regardless of backdrop blur amount. They are rendered in a separate pass at native resolution with no post-processing applied. The text rendering layer is never blurred.

Font: a geometric sans-serif in the 16–22pt range for body text, 28–36pt for headers. Slightly wider letter-spacing than standard (tracking +20) to maintain legibility against a variable background.
Text colour: white at 92% opacity for primary text, white at 62% opacity for secondary text (stat labels, descriptions). The slight opacity reduction prevents halos against bright background patches.
Icons: vector-rendered at native resolution. Element icons use a thin-line style (1.5–2px stroke) — enough presence to read against any background without filling a pixel budget that would make them feel heavy against glass.
Glow on active elements: selected nodes, active tabs, and hovered items emit a 6px radial glow in the element's accent colour at 60% opacity. This glow is applied to the glass backdrop layer, not to the text — the text itself stays clean white.


  🌌  Element Void — Ultra-Quality Background Shaders

5.  Element Void Ultra-Shaders
The void background shaders from v1 are functional and correct. In v2 they receive three upgrades: volumetric particle rendering (instead of billboard sprites), temporal accumulation for noise (stable animation instead of per-frame flicker), and per-element colour grading LUTs applied to the full composite. At ultra quality, they are rendered at 150% resolution and downsampled with a Lanczos filter — this sharpens edges in the caustic and arc patterns before the DOF blur is applied, producing a more distinct background that still reads as depth-of-field soft.

5.1  Volumetric Particles — Replacing Billboards
Billboard sprites for ambient particles (mineral dust, bioluminescent cells, ember motes) are replaced with volumetric rendering. Each particle is a small participating medium volume that scatters light from the scene light source. The particle catches and diffuses the element's directional light rather than merely displaying a pre-authored sprite:

Earth mineral dust: each dust mote is a 0.02-unit sphere with a high-albedo scattering coefficient. The directional light from below produces shafts through accumulated dust density — visible as light god-rays when density is high near the character.
Water bioluminescent drift: each 'cell' is a volume that emits teal-green light. The emission radius is slightly larger than the mesh volume — the light bleeds into surrounding atmosphere, producing soft halos in the dense-fog path.
Storm arc tracers: the arc itself is a volumetric tube with scattering coefficient high enough to glow internally — the core of the arc is bright, the edges fall off in a physically correct decay curve (not a hard edge).
Air lens currents: the anisotropic FBM texture is interpreted as a density field. The low-density regions between currents are slightly more transparent to background light, creating subtle brightness variation in the void that moves as the current animates.

5.2  Per-Element Colour Grading LUTs
A 3D LUT (32×32×32) is applied as the final step before DOF, after all compositing, to give each element a specific photographic feel:

Element
LUT character
Technical adjustments
Earth
Warm, deep. Canyon-at-dusk.
Red channel +4, Blue channel −6. Shadow lift: warm grey (not neutral). Highlight: amber roll-off. Crush blacks slightly.
Water
Cool, deep. Abyssal pressure.
Blue channel +8, Red −4. Strong desaturation below 20% value. Highlights shift teal. High contrast in midtones.
Fire
Warm, high contrast. Volcanic.
Red +10, Blue −8. High highlight saturation (embers are vivid). Shadow: near-black neutral. Aggressive vignette.
Air
High-key, airy. Alpine light.
All channels lifted +8 (bright overall). Slight blue cast in shadows. Low contrast — compressed tonal range. Near-white highlights stay clean.
Storm
Cinematic cool. Overcast.
Blue +6, heavy shadow crush. Purple tint in midtones. Desaturate all except blue and purple channels. High contrast.
Information
Prismatic, clinical. Observatory.
Neutral but with cyan-green push in midtones. Highlights split: warm on one axis, cool on the other. Medium contrast.



  🔍  Equipment Close-Up — Extreme DOF, Wear Maps, Material Detail

6.  Equipment Close-Up Mode
Double-tapping an equipment slot pushes the camera from its default orbital position to a close-up position focused on that body region. The transition is 280ms with easing — the camera pushes in, the DOF narrows dramatically, and the rest of the character dissolves into soft bokeh. Only the selected piece remains sharp. The close-up is the highest-resolution view of any game asset the player will see.

6.1  Material Detail at Close Range
  Equipment piece rendering at close-up zoom
  SURFACE LAYER 1 — Base material (PBR albedo/roughness/metalness):
    Full resolution maps (4096×4096 for equipment, 2048 for world items).
    At close-up, the full texel density is visible. No visible compression artefact.
    Roughness map drives specular at pixel level — individual matte vs glossy regions.

  SURFACE LAYER 2 — Micro-detail (parallax occlusion mapped):
    Depth map drives apparent depth in the surface. Scale geometry on metal looks recessed.
    Crystal facets appear to have actual depth — not just normal-mapped flat.
    Stone equipment shows genuine surface roughness in the POM depth.

  SURFACE LAYER 3 — Wear and history:
    Procedural wear mask generated from play data:
      contact_wear   = accumulated collision and impact events
      edge_wear      = dulling and nicks on striking edges
      use_polish     = high-friction areas smoothed by repeated grip
    The wear map modulates roughness (worn edges are rougher; worn grip is smoother)
    and slightly dims albedo where the finish has abraded.

  SURFACE LAYER 4 — Element oxidation/patina:
    Earth: mineral deposit in crevices — yellow-grey chalky accumulation.
    Water: copper greening on copper alloy; wet-look surface sheen on all materials.
    Fire: heat blueing on steel; carbon black at heat exposure points.
    Storm: galvanic pitting on dissimilar metals; arc burn at discharge points.
    Information: prismatic interference film where light has been channelled.

6.2  Focus Ring Animation
On entering close-up mode, a subtle focus ring appears around the item — a thin circle (1px) at the bokeh boundary where the sharpness transition is most apparent. This is an optical artefact matching real camera bokeh at the near-field edge. It is not drawn explicitly — it emerges from the CoC gradient between the piece (sharp) and the surrounding body (soft). The ring is in the element's accent colour, which makes it feel intentional rather than accidental.

Design note:  The focus ring around the equipment piece is the same visual language as the selection glow in v1 — but rendered optically rather than drawn on top. The player learns to associate that ring with 'this is the thing in focus.' It becomes a legibility cue that requires no UI chrome.


  ✨  Skill Tree Depth — Nodes at Virtual Focal Distances

7.  Skill Tree — Rendered with Virtual Depth
The skill tree exists in a 3D space even though it is presented as a 2D layout in the panel. Each node has a virtual depth value that drives its DOF blur — nearby (prerequisite) nodes are sharp; distant (deeper in the tree) nodes are progressively softer. This creates a sense of the tree extending away from the player, which reinforces the 'depth' of ability specialisation.

7.1  Node Depth Assignment
Root nodes (no prerequisites — directly accessible): virtual depth 0. Same focal plane as the character face. Sharp.
Tier 1 nodes (one prerequisite): virtual depth 0.5. Slightly behind the root nodes. 2–4px CoC blur.
Tier 2 nodes (two prerequisite steps): virtual depth 1.2. 6–8px blur. Still clearly readable.
Tier 3+ nodes (three or more steps): virtual depth 2.0+. 12–18px blur. The deep nodes are intentionally soft — they represent potential not yet achieved. They resolve to sharp when the player hovers them (focus shift to that depth).
Locked nodes behind locked prerequisites: depth + 1.0 additional. Even softer. More distant. More aspirational.

7.2  Node Material — Emissive Glass Sphere
Each skill tree node is rendered as a small volumetric sphere rather than a flat icon:

Unlocked nodes: filled with a translucent element-coloured material. The element's characteristic particle (dust for Earth, water droplet for Water, etc.) drifts slowly inside the sphere volume. The sphere has a thin glass shell that produces a specular highlight from the scene light.
Locked-but-accessible nodes: hollow glass sphere with a faint element glow visible through the shell. The interior is dark — the content will resolve when unlocked.
Locked-behind-prerequisite nodes: opaque dark sphere at elevated virtual depth. Shape is defined but material is neutral until the prerequisite path is clear.
Active/hovered node: a 6px radial glow in accent colour at 70% opacity. The node brightens by 30%. At hover, the sphere volume becomes more active — particles drift faster, the glow pulses once.


  🌊  Transition Language — Defocus, Not Dissolve

8.  Transition Visual Language
Every navigation transition in v2 uses defocus rather than opacity as the primary change signal. Elements don't fade out — they blur out. New elements don't fade in — they resolve from blur to sharp. This matches the language established by the DOF system and makes every transition feel like a natural attention shift rather than a digital scene change.

8.1  Panel Navigation — Defocus Slide
  Panel navigation transition sequence
  STEP 1 — Defocus outgoing panel (80ms):
    The current panel's backdrop blur increases from its nominal value
    to 40px (maximum). Text and icons reduce opacity from 100% to 0%.
    This happens simultaneously — the panel becomes unreadable glass,
    then the glass fades to nothing.

  STEP 2 — Slide (120ms, overlapping step 1 by 60ms):
    The new panel slides in from the right, but starts at maximum blur (40px).
    It arrives at its nominal position still blurred.

  STEP 3 — Resolve incoming panel (100ms):
    The backdrop blur of the new panel decreases from 40px to its nominal value.
    Text and icons increase opacity from 0% to 100%.
    The panel resolves into readability as the focus settles.

  Total transition duration: 240ms
  Peak motion blur: step 2 onset
  Most visually apparent moment: step 3 resolution — the content 'arrives'.

8.2  Discipline Selection — Element Dissolution
Selecting a new discipline is the menu's most dramatic transition. Unlike panel navigation, this transition changes the entire environment:

Phase 1 (0–300ms): The current element's void shader reduces in intensity. Particles slow and drift outward. The character's element-matched ambient particles cease. The directional light dims toward a neutral white.
Phase 2 (200–500ms): The new element's void shader fades in from the edges of the frame inward. The new element's particles begin at the frame edges and accumulate toward the centre. The directional light shifts toward the new element's colour temperature.
Phase 3 (400–700ms): The new element is dominant. The character's SSS profile transitions from the old element to the new (skin tint shifts over 300ms). The colour grading LUT cross-fades from old to new.
Phase 4 (600–800ms): The character briefly demonstrates the new element's idle stance — a pose shift, a breath, a subtle elemental exhale. This moment communicates that the element change has been accepted.


  ⚙  Performance Scaling — Quality Tiers

9.  Performance Scaling
Ultra rendering mode requires capable hardware. Three quality tiers allow the visual specification to scale gracefully:

Feature
Ultra
High / Standard fallback
DOF
Hexagonal bokeh, 19 samples/ring × 3 rings
High: circular Gaussian 13 tap. Standard: CoC mask only, no kernel — sharp/soft binary with soft edge.
Tessellation
Adaptive, 4px target edge, 4 subdivision levels
High: 2 levels. Standard: no tessellation, micro-detail from normal map only.
SSS
3-pass screen-space, per-element profile
High: 2-pass simplified. Standard: pre-baked albedo texture simulating SSS look.
Cloth simulation
Full PBD, 60hz, 800-triangle cloth mesh
High: simplified 200-triangle, 30hz. Standard: blend shape animation, no simulation.
Volumetric particles
Full volumetric, participating medium
High: billboard with soft-particle depth test. Standard: billboard, no depth test.
Panel backdrop blur
Full two-pass Gaussian per panel
High: single-pass 9-tap. Standard: pre-blurred scene capture updated 4× per second.
Colour grading LUT
32³ full-precision LUT with dithering
High: 16³ LUT. Standard: per-channel curve adjustment only.

Recommended hardware:  Ultra rendering at 4K 60fps requires approximately: GPU with 10+ TFLOPS compute, 12+ GB VRAM, hardware tessellation, and compute shader support for cloth simulation. At 1080p, Ultra is achievable on hardware with 6+ TFLOPS. The Standard tier runs on any hardware capable of running the game.



End of Bending Character Menu UI v2 — Ultra Rendering Specification.
Companion to: Bending Character Menu UI v1 (layout, interaction, content structure). All layout positions, panel organisation, tab structure, ability binding logic, stance renown display, and voice integration from v1 are unchanged. v2 specifies the rendering layer on top of v1's structural layer.

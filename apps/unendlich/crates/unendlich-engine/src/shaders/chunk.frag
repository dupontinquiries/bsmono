#version 450
#extension GL_GOOGLE_include_directive : require
// r68-04: shared analytic water-wave field (height/gradient/normal). Read-only
// here — the CPU buoyancy port stays in lockstep with this same file.
#include "water_waves.glsl"
//
// chunk.frag — atmosphere-aware shading for chunk meshes.
//
// v0.11.2 changes from v0.11.1:
//   - Calmer fog. v0.11.1's per-step turbulence (`sin(p.x)*sin(p.z)`)
//     combined with per-pixel jitter on `t` produced visible
//     salt-and-pepper noise on uniform mid-tone surfaces (gravel,
//     sand, stone). Fog density is now a smooth function of position;
//     adjacent jittered pixels sample nearly identical densities so
//     jitter only contributes anti-banding, not surface noise.
//   - Forward scatter moved out of the per-step loop. Applied once
//     to the final fog color; same look without per-step compounding.
//   - Underwater tint now requires the surface to ALSO be water, not
//     just to be above the camera. Without this guard, walking
//     downhill turned the whole frame blue when distant terrain
//     happened to be higher than the player.
//   - Removed the ±0.5/255 film-grain dither: with calmer fog there's
//     nothing left for it to hide, and on dark surfaces it just looks
//     like noise.
//   - Vignette softened (0.35 → 0.25).
//
// v0.11.1 changes (kept):
//   - viewport_rect field on UBO. Vignette correct at any resolution
//     and ready for multi-cam / split-screen.
//   - Logarithmic-depth fog stepping. Step boundaries fall outside
//     the visible band, removing horizon banding lines.
//   - Per-pixel hash jitter on the fog march start offset, which
//     breaks any remaining banding into per-pixel noise. Stable
//     per-pixel (no time mixing), so static shots stay clean.
//
// Design (unchanged from earlier)
// -------------------------------
// One fragment shader. No postprocess pass yet. Everything that costs
// only a few extra ALU ops per fragment is fair game; anything that
// requires screen-space input (real god rays, real refraction, real
// shadow mapping, real reflections) is DEFERRED.
//
// Cloud-as-fog NOTE: the blocks remain opaque (no alpha blending) so
// the chunk pipeline state is unchanged. Real transparent fog banks
// you can stand inside need a transparent draw pass — filed.

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec3 v_world_pos;

layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform CameraUbo {
    mat4 view_proj;
    vec4 camera_pos;       // xyz = eye position
    vec4 sun_dir;          // xyz = direction sunlight TRAVELS, w = directional intensity in [0,1]
    vec4 sun_color;        // rgb = sun radiance
    vec4 sky_color;        // rgb = sky / fog target
    vec4 ambient_color;    // rgb = ambient term (added to diffuse)
    vec4 atmos_params;     // x = fog_density, y = humidity, z = temperature, w = time_of_day
    vec4 wind_params;      // xy = wind vector at player, z = wind magnitude
    vec4 viewport_rect;    // xy = viewport size px, zw = viewport offset px
    mat4 light_view_proj;  // light-space VP for shadow-map sampling
    vec4 flow_coverage;    // r419: xy = flow-grid world origin, z = blocks/cell, w = enable
    vec4 flow_grid[128];   // 16x16 cells, 2 per vec4 (.xy even cell, .zw odd)
    vec4 fx_params;        // r427: x=submerged, y=time (underwater shimmer)
    vec4 water_depth_cov;  // r431: origin_x, origin_z, blocks_per_cell, enable
    vec4 water_depth_grid[64]; // r431: 16x16 depth cells (blocks), 4 per vec4
} ubo;

// Shadow map sampler. The Vulkan side uses a sampler with comparison
// enabled (LESS_OR_EQUAL) — so `texture(shadow_map, vec3(uv, ref_z))`
// returns a value in [0,1] representing "fraction of taps where the
// sampled depth is >= ref_z" (i.e., NOT in shadow). With LINEAR
// filtering the hardware does free 2x2 PCF.
//
// We add explicit 4-tap PCF (offset by ±0.5 texels) to soften the
// edge further — gives smooth penumbra-style transitions over a few
// pixels rather than a single hard texel boundary.
layout(set = 0, binding = 1) uniform sampler2DShadow shadow_map;

// 3D froxel scattering volume (v0.16 groundwork). Sampled in the
// fog block — but currently the volume is unfilled (sentinel value
// alpha=0 for all texels), so the fog code falls back to the
// per-fragment march. v0.17 will add the per-frame compute fill;
// at that point the same `texture(froxel_volume, uvw)` line below
// will start returning real scattering data and the march loop
// becomes the fallback for out-of-volume samples.
layout(set = 0, binding = 2) uniform sampler3D froxel_volume;

// ─── Material classification ────────────────────────────────────────────
//
// Material reconstructed by color match (no per-vertex material id yet).
// Filed: pass a material id varying so this becomes a switch statement.

bool is_water(vec3 c) {
    return distance(c, vec3(0.20, 0.40, 0.85)) < 0.08;
}

bool is_stone(vec3 c) {
    float maxc = max(max(c.r, c.g), c.b);
    float minc = min(min(c.r, c.g), c.b);
    return (maxc - minc) < 0.08
        && maxc > 0.30 && maxc < 0.80;
}

// r311: foliage / cutout vegetation (leaves, grass, ferns, vines, moss, cactus).
// Green-dominant and clearly greener than its own red/blue — separates vegetation
// from grey stone, brown dirt/logs, and the red flowers/mushrooms. Used to drop the
// hard per-block edge outline (bevel + grid) on these blocks, where a crisp cube
// border reads wrong. Color-match heuristic in the spirit of is_water/is_stone; the
// per-block tile id (pack_color_id's reserved alpha byte) makes this exact once the
// texture atlas lands.
bool is_foliage(vec3 c) {
    return c.g > c.r + 0.05 && c.g > c.b + 0.08 && c.g > 0.25;
}

// Cloud cubes are emitted with base color (0.95, 0.95, 1.00) modulated
// by face-shading; the test allows ±0.10 for the modulation range.
bool is_cloud(vec3 c) {
    return c.r > 0.80 && c.g > 0.80 && c.b > 0.85
        && (max(c.r, c.g) - min(c.r, c.g)) < 0.06
        && c.b >= max(c.r, c.g);
}

// ─── ACES filmic tonemap (Narkowicz 2015 fit) ──────────────────────────
//
// Cheap and good. Real ACES is a 3D LUT but this single-line fit looks
// 95% as good for a fraction of the cost. Compresses HDR rolloff so
// bright sky and specular highlights desaturate gracefully instead of
// clipping to flat white.
vec3 aces_film(vec3 x) {
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

// ─── Filmic contrast S-curve ───────────────────────────────────────────
//
// Slight midtone contrast bump. Pushes blacks slightly darker and
// whites slightly brighter without clipping. Default 0.10 makes the
// image read less "flat PowerPoint" and more "cinematic."
vec3 filmic_contrast(vec3 color, float strength) {
    return mix(color, smoothstep(0.0, 1.0, color), strength);
}

// ─── Hash-based screen-space dither ────────────────────────────────────
//
// IQ's classic 1D-from-2D hash. Cheap, no texture, deterministic per
// pixel. Used both for fog-march jitter (avoids horizon banding) and
// for film-grain in the final color (hides 8-bit color banding in dark
// regions).
//
// Made temporally stable by *not* mixing in time — that would create
// crawling noise in motion which looks like JPEG artifacts. Instead
// we accept that the same pixel always gets the same dither value;
// any banding shifts only when the camera moves enough to change pixel
// alignment, which is invisible to the eye.
float hash12(vec2 p) {
    vec3 p3 = fract(vec3(p.xyx) * 0.1031);
    p3 += dot(p3, p3.yzx + 33.33);
    return fract((p3.x + p3.y) * p3.z);
}

// ── UNENDLICH_TEXTURES step 1: procedural per-block surface ────────────────
// Keyed by the per-face tile id packed in the color alpha. id==255 (the default
// when textures are off) returns the albedo unchanged, so this is a no-op for
// the untextured build. Otherwise it adds tile "grout" lines + a per-id grain so
// each block type reads as a distinct textured surface — a stand-in until the
// real atlas sampler lands (step 2).
// ── UNENDLICH_TEXTURES: procedural per-block surface (r354) ────────────────
// Seeded off the block's own albedo (distinct per block type) — NOT the vertex
// alpha, which is always 0xFF now. Adds tile "grout" lines + a per-block grain.
// Gated at the call site by ubo.wind_params.w so the default look is unchanged.
// ── UNENDLICH_TEXTURES: per-block, per-face procedural surface (r357) ──────
// Seeds off the block's albedo (distinct per block type) and the geometric
// normal (distinct per face: top / bottom / side), with UV taken IN the face
// plane so side textures run correctly on walls instead of smearing. No vertex
// data — derives everything from `base`, `wpos`, and the cardinal `n`. Gated at
// the call site by ubo.camera_pos.w.
// r422: fetch a flow vector from the coarse 16x16 grid by cell index (clamped to edge).
// Two cells per vec4 (.xy even, .zw odd).
vec2 ww_flow_cell(int cx, int cz) {
    cx = clamp(cx, 0, 15);
    cz = clamp(cz, 0, 15);
    int cell = cx + cz * 16;
    vec4 p = ubo.flow_grid[cell >> 1];
    return ((cell & 1) == 0) ? p.xy : p.zw;
}

// r431: one water-depth cell (blocks). Grid is 16x16, 4 cells per vec4.
float ww_depth_cell(int cx, int cz) {
    cx = clamp(cx, 0, 15);
    cz = clamp(cz, 0, 15);
    int cell = cx + cz * 16;
    vec4 v = ubo.water_depth_grid[cell >> 2];
    int c = cell & 3;
    return c == 0 ? v.x : (c == 1 ? v.y : (c == 2 ? v.z : v.w));
}

// r431: bilinear water depth (blocks) at a world XZ position, or -1 if outside coverage / off.
float ww_water_depth(vec2 worldxz) {
    if (ubo.water_depth_cov.w < 0.5) return -1.0;
    vec2 rel = (worldxz - ubo.water_depth_cov.xy) / max(ubo.water_depth_cov.z, 0.001);
    if (rel.x < 0.0 || rel.y < 0.0 || rel.x >= 16.0 || rel.y >= 16.0) return -1.0;
    vec2 fc = rel - 0.5;
    vec2 fl = floor(fc);
    ivec2 c0 = ivec2(fl);
    vec2 fr = clamp(fc - fl, 0.0, 1.0);
    float d00 = ww_depth_cell(c0.x,     c0.y);
    float d10 = ww_depth_cell(c0.x + 1, c0.y);
    float d01 = ww_depth_cell(c0.x,     c0.y + 1);
    float d11 = ww_depth_cell(c0.x + 1, c0.y + 1);
    return mix(mix(d00, d10, fr.x), mix(d01, d11, fr.x), fr.y);
}

// r459: water surface tint from environmental fields. Mirrors sim::water_tint / sim::salinity.
// The render shader can't sample worldgen climate cheaply, so temperature + salinity use light
// position-based proxies (authoritative fields live CPU-side in sim::salinity for genome/spawn use);
// depth comes from the water-depth grid. Salty water → teal, cold → blue, warm → green, sediment →
// murky, deep → darker.
const float WT_SEA_LEVEL = 64.0;
float wt_temperature(vec3 wp) {
    float alt = clamp(0.62 - (wp.y - WT_SEA_LEVEL) * 0.004, 0.0, 1.0);
    float band = sin(wp.x * 0.0009) * 0.5 + sin(wp.z * 0.0011) * 0.5;
    return clamp(alt + band * 0.10, 0.0, 1.0);
}
float wt_salinity(float temp, vec3 wp) {
    float elev_rel = wp.y - WT_SEA_LEVEL;
    float sea = clamp(1.0 - max(elev_rel, 0.0) / 10.0, 0.0, 1.0);
    float evap = pow(temp, 1.5) * 0.5; // dry proxy (no humidity in frag)
    return clamp(max(sea, evap), 0.0, 1.0);
}
vec3 tint_water(vec3 base, float depth, float temp, float sal, float ero) {
    vec3 c = base;
    c.g += sal * 0.10; c.b += sal * 0.06;
    c.g += (temp - 0.5) * 0.08; c.b += (0.5 - temp) * 0.10;
    vec3 murk = vec3(0.34, 0.33, 0.22);
    c.r = mix(c.r, murk.r, ero * 0.5);
    c.g = mix(c.g, murk.g, ero * 0.4);
    c.b = mix(c.b, murk.b, ero * 0.5);
    float d = clamp(max(depth, 0.0) / 12.0, 0.0, 1.0);
    return clamp(c * (1.0 - d * 0.5), 0.0, 1.0);
}

vec3 apply_block_texture(vec3 base, vec3 wpos, vec3 n) {
    vec3 qc = floor(base * 6.0 + 0.5);
    float seed = hash12(qc.xy + qc.z * 23.0);
    vec2 uv; float face_salt; float face_shade;
    if (n.y > 0.5) {            // TOP — brightest, horizontal grid
        uv = wpos.xz; face_salt = 0.0; face_shade = 1.04;
    } else if (n.y < -0.5) {    // BOTTOM — darkest
        uv = wpos.xz; face_salt = 7.0; face_shade = 0.88;
    } else if (abs(n.x) > 0.5) {// X-facing SIDE — UV in z/y plane (runs vertically)
        uv = wpos.zy; face_salt = 3.0; face_shade = 0.96;
    } else {                    // Z-facing SIDE — UV in x/y plane
        uv = wpos.xy; face_salt = 3.0; face_shade = 0.96;
    }
    // r381: dropped the per-tile "grout" edge darkening — it read as a grid over
    // every block. Keep the per-block grain + per-face shade (the procedural
    // color+normal look we want until real art lands).
    float grain = hash12(floor(uv * 8.0) + (seed + face_salt) * 37.0);
    float tex = mix(0.85, 1.10, grain) * face_shade;
    return base * tex;
}

// ─── 3D value noise for volumetric moisture ─────────────────────────────
//
// Used by the fog march to give density actual spatial structure
// (wisps, banks, gaps) instead of the smooth low-frequency
// `height_fog × dist_layer` that produced "painted on" fog. The
// noise is sampled at march-sample world positions, advected by
// wind over time, so the moisture pattern visibly drifts.
//
// Implementation: trilinearly-interpolated value noise on a unit
// grid, hashed via the same scheme as `hash12`. Two octaves of
// fbm — first gives the big moisture banks (~10m wavelength),
// second adds finer wisps (~3m wavelength). Cheap; ~16 hashes
// per fbm() call.
//
// IMPORTANT: this is per-march-step, so cost scales with FOG_STEPS.
// With 6 steps × 16 hashes = 96 hashes per fragment. At 1080p that's
// ~200M hashes/frame, well within M2 Max's ALU budget. If profiling
// shows trouble, drop to 1 octave or reduce FOG_STEPS.

float hash13(vec3 p3) {
    p3 = fract(p3 * 0.1031);
    p3 += dot(p3, p3.zyx + 31.32);
    return fract((p3.x + p3.y) * p3.z);
}

float value_noise_3d(vec3 p) {
    vec3 i = floor(p);
    vec3 f = fract(p);
    // Smoothstep interpolation weights — quintic for nicer derivatives
    // than linear.
    vec3 u = f * f * (3.0 - 2.0 * f);
    // 8 corners of the unit cube.
    float c000 = hash13(i + vec3(0,0,0));
    float c100 = hash13(i + vec3(1,0,0));
    float c010 = hash13(i + vec3(0,1,0));
    float c110 = hash13(i + vec3(1,1,0));
    float c001 = hash13(i + vec3(0,0,1));
    float c101 = hash13(i + vec3(1,0,1));
    float c011 = hash13(i + vec3(0,1,1));
    float c111 = hash13(i + vec3(1,1,1));
    // Trilinear blend.
    float x00 = mix(c000, c100, u.x);
    float x10 = mix(c010, c110, u.x);
    float x01 = mix(c001, c101, u.x);
    float x11 = mix(c011, c111, u.x);
    float y0  = mix(x00, x10, u.y);
    float y1  = mix(x01, x11, u.y);
    return mix(y0, y1, u.z);
}

float fbm2(vec3 p) {
    // 2-octave fractal brownian motion. First octave is the dominant
    // signal; second adds finer detail. Total amplitude normalized
    // to [0,1].
    float n  = value_noise_3d(p) * 0.65;
          n += value_noise_3d(p * 2.7) * 0.35;
    return n;
}

// ─── PBR: Cook-Torrance BRDF (r68-01) ──────────────────────────────────
//
// Energy-conserving direct lighting replacing the old ad-hoc
// `albedo * (ndl*sun + ambient)`. GGX/Trowbridge-Reitz NDF + Smith
// geometry (Schlick-GGX, direct-light k) + Schlick fresnel.
//
// Quality/perf A/B (compile-time, zero runtime cost when off):
//   PBR_QUALITY      1 = full Cook-Torrance + height normal mapping,
//                    0 = cheap Lambert + Blinn spec (old-ish look).
//   PBR_NORMAL_MAP   perturb the geometric normal from the surface
//                    grain height (no tangent attribute needed).
//   NORMAL_STRENGTH  bump amount (0 = flat faces, 1 = strong).
//
// Material roughness/metallic SOURCE (documented): the texture/overlay
// material channel is not yet bound to this pipeline (no material
// sampler in the descriptor set — see textures_overlay_system.docx,
// which currently specifies per-block *colour*, not a PBR channel), so
// roughness/metallic are derived from the existing colour-based block
// classification (`is_water`/`is_stone`/default), with humidity wetting
// stone. When the overlay system exposes a material channel/sampler,
// swap `material_params()` to read it — no other change needed.
#define PBR_QUALITY    1
#define PBR_NORMAL_MAP 1

// ─── r244: SHADER VARIANTS (chunk.frag) ────────────────────────────────────
//
// Expensive per-fragment features are gated behind compile-time flags so the
// engine can compile cheaper VARIANTS of this shader and bind the lightest one
// a given chunk actually needs (far / flat / no-water / no-dynamic-lights
// chunks skip what they can't show anyway). Each flag DEFAULTS to the full,
// current behaviour — an undefined build is byte-equivalent to pre-r244 — so
// the "high" variant is unchanged and the others only ever REMOVE work.
//
// The CPU defines these (e.g. `-DCHUNK_FOG=0`) per variant; the selection of
// which variant per chunk is `sim::shader_variant` (pure, tested). See
// docs/design/shader_variants.md.
#ifndef CHUNK_FOG
#define CHUNK_FOG 1          // 0 = skip the volumetric fog march entirely
#endif
#ifndef CHUNK_FOG_STEPS
#define CHUNK_FOG_STEPS 6    // march sample count (lower = cheaper, coarser)
#endif
#ifndef CHUNK_WATER
#define CHUNK_WATER 1        // 0 = no water shading (reflection/refraction/caustics/glint)
#endif
#ifndef CHUNK_DYNAMIC_LIGHTS
#define CHUNK_DYNAMIC_LIGHTS 1 // 0 = sun + ambient only, skip the point-light loop
#endif
#ifndef CHUNK_SHADOWS
#define CHUNK_SHADOWS 1      // 0 = skip shadow-map PCF (fully lit)
#endif
const float NORMAL_STRENGTH = 0.6;
const float PI = 3.14159265359;

float distribution_ggx(float n_dot_h, float roughness) {
    float a  = roughness * roughness;
    float a2 = a * a;
    float d  = (n_dot_h * n_dot_h) * (a2 - 1.0) + 1.0;
    return a2 / max(PI * d * d, 1e-7);
}

// Smith geometry with Schlick-GGX, direct-light remap k = (r+1)²/8.
float geometry_smith(float n_dot_v, float n_dot_l, float roughness) {
    float r = roughness + 1.0;
    float k = (r * r) / 8.0;
    float gv = n_dot_v / (n_dot_v * (1.0 - k) + k);
    float gl = n_dot_l / (n_dot_l * (1.0 - k) + k);
    return gv * gl;
}

vec3 fresnel_schlick(float cos_theta, vec3 f0) {
    float m = clamp(1.0 - cos_theta, 0.0, 1.0);
    float m2 = m * m;
    return f0 + (1.0 - f0) * (m2 * m2 * m); // (1-cosθ)^5
}

// Per-block roughness/metallic/F0 from the colour-based material class.
// Most terrain is dielectric (metallic 0, F0 0.04); wet stone gets
// smoother (lower roughness) so it picks up a sun sheen.
void material_params(vec3 base_color, float humidity,
                     out float roughness, out float metallic, out vec3 f0) {
    metallic = 0.0;
    f0 = vec3(0.04);
    if (is_water(base_color)) {
        roughness = 0.05;          // smooth dielectric
        f0 = vec3(0.02);
    } else if (is_stone(base_color)) {
        float wet = clamp(humidity, 0.0, 1.0);
        roughness = mix(0.78, 0.38, wet); // wet stone is glossier
    } else {
        roughness = 0.90;          // dirt/grass/sand: rough dielectric
    }
    roughness = clamp(roughness, 0.04, 1.0);
}

// World-space tangent/bitangent for an axis-snapped (cardinal) face
// normal — matches the face_uv axes used elsewhere in this shader.
void face_tangents(vec3 n, out vec3 t, out vec3 b) {
    if (abs(n.x) > 0.5)      { t = vec3(0,0,1); b = vec3(0,1,0); } // X face: Z,Y
    else if (abs(n.y) > 0.5) { t = vec3(1,0,0); b = vec3(0,0,1); } // Y face: X,Z
    else                     { t = vec3(1,0,0); b = vec3(0,1,0); } // Z face: X,Y
}

// Perturb the geometric normal from the surface grain HEIGHT field
// (same fbm the albedo grain uses, so bumps line up with shading).
// Central-ish difference along the two face tangents — no normal/
// tangent vertex attribute required.
vec3 perturb_normal(vec3 n, vec3 world_pos, float strength) {
    vec3 t, b;
    face_tangents(n, t, b);
    const float E = 0.15;          // world-space sample step (m)
    float h0 = fbm2(world_pos * 1.7);
    float ht = fbm2((world_pos + t * E) * 1.7);
    float hb = fbm2((world_pos + b * E) * 1.7);
    float dhdt = (ht - h0) / E;
    float dhdb = (hb - h0) / E;
    vec3 perturbed = normalize(n - strength * (dhdt * t + dhdb * b));
    return perturbed;
}

void main() {
    // ─── Camera-space basics ────────────────────────────────────────────
    vec3 cam_pos  = ubo.camera_pos.xyz;
    vec3 to_cam   = cam_pos - v_world_pos;
    float dist    = length(to_cam);
    vec3 view_dir = to_cam / max(dist, 1e-4);

    // ─── Surface normal (axis-snapped, temporally stable) ───────────────
    //
    // Voxel faces are ALWAYS axis-aligned (one of ±X, ±Y, ±Z). The
    // raw cross-derivative gives the right answer plus tiny noise
    // from the 2x2 fragment quad's subpixel shift between frames.
    // That noise drives the "color jitter on tiny camera motion"
    // shimmer — the same surface point gets a slightly different
    // normal per frame, ndl shifts, lighting visibly changes.
    //
    // Fix: snap the noisy normal to the nearest cardinal axis. Since
    // every voxel face's true normal IS a cardinal axis, snapping
    // is correct (not an approximation), and the result is bit-stable
    // under camera motion. Computed by picking the axis whose absolute
    // value of the noisy normal is largest, then forcing the sign
    // from the noisy normal's sign.
    //
    // This eliminates the dFdx/dFdy temporal instability without
    // requiring a vertex normal attribute (which would mean reshaping
    // the mesher and reuploading every chunk's vertex buffer). When
    // we do add real vertex normals later (for lit non-axis-aligned
    // detail like leaves or vines), this fallback can stay as a
    // no-op for axis-aligned blocks.
    vec3 dx = dFdx(v_world_pos);
    vec3 dy = dFdy(v_world_pos);
    vec3 raw_normal = normalize(cross(dx, dy));
    vec3 abs_n = abs(raw_normal);
    vec3 normal;
    if (abs_n.x > abs_n.y && abs_n.x > abs_n.z) {
        normal = vec3(sign(raw_normal.x), 0.0, 0.0);
    } else if (abs_n.y > abs_n.z) {
        normal = vec3(0.0, sign(raw_normal.y), 0.0);
    } else {
        normal = vec3(0.0, 0.0, sign(raw_normal.z));
    }

    // ─── Sun lighting ───────────────────────────────────────────────────
    vec3  sun_dir   = ubo.sun_dir.xyz;
    float sun_power = ubo.sun_dir.w;
    vec3  to_sun    = -sun_dir;
    float ndl       = max(dot(normal, to_sun), 0.0);

    // Shadow attenuation. Project this fragment's world position into
    // light-clip-space using the cascade's view-projection, then sample
    // the shadow map with comparison filtering + 4-tap PCF.
    //
    // shadow_factor in [0,1]: 1 = fully lit, 0 = fully shadowed.
    //
    // Notes on the math:
    //   - light_clip is in clip space (w-divided to NDC at .w).
    //   - NDC xy in [-1,1] → uv in [0,1] via *0.5+0.5.
    //   - Vulkan NDC z is [0,1] already, no remap needed for the
    //     comparison reference value.
    //   - We do NOT flip Y here even though the main pass's projection
    //     applies a Vulkan Y-flip — the cascade's `light_view_proj`
    //     was built without that flip (see render/light.rs), so the
    //     uv space is consistent.
    float shadow_factor;
#if CHUNK_SHADOWS
    {
        vec4 light_clip = ubo.light_view_proj * vec4(v_world_pos, 1.0);
        vec3 ndc = light_clip.xyz / light_clip.w;
        // If outside the light's [0,1] z range, treat as fully lit:
        // the fragment is beyond the cascade's depth range so we
        // can't make a meaningful shadow query for it.
        if (ndc.z > 1.0 || ndc.z < 0.0) {
            shadow_factor = 1.0;
        } else {
            vec2 uv = ndc.xy * 0.5 + 0.5;
            // ─── Slope-scaled depth bias ─────────────────────────────
            //
            // Without a bias, the comparison `frag_z <= shadow_z` is
            // ambiguous when the fragment IS the shadow caster (every
            // surface shadows itself). Surfaces facing the sun
            // edge-on need a larger bias because their depth changes
            // rapidly across one shadow texel; surfaces facing the
            // sun head-on need almost none.
            //
            //   bias = max(0.001 * (1 - n·l), 0.0002)
            //
            // n·l close to 1 (sun-facing) → small bias (0.0002 floor),
            // n·l close to 0 (edge-on)    → larger bias (~0.001).
            //
            // Symptom this fixes: patterned dark patches on tree sides
            // and side-faces of blocks where the shadow was self-
            // shadowing the surface even though it should be lit.
            // Distinct from texel-snapping (which fixes shimmer
            // during motion); this fixes static patches that don't
            // move with the camera.
            float ndl = max(dot(normal, -ubo.sun_dir.xyz), 0.0);
            float bias = max(0.001 * (1.0 - ndl), 0.0002);
            float shadow_z = ndc.z - bias;

            // 4-tap PCF: sample 4 corners around the projected uv at
            // ±0.5 texel offset. The hardware's compare+linear filter
            // gives free 2×2 bilinear PCF per tap, so this 4-tap
            // pattern is effectively a soft 5×5 box.
            vec2 texel = 1.0 / vec2(textureSize(shadow_map, 0));
            float s0 = texture(shadow_map, vec3(uv + texel * vec2(-0.5, -0.5), shadow_z));
            float s1 = texture(shadow_map, vec3(uv + texel * vec2( 0.5, -0.5), shadow_z));
            float s2 = texture(shadow_map, vec3(uv + texel * vec2(-0.5,  0.5), shadow_z));
            float s3 = texture(shadow_map, vec3(uv + texel * vec2( 0.5,  0.5), shadow_z));
            shadow_factor = (s0 + s1 + s2 + s3) * 0.25;
        }
    }
#else
    // r244: no-shadows variant — fully lit (skip the shadow-map PCF taps).
    shadow_factor = 1.0;
#endif

    // Blend: shadowed surfaces still get some sun bleed from
    // skylight/scattering. mix(0.4, 1.0, shadow_factor) means a
    // fully-shadowed fragment retains 40% of the direct sun term —
    // that's roughly the contribution of indirect bounce + skylight
    // in a real outdoor scene, so shadows feel grounded but not
    // black voids.
    float sun_attenuation = mix(0.4, 1.0, shadow_factor);

    // (old ad-hoc `sun_term` removed — direct light is now the
    // Cook-Torrance evaluation below, see r68-01 PBR block.)

    // Hemisphere ambient: faces pointing UP receive more skylight
    // (they "see" more of the bright sky dome) than faces pointing
    // DOWN (they "see" the ground, which is darker). Sides get a
    // mid value. This grounds blocks into the world far better than
    // a uniform ambient term — a stone block's bottom is genuinely
    // darker than its top, even in shadow, because the sky just
    // isn't visible from below it.
    //
    // The split: up-facing fragments get sky color, down-facing
    // get a warm ground-bounce tint (sun color desaturated). The
    // factor is just normal.y mapped from [-1,1] to [0,1].
    float sky_factor    = clamp(normal.y * 0.5 + 0.5, 0.0, 1.0);
    vec3  sky_ambient   = ubo.sky_color.rgb * 0.6;
    vec3  ground_ambient = ubo.sun_color.rgb * vec3(0.5, 0.45, 0.35); // warmer
    vec3  hemi_ambient  = mix(ground_ambient, sky_ambient, sky_factor);
    // Combine the explicit (bin-provided) ambient with the
    // hemisphere term. Bin's ambient_color stays as a baseline
    // (color-grading hook); hemi adds the directional cue.
    vec3 ambient_term = ubo.ambient_color.rgb + hemi_ambient * 0.4;
    // (old `lighting = sun_term + ambient_term` removed; ambient_term
    // feeds the PBR ambient below.)

    // ─── Per-face procedural detail (texturing increment 1) ────────────
    //
    // Derive a block-face-local UV with NO new vertex attribute, mesher
    // change, or descriptor: voxel faces are axis-aligned, so the two
    // world axes perpendicular to the snapped `normal` give an in-plane
    // coordinate, and fract() makes it tile once per 1m block. This is the
    // same face-UV the real texture sampler will use later — proving it
    // here keeps the packed vertex format untouched for now.
    vec2 face_uv;
    if (abs(normal.x) > 0.5)      face_uv = v_world_pos.zy;
    else if (abs(normal.y) > 0.5) face_uv = v_world_pos.xz;
    else                          face_uv = v_world_pos.xy;
    vec2 cell = fract(face_uv);

    // Band-limited beveled edge: darken a thin border at each block
    // boundary, with the width derived from the screen-space footprint
    // (fwidth) so it stays a crisp ~1px line at any distance instead of
    // shimmering or vanishing. This is the derivative-based AA approach
    // procedural texturing relies on (vs prefiltered mips).
    vec2 d = min(cell, 1.0 - cell);     // distance to nearest edge per axis
    float edge = min(d.x, d.y);
    float aa = max(fwidth(edge), 1e-5);
    float bevel = smoothstep(0.0, 0.03 + aa, edge);  // 0 at border → 1 inside

    // r311: foliage skips the block-edge bevel — no hard cube outline on leaves etc.
    if (is_foliage(v_color)) bevel = 1.0;

    // Faint surface grain from the existing world-space 3D noise — solid
    // (3D) so it's seam-free and consistent across all faces of a block.
    // This is the shared-noise hook the tiling-break / wobble / roughness
    // variation will reuse.
    float grain = fbm2(v_world_pos * 1.7);
    float detail = mix(0.82, 1.0, bevel) * mix(0.94, 1.06, grain);

    vec3 albedo = v_color * detail;
    // r459: tint water by environmental fields (salinity/temperature/erosion/depth).
    if (is_water(v_color)) {
        float wd = ww_water_depth(v_world_pos.xz);
        float depth = wd > 0.0 ? wd : 2.0;
        float wt_temp = wt_temperature(v_world_pos);
        float wt_sal = wt_salinity(wt_temp, v_world_pos);
        albedo = tint_water(v_color, depth, wt_temp, wt_sal, 0.15) * detail;
    }
    if (ubo.camera_pos.w > 0.5) albedo = apply_block_texture(albedo, v_world_pos, normal);

    // UNENDLICH_SNOW (r358): altitude snow caps. ubo.sky_color.w = snow-line
    // altitude (0 = feature off). Above the line, blend albedo toward snow white
    // over a soft band; up-facing faces hold more snow than steep sides.
    if (ubo.sky_color.w > 0.5) {
        float snow_line = ubo.sky_color.w;
        float band      = 6.0;
        float s = smoothstep(snow_line - band, snow_line + band, v_world_pos.y);
        s *= mix(0.2, 1.0, clamp(normal.y, 0.0, 1.0)); // tops snowier than sides
        albedo = mix(albedo, vec3(0.92, 0.95, 1.0), s);
    }

    // ─── PBR shading (r68-01): Cook-Torrance direct + hemi ambient ──────
    float humidity_mat = clamp(ubo.atmos_params.y, 0.0, 1.0);
    float m_rough; float m_metal; vec3 m_f0;
    material_params(v_color, humidity_mat, m_rough, m_metal, m_f0);
    vec3 f0 = mix(m_f0, albedo, m_metal); // metals tint reflectance by albedo

    // Shading normal: perturb the snapped geometric normal by the grain
    // height field (normal mapping without a tangent attribute).
    vec3 N = normal;
#if PBR_NORMAL_MAP
    N = perturb_normal(normal, v_world_pos, NORMAL_STRENGTH);
#endif

    vec3 V = view_dir;
    vec3 L = to_sun;
    vec3 H = normalize(V + L);
    float n_dot_l = max(dot(N, L), 0.0);
    float n_dot_v = max(dot(N, V), 1e-4);
    float n_dot_h = max(dot(N, H), 0.0);
    float v_dot_h = max(dot(V, H), 0.0);

    vec3 sun_radiance = ubo.sun_color.rgb * sun_power * sun_attenuation;

    vec3 color;
#if PBR_QUALITY
    // Specular (Cook-Torrance). Note: we omit the diffuse 1/π and the
    // matching π in the light term (the bin authors sun/ambient colour
    // for the engine's existing non-π convention) so absolute exposure
    // matches the previous shading; energy is still conserved because
    // kd steals exactly what fresnel reflects.
    float D = distribution_ggx(n_dot_h, m_rough);
    float G = geometry_smith(n_dot_v, n_dot_l, m_rough);
    vec3  F = fresnel_schlick(v_dot_h, f0);
    vec3  spec = (D * G * F) / max(4.0 * n_dot_v * n_dot_l, 1e-4);

    vec3 kd = (vec3(1.0) - F) * (1.0 - m_metal); // metals have no diffuse
    vec3 direct = (kd * albedo + spec) * sun_radiance * n_dot_l;

    // Hemispheric ambient: diffuse from the sky/ground term + a cheap
    // environment-specular fresnel (no cubemap), roughness-attenuated.
    vec3 ambient_diffuse = ambient_term * albedo * (1.0 - m_metal);
    vec3 amb_F = fresnel_schlick(n_dot_v, f0);
    vec3 ambient_spec = amb_F * ubo.sky_color.rgb * (1.0 - m_rough) * 0.25;

    color = direct + ambient_diffuse + ambient_spec;
#else
    // Cheap A/B path: Lambert diffuse + a single Blinn-Phong spec lobe.
    float spec_pow = mix(8.0, 256.0, 1.0 - m_rough);
    float blinn = pow(n_dot_h, spec_pow) * (1.0 - m_rough);
    color = albedo * (sun_radiance * n_dot_l + ambient_term)
          + sun_radiance * blinn * f0;
#endif
    // Keep a faint floor so deep-shadow faces never crush to pure black
    // (matches the old `max(lighting, 0.15)` intent).
    color = max(color, albedo * 0.12);

    // ─── Material-specific overlays ─────────────────────────────────────
    float humidity = ubo.atmos_params.y;
    // temperature (atmos_params.z) is no longer consumed in the fog
    // march — see v0.11.2 notes; if temperature-driven effects come
    // back they should land outside the per-step loop to avoid
    // interacting with the jitter.

    // CLOUD must be checked BEFORE stone because cloud cubes have
    // r ≈ g ≈ b (bright) and would otherwise pass is_stone.
    if (is_cloud(v_color)) {
        // Cloud rendered as soft fog material.
        vec3 cloud_lit = mix(ubo.sky_color.rgb, vec3(1.0), 0.3);
        cloud_lit += ubo.sun_color.rgb * (ndl * sun_power * 0.4);

        float fresnel = pow(1.0 - max(dot(normal, view_dir), 0.0), 2.5);
        color = mix(cloud_lit, ubo.sky_color.rgb, fresnel * 0.7);

        float sun_align_c = max(dot(view_dir, to_sun), 0.0);
        float warm_c = pow(sun_align_c, 6.0) * sun_power;
        color = mix(color, ubo.sun_color.rgb, warm_c * 0.4);
    } else if (is_water(v_color)) {
        // ─── AAA water surface (r68-04) ─────────────────────────────────
        // Analytic sum-of-sines surface (water_waves.glsl — the same field
        // the CPU buoyancy port uses) → per-pixel normal; Schlick fresnel
        // blends a planar/sky REFLECTION against a Beer-Lambert depth-
        // attenuated REFRACTION tint; foam at wave crests; a cheap animated
        // caustic shimmer. No scene-colour/depth texture is bound, so the
        // reflection is planar-sky and the refraction is analytic (true SSR
        // + refractive sampling would need the scene colour+depth bound — a
        // descriptor change, deferred). All tunables are local consts.
        const float WATER_F0          = 0.02;                   // reflectance at normal incidence
        const vec3  WATER_ABSORB      = vec3(0.46, 0.10, 0.06);  // per-metre absorption (red dies first)
        const vec3  WATER_SHALLOW     = vec3(0.10, 0.34, 0.42);
        const vec3  WATER_DEEP        = vec3(0.01, 0.06, 0.13);
        const float WATER_FOAM_EDGE   = 0.62;                   // crest fraction where foam begins
        const float WATER_FOAM_AMT    = 0.55;
        const float WATER_CAUSTIC_AMT = 0.18;
        const bool  WATER_FOAM        = true;                   // A/B toggle (hot-reload)
        const bool  WATER_CAUSTICS    = true;

        float w_time   = ubo.atmos_params.w;
        float w_temp   = ubo.atmos_params.z;
        vec2  wind_xy  = ubo.wind_params.xy;
        float wind_mag = ubo.wind_params.z;

        // Wave normal from the shared analytic field; only top-facing water
        // gets rippled (side faces keep the block normal).
        WaveParams wp  = water_wave_params(wind_xy, wind_mag, w_temp, w_time);
        // r420: global wave-speed multiplier (ubo.wind_params.w; 0 → treated as 1.0 so the
        // default is unchanged). Lets the wave animation be slowed without touching amplitude.
        wp.speed_scale *= (ubo.wind_params.w > 0.0 ? ubo.wind_params.w : 1.0);
        float water_flow_speed = 0.0; // r420: current strength, for flow-driven foam
        // r419 (flow-as-shader): blend the sim's current into the wave direction. Calm water
        // keeps wind chop; flowing water streams along its current and ripples faster.
        // Gated by ubo.flow_coverage.w (0 = wind-only, identical to before).
        if (ubo.flow_coverage.w > 0.5) {
            vec2 fxz = v_world_pos.xz;
            vec2 rel = (fxz - ubo.flow_coverage.xy) / ubo.flow_coverage.z;
            if (rel.x >= 0.0 && rel.y >= 0.0 && rel.x < 16.0 && rel.y < 16.0) {
                // r422: bilinear across cell centres so the current direction varies
                // smoothly instead of snapping at every 32-block cell edge.
                vec2 fc = rel - 0.5;
                vec2 fl = floor(fc);
                ivec2 c0 = ivec2(fl);
                vec2 fr = clamp(fc - fl, 0.0, 1.0);
                vec2 f00 = ww_flow_cell(c0.x,     c0.y);
                vec2 f10 = ww_flow_cell(c0.x + 1, c0.y);
                vec2 f01 = ww_flow_cell(c0.x,     c0.y + 1);
                vec2 f11 = ww_flow_cell(c0.x + 1, c0.y + 1);
                vec2 flow = mix(mix(f00, f10, fr.x), mix(f01, f11, fr.x), fr.y);
                float fspeed = length(flow);
                water_flow_speed = fspeed;
                if (fspeed > 1e-4) {
                    vec2 fdir = flow / fspeed;
                    float t = clamp(fspeed * 1.5, 0.0, 1.0);
                    vec2 bd = mix(wp.dir, fdir, t);
                    wp.dir = (length(bd) > 1e-3) ? normalize(bd) : fdir;
                    wp.speed_scale *= (1.0 + min(fspeed, 2.0) * 0.6);
                }
            }
        }
        float grazing  = clamp(abs(view_dir.y), 0.0, 1.0);
        vec3  wnormal  = water_wave_normal(v_world_pos.xz, wp, grazing);
        float up_face  = clamp(normal.y, 0.0, 1.0);
        vec3  N        = normalize(mix(normal, wnormal, up_face));

        // Schlick fresnel — more reflective at grazing angles.
        float cos_t = max(dot(N, view_dir), 0.0);
        float fres  = WATER_F0 + (1.0 - WATER_F0) * pow(1.0 - cos_t, 5.0);

        // Planar/sky reflection: reflect the view about N, shade against the
        // sky gradient, add a tight sun glint.
        vec3  refl_dir   = reflect(-view_dir, N);
        float up_r       = clamp(refl_dir.y * 0.5 + 0.5, 0.0, 1.0);
        vec3  reflection = mix(ubo.sky_color.rgb * 0.80, ubo.sky_color.rgb * 1.10, up_r);
        float glint      = pow(max(dot(refl_dir, to_sun), 0.0), 200.0) * sun_power;
        reflection      += ubo.sun_color.rgb * glint;

        // Refraction: no scene sample available, so an analytic body colour
        // attenuated by Beer-Lambert over a path length. r431: use the sim's REAL water depth
        // here when available (shallow puddles → short path → clear/light; deep water → long
        // path → dark blue) so depth reads as volume, not flat. Falls back to the old
        // view-angle estimate outside the sim window / when depth-color is off.
        float angle_path = mix(1.0, 9.0, 1.0 - grazing);
        float wdepth     = ww_water_depth(v_world_pos.xz);
        float path;
        if (wdepth >= 0.0) {
            // real depth, lengthened a little at grazing angles (more water along the ray)
            path = clamp(wdepth, 0.0, 16.0) * mix(1.0, 2.2, 1.0 - grazing);
        } else {
            path = angle_path;
        }
        vec3  clarity    = exp(-WATER_ABSORB * path);           // transmittance
        vec3  refraction = mix(WATER_DEEP, WATER_SHALLOW, clarity);

        // r435: volumetric depth treatment (real depth grid). Makes water read as a body you see
        // INTO rather than a flat sheet: a sunlit faux-bottom shows through shallow water (displaced
        // by the wave normal so it shimmers like refraction), light penetration warms the shallows,
        // and a foam ribbon rides the waterline where depth shades to nothing. Inert (wdepth<0) where
        // there's no depth data, so it only ever adds detail to sim-covered water.
        float shore_foam = 0.0;
        if (wdepth >= 0.0) {
            float shallow01 = exp(-max(wdepth, 0.0) * 0.6);     // 1 at the edge → 0 in the deep
            vec2  refr_uv   = v_world_pos.xz + N.xz * (1.0 + wdepth) * 0.35;
            float sandn     = fbm2(vec3(refr_uv * 0.5, 0.0));
            vec3  bottom    = mix(vec3(0.30, 0.34, 0.26), vec3(0.52, 0.50, 0.38), sandn);
            bottom         *= (0.4 + 0.6 * sun_power) * ubo.sun_color.rgb;
            refraction      = mix(refraction, bottom, shallow01 * 0.7);     // see into the shallows
            refraction     += vec3(0.02, 0.06, 0.05) * shallow01 * sun_power; // light penetration
            float rim       = smoothstep(0.7, 0.05, wdepth) * smoothstep(0.0, 0.12, wdepth);
            shore_foam      = rim * (0.5 + 0.5 * fbm2(vec3(v_world_pos.xz * 1.3, w_time * 0.1)));
        }

        // Animated caustic shimmer folded into the refracted light.
        if (WATER_CAUSTICS) {
            // r204: slower (w_time*0.18 not *0.5) and broken up with fbm so
            // it stops reading as a fast regular grid. Operator: the white
            // pass "goes way too fast and is too patterned (no noise)".
            vec2  cuv  = v_world_pos.xz * 0.6 + wp.dir * w_time * 0.18;
            float c1   = sin(cuv.x + sin(cuv.y * 1.3 + w_time * 0.4));
            float c2   = sin(cuv.y * 1.7 - cuv.x * 0.6 - w_time * 0.5);
            float caus = pow(max(c1 * c2, 0.0), 3.0);
            // fbm modulation: dims the regular lattice unevenly so it looks
            // like dappled light, not a moving screen-door.
            float cnoise = fbm2(vec3(v_world_pos.xz * 0.9, w_time * 0.12));
            caus *= 0.45 + 0.55 * cnoise;
            refraction += ubo.sun_color.rgb * caus * WATER_CAUSTIC_AMT * clarity * sun_power;
        }

        // Reflection/refraction blend by fresnel.
        vec3 water = mix(refraction, reflection, fres);

        // r443: side/bottom faces of a water body fall back to the flat block normal (the wave +
        // see-into treatment is top-only), so from the side or below deep water reads as opaque
        // slabs. Tint non-top faces toward the deep-water colour in proportion to the body depth, so
        // a tall water wall darkens with depth and reads as a volume rather than a sheet. A modest
        // cue, not the real fix (a transparent draw pass / sub-voxel surface is filed). Toggle = A/B.
        const bool WATER_SIDE_DEPTH_TINT = true;
        if (WATER_SIDE_DEPTH_TINT && up_face < 0.5 && wdepth >= 0.0) {
            float deep01 = clamp(wdepth / 8.0, 0.0, 1.0);
            water = mix(water, WATER_DEEP, deep01 * 0.5);
        }

        // r448: DITHERED (screen-door) TRANSPARENCY so water reads as see-through in the OPAQUE
        // pass — no alpha-blend pipeline needed (true blending is the bigger staged change with
        // black-screen risk). We discard a depth-based fraction of water fragments in an ordered
        // 4x4 Bayer pattern; the terrain behind — drawn in the same opaque pass — shows through the
        // gaps. Shallow water is mostly transparent (short light path), deep water stays opaque, so
        // the see-through tracks the depth colour. Toggle + depth scale are local consts for A/B.
        const bool  WATER_DITHER_TRANSP = true;
        const float WATER_MIN_OPACITY   = 0.30; // shallowest water opacity (lower = more see-through)
        const float WATER_OPAQUE_DEPTH  = 6.0;  // depth (blocks) at which water becomes fully opaque
        if (WATER_DITHER_TRANSP) {
            float wd = (wdepth >= 0.0) ? wdepth : (WATER_OPAQUE_DEPTH * 0.6);
            float opacity = clamp(
                WATER_MIN_OPACITY + (1.0 - WATER_MIN_OPACITY) * (wd / WATER_OPAQUE_DEPTH),
                WATER_MIN_OPACITY, 1.0);
            int bx = int(gl_FragCoord.x) & 3;
            int by = int(gl_FragCoord.y) & 3;
            float bayer[16] = float[16](
                 0.0/16.0,  8.0/16.0,  2.0/16.0, 10.0/16.0,
                12.0/16.0,  4.0/16.0, 14.0/16.0,  6.0/16.0,
                 3.0/16.0, 11.0/16.0,  1.0/16.0,  9.0/16.0,
                15.0/16.0,  7.0/16.0, 13.0/16.0,  5.0/16.0);
            if (bayer[by * 4 + bx] >= opacity) discard;
        }

        // Specular sun highlight off the rippled normal.
        vec3  half_vec = normalize(to_sun + view_dir);
        float spec     = pow(max(dot(N, half_vec), 0.0), 128.0) * sun_power;
        water += ubo.sun_color.rgb * spec;

        // Foam at wave crests. r204: the old foam was a clean smoothstep of
        // the analytic wave height — a regular moving band with no texture
        // (operator: "too patterned, no noise on top, physics behind it is
        // strange"). Now (a) gate foam on wave STEEPNESS (the gradient
        // magnitude — foam forms where water is actually piling/breaking,
        // not merely high), and (b) break the crest mask with fbm noise so
        // the foam is dappled and organic, not a screen-door. Slower, too.
        if (WATER_FOAM) {
            float h      = water_wave_height(v_world_pos.xz, wp);
            float crestA = max(WW_BASE_AMP * wp.amp_scale, 1e-3);
            // steepness from the rippled normal: flat water (N.y≈1) → 0,
            // steep crest faces → up toward 1. This is the "physics behind
            // it" — foam tracks where the surface is actually steep.
            float steep  = clamp(1.0 - N.y, 0.0, 1.0);
            float crest  = smoothstep(WATER_FOAM_EDGE, 1.0, h / crestA);
            // organic breakup: two fbm octaves, drifting slowly.
            float fn     = fbm2(vec3(v_world_pos.xz * 1.6, w_time * 0.08));
            float foam   = crest * smoothstep(0.15, 0.55, steep + 0.25) * fn;
            // r420: flow-driven whitewater — fast current foams even without steep crests
            // (rapids). Only contributes when the flow shader is on (speed stays 0 otherwise).
            float rapids = smoothstep(0.25, 1.2, water_flow_speed) * fn;
            foam = max(foam, rapids);
            foam = max(foam, shore_foam); // r435: waterline ribbon
            water = mix(water, vec3(0.86, 0.90, 0.95), clamp(foam, 0.0, 1.0) * WATER_FOAM_AMT);
        }

        color = water;
    } else if (is_stone(v_color)) {
        // Wet stone is handled by PBR (lower roughness when humid, plus
        // the ambient-spec fresnel). Keep only the slight wet darkening
        // of the diffuse albedo here; the old ad-hoc sky-fresnel add is
        // removed to avoid double-counting the sheen.
        float wetness = clamp(humidity, 0.0, 1.0);
        color *= mix(1.0, 0.85, wetness);
    }

    // ─── Underside shading ──────────────────────────────────────────────
    // Faces pointing down get less light (acts like ambient occlusion).
    float up = dot(normal, vec3(0.0, 1.0, 0.0));
    color *= mix(0.6, 1.0, smoothstep(-1.0, 0.2, up));

    // ─── Volumetric fog (shadow-aware in-scatter) ───────────────────────
    //
    // 6-step march with logarithmic depth spacing. v0.13 introduced
    // cascaded shadow maps; v0.14 wires sampling on opaque surfaces.
    // This block extends shadow sampling to the FOG MARCH itself:
    // each step samples the shadow map at its world-space position,
    // and contributes in-scatter only if that position is in light.
    //
    // Result: sunbeams. Shafts of brighter fog where the sun reaches
    // through gaps; darker fog where geometry blocks it. The "god
    // ray" effect that volumetric fog is famous for.
    //
    // This is the pre-froxel approximation. A real froxel system
    // builds a 3D scattering texture in a compute pass, which lets
    // adjacent pixels share march samples for free. We get most of
    // the visual win (shadowed in-scatter) without the compute
    // infrastructure. When froxels do land, they replace this loop
    // with three texture samples; the shader-side architecture is
    // designed to be ready.
    //
    // v0.11.2 calmer-fog tuning is preserved: smooth low-frequency
    // density × per-pixel hash jitter on `t`. The new piece is
    // `lit_factor`: 0..1 multiplier per step from the shadow map.

    const int FOG_STEPS = CHUNK_FOG_STEPS;

    // ─── Froxel volume consumption (v0.17) ──────────────────────────────
    //
    // The compute shader (`froxel_fill.comp`) writes per-froxel local
    // scattering contribution every frame. Each froxel stores
    // `(rgb_scattered, local_transmittance)` where transmittance > 0
    // = "this froxel was filled" and = 0 means "unfilled, fall back".
    //
    // We sample the volume at six points along the view ray, doing
    // front-to-back integration: each successive sample is darkened by
    // the cumulative transmittance of the samples in front of it.
    //
    // Compared to v0.15's per-fragment march, the per-step work
    // (fbm noise, shadow query, height fog, altitude rolloff, wind
    // advection) is BAKED INTO each froxel by the compute pass.
    // So per-fragment cost drops from "6 * (fbm + shadow_sample +
    // smoothstep + ...)" to "6 * texture lookups". Big win — and
    // critically, adjacent fragments share the same froxel samples
    // (low-frequency volume) so the result is also smoother than
    // the per-fragment march was.
    //
    // The volume covers FROXEL_NEAR_M (0.5m) to FROXEL_FAR_M (200m).
    // Fragments outside that range fall back to the per-fragment march
    // at the bottom of this block.
    const float FROXEL_NEAR = 0.5;
    const float FROXEL_FAR  = 200.0;

    // World position → froxel UVW. UV come from screen-space (we use
    // gl_FragCoord against viewport_rect), W comes from logarithmic
    // mapping of distance-along-view to [0,1].
    //
    // The screen-space UV is shared across this fragment and its
    // froxel — perfect for non-view-aligned geometry: a wall fragment
    // and the air beyond it both project to the same froxel column,
    // so they get integrated through the same volume slice.
    vec2 frag_uv = gl_FragCoord.xy / ubo.viewport_rect.xy;

    // Probe sentinel: if the volume hasn't been filled this frame
    // (e.g., bins like unendlich-chunk that don't run the compute
    // pass), the volume is still at vec4(0,0,0,0). Detect by a
    // central probe.
    vec4 froxel_center_probe = texture(froxel_volume, vec3(0.5));
    bool froxel_filled = froxel_center_probe.a > 0.0001;

    // Front-to-back integration. With v0.17.2's cumulative-
    // integration compute kernel, each froxel cell stores the
    // CUMULATIVE (rgb_acc, trans_acc) from the camera through the
    // far edge of that cell. So we sample ONCE at the W coordinate
    // corresponding to the surface depth and read the full integrated
    // fog directly.
    vec3  fog_color_acc = vec3(0.0);
    float fog_trans_acc = 1.0; // remaining transmittance to camera

    if (froxel_filled) {
        // Surface depth → froxel W coordinate. The W axis is log-
        // distributed between FROXEL_NEAR and FROXEL_FAR in view-
        // space distance. Surfaces past FROXEL_FAR clamp to W=1 and
        // get the full integrated atmospheric extinction.
        float ray_far = min(dist, FROXEL_FAR);
        float w       = log(ray_far / FROXEL_NEAR) / log(FROXEL_FAR / FROXEL_NEAR);
        w             = clamp(w, 0.0, 1.0);

        // Single trilinear-filtered sample. The hardware sampler
        // interpolates between adjacent cumulative-state cells which
        // is the right behavior — cumulative state is monotonic in
        // Z, so the linear interpolation gives a valid intermediate
        // integration along the ray.
        vec4 froxel = texture(froxel_volume, vec3(frag_uv, w));
        fog_color_acc = froxel.rgb;
        fog_trans_acc = froxel.a;
    }

    // Final compositing: surface color is dimmed by remaining
    // transmittance, then we add scattered light. This is the
    // standard volumetric compositing formula.
    if (froxel_filled) {
        color = color * fog_trans_acc + fog_color_acc;

        // Skip the per-fragment march below — the froxel volume IS

        // the fog now. The rest of this `void main()` only handles
        // the underwater tint and final tonemap.
        // (Easier to express via an early return after underwater
        // and tonemap blocks; we set fog effectively to zero so
        // the existing march code below is a no-op.)
    }

    // Per-pixel jitter ∈ [0, 1) — based on WORLD position, not
    // screen position. Screen-position-based jitter shimmers under
    // tiny camera motion because the same surface point gets a
    // different screen pixel each frame → different jitter →
    // different fog → visible color drift. World-position-based
    // jitter is invariant under camera motion: the fog at a given
    // world point looks identical from any camera angle, frame to
    // frame. The cost is a small loss of pixel-level decorrelation
    // (adjacent fragments on the same surface see similar jitter)
    // — but with our smooth low-frequency density, that's invisible.
    //
    // ONLY USED IF froxel_filled IS FALSE — fallback per-fragment
    // march for bins that don't run the compute fill.
    vec2 world_uv_for_hash = floor(v_world_pos.xz * 4.0); // 0.25m granularity
    float jitter = hash12(world_uv_for_hash);

    float fog_accum = 0.0;
    // Track lit-fog separately from total-fog so we can tint only
    // the lit portion warm (toward sun_color) while the shadowed
    // portion stays neutral sky color. This is what gives sunbeams
    // their warm glow against cool ambient haze.
    float lit_fog_accum = 0.0;
    float base_density = ubo.atmos_params.x;
    float humidity_curve = pow(humidity, 1.6);
    float density_at_player = base_density * mix(0.6, 2.2, humidity_curve);

    float prev_t = 0.0;
    if (!froxel_filled) {
    for (int i = 0; i < FOG_STEPS; i++) {
        // Logarithmic spacing + per-pixel jitter.
        float fi = (float(i) + jitter) / float(FOG_STEPS);
        float t  = dist * fi * fi;
        float step_len = max(t - prev_t, 0.0);
        prev_t = t;

        vec3 p = cam_pos - view_dir * t;

        // Atmospheric density at this march sample.
        //
        // Three contributions, all smooth and continuous from t=0
        // (no "fog only beyond 10m" cutoff — that hard transition
        // is what makes our previous fog feel painted on rather
        // than atmospheric):
        //
        //   - Height fog: thicker low (where humid air pools), thinner
        //     high. Exponential falloff with altitude. Always present.
        //   - Distance haze: standard atmospheric extinction along the
        //     view ray. Now uses smooth power curve from t=0 instead
        //     of smoothstep starting at 10m. This is what makes even
        //     a 5m-distant block pick up a tiny bit of atmospheric
        //     tint and feel grounded in the world.
        //   - Altitude-rolloff: density drops with march sample's
        //     altitude (the air at 200m up is genuinely thinner).
        //
        // The result: nearby fragments get a faint, warm-or-cool fog
        // wash; distant ones get the full atmospheric extinction.
        // The transition is smooth and never visible as a "fog
        // boundary".
        // Clamp altitude for atmospheric calcs to prevent exp blowup
        // when march samples are far below sea level (player digs
        // deep, or looks down a long view ray onto low terrain).
        float alt_y = clamp(p.y, -20.0, 200.0);
        float height_fog = exp(-alt_y * 0.05);
        float dist_layer = pow(t / 200.0, 0.7); // smooth from t=0
        float altitude_rolloff = exp(-max(alt_y - 60.0, 0.0) * 0.01);
        float smooth_density = density_at_player *
            (height_fog * 0.5 + dist_layer * 1.0) * altitude_rolloff;

        // ─── 3D moisture noise: wisps, banks, gaps ────────────────────
        //
        // Sample a 3D fbm in world space, advected by wind. This is
        // what gives the fog actual VOLUME — visible structure that
        // looks like moisture pockets drifting through the world,
        // not a uniform haze layer.
        //
        // Wind advection: shift the noise lookup by `-wind * time`
        // so the moisture pattern moves WITH the wind. A negative
        // shift on the lookup is equivalent to a positive shift on
        // the pattern (same trick as scrolling textures). Wind speed
        // is in world units per second; we scale by `time_seconds`.
        //
        // Two scales of noise are blended in by altitude:
        //   - Lower altitudes (valley floors): denser, larger wisps
        //     (the warm humid air pools low and forms big banks).
        //   - Higher altitudes: sparser, finer detail (wispier).
        // This is what makes valleys feel cold-foggy and ridges
        // feel clear, which is the intuitive behavior real
        // mountain weather shows.
        //
        // The 0.05 frequency = ~20m wavelength for the dominant
        // octave, ~7m for the second. Tuned so a player walking
        // through fog passes through visible structure rather than
        // a uniform haze.
        // `atmos_params.w` is `time_of_day` in seconds — already a
        // monotonic wall-clock value, no rescaling needed.
        float time_seconds = ubo.atmos_params.w;
        vec2  wind_xy = ubo.wind_params.xy;
        float wind_speed = length(wind_xy);
        vec3  wind_offset = vec3(wind_xy.x, 0.0, wind_xy.y) * time_seconds;
        // Wind-advected blobs sheared into streaks. MUST stay identical to
        // froxel_fill.comp::sample_cell so the froxel volume and this
        // fallback march agree at the same world position.
        const float MOISTURE_FREQ = 0.05;
        const float STREAK_GAIN   = 2.0;
        const float STREAK_MAX    = 5.0;
        vec3  noise_pos = (p - wind_offset) * MOISTURE_FREQ;
        if (wind_speed > 1e-4) {
            vec2  wdir    = wind_xy / wind_speed;
            float along   = dot(noise_pos.xz, wdir);
            vec2  across  = noise_pos.xz - along * wdir;
            float stretch = 1.0 + min(wind_speed * STREAK_GAIN, STREAK_MAX);
            noise_pos.xz  = across + (along / stretch) * wdir;
        }
        float moisture = smoothstep(0.15, 0.85, fbm2(noise_pos));
        // Bias toward "more moisture in valleys / less on ridges".
        // p.y mapped through a smooth curve: dense low, thin high.
        float altitude_moisture_bias = clamp(1.5 - alt_y * 0.012, 0.2, 1.4);
        float wisp_factor = mix(0.4, 1.6, moisture) * altitude_moisture_bias;

        // ─── Player proximity falloff ────────────────────────────────
        //
        // Fog visibly parts as the player moves through it. The
        // density attenuates within ~3m of the camera, so dense
        // banks don't completely envelop the camera in opaque
        // moisture. Realistic — your body's wake displaces the
        // immediate air mass — and gameplay-friendly — you can
        // always see where you're going.
        float dist_from_cam = length(p - cam_pos);
        float proximity_clear = smoothstep(0.0, 3.0, dist_from_cam);

        float layered_density = smooth_density * wisp_factor * proximity_clear;
        // Hard ceiling — see compute fill rationale.
        layered_density = min(layered_density, 2.0);

        // Shadow query for this march sample. Project p into
        // light-clip-space; if outside the cascade or if the
        // shadow comparison says "in shadow", the step
        // contributes only to neutral fog (no in-scatter).
        // Otherwise it contributes to BOTH fog_accum (general
        // haze) and lit_fog_accum (warm-tintable sun shafts).
        float lit_factor = 1.0;
        vec4  lc = ubo.light_view_proj * vec4(p, 1.0);
        vec3  lndc = lc.xyz / lc.w;
        if (lndc.z >= 0.0 && lndc.z <= 1.0) {
            vec2 luv = lndc.xy * 0.5 + 0.5;
            if (luv.x >= 0.0 && luv.x <= 1.0 && luv.y >= 0.0 && luv.y <= 1.0) {
                // Single-tap PCF here; fog doesn't need 4-tap
                // softening because the integration along the
                // ray already smooths the result.
                lit_factor = texture(shadow_map, vec3(luv, lndc.z));
            }
        }

        fog_accum     += layered_density * step_len;
        lit_fog_accum += layered_density * step_len * lit_factor;
    }

    float fog = 1.0 - exp(-fog_accum);
    fog = clamp(fog, 0.0, 1.0);
    // lit_ratio: 0 = all shadowed, 1 = all lit. Drives how warm
    // the fog tints toward sun_color.
    float lit_ratio = (fog_accum > 1e-4) ? (lit_fog_accum / fog_accum) : 1.0;

    // ─── Phase function: Henyey-Greenstein for the sun-aligned scatter ──
    //
    // Replaces v0.11.x's `pow(sun_amount, 6.0)` ad-hoc forward-scatter
    // term with a proper single-scatter phase function. HG with g≈0.6
    // captures Mie scattering's strong forward lobe — what physically
    // creates the "sunbeam" / "halo around the sun" effect — without
    // the analytic complexity of the full Mie integral.
    //
    // g = 0 → isotropic (pure Rayleigh-ish). g → 1 → pure forward.
    // 0.6 is the typical "atmospheric haze with reasonable sun
    // anisotropy" setting; tweakable per-biome eventually (a dusty
    // desert wants higher g, a clear high-altitude sky lower).
    //
    // Why this matters for the user's "atmosphere derives from world
    // state" vision: g is itself a candidate for biome-driven
    // variation. Filed: hook g to atmos_params (humidity already
    // affects density; particle size / haze type would affect g).
    float sun_amount  = max(dot(view_dir, to_sun), 0.0);
    const float HG_G  = 0.6;
    float g2          = HG_G * HG_G;
    // Standard HG: (1 - g²) / (4π · (1 + g² - 2g·cosθ)^1.5)
    // We drop the 4π normalization since we use the result as a
    // tinting weight, not a physical radiance.
    float hg          = (1.0 - g2)
                      / pow(1.0 + g2 - 2.0 * HG_G * sun_amount, 1.5);

    // ─── Altitude-driven sky color shift ────────────────────────────────
    //
    // As the camera (and march samples) climb higher, the air column
    // overhead is thinner and shorter-wavelength scatter dominates →
    // sky tilts toward deeper blue. Below sea level (caves, water)
    // it tilts warmer. This is a cheap approximation of the real
    // atmosphere's Rayleigh extinction profile.
    //
    // sky_color base is what the bin computes from time-of-day; we
    // bias it slightly per-pixel here using the camera altitude.
    // Range: -50m → 200m maps to [warm, cool].
    float altitude_t = clamp((cam_pos.y - (-50.0)) / 250.0, 0.0, 1.0);
    vec3  sky_low    = ubo.sky_color.rgb * vec3(1.04, 1.00, 0.92); // warmer
    vec3  sky_high   = ubo.sky_color.rgb * vec3(0.88, 0.95, 1.10); // cooler/bluer
    vec3  sky_tinted = mix(sky_low, sky_high, altitude_t);

    // ─── Rayleigh-style directional tint ────────────────────────────────
    //
    // View rays looking toward the sun pick up warm scatter
    // (long-wavelength dominates near the source), away from the sun
    // pick up cool blue scatter (short-wavelength survives the
    // longer atmospheric path). This is the classic "sunset is red,
    // anti-sun is blue" effect, in miniature. Reused for fog tinting.
    float to_sun_dot = clamp(dot(view_dir, to_sun) * 0.5 + 0.5, 0.0, 1.0);
    vec3  ray_warm   = vec3(1.10, 0.96, 0.84);
    vec3  ray_cool   = vec3(0.90, 0.96, 1.08);
    vec3  ray_tint   = mix(ray_cool, ray_warm, to_sun_dot);

    // Combine everything for the final fog color:
    //   - Base: altitude-tinted sky.
    //   - Multiply by Rayleigh-style directional tint (always small
    //     bias, it's a multiplier near 1.0).
    //   - Lit portion mixes toward warm sun_color via the HG phase
    //     (instead of pow(sun_amount, ...)). Only lit fog goes warm,
    //     so god rays pop against cool ambient haze.
    //   - Distant fog still desaturates toward the v0.11.x neutral
    //     gray for far horizon depth cues; preserved as final mix.
    vec3  base_fog = sky_tinted * ray_tint;
    float warm_amount = clamp(hg * 0.6 * sun_power * lit_ratio, 0.0, 1.0);
    vec3  fog_color = mix(base_fog, ubo.sun_color.rgb, warm_amount);
    fog_color = mix(fog_color, vec3(0.85), clamp(dist / 200.0, 0.0, 1.0));

    // ─── r158: Climate white-balance ─────────────────────────────────────
    // atmos_params.z (biome temperature) was an unconsumed channel: cold
    // biomes now read slightly blue-cool, hot ones warm — a subtle grade
    // (max ~8% per channel) applied before fog so the fog keeps its color.
    {
        float temp01 = clamp(ubo.atmos_params.z, 0.0, 1.0);
        vec3 cool = vec3(0.94, 0.98, 1.07);
        vec3 warm = vec3(1.06, 1.00, 0.93);
        vec3 grade = mix(cool, warm, smoothstep(0.2, 0.8, temp01));
        color *= mix(vec3(1.0), grade, 0.8);
    }

    // ─── r136: Height fog — mist pools in valleys ────────────────────────
    // Extra fog for fragments BELOW the camera, strongest at dawn/dusk and
    // high humidity (atmos_params.y), exponential in depth-below-eye. Uses
    // only existing inputs; zero cost when the fragment is above the eye.
    {
        float below   = max(ubo.camera_pos.y - v_world_pos.y, 0.0);
        float tod     = ubo.atmos_params.w;                       // [0,1) day phase
        float dawn    = exp(-pow((tod - 0.02) * 8.0, 2.0))        // around sunrise
                      + exp(-pow((tod - 0.52) * 8.0, 2.0));       // around sunset
        float humid   = clamp(ubo.atmos_params.y, 0.0, 1.0);
        float hfog    = (1.0 - exp(-below * 0.05))
                      * (0.10 + 0.25 * dawn) * (0.4 + 0.6 * humid)
                      * clamp(dist / 40.0, 0.0, 1.0);
        color = mix(color, base_fog, clamp(hfog, 0.0, 0.5));
    }

    color = mix(color, fog_color, fog);

    // ─── Aerial perspective (distance desaturation toward sky) ──────────
    //
    // Distinct from the volumetric fog above: aerial perspective is the
    // wavelength-dependent in-scatter that makes FAR terrain (distant hills,
    // ridgelines) fade toward the sky's in-scattered color and lose contrast,
    // independent of local fog banks. It is what sells scene depth in clear
    // weather where `fog` itself is near zero.
    //
    // We blend `color` toward the same `base_fog` in-scattered tint by a
    // smooth distance ramp that only becomes significant in the far field
    // (>~120m), so near and mid terrain are untouched. Crucially this runs
    // ONLY in the non-froxel branch — when the froxel volume is active it
    // already integrates in-scatter along the ray, so applying aerial
    // perspective here too would DOUBLE the haze. (See the r68-03 froxel
    // coordination note: aerial perspective blends with froxel fog, not on
    // top of it.)
    //
    // The ramp uses the surface distance, biased by view elevation: rays
    // looking toward the horizon (low view_dir.y) accumulate more in-scatter
    // for the same distance, matching real aerial perspective where the
    // horizon hazes first.
    float aerial_dist = smoothstep(120.0, 320.0, dist);
    float horizon_bias = 1.0 - clamp(view_dir.y * 0.5 + 0.5, 0.0, 1.0); // 1 at/below horizon
    float aerial = clamp(aerial_dist * mix(0.55, 1.0, horizon_bias), 0.0, 0.85);
    color = mix(color, base_fog, aerial);
    } // end if (!froxel_filled)

    // ─── Underwater tint ────────────────────────────────────────────────
    //
    // Earlier versions did `cam_pos.y < v_world_pos.y + 0.2` which
    // fires for ANY surface above the camera, including distant
    // terrain seen across a valley — turning the whole frame blue
    // when the player walks downhill. The right test is "is the camera
    // currently inside water", which we can't fully answer without a
    // BlockId-at-camera lookup; but we can require the surface ALSO
    // to be water (so it's only a tint when looking at water from
    // below water level), which avoids the false positive on land.
    bool underwater = cam_pos.y < v_world_pos.y + 0.2 && is_water(v_color);
    if (underwater) {
        vec3 water_tint = vec3(0.0, 0.4, 0.7);
        float depth = clamp((v_world_pos.y - cam_pos.y) * 0.2, 0.0, 1.0);
        color = mix(color, water_tint, depth * 0.5);
        color *= vec3(0.75, 0.88, 1.0);
    }

    // ─── UV debug overlay (texturing increment 1) ───────────────────────
    //
    // Procedural proof that per-face UVs reconstruct correctly from world
    // position + the axis-snapped normal, BEFORE any texture image exists.
    // Mirrors world/texture_mapping.rs::face_uv — each face uses its two
    // tangent world axes; fract() gives seamless 1m tiling. We draw a faint
    // 1m grid (proves the UV origin sits on block corners) plus a 2x2 sub-
    // checker (proves UV spans [0,1] across the face). Set SHOW_UV_DEBUG to
    // false to return to normal rendering.
    //
    // Uses only existing varyings: NO new vertex attribute and NO descriptor.
    // Increment 2 will carry a per-block tile id in the reserved color-alpha
    // byte (proven lossless by the ruleset) to select a real tile.
    const bool SHOW_UV_DEBUG = false; // r381: the 2x2 checker + grid was a debug
                                      // overlay left hardcoded on — off now (no real
                                      // atlas yet; procedural color+normals carry the look).
    if (SHOW_UV_DEBUG) {
        vec2 uvd;
        if (abs(normal.x) > 0.5) {
            uvd = fract(vec2(v_world_pos.z, v_world_pos.y)); // X faces: tangents Z,Y
        } else if (abs(normal.y) > 0.5) {
            uvd = fract(vec2(v_world_pos.x, v_world_pos.z)); // Y faces: tangents X,Z
        } else {
            uvd = fract(vec2(v_world_pos.x, v_world_pos.y)); // Z faces: tangents X,Y
        }
        // 2x2 checker across the face. r320: foliage (leaves/grass/ferns) get
        // flat vertex color — they're nonsolid and shouldn't read as a textured
        // placeholder. Solids keep the checker until the real atlas lands.
        if (!is_foliage(v_color)) {
            float checker = mod(floor(uvd.x * 2.0) + floor(uvd.y * 2.0), 2.0);
            color *= mix(0.78, 1.0, checker);
        }
        // Thin grid lines hugging the block boundary (uv near 0 or 1).
        // r311: skipped on foliage — those edge lines are the box outline we're
        // removing. The 2x2 checker stays (texture placeholder, consistent with
        // solid blocks until the real atlas lands).
        if (!is_foliage(v_color)) {
            vec2 edge = min(uvd, 1.0 - uvd);
            float grid = smoothstep(0.0, 0.04, min(edge.x, edge.y));
            color *= mix(0.55, 1.0, grid);
        }
    }

    // ─── Tone mapping & color grading ───────────────────────────────────
    color = aces_film(color);
    color = filmic_contrast(color, 0.08);

    // ─── Vignette (proper screen-space) ─────────────────────────────────
    //
    // Uses viewport_rect from the UBO so vignette intensity is correct
    // at any resolution. Multi-cam-ready.
    vec2 viewport_size = max(ubo.viewport_rect.xy, vec2(1.0, 1.0));
    vec2 viewport_off  = ubo.viewport_rect.zw;
    vec2 screen_uv = (gl_FragCoord.xy - viewport_off) / viewport_size;
    vec2 vv = (screen_uv - 0.5) * 2.0;
    float vignette_dist = dot(vv, vv);
    float vignette = 1.0 - smoothstep(0.5, 1.7, vignette_dist) * 0.25;
    color *= vignette;

    // No film-grain dither in v0.11.2: the previous attempt at
    // ±0.5/255 dither was correct in amplitude (truly invisible),
    // but combined with per-pixel jitter on the fog march producing
    // per-pixel salt-and-pepper, the fog was the actual culprit. With
    // the calmer fog above, dither is unnecessary; banding in
    // smooth gradients is now hidden by the volumetric structure.

    // r427: underwater screen FX — applied when the camera is submerged. Water absorbs red
    // first (blue-green tint), an animated caustic shimmer bands the light, and the edges
    // darken for the "looking through water" feel. (A geometric image-warp needs the post
    // pass; this colour/shimmer layer carries most of the underwater atmosphere.)
    if (ubo.fx_params.x > 0.5) {
        float ut = ubo.fx_params.y;
        float shimmer = sin(screen_uv.x * 22.0 + ut * 1.7)
                      * sin(screen_uv.y * 18.0 - ut * 1.3);
        shimmer = 0.5 + 0.5 * shimmer;
        vec3 tinted = color * vec3(0.30, 0.62, 0.85); // absorb red (water)
        color = mix(color, tinted, 0.7);
        color += vec3(0.012, 0.045, 0.060) * shimmer; // faint caustic glow
        color *= (0.88 + 0.20 * shimmer);             // caustic light banding
        color *= 1.0 - smoothstep(0.2, 1.7, vignette_dist) * 0.35; // murk at edges
    }

    out_color = vec4(color, 1.0);
}

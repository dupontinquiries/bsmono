#version 450
//
// sky.frag — analytic atmospheric sky (Rayleigh + Mie) for the far plane.
//
// Drawn by sky.vert as a fullscreen triangle behind all geometry (depth
// LESS_OR_EQUAL, no depth write). Reconstructs a world-space view ray from the
// interpolated NDC and the inverse camera view_proj, then evaluates a simplified
// single-scattering analytic sky in that direction:
//   - Rayleigh in-scatter: blue zenith, the dominant daytime tint.
//   - Mie in-scatter: forward-biased haze (Henyey-Greenstein) for the warm halo
//     around the sun and horizon brightening.
//   - Sun disk + glow keyed off the sun direction.
//   - Day/dusk variation from sun elevation: when the sun is low, the Rayleigh
//     path length grows and the horizon warms toward orange/red.
//
// DESCRIPTOR CONTRACT: reuses the existing CameraUbo at set=0 binding=0 (the
// SAME struct chunk.frag uses) — NO new descriptor set, binding, or UBO field.
// We need view_proj (to invert), camera_pos, sun_dir, sun_color, sky_color, and
// atmos_params (turbidity is read from a documented default since the UBO has no
// turbidity field — see TURBIDITY below; promote to a UBO field later if desired).
//
// COST: a few dozen ALU ops + one mat4 inverse per fragment. The inverse could
// be precomputed on the CPU into a spare UBO field later; kept in-shader here so
// the descriptor stays untouched for a first version (per the brief).
//
// PERF NOTE (r243, not yet done — needs a CPU-side UBO change so it's deferred):
// `inverse(ubo.view_proj)` below runs PER FRAGMENT for every sky pixel (a full-
// screen triangle → potentially the whole framebuffer). A 4×4 inverse is ~tens
// of FLOPs; across a 4K sky that is millions of redundant inverses of a value
// that is constant for the whole frame. The fix is a one-field UBO addition:
// add `mat4 inv_view_proj;` to CameraUbo (and the matching Rust struct), have
// the CPU write `view_proj.inverse()` once per frame, and replace the
// `inverse(ubo.view_proj)` call here with `ubo.inv_view_proj`. Pure speedup,
// identical output. Held back here only because it's a descriptor/UBO-layout
// change that must be made + runtime-verified together with the Rust side (see
// docs/design/shader_pass_notes.md). The validator (tools/validate_shaders.sh)
// will confirm the GLSL still compiles after the field is added.

layout(location = 0) in vec2 v_ndc;
layout(location = 0) out vec4 out_color;

// Must match chunk.frag's CameraUbo byte-for-byte (set 0, binding 0).
layout(set = 0, binding = 0) uniform CameraUbo {
    mat4 view_proj;
    vec4 camera_pos;       // xyz = eye position
    vec4 sun_dir;          // xyz = direction sunlight TRAVELS, w = directional intensity [0,1]
    vec4 sun_color;        // rgb = sun radiance
    vec4 sky_color;        // rgb = sky / fog target
    vec4 ambient_color;    // rgb = ambient term
    vec4 atmos_params;     // x = fog_density, y = humidity, z = temperature, w = time_of_day
    vec4 wind_params;      // xy = wind vector, z = wind magnitude
    vec4 viewport_rect;    // xy = viewport size px, zw = viewport offset px
    mat4 light_view_proj;  // light-space VP
} ubo;

// ─── Tunable atmosphere constants (documented defaults) ────────────────────
//
// TURBIDITY: haze amount. 1 = pristine high-altitude air, ~10 = thick haze.
// 2.4 is a clean-temperate default. No UBO field exists for it yet; if biome-
// driven turbidity is wanted, add atmos_params -> a w-component reuse or a new
// field and the matching Rust UBO struct (descriptor change — out of scope here).
const float TURBIDITY = 2.4;

// Rayleigh and Mie scale heights as relative in-scatter weights. These are not
// physical units — they balance the blue (Rayleigh) vs white/warm (Mie) lobes.
const vec3  RAYLEIGH_COEFF = vec3(5.8e-1, 13.5e-1, 33.1e-1); // ∝ 1/λ^4, blue-heavy
const float MIE_COEFF      = 21.0e-1;
const float MIE_G          = 0.76; // forward scatter anisotropy for the sun halo

// Sun disk angular size (cos of half-angle). ~0.9995 ≈ 0.5° disk.
const float SUN_DISK_COS = 0.9995;

// Rayleigh phase: (3/16π)(1 + cos²θ). Drop the constant (we normalize visually).
float rayleigh_phase(float cos_theta) {
    return 0.75 * (1.0 + cos_theta * cos_theta);
}

// Henyey-Greenstein Mie phase (forward lobe → sun halo).
float mie_phase(float cos_theta, float g) {
    float g2 = g * g;
    // r243: pow(x, 1.5) == x * sqrt(x), but x*sqrt(x) is cheaper than the
    // generic pow() (one mul + one sqrt vs an exp/log pair). Runs per fragment;
    // visually identical. Validated via tools/validate_shaders.sh.
    float b = max(1.0 + g2 - 2.0 * g * cos_theta, 1e-4);
    float denom = b * sqrt(b);
    return (1.0 - g2) / denom;
}

void main() {
    // ─── Reconstruct world-space view ray from NDC ──────────────────────
    // Unproject a far-plane point (ndc, z=1) and a near point (z=0), the
    // direction between them is the view ray.
    mat4 inv_vp = inverse(ubo.view_proj);
    vec4 near_h = inv_vp * vec4(v_ndc, 0.0, 1.0);
    vec4 far_h  = inv_vp * vec4(v_ndc, 1.0, 1.0);
    vec3 near_p = near_h.xyz / near_h.w;
    vec3 far_p  = far_h.xyz  / far_h.w;
    vec3 view_dir = normalize(far_p - near_p);

    // Sun direction: ubo.sun_dir is the direction light TRAVELS, so the
    // direction TOWARD the sun is its negation.
    vec3  to_sun    = normalize(-ubo.sun_dir.xyz);
    float sun_power = ubo.sun_dir.w;

    // Sun elevation in [-1,1] (1 = overhead, 0 = horizon, <0 = below).
    float sun_elev = to_sun.y;
    // Day factor: 0 at/under the horizon, 1 high in the sky. Smooth dusk band.
    float day = smoothstep(-0.08, 0.30, sun_elev);

    // ─── Air-mass / horizon path length ─────────────────────────────────
    // Looking toward the horizon, the optical path is much longer than
    // looking up. Approximate with 1/(cosθ + small) clamped.
    float up = max(view_dir.y, 0.0);
    float air_mass = 1.0 / (up + 0.10);          // ~10 at horizon, ~1 at zenith
    air_mass = clamp(air_mass, 1.0, 12.0);

    // ─── Scatter phases ─────────────────────────────────────────────────
    float cos_theta = dot(view_dir, to_sun);
    float r_phase = rayleigh_phase(cos_theta);
    float m_phase = mie_phase(cos_theta, MIE_G);

    // Rayleigh in-scatter: blue-dominant, modulated by air mass and turbidity.
    // Longer path (low sun, horizon view) shifts the surviving light warm by
    // attenuating blue more — approximated by an exp extinction on the coeffs.
    vec3  r_ext   = exp(-RAYLEIGH_COEFF * air_mass * 0.12);
    vec3  rayleigh = RAYLEIGH_COEFF * r_phase * r_ext;

    // Mie in-scatter: near-white haze, forward-biased toward the sun, scaled by
    // turbidity. Adds the warm halo and horizon brightening.
    float mie = MIE_COEFF * m_phase * TURBIDITY * 0.02;

    // Combine. sky_color from the UBO is the bin's time-of-day base tint; we use
    // it to ground the analytic result to the rest of the frame's grading.
    vec3 sky = (rayleigh + vec3(mie)) * mix(0.15, 1.0, day);
    sky = mix(sky, ubo.sky_color.rgb, 0.35);     // anchor to bin's sky tint

    // ─── Horizon warmth at dusk ─────────────────────────────────────────
    // Near the horizon when the sun is low, push toward sun_color (orange/red).
    float horizon = 1.0 - up;                    // 1 at horizon, 0 at zenith
    float dusk    = (1.0 - day) * smoothstep(0.0, 0.6, horizon);
    sky = mix(sky, ubo.sun_color.rgb, dusk * 0.5 * sun_power);

    // ─── Sun disk + glow ────────────────────────────────────────────────
    float sun_glow = pow(max(cos_theta, 0.0), 8.0);          // broad glow
    float sun_disk = smoothstep(SUN_DISK_COS, 1.0, cos_theta); // sharp disk
    // Disk and glow only above the horizon and only when the sun is up.
    float sun_vis  = step(0.0, to_sun.y) * sun_power;
    sky += ubo.sun_color.rgb * sun_glow * 0.25 * sun_vis;
    sky += ubo.sun_color.rgb * sun_disk * 6.0  * sun_vis;     // bright core (HDR)

    // ─── r136: Night sky — procedural starfield + moon ──────────────────
    // Stars: hash the quantized view direction; the brightest cells become
    // stars, twinkling slowly on time_of_day. Fades in as `day` fades out
    // and is occluded by horizon glow (only above ~10° elevation).
    float night = 1.0 - day;
    if (night > 0.02 && view_dir.y > 0.05) {
        vec3  sq  = floor(view_dir * 220.0);
        float h   = fract(sin(dot(sq, vec3(127.1, 311.7, 74.7))) * 43758.5453);
        float star = smoothstep(0.9975, 1.0, h);
        float tw  = 0.75 + 0.25 * sin(ubo.atmos_params.w * 60.0 + h * 6.2831);
        sky += vec3(0.9, 0.95, 1.0) * star * tw * night * smoothstep(0.05, 0.2, view_dir.y);
    }
    // Moon: a cool disk opposite the sun, visible at night.
    {
        vec3  to_moon  = normalize(-to_sun + vec3(0.0, 0.18, 0.0));
        float moon_cos = dot(view_dir, to_moon);
        float moon_disk = smoothstep(0.9996, 0.9999, moon_cos);
        float moon_glow = pow(max(moon_cos, 0.0), 64.0) * 0.08;
        float moon_vis  = night * step(0.0, to_moon.y);
        sky += vec3(0.75, 0.78, 0.85) * (moon_disk * 1.6 + moon_glow) * moon_vis;
    }

    out_color = vec4(max(sky, vec3(0.0)), 1.0);
}

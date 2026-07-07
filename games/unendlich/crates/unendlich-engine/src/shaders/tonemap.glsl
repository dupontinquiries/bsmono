// tonemap.glsl — shared HDR -> display tonemap + cinematic grade + vignette.
//
// Used by BOTH chunk.frag (the direct-to-swapchain path) and present.frag (the
// post-FX path), so the final look is identical wherever tone mapping happens.
//
// When UNENDLICH_POSTFX is active, chunk.frag writes LINEAR HDR scene color into
// the R16G16B16A16_SFLOAT offscreen target (skipping the tonemap, gated on
// ubo.wind_params.w) and present.frag applies this chain ONCE at the end — which
// is also where bloom + SSR composite into linear color before tone mapping.
//
// r68-05: the curve is now selectable (Narkowicz ACES / ACES fitted (Hill) / AgX)
// and exposure is a compile-time knob, so the operator can A/B on the M2 by
// rebuilding (build.rs recompiles the .spv). These are #defines rather than new
// uniforms ON PURPOSE: the present pass has no UBO and only a one-float push
// constant, so adding runtime uniforms would change the descriptor/push contract
// (descriptor_gate / ubo_parity). Promoting CURVE/EXPOSURE to a runtime uniform
// is a follow-up that must also extend the Rust pipeline layout + UBO.
//
// Color space: every curve returns LINEAR display-referred color (AgX's internal
// sRGB ODT is undone with a 2.2 EOTF) so the swapchain applies its sRGB OETF
// exactly once — "work in linear, encode at the very end" stays true. The two
// paths (POSTFX 0/1) share this file, so they remain matched to each other.

#ifndef TONEMAP_GLSL
#define TONEMAP_GLSL

// -- operator knobs (defaults below; override at compile time to A/B) --
#ifndef TONEMAP_CURVE
#define TONEMAP_CURVE 2        // 0 = Narkowicz ACES, 1 = ACES fitted (Hill), 2 = AgX (default)
#endif
#ifndef TONEMAP_EXPOSURE
#define TONEMAP_EXPOSURE 1.0   // linear exposure multiplier, applied before the curve
#endif
#ifndef FILMIC_CONTRAST
#define FILMIC_CONTRAST 0.08   // midtone S-curve strength (neutral 0.0)
#endif
#ifndef VIGNETTE_STRENGTH
#define VIGNETTE_STRENGTH 0.25 // corner darkening (neutral 0.0)
#endif
#ifndef ENABLE_DITHER
#define ENABLE_DITHER 1        // output dithering to kill dark-gradient banding
#endif

float tm_luma(vec3 c) {
    return dot(c, vec3(0.2126, 0.7152, 0.0722));
}

// -- ACES Narkowicz (2015 fit) — the legacy curve (TONEMAP_CURVE 0) --
vec3 aces_film(vec3 x) {
    const float a = 2.51;
    const float b = 0.03;
    const float c = 2.43;
    const float d = 0.59;
    const float e = 0.14;
    return clamp((x * (a * x + b)) / (x * (c * x + d) + e), 0.0, 1.0);
}

// -- ACES fitted (Stephen Hill RRT+ODT fit) — richer than Narkowicz (CURVE 1) --
vec3 rrt_odt_fit(vec3 v) {
    vec3 a = v * (v + 0.0245786) - 0.000090537;
    vec3 b = v * (0.983729 * v + 0.4329510) + 0.238081;
    return a / b;
}
vec3 aces_fitted(vec3 color) {
    // GLSL column-major matrices (canonical Hill form).
    const mat3 in_mat = mat3(
        0.59719, 0.07600, 0.02840,
        0.35458, 0.90834, 0.13383,
        0.04823, 0.01566, 0.83777);
    const mat3 out_mat = mat3(
         1.60475, -0.10208, -0.00327,
        -0.53108,  1.10813, -0.07276,
        -0.07367, -0.00605,  1.07602);
    color = in_mat * color;
    color = rrt_odt_fit(color);
    color = out_mat * color;
    return clamp(color, 0.0, 1.0);
}

// -- AgX (minimal approximation, Sobotka/Wrensch) — gentle desaturating
//    highlight rolloff, the modern filmic default (CURVE 2). --
vec3 agx_contrast_approx(vec3 x) {
    vec3 x2 = x * x;
    vec3 x4 = x2 * x2;
    return 15.5 * x4 * x2 - 40.14 * x4 * x + 31.96 * x4
         - 6.868 * x2 * x + 0.4298 * x2 + 0.1191 * x - 0.00232;
}
vec3 agx_tonemap(vec3 color) {
    const mat3 agx_in = mat3(
        0.842479062253094, 0.0423282422610123, 0.0423756549057051,
        0.0784335999999992, 0.878468636469772,  0.0784336,
        0.0792237451477643, 0.0791661274605434, 0.879142973793104);
    const mat3 agx_out = mat3(
         1.19687900512017,   -0.0528968517574562, -0.0529716355144438,
        -0.0980208811401368,  1.15190312990417,   -0.0980434501171241,
        -0.0990297440797205, -0.0989611768448433,  1.15107367264116);
    const float min_ev = -12.47393;
    const float max_ev = 4.026069;

    color = agx_in * color;
    color = clamp(log2(max(color, vec3(1e-10))), min_ev, max_ev);
    color = (color - min_ev) / (max_ev - min_ev);
    color = agx_contrast_approx(color);   // AgX "base" look (sRGB-encoded)
    color = agx_out * color;
    color = clamp(color, 0.0, 1.0);
    // Undo AgX's sRGB ODT so we stay linear display-referred; the swapchain
    // re-encodes once at write. (2.2 EOTF approximation.)
    return pow(color, vec3(2.2));
}

// Selected tonemap curve (compile-time).
vec3 tonemap_curve(vec3 c) {
#if TONEMAP_CURVE == 0
    return aces_film(c);
#elif TONEMAP_CURVE == 1
    return aces_fitted(c);
#else
    return agx_tonemap(c);
#endif
}

// Filmic contrast S-curve. Slight midtone contrast bump without clipping.
vec3 filmic_contrast(vec3 color, float strength) {
    return mix(color, smoothstep(0.0, 1.0, color), strength);
}

// Tone map + cinematic grade (color-only): linear HDR -> graded display [0,1].
vec3 tonemap_grade(vec3 color) {
    const float SATURATION = 1.10;  // neutral 1.0
    const float SPLIT_TONE = 0.35;  // neutral 0.0

    color *= TONEMAP_EXPOSURE;            // exposure (before the curve)
    color = tonemap_curve(color);         // selectable ACES/AgX
    color = filmic_contrast(color, FILMIC_CONTRAST);

    // Vibrance.
    float grade_luma = tm_luma(color);
    color = mix(vec3(grade_luma), color, SATURATION);

    // Split-tone: cool shadows, warm highlights (small bias, reads as grading).
    vec3 cool_shadows    = vec3(0.93, 0.98, 1.08);
    vec3 warm_highlights = vec3(1.08, 1.00, 0.92);
    vec3 tone = mix(cool_shadows, warm_highlights, smoothstep(0.0, 1.0, grade_luma));
    color *= mix(vec3(1.0), tone, SPLIT_TONE);
    return clamp(color, 0.0, 1.0);
}

// Screen-space vignette. `screen_uv` in [0,1] across the viewport (so chunk.frag
// can pass gl_FragCoord/viewport and present.frag can pass its fullscreen v_uv —
// both radial from the screen centre, so the result matches).
vec3 apply_vignette(vec3 color, vec2 screen_uv) {
    vec2 vv = (screen_uv - 0.5) * 2.0;
    float vignette_dist = dot(vv, vv);
    float vignette = 1.0 - smoothstep(0.5, 1.7, vignette_dist) * VIGNETTE_STRENGTH;
    return color * vignette;
}

// Output dithering: a triangular-PDF hash applied at ~1 LSB (8-bit) right before
// the swapchain write, to break up banding in dark skies/shadow gradients.
// Cheap, no inputs. Present-path only (chunk.frag's direct path is unchanged).
vec3 apply_dither(vec3 color, vec2 screen_uv) {
#if ENABLE_DITHER
    vec2 s = screen_uv * 1024.0;
    float r0 = fract(sin(dot(s, vec2(12.9898, 78.233))) * 43758.5453);
    float r1 = fract(sin(dot(s + 17.0, vec2(12.9898, 78.233))) * 43758.5453);
    float tri = (r0 + r1) - 1.0; // triangular distribution in [-1, 1]
    return color + tri * (1.0 / 255.0);
#else
    return color;
#endif
}

// Optional subtle procedural lens-dirt mask (>= 1.0; brightens bloom in patches).
// Used only when ENABLE_LENS_DIRT is set by the includer (default off), so no
// texture binding is added.
float lens_dirt_mask(vec2 uv) {
    float d = 1.0;
    d += 0.40 * smoothstep(0.40, 0.0, length(uv - vec2(0.30, 0.68)));
    d += 0.30 * smoothstep(0.32, 0.0, length(uv - vec2(0.74, 0.40)));
    d += 0.22 * smoothstep(0.26, 0.0, length(uv - vec2(0.55, 0.80)));
    return d;
}

#endif // TONEMAP_GLSL

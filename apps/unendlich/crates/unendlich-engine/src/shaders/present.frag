#version 450
// v0.24.113: declare the include extension explicitly so the shader compiles
// under glslangValidator (its #include support is gated on this), not just
// glslc. glslc accepts the pragma too, so Linux/glslc builds are unchanged.
#extension GL_GOOGLE_include_directive : require

// Present / post-process fragment shader (gated post-FX pipeline).
//
// The offscreen target is R16G16B16A16_SFLOAT holding LINEAR HDR scene color
// (chunk.frag skips its tonemap under post-FX). This pass is the COMPOSITE
// STAGE: it adds bloom (and later SSR) into the linear color, then applies the
// shared tonemap + grade + vignette ONCE and writes the swapchain. The same
// chain runs in chunk.frag on the direct-to-swapchain path, so POSTFX=1 with
// bloom OFF stays identical to POSTFX=0.
//
// Bindings (set 0): binding 0 = scene_color (offscreen HDR), binding 1 =
// bloom_color (half-res bloom target; bound to the scene image when bloom is
// off so the descriptor is always valid). Push constant bloom_intensity is 0
// when bloom is off, so the composite is an exact no-op. See
// toolkit/harness/RULESET.md §bloom.

#include "tonemap.glsl"

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D scene_color; // linear HDR scene
layout(set = 0, binding = 1) uniform sampler2D bloom_color; // half-res bloom

layout(push_constant) uniform Pc {
    float bloom_intensity; // 0.0 when bloom is off (exact no-op)
} pc;

// r68-05: optional procedural lens-dirt on bloom (off by default — no new
// texture binding, so the descriptor set is unchanged). Set to 1 to A/B.
#ifndef ENABLE_LENS_DIRT
#define ENABLE_LENS_DIRT 0
#endif

void main() {
    vec3 color = texture(scene_color, v_uv).rgb;   // linear HDR scene
    // Bloom composites additively in LINEAR space, before the tonemap. The
    // half-res target is bilinearly upsampled by the sampler. (SSR will add
    // here too, in linear, before tonemap_grade.)
    vec3 bloom = texture(bloom_color, v_uv).rgb;
#if ENABLE_LENS_DIRT
    bloom *= lens_dirt_mask(v_uv);                 // subtle smudge highlights
#endif
    color += bloom * pc.bloom_intensity;
    color = tonemap_grade(color);
    color = apply_vignette(color, v_uv);
    // Dither at the very end (display space) to kill banding before the 8-bit
    // swapchain quantizes. ~1 LSB triangular noise.
    color = apply_dither(color, v_uv);
    out_color = vec4(color, 1.0);
}

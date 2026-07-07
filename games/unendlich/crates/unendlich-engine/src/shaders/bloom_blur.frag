#version 450

// bloom_blur.frag — one axis of a separable Gaussian for bloom (HDR).
//
// The fragment-shader counterpart of ui_blur.comp: samples a sampler2D source
// and writes the half-res bloom target (RGBA16F, not the rgba8 the UI blur
// clamps to), so HDR energy > 1.0 survives. Rendered with the fullscreen
// present.vert and run TWICE, ping-ponging two half-res targets:
//   pass H: dir = (1/width, 0)  src = bright-pass A   dst = B
//   pass V: dir = (0, 1/height) src = B               dst = A
// 9-tap, pre-normalized weights (sum = 1), same kernel as the UI blur. See
// toolkit/harness/RULESET.md §bloom.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D src;

layout(push_constant) uniform Pc {
    vec2 dir_texel; // one-texel step along the blur axis (1/size on that axis)
} pc;

// r68-05: wider, smoother glow via LINEAR-SAMPLED Gaussian taps. Bilinear taps
// placed between texels let 3 fetches per side cover the same energy as the old
// discrete 9-tap while reaching further (and at *fewer* samples: 5 vs 9). The
// offsets/weights are the standard linear reduction of a Gaussian; BLOOM_BLUR_WIDTH
// scales the reach so the operator can widen/tighten the glow on the M2.
#ifndef BLOOM_BLUR_WIDTH
#define BLOOM_BLUR_WIDTH 1.8
#endif
const float OFF[3] = float[](0.0, 1.3846153846, 3.2307692308);
const float WT[3]  = float[](0.2270270270, 0.3162162162, 0.0702702703);

void main() {
    vec3 c = texture(src, v_uv).rgb * WT[0];
    for (int i = 1; i < 3; ++i) {
        vec2 off = pc.dir_texel * (OFF[i] * BLOOM_BLUR_WIDTH);
        c += texture(src, v_uv + off).rgb * WT[i];
        c += texture(src, v_uv - off).rgb * WT[i];
    }
    out_color = vec4(c, 1.0);
}

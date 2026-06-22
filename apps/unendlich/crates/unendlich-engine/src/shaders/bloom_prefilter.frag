#version 450

// bloom_prefilter.frag — bloom bright-pass + half-res downsample.
//
// Rendered with the fullscreen-triangle present.vert into the HALF-RES bloom
// target. Samples the linear-HDR scene with a 4-tap bilinear box (the cheap
// downsample, same trick as ui_downsample.comp) and applies a soft-knee
// threshold so only bright energy (sky, sun disc, specular glints on water)
// bleeds. The result is composited additively in present.frag BEFORE the
// tonemap, in linear space. Gated by UNENDLICH_BLOOM (requires UNENDLICH_POSTFX,
// whose offscreen target holds the linear HDR scene). The blur that turns this
// into a soft glow is a follow-up pass; see toolkit/harness/RULESET.md §bloom.

layout(location = 0) in  vec2 v_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D scene_color; // linear HDR scene

layout(push_constant) uniform Pc {
    float threshold; // luma at/below this contributes nothing
    float knee;      // soft-knee width (smooth onset above the threshold)
} pc;

void main() {
    // 4-tap box downsample of the full-res scene around this half-res texel.
    // Each tap is itself bilinear, so ±0.5 source-texel offsets average the
    // 2x2 footprint a half-res texel covers.
    vec2 texel = 1.0 / vec2(textureSize(scene_color, 0));
    vec3 t0 = texture(scene_color, v_uv + texel * vec2(-0.5, -0.5)).rgb;
    vec3 t1 = texture(scene_color, v_uv + texel * vec2( 0.5, -0.5)).rgb;
    vec3 t2 = texture(scene_color, v_uv + texel * vec2(-0.5,  0.5)).rgb;
    vec3 t3 = texture(scene_color, v_uv + texel * vec2( 0.5,  0.5)).rgb;

    // r68-05: Karis average instead of a plain box. Weight each tap by
    // 1/(1+luma) so a single very bright sub-pixel (a firefly / specular
    // sparkle) can't dominate the downsample — this is what keeps the bloom
    // from shimmering. (Karis, "Tone-mapped fireflies", SIGGRAPH 2013.)
    float l0 = dot(t0, vec3(0.2126, 0.7152, 0.0722));
    float l1 = dot(t1, vec3(0.2126, 0.7152, 0.0722));
    float l2 = dot(t2, vec3(0.2126, 0.7152, 0.0722));
    float l3 = dot(t3, vec3(0.2126, 0.7152, 0.0722));
    float w0 = 1.0 / (1.0 + l0);
    float w1 = 1.0 / (1.0 + l1);
    float w2 = 1.0 / (1.0 + l2);
    float w3 = 1.0 / (1.0 + l3);
    vec3 c = (t0 * w0 + t1 * w1 + t2 * w2 + t3 * w3) / (w0 + w1 + w2 + w3);

    // Soft-knee threshold (Karis/COD): a quadratic knee of width `knee` gives a
    // smooth onset just above `threshold`, avoiding a hard flicker edge.
    float br   = max(c.r, max(c.g, c.b));
    float kk   = max(pc.knee, 1e-4);
    float soft = clamp(br - pc.threshold + kk, 0.0, 2.0 * kk);
    soft       = (soft * soft) / (4.0 * kk + 1e-4);
    float contrib = max(soft, br - pc.threshold) / max(br, 1e-4);

    out_color = vec4(c * max(contrib, 0.0), 1.0);
}

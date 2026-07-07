#version 450

// dof.frag — depth-of-field post-process (circle-of-confusion disk blur).
//
// Fullscreen pass (uses present.vert, no vertex buffer). Samples the HDR scene
// color + the scene depth, computes a per-pixel circle of confusion (CoC) from
// linearized depth relative to a focus plane, and blurs by a disk scaled to the
// CoC. Pixels inside the focus band pass through untouched (cheap early-out).
//
// Wiring (see SHADERS_DOF_r96.md): slot it into the gated post-FX chain
// (UNENDLICH_POSTFX) AFTER bloom composite, BEFORE present/tonemap. Descriptor
// set 0: binding 0 = HDR scene color, binding 1 = scene depth. Push constants are
// 32 bytes (well under the 128-byte floor). Same fullscreen-triangle draw as bloom.

layout(location = 0) in vec2 v_uv;
layout(location = 0) out vec4 out_color;

layout(set = 0, binding = 0) uniform sampler2D scene; // HDR scene color
layout(set = 0, binding = 1) uniform sampler2D depth; // scene depth attachment

layout(push_constant) uniform Pc {
    vec2 texel;        // 1/size (x,y)
    float focus_dist;  // linear world distance that is perfectly in focus
    float focus_range; // half-width (world units) of the in-focus band
    float max_coc;     // maximum blur radius, in texels
    float near;        // camera near plane
    float far;         // camera far plane
    float reverse_z;   // 1.0 if the depth buffer is reversed-Z, else 0.0
} pc;

// hardware depth sample -> linear world distance
float linearize(float d) {
    if (pc.reverse_z > 0.5) {
        d = 1.0 - d;
    }
    float z_ndc = d * 2.0 - 1.0;
    return (2.0 * pc.near * pc.far) / (pc.far + pc.near - z_ndc * (pc.far - pc.near));
}

// 16-tap unit disk (roughly even coverage) for the bokeh blur
const int N = 16;
const vec2 DISK[16] = vec2[](
    vec2( 0.000,  0.000), vec2( 0.540,  0.000), vec2( 0.170,  0.513),
    vec2(-0.437,  0.317), vec2(-0.437, -0.317), vec2( 0.170, -0.513),
    vec2( 0.908,  0.295), vec2( 0.000,  0.954), vec2(-0.908,  0.295),
    vec2(-0.561, -0.772), vec2( 0.561, -0.772), vec2( 0.281,  0.866),
    vec2(-0.736,  0.535), vec2(-0.736, -0.535), vec2( 0.281, -0.866),
    vec2( 1.000,  0.000)
);

void main() {
    float lin = linearize(texture(depth, v_uv).r);
    float coc = clamp(
        (abs(lin - pc.focus_dist) - pc.focus_range) / max(pc.focus_range, 1e-3),
        0.0, 1.0) * pc.max_coc;

    if (coc < 0.5) {
        out_color = texture(scene, v_uv); // in focus: passthrough (no blur cost)
        return;
    }

    vec3 sum = vec3(0.0);
    for (int i = 0; i < N; ++i) {
        vec2 off = DISK[i] * coc * pc.texel;
        sum += texture(scene, v_uv + off).rgb;
    }
    out_color = vec4(sum / float(N), 1.0);
}

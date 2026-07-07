#version 450
// chunk_gbuffer.frag — the G-buffer producer for the neural deferred shading path.
//
// A trimmed MRT variant of chunk.frag: it reuses the SAME vertex shader
// (chunk.vert, which already passes v_color @0 and v_world_pos @1) and emits the
// three channels the neural feature pass consumes, instead of doing the forward
// lighting. It carries NO descriptor sets (no UBO/shadow/textures) — the only
// per-fragment inputs are the two interpolated varyings, so it is cheap and
// self-contained.
//
// This is INERT until wired: it needs a render pass + framebuffer with these three
// color attachments and a pipeline that reuses chunk.vert's vertex input. See
// neural_shading_integration.md §2 for the attachment wiring; nothing in r300
// constructs a pipeline from this yet, so adding it cannot disturb the working
// forward path (which keeps using chunk.frag).
//
// === G-BUFFER ENCODING CONTRACT (the trainer MUST match this; BUGWATCH B2) ===
//   location 0  out_gnormal   vec4(n, 0)      axis-snapped face normal, components in {-1,0,1}
//   location 1  out_galbedo   vec4(albedo, 1) base albedo, LINEAR (here = v_color)
//   location 2  out_gworldpos vec4(wpos, 1)   ABSOLUTE world position, emitted DIRECT (B1)
//
// B1: out_gworldpos is the most sensitive channel (it is the hint-ray origin) — it
// is written directly from the interpolated world position, never reconstructed
// from depth, and its attachment wants full fp32 (R32G32B32A32_SFLOAT); the other
// two tolerate fp16 (R16G16B16A16_SFLOAT). The normal here is the GEOMETRIC face
// normal (matching chunk.frag's pre-perturb `normal`), which is what orients the
// AO/shadow hemisphere — NOT the bump-perturbed shading normal.

layout(location = 0) in vec3 v_color;
layout(location = 1) in vec3 v_world_pos;

layout(location = 0) out vec4 out_gnormal;
layout(location = 1) out vec4 out_galbedo;
layout(location = 2) out vec4 out_gworldpos;

void main() {
    // Axis-snapped face normal — the same derivation chunk.frag uses for its
    // pre-perturb `normal` (screen-space derivatives of world position, snapped to
    // the dominant axis), so the G-buffer normal matches the forward geometry.
    vec3 dx = dFdx(v_world_pos);
    vec3 dy = dFdy(v_world_pos);
    vec3 rn = normalize(cross(dx, dy));
    vec3 a = abs(rn);
    vec3 normal;
    if (a.x >= a.y && a.x >= a.z) {
        normal = vec3(sign(rn.x), 0.0, 0.0);
    } else if (a.y >= a.z) {
        normal = vec3(0.0, sign(rn.y), 0.0);
    } else {
        normal = vec3(0.0, 0.0, sign(rn.z));
    }

    out_gnormal = vec4(normal, 0.0);
    // Base albedo. chunk.frag grains this with an fbm `detail` term; the neural net
    // only needs train==emit (B2), so the clean base vertex color is sufficient.
    // To make the G-buffer albedo pixel-identical to the forward image, replicate
    // chunk.frag's `albedo = v_color * detail` here (and train on that).
    out_galbedo = vec4(v_color, 1.0);
    out_gworldpos = vec4(v_world_pos, 1.0); // DIRECT — never from depth (B1)
}

// shader.wgsl — CADscope's single lit pass. Lineage: skydribble's object path (dynamic-offset
// 256-byte Obj uniform, srgb target, Depth32Float) crossed with logscope gpuscene's look
// (hemispherical sun + fresnel rim + distance fog) so the viewport reads like the family.
// params.x selects a mode: 0 = solid part, 1 = ground slab (adds a world-XZ grid).
// params.w carries pick_id + 1 (skydribble's "rt self id + 1" slot, repurposed verbatim);
// misc.x carries selected_id + 1 — equality tints the selection amber.

struct Globals {
    view_proj: mat4x4<f32>,
    inv_view_proj: mat4x4<f32>,
    cam: vec4<f32>,   // xyz eye, w time (s)
    sun: vec4<f32>,   // xyz TO-sun dir (normalized on CPU), w intensity
    fog: vec4<f32>,   // rgb sky/fog colour, w density
    misc: vec4<f32>,  // x selected_id+1, yzw spare
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Obj {
    model: mat4x4<f32>,
    nrm: mat4x4<f32>,
    color: vec4<f32>,
    params: vec4<f32>, // x mode, y emissive, z spec, w pick_id+1
    _pad: array<vec4<f32>, 6>,
};
@group(1) @binding(0) var<uniform> obj: Obj;

struct VsOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
};

@vertex
fn vs_main(
    @location(0) pos: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) uv: vec2<f32>,
) -> VsOut {
    var out: VsOut;
    let world = obj.model * vec4<f32>(pos, 1.0);
    out.world = world.xyz;
    out.nrm = (obj.nrm * vec4<f32>(nrm, 0.0)).xyz;
    out.uv = uv;
    out.clip = g.view_proj * world;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let n = normalize(in.nrm);
    let v = normalize(g.cam.xyz - in.world);
    let s = normalize(g.sun.xyz);

    // hemispherical ambient: cool floor bounce below, sky above (gpuscene's sky feel)
    let hemi = mix(vec3<f32>(0.30, 0.32, 0.36), vec3<f32>(0.55, 0.59, 0.66), n.y * 0.5 + 0.5);
    let lam = max(dot(n, s), 0.0) * g.sun.w;
    let h = normalize(s + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * obj.params.z;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.22;

    var col = obj.color.rgb * (hemi + vec3<f32>(lam)) + vec3<f32>(spec + rim);
    col += obj.color.rgb * obj.params.y; // emissive

    // ground slab mode: minor/major world grid, machinist's mat
    if (obj.params.x > 0.5) {
        let cell = 5.0;
        let q = abs(fract(in.world.xz / cell + vec2<f32>(0.5)) - vec2<f32>(0.5)) * cell;
        let minor = 1.0 - smoothstep(0.0, 0.06, min(q.x, q.y));
        let q2 = abs(fract(in.world.xz / (cell * 5.0) + vec2<f32>(0.5)) - vec2<f32>(0.5)) * cell * 5.0;
        let major = 1.0 - smoothstep(0.0, 0.12, min(q2.x, q2.y));
        col = mix(col, col * 0.70, clamp(minor * 0.6 + major, 0.0, 1.0));
    }

    // selection: steady amber lift with a slow breath, driven by cam.w time
    if (g.misc.x > 0.5 && abs(obj.params.w - g.misc.x) < 0.5) {
        let breathe = 0.32 + 0.08 * sin(g.cam.w * 4.0);
        col = mix(col, vec3<f32>(1.00, 0.72, 0.28), breathe);
    }

    // distance fog toward the sky colour
    let d = length(g.cam.xyz - in.world);
    let f = clamp(1.0 - exp(-d * g.fog.w), 0.0, 1.0);
    col = mix(col, g.fog.rgb, f);

    return vec4<f32>(col, obj.color.a);
}

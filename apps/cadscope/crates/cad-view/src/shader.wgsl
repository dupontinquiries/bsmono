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
    misc: vec4<f32>,  // x selected_id+1, y texture scale, z normal-map strength, w spare
};
@group(0) @binding(0) var<uniform> g: Globals;

struct Obj {
    model: mat4x4<f32>,
    nrm: mat4x4<f32>,
    color: vec4<f32>,
    params: vec4<f32>, // x mode, y material layer (<0 = none), z spec, w pick_id+1
    _pad: array<vec4<f32>, 6>,
};
@group(1) @binding(0) var<uniform> obj: Obj;

// The material atlas. One 2D array texture, one layer per loaded material, so switching a part's
// material is an index in the object uniform rather than a rebind — the whole scene stays one
// draw call per mesh. params.y is the layer; a negative value means "no material, use flat colour",
// which is what every part gets until one is assigned.
@group(2) @binding(0) var mat_tex: texture_2d_array<f32>;
@group(2) @binding(1) var mat_samp: sampler;
// The normal and specular maps that ride beside every albedo in the engine's convention. Same
// layer indexing, so one material is one index across all three.
@group(2) @binding(2) var norm_tex: texture_2d_array<f32>;
@group(2) @binding(3) var spec_tex: texture_2d_array<f32>;

// Triplanar projection, matching cad-tex's CPU sample_triplanar exactly: abs(normal) raised to a
// sharpness, normalised to weights, three lookups blended. Keeping the two implementations in step
// is what lets a bake preview in the viewport and then export to the same pixels.
fn tri_weights(n: vec3<f32>) -> vec3<f32> {
    let w = pow(abs(n), vec3<f32>(4.0));
    let sum = w.x + w.y + w.z;
    return select(vec3<f32>(0.0, 1.0, 0.0), w / sum, sum > 1e-6);
}

fn triplanar(world: vec3<f32>, n: vec3<f32>, layer: i32, scale: f32) -> vec3<f32> {
    let w = tri_weights(n);
    let s = world * scale;
    let cx = textureSample(mat_tex, mat_samp, vec2<f32>(s.z, s.y), layer).rgb;
    let cy = textureSample(mat_tex, mat_samp, vec2<f32>(s.x, s.z), layer).rgb;
    let cz = textureSample(mat_tex, mat_samp, vec2<f32>(s.x, s.y), layer).rgb;
    return cx * w.x + cy * w.y + cz * w.z;
}

/// Perturb the surface normal by the material's normal map, blended the triplanar way.
///
/// This is "whiteout" blending: each projection's tangent-space normal is added to the geometric
/// normal in that projection's own frame, rather than being decoded into a shared tangent basis.
/// An f-rep surface has no tangent basis to share — there are no UVs and therefore no consistent
/// tangent direction — so the usual TBN construction has nothing to build from. Whiteout is the
/// standard answer, and it is cheap: swizzle each sample into its plane and sum.
fn triplanar_normal(world: vec3<f32>, n: vec3<f32>, layer: i32, scale: f32, strength: f32) -> vec3<f32> {
    let w = tri_weights(n);
    let s = world * scale;
    // Decode from [0,1] to [-1,1].
    let nx = textureSample(norm_tex, mat_samp, vec2<f32>(s.z, s.y), layer).rgb * 2.0 - 1.0;
    let ny = textureSample(norm_tex, mat_samp, vec2<f32>(s.x, s.z), layer).rgb * 2.0 - 1.0;
    let nz = textureSample(norm_tex, mat_samp, vec2<f32>(s.x, s.y), layer).rgb * 2.0 - 1.0;
    // Each map's xy is a slope in its own plane; z points along that plane's axis, signed so a
    // back-facing projection perturbs the right way.
    let sign_n = sign(n);
    let px = vec3<f32>(nx.z * sign_n.x, nx.y, nx.x);
    let py = vec3<f32>(ny.x, ny.z * sign_n.y, ny.y);
    let pz = vec3<f32>(nz.x, nz.y, nz.z * sign_n.z);
    let perturbed = px * w.x + py * w.y + pz * w.z;
    return normalize(mix(n, normalize(perturbed), clamp(strength, 0.0, 1.0)));
}

fn triplanar_spec(world: vec3<f32>, n: vec3<f32>, layer: i32, scale: f32) -> f32 {
    let w = tri_weights(n);
    let s = world * scale;
    let cx = textureSample(spec_tex, mat_samp, vec2<f32>(s.z, s.y), layer).r;
    let cy = textureSample(spec_tex, mat_samp, vec2<f32>(s.x, s.z), layer).r;
    let cz = textureSample(spec_tex, mat_samp, vec2<f32>(s.x, s.y), layer).r;
    return cx * w.x + cy * w.y + cz * w.z;
}

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
    var n = normalize(in.nrm);
    // A material perturbs the geometric normal before any lighting is computed, which is what
    // makes a texture read as relief rather than as paint on a smooth surface.
    var spec_mask = 1.0;
    if (obj.params.y >= 0.0) {
        n = triplanar_normal(in.world, n, i32(obj.params.y), g.misc.y, g.misc.z);
        spec_mask = triplanar_spec(in.world, n, i32(obj.params.y), g.misc.y);
    }
    let v = normalize(g.cam.xyz - in.world);
    let s = normalize(g.sun.xyz);

    // hemispherical ambient: cool floor bounce below, sky above (gpuscene's sky feel)
    let hemi = mix(vec3<f32>(0.30, 0.32, 0.36), vec3<f32>(0.55, 0.59, 0.66), n.y * 0.5 + 0.5);
    let lam = max(dot(n, s), 0.0) * g.sun.w;
    let h = normalize(s + v);
    let spec = pow(max(dot(n, h), 0.0), 48.0) * obj.params.z * spec_mask;
    let rim = pow(1.0 - max(dot(n, v), 0.0), 3.0) * 0.22;

    // A material replaces the flat tint, still multiplied by the part's colour so a material can
    // be shaded per part without regenerating it.
    var albedo = obj.color.rgb;
    if (obj.params.y >= 0.0) {
        albedo = albedo * triplanar(in.world, n, i32(obj.params.y), g.misc.y) * 2.0;
    }
    var col = albedo * (hemi + vec3<f32>(lam)) + vec3<f32>(spec + rim);

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

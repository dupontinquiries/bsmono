// paper.wgsl — the Foldworks look, on the GPU.
//
// The style is not negotiable and the shader is written to protect it:
//
//  * HARD FACETS. Normals arrive PER FACE from SceneBuffers (every triangle's
//    three vertices carry that triangle's plane normal) and are used as-is —
//    no interpolated smoothing, no normal maps. A folded sheet has creases,
//    not gradients, so a flat facet must stay flat.
//  * CANDLELIT DESK. One warm key low and to the left (the candle), one cool
//    dim fill from the opposite side (the room), and a rim term that lifts
//    silhouette edges the way lamplight catches a paper edge. Lighting is
//    quantized into a few bands, because paint on paper reads as steps.
//  * PAPER GRAIN. A cheap value-noise fibre modulates albedo very slightly and
//    is anchored to WORLD space, so the grain belongs to the sheet rather than
//    swimming across it as the camera moves.
//  * INK EDGES. Facets whose normal turns away from the eye darken toward the
//    ink colour, which is what gives the software renderer its drawn outline.
//
// Kept deliberately in one file with no bind-group churn: a frame is a camera
// uniform and one draw per mesh.

struct Camera {
    view_proj : mat4x4<f32>,
    eye       : vec4<f32>,   // xyz + unused
    key_dir   : vec4<f32>,   // xyz normalized + intensity
    key_col   : vec4<f32>,   // rgb + ambient amount
    fill_col  : vec4<f32>,   // rgb + rim amount
    ink_col   : vec4<f32>,   // rgb + grain amount
};

@group(0) @binding(0) var<uniform> cam : Camera;

struct VsIn {
    @location(0) pos : vec3<f32>,
    @location(1) nrm : vec3<f32>,
    @location(2) col : vec3<f32>,
};

struct VsOut {
    @builtin(position) clip : vec4<f32>,
    @location(0) world : vec3<f32>,
    @location(1) nrm   : vec3<f32>,
    @location(2) col   : vec3<f32>,
};

@vertex
fn vs_main(in : VsIn) -> VsOut {
    var out : VsOut;
    out.world = in.pos;
    out.nrm   = in.nrm;
    out.col   = in.col;
    out.clip  = cam.view_proj * vec4<f32>(in.pos, 1.0);
    return out;
}

// value noise — cheap, deterministic, world-anchored
fn hash3(p : vec3<f32>) -> f32 {
    let q = fract(p * 0.3183099 + vec3<f32>(0.1, 0.2, 0.3));
    let r = q * 17.0;
    return fract(r.x * r.y * r.z * (r.x + r.y + r.z));
}

fn fibre(p : vec3<f32>) -> f32 {
    // stretched along x so it reads as laid paper fibre, not TV static
    let s = vec3<f32>(p.x * 3.0, p.y * 26.0, p.z * 26.0);
    let i = floor(s);
    let f = fract(s);
    let w = f * f * (3.0 - 2.0 * f);
    let a = mix(hash3(i + vec3<f32>(0.0, 0.0, 0.0)), hash3(i + vec3<f32>(1.0, 0.0, 0.0)), w.x);
    let b = mix(hash3(i + vec3<f32>(0.0, 1.0, 0.0)), hash3(i + vec3<f32>(1.0, 1.0, 0.0)), w.x);
    let c = mix(hash3(i + vec3<f32>(0.0, 0.0, 1.0)), hash3(i + vec3<f32>(1.0, 0.0, 1.0)), w.x);
    let d = mix(hash3(i + vec3<f32>(0.0, 1.0, 1.0)), hash3(i + vec3<f32>(1.0, 1.0, 1.0)), w.x);
    return mix(mix(a, b, w.y), mix(c, d, w.y), w.z);
}

// quantize a lambert term into paper-paint bands
fn band(x : f32) -> f32 {
    let steps = 4.0;
    return floor(clamp(x, 0.0, 1.0) * steps + 0.5) / steps;
}

@fragment
fn fs_main(in : VsOut) -> @location(0) vec4<f32> {
    // The facet normal is used RAW: no normalize-of-interpolated smoothing that
    // would round a crease off. (It arrives unit-length from the CPU side.)
    let n = in.nrm;
    let v = normalize(cam.eye.xyz - in.world);

    let key_dir = normalize(cam.key_dir.xyz);
    let key = band(max(dot(n, key_dir), 0.0)) * cam.key_dir.w;
    let fill = max(dot(n, -key_dir), 0.0) * 0.5;
    let amb = cam.key_col.w;

    var lit = in.col * (amb + key * cam.key_col.rgb + fill * cam.fill_col.rgb);

    // rim: a paper edge catching the candle
    let rim = pow(1.0 - clamp(dot(n, v), 0.0, 1.0), 3.0) * cam.fill_col.w;
    lit = lit + cam.key_col.rgb * rim;

    // ink edge: facets turning away from the eye sink toward ink
    let facing = clamp(dot(n, v), 0.0, 1.0);
    lit = mix(cam.ink_col.rgb, lit, clamp(facing * 1.35 + 0.25, 0.0, 1.0));

    // paper fibre, world-anchored so it stays ON the sheet
    let g = (fibre(in.world) - 0.5) * cam.ink_col.w;
    lit = lit * (1.0 + g);

    return vec4<f32>(clamp(lit, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}

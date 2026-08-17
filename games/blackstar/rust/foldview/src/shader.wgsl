struct Uniforms {
    view_proj: mat4x4<f32>,
    eye: vec4<f32>,
};
@group(0) @binding(0) var<uniform> u: Uniforms;

struct VOut {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) nrm: vec3<f32>,
    @location(2) tint: vec3<f32>,
};

@vertex
fn vs(@location(0) pos: vec3<f32>, @location(1) nrm: vec3<f32>, @location(2) tint: vec3<f32>) -> VOut {
    var o: VOut;
    o.clip = u.view_proj * vec4<f32>(pos, 1.0);
    o.world = pos;
    o.nrm = nrm;
    o.tint = tint;
    return o;
}

@fragment
fn fs(v: VOut) -> @location(0) vec4<f32> {
    let n = normalize(v.nrm);
    let l1 = normalize(vec3<f32>(0.6, 0.8, 0.4));
    let l2 = normalize(vec3<f32>(-0.5, 0.2, -0.8));
    let view = normalize(u.eye.xyz - v.world);
    let lit = 0.18 + 0.62 * max(dot(n, l1), 0.0) + 0.25 * max(dot(n, l2), 0.0);
    let rim = pow(1.0 - abs(dot(n, view)), 3.0) * 0.55;
    return vec4<f32>(min(v.tint * lit + vec3<f32>(rim), vec3<f32>(1.0)), 1.0);
}

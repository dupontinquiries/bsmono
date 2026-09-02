// dst[i] = dst[i] * scale + src[i], 4 elements per thread.
struct Params { n: u32, _p0: u32, _p1: u32, _p2: u32, scale: f32, _p3: f32, _p4: f32, _p5: f32, };
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> dst: array<f32>;
@group(0) @binding(2) var<storage, read_write> src: array<f32>;
@compute @workgroup_size(256, 1, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let i0 = gid.x * 4u;
    for (var k = 0u; k < 4u; k = k + 1u) {
        let i = i0 + k;
        if (i < p.n) {
            dst[i] = dst[i] * p.scale + src[i];
        }
    }
}

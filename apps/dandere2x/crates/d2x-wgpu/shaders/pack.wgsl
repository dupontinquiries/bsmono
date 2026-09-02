// Network output planes -> packed RGBA8 at the tile's output resolution:
// pixel-shuffle by r, optional nearest-upsampled input skip, clamp, pack.
//   planes  [3*r*r][in_h][in_w]  f32        (the network's output buffer)
//   skip    [3*u*u][sk_h][sk_w]  f32        (the network's input buffer, pixel-unshuffled by u)
//   out     [out_h][out_w]       u32 RGBA8  with out_w = in_w*r
struct Params {
    in_w: u32, in_h: u32, r: u32, s: u32,
    u: u32, sk_w: u32, sk_h: u32, add_input: u32,
};
@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> planes: array<f32>;
@group(0) @binding(2) var<storage, read_write> skip: array<f32>;
@group(0) @binding(3) var<storage, read_write> out: array<u32>;
@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let x = gid.x;
    let y = gid.y;
    let out_w = p.in_w * p.r;
    let out_h = p.in_h * p.r;
    if (x >= out_w || y >= out_h) {
        return;
    }
    let in_plane = p.in_w * p.in_h;
    let sk_plane = p.sk_w * p.sk_h;
    var rgb = vec3<f32>(0.0);
    for (var c = 0u; c < 3u; c = c + 1u) {
        let ch = c * p.r * p.r + (y % p.r) * p.r + (x % p.r);
        var v = planes[ch * in_plane + (y / p.r) * p.in_w + (x / p.r)];
        if (p.add_input == 1u) {
            let iy = y / p.s;
            let ix = x / p.s;
            let ich = c * p.u * p.u + (iy % p.u) * p.u + (ix % p.u);
            v = v + skip[ich * sk_plane + (iy / p.u) * p.sk_w + (ix / p.u)];
        }
        if (c == 0u) { rgb.x = v; } else if (c == 1u) { rgb.y = v; } else { rgb.z = v; }
    }
    out[y * out_w + x] = pack4x8unorm(vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

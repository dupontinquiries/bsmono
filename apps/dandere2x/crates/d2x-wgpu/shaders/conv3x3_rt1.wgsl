// 3x3 convolution over planar f32 channels, same or valid padding, with the
// source optionally read as nearest-upsampled x2 (in_scale), a fused
// residual (out = acc*res_scale + residual), and a fused activation
// (none / LeakyReLU / per-channel PReLU). Writes channels
// [dst_off, dst_off+out_ch) of the output buffer, so dense concatenation is
// a channel offset and never a copy.
//
// Minimum-register variant: one output pixel x 4 channels per thread, one
// vec4 accumulator live. 9 FMAs per 9 loads per input channel — the worst
// arithmetic intensity of the three kernels and the best occupancy. On AGX,
// where occupancy falls off a cliff past ~64 registers, that trade can win;
// `dandere2x bench` decides per device.
//
// Layouts:
//   input     [ic][y][x]            f32   (read_write: a dense block reads and
//                                          writes disjoint ranges of one buffer)
//   weights   [oc/4][ic][ky][kx]    vec4
//   bias      [oc/4]                vec4
//   output    [oc][y][x]            f32
//   prelu     [oc/4]                vec4  (slopes; only read when act == 2)
//   residual  [oc][y][x]            f32   (only read when has_res == 1)

struct Params {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_ch4: u32,
    out_w: u32,
    out_h: u32,
    in_scale: u32,
    pad: u32,
    dst_off: u32,
    act: u32,
    has_res: u32,
    _p0: u32,
    slope: f32,
    res_scale: f32,
    _p1: f32,
    _p2: f32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> input: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> bias: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> output: array<f32>;
@group(0) @binding(5) var<storage, read> prelu: array<vec4<f32>>;
@group(0) @binding(6) var<storage, read_write> residual: array<f32>;

// Source sample at virtual (upsampled) coordinates; zero outside.
fn load(ic_base: u32, sy: i32, sx: i32) -> f32 {
    let vw = i32(p.in_w * p.in_scale);
    let vh = i32(p.in_h * p.in_scale);
    if (sx < 0 || sy < 0 || sx >= vw || sy >= vh) {
        return 0.0;
    }
    var x = u32(sx);
    var y = u32(sy);
    if (p.in_scale == 2u) {
        x = x / 2u;
        y = y / 2u;
    }
    return input[ic_base + y * p.in_w + x];
}

fn epilogue(acc: vec4<f32>, oc4: u32, idx: u32, out_plane: u32) -> vec4<f32> {
    var v = acc;
    if (p.has_res == 1u) {
        let rb = (oc4 * 4u) * out_plane + idx;
        let r = vec4<f32>(residual[rb], residual[rb + out_plane], residual[rb + 2u * out_plane], residual[rb + 3u * out_plane]);
        v = v * p.res_scale + r;
    }
    if (p.act == 1u) {
        v = select(v * p.slope, v, v >= vec4<f32>(0.0));
    } else if (p.act == 2u) {
        v = select(v * prelu[oc4], v, v >= vec4<f32>(0.0));
    }
    return v;
}

fn store4(base: u32, plane: u32, v: vec4<f32>) {
    output[base] = v.x;
    output[base + plane] = v.y;
    output[base + 2u * plane] = v.z;
    output[base + 3u * plane] = v.w;
}

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let oy = gid.y;
    let oc4 = gid.z;
    if (ox >= p.out_w || oy >= p.out_h || oc4 >= p.out_ch4) {
        return;
    }
    var acc = bias[oc4];
    let in_plane = p.in_w * p.in_h;
    var wi = oc4 * p.in_ch * 9u;
    let sx0 = i32(ox) - i32(p.pad);
    let sy0 = i32(oy) - i32(p.pad);
    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let ib = ic * in_plane;
        acc = acc + weights[wi + 0u] * load(ib, sy0 + 0, sx0 + 0);
        acc = acc + weights[wi + 1u] * load(ib, sy0 + 0, sx0 + 1);
        acc = acc + weights[wi + 2u] * load(ib, sy0 + 0, sx0 + 2);
        acc = acc + weights[wi + 3u] * load(ib, sy0 + 1, sx0 + 0);
        acc = acc + weights[wi + 4u] * load(ib, sy0 + 1, sx0 + 1);
        acc = acc + weights[wi + 5u] * load(ib, sy0 + 1, sx0 + 2);
        acc = acc + weights[wi + 6u] * load(ib, sy0 + 2, sx0 + 0);
        acc = acc + weights[wi + 7u] * load(ib, sy0 + 2, sx0 + 1);
        acc = acc + weights[wi + 8u] * load(ib, sy0 + 2, sx0 + 2);
        wi = wi + 9u;
    }
    let out_plane = p.out_w * p.out_h;
    let idx = oy * p.out_w + ox;
    store4((p.dst_off + oc4 * 4u) * out_plane + idx, out_plane, epilogue(acc, oc4, idx, out_plane));
}

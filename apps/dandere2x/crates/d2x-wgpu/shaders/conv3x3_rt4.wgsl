// 3x3 convolution over planar f32 channels, same or valid padding, with the
// source optionally read as nearest-upsampled x2 (in_scale), a fused
// residual (out = acc*res_scale + residual), and a fused activation
// (none / LeakyReLU / per-channel PReLU). Writes channels
// [dst_off, dst_off+out_ch) of the output buffer, so dense concatenation is
// a channel offset and never a copy.
//
// Register-tiled variant: each invocation produces a 4x4 patch of output
// pixels for 4 output channels. Per input channel it loads a 6x6 input
// patch (36 scalars) and 9 vec4 weights and does 144 vec4 FMAs — about
// 2.9x the arithmetic per byte of the 2x2 kernel, at ~130 live registers.
// Workgroup 8x8 threads = 32x32 output pixels, so small images still fill
// several workgroups. Same layouts, params and epilogue as conv3x3.wgsl.
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

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x * 4u;
    let oy = gid.y * 4u;
    let oc4 = gid.z;
    if (ox >= p.out_w || oy >= p.out_h || oc4 >= p.out_ch4) {
        return;
    }
    let b = bias[oc4];
    var a00 = b;
    var a01 = b;
    var a02 = b;
    var a03 = b;
    var a10 = b;
    var a11 = b;
    var a12 = b;
    var a13 = b;
    var a20 = b;
    var a21 = b;
    var a22 = b;
    var a23 = b;
    var a30 = b;
    var a31 = b;
    var a32 = b;
    var a33 = b;
    let in_plane = p.in_w * p.in_h;
    var wi = oc4 * p.in_ch * 9u;
    let sx0 = i32(ox) - i32(p.pad);
    let sy0 = i32(oy) - i32(p.pad);
    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let ib = ic * in_plane;
        let i00 = load(ib, sy0 + 0, sx0 + 0); let i01 = load(ib, sy0 + 0, sx0 + 1); let i02 = load(ib, sy0 + 0, sx0 + 2); let i03 = load(ib, sy0 + 0, sx0 + 3); let i04 = load(ib, sy0 + 0, sx0 + 4); let i05 = load(ib, sy0 + 0, sx0 + 5);
        let i10 = load(ib, sy0 + 1, sx0 + 0); let i11 = load(ib, sy0 + 1, sx0 + 1); let i12 = load(ib, sy0 + 1, sx0 + 2); let i13 = load(ib, sy0 + 1, sx0 + 3); let i14 = load(ib, sy0 + 1, sx0 + 4); let i15 = load(ib, sy0 + 1, sx0 + 5);
        let i20 = load(ib, sy0 + 2, sx0 + 0); let i21 = load(ib, sy0 + 2, sx0 + 1); let i22 = load(ib, sy0 + 2, sx0 + 2); let i23 = load(ib, sy0 + 2, sx0 + 3); let i24 = load(ib, sy0 + 2, sx0 + 4); let i25 = load(ib, sy0 + 2, sx0 + 5);
        let i30 = load(ib, sy0 + 3, sx0 + 0); let i31 = load(ib, sy0 + 3, sx0 + 1); let i32 = load(ib, sy0 + 3, sx0 + 2); let i33 = load(ib, sy0 + 3, sx0 + 3); let i34 = load(ib, sy0 + 3, sx0 + 4); let i35 = load(ib, sy0 + 3, sx0 + 5);
        let i40 = load(ib, sy0 + 4, sx0 + 0); let i41 = load(ib, sy0 + 4, sx0 + 1); let i42 = load(ib, sy0 + 4, sx0 + 2); let i43 = load(ib, sy0 + 4, sx0 + 3); let i44 = load(ib, sy0 + 4, sx0 + 4); let i45 = load(ib, sy0 + 4, sx0 + 5);
        let i50 = load(ib, sy0 + 5, sx0 + 0); let i51 = load(ib, sy0 + 5, sx0 + 1); let i52 = load(ib, sy0 + 5, sx0 + 2); let i53 = load(ib, sy0 + 5, sx0 + 3); let i54 = load(ib, sy0 + 5, sx0 + 4); let i55 = load(ib, sy0 + 5, sx0 + 5);
        let w00 = weights[wi + 0u]; let w01 = weights[wi + 1u]; let w02 = weights[wi + 2u]; let w10 = weights[wi + 3u]; let w11 = weights[wi + 4u]; let w12 = weights[wi + 5u]; let w20 = weights[wi + 6u]; let w21 = weights[wi + 7u]; let w22 = weights[wi + 8u];
        wi = wi + 9u;
        a00 = a00 + w00 * i00 + w01 * i01 + w02 * i02 + w10 * i10 + w11 * i11 + w12 * i12 + w20 * i20 + w21 * i21 + w22 * i22;
        a01 = a01 + w00 * i01 + w01 * i02 + w02 * i03 + w10 * i11 + w11 * i12 + w12 * i13 + w20 * i21 + w21 * i22 + w22 * i23;
        a02 = a02 + w00 * i02 + w01 * i03 + w02 * i04 + w10 * i12 + w11 * i13 + w12 * i14 + w20 * i22 + w21 * i23 + w22 * i24;
        a03 = a03 + w00 * i03 + w01 * i04 + w02 * i05 + w10 * i13 + w11 * i14 + w12 * i15 + w20 * i23 + w21 * i24 + w22 * i25;
        a10 = a10 + w00 * i10 + w01 * i11 + w02 * i12 + w10 * i20 + w11 * i21 + w12 * i22 + w20 * i30 + w21 * i31 + w22 * i32;
        a11 = a11 + w00 * i11 + w01 * i12 + w02 * i13 + w10 * i21 + w11 * i22 + w12 * i23 + w20 * i31 + w21 * i32 + w22 * i33;
        a12 = a12 + w00 * i12 + w01 * i13 + w02 * i14 + w10 * i22 + w11 * i23 + w12 * i24 + w20 * i32 + w21 * i33 + w22 * i34;
        a13 = a13 + w00 * i13 + w01 * i14 + w02 * i15 + w10 * i23 + w11 * i24 + w12 * i25 + w20 * i33 + w21 * i34 + w22 * i35;
        a20 = a20 + w00 * i20 + w01 * i21 + w02 * i22 + w10 * i30 + w11 * i31 + w12 * i32 + w20 * i40 + w21 * i41 + w22 * i42;
        a21 = a21 + w00 * i21 + w01 * i22 + w02 * i23 + w10 * i31 + w11 * i32 + w12 * i33 + w20 * i41 + w21 * i42 + w22 * i43;
        a22 = a22 + w00 * i22 + w01 * i23 + w02 * i24 + w10 * i32 + w11 * i33 + w12 * i34 + w20 * i42 + w21 * i43 + w22 * i44;
        a23 = a23 + w00 * i23 + w01 * i24 + w02 * i25 + w10 * i33 + w11 * i34 + w12 * i35 + w20 * i43 + w21 * i44 + w22 * i45;
        a30 = a30 + w00 * i30 + w01 * i31 + w02 * i32 + w10 * i40 + w11 * i41 + w12 * i42 + w20 * i50 + w21 * i51 + w22 * i52;
        a31 = a31 + w00 * i31 + w01 * i32 + w02 * i33 + w10 * i41 + w11 * i42 + w12 * i43 + w20 * i51 + w21 * i52 + w22 * i53;
        a32 = a32 + w00 * i32 + w01 * i33 + w02 * i34 + w10 * i42 + w11 * i43 + w12 * i44 + w20 * i52 + w21 * i53 + w22 * i54;
        a33 = a33 + w00 * i33 + w01 * i34 + w02 * i35 + w10 * i43 + w11 * i44 + w12 * i45 + w20 * i53 + w21 * i54 + w22 * i55;
    }
    let out_plane = p.out_w * p.out_h;
    let obase = (p.dst_off + oc4 * 4u) * out_plane;
    if (oy + 0u < p.out_h) {
        if (ox + 0u < p.out_w) { let idx = (oy + 0u) * p.out_w + ox + 0u; store4(obase + idx, out_plane, epilogue(a00, oc4, idx, out_plane)); }
        if (ox + 1u < p.out_w) { let idx = (oy + 0u) * p.out_w + ox + 1u; store4(obase + idx, out_plane, epilogue(a01, oc4, idx, out_plane)); }
        if (ox + 2u < p.out_w) { let idx = (oy + 0u) * p.out_w + ox + 2u; store4(obase + idx, out_plane, epilogue(a02, oc4, idx, out_plane)); }
        if (ox + 3u < p.out_w) { let idx = (oy + 0u) * p.out_w + ox + 3u; store4(obase + idx, out_plane, epilogue(a03, oc4, idx, out_plane)); }
    }
    if (oy + 1u < p.out_h) {
        if (ox + 0u < p.out_w) { let idx = (oy + 1u) * p.out_w + ox + 0u; store4(obase + idx, out_plane, epilogue(a10, oc4, idx, out_plane)); }
        if (ox + 1u < p.out_w) { let idx = (oy + 1u) * p.out_w + ox + 1u; store4(obase + idx, out_plane, epilogue(a11, oc4, idx, out_plane)); }
        if (ox + 2u < p.out_w) { let idx = (oy + 1u) * p.out_w + ox + 2u; store4(obase + idx, out_plane, epilogue(a12, oc4, idx, out_plane)); }
        if (ox + 3u < p.out_w) { let idx = (oy + 1u) * p.out_w + ox + 3u; store4(obase + idx, out_plane, epilogue(a13, oc4, idx, out_plane)); }
    }
    if (oy + 2u < p.out_h) {
        if (ox + 0u < p.out_w) { let idx = (oy + 2u) * p.out_w + ox + 0u; store4(obase + idx, out_plane, epilogue(a20, oc4, idx, out_plane)); }
        if (ox + 1u < p.out_w) { let idx = (oy + 2u) * p.out_w + ox + 1u; store4(obase + idx, out_plane, epilogue(a21, oc4, idx, out_plane)); }
        if (ox + 2u < p.out_w) { let idx = (oy + 2u) * p.out_w + ox + 2u; store4(obase + idx, out_plane, epilogue(a22, oc4, idx, out_plane)); }
        if (ox + 3u < p.out_w) { let idx = (oy + 2u) * p.out_w + ox + 3u; store4(obase + idx, out_plane, epilogue(a23, oc4, idx, out_plane)); }
    }
    if (oy + 3u < p.out_h) {
        if (ox + 0u < p.out_w) { let idx = (oy + 3u) * p.out_w + ox + 0u; store4(obase + idx, out_plane, epilogue(a30, oc4, idx, out_plane)); }
        if (ox + 1u < p.out_w) { let idx = (oy + 3u) * p.out_w + ox + 1u; store4(obase + idx, out_plane, epilogue(a31, oc4, idx, out_plane)); }
        if (ox + 2u < p.out_w) { let idx = (oy + 3u) * p.out_w + ox + 2u; store4(obase + idx, out_plane, epilogue(a32, oc4, idx, out_plane)); }
        if (ox + 3u < p.out_w) { let idx = (oy + 3u) * p.out_w + ox + 3u; store4(obase + idx, out_plane, epilogue(a33, oc4, idx, out_plane)); }
    }
}

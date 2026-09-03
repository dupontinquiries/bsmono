// 3x3 convolution over planar f32 channels, same or valid padding, with the
// source optionally read as nearest-upsampled x2 (in_scale), a fused
// residual (out = acc*res_scale + residual), and a fused activation
// (none / LeakyReLU / per-channel PReLU). Writes channels
// [dst_off, dst_off+out_ch) of the output buffer, so dense concatenation is
// a channel offset and never a copy.
//
// rt2c8 (2x2 px x 8 channels, 988 GMAC/s on M2 Ultra) plus the interior
// fast path: the two independent wins composed. Channel blocking raises
// arithmetic per load; the fast path removes the per-scalar bounds test and
// in_scale divide. Neither costs input registers.
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
    let ox = gid.x * 2u;
    let oy = gid.y * 2u;
    let g0 = gid.z * 2u;
    if (ox >= p.out_w || oy >= p.out_h || g0 >= p.out_ch4) {
        return;
    }
    let g1 = g0 + 1u;
    let has_g1 = g1 < p.out_ch4;
    let s1 = select(g0, g1, has_g1);
    let has_x1 = (ox + 1u) < p.out_w;
    let has_y1 = (oy + 1u) < p.out_h;
    let bA = bias[g0];
    var A00 = bA;
    var A01 = bA;
    var A10 = bA;
    var A11 = bA;
    let bB = bias[s1];
    var B00 = bB;
    var B01 = bB;
    var B10 = bB;
    var B11 = bB;
    let in_plane = p.in_w * p.in_h;
    var wA = g0 * p.in_ch * 9u;
    var wB = s1 * p.in_ch * 9u;
    let sx0 = i32(ox) - i32(p.pad);
    let sy0 = i32(oy) - i32(p.pad);
    let vw = i32(p.in_w * p.in_scale);
    let vh = i32(p.in_h * p.in_scale);
    // Interior threads index precomputed row bases: no per-scalar bounds
    // test and no in_scale divide. Costs no registers.
    let interior = sx0 >= 0 && sy0 >= 0 && (sx0 + 3) < vw && (sy0 + 3) < vh && p.in_scale == 1u;
    if (interior) {
        let r0 = u32(sy0) * p.in_w + u32(sx0);
        for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
            let q0 = ic * in_plane + r0;
            let q1 = q0 + p.in_w;
            let q2 = q1 + p.in_w;
            let q3 = q2 + p.in_w;
            let i00 = input[q0]; let i01 = input[q0 + 1u]; let i02 = input[q0 + 2u]; let i03 = input[q0 + 3u];
            let i10 = input[q1]; let i11 = input[q1 + 1u]; let i12 = input[q1 + 2u]; let i13 = input[q1 + 3u];
            let i20 = input[q2]; let i21 = input[q2 + 1u]; let i22 = input[q2 + 2u]; let i23 = input[q2 + 3u];
            let i30 = input[q3]; let i31 = input[q3 + 1u]; let i32 = input[q3 + 2u]; let i33 = input[q3 + 3u];
            {
                let w00 = weights[wA + 0u]; let w01 = weights[wA + 1u]; let w02 = weights[wA + 2u]; let w10 = weights[wA + 3u]; let w11 = weights[wA + 4u]; let w12 = weights[wA + 5u]; let w20 = weights[wA + 6u]; let w21 = weights[wA + 7u]; let w22 = weights[wA + 8u];
                A00 = A00 + w00 * i00 + w01 * i01 + w02 * i02 + w10 * i10 + w11 * i11 + w12 * i12 + w20 * i20 + w21 * i21 + w22 * i22;
                A01 = A01 + w00 * i01 + w01 * i02 + w02 * i03 + w10 * i11 + w11 * i12 + w12 * i13 + w20 * i21 + w21 * i22 + w22 * i23;
                A10 = A10 + w00 * i10 + w01 * i11 + w02 * i12 + w10 * i20 + w11 * i21 + w12 * i22 + w20 * i30 + w21 * i31 + w22 * i32;
                A11 = A11 + w00 * i11 + w01 * i12 + w02 * i13 + w10 * i21 + w11 * i22 + w12 * i23 + w20 * i31 + w21 * i32 + w22 * i33;
            }
            {
                let w00 = weights[wB + 0u]; let w01 = weights[wB + 1u]; let w02 = weights[wB + 2u]; let w10 = weights[wB + 3u]; let w11 = weights[wB + 4u]; let w12 = weights[wB + 5u]; let w20 = weights[wB + 6u]; let w21 = weights[wB + 7u]; let w22 = weights[wB + 8u];
                B00 = B00 + w00 * i00 + w01 * i01 + w02 * i02 + w10 * i10 + w11 * i11 + w12 * i12 + w20 * i20 + w21 * i21 + w22 * i22;
                B01 = B01 + w00 * i01 + w01 * i02 + w02 * i03 + w10 * i11 + w11 * i12 + w12 * i13 + w20 * i21 + w21 * i22 + w22 * i23;
                B10 = B10 + w00 * i10 + w01 * i11 + w02 * i12 + w10 * i20 + w11 * i21 + w12 * i22 + w20 * i30 + w21 * i31 + w22 * i32;
                B11 = B11 + w00 * i11 + w01 * i12 + w02 * i13 + w10 * i21 + w11 * i22 + w12 * i23 + w20 * i31 + w21 * i32 + w22 * i33;
            }
            wA = wA + 9u;
            wB = wB + 9u;
        }
    } else {
        for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
            let ib = ic * in_plane;
            let i00 = load(ib, sy0 + 0, sx0 + 0); let i01 = load(ib, sy0 + 0, sx0 + 1); let i02 = load(ib, sy0 + 0, sx0 + 2); let i03 = load(ib, sy0 + 0, sx0 + 3);
            let i10 = load(ib, sy0 + 1, sx0 + 0); let i11 = load(ib, sy0 + 1, sx0 + 1); let i12 = load(ib, sy0 + 1, sx0 + 2); let i13 = load(ib, sy0 + 1, sx0 + 3);
            let i20 = load(ib, sy0 + 2, sx0 + 0); let i21 = load(ib, sy0 + 2, sx0 + 1); let i22 = load(ib, sy0 + 2, sx0 + 2); let i23 = load(ib, sy0 + 2, sx0 + 3);
            let i30 = load(ib, sy0 + 3, sx0 + 0); let i31 = load(ib, sy0 + 3, sx0 + 1); let i32 = load(ib, sy0 + 3, sx0 + 2); let i33 = load(ib, sy0 + 3, sx0 + 3);
            {
                let w00 = weights[wA + 0u]; let w01 = weights[wA + 1u]; let w02 = weights[wA + 2u]; let w10 = weights[wA + 3u]; let w11 = weights[wA + 4u]; let w12 = weights[wA + 5u]; let w20 = weights[wA + 6u]; let w21 = weights[wA + 7u]; let w22 = weights[wA + 8u];
                A00 = A00 + w00 * i00 + w01 * i01 + w02 * i02 + w10 * i10 + w11 * i11 + w12 * i12 + w20 * i20 + w21 * i21 + w22 * i22;
                A01 = A01 + w00 * i01 + w01 * i02 + w02 * i03 + w10 * i11 + w11 * i12 + w12 * i13 + w20 * i21 + w21 * i22 + w22 * i23;
                A10 = A10 + w00 * i10 + w01 * i11 + w02 * i12 + w10 * i20 + w11 * i21 + w12 * i22 + w20 * i30 + w21 * i31 + w22 * i32;
                A11 = A11 + w00 * i11 + w01 * i12 + w02 * i13 + w10 * i21 + w11 * i22 + w12 * i23 + w20 * i31 + w21 * i32 + w22 * i33;
            }
            {
                let w00 = weights[wB + 0u]; let w01 = weights[wB + 1u]; let w02 = weights[wB + 2u]; let w10 = weights[wB + 3u]; let w11 = weights[wB + 4u]; let w12 = weights[wB + 5u]; let w20 = weights[wB + 6u]; let w21 = weights[wB + 7u]; let w22 = weights[wB + 8u];
                B00 = B00 + w00 * i00 + w01 * i01 + w02 * i02 + w10 * i10 + w11 * i11 + w12 * i12 + w20 * i20 + w21 * i21 + w22 * i22;
                B01 = B01 + w00 * i01 + w01 * i02 + w02 * i03 + w10 * i11 + w11 * i12 + w12 * i13 + w20 * i21 + w21 * i22 + w22 * i23;
                B10 = B10 + w00 * i10 + w01 * i11 + w02 * i12 + w10 * i20 + w11 * i21 + w12 * i22 + w20 * i30 + w21 * i31 + w22 * i32;
                B11 = B11 + w00 * i11 + w01 * i12 + w02 * i13 + w10 * i21 + w11 * i22 + w12 * i23 + w20 * i31 + w21 * i32 + w22 * i33;
            }
            wA = wA + 9u;
            wB = wB + 9u;
        }
    }
    let out_plane = p.out_w * p.out_h;
    let idx00 = oy * p.out_w + ox;
    let baseA = (p.dst_off + g0 * 4u) * out_plane;
    store4(baseA + idx00, out_plane, epilogue(A00, g0, idx00, out_plane));
    if (has_x1) { store4(baseA + idx00 + 1u, out_plane, epilogue(A01, g0, idx00 + 1u, out_plane)); }
    if (has_y1) {
        let j10 = idx00 + p.out_w;
        store4(baseA + j10, out_plane, epilogue(A10, g0, j10, out_plane));
        if (has_x1) { store4(baseA + j10 + 1u, out_plane, epilogue(A11, g0, j10 + 1u, out_plane)); }
    }
    if (has_g1) {
        let baseB = (p.dst_off + g1 * 4u) * out_plane;
        store4(baseB + idx00, out_plane, epilogue(B00, g1, idx00, out_plane));
        if (has_x1) { store4(baseB + idx00 + 1u, out_plane, epilogue(B01, g1, idx00 + 1u, out_plane)); }
        if (has_y1) {
            let j10 = idx00 + p.out_w;
            store4(baseB + j10, out_plane, epilogue(B10, g1, j10, out_plane));
            if (has_x1) { store4(baseB + j10 + 1u, out_plane, epilogue(B11, g1, j10 + 1u, out_plane)); }
        }
    }
}

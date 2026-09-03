// 3x3 convolution over planar f32 channels, same or valid padding, with the
// source optionally read as nearest-upsampled x2 (in_scale), a fused
// residual (out = acc*res_scale + residual), and a fused activation
// (none / LeakyReLU / per-channel PReLU). Writes channels
// [dst_off, dst_off+out_ch) of the output buffer, so dense concatenation is
// a channel offset and never a copy.
//
// Hoisted addressing x 8 output channels: the two measured wins composed
// without duplicating the accumulator block.
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
    let gA0 = gid.z * 2u;
    if (ox >= p.out_w || oy >= p.out_h || gA0 >= p.out_ch4) { return; }
    let gA1 = gA0 + 1u;
    let okg1 = gA1 < p.out_ch4;
    let sg1 = select(gA0, gA1, okg1);
    let okx = (ox + 1u) < p.out_w;
    let oky = (oy + 1u) < p.out_h;
    let biasA = bias[gA0];
    var accA00 = biasA;
    var accA01 = biasA;
    var accA10 = biasA;
    var accA11 = biasA;
    let biasB = bias[sg1];
    var accB00 = biasB;
    var accB01 = biasB;
    var accB10 = biasB;
    var accB11 = biasB;
    let in_plane = p.in_w * p.in_h;
    var wpA = gA0 * p.in_ch * 9u;
    var wpB = sg1 * p.in_ch * 9u;
    let bx = i32(ox) - i32(p.pad);
    let by = i32(oy) - i32(p.pad);
    let vw = i32(p.in_w * p.in_scale);
    let vh = i32(p.in_h * p.in_scale);
    // Addressing hoisted out of the channel loop: the bounds test and the
    // in_scale divide depend only on this thread's position, yet the generic
    // load() recomputed them 16 times per input channel. Resolve each row
    // base and column offset once, with a 0/1 mask standing in for the bounds
    // test, and keep a SINGLE copy of the accumulator block — duplicating it
    // for a fast-path branch is what made rt2c8f (803) lose to both parents
    // (rt2f 1022, rt2c8 977) on the M2 Ultra.
    let cy0 = by + 0;
    let my0 = select(0.0, 1.0, cy0 >= 0 && cy0 < vh);
    let ky0 = u32(clamp(cy0, 0, vh - 1));
    let rb0 = select(ky0, ky0 / 2u, p.in_scale == 2u) * p.in_w;
    let cy1 = by + 1;
    let my1 = select(0.0, 1.0, cy1 >= 0 && cy1 < vh);
    let ky1 = u32(clamp(cy1, 0, vh - 1));
    let rb1 = select(ky1, ky1 / 2u, p.in_scale == 2u) * p.in_w;
    let cy2 = by + 2;
    let my2 = select(0.0, 1.0, cy2 >= 0 && cy2 < vh);
    let ky2 = u32(clamp(cy2, 0, vh - 1));
    let rb2 = select(ky2, ky2 / 2u, p.in_scale == 2u) * p.in_w;
    let cy3 = by + 3;
    let my3 = select(0.0, 1.0, cy3 >= 0 && cy3 < vh);
    let ky3 = u32(clamp(cy3, 0, vh - 1));
    let rb3 = select(ky3, ky3 / 2u, p.in_scale == 2u) * p.in_w;
    let cx0 = bx + 0;
    let mx0 = select(0.0, 1.0, cx0 >= 0 && cx0 < vw);
    let kx0 = u32(clamp(cx0, 0, vw - 1));
    let cb0 = select(kx0, kx0 / 2u, p.in_scale == 2u);
    let cx1 = bx + 1;
    let mx1 = select(0.0, 1.0, cx1 >= 0 && cx1 < vw);
    let kx1 = u32(clamp(cx1, 0, vw - 1));
    let cb1 = select(kx1, kx1 / 2u, p.in_scale == 2u);
    let cx2 = bx + 2;
    let mx2 = select(0.0, 1.0, cx2 >= 0 && cx2 < vw);
    let kx2 = u32(clamp(cx2, 0, vw - 1));
    let cb2 = select(kx2, kx2 / 2u, p.in_scale == 2u);
    let cx3 = bx + 3;
    let mx3 = select(0.0, 1.0, cx3 >= 0 && cx3 < vw);
    let kx3 = u32(clamp(cx3, 0, vw - 1));
    let cb3 = select(kx3, kx3 / 2u, p.in_scale == 2u);
    let msk00 = my0 * mx0;
    let msk01 = my0 * mx1;
    let msk02 = my0 * mx2;
    let msk03 = my0 * mx3;
    let msk10 = my1 * mx0;
    let msk11 = my1 * mx1;
    let msk12 = my1 * mx2;
    let msk13 = my1 * mx3;
    let msk20 = my2 * mx0;
    let msk21 = my2 * mx1;
    let msk22 = my2 * mx2;
    let msk23 = my2 * mx3;
    let msk30 = my3 * mx0;
    let msk31 = my3 * mx1;
    let msk32 = my3 * mx2;
    let msk33 = my3 * mx3;
    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let ib = ic * in_plane;
        let v00 = input[ib + rb0 + cb0] * msk00; let v01 = input[ib + rb0 + cb1] * msk01; let v02 = input[ib + rb0 + cb2] * msk02; let v03 = input[ib + rb0 + cb3] * msk03;
        let v10 = input[ib + rb1 + cb0] * msk10; let v11 = input[ib + rb1 + cb1] * msk11; let v12 = input[ib + rb1 + cb2] * msk12; let v13 = input[ib + rb1 + cb3] * msk13;
        let v20 = input[ib + rb2 + cb0] * msk20; let v21 = input[ib + rb2 + cb1] * msk21; let v22 = input[ib + rb2 + cb2] * msk22; let v23 = input[ib + rb2 + cb3] * msk23;
        let v30 = input[ib + rb3 + cb0] * msk30; let v31 = input[ib + rb3 + cb1] * msk31; let v32 = input[ib + rb3 + cb2] * msk32; let v33 = input[ib + rb3 + cb3] * msk33;
        {
            let k00 = weights[wpA + 0u]; let k01 = weights[wpA + 1u]; let k02 = weights[wpA + 2u]; let k10 = weights[wpA + 3u]; let k11 = weights[wpA + 4u]; let k12 = weights[wpA + 5u]; let k20 = weights[wpA + 6u]; let k21 = weights[wpA + 7u]; let k22 = weights[wpA + 8u];
            accA00 = accA00 + k00 * v00 + k01 * v01 + k02 * v02 + k10 * v10 + k11 * v11 + k12 * v12 + k20 * v20 + k21 * v21 + k22 * v22;
            accA01 = accA01 + k00 * v01 + k01 * v02 + k02 * v03 + k10 * v11 + k11 * v12 + k12 * v13 + k20 * v21 + k21 * v22 + k22 * v23;
            accA10 = accA10 + k00 * v10 + k01 * v11 + k02 * v12 + k10 * v20 + k11 * v21 + k12 * v22 + k20 * v30 + k21 * v31 + k22 * v32;
            accA11 = accA11 + k00 * v11 + k01 * v12 + k02 * v13 + k10 * v21 + k11 * v22 + k12 * v23 + k20 * v31 + k21 * v32 + k22 * v33;
        }
        wpA = wpA + 9u;
        {
            let k00 = weights[wpB + 0u]; let k01 = weights[wpB + 1u]; let k02 = weights[wpB + 2u]; let k10 = weights[wpB + 3u]; let k11 = weights[wpB + 4u]; let k12 = weights[wpB + 5u]; let k20 = weights[wpB + 6u]; let k21 = weights[wpB + 7u]; let k22 = weights[wpB + 8u];
            accB00 = accB00 + k00 * v00 + k01 * v01 + k02 * v02 + k10 * v10 + k11 * v11 + k12 * v12 + k20 * v20 + k21 * v21 + k22 * v22;
            accB01 = accB01 + k00 * v01 + k01 * v02 + k02 * v03 + k10 * v11 + k11 * v12 + k12 * v13 + k20 * v21 + k21 * v22 + k22 * v23;
            accB10 = accB10 + k00 * v10 + k01 * v11 + k02 * v12 + k10 * v20 + k11 * v21 + k12 * v22 + k20 * v30 + k21 * v31 + k22 * v32;
            accB11 = accB11 + k00 * v11 + k01 * v12 + k02 * v13 + k10 * v21 + k11 * v22 + k12 * v23 + k20 * v31 + k21 * v32 + k22 * v33;
        }
        wpB = wpB + 9u;
    }
    let out_plane = p.out_w * p.out_h;
    let o00 = oy * p.out_w + ox;
    let obA = (p.dst_off + gA0 * 4u) * out_plane;
    store4(obA + o00, out_plane, epilogue(accA00, gA0, o00, out_plane));
    if (okx) { store4(obA + o00 + 1u, out_plane, epilogue(accA01, gA0, o00 + 1u, out_plane)); }
    if (oky) {
        let o10 = o00 + p.out_w;
        store4(obA + o10, out_plane, epilogue(accA10, gA0, o10, out_plane));
        if (okx) { store4(obA + o10 + 1u, out_plane, epilogue(accA11, gA0, o10 + 1u, out_plane)); }
    }
    if (okg1) {
        let obB = (p.dst_off + gA1 * 4u) * out_plane;
        store4(obB + o00, out_plane, epilogue(accB00, gA1, o00, out_plane));
        if (okx) { store4(obB + o00 + 1u, out_plane, epilogue(accB01, gA1, o00 + 1u, out_plane)); }
        if (oky) {
            let o10 = o00 + p.out_w;
            store4(obB + o10, out_plane, epilogue(accB10, gA1, o10, out_plane));
            if (okx) { store4(obB + o10 + 1u, out_plane, epilogue(accB11, gA1, o10 + 1u, out_plane)); }
        }
    }
}

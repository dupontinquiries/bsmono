// 3x3 convolution over planar f32 channels, same or valid padding, with the
// source optionally read as nearest-upsampled x2 (in_scale), a fused
// residual (out = acc*res_scale + residual), and a fused activation
// (none / LeakyReLU / per-channel PReLU). Writes channels
// [dst_off, dst_off+out_ch) of the output buffer, so dense concatenation is
// a channel offset and never a copy.
//
// Winograd F(2x2, 3x3): each invocation produces a 2x2 patch of output
// pixels for 4 output channels from a 4x4 input patch, using 16 multiplies
// per input channel instead of the direct form's 36 — 2.25x fewer. The
// input transform (B^T d B) is 32 adds of the 4x4 patch, the output
// transform (A^T m A) is 12 adds; both matrices are +/-1, so the only
// non-exact step is the weight transform G g G^T, which is done once on the
// host in f32 and has entries in {1, 1/2}: exactly representable, so the
// result matches the direct kernel to f32 rounding.
//
// The weight buffer therefore holds 16 transformed taps per (oc4, ic)
// instead of 9 — `ConvKernel::Wino` repacks at upload. Register pressure
// sits between rt2 and rt4: 4 accumulators of 4 lanes plus the 4x4 patch.
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
    let oc4 = gid.z;
    if (ox >= p.out_w || oy >= p.out_h || oc4 >= p.out_ch4) {
        return;
    }
    let has_x1 = (ox + 1u) < p.out_w;
    let has_y1 = (oy + 1u) < p.out_h;
    // m[k] accumulates the element-wise product of transformed input and weights.
    var m0 = vec4<f32>(0.0);
    var m1 = vec4<f32>(0.0);
    var m2 = vec4<f32>(0.0);
    var m3 = vec4<f32>(0.0);
    var m4 = vec4<f32>(0.0);
    var m5 = vec4<f32>(0.0);
    var m6 = vec4<f32>(0.0);
    var m7 = vec4<f32>(0.0);
    var m8 = vec4<f32>(0.0);
    var m9 = vec4<f32>(0.0);
    var m10 = vec4<f32>(0.0);
    var m11 = vec4<f32>(0.0);
    var m12 = vec4<f32>(0.0);
    var m13 = vec4<f32>(0.0);
    var m14 = vec4<f32>(0.0);
    var m15 = vec4<f32>(0.0);
    let in_plane = p.in_w * p.in_h;
    var wi = oc4 * p.in_ch * 16u;
    let sx0 = i32(ox) - i32(p.pad);
    let sy0 = i32(oy) - i32(p.pad);
    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let ib = ic * in_plane;
        let d00 = load(ib, sy0 + 0, sx0 + 0); let d01 = load(ib, sy0 + 0, sx0 + 1); let d02 = load(ib, sy0 + 0, sx0 + 2); let d03 = load(ib, sy0 + 0, sx0 + 3);
        let d10 = load(ib, sy0 + 1, sx0 + 0); let d11 = load(ib, sy0 + 1, sx0 + 1); let d12 = load(ib, sy0 + 1, sx0 + 2); let d13 = load(ib, sy0 + 1, sx0 + 3);
        let d20 = load(ib, sy0 + 2, sx0 + 0); let d21 = load(ib, sy0 + 2, sx0 + 1); let d22 = load(ib, sy0 + 2, sx0 + 2); let d23 = load(ib, sy0 + 2, sx0 + 3);
        let d30 = load(ib, sy0 + 3, sx0 + 0); let d31 = load(ib, sy0 + 3, sx0 + 1); let d32 = load(ib, sy0 + 3, sx0 + 2); let d33 = load(ib, sy0 + 3, sx0 + 3);
        // B^T d B, rows then columns (B^T = [[1,0,-1,0],[0,1,1,0],[0,-1,1,0],[0,1,0,-1]]).
        let t00 = d00 - d20;
        let t10 = d10 + d20;
        let t20 = d20 - d10;
        let t30 = d10 - d30;
        let t01 = d01 - d21;
        let t11 = d11 + d21;
        let t21 = d21 - d11;
        let t31 = d11 - d31;
        let t02 = d02 - d22;
        let t12 = d12 + d22;
        let t22 = d22 - d12;
        let t32 = d12 - d32;
        let t03 = d03 - d23;
        let t13 = d13 + d23;
        let t23 = d23 - d13;
        let t33 = d13 - d33;
        let v00 = t00 - t02;
        let v01 = t01 + t02;
        let v02 = t02 - t01;
        let v03 = t01 - t03;
        let v10 = t10 - t12;
        let v11 = t11 + t12;
        let v12 = t12 - t11;
        let v13 = t11 - t13;
        let v20 = t20 - t22;
        let v21 = t21 + t22;
        let v22 = t22 - t21;
        let v23 = t21 - t23;
        let v30 = t30 - t32;
        let v31 = t31 + t32;
        let v32 = t32 - t31;
        let v33 = t31 - t33;
        m0 = m0 + weights[wi + 0u] * v00;
        m1 = m1 + weights[wi + 1u] * v01;
        m2 = m2 + weights[wi + 2u] * v02;
        m3 = m3 + weights[wi + 3u] * v03;
        m4 = m4 + weights[wi + 4u] * v10;
        m5 = m5 + weights[wi + 5u] * v11;
        m6 = m6 + weights[wi + 6u] * v12;
        m7 = m7 + weights[wi + 7u] * v13;
        m8 = m8 + weights[wi + 8u] * v20;
        m9 = m9 + weights[wi + 9u] * v21;
        m10 = m10 + weights[wi + 10u] * v22;
        m11 = m11 + weights[wi + 11u] * v23;
        m12 = m12 + weights[wi + 12u] * v30;
        m13 = m13 + weights[wi + 13u] * v31;
        m14 = m14 + weights[wi + 14u] * v32;
        m15 = m15 + weights[wi + 15u] * v33;
        wi = wi + 16u;
    }
    // A^T m A (A^T = [[1,1,1,0],[0,1,-1,-1]]).
    let s00 = m0 + m4 + m8;
    let s10 = m4 - m8 - m12;
    let s01 = m1 + m5 + m9;
    let s11 = m5 - m9 - m13;
    let s02 = m2 + m6 + m10;
    let s12 = m6 - m10 - m14;
    let s03 = m3 + m7 + m11;
    let s13 = m7 - m11 - m15;
    let y00 = s00 + s01 + s02;
    let y01 = s01 - s02 - s03;
    let y10 = s10 + s11 + s12;
    let y11 = s11 - s12 - s13;
    let out_plane = p.out_w * p.out_h;
    let idx00 = oy * p.out_w + ox;
    let obase = (p.dst_off + oc4 * 4u) * out_plane;
    store4(obase + idx00, out_plane, epilogue(y00 + bias[oc4], oc4, idx00, out_plane));
    if (has_x1) { store4(obase + idx00 + 1u, out_plane, epilogue(y01 + bias[oc4], oc4, idx00 + 1u, out_plane)); }
    if (has_y1) {
        let idx10 = idx00 + p.out_w;
        store4(obase + idx10, out_plane, epilogue(y10 + bias[oc4], oc4, idx10, out_plane));
        if (has_x1) { store4(obase + idx10 + 1u, out_plane, epilogue(y11 + bias[oc4], oc4, idx10 + 1u, out_plane)); }
    }
}

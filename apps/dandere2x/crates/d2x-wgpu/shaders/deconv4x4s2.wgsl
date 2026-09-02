// waifu2x's transposed 4x4 / stride 2 / pad 3 convolution (valid: n -> 2n-4),
// gather form, planar f32 in and out, no activation (the pack pass clamps).
//   weights [ic][ky][kx]  vec4 = (r, g, b, 0);  bias vec4;  output [3][y][x]

struct Params {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_w: u32,
    out_h: u32,
    _p0: u32,
    _p1: u32,
    _p2: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read_write> input: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> bias: vec4<f32>;
@group(0) @binding(4) var<storage, read_write> output: array<f32>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let oy = gid.y;
    if (ox >= p.out_w || oy >= p.out_h) {
        return;
    }
    let ky0 = (oy + 3u) % 2u;
    let kx0 = (ox + 3u) % 2u;
    let iy_hi = (oy + 3u - ky0) / 2u;
    let ix_hi = (ox + 3u - kx0) / 2u;
    let y_hi_ok = iy_hi < p.in_h;
    let x_hi_ok = ix_hi < p.in_w;
    let y_lo_ok = iy_hi >= 1u;
    let x_lo_ok = ix_hi >= 1u;
    let iy_lo = select(0u, iy_hi - 1u, y_lo_ok);
    let ix_lo = select(0u, ix_hi - 1u, x_lo_ok);
    let m_hh = select(0.0, 1.0, y_hi_ok && x_hi_ok);
    let m_hl = select(0.0, 1.0, y_hi_ok && x_lo_ok);
    let m_lh = select(0.0, 1.0, y_lo_ok && x_hi_ok);
    let m_ll = select(0.0, 1.0, y_lo_ok && x_lo_ok);
    let iy_h = min(iy_hi, p.in_h - 1u);
    let ix_h = min(ix_hi, p.in_w - 1u);
    let in_plane = p.in_w * p.in_h;
    let t_hh = ky0 * 4u + kx0;
    let t_hl = ky0 * 4u + kx0 + 2u;
    let t_lh = (ky0 + 2u) * 4u + kx0;
    let t_ll = (ky0 + 2u) * 4u + kx0 + 2u;
    var acc = bias.xyz;
    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let base = ic * in_plane;
        let v_hh = input[base + iy_h * p.in_w + ix_h] * m_hh;
        let v_hl = input[base + iy_h * p.in_w + ix_lo] * m_hl;
        let v_lh = input[base + iy_lo * p.in_w + ix_h] * m_lh;
        let v_ll = input[base + iy_lo * p.in_w + ix_lo] * m_ll;
        let wb = ic * 16u;
        acc = acc + weights[wb + t_hh].xyz * v_hh + weights[wb + t_hl].xyz * v_hl + weights[wb + t_lh].xyz * v_lh + weights[wb + t_ll].xyz * v_ll;
    }
    let out_plane = p.out_w * p.out_h;
    let idx = oy * p.out_w + ox;
    output[idx] = acc.x;
    output[idx + out_plane] = acc.y;
    output[idx + 2u * out_plane] = acc.z;
}

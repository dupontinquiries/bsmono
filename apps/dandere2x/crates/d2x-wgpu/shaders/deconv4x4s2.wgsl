// Transposed 4x4 / stride 2 / pad 3 convolution to RGB, in gather form:
// out[oy][ox] = bias + sum over (ic, ky, kx) of in[ic][iy][ix] * w[ic][ky][kx]
// where oy = 2*iy - 3 + ky, so for a given oy only ky with the parity of
// (oy + 3) contribute and iy = (oy + 3 - ky) / 2. Same in x.
//
// Output is clamped to [0,1] and packed as RGBA8 (pack4x8unorm rounds like
// d2x_core::waifu2x::unorm8), which quarters the readback versus f32 planes.
//
// Layouts:
//   input   [ic][y][x]         f32
//   weights [ic][ky][kx]       vec4 = (r, g, b, 0)
//   bias    vec4               (r, g, b, 0)
//   output  [y][x]             u32 RGBA8

struct Params {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
    _pad2: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> input: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<uniform> bias: vec4<f32>;
@group(0) @binding(4) var<storage, read_write> output: array<u32>;

@compute @workgroup_size(16, 16, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    let ox = gid.x;
    let oy = gid.y;
    if (ox >= p.out_w || oy >= p.out_h) {
        return;
    }
    // Tap parity; the two contributing ky are ky0 and ky0 + 2.
    let ky0 = (oy + 3u) % 2u;
    let kx0 = (ox + 3u) % 2u;
    // iy for ky0 is the larger index; for ky0 + 2 it is one less.
    let iy_hi = (oy + 3u - ky0) / 2u;
    let ix_hi = (ox + 3u - kx0) / 2u;
    let y_hi_ok = iy_hi < p.in_h;
    let x_hi_ok = ix_hi < p.in_w;
    let y_lo_ok = iy_hi >= 1u;              // iy_hi - 1 >= 0
    let x_lo_ok = ix_hi >= 1u;
    let iy_lo = select(0u, iy_hi - 1u, y_lo_ok);
    let ix_lo = select(0u, ix_hi - 1u, x_lo_ok);
    // Zero-out contributions from taps that fall outside the input.
    let m_hh = select(0.0, 1.0, y_hi_ok && x_hi_ok);
    let m_hl = select(0.0, 1.0, y_hi_ok && x_lo_ok);
    let m_lh = select(0.0, 1.0, y_lo_ok && x_hi_ok);
    let m_ll = select(0.0, 1.0, y_lo_ok && x_lo_ok);
    // Clamp indices so reads stay in bounds even when masked.
    let iy_h = min(iy_hi, p.in_h - 1u);
    let ix_h = min(ix_hi, p.in_w - 1u);

    let in_plane = p.in_w * p.in_h;
    // Weight taps: (ky0, kx0) pairs with (iy_hi, ix_hi); (ky0+2, kx0) with (iy_lo, ix_hi); etc.
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
        acc = acc + weights[wb + t_hh].xyz * v_hh
                  + weights[wb + t_hl].xyz * v_hl
                  + weights[wb + t_lh].xyz * v_lh
                  + weights[wb + t_ll].xyz * v_ll;
    }
    let rgb = clamp(acc, vec3<f32>(0.0), vec3<f32>(1.0));
    output[oy * p.out_w + ox] = pack4x8unorm(vec4<f32>(rgb, 1.0));
}

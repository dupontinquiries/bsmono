// Valid 3x3 convolution + LeakyReLU(0.1), planar f32 in and out.
//
// Each invocation produces a 2x2 patch of output pixels for 4 output
// channels (one vec4 per pixel). Per input channel it loads a 4x4 input
// patch (16 scalars) and 9 vec4 weights and performs 36 vec4 FMAs — about
// 2.2x the arithmetic per byte of a one-pixel-per-thread kernel, with no
// workgroup memory (which on AGX costs occupancy — see ASAHI_GPU.md).
//
// Layouts:
//   input   [ic][y][x]            f32
//   weights [oc/4][ic][ky][kx]    vec4 = (oc+0, oc+1, oc+2, oc+3)
//   bias    [oc/4]                vec4
//   output  [oc][y][x]            f32
//
// No dynamic shifts anywhere (K2 hazard on Honeykrisp); the only divisions
// are by constants.

struct Params {
    in_w: u32,
    in_h: u32,
    in_ch: u32,
    out_ch4: u32,
    out_w: u32,
    out_h: u32,
    _pad0: u32,
    _pad1: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var<storage, read> input: array<f32>;
@group(0) @binding(2) var<storage, read> weights: array<vec4<f32>>;
@group(0) @binding(3) var<storage, read> bias: array<vec4<f32>>;
@group(0) @binding(4) var<storage, read_write> output: array<f32>;

const SLOPE: f32 = 0.1;

fn leaky(v: vec4<f32>) -> vec4<f32> {
    return select(v * SLOPE, v, v >= vec4<f32>(0.0));
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
    // Edge patches may be 1 pixel wide/high; compute all four and store
    // only what exists (reads are clamped to stay in the input).
    let has_x1 = (ox + 1u) < p.out_w;
    let has_y1 = (oy + 1u) < p.out_h;
    let xw = select(0u, 1u, has_x1);
    let yw = select(0u, 1u, has_y1);

    let b = bias[oc4];
    var a00 = b;
    var a01 = b;
    var a10 = b;
    var a11 = b;

    let in_plane = p.in_w * p.in_h;
    var wi = oc4 * p.in_ch * 9u;

    for (var ic = 0u; ic < p.in_ch; ic = ic + 1u) {
        let r0 = ic * in_plane + oy * p.in_w + ox;
        let r1 = r0 + p.in_w;
        let r2 = r1 + p.in_w;
        let r3 = r2 + p.in_w * yw;   // clamped when the patch is 1 high
        let x3 = 2u + xw;            // clamped when the patch is 1 wide

        // 4x4 input patch.
        let i00 = input[r0]; let i01 = input[r0 + 1u]; let i02 = input[r0 + 2u]; let i03 = input[r0 + x3];
        let i10 = input[r1]; let i11 = input[r1 + 1u]; let i12 = input[r1 + 2u]; let i13 = input[r1 + x3];
        let i20 = input[r2]; let i21 = input[r2 + 1u]; let i22 = input[r2 + 2u]; let i23 = input[r2 + x3];
        let i30 = input[r3]; let i31 = input[r3 + 1u]; let i32 = input[r3 + 2u]; let i33 = input[r3 + x3];

        let w00 = weights[wi];      let w01 = weights[wi + 1u]; let w02 = weights[wi + 2u];
        let w10 = weights[wi + 3u]; let w11 = weights[wi + 4u]; let w12 = weights[wi + 5u];
        let w20 = weights[wi + 6u]; let w21 = weights[wi + 7u]; let w22 = weights[wi + 8u];
        wi = wi + 9u;

        // Same summation order as the CPU reference: rows, then columns.
        a00 = a00 + w00 * i00 + w01 * i01 + w02 * i02
                  + w10 * i10 + w11 * i11 + w12 * i12
                  + w20 * i20 + w21 * i21 + w22 * i22;
        a01 = a01 + w00 * i01 + w01 * i02 + w02 * i03
                  + w10 * i11 + w11 * i12 + w12 * i13
                  + w20 * i21 + w21 * i22 + w22 * i23;
        a10 = a10 + w00 * i10 + w01 * i11 + w02 * i12
                  + w10 * i20 + w11 * i21 + w12 * i22
                  + w20 * i30 + w21 * i31 + w22 * i32;
        a11 = a11 + w00 * i11 + w01 * i12 + w02 * i13
                  + w10 * i21 + w11 * i22 + w12 * i23
                  + w20 * i31 + w21 * i32 + w22 * i33;
    }

    let out_plane = p.out_w * p.out_h;
    let o00 = (oc4 * 4u) * out_plane + oy * p.out_w + ox;
    store4(o00, out_plane, leaky(a00));
    if (has_x1) {
        store4(o00 + 1u, out_plane, leaky(a01));
    }
    if (has_y1) {
        store4(o00 + p.out_w, out_plane, leaky(a10));
        if (has_x1) {
            store4(o00 + p.out_w + 1u, out_plane, leaky(a11));
        }
    }
}

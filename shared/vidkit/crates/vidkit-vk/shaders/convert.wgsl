// YUV -> linear RGBA, one compute dispatch per frame.
//
// Why a hand-written pass instead of VK_KHR_sampler_ycbcr_conversion:
// ycbcr conversion requires an *immutable sampler* baked into the descriptor
// set layout, which wgpu's binding model cannot express, and egui's renderer
// certainly cannot. Doing it ourselves costs one pass (~0.15 ms at 1080p on
// Adreno 6xx by my estimate — measure it) and buys three things we need:
// explicit matrix/range control, PQ/HLG tonemapping, and an output texture
// that egui samples like any other.
//
// Planes arrive as separate textures because that is what a DMA-BUF import
// with VK_EXT_image_drm_format_modifier gives us anyway: one VkImage per
// plane, via DISJOINT + the per-plane memory aspect bits.

struct Params {
    // 0 = BT.601, 1 = BT.709, 2 = BT.2020 NCL
    matrix_id: u32,
    // 0 = SDR (already ~linear enough after the matrix), 1 = PQ, 2 = HLG
    transfer_id: u32,
    // 0 = limited (16..235 / 64..940), 1 = full
    range_id: u32,
    // 2 for NV12/P010 (interleaved chroma), 3 for I420 family
    plane_count: u32,
    width: u32,
    height: u32,
    // Peak luminance to map PQ content to, in nits. 203 is the ITU reference
    // white; going higher just clips on an SDR panel.
    target_nits: f32,
    _pad: u32,
};

@group(0) @binding(0) var<uniform> p: Params;
@group(0) @binding(1) var plane_y: texture_2d<f32>;
// For NV12/P010 this is the interleaved CbCr plane and plane_v is unused
// (bound to a 1x1 dummy — Vulkan requires every declared binding to be valid).
@group(0) @binding(2) var plane_u: texture_2d<f32>;
@group(0) @binding(3) var plane_v: texture_2d<f32>;
@group(0) @binding(4) var samp: sampler;
@group(0) @binding(5) var out_tex: texture_storage_2d<rgba16float, write>;

fn yuv_to_rgb_matrix(id: u32) -> mat3x3<f32> {
    // Columns are the Y, Cb, Cr contributions.
    switch id {
        case 0u: { // BT.601
            return mat3x3<f32>(
                vec3<f32>(1.0,      1.0,      1.0),
                vec3<f32>(0.0,     -0.39176,  2.01723),
                vec3<f32>(1.59603, -0.81297,  0.0),
            );
        }
        case 2u: { // BT.2020 non-constant luminance
            return mat3x3<f32>(
                vec3<f32>(1.0,      1.0,      1.0),
                vec3<f32>(0.0,     -0.16455,  1.88140),
                vec3<f32>(1.47460, -0.57135,  0.0),
            );
        }
        default: { // BT.709
            return mat3x3<f32>(
                vec3<f32>(1.0,      1.0,      1.0),
                vec3<f32>(0.0,     -0.18732,  1.85560),
                vec3<f32>(1.57480, -0.46812,  0.0),
            );
        }
    }
}

// SMPTE ST 2084 EOTF, normalised so 1.0 == 10000 nits.
fn pq_eotf(e: f32) -> f32 {
    let m1 = 0.1593017578125;
    let m2 = 78.84375;
    let c1 = 0.8359375;
    let c2 = 18.8515625;
    let c3 = 18.6875;
    let ep = pow(max(e, 0.0), 1.0 / m2);
    let num = max(ep - c1, 0.0);
    let den = c2 - c3 * ep;
    return pow(num / max(den, 1e-6), 1.0 / m1);
}

// ARIB STD-B67 inverse OETF, scene-referred.
fn hlg_eotf(e: f32) -> f32 {
    let a = 0.17883277;
    let b = 0.28466892;
    let c = 0.55991073;
    if e <= 0.5 {
        return (e * e) / 3.0;
    }
    return (exp((e - c) / a) + b) / 12.0;
}

// Soft-knee roll-off: linear below the knee, hyperbolic above, asymptotic to
// 1.0. `peak` is the luminance that should render as SDR white.
//
// The obvious `v / (1 + v)` Reinhard is wrong here and not subtly: it maps
// v = 1 — that is, exactly the target luminance — to 0.5. ITU reference white
// is 203 nits, so every HDR file would render diffuse white as mid-grey and
// look uniformly dim. Measured at 0.487 before this change; see
// `pq_reference_white_lands_near_sdr_white`.
//
// With knee = 0.8: reference white lands at 0.9, 1000 nits at 0.99, and 10000
// nits at 0.999, so highlights stay ordered instead of clipping flat.
// Still a placeholder — a real display-referred tonemap is a tuning exercise —
// but one whose anchor points are defensible.
fn tonemap(x: f32, peak: f32) -> f32 {
    let knee = 0.8;
    let v = x / max(peak, 1.0);
    if v <= knee {
        return v;
    }
    let e = v - knee;
    return knee + (1.0 - knee) * (e / (e + (1.0 - knee)));
}

@compute @workgroup_size(8, 8, 1)
fn main(@builtin(global_invocation_id) gid: vec3<u32>) {
    if gid.x >= p.width || gid.y >= p.height {
        return;
    }

    let uv = (vec2<f32>(f32(gid.x), f32(gid.y)) + vec2<f32>(0.5))
           / vec2<f32>(f32(p.width), f32(p.height));

    let y_raw = textureSampleLevel(plane_y, samp, uv, 0.0).r;

    var cb: f32;
    var cr: f32;
    if p.plane_count == 2u {
        // NV12 / P010: chroma is interleaved in an RG texture, and the
        // hardware sampler does the 2x chroma upscale for free (bilinear).
        let c = textureSampleLevel(plane_u, samp, uv, 0.0).rg;
        cb = c.r;
        cr = c.g;
    } else {
        cb = textureSampleLevel(plane_u, samp, uv, 0.0).r;
        cr = textureSampleLevel(plane_v, samp, uv, 0.0).r;
    }

    // Range expansion. Note this is done in normalised [0,1] space, which is
    // correct for both 8- and 10-bit as long as the 10-bit planes were
    // imported as R16_UNORM with the data left-shifted (P010's layout) —
    // 10-bit-in-low-bits (I420_10LE) needs the *_10 scale factor applied on
    // import instead, or everything comes out dark. Easy to get wrong.
    var yv = y_raw;
    var u = cb;
    var v = cr;
    if p.range_id == 0u {
        yv = (yv - 16.0 / 255.0) * (255.0 / 219.0);
        u  = (u  - 128.0 / 255.0) * (255.0 / 224.0);
        v  = (v  - 128.0 / 255.0) * (255.0 / 224.0);
    } else {
        u = u - 0.5;
        v = v - 0.5;
    }

    let m = yuv_to_rgb_matrix(p.matrix_id);
    var rgb = m * vec3<f32>(yv, u, v);

    // HDR -> SDR, mirroring vidkit-core's `hdr_to_sdr`: linear light with
    // reference white (target_nits) at 1.0, BT.2020 -> BT.709 in linear
    // light when the matrix is BT.2020, the knee on the MAX channel (per
    // channel shifted hue), then sRGB encoding -- the step that was missing:
    // linear light went into the non-linear output and 18% grey rendered at
    // ~43/255 instead of ~118. SDR stays gamma-encoded, as before.
    //
    // KNOWN REMAINING DIFFERENCE, not fixed here: the limited-range
    // constants below are 8-bit (16/219/128/224). vidkit-core now uses
    // 64/876/512/896 for 10-bit; on P010 input this shader still centres
    // chroma at 128/255, a ~5% green cast in PQ. Fixing it needs a bit-depth
    // field in Params, i.e. the Rust uniform layout, and there is no Vulkan
    // device in the sandbox to run the parity tests that would check it.
    if p.transfer_id == 1u || p.transfer_id == 2u {
        var lin: vec3<f32>;
        if p.transfer_id == 1u {
            lin = vec3<f32>(pq_eotf(rgb.r), pq_eotf(rgb.g), pq_eotf(rgb.b)) * 10000.0 / max(p.target_nits, 1.0);
        } else {
            let scene = vec3<f32>(hlg_eotf(rgb.r), hlg_eotf(rgb.g), hlg_eotf(rgb.b));
            let ys = dot(scene, vec3<f32>(0.2627, 0.6780, 0.0593));
            let k = select(0.0, pow(ys, 0.2), ys > 0.0);
            lin = scene * k * 1000.0 / max(p.target_nits, 1.0);
        }
        if p.matrix_id == 2u {
            // WGSL matrices are column-major: these are BT.2087's rows
            // written as columns.
            let m = mat3x3<f32>(
                vec3<f32>(1.6605, -0.1246, -0.0182),
                vec3<f32>(-0.5876, 1.1329, -0.1006),
                vec3<f32>(-0.0728, -0.0083, 1.1187),
            );
            lin = m * lin;
        }
        lin = max(lin, vec3<f32>(0.0));
        let peak = max(lin.r, max(lin.g, lin.b));
        let scale = select(0.0, tonemap(peak, 1.0) / peak, peak > 0.0);
        let l = clamp(lin * scale, vec3<f32>(0.0), vec3<f32>(1.0));
        rgb = select(1.055 * pow(l, vec3<f32>(1.0 / 2.4)) - 0.055, 12.92 * l, l <= vec3<f32>(0.0031308));
    }

    textureStore(out_tex, vec2<i32>(i32(gid.x), i32(gid.y)),
                 vec4<f32>(clamp(rgb, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0));
}

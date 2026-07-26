
struct D { nblk:u32, qs_off:u32, qw_base:u32, grid_w:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;      // raw Q4_K bytes as words
@group(0)@binding(1) var<storage,read_write> dst: array<u32>; // qw words, then qs (bitcast f32)
@group(0)@binding(2) var<uniform> dims: D;

fn byte_at(i: u32) -> u32 { return (src[i >> 2u] >> ((i & 3u) * 8u)) & 0xFFu; }

// Bit-for-bit twin of the CPU gguf::f16_to_f32 (see the note on the dequant kernel): construct the f32
// bit pattern and bitcast, so Inf/NaN survive instead of being approximated away.
fn f16(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp  = (h >> 10u) & 0x1Fu;
    let mant = h & 0x3FFu;
    var bits: u32;
    if (exp == 0u) {
        if (mant == 0u) {
            bits = sign << 31u;
        } else {
            var e: i32 = -14; var m: u32 = mant;
            loop { if ((m & 0x400u) != 0u) { break; } m = m << 1u; e = e - 1; }
            bits = (sign << 31u) | (u32(e + 127) << 23u) | ((m & 0x3FFu) << 13u);
        }
    } else if (exp == 0x1Fu) {
        bits = (sign << 31u) | (0xFFu << 23u) | (mant << 13u);
    } else {
        bits = (sign << 31u) | (u32(i32(exp) + 127 - 15) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits);
}

fn scale_min(j: u32, base: u32) -> vec2<f32> {
    var sc: u32; var mn: u32;
    if (j < 4u) { sc = byte_at(base + j) & 63u; mn = byte_at(base + j + 4u) & 63u; }
    else {
        sc = (byte_at(base + j + 4u) & 0x0Fu) | ((byte_at(base + j - 4u) >> 6u) << 4u);
        mn = (byte_at(base + j + 4u) >> 4u) | ((byte_at(base + j) >> 6u) << 4u);
    }
    return vec2<f32>(f32(sc), f32(mn));
}

/// weight at position `t` (0..256) of super-block `blk`
fn wq(blk: u32, t: u32) -> f32 {
    let bbase = blk * 144u;
    let d    = f16(byte_at(bbase) | (byte_at(bbase + 1u) << 8u));
    let dmin = f16(byte_at(bbase + 2u) | (byte_at(bbase + 3u) << 8u));
    let grp = t / 64u; let idx = t % 64u; let half = idx / 32u; let l = idx % 32u;
    let sm = scale_min(grp * 2u + half, bbase + 4u);
    let q = byte_at(bbase + 16u + grp * 32u + l);
    let nib = select(q & 0xFu, q >> 4u, half == 1u);
    return d * sm.x * f32(nib) - dmin * sm.y;
}

@compute @workgroup_size(8)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    // see the note on the dequant kernel: the grid wraps at grid_w to stay under 65535 per dimension
    let blk = wg.y * dims.grid_w + wg.x;
    if (blk >= dims.nblk) { return; }
    let j = lid.x;                       // which 32-element block inside this super-block
    let t0 = j * 32u;
    // absmax over the 32 weights, then the same scale rule as the CPU quantizer
    var amax = 0.0;
    for (var l = 0u; l < 32u; l = l + 1u) {
        let v = abs(wq(blk, t0 + l));
        if (v == v) { amax = max(amax, v); }   // NaN-skipping, matching Rust's f32::max
    }
    var scale = 1.0;
    if (amax > 0.0) { scale = amax / 7.0; }
    let bi = blk * 8u + j;               // global index of this 32-block
    dst[dims.qs_off + bi] = bitcast<u32>(scale);
    // pack 8 nibbles per word, 4 words per 32-block
    for (var w = 0u; w < 4u; w = w + 1u) {
        var acc = 0u;
        for (var b = 0u; b < 8u; b = b + 1u) {
            let v = wq(blk, t0 + w * 8u + b);
            // Two deliberate mismatches with the naive spelling, both to match the CPU reference:
            //  * Rust's `f32 as i32` saturates NaN to 0; WGSL's i32() of NaN is undefined.
            //  * Rust's f32::round() is half-AWAY-from-zero; WGSL's round() is half-to-even.
            var qv = 0.0;
            if (v == v) {
                let x = v / scale;
                let r = sign(x) * floor(abs(x) + 0.5);
                qv = clamp(r, -7.0, 7.0);
            }
            let nib = u32(i32(qv) + 8) & 0xFu;
            acc = acc | (nib << (b * 4u));
        }
        dst[dims.qw_base + bi * 4u + w] = acc;
    }
}

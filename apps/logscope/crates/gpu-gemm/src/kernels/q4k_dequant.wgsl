// Q4K_DEQUANT
struct D { n:u32, grid_w:u32, _b:u32, _c:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;     // raw Q4_K bytes, viewed as words
@group(0)@binding(1) var<storage,read_write> dst: array<f32>;
@group(0)@binding(2) var<uniform> dims: D;                  // n = super-block count

// extractBits, NOT shift-and-mask: a shift by a lane-varying amount returns zero on
// Honeykrisp for every lane whose shift is non-zero (--gate-bytes, extractBits CORRECT
// where this form is WRONG). Four consecutive threads select four different bytes of one
// word here, so three of every four got zero.
fn byte_at(i: u32) -> u32 { return extractBits(src[i >> 2u], (i & 3u) * 8u, 8u); }

// half -> f32 (subnormals and inf/nan handled the same way the CPU helper does)
// Bit-for-bit twin of the CPU gguf::f16_to_f32: construct the f32 bit pattern and bitcast, rather than
// computing a value. Approximating here is not safe — mapping Inf/NaN to a finite number diverges from the
// reference exactly where a corrupt or synthetic block carries one, and the divergence shows up as
// slightly wrong weights rather than an error.
fn f16(h: u32) -> f32 {
    let sign = (h >> 15u) & 1u;
    let exp  = (h >> 10u) & 0x1Fu;
    let mant = h & 0x3FFu;
    var bits: u32;
    if (exp == 0u) {
        if (mant == 0u) {
            bits = sign << 31u;
        } else {
            // subnormal: normalize until bit 10 is the implicit leading 1
            var e: i32 = -14; var m: u32 = mant;
            loop { if ((m & 0x400u) != 0u) { break; } m = m << 1u; e = e - 1; }
            bits = (sign << 31u) | (u32(e + 127) << 23u) | ((m & 0x3FFu) << 13u);
        }
    } else if (exp == 0x1Fu) {
        bits = (sign << 31u) | (0xFFu << 23u) | (mant << 13u);   // inf / nan, preserved
    } else {
        bits = (sign << 31u) | (u32(i32(exp) + 127 - 15) << 23u) | (mant << 13u);
    }
    return bitcast<f32>(bits);
}

// the 6-bit scale/min unpack, byte-for-byte with get_scale_min_k4
fn scale_min(j: u32, base: u32) -> vec2<f32> {
    var sc: u32; var mn: u32;
    if (j < 4u) {
        sc = byte_at(base + j) & 63u;
        mn = byte_at(base + j + 4u) & 63u;
    } else {
        sc = (byte_at(base + j + 4u) & 0x0Fu) | ((byte_at(base + j - 4u) >> 6u) << 4u);
        mn = (byte_at(base + j + 4u) >> 4u) | ((byte_at(base + j) >> 6u) << 4u);
    }
    return vec2<f32>(f32(sc), f32(mn));
}

@compute @workgroup_size(64)
fn main(@builtin(workgroup_id) wg: vec3<u32>, @builtin(local_invocation_id) lid: vec3<u32>) {
    // 2D grid: a workgroup-per-super-block overflows the 65535 per-dimension dispatch limit on any
    // large tensor (a 26M-weight matrix needs 102400 of them), so the grid wraps at grid_w.
    let blk = wg.y * dims.grid_w + wg.x;
    if (blk >= dims.n) { return; }
    let bbase = blk * 144u;                       // byte offset of this super-block
    let d    = f16(byte_at(bbase) | (byte_at(bbase + 1u) << 8u));
    let dmin = f16(byte_at(bbase + 2u) | (byte_at(bbase + 3u) << 8u));
    let sbase = bbase + 4u;                       // 12 scale bytes
    let qbase = bbase + 16u;                      // 128 nibble bytes
    // 64 lanes each handle 4 of the 256 outputs, mirroring the CPU's 4 groups of 64
    for (var t = lid.x; t < 256u; t = t + 64u) {
        let grp = t / 64u;                        // which of the 4 groups
        let idx = t % 64u;                        // position inside the group
        let half = idx / 32u;                     // low nibbles then high nibbles
        let l = idx % 32u;
        let sm = scale_min(grp * 2u + half, sbase);
        let q = byte_at(qbase + grp * 32u + l);
        let nib = select(q & 0xFu, q >> 4u, half == 1u);
        // THE SECOND SITE. The one below already binds these products, with a comment about FMA
        // fusion — and this one was left as a single expression, so the compiler fused it here and
        // not there. That is why 57 of 112 block scales came out an ULP from the CPU while the
        // dequant itself was exact: two paths, one fixed.
        //
        // The rule is not "never fuse". Fusion is safe where the output is CONTINUOUS and unsafe
        // where it feeds a DISCRETIZATION: an ULP changes nothing until the value crosses a
        // quantization boundary, at which point it becomes a whole level. Q6K_DEQUANT outputs
        // floats to be accumulated and fuses freely — faster AND more accurate, and bit-exact on
        // hardware. This value feeds round(v / scale) in the re-quantizer, so it must not.
        let ds = d * sm.x;
        let dm = dmin * sm.y;
        // Same explicit fma as the converter's wq, so the two GPU dequant paths cannot diverge —
        // which is exactly how this bug hid: the gate tested this one and the converter used the
        // other.
        dst[blk * 256u + grp * 64u + half * 32u + l] = fma(ds, f32(nib), -dm);
    }
}

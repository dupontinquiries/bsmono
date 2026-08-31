// Q4K_TO_Q4
struct D { nblk:u32, qs_off:u32, qw_base:u32, grid_w:u32 };
@group(0)@binding(0) var<storage,read> src: array<u32>;      // raw Q4_K bytes as words
@group(0)@binding(1) var<storage,read_write> dst: array<u32>; // qw words, then qs (bitcast f32)
@group(0)@binding(2) var<uniform> dims: D;

// extractBits, NOT shift-and-mask: a shift by a lane-varying amount returns zero on
// Honeykrisp for every lane whose shift is non-zero (--gate-bytes, extractBits CORRECT
// where this form is WRONG). Four consecutive threads select four different bytes of one
// word here, so three of every four got zero.
fn byte_at(i: u32) -> u32 { return extractBits(src[i >> 2u], (i & 3u) * 8u, 8u); }

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
    // BIND THE INTERMEDIATES, the same rule Q6_K_DEQUANT already documents and follows.
  //
  // As one expression the compiler may fuse `d * sm.x * nib` and `dmin * sm.y` into FMAs, rounding
  // ONCE where the CPU reference rounds twice. That is a one-ULP difference — gate-q4k saw
  // 0.31525478 against 0.31525475, 1.192e-7 — and it is not corruption, but it is enough to move a
  // weight across a quantization boundary and produce whole differing nibbles downstream.
  //
  // Q6_K binds d1..d4 for exactly this reason and is bit-exact everywhere. Q4_K never got the same
  // treatment, which is why one gate passes and the other does not.
  let dsc = d * sm.x;
  let dm = dmin * sm.y;
  // EXPLICIT fma, not a hope that the compiler contracts.
  //
  // The measurement: gate-q4k printed the amax each side's scale implies — gpu 2.206783533 against
  // cpu 2.206783295 — while "GPU dequant == CPU dequant" passed with 0 differing. Both are true
  // because there are TWO GPU dequant paths: the standalone Q4K_DEQUANT the gate exercises, and this
  // one inside the converter. The compiler contracted one and not the other, so they disagreed by an
  // ULP and the gate could not see it.
  //
  // Writing `dsc * nib - dm` leaves that to the compiler. `fma` states it, and matches the CPU
  // reference's `mul_add` exactly — one rounding on both sides, by construction rather than by luck.
  //
  // Three fixes before this one reasoned about the rule instead of reading the inputs, and all three
  // were wrong. The print that settled it took four lines.
  return fma(dsc, f32(nib), -dm);
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
    // MULTIPLY BY THE RECIPROCAL, matching what this driver does to `/` anyway.
  //
  // Metal's default fast-math lowers float division to a multiply by an f32-rounded reciprocal,
  // which is allowed 2.5 ULP and is NOT correctly rounded. That is the entire bug:
  //
  //     amax               0x400D3BF0
  //     amax / 7.0         0x3EA16912   <- what the CPU computes
  //     amax * f32(1/7)    0x3EA16913   <- what the GPU actually computed
  //
  // Both reproduce exactly, so there is nothing left to guess. The dequant was always exact (the wq
  // probe: 0 of 3584 differ), the reduction is `max` and therefore exact, and division was the one
  // operation in the chain I never tested — because I checked the reciprocal in DOUBLE precision,
  // where it round-trips and hides this.
  //
  // Written as an explicit reciprocal so the source says what the hardware does. The CPU reference is
  // changed to match in the same commit; a division here would keep silently becoming this anyway.
  if (amax > 0.0) { scale = amax * 0.14285714285714285; }
    let bi = blk * 8u + j;               // global index of this 32-block
    dst[dims.qs_off + bi] = bitcast<u32>(scale);
    // Bound once and shared by every weight in the block, so all 32 divide identically.
    var inv_scale = 0.0;
    if (scale > 0.0) { inv_scale = 1.0 / scale; }
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
                // RECIPROCAL AGAIN, one level down. The same fast-math lowering that broke amax/7 breaks this
      // division too — and here an ULP crosses a .5 boundary, where WGSL's round() (half-to-even) and
      // Rust's round() (half-away-from-zero) then disagree by a whole level.
      //
      // That is the 1104 survivors: not a dispatch or stride error, but the last of the same bug in
      // the quantizer's own divide. Their period tracks the test pattern rather than the grid, which
      // is what says "specific VALUES" rather than "specific positions".
      let x = v * inv_scale;
                let r = sign(x) * floor(abs(x) + 0.5);
                qv = clamp(r, -7.0, 7.0);
            }
            let nib = u32(i32(qv) + 8) & 0xFu;
            // insertBits, NOT a shift by a lane-varying amount. `nib << (b * 4u)` yields zero for
            // every lane whose shift is non-zero on Honeykrisp/Asahi — confirmed by --gate-bytes,
            // where extractBits with the same varying offset is correct and shift-and-mask is not.
            //
            // This is the conversion EVERY Q4_K model runs at load, so a silently wrong pack here is
            // wrong weights for every such checkpoint on this driver.
            acc = insertBits(acc, nib, b * 4u, 4u);
        }
        dst[dims.qw_base + bi * 4u + w] = acc;
    }
}

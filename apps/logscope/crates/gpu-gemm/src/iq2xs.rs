//! iq2xs.rs — IQ2_XS (ggml type 17), the last i-quant this engine refused.
//!
//! The grid (`IQ2XS_GRID`, 512 entries) has been in `iqgrids.rs` since the IQ2_XXS work; what was
//! missing was the decoder, and the refusal was right until there was one — a codebook format read
//! through the wrong decoder produces plausible floats and a model that generates confident nonsense.
//!
//! ## Layout (ggml `block_iq2_xs`)
//!
//! 256 weights in 74 bytes: f16 `d`, 32 u16 `qs`, 8 u8 `scales`. Each `qs` is one group of 8
//! weights: its low 9 bits index the 512-entry grid, its high 7 bits index `ksigns_iq2xs`. Each
//! `scales` byte holds two 4-bit scales, low nibble for the first two groups of a 32-weight
//! sub-block and high nibble for the last two — sixteen scales per block, so IQ2_XS is IQ2_XXS with
//! twice the scale resolution and a 512-entry grid instead of 256 (2.3125 vs 2.0625 bpw).
//!
//! The scale is `d · (0.5 + nibble) · 0.25`, the same half-offset as IQ2_XXS and for the same reason.
//!
//! Ported from llama.cpp `dequantize_row_iq2_xs` (ggml/src/ggml-quants.c); the reference vectors in
//! the tests were produced by gguf-py's `IQ2_XS.dequantize_blocks` (the same tables, numpy), so the
//! two implementations are compared, not one of them trusted.

use crate::iqgrids::{IQ2XS_GRID, KSIGNS_IQ2XS, KMASK_IQ2XS};

pub const IQ2_XS_BLOCK: usize = 74;

fn f16(lo: u8, hi: u8) -> f32 {
    let h = u16::from_le_bytes([lo, hi]) as u32;
    let (sign, exp, mant) = ((h >> 15) & 1, (h >> 10) & 0x1F, h & 0x3FF);
    let bits = if exp == 0 {
        if mant == 0 { sign << 31 } else {
            let (mut e, mut m) = (-14i32, mant);
            while m & 0x400 == 0 { m <<= 1; e -= 1; }
            (sign << 31) | (((e + 127) as u32) << 23) | ((m & 0x3FF) << 13)
        }
    } else if exp == 0x1F { (sign << 31) | 0x7F80_0000 | (mant << 13) }
    else { (sign << 31) | ((exp + 112) << 23) | (mant << 13) };
    f32::from_bits(bits)
}

/// Dequantize IQ2_XS.
pub fn dequant_iq2_xs(raw: &[u8], n: usize) -> Result<Vec<f32>, String> {
    if n % 256 != 0 { return Err(format!("IQ2_XS needs a multiple of 256 weights, got {n}")) }
    let blocks = n / 256;
    if raw.len() < blocks * IQ2_XS_BLOCK {
        return Err(format!("IQ2_XS: {} bytes for {blocks} blocks, need {}", raw.len(), blocks * IQ2_XS_BLOCK));
    }
    let mut out = vec![0.0f32; n];
    for b in 0..blocks {
        let p = b * IQ2_XS_BLOCK;
        let d = f16(raw[p], raw[p + 1]);
        let qs = &raw[p + 2..p + 66];
        let scales = &raw[p + 66..p + 74];
        for ib32 in 0..8 {
            // two 4-bit scales per 32-weight sub-block: low nibble → groups 0,1; high → groups 2,3
            let db = [
                d * (0.5 + (scales[ib32] & 0xF) as f32) * 0.25,
                d * (0.5 + (scales[ib32] >> 4) as f32) * 0.25,
            ];
            for l in 0..4 {
                let q = u16::from_le_bytes([qs[8 * ib32 + 2 * l], qs[8 * ib32 + 2 * l + 1]]);
                let g = IQ2XS_GRID[(q & 511) as usize].to_le_bytes();
                let signs = KSIGNS_IQ2XS[(q >> 9) as usize];
                let base = b * 256 + ib32 * 32 + l * 8;
                for j in 0..8 {
                    let s = if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
                    out[base + j] = db[l / 2] * g[j] as f32 * s;
                }
            }
        }
    }
    Ok(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A block with structure in every field — every grid index, several sign selectors, all
    /// sixteen scale nibbles — against gguf-py's `IQ2_XS.dequantize_blocks` output. The bytes and
    /// the reference were produced by the script in the test's body comment; the reference is
    /// checked at sampled positions covering both nibbles of a scale byte, a negative sign, and
    /// the last group of the block.
    #[test]
    fn matches_gguf_py_on_a_structured_block() {
        // python3: raw = bytes([0x00,0x3c]) + bytes(sum(([(i*37+5)&0xff, ((i*37+5)>>8)&0xff] for i in range(32)), []))
        //          + bytes([(i*0x31+0x07)&0xff for i in range(8)])
        //          ref = gguf.quants.IQ2_XS.dequantize_blocks(np.frombuffer(raw, np.uint8).reshape(1,74))[0]
        let mut raw = vec![0u8; IQ2_XS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C; // d = 1.0
        for i in 0..32u32 { let v = i * 37 + 5; raw[2 + 2 * i as usize] = (v & 0xFF) as u8; raw[3 + 2 * i as usize] = ((v >> 8) & 0xFF) as u8; }
        for i in 0..8u32 { raw[66 + i as usize] = ((i * 0x31 + 0x07) & 0xFF) as u8; }
        let v = dequant_iq2_xs(&raw, 256).unwrap();
        assert_eq!(v.len(), 256);
        let mut worst = 0f32;
        for (i, (got, want)) in v.iter().zip(GGUF_PY_REF).enumerate() {
            assert!((got - want).abs() <= 1e-6 * want.abs().max(1.0), "v[{i}] = {got} vs gguf-py {want}");
            worst = worst.max((got - want).abs());
        }
        assert_eq!(worst, 0.0, "the two decoders agree bit for bit on this block");
        // the block has structure: both signs, a spread of magnitudes
        assert!(v.iter().any(|x| *x < 0.0) && v.iter().any(|x| *x > 100.0), "fixture is not degenerate");
        // and two blocks decode as two copies (block stride is 74, not 66)
        let mut two = raw.clone(); two.extend_from_slice(&raw);
        let v2 = dequant_iq2_xs(&two, 512).unwrap();
        assert_eq!(&v2[..256], &v[..]); assert_eq!(&v2[256..], &v[..]);
    }

    /// gguf-py `quants.dequantize(raw, GGMLQuantizationType.IQ2_XS)` (gguf 0.19.0, numpy) on the
    /// block above; 256 values, row-major.
    const GGUF_PY_REF: [f32; 256] = [
    46.875, 15.0, 46.875, 15.0, 15.0, 15.0, 15.0, 15.0,
    15.0, 15.0, 15.0, 46.875, 46.875, 15.0, 15.0, 15.0,
    1.0, 5.375, 1.0, 3.125, 1.0, 3.125, 1.0, 1.0,
    1.0, 3.125, 1.0, 3.125, 1.0, 5.375, 1.0, 1.0,
    17.0, 17.0, 53.125, 91.375, 17.0, 17.0, 53.125, 17.0,
    17.0, 17.0, 17.0, 17.0, 91.375, 53.125, 53.125, 17.0,
    7.0, 37.625, 37.625, 37.625, 37.625, 7.0, 37.625, 7.0,
    7.0, 7.0, 7.0, 21.875, 7.0, 7.0, 7.0, 21.875,
    59.375, 102.125, 59.375, 19.0, 19.0, 59.375, 19.0, 59.375,
    19.0, 102.125, 102.125, 19.0, 19.0, 19.0, 59.375, 59.375,
    40.625, 40.625, 13.0, 69.875, 69.875, 69.875, 40.625, 40.625,
    69.875, 13.0, 13.0, 69.875, 13.0, 13.0, 13.0, 69.875,
    112.875, 21.0, 112.875, 21.0, 112.875, 112.875, 21.0, 112.875,
    65.625, 21.0, 65.625, 65.625, 65.625, 21.0, 112.875, 112.875,
    -59.375, 59.375, 102.125, 19.0, 19.0, 19.0, 19.0, -19.0,
    -19.0, 19.0, 102.125, 59.375, 59.375, 19.0, 19.0, -19.0,
    -23.0, 71.875, 23.0, 123.625, 23.0, 71.875, 23.0, -23.0,
    -71.875, 23.0, 23.0, 23.0, 71.875, 123.625, 23.0, -23.0,
    -25.0, 78.125, 78.125, 25.0, 78.125, 25.0, 78.125, -25.0,
    -134.375, 25.0, 78.125, 25.0, 25.0, 134.375, 78.125, -25.0,
    -25.0, 25.0, 25.0, 25.0, 78.125, 78.125, 134.375, -25.0,
    -25.0, 78.125, 78.125, 78.125, 25.0, 25.0, 25.0, -78.125,
    -31.0, 96.875, 96.875, 166.625, 31.0, 96.875, 31.0, -96.875,
    -31.0, 96.875, 31.0, 31.0, 96.875, 31.0, 96.875, -96.875,
    -145.125, 27.0, 145.125, 84.375, 27.0, 27.0, 145.125, -84.375,
    -27.0, 27.0, 84.375, 27.0, 84.375, 27.0, 27.0, -145.125,
    -5.0, 15.625, 5.0, 5.0, 5.0, 5.0, 15.625, -26.875,
    -26.875, 26.875, 26.875, 5.0, 26.875, 5.0, 26.875, -26.875,
    29.0, -29.0, 90.625, 90.625, 29.0, 29.0, 29.0, -29.0,
    155.875, -29.0, 29.0, 29.0, 155.875, 29.0, 29.0, -29.0,
    34.375, -11.0, 34.375, 11.0, 34.375, 34.375, 11.0, -11.0,
    11.0, -11.0, 11.0, 11.0, 59.125, 59.125, 11.0, -11.0,
    ];

    #[test]
    fn the_scale_nibbles_are_low_then_high() {
        // scales[0] = 0x3A: groups 0,1 of sub-block 0 use 0xA, groups 2,3 use 0x3. With grid entry 0
        // and no signs, the magnitudes differ exactly by (0.5+10)/(0.5+3).
        let mut raw = vec![0u8; IQ2_XS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        raw[66] = 0x3A;
        let v = dequant_iq2_xs(&raw, 256).unwrap();
        let g0 = IQ2XS_GRID[0].to_le_bytes();
        let j = (0..8).find(|&j| g0[j] != 0).expect("grid entry 0 has a non-zero byte");
        assert!((v[j] / v[16 + j] - 10.5 / 3.5).abs() < 1e-6, "low nibble for groups 0-1, high for 2-3: {} vs {}", v[j], v[16 + j]);
    }

    #[test]
    fn the_sign_selector_is_the_top_seven_bits_and_the_grid_the_low_nine() {
        let mut raw = vec![0u8; IQ2_XS_BLOCK];
        raw[0] = 0x00; raw[1] = 0x3C;
        // qs[0] = 0x0200 (bit 9 set): grid index 0, sign selector 1
        raw[2] = 0x00; raw[3] = 0x02;
        let v = dequant_iq2_xs(&raw, 256).unwrap();
        let g0 = IQ2XS_GRID[0].to_le_bytes();
        let signs = KSIGNS_IQ2XS[1];
        for j in 0..8 {
            let want = 0.125 * g0[j] as f32 * if signs & KMASK_IQ2XS[j] != 0 { -1.0 } else { 1.0 };
            assert_eq!(v[j], want, "j={j}");
        }
        // qs[1] = 0x01FF: grid index 511, selector 0
        let mut raw2 = raw.clone(); raw2[4] = 0xFF; raw2[5] = 0x01;
        let v2 = dequant_iq2_xs(&raw2, 256).unwrap();
        let g511 = IQ2XS_GRID[511].to_le_bytes();
        for j in 0..8 { assert_eq!(v2[8 + j], 0.125 * g511[j] as f32, "j={j}"); }
    }

    #[test]
    fn wrong_lengths_are_refused() {
        assert!(dequant_iq2_xs(&[0u8; 4], 256).is_err());
        assert!(dequant_iq2_xs(&vec![0u8; IQ2_XS_BLOCK], 300).is_err());
        assert_eq!(dequant_iq2_xs(&vec![0u8; IQ2_XS_BLOCK], 256).unwrap().len(), 256);
    }
}

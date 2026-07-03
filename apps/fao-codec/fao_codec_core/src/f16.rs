//! IEEE 754 binary16 (half-precision float) conversion.
//!
//! Half-precision floats are useful for compact storage of values where 
//! ~3-4 decimal digits of precision suffice — typical for depth maps, 
//! confidence scores, intermediate normals. They use 2 bytes vs 4 for f32.
//!
//! Range: ±6.55e4. Precision: ~0.1% of value (relative).
//!
//! This module avoids pulling in the `half` crate, which adds a dependency 
//! for what's a small standalone conversion. The code is the standard 
//! reference algorithm with correct handling of subnormals, infinity, and 
//! NaN.
//!
//! # Examples
//!
//! ```
//! use fao_core::f16::{f32_to_f16_bits, f16_bits_to_f32};
//! 
//! let bits = f32_to_f16_bits(3.14159);
//! let back = f16_bits_to_f32(bits);
//! assert!((back - 3.14159).abs() < 0.01);
//! ```

/// Convert IEEE 754 f32 to f16 (binary16) bits.
/// 
/// Round-trip-able via `f16_bits_to_f32`. Out-of-range values clamp to 
/// signed infinity. NaN is preserved as NaN (any NaN bit pattern, not 
/// necessarily the same one).
#[inline]
pub fn f32_to_f16_bits(f: f32) -> u16 {
    let bits = f.to_bits();
    let sign = ((bits >> 31) & 0x1) as u16;
    let exp = ((bits >> 23) & 0xFF) as i32;
    let mant = bits & 0x7FFFFF;
    
    // Special: NaN, infinity
    if exp == 0xFF {
        if mant != 0 {
            // NaN — preserve at least one mantissa bit
            return (sign << 15) | 0x7E00;
        }
        // Infinity
        return (sign << 15) | 0x7C00;
    }
    
    // Compute new exponent (bias 127 → 15)
    let new_exp = exp - 127 + 15;
    
    if new_exp >= 31 {
        // Overflow → infinity
        return (sign << 15) | 0x7C00;
    } else if new_exp <= 0 {
        // Subnormal or zero
        if new_exp < -10 {
            return sign << 15;  // too small → zero
        }
        // Subnormal: shift mantissa with implicit 1 bit
        let mant_with_1 = mant | 0x800000;
        let shift = 14 - new_exp;
        let mant16 = (mant_with_1 >> shift) as u16;
        return (sign << 15) | mant16;
    }
    
    // Normal case
    let mant16 = (mant >> 13) as u16;
    (sign << 15) | ((new_exp as u16) << 10) | mant16
}

/// Convert f16 (binary16) bits back to IEEE 754 f32.
#[inline]
pub fn f16_bits_to_f32(bits: u16) -> f32 {
    let sign = ((bits >> 15) & 0x1) as u32;
    let exp = ((bits >> 10) & 0x1F) as u32;
    let mant = (bits & 0x3FF) as u32;
    
    let f32_bits = if exp == 0 {
        if mant == 0 {
            sign << 31  // ±0
        } else {
            // Subnormal in f16 → normal in f32
            let mut m = mant;
            let mut e: i32 = 1;
            while (m & 0x400) == 0 {
                m <<= 1;
                e -= 1;
            }
            m &= 0x3FF;
            let new_exp = (e - 15 + 127) as u32;
            (sign << 31) | (new_exp << 23) | (m << 13)
        }
    } else if exp == 0x1F {
        // ±infinity or NaN
        let m = if mant == 0 { 0 } else { 0x400000 };  // signal-NaN bit
        (sign << 31) | 0x7F800000 | m
    } else {
        // Normal
        let new_exp = exp + 127 - 15;
        (sign << 31) | (new_exp << 23) | (mant << 13)
    };
    f32::from_bits(f32_bits)
}

#[cfg(test)]
mod tests {
    use super::*;
    
    #[test]
    fn integer_powers_of_two_exact() {
        // f16 represents these exactly.
        for v in [1.0f32, 2.0, 4.0, 0.5, 0.25, 1024.0] {
            let back = f16_bits_to_f32(f32_to_f16_bits(v));
            assert_eq!(back, v, "exact representation expected for {}", v);
        }
    }
    
    #[test]
    fn typical_values_within_tolerance() {
        for v in [3.14159f32, -100.5, 0.001, 1e3, 0.9, 0.123] {
            let back = f16_bits_to_f32(f32_to_f16_bits(v));
            let rel_err = (back - v).abs() / v.abs().max(1e-9);
            assert!(rel_err < 0.001, "rel_err {} for {} → {}", rel_err, v, back);
        }
    }
    
    #[test]
    fn zero_signs_preserved() {
        assert_eq!(f16_bits_to_f32(f32_to_f16_bits(0.0)), 0.0);
        let neg_zero = f16_bits_to_f32(f32_to_f16_bits(-0.0));
        assert_eq!(neg_zero, 0.0);
        assert!(neg_zero.is_sign_negative());
    }
    
    #[test]
    fn overflow_to_infinity() {
        let huge = 1e10f32;
        let back = f16_bits_to_f32(f32_to_f16_bits(huge));
        assert!(back.is_infinite() && back > 0.0);
    }
    
    #[test]
    fn underflow_to_zero() {
        let tiny = 1e-30f32;
        let back = f16_bits_to_f32(f32_to_f16_bits(tiny));
        assert_eq!(back, 0.0);
    }
}

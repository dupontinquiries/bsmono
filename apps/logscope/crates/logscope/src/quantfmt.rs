//! quantfmt.rs — GGUF quantization-format layouts: block sizes and byte footprints for every ggml type.
//!
//! This is the *sizing* counterpart to the engine's dequantizer (which turns bytes into floats): given a
//! `ggml_type`, what's its block shape and how many bytes does a tensor of N weights occupy? That's what
//! memory planning, mmap offset math, and load-time validation need — including for quants the engine
//! can't yet decode (you still must size them correctly to map the file). Objective facts (the published
//! ggml block layouts), pure and unit-tested. `QK_K = 256` for K-quants, 32 for the legacy blocks.
#![allow(dead_code)]

/// A quant type's block shape.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Layout { pub name: &'static str, pub block_elems: u32, pub block_bytes: u32 }

/// Layout for a `ggml_type` id, or `None` if unknown to this table.
pub fn layout(ggml_type: u32) -> Option<Layout> {
    let (name, be, bb) = match ggml_type {
        0  => ("F32",  1, 4),
        1  => ("F16",  1, 2),
        2  => ("Q4_0", 32, 18),  // d(f16) + 16×2 nibbles
        3  => ("Q4_1", 32, 20),  // d + m + 16
        6  => ("Q5_0", 32, 22),  // d + qh(4) + 16
        7  => ("Q5_1", 32, 24),  // d + m + qh(4) + 16
        8  => ("Q8_0", 32, 34),  // d + 32×i8
        9  => ("Q8_1", 32, 36),  // d + s + 32×i8
        10 => ("Q2_K", 256, 84),
        11 => ("Q3_K", 256, 110),
        12 => ("Q4_K", 256, 144),
        13 => ("Q5_K", 256, 176),
        14 => ("Q6_K", 256, 210),
        15 => ("Q8_K", 256, 292),
        _  => return None,
    };
    Some(Layout { name, block_elems: be, block_bytes: bb })
}

/// Bytes a tensor of `n_elems` weights occupies in this quant. `None` if the type is unknown or the count
/// isn't a whole number of blocks (GGUF tensors are always block-aligned per row).
pub fn tensor_bytes(ggml_type: u32, n_elems: u64) -> Option<u64> {
    let l = layout(ggml_type)?;
    let be = l.block_elems as u64;
    if n_elems % be != 0 { return None; }
    Some(n_elems / be * l.block_bytes as u64)
}

/// Effective bits per weight (for reporting / comparing quant levels).
pub fn bits_per_weight(ggml_type: u32) -> Option<f32> {
    let l = layout(ggml_type)?;
    Some(l.block_bytes as f32 * 8.0 / l.block_elems as f32)
}

/// Whether this id is a known quantized (block) format, as opposed to plain F32/F16 or unknown.
pub fn is_quantized(ggml_type: u32) -> bool {
    matches!(ggml_type, 2 | 3 | 6 | 7 | 8 | 9 | 10..=15)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legacy_and_k_quant_block_sizes_match_ggml() {
        assert_eq!(layout(0).unwrap().block_bytes, 4);   // F32
        assert_eq!(layout(1).unwrap().block_bytes, 2);   // F16
        assert_eq!(layout(2).unwrap(), Layout { name: "Q4_0", block_elems: 32, block_bytes: 18 });
        assert_eq!(layout(8).unwrap().block_bytes, 34);  // Q8_0
        assert_eq!(layout(12).unwrap().block_bytes, 144); // Q4_K
        assert_eq!(layout(14).unwrap().block_bytes, 210); // Q6_K
        assert!(layout(999).is_none());
    }

    #[test]
    fn bits_per_weight_matches_published_numbers() {
        let bpw = |t| (bits_per_weight(t).unwrap() * 1000.0).round() / 1000.0;
        assert_eq!(bpw(2), 4.5);    // Q4_0 = 18·8/32
        assert_eq!(bpw(8), 8.5);    // Q8_0
        assert_eq!(bpw(12), 4.5);   // Q4_K
        assert_eq!(bpw(14), 6.563); // Q6_K = 210·8/256
        assert_eq!(bpw(0), 32.0);   // F32
    }

    #[test]
    fn tensor_bytes_counts_whole_blocks() {
        // a 4096×4096 Q4_0 weight: (4096·4096/32) blocks × 18 bytes
        let n = 4096u64 * 4096;
        assert_eq!(tensor_bytes(2, n), Some(n / 32 * 18));
        // not block-aligned ⇒ None
        assert_eq!(tensor_bytes(2, 40), None);
        // F16 is per-element
        assert_eq!(tensor_bytes(1, 1000), Some(2000));
    }

    #[test]
    fn is_quantized_excludes_float_and_unknown() {
        assert!(is_quantized(2) && is_quantized(12) && is_quantized(14));
        assert!(!is_quantized(0) && !is_quantized(1) && !is_quantized(999));
    }
}

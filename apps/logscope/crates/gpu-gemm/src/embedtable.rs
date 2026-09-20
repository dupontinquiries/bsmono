//! embedtable — the host-side token embedding, held the way the file holds it.
//!
//! `Model.embed` was a `Vec<f32>` of the WHOLE table, dequantized at load and kept for the life of
//! the model so each token could read one row of it. At Qwen3.5 / Ornith's 248,320 x 4096 that is
//! 4.07 GB of host RAM (2.1 GB for a Llama-3-sized vocabulary); in the file the same tensor is
//! 570-830 MB (Q4_K / Q6_K). On a unified-memory machine those gigabytes come straight out of what
//! the weights and the page cache could be using — and memory pressure is paid in swapped pages,
//! evicted file cache and the I/O that refills it, which is energy long before it is a crash.
//!
//! [`EmbedTable::Raw`] keeps the tensor's bytes and dequantizes ONE ROW per lookup (4096 weights:
//! microseconds against a token's milliseconds). It is EXACT: a block-quantized row dequantized
//! alone equals that row of the whole-tensor dequantization, because a block never straddles two
//! rows when the width is a multiple of the block size — which `from_raw` checks rather than
//! assumes, falling back to f32 when it is not so. `a_row_alone_is_that_row_of_the_whole_table`
//! holds that for every format the loader is likely to meet.
//!
//! The f32 form stays for sources with no raw bytes to keep (the processed-weight store, synthetic
//! models, tests) and behind `LOGSCOPE_EMBED_F32=1` as the A/B.

use std::borrow::Cow;

pub enum EmbedTable {
    /// The whole table as f32 — what `Model.embed` always was.
    F32(Vec<f32>),
    /// The tensor's own bytes. `row_bytes` whole blocks per row; rows contiguous.
    Raw { ty: u32, bytes: Vec<u8>, d: usize, rows: usize, row_bytes: usize },
}

impl EmbedTable {
    /// Keep `bytes` (a whole `[rows, d]` tensor of ggml type `ty`) if every row can be dequantized
    /// on its own, exactly. `None` means "use the f32 table": an unsupported type, a width that is
    /// not whole blocks, or a byte count that does not match.
    pub fn from_raw(ty: u32, bytes: &[u8], rows: usize, d: usize) -> Option<EmbedTable> {
        if rows == 0 || d == 0 { return None; }
        let row_bytes = crate::gguf::type_nbytes(ty, d)?;
        // whole blocks per row: the row's bytes times `rows` must be the tensor's bytes, which is
        // false exactly when a block would straddle a row boundary
        if crate::gguf::type_nbytes(ty, rows * d)? != row_bytes * rows || bytes.len() < row_bytes * rows { return None; }
        // and the type must actually dequantize (type_nbytes knows sizes dequantize does not implement)
        crate::gguf::dequantize(ty, &bytes[..row_bytes], d).ok()?;
        Some(EmbedTable::Raw { ty, bytes: bytes[..row_bytes * rows].to_vec(), d, rows, row_bytes })
    }

    /// Length in FLOATS (`vocab * d`) — the quantity every caller's bounds check was written against.
    pub fn len(&self) -> usize {
        match self { EmbedTable::F32(v) => v.len(), EmbedTable::Raw { d, rows, .. } => d * rows }
    }
    pub fn is_empty(&self) -> bool { self.len() == 0 }

    /// `&table[start..start + len]` as it was written at every call site: one row, addressed in
    /// floats. Borrowed from an f32 table, dequantized from a raw one.
    pub fn slice(&self, start: usize, len: usize) -> Cow<'_, [f32]> {
        match self {
            EmbedTable::F32(v) => Cow::Borrowed(&v[start..start + len]),
            EmbedTable::Raw { ty, bytes, d, rows, row_bytes } => {
                assert!(len == *d && start % *d == 0 && start / *d < *rows,
                    "embedding lookup [{start}..{}] is not one row of a {rows} x {d} table", start + len);
                let r = start / *d;
                Cow::Owned(crate::gguf::dequantize(*ty, &bytes[r * row_bytes..(r + 1) * row_bytes], *d)
                    .expect("from_raw checked that this type dequantizes"))
            }
        }
    }

    /// Rows `r0..r1` of a `d`-wide table as f32. For a raw table this is one dequantize over those
    /// rows' bytes — the same values `to_f32()[r0*d .. r1*d]` holds, without the rest of the table
    /// ever existing.
    pub fn rows_f32(&self, r0: usize, r1: usize, d: usize) -> Cow<'_, [f32]> {
        match self {
            EmbedTable::F32(v) => Cow::Borrowed(&v[r0 * d..r1 * d]),
            EmbedTable::Raw { ty, bytes, d: td, rows, row_bytes } => {
                assert!(d == *td && r0 <= r1 && r1 <= *rows, "rows {r0}..{r1} x {d} of a {rows} x {td} table");
                Cow::Owned(crate::gguf::dequantize(*ty, &bytes[r0 * row_bytes..r1 * row_bytes], (r1 - r0) * d)
                    .expect("from_raw checked that this type dequantizes"))
            }
        }
    }
    /// (rows, width) of a raw table.
    pub fn raw_shape(&self) -> Option<(usize, usize)> { match self { EmbedTable::Raw { d, rows, .. } => Some((*rows, *d)), _ => None } }

    /// The whole table as f32 — for the few callers that need all of it (building a GPU-resident
    /// copy, a tied output matrix, the weight store). Transient for a raw table.
    pub fn to_f32(&self) -> Cow<'_, [f32]> {
        match self {
            EmbedTable::F32(v) => Cow::Borrowed(v),
            EmbedTable::Raw { ty, bytes, d, rows, .. } =>
                Cow::Owned(crate::gguf::dequantize(*ty, bytes, d * rows).expect("from_raw checked that this type dequantizes")),
        }
    }

    /// Bytes this table holds on the host.
    pub fn host_bytes(&self) -> usize {
        match self { EmbedTable::F32(v) => v.len() * 4, EmbedTable::Raw { bytes, .. } => bytes.len() }
    }
    pub fn is_raw(&self) -> bool { matches!(self, EmbedTable::Raw { .. }) }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn raw(ty: u32, n: usize, seed: u32) -> Vec<u8> {
        // (block weights, block bytes, offsets of f16 scale fields to make normal)
        let (bw, bb, scales): (usize, usize, &[usize]) = match ty {
            1 => (1, 2, &[0]), 8 => (32, 34, &[0]), 2 => (32, 18, &[0]),
            12 => (256, 144, &[0, 2]), 13 => (256, 176, &[0, 2]), 14 => (256, 210, &[208]), _ => unreachable!() };
        let mut x = seed | 1;
        let mut v: Vec<u8> = (0..n / bw * bb).map(|_| { x = x.wrapping_mul(1664525).wrapping_add(1013904223); (x >> 24) as u8 }).collect();
        for (b, blk) in v.chunks_mut(bb).enumerate() { for &o in scales { let h = 0x2800u16 + ((b * 37 + o) % 0x0700) as u16; blk[o] = h as u8; blk[o + 1] = (h >> 8) as u8; } }
        v
    }

    /// The property the whole module rests on: a row dequantized ALONE is bit-for-bit that row of
    /// the whole-table dequantization — first, last and middle rows, every format.
    #[test]
    fn a_row_alone_is_that_row_of_the_whole_table() {
        for (ty, name) in [(1u32, "F16"), (8, "Q8_0"), (2, "Q4_0"), (12, "Q4_K"), (13, "Q5_K"), (14, "Q6_K")] {
            let (rows, d) = (37usize, 512usize);
            let bytes = raw(ty, rows * d, 0xC0FFEE ^ ty);
            let whole = crate::gguf::dequantize(ty, &bytes, rows * d).expect(name);
            let t = EmbedTable::from_raw(ty, &bytes, rows, d).unwrap_or_else(|| panic!("{name}: should be held raw"));
            assert!(t.is_raw() && t.len() == rows * d, "{name}");
            for r in [0usize, 1, 17, 36] {
                let got = t.slice(r * d, d);
                assert!(got.iter().zip(&whole[r * d..(r + 1) * d]).all(|(a, b)| a.to_bits() == b.to_bits()), "{name} row {r}");
            }
            assert!(t.to_f32().iter().zip(&whole).all(|(a, b)| a.to_bits() == b.to_bits()), "{name}: to_f32");
            assert!(t.host_bytes() * 3 < whole.len() * 4 || ty == 1, "{name}: raw should be far smaller than f32 ({} vs {})", t.host_bytes(), whole.len() * 4);
        }
    }

    /// A RANGE of rows is those rows of the whole table, bit for bit — including ranges that start
    /// and end mid-table, a single row, and an empty range.
    #[test]
    fn a_range_of_rows_is_that_range_of_the_whole_table() {
        for ty in [8u32, 12, 14] {
            let (rows, d) = (23usize, 256usize);
            let bytes = raw(ty, rows * d, 77 ^ ty);
            let whole = crate::gguf::dequantize(ty, &bytes, rows * d).unwrap();
            let t = EmbedTable::from_raw(ty, &bytes, rows, d).unwrap();
            for (r0, r1) in [(0usize, 23usize), (0, 1), (5, 17), (22, 23), (9, 9)] {
                let got = t.rows_f32(r0, r1, d);
                assert!(got.len() == (r1 - r0) * d && got.iter().zip(&whole[r0 * d..r1 * d]).all(|(a, b)| a.to_bits() == b.to_bits()), "type {ty} rows {r0}..{r1}");
            }
            assert_eq!(t.raw_shape(), Some((rows, d)));
            let f = EmbedTable::F32(whole.clone());
            assert_eq!(f.raw_shape(), None);
            assert_eq!(&f.rows_f32(5, 7, d)[..], &whole[5 * d..7 * d]);
        }
    }

    /// What must NOT be held raw, and what a lookup must refuse.
    #[test]
    fn a_table_that_cannot_be_read_row_by_row_stays_f32() {
        let bytes = raw(12, 4 * 256, 9);
        // d = 128 is half a Q4_K block: a block would straddle two rows, so a row has no bytes of its own
        assert!(EmbedTable::from_raw(12, &bytes, 8, 128).is_none());
        assert!(EmbedTable::from_raw(12, &bytes[..100], 4, 256).is_none(), "short buffer");
        assert!(EmbedTable::from_raw(9999, &bytes, 4, 256).is_none(), "unknown type");
        assert!(EmbedTable::from_raw(12, &bytes, 0, 256).is_none());
        let f = EmbedTable::F32((0..12).map(|i| i as f32).collect());
        assert_eq!(&f.slice(4, 4)[..], &[4.0, 5.0, 6.0, 7.0]);
        assert_eq!((f.len(), f.host_bytes(), f.is_raw()), (12, 48, false));
        let t = EmbedTable::from_raw(12, &bytes, 4, 256).unwrap();
        for (start, len) in [(256usize, 128usize), (100, 256), (4 * 256, 256)] {
            let r = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| t.slice(start, len).len()));
            assert!(r.is_err(), "slice({start}, {len}) is not one row and must be refused");
        }
    }
}

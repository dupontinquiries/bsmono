//! qkvsplit.rs — splitting a fused `attn_qkv` into q, k and v.
//!
//! Reported from a real load: `blk.0 has no attn_q.weight. What it DOES have: [attn_qkv [4096, 8192],
//! attn_norm [4096]]`. The loader named the problem correctly and could not proceed.
//!
//! ## The layout question, and why it is the whole job
//!
//! A fused QKV tensor is rows of `q`, then rows of `k`, then rows of `v` — **blocked**, not interleaved
//! per head. That distinction is exactly what the MLA `kv_b` fusion got wrong in the other direction,
//! where the layout WAS per-head and a blocked concatenation looked right for head 0 and was wrong for
//! every head after.
//!
//! So the split is arithmetic, and the arithmetic is checkable: `q_rows + k_rows + v_rows` must equal
//! the tensor's row count exactly. If it does not, the layout is not what we assumed and the right
//! answer is to REFUSE rather than to slice something plausible — a wrong split produces attention over
//! the wrong projections, which is coherent-looking garbage rather than an error.
//!
//! ## GQA makes the three parts different sizes
//!
//! With `n_head` query heads and `n_kv` key/value heads sharing `head_dim`, q is `n_head * head_dim`
//! rows and k and v are `n_kv * head_dim` each. Assuming three equal thirds is the obvious mistake and
//! is wrong for every GQA model, which is most of them now.

/// Where each projection lives in the fused tensor.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct Split {
    /// Row ranges, in units of rows (not bytes).
    pub q: (usize, usize),
    pub k: (usize, usize),
    pub v: (usize, usize),
}

/// Why a fused tensor could not be split.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum SplitError {
    /// The parts do not sum to the tensor.
    RowMismatch { declared: usize, actual: usize },
    /// A head count or dimension is zero.
    BadDims,
}

impl SplitError {
    pub fn explain(&self) -> String {
        match self {
            SplitError::RowMismatch { declared, actual } => format!(
                "the head counts describe {declared} rows and the fused tensor has {actual}. The \
layout is not q|k|v blocked as assumed, so this refuses rather than slicing something plausible — a \
wrong split produces attention over the wrong projections, which reads as a working model writing \
nonsense"),
            SplitError::BadDims => "a head count or head_dim is zero, so no split can be computed".into(),
        }
    }
}

/// Compute the split from the declared shapes.
///
/// `rows` is the fused tensor's row count. Everything else comes from metadata, so a disagreement
/// between them is exactly the signal worth stopping on.
pub fn split(rows: usize, n_head: usize, n_kv: usize, head_dim: usize) -> Result<Split, SplitError> {
    if n_head == 0 || n_kv == 0 || head_dim == 0 { return Err(SplitError::BadDims); }
    let q_rows = n_head * head_dim;
    let kv_rows = n_kv * head_dim;
    let declared = q_rows + 2 * kv_rows;
    if declared != rows { return Err(SplitError::RowMismatch { declared, actual: rows }); }
    Ok(Split {
        q: (0, q_rows),
        k: (q_rows, q_rows + kv_rows),
        v: (q_rows + kv_rows, declared),
    })
}

impl Split {
    pub fn q_rows(&self) -> usize { self.q.1 - self.q.0 }
    pub fn k_rows(&self) -> usize { self.k.1 - self.k.0 }
    pub fn v_rows(&self) -> usize { self.v.1 - self.v.0 }

    /// Byte range for a row span, given a per-row stride.
    ///
    /// Kept separate from the row arithmetic because the stride depends on the quantization and
    /// mixing the two is how a Q6_K tensor gets sliced at a Q4_K stride.
    pub fn byte_range(span: (usize, usize), row_bytes: usize) -> (usize, usize) {
        (span.0 * row_bytes, span.1 * row_bytes)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_GQA_model_splits_into_THREE_UNEQUAL_parts() {
        // Assuming three equal thirds is the obvious mistake and is wrong for every GQA model, which
        // is most of them now. 32 query heads and 8 kv heads at head_dim 128: 4096 + 1024 + 1024.
        let s = split(6144, 32, 8, 128).expect("sums");
        assert_eq!(s.q_rows(), 4096);
        assert_eq!(s.k_rows(), 1024);
        assert_eq!(s.v_rows(), 1024);
        assert_ne!(s.q_rows(), s.k_rows(), "not thirds");
    }

    #[test]
    fn the_parts_are_CONTIGUOUS_and_cover_the_tensor() {
        // Blocked, not interleaved — the distinction the MLA kv_b fusion got wrong in the other
        // direction, where a blocked concatenation looked right for head 0 and was wrong after it.
        let s = split(6144, 32, 8, 128).unwrap();
        assert_eq!(s.q.1, s.k.0);
        assert_eq!(s.k.1, s.v.0);
        assert_eq!(s.v.1, 6144);
    }

    #[test]
    fn a_row_count_that_DISAGREES_with_the_metadata_refuses() {
        // The check that matters. A wrong split produces attention over the wrong projections — a model
        // that loads, runs, and writes nonsense, which is the failure this codebase keeps paying for.
        let e = split(8192, 32, 8, 128).unwrap_err();
        assert_eq!(e, SplitError::RowMismatch { declared: 6144, actual: 8192 });
        assert!(e.explain().contains("reads as a working model writing nonsense"));
    }

    #[test]
    fn multi_query_attention_is_a_valid_split() {
        // n_kv = 1. The arithmetic must not assume k and v are large.
        let s = split(4096 + 128 + 128, 32, 1, 128).unwrap();
        assert_eq!(s.k_rows(), 128);
        assert_eq!(s.v_rows(), 128);
    }

    #[test]
    fn full_attention_gives_three_equal_parts_and_that_is_the_special_case() {
        // n_kv == n_head. Worth a test because it is the case where a thirds assumption would have
        // passed, which is why the bug would survive a casual check.
        let s = split(3 * 4096, 32, 32, 128).unwrap();
        assert_eq!(s.q_rows(), s.k_rows());
        assert_eq!(s.k_rows(), s.v_rows());
    }

    #[test]
    fn zero_dims_are_refused_rather_than_producing_empty_spans() {
        // An empty span slices nothing and reads as a tensor of zeros — plausible weights again.
        assert_eq!(split(0, 0, 8, 128), Err(SplitError::BadDims));
        assert_eq!(split(6144, 32, 0, 128), Err(SplitError::BadDims));
        assert_eq!(split(6144, 32, 8, 0), Err(SplitError::BadDims));
    }

    #[test]
    fn byte_ranges_are_computed_from_a_STRIDE_not_assumed() {
        // Kept separate from the row arithmetic because the stride depends on quantization — mixing
        // the two is how a Q6_K tensor gets sliced at a Q4_K stride, which happened in the expert path.
        let s = split(6144, 32, 8, 128).unwrap();
        assert_eq!(Split::byte_range(s.k, 72), (4096 * 72, 5120 * 72));
    }
}

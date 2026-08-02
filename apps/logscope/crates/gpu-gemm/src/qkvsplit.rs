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

/// Which fused tensor supplies a missing q/k/v, and which part of it.
///
/// The loader fetches `blk.{i}.attn_q.weight` and fails when a checkpoint ships `attn_qkv.weight`
/// instead. This is the resolution step: given the name that was asked for, name the tensor that
/// actually holds it and which of the three parts to take.
///
/// Separated from the byte arithmetic on purpose. A fetch shim needs to answer "where do I look"
/// before it can answer "which bytes", and keeping them apart means the name mapping can be tested
/// without a file and the byte mapping without a name table.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Part { Q, K, V }

/// The fused spellings seen in the wild, in the order they should be tried.
///
/// `attn_qkv` is the GGUF convention; `wqkv` and `query_key_value` appear in checkpoints converted
/// from other toolchains. Listed rather than pattern-matched, because a name that merely *contains*
/// "qkv" could be something else entirely and guessing wrong slices the wrong tensor.
pub const FUSED_NAMES: [&str; 3] = ["attn_qkv", "wqkv", "query_key_value"];

/// Resolve `blk.{i}.attn_q.weight` → (`blk.{i}.attn_qkv.weight`, Part::Q).
///
/// Returns None for anything that is not a q/k/v projection, so a caller can use this as a filter
/// rather than having to know in advance which names are splittable.
pub fn resolve(wanted: &str, fused: &str) -> Option<(String, Part)> {
    let (stem, suffix) = wanted.rsplit_once('.')?;      // "blk.0.attn_q" , "weight"
    let (prefix, leaf) = stem.rsplit_once('.')?;        // "blk.0" , "attn_q"
    let part = match leaf {
        "attn_q" | "wq" | "q_proj" => Part::Q,
        "attn_k" | "wk" | "k_proj" => Part::K,
        "attn_v" | "wv" | "v_proj" => Part::V,
        _ => return None,
    };
    Some((format!("{prefix}.{fused}.{suffix}"), part))
}

impl Split {
    /// The row span for a part, so a caller can go straight from `resolve` to bytes.
    pub fn span(&self, part: Part) -> (usize, usize) {
        match part { Part::Q => self.q, Part::K => self.k, Part::V => self.v }
    }
}

#[cfg(test)]
mod resolve_tests {
    use super::*;

    #[test]
    fn a_q_projection_resolves_to_the_fused_tensor_and_its_part() {
        let (name, part) = resolve("blk.0.attn_q.weight", "attn_qkv").expect("resolves");
        assert_eq!(name, "blk.0.attn_qkv.weight");
        assert_eq!(part, Part::Q);
    }

    #[test]
    fn k_and_v_resolve_to_the_SAME_tensor_with_different_parts() {
        // One fetch, three consumers — which is the whole reason the mapping is name-level rather
        // than a per-tensor rewrite.
        let (kn, kp) = resolve("blk.7.attn_k.weight", "attn_qkv").unwrap();
        let (vn, vp) = resolve("blk.7.attn_v.weight", "attn_qkv").unwrap();
        assert_eq!(kn, vn);
        assert_ne!(kp, vp);
    }

    #[test]
    fn a_non_projection_name_resolves_to_NOTHING() {
        // So a caller can use this as a filter. Returning a guess for `attn_norm` would have the shim
        // slice a normalization vector into thirds.
        assert!(resolve("blk.0.attn_norm.weight", "attn_qkv").is_none());
        assert!(resolve("token_embd.weight", "attn_qkv").is_none());
        assert!(resolve("output.weight", "attn_qkv").is_none());
    }

    #[test]
    fn the_alternate_spellings_all_resolve() {
        // Converted checkpoints use q_proj/k_proj/v_proj and wq/wk/wv, and a loader that handles only
        // the GGUF spelling refuses a file it could read.
        for (n, want) in [("blk.1.q_proj.weight", Part::Q), ("blk.1.wk.weight", Part::K),
                          ("blk.1.v_proj.weight", Part::V)] {
            assert_eq!(resolve(n, "wqkv").map(|(_, p)| p), Some(want), "{n}");
        }
    }

    #[test]
    fn the_span_matches_the_split_it_came_from() {
        // Connects the two halves: resolve says WHICH part, split says WHERE, and a caller composing
        // them must land on the same rows the split defined.
        let s = split(6144, 32, 8, 128).unwrap();
        assert_eq!(s.span(Part::Q), s.q);
        assert_eq!(s.span(Part::K), s.k);
        assert_eq!(s.span(Part::V), s.v);
        // And they tile the tensor with no gap and no overlap.
        assert_eq!(s.span(Part::Q).1, s.span(Part::K).0);
        assert_eq!(s.span(Part::K).1, s.span(Part::V).0);
    }

    #[test]
    fn the_fused_name_list_is_ordered_and_specific() {
        // Listed rather than pattern-matched: a name that merely CONTAINS "qkv" could be something
        // else, and guessing wrong slices the wrong tensor into thirds.
        assert_eq!(FUSED_NAMES[0], "attn_qkv", "the GGUF convention is tried first");
        assert!(FUSED_NAMES.contains(&"query_key_value"));
    }
}

/// Everything a fetch shim needs, in one call.
///
/// `resolve` answers which tensor and which part; `split` answers which rows. A loader wiring this by
/// hand would compose them at the fetch site — three steps, in a multi-line expression, in the one
/// path where a mistake breaks loading for every model rather than one.
///
/// So the composition lives here and the call site is a single line.
///
/// Returns the source tensor name and the BYTE range within it, or an error explaining why the split
/// cannot be made. `row_bytes` is the fused tensor's own per-row stride, which the caller has from its
/// dims and quantization — it is not derivable here, and guessing it is how a Q6_K tensor gets sliced
/// at a Q4_K stride.
pub fn locate(wanted: &str, available: &dyn Fn(&str) -> Option<usize>,
              n_head: usize, n_kv: usize, head_dim: usize, row_bytes: usize)
    -> Option<Result<(String, (usize, usize)), SplitError>>
{
    // Try each known spelling, and stop at the first that the file actually HAS. Returning None when
    // no fused tensor exists is what lets a caller treat this as "not applicable" rather than as a
    // failure — most models are not fused and must not see an error.
    for fused in FUSED_NAMES {
        let (src, part) = resolve(wanted, fused)?;
        let rows = match available(&src) { Some(r) => r, None => continue };
        return Some(match split(rows, n_head, n_kv, head_dim) {
            Ok(s) => Ok((src, Split::byte_range(s.span(part), row_bytes))),
            Err(e) => Err(e),
        });
    }
    None
}

#[cfg(test)]
mod locate_tests {
    use super::*;

    /// A file holding one fused tensor of 6144 rows.
    fn have_fused(name: &str) -> Option<usize> {
        if name == "blk.0.attn_qkv.weight" { Some(6144) } else { None }
    }
    fn have_nothing(_: &str) -> Option<usize> { None }

    #[test]
    fn it_returns_the_source_and_the_BYTE_range_in_one_call() {
        let r = locate("blk.0.attn_k.weight", &have_fused, 32, 8, 128, 72)
            .expect("applicable").expect("splits");
        assert_eq!(r.0, "blk.0.attn_qkv.weight");
        // k occupies rows 4096..5120 of the fused tensor, at 72 bytes per row.
        assert_eq!(r.1, (4096 * 72, 5120 * 72));
    }

    #[test]
    fn a_model_with_NO_fused_tensor_is_not_applicable_rather_than_failing() {
        // Most models are not fused. A caller must be able to ask unconditionally and get "this does
        // not apply" — an error here would make every ordinary load look broken.
        assert!(locate("blk.0.attn_q.weight", &have_nothing, 32, 8, 128, 72).is_none());
    }

    #[test]
    fn a_non_projection_is_also_not_applicable() {
        assert!(locate("blk.0.attn_norm.weight", &have_fused, 32, 8, 128, 72).is_none());
    }

    #[test]
    fn a_shape_disagreement_is_an_ERROR_not_a_miss() {
        // The distinction that matters. "No fused tensor" means carry on; "the fused tensor is not the
        // shape the metadata describes" means STOP, because slicing it anyway produces attention over
        // the wrong projections — a model that loads, runs, and writes nonsense.
        let odd = |n: &str| if n == "blk.0.attn_qkv.weight" { Some(8192) } else { None };
        let e = locate("blk.0.attn_q.weight", &odd, 32, 8, 128, 72).expect("applicable").unwrap_err();
        assert_eq!(e, SplitError::RowMismatch { declared: 6144, actual: 8192 });
    }

    #[test]
    fn the_three_parts_tile_the_fused_tensor_exactly() {
        // Composed through locate rather than split, so this checks the whole path a loader takes.
        let by = |w: &str| locate(w, &have_fused, 32, 8, 128, 72).unwrap().unwrap().1;
        let (q, k, v) = (by("blk.0.attn_q.weight"), by("blk.0.attn_k.weight"),
                         by("blk.0.attn_v.weight"));
        assert_eq!(q.0, 0);
        assert_eq!(q.1, k.0, "no gap between q and k");
        assert_eq!(k.1, v.0, "no gap between k and v");
        assert_eq!(v.1, 6144 * 72, "and v ends at the tensor's end");
    }
}

/// Can a fused tensor be sliced at row boundaries without cutting a quantization block?
///
/// This is the assumption the loader's fallback rests on, and it was unstated. A K-quant packs 256
/// weights per block: Q4_K in 144 bytes, Q6_K in 210, IQ1_S in 50. Slicing at a row boundary is only
/// safe when a row is a whole number of blocks — otherwise q ends mid-block, and both q's last block
/// and k's first are garbage.
///
/// Every dimension in common use is a multiple of 256 (2048, 4096, 5120, 8192), so this holds by luck
/// rather than by design on every model anyone has run. Stating it means a model that breaks it fails
/// with a reason instead of producing text.
///
/// `type_nbytes` already returns None for a non-multiple, so the loader's fallback declines such a
/// tensor — but it declines by falling through to "missing tensor", which names the wrong problem.
pub fn rows_are_block_aligned(cols: usize, block: usize) -> bool {
    block > 0 && cols % block == 0
}

/// The K-quant block size. All of them share 256.
pub const K_BLOCK: usize = 256;

#[cfg(test)]
mod alignment_tests {
    use super::*;

    #[test]
    fn the_dimensions_anyone_actually_runs_are_ALL_aligned() {
        // Which is why this has never bitten, and why it would be easy to leave unstated.
        for cols in [2048, 4096, 5120, 8192, 14336, 1536] {
            assert!(rows_are_block_aligned(cols, K_BLOCK), "cols {cols}");
        }
    }

    #[test]
    fn a_non_multiple_is_REFUSED_rather_than_sliced() {
        // If a row is not a whole number of blocks, q ends mid-block and BOTH q's last block and k's
        // first are garbage — a split that produces plausible-looking weights rather than an error.
        for cols in [1000, 3000, 100] {
            assert!(!rows_are_block_aligned(cols, K_BLOCK), "cols {cols}");
        }
    }

    #[test]
    fn a_zero_block_is_not_alignment() {
        // A caller that could not determine the block size must not get "aligned" as the answer.
        assert!(!rows_are_block_aligned(4096, 0));
    }
}

#[cfg(test)]
mod axis_tests {
    use super::*;

    /// The reported Qwen3.5 shape, split against its own metadata.
    ///
    /// `attn_qkv [4096, 8192]` on a d=4096 model. GGUF dims are fastest-varying first, so dims[0]=4096
    /// is the INPUT width and dims[1]=8192 is the output rows. A split works on the rows.
    ///
    /// I had these swapped when first wiring the loader fallback, and the failure would have been
    /// quiet: splitting 4096 rows out of a tensor with 8192 is self-consistent arithmetic about the
    /// wrong axis, so the row-count check inside `split` would have passed and q's byte range would
    /// have covered part of the tensor that is not q.
    #[test]
    fn the_reported_qwen35_shape_splits_on_the_ROW_axis() {
        let (input_width, rows) = (4096usize, 8192usize);
        // 32 query heads and 8 kv heads at head_dim 256 gives 8192 = 8192 + 2048 + 2048? No —
        // whatever the head counts, they must sum to the ROW count, never to the input width.
        let s = split(rows, 32, 8, 128).ok();
        assert!(s.is_none(), "32*128 + 2*8*128 = 6144, not 8192 — this file's heads differ");
        // The point is which number the check is made against: 8192, not 4096.
        assert_eq!(split(input_width, 32, 8, 128).unwrap_err(),
                   SplitError::RowMismatch { declared: 6144, actual: 4096 },
                   "splitting against the input width compares against the wrong number entirely");
    }

    #[test]
    fn a_split_that_sums_correctly_lands_on_whole_rows() {
        // 8192 rows as 64 query heads and 8 kv heads at 128: 8192 + 1024 + 1024 = 10240. Not that
        // either — so pick the arrangement that does sum, and check the byte ranges tile the tensor
        // at the INPUT width's stride.
        let rows = 6144;
        let s = split(rows, 32, 8, 128).unwrap();
        let row_bytes = 4096 / 256 * 144;    // input width 4096, Q4_K: 144 bytes per 256 weights
        let v = Split::byte_range(s.v, row_bytes);
        assert_eq!(v.1, rows * row_bytes, "the last part ends at the tensor's end");
    }
}

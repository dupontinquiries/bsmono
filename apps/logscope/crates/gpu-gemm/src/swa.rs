//! swa.rs — sliding-window attention masking and layer patterns.
//!
//! Ported from llama.cpp (`llama_hparams::is_masked_swa` in src/llama-hparams.h, `set_swa_pattern` in
//! src/llama-hparams.cpp), which is the reference these architectures are defined against.
//!
//! This closes a gap `archfeat` declares for three families we otherwise claim to support:
//!
//! * **Gemma 2/3/4** — `LLAMA_SWA_TYPE_STANDARD`, window 4096 on Gemma2, on a repeating layer pattern.
//!   Without it, windowed layers attend to the whole context: correct below the window, increasingly
//!   wrong beyond it.
//! * **Llama-4** — `LLAMA_SWA_TYPE_CHUNKED`, window 8192, pattern 4 (3 chunked, 1 full).
//!
//! Deliberately PURE and not yet wired into decode. The masking rule is small and exactly specifiable, so
//! it can be made correct and tested on its own; wiring it into the attention kernels is a separate,
//! larger change. `archfeat` continues to report SWA as unimplemented until that lands — a validated
//! kernel that nothing calls is not support, and saying otherwise is the mistake this whole module exists
//! to prevent.

/// Which windowing rule an architecture uses. These are NOT interchangeable: Gemma's standard window and
/// Llama-4's chunked window mask different cells for the same `n_swa`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum SwaType {
    /// No windowing — every position attends to all earlier positions.
    None,
    /// Attend only to the last `n_swa` positions (Gemma).
    Standard,
    /// Attend only within the current aligned chunk of `n_swa` positions (Llama-4).
    Chunked,
    /// Attend within +/- n_swa/2 (used by some encoders).
    Symmetric,
}

/// Is key position `p0` masked out for query position `p1`?
///
/// Mirrors `llama_hparams::is_masked_swa` exactly, including the asymmetry: `Standard` compares
/// `p1 - p0 >= n_swa`, while `Chunked` masks anything before the start of the query's chunk. A window of
/// 0 disables masking, matching llama.cpp's behaviour when `n_swa` is unset.
pub fn is_masked(n_swa: u32, ty: SwaType, p0: i64, p1: i64) -> bool {
    if n_swa == 0 || ty == SwaType::None { return false; }
    let n = n_swa as i64;
    match ty {
        SwaType::None => false,
        SwaType::Standard => p1 - p0 >= n,
        SwaType::Chunked => {
            let chunk_start = (p1 / n) * n;
            p0 < chunk_start
        }
        SwaType::Symmetric => {
            let half = n / 2;
            let d = p1 - p0;
            d > half || d < -half
        }
    }
}

/// Which layers are windowed, for a repeating pattern of length `n_pattern`.
///
/// `dense_first == false` (llama.cpp's default) makes the LAST layer of each group the full-attention
/// one: for `n_pattern = 4`, layers 0,1,2 are windowed and layer 3 is dense — Llama-4's documented
/// "3 chunked, 1 full". `n_pattern == 0` means every layer is windowed.
pub fn swa_layers(n_layer: usize, n_pattern: u32, dense_first: bool) -> Vec<bool> {
    (0..n_layer).map(|il| {
        if n_pattern == 0 { return true; }
        let p = n_pattern as usize;
        if dense_first { il % p != 0 } else { il % p < p - 1 }
    }).collect()
}

/// The earliest key position a query at `p1` may attend to, or 0 when unbounded. Useful for sizing a KV
/// read range rather than masking cell by cell.
pub fn window_start(n_swa: u32, ty: SwaType, p1: i64) -> i64 {
    if n_swa == 0 { return 0; }
    let n = n_swa as i64;
    match ty {
        SwaType::None => 0,
        SwaType::Standard => (p1 - n + 1).max(0),
        SwaType::Chunked => (p1 / n) * n,
        SwaType::Symmetric => (p1 - n / 2).max(0),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn standard_window_keeps_the_last_n_positions() {
        // window 4: a query at 10 sees 7,8,9,10 and nothing earlier
        for p0 in 0..=10i64 {
            let masked = is_masked(4, SwaType::Standard, p0, 10);
            assert_eq!(masked, p0 < 7, "p0={p0} masked={masked}");
        }
    }

    #[test]
    fn chunked_window_keeps_the_current_aligned_chunk() {
        // window 4, query at 10 -> chunk starts at 8, so 8,9,10 are visible and 7 is not.
        // This is the difference from Standard, which at p1=10 would have kept 7.
        assert!(is_masked(4, SwaType::Chunked, 7, 10));
        assert!(!is_masked(4, SwaType::Chunked, 8, 10));
        assert!(!is_masked(4, SwaType::Standard, 7, 10), "Standard keeps 7 — the two rules differ");
        // at a chunk boundary the query sees only itself
        assert!(is_masked(4, SwaType::Chunked, 7, 8));
        assert!(!is_masked(4, SwaType::Chunked, 8, 8));
    }

    #[test]
    fn a_zero_window_or_none_masks_nothing() {
        for ty in [SwaType::Standard, SwaType::Chunked, SwaType::Symmetric] {
            assert!(!is_masked(0, ty, 0, 1_000_000), "n_swa=0 must disable masking");
        }
        assert!(!is_masked(4, SwaType::None, 0, 1_000_000));
    }

    #[test]
    fn a_query_always_sees_itself() {
        for ty in [SwaType::Standard, SwaType::Chunked, SwaType::Symmetric] {
            for p in [0i64, 1, 7, 8, 4095, 4096, 100_000] {
                assert!(!is_masked(4096, ty, p, p), "{ty:?} masked its own position at {p}");
            }
        }
    }

    #[test]
    fn llama4_pattern_is_three_windowed_then_one_full() {
        // llama.cpp: swa_period = 4, "pattern: 3 chunked - 1 full", dense_first = false
        let l = swa_layers(8, 4, false);
        assert_eq!(l, vec![true, true, true, false, true, true, true, false]);
    }

    #[test]
    fn dense_first_inverts_which_layer_is_full() {
        let l = swa_layers(8, 4, true);
        assert_eq!(l, vec![false, true, true, true, false, true, true, true]);
        // a zero pattern means every layer is windowed
        assert!(swa_layers(4, 0, false).iter().all(|b| *b));
    }

    #[test]
    fn window_start_agrees_with_the_mask() {
        // the two must not drift: everything below window_start is masked, everything from it is not
        for ty in [SwaType::Standard, SwaType::Chunked] {
            for p1 in [0i64, 3, 7, 8, 9, 31, 32] {
                let s = window_start(8, ty, p1);
                for p0 in 0..=p1 {
                    assert_eq!(is_masked(8, ty, p0, p1), p0 < s,
                        "{ty:?} p0={p0} p1={p1} start={s}");
                }
            }
        }
    }

    #[test]
    fn gemma2_window_is_four_thousand_and_ninety_six() {
        // gemma2.cpp: n_swa = 4096. A query at 5000 must not see position 0.
        assert!(is_masked(4096, SwaType::Standard, 0, 5000));
        // the rule is `p1 - p0 >= n_swa`, so the boundary is exact: 5000-904 == 4096 is MASKED and 905
        // is the first visible position. Off-by-one here would silently drop a token from every window.
        assert!(is_masked(4096, SwaType::Standard, 904, 5000), "distance exactly n_swa is masked");
        assert!(!is_masked(4096, SwaType::Standard, 905, 5000), "distance n_swa-1 is visible");
        assert_eq!(window_start(4096, SwaType::Standard, 5000), 905);
    }
}

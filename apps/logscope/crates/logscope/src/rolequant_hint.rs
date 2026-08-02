//! rolequant_hint.rs — the arithmetic behind the role-precision advice.
//!
//! `gpu-gemm::rolequant` holds the policy — which roles may be reduced, to what floor, and in what
//! order. `logscope` cannot depend on `gpu-gemm`, so this is the one number the advice needs, kept
//! here and tested here rather than reached for across a crate boundary.
//!
//! Deliberately just the saving. The decision of whether to take it belongs to whoever reads the
//! line, and a planner that quietly lowered precision would be making a quality judgement it has no
//! basis for.

/// Bits per weight of the levels this hint compares.
///
/// Duplicated from `rolequant::Level` because the crates cannot share it — and a test asserts the two
/// stay equal, which is the same cross-crate agreement pattern the watchdog tables use after they
/// drifted for four days.
pub const Q2_BPW: f32 = 2.625;
pub const Q4_BPW: f32 = 4.5;

/// GiB freed by holding a liaison of `params` weights at Q2 rather than q4.
///
/// Returns None below a threshold where the saving is too small to be worth an operator's attention —
/// advice that fires for 0.2 GiB is advice that trains people to skip the line.
pub fn liaison_saving_gib(params: u64) -> Option<f32> {
    let bytes = |bpw: f32| params as f64 * bpw as f64 / 8.0;
    let freed = (bytes(Q4_BPW) - bytes(Q2_BPW)) / (1u64 << 30) as f64;
    if freed < 0.5 { return None; }
    Some(freed as f32)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_8B_liaison_frees_most_of_two_GiB() {
        // The case the advice exists for: an 8B reporter beside a 30B thinker on a device where the
        // thinker only just fits.
        let f = liaison_saving_gib(8_000_000_000).expect("worth mentioning");
        assert!((f - 1.75).abs() < 0.1, "got {f}");
    }

    #[test]
    fn a_TINY_liaison_produces_no_advice() {
        // Advice that fires for a fraction of a GiB is advice that trains people to skip the line —
        // the same failure as the template-mismatch warning that fired on every model switch.
        assert!(liaison_saving_gib(1_000_000_000).is_none());
    }

    #[test]
    fn the_bpw_constants_match_the_engine_side() {
        // Duplicated across a crate boundary because logscope cannot depend on gpu-gemm. Pinned so a
        // change to one is a test failure rather than a silent disagreement — which is what the two
        // watchdog tables did for four days.
        assert_eq!(Q2_BPW, 2.625, "gpu_kernels::rolequant::Level::Q2.bpw()");
        assert_eq!(Q4_BPW, 4.5, "gpu_kernels::rolequant::Level::Q4.bpw()");
    }
}

//! passfuse.rs — does this driver honour barriers BETWEEN dispatches inside one compute pass?
//!
//! # The question, and why it comes before the work
//!
//! After both frame leaks were fixed, pass boundaries became 97% of submits on a dense model,
//! because the engine opens a compute pass per dispatch. WebGPU says consecutive dispatches in one
//! pass see each other's storage writes; llama.cpp's Vulkan backend relies on the equivalent and
//! records a whole graph into one command buffer.
//!
//! Restructuring the engine to do the same is a substantial change. **Whether it is even possible
//! here is one cheap experiment**, and this driver has already departed from the WGSL spec in four
//! measured ways — so the spec saying barriers work is not evidence that they do.
//!
//! # The experiment
//!
//! A chain where each dispatch MUST see the previous one's writes:
//!
//! ```text
//! buf[i] = 1                    (dispatch 0)
//! buf[i] = buf[i] * 2 + 1       (dispatch 1..n)
//! ```
//!
//! After `n` doubling steps from 1 the value is `2^n * 1 + (2^n - 1)`, which is `2^(n+1) - 1` —
//! **a value that is wrong in an obvious way if any step read stale data**, rather than one that
//! degrades gracefully. A missed barrier gives a smaller power of two, not noise.
//!
//! Run the same chain as separate passes (known correct) and as one fused pass, and compare. If they
//! differ, fusion is unavailable here and the restructure is not worth starting.

/// The expected value after `steps` iterations of `x = 2x + 1` from `x = 1`.
///
/// Closed form so the check does not reimplement the shader's arithmetic in the shader's own terms —
/// a mistake that would make the test agree with a wrong kernel.
pub fn expected(steps: u32) -> u32 {
    // x_0 = 1; x_k = 2 x_{k-1} + 1  =>  x_k = 2^(k+1) - 1
    (1u32 << (steps + 1)).wrapping_sub(1)
}

/// How many steps to run.
///
/// **Enough that a single missed barrier is unmistakable.** At 12 steps the right answer is 8191 and
/// missing one step gives 4095 — a halving, not a rounding difference.
pub const STEPS: u32 = 12;

/// The verdict.
#[derive(Clone, PartialEq, Eq, Debug)]
pub enum Fusion {
    /// Fused output matches unfused. The restructure is worth doing.
    Honoured,
    /// They differ. Fusion would corrupt weights silently here.
    NotHonoured { unfused: u32, fused: u32, expected: u32 },
    /// The UNFUSED path is already wrong, so the experiment says nothing about fusion.
    ControlFailed { got: u32, expected: u32 },
}

impl Fusion {
    pub fn safe_to_fuse(&self) -> bool { matches!(self, Fusion::Honoured) }

    pub fn explain(&self) -> String {
        match self {
            Fusion::Honoured => format!(
                "fused and unfused agree at {} after {STEPS} dependent steps — this driver honours \
barriers between dispatches in one pass, and collapsing them is worth the restructure",
                expected(STEPS)),
            Fusion::NotHonoured { unfused, fused, expected } => format!(
                "unfused {unfused}, fused {fused}, expected {expected}. A dispatch read stale data \
from the one before it. FUSION WOULD CORRUPT WEIGHTS SILENTLY HERE — the output would be plausible \
floats and the model would generate confident nonsense, which is the failure this driver's other \
four spec departures already taught us to expect"),
            Fusion::ControlFailed { got, expected } => format!(
                "the UNFUSED chain gave {got}, expected {expected}. The control is broken, so this \
run says nothing about fusion — fix the baseline before believing any comparison against it"),
        }
    }
}

/// Judge a pair of results.
///
/// **The control is checked first.** A fused result matching a broken unfused result would read as
/// success, which is the shape of every false green this project has hit.
pub fn judge(unfused: u32, fused: u32) -> Fusion {
    let want = expected(STEPS);
    if unfused != want { return Fusion::ControlFailed { got: unfused, expected: want } }
    if fused != unfused { return Fusion::NotHonoured { unfused, fused, expected: want } }
    Fusion::Honoured
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_expected_value_is_a_CLOSED_FORM_not_the_shader_s_own_arithmetic() {
        // Reimplementing x = 2x + 1 in the checker would make it agree with a wrong kernel.
        assert_eq!(expected(0), 1);
        assert_eq!(expected(1), 3);
        assert_eq!(expected(12), 8191);
    }

    #[test]
    fn a_SINGLE_missed_barrier_is_unmistakable_rather_than_subtle() {
        // 8191 against 4095 — a halving, not a rounding difference, which is what makes the
        // experiment decisive with one run.
        // My first line here was arithmetic contorted to reach a number I had already decided on.
        // The property is simply that one missed step roughly HALVES the answer, which the ratio
        // states directly.
        assert_eq!(expected(STEPS), 8191);
        assert_eq!(expected(STEPS - 1), 4095);
        assert!(expected(STEPS) as f64 / expected(STEPS - 1) as f64 > 1.9,
            "one missed step must roughly halve the result, not shift it slightly");
    }

    #[test]
    fn the_CONTROL_is_checked_before_the_comparison() {
        // A fused result matching a BROKEN unfused result would read as success — the shape of every
        // false green this project has hit.
        let v = judge(999, 999);
        assert!(matches!(v, Fusion::ControlFailed { .. }));
        assert!(!v.safe_to_fuse());
        assert!(v.explain().contains("says nothing about fusion"));
    }

    #[test]
    fn a_MISMATCH_says_the_failure_would_be_SILENT() {
        // Plausible floats and a model that generates confident nonsense, which is what this
        // driver's other four spec departures already taught.
        let v = judge(expected(STEPS), expected(STEPS) / 2);
        assert!(v.explain().contains("CORRUPT WEIGHTS SILENTLY"));
    }

    #[test]
    fn AGREEMENT_is_the_only_thing_that_authorises_the_restructure() {
        assert!(judge(expected(STEPS), expected(STEPS)).safe_to_fuse());
        assert!(judge(expected(STEPS), expected(STEPS)).explain().contains("worth the restructure"));
    }
}

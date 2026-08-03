//! sensitivity.rs — which experts can be demoted, measured rather than guessed.
//!
//! ## The signal, and why frequency is the wrong one
//!
//! The obvious rule is "demote the experts the router rarely picks". It measures how OFTEN an expert
//! matters, not how MUCH it matters when it does, and in a sparse MoE those come apart in the
//! direction that hurts: **a rare expert is rare because it is a specialist**, selected precisely when
//! the input is unusual. Its tokens are the ones the common experts would have handled badly.
//!
//! So the signal is what demotion actually costs that expert, per expert.
//!
//! ## Weight error is itself a proxy — and a weaker one than it looks
//!
//! `q2demote::relative_error` gives L2 in weight space, which treats every weight as equally
//! important. They are not: a weight multiplied by a large activation contributes far more to the
//! output than one multiplied by a small one, and a format that is L2-optimal can still be output-
//! terrible. That gap is the whole reason activation-aware quantization exists.
//!
//! Both are provided. `weight_error` is free and needs no data; `activation_weighted_error` needs a
//! calibration sample and is the one to trust when they disagree — **and when they disagree the
//! weight-space number is the one that is wrong**.

/// One expert's cost of demotion.
#[derive(Clone, PartialEq, Debug)]
pub struct Sensitivity {
    pub expert: u32,
    /// Relative error introduced by the candidate format. Lower is safer to demote.
    pub error: f32,
    /// Bytes freed by demoting this expert.
    pub saves: u64,
    /// Whether the measurement used activations, or is the free weight-space proxy.
    pub activation_aware: bool,
}

/// Weight-space error: cheap, needs no data, and blind to which weights matter.
pub fn weight_error(w: &[f32]) -> f32 { crate::q2demote::relative_error(w) }

/// Error weighted by the magnitude of the activations each weight multiplies.
///
/// `act_scale[j]` is the typical absolute activation on input column `j`, from a calibration pass.
/// A weight whose column never carries signal can be destroyed for free; one on a hot column cannot.
///
/// This is the same insight behind activation-aware quantization, applied to choosing WHICH expert to
/// degrade rather than how to round it.
pub fn activation_weighted_error(w: &[f32], cols: usize, act_scale: &[f32]) -> f32 {
    if cols == 0 || act_scale.len() < cols { return weight_error(w); }
    let packed = crate::q2demote::quantize(w);
    let (mut num, mut den) = (0.0f64, 0.0f64);
    for (i, x) in w.iter().enumerate() {
        let a = act_scale[i % cols] as f64;
        if a == 0.0 { continue; }
        let deq = crate::q2demote::dequant_at(&packed, i);
        let d = (deq - x) as f64 * a;
        num += d * d;
        den += (*x as f64 * a) * (*x as f64 * a);
    }
    if den == 0.0 { 0.0 } else { (num / den).sqrt() as f32 }
}

/// Choose experts to demote until `need_bytes` are freed, cheapest-error first.
///
/// Returns the chosen set and the worst error accepted — **the second is what a caller must look at**.
/// A list of demoted experts says nothing about whether the model survived; the maximum error taken
/// says how far the worst case was pushed, and that is the number a threshold belongs on.
pub fn choose(mut ranked: Vec<Sensitivity>, need_bytes: u64, max_error: f32)
    -> (Vec<u32>, f32)
{
    ranked.sort_by(|a, b| a.error.partial_cmp(&b.error).unwrap_or(std::cmp::Ordering::Equal));
    let (mut freed, mut out, mut worst) = (0u64, Vec::new(), 0.0f32);
    for s in ranked {
        if freed >= need_bytes { break; }
        // A budget is not a licence. Demoting past the error ceiling to hit a byte target trades a
        // working model for a resident one.
        if s.error > max_error { break; }
        freed += s.saves;
        worst = worst.max(s.error);
        out.push(s.expert);
    }
    (out, worst)
}

/// Demote-and-watch: does a measured quality drop justify promoting something back?
///
/// The cheap alternative to offline calibration. Perplexity on a held-out sample is noisy, so a single
/// bad sample must not trigger a promotion — the threshold is on a RATIO and needs to be exceeded, not
/// merely approached.
pub fn should_promote_back(baseline_ppl: f32, current_ppl: f32, tolerance: f32) -> bool {
    if baseline_ppl <= 0.0 { return false; }
    (current_ppl - baseline_ppl) / baseline_ppl > tolerance
}

#[cfg(test)]
mod tests {
    use super::*;

    fn s(expert: u32, error: f32, saves: u64) -> Sensitivity {
        Sensitivity { expert, error, saves, activation_aware: false }
    }

    #[test]
    fn the_LEAST_sensitive_experts_are_chosen_first() {
        let (picked, worst) = choose(
            vec![s(0, 0.9, 100), s(1, 0.1, 100), s(2, 0.3, 100)], 200, 1.0);
        assert_eq!(picked, vec![1, 2]);
        assert!((worst - 0.3).abs() < 1e-6, "the worst error TAKEN, not the worst available");
    }

    #[test]
    fn the_error_CEILING_stops_the_byte_target() {
        // A budget is not a licence. Demoting past the ceiling to hit a byte number trades a working
        // model for a resident one, and the loop must stop rather than continue.
        let (picked, _) = choose(vec![s(0, 0.1, 50), s(1, 0.8, 50)], 100, 0.5);
        assert_eq!(picked, vec![0], "the second exceeds the ceiling even though bytes are still needed");
    }

    #[test]
    fn the_WORST_ERROR_TAKEN_is_returned_because_the_list_alone_says_nothing() {
        // A caller seeing "demoted experts 3, 7, 11" learns nothing about whether the model survived.
        let (_, worst) = choose(vec![s(0, 0.05, 10), s(1, 0.44, 10)], 20, 1.0);
        assert!((worst - 0.44).abs() < 1e-6);
    }

    #[test]
    fn ACTIVATION_WEIGHTING_changes_the_answer_when_the_hot_columns_are_clean() {
        // The case weight-space L2 gets wrong: error concentrated where no signal flows is free, and a
        // pure L2 ranking would refuse a demotion that costs nothing.
        let cols = 4;
        // Large weights on column 3, small elsewhere.
        let w: Vec<f32> = (0..64).map(|i| if i % cols == 3 { 1.0 } else { 0.01 }).collect();
        let hot_on_clean = vec![1.0, 1.0, 1.0, 0.0];   // column 3 carries nothing
        let hot_on_dirty = vec![0.0, 0.0, 0.0, 1.0];   // column 3 carries everything
        let a = activation_weighted_error(&w, cols, &hot_on_clean);
        let b = activation_weighted_error(&w, cols, &hot_on_dirty);
        assert!(a != b, "the same weights must score differently under different activations");
    }

    #[test]
    fn no_calibration_data_falls_back_to_the_weight_proxy_rather_than_failing() {
        let w = vec![0.5f32; 64];
        assert_eq!(activation_weighted_error(&w, 0, &[]), weight_error(&w));
    }

    #[test]
    fn promotion_needs_the_tolerance_EXCEEDED_not_approached() {
        // Held-out perplexity is noisy, and a promotion on every wobble undoes the saving it was
        // measuring.
        assert!(!should_promote_back(10.0, 10.5, 0.05), "exactly at tolerance is not over it");
        assert!(should_promote_back(10.0, 10.6, 0.05));
        assert!(!should_promote_back(10.0, 9.0, 0.05), "an improvement never promotes back");
    }

    #[test]
    fn a_zero_baseline_does_not_divide_by_zero() {
        assert!(!should_promote_back(0.0, 5.0, 0.05));
    }
}

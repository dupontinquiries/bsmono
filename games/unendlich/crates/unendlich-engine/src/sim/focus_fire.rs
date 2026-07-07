//! Pack focus-fire target selection: pick the index with the lowest effective HP,
//! where higher exposure discounts HP (easier to finish off)
//! (handoff/COMBAT_STANCE.md open list). None if there are no candidates.

pub fn best_target(hps: &[f32], exposures: &[f32]) -> Option<usize> {
    if hps.is_empty() {
        return None;
    }
    let mut best = 0usize;
    let mut best_score = f32::MAX;
    for i in 0..hps.len() {
        let exp = exposures.get(i).copied().unwrap_or(0.0).clamp(0.0, 1.0);
        let score = hps[i].max(0.0) * (1.0 - 0.5 * exp);
        if score < best_score {
            best_score = score;
            best = i;
        }
    }
    Some(best)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn empty_is_none() { assert_eq!(best_target(&[], &[]), None); }
    #[test] fn lowest_hp_chosen() { assert_eq!(best_target(&[50.0, 10.0, 80.0], &[0.0, 0.0, 0.0]), Some(1)); }
    #[test] fn exposure_discounts_hp() { assert_eq!(best_target(&[100.0, 100.0], &[0.0, 1.0]), Some(1)); }
    #[test] fn single_candidate() { assert_eq!(best_target(&[42.0], &[0.0]), Some(0)); }
    #[test] fn shorter_exposures_ok() { assert_eq!(best_target(&[10.0, 5.0], &[]), Some(1)); }
    #[test] fn deterministic() { assert_eq!(best_target(&[7.0, 3.0], &[0.1, 0.2]), best_target(&[7.0, 3.0], &[0.1, 0.2])); }
}

//! Skill XP gained from an action: scales with task difficulty and remaining
//! headroom to mastery (diminishing as skill → 1). In [0, max_gain]. Pure.

pub fn gain(current_skill: f32, difficulty: f32, max_gain: f32) -> f32 {
    let s = current_skill.clamp(0.0, 1.0);
    let diff = difficulty.clamp(0.0, 1.0);
    let mg = max_gain.max(0.0);
    (mg * diff * (1.0 - s)).clamp(0.0, mg)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn novice_gains_more() { assert!(gain(0.1, 0.8, 10.0) > gain(0.9, 0.8, 10.0)); }
    #[test] fn harder_more() { assert!(gain(0.3, 0.9, 10.0) > gain(0.3, 0.2, 10.0)); }
    #[test] fn mastery_zero() { assert_eq!(gain(1.0, 0.8, 10.0), 0.0); }
    #[test] fn bounded() {
        let mut s = 0.0f32;
        while s <= 1.0 { assert!((0.0..=10.0).contains(&gain(s, 0.8, 10.0))); s += 0.1; }
    }
    #[test] fn monotone_in_difficulty() {
        let (mut prev, mut d) = (f32::MIN, 0.0f32);
        while d <= 1.0 { let g = gain(0.4, d, 10.0); assert!(g >= prev); prev = g; d += 0.1; }
    }
    #[test] fn deterministic() { assert_eq!(gain(0.4, 0.7, 10.0).to_bits(), gain(0.4, 0.7, 10.0).to_bits()); }
}

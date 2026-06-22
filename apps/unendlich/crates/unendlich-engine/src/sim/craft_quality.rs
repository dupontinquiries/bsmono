//! Crafted-item quality from crafter skill, input material quality, and a luck
//! roll (all in [0,1]). Skill-weighted, material next, luck a small nudge. Pure.

pub fn quality(skill: f32, material_quality: f32, luck_roll: f32) -> f32 {
    let s = skill.clamp(0.0, 1.0);
    let m = material_quality.clamp(0.0, 1.0);
    let l = luck_roll.clamp(0.0, 1.0);
    (0.5 * s + 0.4 * m + 0.1 * l).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn max_inputs_max_quality() { assert!((quality(1.0, 1.0, 1.0) - 1.0).abs() < 1e-6); }
    #[test] fn more_skill_higher() { assert!(quality(0.9, 0.5, 0.5) > quality(0.1, 0.5, 0.5)); }
    #[test] fn more_material_higher() { assert!(quality(0.5, 0.9, 0.5) > quality(0.5, 0.1, 0.5)); }
    #[test] fn luck_nudges() { assert!(quality(0.5, 0.5, 1.0) > quality(0.5, 0.5, 0.0)); }
    #[test] fn bounded() { assert!((0.0..=1.0).contains(&quality(2.0, -1.0, 0.5))); }
    #[test] fn deterministic() { assert_eq!(quality(0.6, 0.7, 0.5).to_bits(), quality(0.6, 0.7, 0.5).to_bits()); }
}

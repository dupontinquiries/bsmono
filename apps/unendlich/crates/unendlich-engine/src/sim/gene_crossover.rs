//! Blend two parent gene values (each in [0,1]) by `mix` (0 = all A, 1 = all B).
//! Feeds egg breeding from the viewer's exported genome vector. Pure.

pub fn crossover(parent_a: f32, parent_b: f32, mix: f32) -> f32 {
    let a = parent_a.clamp(0.0, 1.0);
    let b = parent_b.clamp(0.0, 1.0);
    let m = mix.clamp(0.0, 1.0);
    (a * (1.0 - m) + b * m).clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn mix0_is_a() { assert_eq!(crossover(0.2, 0.9, 0.0), 0.2); }
    #[test] fn mix1_is_b() { assert_eq!(crossover(0.2, 0.9, 1.0), 0.9); }
    #[test] fn mid_between() { let v = crossover(0.2, 0.8, 0.5); assert!(v > 0.2 && v < 0.8); }
    #[test] fn bounded() {
        let mut m = 0.0f32;
        while m <= 1.0 { let v = crossover(0.3, 0.7, m); assert!((0.0..=1.0).contains(&v)); m += 0.1; }
    }
    #[test] fn clamps_inputs() { assert!((0.0..=1.0).contains(&crossover(5.0, -2.0, 0.5))); }
    #[test] fn deterministic() { assert_eq!(crossover(0.2, 0.9, 0.5).to_bits(), crossover(0.2, 0.9, 0.5).to_bits()); }
}

//! Contact ambient occlusion: nearby geometry darkens a surface. Returns a
//! visibility factor in [0,1] — 1 = unoccluded, → 0 as the gap closes.

pub fn ao(gap: f32, radius: f32) -> f32 {
    let r = radius.max(1e-6);
    let g = gap.max(0.0);
    if g >= r {
        1.0
    } else {
        (g / r).clamp(0.0, 1.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn touching_fully_occluded() { assert_eq!(ao(0.0, 1.0), 0.0); }
    #[test] fn beyond_radius_unoccluded() { assert_eq!(ao(2.0, 1.0), 1.0); }
    #[test] fn closer_is_darker() { assert!(ao(0.2, 1.0) < ao(0.8, 1.0)); }
    #[test] fn bounded() {
        let mut g = 0.0f32;
        while g < 2.0 { let v = ao(g, 1.0); assert!((0.0..=1.0).contains(&v)); g += 0.1; }
    }
    #[test] fn monotone() {
        let (mut prev, mut g) = (f32::MIN, 0.0f32);
        while g < 1.0 { let v = ao(g, 1.0); assert!(v >= prev); prev = v; g += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(ao(0.5, 1.0).to_bits(), ao(0.5, 1.0).to_bits()); }
}

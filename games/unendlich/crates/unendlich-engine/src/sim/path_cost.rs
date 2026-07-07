//! Per-tile movement cost: base × terrain difficulty × (1 + slope). Floored at a
//! small positive value so A* always terminates. Pure, deterministic.

pub fn path_cost(base: f32, terrain_mult: f32, slope: f32) -> f32 {
    let b = base.max(0.0);
    let tm = terrain_mult.max(0.0);
    let s = slope.max(0.0);
    (b * tm * (1.0 + s)).max(0.01)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn flat_easy_is_base() { assert!((path_cost(1.0, 1.0, 0.0) - 1.0).abs() < 1e-6); }
    #[test] fn rough_more() { assert!(path_cost(1.0, 3.0, 0.0) > path_cost(1.0, 1.0, 0.0)); }
    #[test] fn steeper_more() { assert!(path_cost(1.0, 1.0, 2.0) > path_cost(1.0, 1.0, 0.0)); }
    #[test] fn floored_positive() { assert!(path_cost(0.0, 0.0, 0.0) >= 0.01); }
    #[test] fn monotone_in_slope() {
        let (mut prev, mut s) = (f32::MIN, 0.0f32);
        while s < 3.0 { let c = path_cost(1.0, 1.0, s); assert!(c >= prev); prev = c; s += 0.2; }
    }
    #[test] fn deterministic() { assert_eq!(path_cost(1.0, 2.0, 1.0).to_bits(), path_cost(1.0, 2.0, 1.0).to_bits()); }
}

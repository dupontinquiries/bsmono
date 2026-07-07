//! Octile-distance heuristic for 8-way grid A*: admissible cost estimate using
//! straight + diagonal moves. dx, dy are grid deltas. Pure, deterministic.

pub fn octile(dx: f32, dy: f32) -> f32 {
    let ax = dx.abs();
    let ay = dy.abs();
    let (lo, hi) = if ax < ay { (ax, ay) } else { (ay, ax) };
    const SQRT2: f32 = 1.4142135;
    SQRT2 * lo + (hi - lo)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero() { assert_eq!(octile(0.0, 0.0), 0.0); }
    #[test] fn straight() { assert!((octile(5.0, 0.0) - 5.0).abs() < 1e-6); }
    #[test] fn diagonal() { assert!((octile(3.0, 3.0) - 1.4142135 * 3.0).abs() < 1e-4); }
    #[test] fn symmetric() { assert_eq!(octile(3.0, 5.0), octile(5.0, 3.0)); }
    #[test] fn at_least_chebyshev() { assert!(octile(3.0, 5.0) >= 5.0); }
    #[test] fn deterministic() { assert_eq!(octile(3.0, 5.0).to_bits(), octile(3.0, 5.0).to_bits()); }
}

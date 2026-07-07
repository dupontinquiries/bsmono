//! Tracer/streak render width: base plus a motion-stretch term ∝ speed, capped at
//! max_width — faster projectiles render as longer streaks. Pure, deterministic.

pub fn width(speed: f32, base_width: f32, stretch: f32, max_width: f32) -> f32 {
    let s = speed.max(0.0);
    let b = base_width.max(0.0);
    let st = stretch.max(0.0);
    let mw = max_width.max(b);
    (b + s * st).min(mw)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn still_is_base() { assert_eq!(width(0.0, 1.0, 0.1, 5.0), 1.0); }
    #[test] fn faster_wider() { assert!(width(20.0, 1.0, 0.1, 5.0) > width(5.0, 1.0, 0.1, 5.0)); }
    #[test] fn capped() { assert_eq!(width(1000.0, 1.0, 0.1, 5.0), 5.0); }
    #[test] fn never_below_base() { assert!(width(0.0, 1.0, 0.1, 5.0) >= 1.0); }
    #[test] fn monotone() { assert!(width(30.0, 1.0, 0.1, 100.0) > width(10.0, 1.0, 0.1, 100.0)); }
    #[test] fn deterministic() { assert_eq!(width(20.0, 1.0, 0.1, 5.0).to_bits(), width(20.0, 1.0, 0.1, 5.0).to_bits()); }
}

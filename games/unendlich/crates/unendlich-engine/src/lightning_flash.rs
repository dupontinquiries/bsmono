//! Sky-flash brightness from a lightning strike over its brief life: fast
//! exponential decay from 1. In [0,1]. Pure, deterministic.

pub fn flash(age: f32, decay_rate: f32) -> f32 {
    let a = age.max(0.0);
    let k = decay_rate.max(0.0);
    (-(k * a)).exp().clamp(0.0, 1.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn strike_full() { assert_eq!(flash(0.0, 5.0), 1.0); }
    #[test] fn later_dimmer() { assert!(flash(0.5, 5.0) < 1.0); }
    #[test] fn faster_decay_dimmer() { assert!(flash(0.3, 10.0) < flash(0.3, 2.0)); }
    #[test] fn bounded() { let mut a = 0.0f32; while a < 3.0 { assert!((0.0..=1.0).contains(&flash(a, 5.0))); a += 0.1; } }
    #[test] fn monotone() { assert!(flash(0.6, 5.0) < flash(0.2, 5.0)); }
    #[test] fn deterministic() { assert_eq!(flash(0.3, 5.0).to_bits(), flash(0.3, 5.0).to_bits()); }
}

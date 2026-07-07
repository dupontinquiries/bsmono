//! Stalactite/speleothem length grown from drip rate over age, with diminishing
//! (sqrt-time) growth like real mineral accretion, capped at max_length. Pure.

pub fn length(drip_rate: f32, age: f32, max_length: f32) -> f32 {
    let dr = drip_rate.max(0.0);
    let a = age.max(0.0);
    let ml = max_length.max(0.0);
    (dr * a.sqrt()).min(ml)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn zero_age_zero() { assert_eq!(length(0.5, 0.0, 10.0), 0.0); }
    #[test] fn grows_with_age() { assert!(length(0.5, 16.0, 100.0) > length(0.5, 4.0, 100.0)); }
    #[test] fn diminishing_growth() {
        let early = length(1.0, 4.0, 100.0) - length(1.0, 0.0, 100.0);
        let late = length(1.0, 16.0, 100.0) - length(1.0, 12.0, 100.0);
        assert!(early > late);
    }
    #[test] fn faster_drip_longer() { assert!(length(1.0, 16.0, 100.0) > length(0.5, 16.0, 100.0)); }
    #[test] fn capped() { assert_eq!(length(10.0, 10000.0, 5.0), 5.0); }
    #[test] fn deterministic() { assert_eq!(length(0.5, 16.0, 100.0).to_bits(), length(0.5, 16.0, 100.0).to_bits()); }
}

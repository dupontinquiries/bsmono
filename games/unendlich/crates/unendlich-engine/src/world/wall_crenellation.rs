//! Battlement height along a wall at position `x`: alternating merlons (raised,
//! occupying `merlon_frac` of each `period`) and crenels (gaps). Pure, deterministic.

pub fn height(x: f32, period: f32, merlon_frac: f32, base_height: f32, merlon_height: f32) -> f32 {
    let p = period.max(1e-4);
    let frac = merlon_frac.clamp(0.0, 1.0);
    let phase = x.rem_euclid(p) / p;
    if phase < frac {
        merlon_height.max(0.0)
    } else {
        base_height.max(0.0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn on_merlon() { assert_eq!(height(0.0, 4.0, 0.5, 2.0, 5.0), 5.0); }
    #[test] fn in_crenel() { assert_eq!(height(3.0, 4.0, 0.5, 2.0, 5.0), 2.0); }
    #[test] fn periodic() { assert_eq!(height(0.5, 4.0, 0.5, 2.0, 5.0), height(4.5, 4.0, 0.5, 2.0, 5.0)); }
    #[test] fn all_crenel_when_frac0() { assert_eq!(height(0.0, 4.0, 0.0, 2.0, 5.0), 2.0); }
    #[test] fn all_merlon_when_frac1() { assert_eq!(height(3.0, 4.0, 1.0, 2.0, 5.0), 5.0); }
    #[test] fn deterministic() { assert_eq!(height(1.0, 4.0, 0.5, 2.0, 5.0), height(1.0, 4.0, 0.5, 2.0, 5.0)); }
}

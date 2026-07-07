//! Extra path cost for changing heading: proportional to the turn angle so A*
//! prefers smoother routes. 0 straight, `weight` at a full reversal. Pure.

use std::f32::consts::PI;

pub fn turn_penalty(prev_heading: f32, new_heading: f32, weight: f32) -> f32 {
    let w = weight.max(0.0);
    let two_pi = 2.0 * PI;
    let mut d = (new_heading - prev_heading).abs() % two_pi;
    if d > PI {
        d = two_pi - d;
    }
    w * (d / PI)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn straight_no_penalty() { assert_eq!(turn_penalty(1.0, 1.0, 2.0), 0.0); }
    #[test] fn uturn_full() { assert!((turn_penalty(0.0, PI, 2.0) - 2.0).abs() < 1e-5); }
    #[test] fn right_angle_half() { assert!((turn_penalty(0.0, PI / 2.0, 2.0) - 1.0).abs() < 1e-5); }
    #[test] fn symmetric() { assert!((turn_penalty(0.4, 1.9, 2.0) - turn_penalty(1.9, 0.4, 2.0)).abs() < 1e-6); }
    #[test] fn bounded() {
        let mut h = 0.0f32;
        while h < 7.0 { let p = turn_penalty(0.0, h, 2.0); assert!((0.0..=2.0 + 1e-6).contains(&p)); h += 0.2; }
    }
    #[test] fn deterministic() { assert_eq!(turn_penalty(0.0, 1.5, 2.0).to_bits(), turn_penalty(0.0, 1.5, 2.0).to_bits()); }
}

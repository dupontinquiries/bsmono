//! Combat stance: present the healthier flank to the threat (handoff/COMBAT_STANCE.md).
//! Pure, deterministic. -1 = present LEFT side, +1 = present RIGHT, 0 = square.

/// Turn so the higher-HP side faces the threat. Within `hysteresis` of equal ⇒ 0
/// (stay square / don't jitter).
pub fn present_side(left_hp: f32, right_hp: f32, hysteresis: f32) -> i8 {
    let h = hysteresis.max(0.0);
    let d = left_hp - right_hp;
    if d > h {
        -1
    } else if d < -h {
        1
    } else {
        0
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn left_healthier_presents_left() { assert_eq!(present_side(100.0, 40.0, 5.0), -1); }
    #[test] fn right_healthier_presents_right() { assert_eq!(present_side(40.0, 100.0, 5.0), 1); }
    #[test] fn equal_is_square() { assert_eq!(present_side(70.0, 70.0, 5.0), 0); }
    #[test] fn within_hysteresis_is_square() { assert_eq!(present_side(72.0, 70.0, 5.0), 0); }
    #[test] fn sign_symmetric() {
        for &(l, r) in &[(90.0f32, 30.0f32), (10.0, 80.0), (50.0, 50.0)] {
            assert_eq!(present_side(l, r, 3.0), -present_side(r, l, 3.0));
        }
    }
    #[test] fn deterministic() { assert_eq!(present_side(80.0, 20.0, 1.0), present_side(80.0, 20.0, 1.0)); }
}

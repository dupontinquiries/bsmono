//! Pack flanking: spread members around the enemy, biased toward the side it is
//! NOT guarding (handoff/COMBAT_STANCE.md open list). Returns an approach angle
//! (radians) around the enemy; 0 = head-on. enemy_guard_side: -1 left/+1 right/0 square.

use std::f32::consts::PI;

pub fn flank_angle(enemy_guard_side: i8, member_idx: usize, pack_size: usize) -> f32 {
    let n = pack_size.max(1);
    let i = member_idx.min(n - 1);
    let frac = if n == 1 { 0.5 } else { i as f32 / (n - 1) as f32 };
    let base = (frac - 0.5) * PI;                       // spread over [-PI/2, PI/2]
    let exposed = -(enemy_guard_side as f32) * (PI / 4.0); // shift toward exposed flank
    base + exposed
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn single_member_on_exposed_center() { assert!(flank_angle(0, 0, 1).abs() < 1e-6); }
    #[test] fn members_spread() { assert!((flank_angle(0, 0, 4) - flank_angle(0, 3, 4)).abs() > 0.1); }
    #[test] fn guard_biases_opposite() { assert!(flank_angle(-1, 1, 3) > flank_angle(1, 1, 3)); }
    #[test] fn square_symmetric_arc() { assert!((flank_angle(0, 0, 3) + flank_angle(0, 2, 3)).abs() < 1e-5); }
    #[test] fn bounded() {
        for g in [-1i8, 0, 1] { for i in 0..6 { assert!(flank_angle(g, i, 6).abs() <= PI); } }
    }
    #[test] fn deterministic() { assert_eq!(flank_angle(1, 2, 5).to_bits(), flank_angle(1, 2, 5).to_bits()); }
}

//! Turn radius in a coordinated bank: r = v²/(g·tan(bank)). Level flight (bank→0)
//! ⇒ effectively straight (large radius). Bank clamped below π/2. Pure.

pub fn bank_radius(speed: f32, bank_rad: f32) -> f32 {
    let v = speed.max(0.0);
    let g = 9.81;
    let t = bank_rad.clamp(0.0, 1.553).tan();
    if t < 1e-4 {
        return 1.0e9;
    }
    (v * v / (g * t)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn more_bank_tighter() { assert!(bank_radius(20.0, 1.0) < bank_radius(20.0, 0.3)); }
    #[test] fn faster_wider() { assert!(bank_radius(40.0, 0.5) > bank_radius(20.0, 0.5)); }
    #[test] fn level_is_huge() { assert!(bank_radius(20.0, 0.0) > 1.0e8); }
    #[test] fn nonneg() { assert!(bank_radius(20.0, 0.5) >= 0.0); }
    #[test] fn monotone_in_bank() {
        let (mut prev, mut b) = (f32::MAX, 0.1f32);
        while b < 1.4 { let r = bank_radius(25.0, b); assert!(r <= prev + 1e-3); prev = r; b += 0.05; }
    }
    #[test] fn deterministic() { assert_eq!(bank_radius(25.0, 0.5).to_bits(), bank_radius(25.0, 0.5).to_bits()); }
}

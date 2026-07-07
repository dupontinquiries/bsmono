//! To-hit probability [0,1]: accuracy vs evasion, acc/(acc+eva). Pure, deterministic.

pub fn hit_chance(accuracy: f32, evasion: f32) -> f32 {
    let acc = accuracy.max(0.0);
    let eva = evasion.max(0.0);
    let denom = acc + eva;
    if denom <= 0.0 { 0.5 } else { (acc / denom).clamp(0.0, 1.0) }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn equal_half() { assert_eq!(hit_chance(50.0, 50.0), 0.5); }
    #[test] fn more_acc_higher() { assert!(hit_chance(80.0, 50.0) > 0.5); }
    #[test] fn more_eva_lower() { assert!(hit_chance(50.0, 80.0) < 0.5); }
    #[test] fn zero_eva_certain() { assert_eq!(hit_chance(50.0, 0.0), 1.0); }
    #[test] fn both_zero_half() { assert_eq!(hit_chance(0.0, 0.0), 0.5); }
    #[test] fn deterministic() { assert_eq!(hit_chance(80.0, 50.0).to_bits(), hit_chance(80.0, 50.0).to_bits()); }
}

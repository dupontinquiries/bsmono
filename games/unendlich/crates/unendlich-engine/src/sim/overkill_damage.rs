//! Damage spilling past a target's death ("overkill"), for cleave/carryover:
//! max(damage − remaining_hp, 0). Pure, deterministic.

pub fn overkill(damage: f32, remaining_hp: f32) -> f32 {
    (damage.max(0.0) - remaining_hp.max(0.0)).max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn lethal_spills() { assert_eq!(overkill(100.0, 30.0), 70.0); }
    #[test] fn nonlethal_zero() { assert_eq!(overkill(20.0, 30.0), 0.0); }
    #[test] fn exact_kill_zero() { assert_eq!(overkill(30.0, 30.0), 0.0); }
    #[test] fn more_damage_more_spill() { assert!(overkill(120.0, 30.0) > overkill(80.0, 30.0)); }
    #[test] fn nonneg() { assert!(overkill(10.0, 30.0) >= 0.0); }
    #[test] fn deterministic() { assert_eq!(overkill(100.0, 30.0).to_bits(), overkill(100.0, 30.0).to_bits()); }
}

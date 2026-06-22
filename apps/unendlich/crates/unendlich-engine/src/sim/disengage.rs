//! Should the pack break off the fight? Retreat when own HP is low and the pack
//! is thinned, with courage letting them fight on longer
//! (handoff/COMBAT_STANCE.md open list).

pub fn should_retreat(own_hp_frac: f32, allies_alive: u32, allies_total: u32, courage: f32) -> bool {
    let hp = own_hp_frac.clamp(0.0, 1.0);
    let total = allies_total.max(1);
    let strength = allies_alive.min(total) as f32 / total as f32;
    let resolve = (hp + strength) * 0.5 + courage.clamp(0.0, 1.0) * 0.25;
    resolve < 0.35
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn low_hp_few_allies_retreats() { assert!(should_retreat(0.1, 1, 8, 0.0)); }
    #[test] fn healthy_full_pack_stands() { assert!(!should_retreat(1.0, 8, 8, 0.5)); }
    #[test] fn courage_keeps_them_in() { assert!(!should_retreat(0.3, 2, 8, 1.0) && should_retreat(0.3, 2, 8, 0.0)); }
    #[test] fn wiped_allies_more_likely() { assert!(should_retreat(0.5, 0, 8, 0.0) && !should_retreat(0.5, 8, 8, 0.0)); }
    #[test] fn zero_total_safe() { let _ = should_retreat(0.5, 0, 0, 0.5); }
    #[test] fn deterministic() { assert_eq!(should_retreat(0.4, 3, 8, 0.5), should_retreat(0.4, 3, 8, 0.5)); }
}

//! Elemental type effectiveness (0=Wood 1=Water 2=Air 3=Dust 4=Fire 5=Storm,
//! matching the dragon palette). Each element beats the NEXT in the 6-cycle (×2.0)
//! and is weak to the PREVIOUS (×0.5); otherwise ×1.0. Pure, deterministic.

pub fn multiplier(attack_elem: u8, defend_elem: u8) -> f32 {
    let a = (attack_elem % 6) as i32;
    let d = (defend_elem % 6) as i32;
    let diff = ((d - a) % 6 + 6) % 6;
    match diff {
        1 => 2.0,
        5 => 0.5,
        _ => 1.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn same_element_neutral() { assert_eq!(multiplier(3, 3), 1.0); }
    #[test] fn beats_next() { assert_eq!(multiplier(0, 1), 2.0); }
    #[test] fn weak_to_prev() { assert_eq!(multiplier(0, 5), 0.5); }
    #[test] fn neutral_otherwise() { assert_eq!(multiplier(0, 2), 1.0); }
    #[test] fn wraps_mod6() { assert_eq!(multiplier(5, 0), 2.0); assert_eq!(multiplier(12, 1), 2.0); }
    #[test] fn deterministic() { assert_eq!(multiplier(4, 0), multiplier(4, 0)); }
}

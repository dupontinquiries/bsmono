//! Map an item quality score [0,1] to a rarity tier 0..5
//! (common, uncommon, rare, epic, legendary, mythic). Pure, deterministic.

pub fn tier(score: f32) -> u8 {
    let s = score.clamp(0.0, 1.0);
    if s < 0.2 { 0 } else if s < 0.4 { 1 } else if s < 0.6 { 2 } else if s < 0.8 { 3 } else if s < 0.95 { 4 } else { 5 }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test] fn low_common() { assert_eq!(tier(0.1), 0); }
    #[test] fn mid_rare() { assert_eq!(tier(0.5), 2); }
    #[test] fn top_mythic() { assert_eq!(tier(1.0), 5); }
    #[test] fn monotone_nondecreasing() {
        let (mut prev, mut s) = (0u8, 0.0f32);
        while s <= 1.0 { let t = tier(s); assert!(t >= prev); prev = t; s += 0.02; }
    }
    #[test] fn bounded() { for s in [0.0f32, 0.3, 0.7, 1.0] { assert!(tier(s) <= 5); } }
    #[test] fn deterministic() { assert_eq!(tier(0.5), tier(0.5)); }
}

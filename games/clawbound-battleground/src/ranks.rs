//! ranks -- what a cat's level does to its abilities, levels 1 to 15.
//!
//! Every level brings something, on a fixed schedule ([`RANK_UP`]):
//!
//! | level | gains                    | level | gains                    |
//! |-------|--------------------------|-------|--------------------------|
//! | 1     | ability 1 (rank 1)       | 9     | ability 3 r3, ultimate r2|
//! | 2     | ability 2 (rank 1)       | 10    | ability 1 r4             |
//! | 3     | ability 3 (rank 1)       | 11    | ability 2 r4             |
//! | 4     | ability 1 r2             | 12    | ability 3 r4             |
//! | 5     | ability 2 r2, ULTIMATE   | 13    | ability 1 r5, ultimate r3|
//! | 6     | ability 3 r2             | 14    | ability 2 r5             |
//! | 7     | ability 1 r3             | 15    | ability 3 r5             |
//! | 8     | ability 2 r3             |       |                          |
//!
//! Rank 0 is locked: it cannot be cast. Each rank past the first makes an
//! ability hit (heal, shield) harder and come back sooner ([`amount_mul`],
//! [`cooldown_mul`]); rank 3 makes it cheaper ([`cost_mul`]); the top rank
//! -- 5, or 3 for an ultimate -- is its MASTERY, an extra effect that
//! depends on what the ability is ([`mastery`]: more kittens, a wider beam,
//! a longer storm...). [`ranked`] applies all of it to an `AbilityDef`, so
//! the sim casts the ranked ability and nothing else needs to know.
//!
//! Headless; `Game::ability` is the sim's door to it, the HUD draws the
//! pips and locks.

use crate::gods::{AbilityDef, Archetype};

/// The top level; the XP curve stops here.
pub const MAX: u32 = 15;
/// Top rank of a basic ability, and of an ultimate.
pub const TOP: u32 = 5;
pub const TOP_ULT: u32 = 3;

/// The levels at which each slot gains a rank (the first is its unlock).
pub const RANK_UP: [&[u32]; 4] = [&[1, 4, 7, 10, 13], &[2, 5, 8, 11, 14], &[3, 6, 9, 12, 15], &[5, 9, 13]];

/// Slot `slot`'s rank at `level` (0: locked).
pub fn rank(slot: u32, level: u32) -> u32 {
    RANK_UP[slot.min(3) as usize].iter().filter(|&&l| l <= level).count() as u32
}

/// The level slot `slot` reaches rank `r` (None past its top).
pub fn level_for(slot: u32, r: u32) -> Option<u32> {
    if r == 0 {
        return Some(1);
    }
    RANK_UP[slot.min(3) as usize].get(r as usize - 1).copied()
}

pub fn top(ult: bool) -> u32 {
    if ult { TOP_ULT } else { TOP }
}

/// Damage, healing and shields at rank `r`: +12% a rank (+20% an
/// ultimate's).
pub fn amount_mul(r: u32, ult: bool) -> f32 {
    1.0 + if ult { 0.20 } else { 0.12 } * r.saturating_sub(1) as f32
}

/// Cooldown at rank `r`: 5% shorter a rank (10% an ultimate's).
pub fn cooldown_mul(r: u32, ult: bool) -> f32 {
    1.0 - if ult { 0.10 } else { 0.05 } * r.saturating_sub(1) as f32
}

/// Mana cost: 15% cheaper from rank 3 on.
pub fn cost_mul(r: u32) -> f32 {
    if r >= 3 { 0.85 } else { 1.0 }
}

/// The mastery an ability gains at its top rank, in words.
pub fn mastery_text(arch: &Archetype) -> &'static str {
    match arch {
        Archetype::Bolt { .. } => "Mastery: a bigger, faster bolt (+30% size, +20% speed)",
        Archetype::Volley { .. } => "Mastery: two more projectiles in the fan",
        Archetype::Lob { .. } => "Mastery: a 25% wider blast",
        Archetype::Beam { .. } => "Mastery: a 35% wider, 15% longer beam",
        Archetype::Nova { .. } => "Mastery: 15% wider, and it slows 15 points more",
        Archetype::Ring { .. } => "Mastery: the ring closes 30% sooner",
        Archetype::Storm { .. } => "Mastery: the storm lasts 40% longer",
        Archetype::Dash { .. } => "Mastery: a 25% longer dash",
        Archetype::Blink { .. } => "Mastery: a 30% longer blink",
        Archetype::Shield { .. } => "Mastery: the shield lasts 50% longer",
        Archetype::Buff { .. } => "Mastery: the rush lasts 50% longer",
        Archetype::Heal { .. } => "Mastery: the heal reaches 50% further",
        Archetype::Pull { .. } => "Mastery: it drags from 30% further",
        Archetype::Meteor { .. } => "Mastery: a 25% wider crater, and it falls 20% sooner",
        Archetype::Litter { .. } => "Mastery: two more kittens",
        Archetype::Giggle { .. } => "Mastery: the giggles last 35% longer",
        Archetype::DanceParty { .. } => "Mastery: the party goes on 30% longer",
        Archetype::Swap { .. } => "Mastery: arriving staggers a 35% wider ring",
        Archetype::Send { .. } => "Mastery: the other goes 40% farther, and faster",
        Archetype::Echoes { .. } => "Mastery: the echoes linger 30% longer",
        Archetype::Lantern { .. } => "Mastery: the light reaches 25% further",
    }
}

/// The mastery itself: the ability's shape, changed.
pub fn mastery(arch: Archetype) -> Archetype {
    use Archetype::*;
    match arch {
        Bolt { speed, dmg, radius } => Bolt { speed: speed * 1.2, dmg, radius: radius * 1.3 },
        Volley { count, spread_deg, dmg, speed } => Volley { count: count + 2, spread_deg: spread_deg * 1.2, dmg, speed },
        Lob { radius, dmg } => Lob { radius: radius * 1.25, dmg },
        Beam { length, width, dmg } => Beam { length: length * 1.15, width: width * 1.35, dmg },
        Nova { radius, dmg, slow } => Nova { radius: radius * 1.15, dmg, slow: (slow + 0.15).min(0.8) },
        Ring { radius, delay, dmg } => Ring { radius, delay: delay * 0.7, dmg },
        Storm { radius, duration, dps } => Storm { radius, duration: duration * 1.4, dps },
        Dash { dist, dmg } => Dash { dist: dist * 1.25, dmg },
        Blink { dist, dmg } => Blink { dist: dist * 1.3, dmg },
        Shield { amount, duration } => Shield { amount, duration: duration * 1.5 },
        Buff { atk_speed, move_speed, duration } => Buff { atk_speed, move_speed, duration: duration * 1.5 },
        Heal { amount, radius } => Heal { amount, radius: radius * 1.5 },
        Pull { radius, dmg } => Pull { radius: radius * 1.3, dmg },
        Meteor { range, radius, dmg, vault } => Meteor { range, radius: radius * 1.25, dmg, vault: vault * 0.8 },
        Litter { count, dmg, speed, life } => Litter { count: count + 2, dmg, speed, life },
        Giggle { radius, dmg, secs } => Giggle { radius, dmg, secs: secs * 1.35 },
        DanceParty { radius, duration, slow, slow_secs } => DanceParty { radius, duration: duration * 1.3, slow, slow_secs },
        Swap { radius, dmg, slow, secs } => Swap { radius: radius * 1.35, dmg, slow, secs },
        Send { range, speed } => Send { range: range * 1.4, speed: speed * 1.25 },
        Echoes { each, life, dmg } => Echoes { each, life: life * 1.3, dmg },
        Lantern { radius, dmg, flee, reveal } => Lantern { radius: radius * 1.25, dmg, flee, reveal },
    }
}

/// Every amount the ability deals or gives (damage, healing, shielding),
/// scaled by `m`.
fn scale_amounts(arch: Archetype, m: f32) -> Archetype {
    use Archetype::*;
    match arch {
        Bolt { speed, dmg, radius } => Bolt { speed, dmg: dmg * m, radius },
        Volley { count, spread_deg, dmg, speed } => Volley { count, spread_deg, dmg: dmg * m, speed },
        Lob { radius, dmg } => Lob { radius, dmg: dmg * m },
        Beam { length, width, dmg } => Beam { length, width, dmg: dmg * m },
        Nova { radius, dmg, slow } => Nova { radius, dmg: dmg * m, slow },
        Ring { radius, delay, dmg } => Ring { radius, delay, dmg: dmg * m },
        Storm { radius, duration, dps } => Storm { radius, duration, dps: dps * m },
        Dash { dist, dmg } => Dash { dist, dmg: dmg * m },
        Blink { dist, dmg } => Blink { dist, dmg: dmg * m },
        Shield { amount, duration } => Shield { amount: amount * m, duration },
        // a rush has no amount: its speed grows a little instead
        Buff { atk_speed, move_speed, duration } => Buff { atk_speed: atk_speed * (1.0 + (m - 1.0) * 0.5), move_speed, duration },
        Heal { amount, radius } => Heal { amount: amount * m, radius },
        Pull { radius, dmg } => Pull { radius, dmg: dmg * m },
        Meteor { range, radius, dmg, vault } => Meteor { range, radius, dmg: dmg * m, vault },
        Litter { count, dmg, speed, life } => Litter { count, dmg: dmg * m, speed, life },
        Giggle { radius, dmg, secs } => Giggle { radius, dmg: dmg * m, secs },
        DanceParty { radius, duration, slow, slow_secs } => DanceParty { radius, duration, slow, slow_secs: slow_secs * (1.0 + (m - 1.0) * 0.5) },
        Swap { radius, dmg, slow, secs } => Swap { radius, dmg: dmg * m, slow, secs },
        // a sending has no amount: the other glides a little faster instead
        Send { range, speed } => Send { range, speed: speed * (1.0 + (m - 1.0) * 0.5) },
        Echoes { each, life, dmg } => Echoes { each, life, dmg: dmg * m },
        Lantern { radius, dmg, flee, reveal } => Lantern { radius, dmg: dmg * m, flee, reveal },
    }
}

/// `ab` at rank `r`; None when locked (rank 0).
pub fn ranked(ab: AbilityDef, r: u32) -> Option<AbilityDef> {
    if r == 0 {
        return None;
    }
    let r = r.min(top(ab.ult));
    let mut a = ab;
    a.arch = scale_amounts(a.arch, amount_mul(r, ab.ult));
    if r >= top(ab.ult) {
        a.arch = mastery(a.arch);
    }
    a.cooldown *= cooldown_mul(r, ab.ult);
    a.cost *= cost_mul(r);
    Some(a)
}

/// What reaching `level` brought, in words: one line per slot that ranked
/// up (for the level-up toast and the tooltip).
pub fn gains_at(abilities: &[AbilityDef; 4], level: u32) -> Vec<String> {
    let mut v = Vec::new();
    for (slot, ab) in abilities.iter().enumerate() {
        let r = rank(slot as u32, level);
        if r > rank(slot as u32, level.saturating_sub(1)) || (level == 1 && r == 1) {
            let t = top(ab.ult);
            v.push(if r == 1 {
                format!("{} unlocked", ab.name)
            } else if r == t {
                format!("{} rank {r} -- {}", ab.name, mastery_text(&ab.arch))
            } else {
                format!("{} rank {r}", ab.name)
            });
        }
    }
    v
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::gods::ROSTER;

    #[test]
    fn every_level_brings_something_and_all_is_had_by_fifteen() {
        for level in 1..=MAX {
            assert!(!gains_at(&ROSTER[0].abilities, level).is_empty(), "level {level} gives nothing");
        }
        for slot in 0..4 {
            let ult = slot == 3;
            assert_eq!(rank(slot, MAX), top(ult), "slot {slot} is not at its top by {MAX}");
            assert_eq!(rank(slot, MAX + 5), top(ult));
        }
        assert_eq!((rank(0, 1), rank(1, 1), rank(2, 1), rank(3, 1)), (1, 0, 0, 0), "level 1: the first ability only");
        assert_eq!(rank(3, 4), 0);
        assert_eq!(rank(3, 5), 1, "the ultimate at 5");
        // ranks never go down as levels go up
        for slot in 0..4 {
            for l in 1..MAX {
                assert!(rank(slot, l + 1) >= rank(slot, l));
            }
        }
    }

    #[test]
    fn a_rank_hits_harder_comes_back_sooner_and_the_top_masters() {
        for def in ROSTER.iter() {
            for ab in def.abilities {
                assert!(ranked(ab, 0).is_none(), "locked");
                let one = ranked(ab, 1).unwrap();
                let t = top(ab.ult);
                let full = ranked(ab, t).unwrap();
                assert_eq!(format!("{:?}", one.arch), format!("{:?}", ab.arch), "{}: rank 1 is the ability as written", ab.name);
                assert!(full.cooldown < one.cooldown && full.cost < one.cost, "{}", ab.name);
                // the top rank is more than the amounts: its shape changed
                let amounts_only = scale_amounts(ab.arch, amount_mul(t, ab.ult));
                assert_ne!(format!("{:?}", full.arch), format!("{:?}", amounts_only), "{}: no mastery", ab.name);
            }
        }
    }
}

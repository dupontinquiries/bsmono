//! DAMAGE — what a hit takes off a ship, and what that costs next time.
//!
//! `foothold` already says an LZ approach PINS THE SHIP: it lands and stays,
//! and a large stationary thing on a hostile world is a target. Until now
//! that was a stated cost with no mechanism. This is the mechanism.
//!
//! **Modules are not equally exposed, and that is the whole design.** A hold
//! is deep inside; armour and a dropship cradle are bolted to the outside.
//! So a hit works through the hull from the skin inward, which makes armour
//! do its job twice — it absorbs, and it is also the thing that gets hit
//! first. Losing armour is how you find out you had it.
//!
//! **And the cost lands on the NEXT voyage, not this raid.** A holed reactor
//! does not stop the fight; it means the trip home is shorter than the trip
//! out, and the world you were going to next is now out of reach. That is a
//! consequence with a delay, which is the kind worth having.

use crate::loadout::{Module, Ship};

/// How exposed a module is: 0 buried, 1 bolted to the skin.
///
/// Reasoned from where the thing has to BE rather than assigned. Armour is
/// the outermost layer by definition. A dropship needs a door to the
/// outside. A reactor wants shielding around it and sits deep. A hold is
/// whatever volume is left in the middle, which is the safest place on any
/// vessel and the reason cargo survives when crew does not.
pub fn exposure(m: Module) -> f32 {
    match m {
        Module::Armour => 1.00,
        Module::Dropship => 0.75,
        Module::Fabricator => 0.40,
        Module::Reactor => 0.25,
        Module::Hold => 0.15,
    }
}

/// A module's condition, 0 destroyed .. 1 intact.
#[derive(Clone, Debug)]
pub struct Condition {
    pub modules: Vec<(Module, f32)>,
    /// hull integrity itself, 0 wrecked .. 1 sound
    pub hull: f32,
}

impl Condition {
    pub fn intact(s: &Ship) -> Condition {
        Condition { modules: s.modules.iter().map(|m| (*m, 1.0)).collect(), hull: 1.0 }
    }

    /// Is this module still doing its job? Below a third it is scrap that
    /// happens to still be bolted on.
    pub fn works(&self, m: Module) -> bool {
        self.modules.iter().any(|(k, c)| *k == m && *c >= 0.34)
    }

    pub fn count_working(&self, m: Module) -> usize {
        self.modules.iter().filter(|(k, c)| *k == m && *c >= 0.34).count()
    }

    /// Delta-v after damage. A holed reactor is the difference between
    /// getting home and not.
    pub fn dv_kms(&self, base: f32) -> f32 {
        let want = self.modules.iter().filter(|(k, _)| *k == Module::Reactor).count();
        if want == 0 {
            return base * self.hull.max(0.3);
        }
        let have: f32 = self
            .modules
            .iter()
            .filter(|(k, _)| *k == Module::Reactor)
            .map(|(_, c)| c.clamp(0.0, 1.0))
            .sum();
        base * (0.35 + 0.65 * have / want as f32) * self.hull.max(0.3)
    }

    /// Capacity after damage — a breached hold spills.
    pub fn capacity(&self, base: i32) -> i32 {
        let lost: i32 = self
            .modules
            .iter()
            .filter(|(k, c)| *k == Module::Hold && *c < 0.34)
            .count() as i32;
        (base - lost * 6).max(0)
    }
}

/// APPLY A HIT. Damage works inward from the skin: the most exposed working
/// module takes it, and only what gets past reaches the next one.
///
/// Returns what was destroyed, if anything.
pub fn hit(c: &mut Condition, force: f32, seed: u32) -> Option<Module> {
    let mut s = seed.wrapping_mul(2_246_822_519).wrapping_add(0x9E37_79B9) | 1;
    let mut rnd = move || {
        s ^= s << 13;
        s ^= s >> 17;
        s ^= s << 5;
        (s >> 8) as f32 / 16_777_216.0
    };
    let mut left = force.max(0.0);
    // outermost first, and among equals the one still healthy enough to
    // absorb — a shredded plate does not stop the next round
    let mut order: Vec<usize> = (0..c.modules.len()).collect();
    order.sort_by(|a, b| {
        let ka = (exposure(c.modules[*a].0) * 100.0 + c.modules[*a].1 * 10.0) as i32;
        let kb = (exposure(c.modules[*b].0) * 100.0 + c.modules[*b].1 * 10.0) as i32;
        kb.cmp(&ka)
    });
    let mut killed = None;
    for i in order {
        if left <= 0.0 {
            break;
        }
        let (m, ref mut cond) = c.modules[i];
        if *cond <= 0.0 {
            continue;
        }
        // ARMOUR ABSORBS MORE THAN IT LOSES, which is what makes it worth a
        // hull slot: it soaks 2.2x the damage it takes itself.
        let soak = if m == Module::Armour { 2.2 } else { 1.0 };
        // A PLATE STOPS WHAT IT CAN AND PASSES THE EXCESS. The first
        // version leaked a fraction of every hit onward even when the
        // armour had absorbed it, so a light hit reached the hold through
        // an intact plate — which is not what armour is. What gets past is
        // what OVERWHELMED the layer, plus a little spall for anything that
        // is not armour and so shatters rather than stopping.
        let capacity = *cond * soak;
        let taken = left.min(capacity);
        *cond = (*cond - taken / soak).max(0.0);
        let spall = if m == Module::Armour { 0.0 } else { taken * 0.15 * rnd() };
        left = (left - taken + spall).max(0.0);
        if *cond <= 0.0 && killed.is_none() {
            killed = Some(m);
        }
    }
    // whatever got through the modules is the hull's problem
    if left > 0.0 {
        c.hull = (c.hull - left * 0.25).clamp(0.0, 1.0);
    }
    killed
}

/// REPAIR. A fabricator fixes modules in the field but cannot rebuild a
/// hull; a dock does both. That asymmetry is what makes a fabricator worth
/// carrying and a dock worth returning to.
pub fn repair_field(c: &mut Condition, hours: f32) -> usize {
    if !c.works(Module::Fabricator) {
        return 0;
    }
    let mut fixed = 0;
    let mut budget = hours * 0.18;
    for (m, cond) in c.modules.iter_mut() {
        if *m == Module::Fabricator || *cond >= 1.0 || budget <= 0.0 {
            continue;
        }
        // a destroyed module cannot be brought back in the field; there is
        // nothing left to work with
        if *cond <= 0.0 {
            continue;
        }
        let want = 1.0 - *cond;
        let give = budget.min(want);
        *cond += give;
        budget -= give;
        if *cond >= 1.0 {
            fixed += 1;
        }
    }
    fixed
}

pub fn repair_dock(c: &mut Condition, s: &Ship) {
    *c = Condition::intact(s);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship() -> Ship {
        Ship {
            hull: 12,
            modules: vec![
                Module::Armour,
                Module::Dropship,
                Module::Reactor,
                Module::Hold,
                Module::Fabricator,
            ],
        }
    }

    #[test]
    fn a_hit_works_inward_from_the_skin() {
        // the design: armour is the outermost layer BY DEFINITION, so it is
        // both what protects and what gets hit
        let mut c = Condition::intact(&ship());
        hit(&mut c, 0.8, 1);
        let armour = c.modules.iter().find(|(m, _)| *m == Module::Armour).unwrap().1;
        let hold = c.modules.iter().find(|(m, _)| *m == Module::Hold).unwrap().1;
        assert!(armour < 1.0, "armour should take it first");
        assert_eq!(hold, 1.0, "the hold is buried and should be untouched");
    }

    #[test]
    fn losing_armour_is_how_you_find_out_you_had_it() {
        let mut with = Condition::intact(&ship());
        let bare = Ship { hull: 12, modules: vec![Module::Reactor, Module::Hold] };
        let mut without = Condition::intact(&bare);
        for s in 0..4 {
            hit(&mut with, 0.9, s);
            hit(&mut without, 0.9, s);
        }
        assert!(with.hull > without.hull, "with {} without {}", with.hull, without.hull);
        // and armour soaks more than it loses, which is why it earns a slot
        let mut solo = Condition::intact(&Ship { hull: 3, modules: vec![Module::Armour] });
        assert!(hit(&mut solo, 1.5, 7).is_none(), "1.5 should not destroy a plate that soaks 2.2x");
    }

    #[test]
    fn a_holed_reactor_costs_the_next_voyage_not_this_raid() {
        // consequence with a delay, which is the kind worth having
        let mut c = Condition::intact(&ship());
        let full = c.dv_kms(40.0);
        for (m, cond) in c.modules.iter_mut() {
            if *m == Module::Reactor {
                *cond = 0.0;
            }
        }
        let holed = c.dv_kms(40.0);
        assert!(holed < full * 0.5, "full {full} holed {holed}");
        assert!(holed > 0.0, "a dead reactor should still coast, not vanish");
    }

    #[test]
    fn a_breached_hold_spills() {
        let mut c = Condition::intact(&ship());
        assert_eq!(c.capacity(10), 10);
        for (m, cond) in c.modules.iter_mut() {
            if *m == Module::Hold {
                *cond = 0.1;
            }
        }
        assert!(c.capacity(10) < 10);
        assert!(!c.works(Module::Hold));
    }

    #[test]
    fn a_third_gone_is_scrap_that_happens_to_be_bolted_on() {
        let mut c = Condition::intact(&ship());
        for (m, cond) in c.modules.iter_mut() {
            if *m == Module::Reactor {
                *cond = 0.33;
            }
        }
        assert!(!c.works(Module::Reactor));
        for (m, cond) in c.modules.iter_mut() {
            if *m == Module::Reactor {
                *cond = 0.35;
            }
        }
        assert!(c.works(Module::Reactor));
    }

    #[test]
    fn a_fabricator_mends_what_is_damaged_and_cannot_raise_the_dead() {
        let mut c = Condition::intact(&ship());
        for (m, cond) in c.modules.iter_mut() {
            match m {
                Module::Reactor => *cond = 0.5,
                Module::Dropship => *cond = 0.0,
                _ => {}
            }
        }
        repair_field(&mut c, 6.0);
        let r = c.modules.iter().find(|(m, _)| *m == Module::Reactor).unwrap().1;
        let d = c.modules.iter().find(|(m, _)| *m == Module::Dropship).unwrap().1;
        assert!(r > 0.5, "a damaged reactor should mend: {r}");
        assert_eq!(d, 0.0, "a destroyed module has nothing left to work with");
        // and with no fabricator, nothing happens at all
        let mut bare = Condition::intact(&Ship { hull: 4, modules: vec![Module::Reactor] });
        bare.modules[0].1 = 0.5;
        assert_eq!(repair_field(&mut bare, 20.0), 0);
        assert_eq!(bare.modules[0].1, 0.5);
    }

    #[test]
    fn a_dock_does_what_a_fabricator_cannot() {
        // which is what makes a fabricator worth carrying AND a dock worth
        // returning to, rather than one replacing the other
        let s = ship();
        let mut c = Condition::intact(&s);
        c.hull = 0.2;
        for (_, cond) in c.modules.iter_mut() {
            *cond = 0.0;
        }
        repair_field(&mut c, 100.0);
        assert_eq!(c.hull, 0.2, "the field cannot rebuild a hull");
        repair_dock(&mut c, &s);
        assert_eq!(c.hull, 1.0);
        assert!(c.works(Module::Reactor));
    }

    #[test]
    fn enough_force_gets_through_to_the_hull() {
        let mut c = Condition::intact(&ship());
        for s in 0..30 {
            hit(&mut c, 2.0, s);
        }
        assert!(c.hull < 0.5, "thirty heavy hits should reach the hull: {}", c.hull);
        assert!(c.hull >= 0.0);
    }
}

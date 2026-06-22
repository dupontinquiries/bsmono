//! Elemental afflictions — status effects that tick over time and react to one
//! another, turning the element system into combat feel. Fire and lightning are
//! damage-over-time (burning, shocked); water soaks (no damage itself, but it
//! douses fire and *conducts* lightning); wood roots and slows. Applying an
//! element onto an opposing affliction triggers a reaction instead of just
//! stacking: water quenches burning, lightning on a soaked target conducts for a
//! burst, fire on a soaked target evaporates the water (and lands weakened).
//!
//! Deterministic fixed-point — safe for the rollback sim. Reuses [`Element`].

use crate::sim::elemental::Element;
use crate::sim::fixed::Fixed;

/// What happened when an element was applied to an already-afflicted target.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Reaction {
    /// Plain application — stacked or refreshed.
    None,
    /// Water put out a fire.
    Quench,
    /// Lightning arced through water for bonus burst damage.
    Conduct,
    /// Fire boiled off a soak and landed weakened.
    Evaporate,
}

/// A single active affliction.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Affliction {
    pub element: Element,
    pub intensity: Fixed,
    pub ticks_left: i32,
}

/// Result of one tick of all active afflictions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TickOutcome {
    pub damage: Fixed,
    /// Movement multiplier this tick: 1 = normal, smaller = more slowed.
    pub slow_factor: Fixed,
}

/// Result of applying an element.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ApplyOutcome {
    pub reaction: Reaction,
    /// Immediate damage from a reaction (e.g. conduct), separate from DoT.
    pub burst_damage: Fixed,
}

/// Per-tick damage an element deals while afflicting. Only fire and lightning
/// burn; the rest are control/utility.
fn per_tick_damage(element: Element, intensity: Fixed) -> Fixed {
    match element {
        Element::Fire | Element::Lightning => intensity,
        _ => Fixed::ZERO,
    }
}

/// Movement multiplier an element imposes while active. Wood roots hard, water
/// makes you sluggish; others don't slow.
fn slow_factor(element: Element) -> Fixed {
    match element {
        Element::Wood => Fixed::from_frac(1, 2),  // rooted
        Element::Water => Fixed::from_frac(4, 5), // soaked, sluggish
        _ => Fixed::ONE,
    }
}

/// The set of afflictions on a target.
#[derive(Debug, Clone, Default)]
pub struct Afflicted {
    pub afflictions: Vec<Affliction>,
}

impl Afflicted {
    pub fn new() -> Self {
        Self::default()
    }

    fn find(&self, element: Element) -> Option<usize> {
        self.afflictions.iter().position(|a| a.element == element)
    }

    fn add_or_refresh(&mut self, element: Element, intensity: Fixed, ticks: i32) {
        if let Some(i) = self.find(element) {
            let a = &mut self.afflictions[i];
            a.intensity = a.intensity + intensity; // stack strength
            if ticks > a.ticks_left {
                a.ticks_left = ticks; // refresh duration
            }
        } else {
            self.afflictions.push(Affliction { element, intensity, ticks_left: ticks });
        }
    }

    /// Apply an element, resolving any reaction with existing afflictions.
    pub fn apply(&mut self, element: Element, intensity: Fixed, ticks: i32) -> ApplyOutcome {
        // Water douses an existing fire instead of stacking.
        if element == Element::Water {
            if let Some(i) = self.find(Element::Fire) {
                self.afflictions.remove(i);
                self.add_or_refresh(Element::Water, intensity, ticks); // leaves a soak
                return ApplyOutcome { reaction: Reaction::Quench, burst_damage: Fixed::ZERO };
            }
        }
        // Lightning conducts through a soak: burst now, consume the water.
        if element == Element::Lightning {
            if let Some(i) = self.find(Element::Water) {
                let soak = self.afflictions[i].intensity;
                self.afflictions.remove(i);
                let burst = intensity * Fixed::from_frac(3, 2) + soak;
                self.add_or_refresh(Element::Lightning, intensity, ticks);
                return ApplyOutcome { reaction: Reaction::Conduct, burst_damage: burst };
            }
        }
        // Fire boils off a soak and lands at half strength.
        if element == Element::Fire {
            if let Some(i) = self.find(Element::Water) {
                self.afflictions.remove(i);
                let reduced = intensity * Fixed::from_frac(1, 2);
                self.add_or_refresh(Element::Fire, reduced, ticks);
                return ApplyOutcome { reaction: Reaction::Evaporate, burst_damage: Fixed::ZERO };
            }
        }
        self.add_or_refresh(element, intensity, ticks);
        ApplyOutcome { reaction: Reaction::None, burst_damage: Fixed::ZERO }
    }

    /// Advance one tick: accumulate DoT, take the strongest slow, expire spent
    /// afflictions.
    pub fn tick(&mut self) -> TickOutcome {
        let mut damage = Fixed::ZERO;
        let mut slow = Fixed::ONE;
        for a in self.afflictions.iter_mut() {
            damage = damage + per_tick_damage(a.element, a.intensity);
            let s = slow_factor(a.element);
            if s < slow {
                slow = s;
            }
            a.ticks_left -= 1;
        }
        self.afflictions.retain(|a| a.ticks_left > 0);
        TickOutcome { damage, slow_factor: slow }
    }

    pub fn has(&self, element: Element) -> bool {
        self.find(element).is_some()
    }

    pub fn is_clear(&self) -> bool {
        self.afflictions.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn i(n: i64) -> Fixed {
        Fixed::from_int(n)
    }

    #[test]
    fn burning_deals_damage_each_tick_then_expires() {
        let mut t = Afflicted::new();
        t.apply(Element::Fire, i(5), 3);
        assert_eq!(t.tick().damage, i(5));
        assert_eq!(t.tick().damage, i(5));
        assert_eq!(t.tick().damage, i(5));
        assert!(t.is_clear(), "burning expires after its ticks");
        assert_eq!(t.tick().damage, Fixed::ZERO);
    }

    #[test]
    fn water_quenches_fire_instead_of_stacking() {
        let mut t = Afflicted::new();
        t.apply(Element::Fire, i(5), 5);
        let out = t.apply(Element::Water, i(3), 4);
        assert_eq!(out.reaction, Reaction::Quench);
        assert!(!t.has(Element::Fire), "fire put out");
        assert!(t.has(Element::Water), "left soaked");
        assert_eq!(t.tick().damage, Fixed::ZERO, "no more burn");
    }

    #[test]
    fn lightning_conducts_through_water_for_a_burst() {
        let mut t = Afflicted::new();
        t.apply(Element::Water, i(4), 5);
        let out = t.apply(Element::Lightning, i(10), 2);
        assert_eq!(out.reaction, Reaction::Conduct);
        // burst = 10 * 1.5 + soak(4) = 19, far more than the plain 10.
        assert_eq!(out.burst_damage, i(19));
        assert!(!t.has(Element::Water), "soak consumed by the arc");
        assert!(t.has(Element::Lightning));
    }

    #[test]
    fn fire_evaporates_a_soak_and_lands_weakened() {
        let mut t = Afflicted::new();
        t.apply(Element::Water, i(4), 5);
        let out = t.apply(Element::Fire, i(8), 3);
        assert_eq!(out.reaction, Reaction::Evaporate);
        assert!(!t.has(Element::Water));
        // Fire intensity halved to 4 by the boil-off.
        assert_eq!(t.tick().damage, i(4));
    }

    #[test]
    fn wood_roots_harder_than_water_soaks() {
        let mut t = Afflicted::new();
        t.apply(Element::Water, i(1), 5);
        assert_eq!(t.tick().slow_factor, Fixed::from_frac(4, 5));
        t.apply(Element::Wood, i(1), 5);
        // Strongest slow wins → wood's 1/2.
        assert_eq!(t.tick().slow_factor, Fixed::from_frac(1, 2));
    }

    #[test]
    fn same_element_stacks_intensity_and_refreshes_duration() {
        let mut t = Afflicted::new();
        t.apply(Element::Fire, i(3), 2);
        t.apply(Element::Fire, i(4), 5); // stack to 7, refresh to 5 ticks
        assert_eq!(t.afflictions.len(), 1);
        assert_eq!(t.tick().damage, i(7));
        assert_eq!(t.afflictions[0].ticks_left, 4, "refreshed duration, now ticked once");
    }

    #[test]
    fn unrelated_elements_coexist() {
        let mut t = Afflicted::new();
        t.apply(Element::Fire, i(2), 3);
        t.apply(Element::Lightning, i(3), 3);
        // Both DoT, no reaction between them.
        assert_eq!(t.tick().damage, i(5));
        assert_eq!(t.afflictions.len(), 2);
    }
}

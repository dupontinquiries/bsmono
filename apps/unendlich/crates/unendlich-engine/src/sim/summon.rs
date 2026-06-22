//! Summoned companions — the spirit wolf called from a blade, and its kin. A
//! companion is a short-lived ally: it pads alongside its summoner, lunges at
//! whatever threatens them, and fades when its borrowed life runs out. The
//! blade's element colours its bite — a lightning wolf arcs hard, a wood wolf
//! barely nips.
//!
//! Built on the [`ItemBody`](crate::sim::item_entity::ItemBody) chassis (so it
//! collides and moves like any world entity) with a small combat brain over the
//! top. Pure, deterministic fixed-point — snapshots and rolls back with the sim.

use crate::sim::elemental::{base_profile, Element};
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::item_entity::ItemBody;

/// What the companion is doing.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CompanionMode {
    /// Padding along near its summoner.
    #[default]
    Follow,
    /// Closing on a threat.
    Engage,
    /// Its borrowed life is spent; dissolving.
    Fading,
}

/// A summoned ally.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Companion {
    pub body: ItemBody,
    pub element: Element,
    pub mode: CompanionMode,
    /// Ticks of borrowed life remaining; at 0 it fades.
    pub lifetime: u16,
    pub attack_cooldown: u16,
    pub speed: Fixed,
}

/// What the companion wants to do this tick (the caller resolves world effects).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CompanionAction {
    /// Idle at the summoner's side.
    Idle,
    /// Moving toward this point.
    MoveTo(Vec3),
    /// Biting the target now for `damage` of `element`.
    Bite(Vec3, u16, Element),
    /// Borrowed life spent — despawn.
    Faded,
}

/// How far it keeps from the summoner before trotting back.
const HEEL_DISTANCE: i64 = 4;
/// Reach of a bite.
const BITE_REACH: i64 = 2;
/// Ticks between bites.
const BITE_COOLDOWN: u16 = 16;

impl Companion {
    /// Call a companion from a blade of the given element, at a position, with a
    /// borrowed lifetime (ticks) and a move speed.
    pub fn summon(element: Element, at: Vec3, half: Vec3, lifetime: u16, speed: Fixed) -> Companion {
        Companion {
            body: ItemBody::new(at, half),
            element,
            mode: CompanionMode::Follow,
            lifetime,
            attack_cooldown: 0,
            speed,
        }
    }

    /// The bite strength of this companion's element.
    pub fn bite_damage(&self) -> u16 {
        base_profile(self.element).potency
    }

    /// One tick. `owner` is the summoner's position; `enemy` is the nearest
    /// threat the caller found (already filtered to aggro range), or `None`.
    pub fn update(&mut self, owner: Vec3, enemy: Option<Vec3>) -> CompanionAction {
        // Borrowed life always drains.
        if self.lifetime == 0 {
            self.mode = CompanionMode::Fading;
            return CompanionAction::Faded;
        }
        self.lifetime -= 1;
        if self.attack_cooldown > 0 {
            self.attack_cooldown -= 1;
        }

        match enemy {
            Some(foe) => {
                self.mode = CompanionMode::Engage;
                if within(self.body.pos, foe, BITE_REACH) {
                    if self.attack_cooldown == 0 {
                        self.attack_cooldown = BITE_COOLDOWN;
                        self.body.vel = Vec3::ZERO;
                        return CompanionAction::Bite(foe, self.bite_damage(), self.element);
                    }
                    // In reach but recovering — hold.
                    self.body.vel = Vec3::ZERO;
                    CompanionAction::Idle
                } else {
                    self.seek(foe);
                    CompanionAction::MoveTo(foe)
                }
            }
            None => {
                self.mode = CompanionMode::Follow;
                if within(self.body.pos, owner, HEEL_DISTANCE) {
                    self.body.vel = Vec3::ZERO;
                    CompanionAction::Idle
                } else {
                    self.seek(owner);
                    CompanionAction::MoveTo(owner)
                }
            }
        }
    }

    /// Deterministic homing: per-axis move toward `target`, clamped to `speed`
    /// (slows to arrive). No sqrt → bit-exact.
    fn seek(&mut self, target: Vec3) {
        let s = self.speed;
        let neg = Fixed::ZERO - s;
        self.body.vel = Vec3 {
            x: (target.x - self.body.pos.x).clamp(neg, s),
            y: (target.y - self.body.pos.y).clamp(neg, s),
            z: (target.z - self.body.pos.z).clamp(neg, s),
        };
        self.body.step();
    }
}

/// Chebyshev "within `range` blocks on every axis" — cheap, deterministic, no sqrt.
fn within(a: Vec3, b: Vec3, range: i64) -> bool {
    let r = Fixed::from_int(range);
    (a.x - b.x).abs() <= r && (a.y - b.y).abs() <= r && (a.z - b.z).abs() <= r
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }
    fn wolf(element: Element, at: Vec3) -> Companion {
        Companion::summon(element, at, v(0.4, 0.4, 0.4), 200, Fixed::from_int(1))
    }

    #[test]
    fn follows_its_summoner_when_far() {
        let mut w = wolf(Element::Lightning, v(20.0, 0.0, 0.0));
        let owner = v(0.0, 0.0, 0.0);
        let mut last = w.body.pos.x;
        for _ in 0..30 {
            match w.update(owner, None) {
                CompanionAction::MoveTo(_) => {
                    assert!(w.body.pos.x <= last, "trotting back toward the owner");
                    last = w.body.pos.x;
                }
                CompanionAction::Idle => break,
                _ => {}
            }
        }
        assert_eq!(w.mode, CompanionMode::Follow);
        assert!(within(w.body.pos, owner, HEEL_DISTANCE), "ended up at heel");
    }

    #[test]
    fn idles_at_heel_when_close_and_safe() {
        let mut w = wolf(Element::Fire, v(1.0, 0.0, 0.0));
        assert_eq!(w.update(v(0.0, 0.0, 0.0), None), CompanionAction::Idle);
    }

    #[test]
    fn engages_and_bites_a_threat_then_cools_down() {
        let mut w = wolf(Element::Lightning, v(0.5, 0.0, 0.0));
        let owner = v(0.0, 0.0, 0.0);
        let foe = v(1.0, 0.0, 0.0); // already in reach
        match w.update(owner, Some(foe)) {
            CompanionAction::Bite(at, dmg, el) => {
                assert_eq!(at, foe);
                assert_eq!(el, Element::Lightning);
                assert_eq!(dmg, base_profile(Element::Lightning).potency);
            }
            other => panic!("expected a bite, got {other:?}"),
        }
        assert_eq!(w.mode, CompanionMode::Engage);
        assert!(w.attack_cooldown > 0);
        // Immediately after, it's recovering — holds rather than biting again.
        assert_eq!(w.update(owner, Some(foe)), CompanionAction::Idle);
    }

    #[test]
    fn closes_distance_to_a_far_foe() {
        let mut w = wolf(Element::Fire, v(0.0, 0.0, 0.0));
        let foe = v(10.0, 0.0, 0.0);
        let action = w.update(v(0.0, 0.0, 0.0), Some(foe));
        assert_eq!(action, CompanionAction::MoveTo(foe));
        assert!(w.body.pos.x > Fixed::ZERO, "moved toward the foe");
        assert_eq!(w.mode, CompanionMode::Engage);
    }

    #[test]
    fn element_colours_the_bite() {
        let storm = wolf(Element::Lightning, v(0.0, 0.0, 0.0)).bite_damage();
        let timber = wolf(Element::Wood, v(0.0, 0.0, 0.0)).bite_damage();
        assert!(storm > timber, "a lightning wolf bites harder than a wood one: {storm} > {timber}");
    }

    #[test]
    fn fades_when_its_borrowed_life_runs_out() {
        let mut w = Companion::summon(Element::Air, v(0.0, 0.0, 0.0), v(0.4, 0.4, 0.4), 1, Fixed::from_int(1));
        assert_ne!(w.update(v(0.0, 0.0, 0.0), None), CompanionAction::Faded, "still alive on the last tick");
        assert_eq!(w.update(v(0.0, 0.0, 0.0), None), CompanionAction::Faded, "then dissolves");
        assert_eq!(w.mode, CompanionMode::Fading);
    }
}

//! Elemental projectiles with real medium physics — no scripted parabolas.
//!
//! A **fire** bolt is hot, so it *convects*: while its heat is high the
//! convective lift beats gravity and it climbs; as it cools the lift fades,
//! gravity wins, and it arcs back down. A thrown **dust/earth** clod is a
//! coalesced grain mass — ballistic, draggy (grains catch the air), and it
//! sheds cohesion when it hits something, dispersing once it's spent.
//!
//! Reuses [`medium`](crate::sim::medium) for air drag (so wind carries these too).
//! Pure, deterministic fixed-point.

use crate::sim::elemental::Element;
use crate::sim::fixed::{Fixed, Vec3};
use crate::sim::medium::{drag_dv, Medium};

/// Convective lift per unit of heat (a hot bolt of heat 1000 lifts at ~0.1/tick).
const LIFT_PER_HEAT: Fixed = Fixed::from_frac(1, 10_000);
/// Heat lost per tick (a fire bolt cools and stops rising).
const COOL_PER_TICK: i64 = 15;
/// Dust grains drag harder than a smooth body.
const DUST_DRAG: Fixed = Fixed::from_frac(12, 100);

/// A flying elemental projectile.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Projectile {
    pub pos: Vec3,
    pub vel: Vec3,
    pub element: Element,
    /// Fire: remaining heat driving convective lift (decays each tick).
    pub heat: i64,
    /// Dust/Earth: grain cohesion; drops on impact, disperses at 0.
    pub cohesion: i64,
    pub ttl: u16,
}

impl Projectile {
    /// A convecting fire bolt.
    pub fn fireball(pos: Vec3, vel: Vec3, heat: i64, ttl: u16) -> Projectile {
        Projectile { pos, vel, element: Element::Fire, heat, cohesion: 0, ttl }
    }
    /// A thrown clod of coalesced dust/earth.
    pub fn dust_clod(pos: Vec3, vel: Vec3, cohesion: i64, ttl: u16) -> Projectile {
        Projectile { pos, vel, element: Element::Dust, heat: 0, cohesion, ttl }
    }

    fn air_drag(&self) -> Fixed {
        match self.element {
            Element::Dust | Element::Earth => DUST_DRAG,
            _ => Medium::AIR.drag,
        }
    }

    /// Striking something costs the clod cohesion; once it's gone the grains
    /// scatter and it disperses.
    pub fn impact(&mut self, hardness: i64) {
        self.cohesion = (self.cohesion - hardness.max(0)).max(0);
    }

    /// A dust clod that has lost all cohesion has scattered.
    pub fn dispersed(&self) -> bool {
        matches!(self.element, Element::Dust | Element::Earth) && self.cohesion <= 0
    }

    /// Advance one tick under `gravity` (a downward, i.e. negative, accel) and a
    /// `wind` velocity. Returns whether the projectile is still alive.
    pub fn step(&mut self, gravity: Fixed, wind: Vec3) -> bool {
        if self.ttl == 0 || self.dispersed() {
            return false;
        }
        self.ttl -= 1;

        // Fire convects: lift proportional to heat, then cool.
        if self.element == Element::Fire {
            let lift = Fixed::from_int(self.heat) * LIFT_PER_HEAT;
            self.vel.y = self.vel.y + lift;
            self.heat = (self.heat - COOL_PER_TICK).max(0);
        }

        // Gravity acts on the mass.
        self.vel.y = self.vel.y + gravity;

        // Air drag pulls velocity toward the wind (so wind carries it).
        let dv = drag_dv(self.vel, wind, self.air_drag(), Fixed::ONE);
        self.vel.x = self.vel.x + dv.x;
        self.vel.y = self.vel.y + dv.y;
        self.vel.z = self.vel.z + dv.z;

        // Integrate.
        self.pos.x = self.pos.x + self.vel.x;
        self.pos.y = self.pos.y + self.vel.y;
        self.pos.z = self.pos.z + self.vel.z;

        self.ttl > 0 && !self.dispersed()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn v(x: f64, y: f64, z: f64) -> Vec3 {
        Vec3 { x: Fixed::from_f64(x), y: Fixed::from_f64(y), z: Fixed::from_f64(z) }
    }
    fn gravity() -> Fixed {
        Fixed::ZERO - Fixed::from_frac(5, 100) // -0.05/tick
    }

    #[test]
    fn a_fireball_rises_while_hot_then_arcs_down() {
        let mut f = Projectile::fireball(v(0.0, 0.0, 0.0), Vec3::ZERO, 1000, 200);
        let mut peak = f.pos.y;
        let mut peaked_then_fell = false;
        let mut prev = f.pos.y;
        for _ in 0..200 {
            if !f.step(gravity(), Vec3::ZERO) {
                break;
            }
            if f.pos.y > peak {
                peak = f.pos.y;
            }
            if f.pos.y < prev && peak > Fixed::ZERO {
                peaked_then_fell = true; // it climbed, then started descending
            }
            prev = f.pos.y;
        }
        assert!(peak > Fixed::ZERO, "the hot bolt convected upward (peak {peak:?})");
        assert!(peaked_then_fell, "and arced back down as it cooled");
    }

    #[test]
    fn a_fireball_cools_to_nothing() {
        let mut f = Projectile::fireball(v(0.0, 0.0, 0.0), Vec3::ZERO, 300, 200);
        for _ in 0..40 {
            f.step(gravity(), Vec3::ZERO);
        }
        assert_eq!(f.heat, 0, "all heat spent");
    }

    #[test]
    fn a_dust_clod_falls_ballistically() {
        let mut d = Projectile::dust_clod(v(0.0, 10.0, 0.0), Vec3::ZERO, 100, 200);
        let start = d.pos.y;
        for _ in 0..10 {
            d.step(gravity(), Vec3::ZERO);
        }
        assert!(d.pos.y < start, "dust just falls (no lift): {:?} < {:?}", d.pos.y, start);
    }

    #[test]
    fn wind_carries_a_projectile() {
        let mut d = Projectile::dust_clod(v(0.0, 50.0, 0.0), Vec3::ZERO, 100, 200);
        let wind = v(3.0, 0.0, 0.0);
        for _ in 0..20 {
            d.step(gravity(), wind);
        }
        assert!(d.vel.x > Fixed::ZERO && d.pos.x > Fixed::ZERO, "drifted downwind");
    }

    #[test]
    fn dust_loses_cohesion_on_impact_and_disperses() {
        let mut d = Projectile::dust_clod(v(0.0, 5.0, 0.0), v(2.0, 0.0, 0.0), 50, 200);
        assert!(!d.dispersed());
        d.impact(20);
        assert_eq!(d.cohesion, 30);
        d.impact(40); // more than is left
        assert!(d.dispersed(), "scattered once cohesion ran out");
        assert!(!d.step(gravity(), Vec3::ZERO), "a dispersed clod is no longer alive");
    }

    #[test]
    fn ttl_expires() {
        let mut f = Projectile::fireball(v(0.0, 0.0, 0.0), Vec3::ZERO, 100, 1);
        assert!(!f.step(gravity(), Vec3::ZERO), "last tick consumed");
        assert_eq!(f.ttl, 0);
    }
}

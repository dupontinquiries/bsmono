//! BIG BODIES — the thing that is both an entity and a map.
//!
//! A ship has to be two things that this engine keeps in different places. It
//! is an ENTITY: it moves, it has hit points, you can shoot it down. It is
//! also a MAP: you stand in its hangar, you drive down its ramp, you take
//! cover behind its spine. Cars collide against `World`'s signed distance
//! field; entities are `physics::Car`s. A ship is neither and needs to be
//! both.
//!
//! The unification is smaller than it sounds, and the engine half-invented
//! it already — `world.rs`'s portals have moving mounts that "carry their
//! HOLE with them", which is exactly this problem solved once for a doorway.
//! Generalised:
//!
//!   * A big body owns a FRAME (position, rotation, and their derivatives)
//!     and a local SDF. Its world SDF is the local one evaluated at the
//!     point pulled back into local space. That is all "map-like collider"
//!     means, and it costs one inverse transform.
//!   * Anything standing on it needs the frame's velocity AT THAT POINT,
//!     which is `vel + ang x r` — not just `vel`. Miss the cross product and
//!     a bot in the hangar of a rolling ship slides to one wall, which is
//!     the classic tell of a moving platform done cheaply.
//!   * What changes when it is shot down is only WHO INTEGRATES THE FRAME.
//!     Kinematic: the flight script drives it. Falling: gravity does, and
//!     the same world physics as everything else applies. Wrecked: nothing
//!     does, and it is terrain from then on.
//!
//! So "if something shoots you down and you crash, then the normal world
//! physics applies" is not a special case in this design — it is the default
//! case, arrived at by taking the script away.

use glam::{Quat, Vec3};

/// Who is driving the frame.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Drive {
    /// a flight script owns it: ascent, cruise, a scripted descent
    Kinematic,
    /// nobody owns it. Gravity and drag do, and it tumbles.
    Falling,
    /// it has hit something and stopped. Now it is scenery, and the cheapest
    /// scenery there is: a frame that never changes.
    Wrecked,
}

/// Position and rotation, and how fast both are changing.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    pub pos: Vec3,
    pub rot: Quat,
    pub vel: Vec3,
    /// angular velocity, radians per second, axis-angle packed into a vector
    pub ang: Vec3,
}

impl Default for Frame {
    fn default() -> Frame {
        Frame { pos: Vec3::ZERO, rot: Quat::IDENTITY, vel: Vec3::ZERO, ang: Vec3::ZERO }
    }
}

impl Frame {
    /// World point -> the body's own space.
    pub fn to_local(&self, p: Vec3) -> Vec3 {
        self.rot.inverse() * (p - self.pos)
    }
    /// The body's own space -> world.
    pub fn to_world(&self, p: Vec3) -> Vec3 {
        self.pos + self.rot * p
    }
    /// Direction only, no translation.
    pub fn dir_to_world(&self, d: Vec3) -> Vec3 {
        self.rot * d
    }

    /// HOW FAST THE SURFACE IS MOVING AT A WORLD POINT.
    ///
    /// `vel + ang x r`. The cross product is the part that is easy to skip
    /// and impossible to unsee once skipped: without it every point on the
    /// body moves at the centre's velocity, so a bot standing in a hangar
    /// forty metres off the roll axis of a banking ship slides quietly into
    /// the wall.
    pub fn carry_at(&self, p: Vec3) -> Vec3 {
        self.vel + self.ang.cross(p - self.pos)
    }

    pub fn integrate(&mut self, dt: f32) {
        self.pos += self.vel * dt;
        let w = self.ang.length();
        if w > 1e-6 {
            self.rot = (Quat::from_axis_angle(self.ang / w, w * dt) * self.rot).normalize();
        }
    }
}

/// The local shape. Deliberately crude — a big body's collider does not need
/// to be its mesh, it needs to be something an entity can stand on and take
/// cover behind, and a rounded box plus a hangar void is most ships.
#[derive(Clone, Copy, Debug)]
pub struct Shape {
    /// half extents in local space
    pub half: Vec3,
    /// corner rounding, so nothing catches on an exact edge
    pub round: f32,
    /// an open bay carved out of it: half extents and centre, local space.
    /// This is what makes the ship a MAP rather than a rock — you can be
    /// inside it.
    pub bay: Option<(Vec3, Vec3)>,
}

impl Shape {
    /// Signed distance in LOCAL space. Negative inside the hull.
    pub fn sdf_local(&self, p: Vec3) -> f32 {
        let q = p.abs() - self.half + Vec3::splat(self.round);
        let outside = q.max(Vec3::ZERO).length();
        let inside = q.max_element().min(0.0);
        let hull = outside + inside - self.round;
        match self.bay {
            // subtract the bay: max(hull, -bay) is the CSG difference, and it
            // is why you can fly into the hangar instead of bouncing off it
            Some((c, h)) => {
                let b = p - c;
                let qb = b.abs() - h;
                let bay = qb.max(Vec3::ZERO).length() + qb.max_element().min(0.0);
                hull.max(-bay)
            }
            None => hull,
        }
    }
}

/// A ship, a dropped hull section, a wreck — anything too big to be a `Car`
/// and too alive to be terrain.
pub struct BigBody {
    pub frame: Frame,
    pub shape: Shape,
    pub drive: Drive,
    /// 0..1
    pub hp: f32,
    /// how hard it hit when it landed, for the caller to turn into an event
    pub last_impact: f32,
}

impl BigBody {
    pub fn new(shape: Shape, at: Vec3) -> BigBody {
        BigBody {
            frame: Frame { pos: at, ..Frame::default() },
            shape,
            drive: Drive::Kinematic,
            hp: 1.0,
            last_impact: 0.0,
        }
    }

    /// Signed distance from a WORLD point. This is the whole map-like half:
    /// the same call signature `World::sdf` has, so anything that can
    /// collide against the world can collide against a ship.
    pub fn sdf(&self, p: Vec3) -> f32 {
        self.shape.sdf_local(self.frame.to_local(p))
    }

    /// Surface normal at a world point, by central difference.
    pub fn normal(&self, p: Vec3) -> Vec3 {
        let e = 0.05;
        let l = self.frame.to_local(p);
        let n = Vec3::new(
            self.shape.sdf_local(l + Vec3::X * e) - self.shape.sdf_local(l - Vec3::X * e),
            self.shape.sdf_local(l + Vec3::Y * e) - self.shape.sdf_local(l - Vec3::Y * e),
            self.shape.sdf_local(l + Vec3::Z * e) - self.shape.sdf_local(l - Vec3::Z * e),
        );
        self.frame.dir_to_world(n.normalize_or(Vec3::Y))
    }

    /// Is this world point inside the hull?
    pub fn contains(&self, p: Vec3) -> bool {
        self.sdf(p) < 0.0
    }

    /// Is this world point inside the open bay — aboard, but in fresh air?
    pub fn in_bay(&self, p: Vec3) -> bool {
        let Some((c, h)) = self.shape.bay else { return false };
        let b = self.frame.to_local(p) - c;
        b.abs().cmple(h).all()
    }

    /// TAKE A HIT. Enough of them and the script stops driving.
    ///
    /// Losing the drive is the entire crash mechanic: nothing switches to a
    /// crash simulation, the flight script simply stops and gravity is left
    /// holding it. The tumble comes from where the shot landed — a hit off
    /// the centreline imparts spin, which is what makes a shot-down ship
    /// look shot down rather than lowered.
    pub fn damage(&mut self, amount: f32, at: Vec3) {
        if self.drive == Drive::Wrecked {
            return;
        }
        self.hp = (self.hp - amount).max(0.0);
        if self.hp <= 0.0 && self.drive == Drive::Kinematic {
            self.drive = Drive::Falling;
            let r = at - self.frame.pos;
            // torque about the centre, scaled down by size: a big hull does
            // not spin like a coin
            let scale = 0.6 / self.shape.half.length().max(1.0);
            self.frame.ang += r.cross(Vec3::Y) * scale;
        }
    }

    /// Advance. `gravity` is the world's field at the body's position and is
    /// only consulted when nothing else is driving.
    ///
    /// `ground` returns the world distance-to-surface at a point, which is
    /// how a falling ship finds out it has arrived. Passing the world's own
    /// SDF in means a wreck lands on real terrain rather than on y=0.
    pub fn step(&mut self, dt: f32, gravity: Vec3, ground: &dyn Fn(Vec3) -> f32) {
        match self.drive {
            Drive::Wrecked => {}
            Drive::Kinematic => self.frame.integrate(dt),
            Drive::Falling => {
                self.frame.vel += gravity * dt;
                // a hull this size has real drag; without it a big ship
                // reaches silly speeds before it hits anything
                self.frame.vel *= 1.0 - (0.12 * dt).min(0.5);
                self.frame.integrate(dt);
                // contact: the lowest point of the hull, not its centre
                let reach = self.shape.half.length();
                let d = ground(self.frame.pos);
                if d < reach {
                    self.last_impact = self.frame.vel.length();
                    self.drive = Drive::Wrecked;
                    self.frame.vel = Vec3::ZERO;
                    self.frame.ang = Vec3::ZERO;
                    // settle it just clear of the surface so nothing standing
                    // on it is left inside the ground
                    self.frame.pos += -gravity.normalize_or(Vec3::Y) * (reach - d).max(0.0);
                }
            }
        }
    }

    /// Convert an entity's WORLD velocity into the body's frame at a point.
    ///
    /// The other half of "standing on a moving thing works": an entity's
    /// physics should see the velocity RELATIVE to the deck, or it is
    /// permanently sprinting to keep up with a ship doing 200 m/s.
    pub fn relative_vel(&self, p: Vec3, world_vel: Vec3) -> Vec3 {
        world_vel - self.frame.carry_at(p)
    }

    /// And back out again, once the entity's own physics has run.
    pub fn absolute_vel(&self, p: Vec3, local_vel: Vec3) -> Vec3 {
        local_vel + self.frame.carry_at(p)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ship() -> BigBody {
        BigBody::new(
            Shape {
                half: Vec3::new(12.0, 6.0, 45.0),
                round: 1.0,
                bay: Some((Vec3::new(0.0, -2.0, 20.0), Vec3::new(6.0, 3.0, 10.0))),
            },
            Vec3::new(0.0, 200.0, 0.0),
        )
    }
    fn flat() -> impl Fn(Vec3) -> f32 {
        |p: Vec3| p.y
    }

    #[test]
    fn the_hull_is_solid_wherever_the_ship_happens_to_be() {
        let mut s = ship();
        // a point at the centre is inside, and stays inside after the ship
        // has moved and rotated — that is what makes it a collider rather
        // than a bounding box in world space
        assert!(s.contains(s.frame.pos));
        s.frame.pos = Vec3::new(500.0, -30.0, 900.0);
        s.frame.rot = Quat::from_axis_angle(Vec3::X, 1.1) * Quat::from_axis_angle(Vec3::Y, 0.7);
        assert!(s.contains(s.frame.pos));
        // and a point well outside never is
        assert!(!s.contains(s.frame.pos + Vec3::new(0.0, 400.0, 0.0)));
        // the local nose stays the nose through any rotation
        let nose = s.frame.to_world(Vec3::new(0.0, 0.0, -44.0));
        assert!(s.contains(nose), "the bow stopped being solid when rotated");
    }

    #[test]
    fn the_bay_is_a_hole_you_can_be_inside() {
        let s = ship();
        let inside_bay = s.frame.to_world(Vec3::new(0.0, -2.0, 20.0));
        // in the bay means aboard but NOT inside solid hull
        assert!(s.in_bay(inside_bay));
        assert!(!s.contains(inside_bay), "the hangar is filled in");
        // hull around it is still solid
        let wall = s.frame.to_world(Vec3::new(10.0, -2.0, 20.0));
        assert!(s.contains(wall));
    }

    #[test]
    fn a_point_off_the_axis_moves_faster_than_the_centre() {
        // the cross-product term. Without it a bot in the hangar of a
        // banking ship slides into the wall.
        let mut s = ship();
        s.frame.ang = Vec3::new(0.0, 0.0, 1.5); // rolling
        let centre = s.frame.carry_at(s.frame.pos);
        let wingtip = s.frame.carry_at(s.frame.pos + Vec3::new(12.0, 0.0, 0.0));
        assert!(centre.length() < 1e-6, "the centre is moving on its own");
        assert!(wingtip.length() > 10.0, "the wingtip is not moving: {wingtip:?}");
        // and it moves perpendicular to the radius, as rotation does
        assert!(wingtip.dot(Vec3::X).abs() < 1e-4);
    }

    #[test]
    fn standing_on_the_deck_costs_nothing_while_the_ship_flies() {
        // an entity at rest RELATIVE to the deck should read as at rest,
        // however fast the ship is going
        let mut s = ship();
        s.frame.vel = Vec3::new(0.0, 0.0, -200.0);
        let p = s.frame.to_world(Vec3::new(0.0, -2.0, 20.0));
        let deck = s.frame.carry_at(p);
        assert!(s.relative_vel(p, deck).length() < 1e-3, "a standing bot is sprinting");
        // and a bot walking at 3 m/s relative comes out at 3 m/s relative
        let walk = deck + Vec3::new(3.0, 0.0, 0.0);
        assert!((s.relative_vel(p, walk).length() - 3.0).abs() < 1e-3);
        // round trip
        let back = s.absolute_vel(p, s.relative_vel(p, walk));
        assert!((back - walk).length() < 1e-3);
    }

    #[test]
    fn shooting_it_down_hands_it_to_gravity_and_nothing_else() {
        let mut s = ship();
        let g = Vec3::new(0.0, -9.81, 0.0);
        // healthy: the script drives it, gravity is ignored
        s.frame.vel = Vec3::new(0.0, 5.0, 0.0);
        for _ in 0..60 {
            s.step(1.0 / 60.0, g, &flat());
        }
        assert_eq!(s.drive, Drive::Kinematic);
        assert!(s.frame.vel.y > 4.9, "gravity leaked into a driven ship");

        // shot off the centreline: it starts falling AND tumbling
        let hit = s.frame.to_world(Vec3::new(11.0, 0.0, 0.0));
        s.damage(2.0, hit);
        assert_eq!(s.drive, Drive::Falling);
        assert!(s.frame.ang.length() > 1e-3, "it went down without spinning");

        // and now ordinary physics: it accelerates downward and lands
        let before = s.frame.pos.y;
        for _ in 0..6000 {
            s.step(1.0 / 60.0, g, &flat());
            if s.drive == Drive::Wrecked {
                break;
            }
        }
        assert_eq!(s.drive, Drive::Wrecked, "it never hit anything");
        assert!(s.frame.pos.y < before);
        assert!(s.last_impact > 5.0, "it landed like a feather: {}", s.last_impact);
    }

    #[test]
    fn a_wreck_is_terrain_and_stays_where_it_stopped() {
        let mut s = ship();
        s.hp = 0.0;
        s.drive = Drive::Falling;
        let g = Vec3::new(0.0, -9.81, 0.0);
        for _ in 0..6000 {
            s.step(1.0 / 60.0, g, &flat());
            if s.drive == Drive::Wrecked {
                break;
            }
        }
        let rest = s.frame.pos;
        for _ in 0..600 {
            s.step(1.0 / 60.0, g, &flat());
        }
        assert!((s.frame.pos - rest).length() < 1e-4, "the wreck drifted");
        assert_eq!(s.frame.vel, Vec3::ZERO);
        // still a collider: it is scenery now, not gone
        assert!(s.contains(s.frame.pos));
        // and it stopped clear of the ground rather than half-buried
        assert!(s.frame.pos.y > 0.0, "the wreck sank: {}", s.frame.pos.y);
        // further damage does nothing; it is already wreckage
        s.damage(1.0, rest);
        assert_eq!(s.drive, Drive::Wrecked);
    }

    #[test]
    fn the_normal_points_out_of_the_hull_in_world_space() {
        let mut s = ship();
        s.frame.rot = Quat::from_axis_angle(Vec3::Y, 0.9);
        // just outside the port flank in local space
        let p = s.frame.to_world(Vec3::new(13.5, 0.0, 0.0));
        let n = s.normal(p);
        let expect = s.frame.dir_to_world(Vec3::X);
        assert!(n.dot(expect) > 0.85, "normal {n:?} not along the rotated flank");
    }

    #[test]
    fn a_big_hull_does_not_spin_like_a_coin() {
        // the same off-centre hit on a lander and a carrier
        let mk = |h: Vec3| BigBody::new(Shape { half: h, round: 1.0, bay: None }, Vec3::ZERO);
        let mut small = mk(Vec3::new(4.0, 3.0, 14.0));
        let mut big = mk(Vec3::new(60.0, 30.0, 1500.0));
        small.damage(2.0, Vec3::new(4.0, 0.0, 0.0));
        big.damage(2.0, Vec3::new(60.0, 0.0, 0.0));
        assert!(small.frame.ang.length() > big.frame.ang.length(),
            "the carrier spun harder than the lander");
    }
}

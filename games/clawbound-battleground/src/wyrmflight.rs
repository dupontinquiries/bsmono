//! wyrmflight -- the Moonwyrm takes wing.
//!
//! Every few minutes, when nobody has struck it for a while, the Moonwyrm
//! uncoils from the Moonwell and flies one long figure-of-eight over the
//! whole of Starwell Glacier: over the islands, and down into the chasms
//! between them, below the bridges and the ferry clouds, skimming the cloud
//! sea. Aloft it is drawn [`FLIGHT_SCALE`] times its size in the well and
//! cannot be struck. As it flies it lets its stars fall: each lands where a
//! ring of light has marked it for [`TELEGRAPH`] seconds, and bursts in a
//! column of light that burns every cat and minion in [`IMPACT_R`] --
//! either side, anyone's fight. Most fall at random; some are aimed where a
//! cat stands (not where it will be: a cat that moves is safe).
//!
//! The path is a pure function of the flight's clock and the terrain
//! ([`pose`]), so every peer and every replay sees the same flight. Where
//! the stars fall is drawn from the match rng, like everything else random.

use crate::sim::{Event, Game, TargetRef};
use glam::Vec3;

/// The first flight, seconds after the Moonwyrm rises...
pub const FIRST_FLIGHT: f32 = 75.0;
/// ...and then this long after each landing.
pub const FLIGHT_EVERY: f32 = 150.0;
/// A flight lasts this long, climbing out and settling back included.
pub const FLIGHT_SECS: f32 = 70.0;
/// The share of the flight spent climbing out of the well (and again
/// settling back into it). No stars fall then.
pub const RISE: f32 = 0.1;
/// It takes wing only once nobody has struck it for this long.
pub const CALM_SECS: f32 = 8.0;
/// Aloft it is drawn this much bigger than in the well.
pub const FLIGHT_SCALE: f32 = 2.2;
/// A star falls this often while it flies.
pub const DROP_EVERY: f32 = 0.7;
/// Seconds between the mark appearing and the star landing.
pub const TELEGRAPH: f32 = 1.8;
/// A fallen star burns everyone within this.
pub const IMPACT_R: f32 = 6.5;
/// Stars fall within this of the ground under it...
pub const DROP_SPREAD: f32 = 55.0;
/// ...and this share of them is aimed where a cat stands.
pub const AT_CATS: f32 = 0.35;
/// Minions take this share of a fallen star's damage (32 a side: a full
/// burn would clear waves for nobody).
pub const MINION_SHARE: f32 = 0.6;
/// No star falls this close to a fountain.
pub const FOUNTAIN_CLEAR: f32 = 26.0;

/// A star on its way down.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Starfall {
    /// Where it lands (on the floor), and where it let go (the wyrm).
    pub at: Vec3,
    pub from: Vec3,
    /// Seconds until it lands.
    pub left: f32,
    pub dmg: f32,
}

impl Starfall {
    /// Where the falling star is now: it drops from the wyrm, gathering
    /// speed, and lands on its mark.
    pub fn pos(&self) -> Vec3 {
        let u = (1.0 - self.left / TELEGRAPH).clamp(0.0, 1.0);
        self.from.lerp(self.at, u * u)
    }
}

/// The ground track and the wanted height of the loop, at angle `a`
/// (a figure of eight over the map's middle, crossing above the well; the
/// bases lie beyond its ends, at z = +-252).
fn track(a: f32) -> Vec3 {
    Vec3::new(230.0 * a.sin(), 8.0 + 50.0 * (3.0 * a + 0.6).sin(), 165.0 * (2.0 * a).sin())
}

/// How high the wyrm must stay over (x, z): clear of the terrain round it.
fn clearance(x: f32, z: f32) -> f32 {
    let mut h = f32::MIN;
    for (dx, dz) in [(0.0, 0.0), (10.0, 0.0), (-10.0, 0.0), (0.0, 10.0), (0.0, -10.0), (7.0, 7.0), (-7.0, 7.0), (7.0, -7.0), (-7.0, -7.0)] {
        h = h.max(crate::sky::terrain_height(x + dx, z + dz));
    }
    h + 9.0
}

/// Samples of the loop's altitude table.
const ALT_N: usize = 2400;
/// Steepest climb or dive, height per unit flown over the ground.
const ALT_SLOPE: f32 = 0.9;

/// The loop's height along its length: the wanted curve, lifted clear of
/// the terrain, then eased so it never climbs or dives steeper than
/// `ALT_SLOPE` -- it rises ahead of an island's edge instead of jumping it.
/// (Clearance sampled pointwise steps by 30 u as a sample crosses an edge;
/// a flight built on it moved at 200 u/s for a frame.)
fn altitudes() -> &'static [f32] {
    static T: std::sync::OnceLock<Vec<f32>> = std::sync::OnceLock::new();
    T.get_or_init(|| {
        let pts: Vec<Vec3> = (0..=ALT_N).map(|i| track(i as f32 / ALT_N as f32 * std::f32::consts::TAU)).collect();
        let mut h: Vec<f32> = pts.iter().map(|p| p.y.max(clearance(p.x, p.z))).collect();
        let step = |a: Vec3, b: Vec3| Vec3::new(b.x - a.x, 0.0, b.z - a.z).length() * ALT_SLOPE;
        for i in 1..=ALT_N {
            h[i] = h[i].max(h[i - 1] - step(pts[i - 1], pts[i]));
        }
        for i in (0..ALT_N).rev() {
            h[i] = h[i].max(h[i + 1] - step(pts[i], pts[i + 1]));
        }
        h
    })
}

/// A point on the loop, `u` of the way round (0..=1).
fn on_loop(u: f32) -> Vec3 {
    let p = track(u * std::f32::consts::TAU);
    let f = u.clamp(0.0, 1.0) * ALT_N as f32;
    let i = (f as usize).min(ALT_N - 1);
    let t = altitudes();
    Vec3::new(p.x, t[i] + (t[i + 1] - t[i]) * (f - i as f32), p.z)
}

fn smooth(x: f32) -> f32 {
    let x = x.clamp(0.0, 1.0);
    x * x * (3.0 - 2.0 * x)
}

/// Where the wyrm is `secs` into a flight from the well at `home`: its
/// position, the way it faces, and how far aloft it is (0 in the well, 1 on
/// the loop) -- which the scene uses for its size.
pub fn pose(home: Vec3, secs: f32) -> (Vec3, Vec3, f32) {
    let u = (secs / FLIGHT_SECS).clamp(0.0, 1.0);
    let at = |u: f32| {
        let on_loop = on_loop(u);
        // out of the well and back into it: the loop begins and ends right
        // above it, so the climb is straight up
        let blend = smooth(u / RISE).min(smooth((1.0 - u) / RISE));
        (home.lerp(on_loop, blend), blend)
    };
    let (p, blend) = at(u);
    let (q, _) = at((u + 0.004).min(1.0));
    let (o, _) = at((u - 0.004).max(0.0));
    let mut dir = (q - o).normalize_or_zero();
    if dir.length_squared() < 0.5 {
        dir = Vec3::Z;
    }
    (p, dir, blend)
}

/// Is the wyrm far enough out that stars fall?
pub fn dropping(secs: f32) -> bool {
    let u = secs / FLIGHT_SECS;
    u > RISE && u < 1.0 - RISE
}

impl Game {
    /// The Moonwyrm's flight, each tick, for boss `bi`: take wing when it is
    /// due and calm, fly the loop dropping stars, settle back into the
    /// well. True while it is aloft (the boss tick then skips its fighting).
    pub fn wyrm_flight_tick(&mut self, bi: usize, dt: f32) -> bool {
        let now = self.t;
        let b = &self.bosses.list[bi];
        let (home, flight) = (b.home, b.flight);
        let Some(secs) = flight else {
            let calm = now - b.last_dmg_t > CALM_SECS;
            if now >= b.next_flight_t && calm {
                let b = &mut self.bosses.list[bi];
                b.flight = Some(0.0);
                b.drop_t = RISE * FLIGHT_SECS;
                b.target = None;
                self.events.push(Event::Announce { text: "THE MOONWYRM TAKES WING".into() });
                self.events.push(Event::NovaFx { pos: home, radius: crate::sky::POOL_R, color: [0.85, 0.92, 1.0] });
                return true;
            }
            return false;
        };
        let secs = secs + dt;
        if secs >= FLIGHT_SECS {
            let b = &mut self.bosses.list[bi];
            b.flight = None;
            b.pos = home;
            b.next_flight_t = now + FLIGHT_EVERY;
            self.events.push(Event::NovaFx { pos: home, radius: crate::sky::POOL_R, color: [0.85, 0.92, 1.0] });
            return false;
        }
        let (pos, dir, _) = pose(home, secs);
        {
            let b = &mut self.bosses.list[bi];
            b.flight = Some(secs);
            b.pos = pos;
            b.yaw = dir.z.atan2(dir.x);
            b.anim += dt * 3.0;
            b.drop_t -= dt;
        }
        if dropping(secs) && self.bosses.list[bi].drop_t <= 0.0 {
            self.bosses.list[bi].drop_t = DROP_EVERY;
            if let Some(at) = self.pick_starfall(pos) {
                let dmg = 140.0 + now * 0.08;
                self.starfalls.push(Starfall { at, from: pos, left: TELEGRAPH, dmg });
                self.events.push(Event::RingTelegraph { pos: at, radius: IMPACT_R, delay: TELEGRAPH, color: [0.95, 0.97, 1.0] });
            }
        }
        true
    }

    /// Where the next star falls, from a wyrm at `over`: on the floor, near
    /// the ground under it, clear of the fountains; sometimes on a cat.
    fn pick_starfall(&mut self, over: Vec3) -> Option<Vec3> {
        let floor = crate::sky::floor();
        let ok = |p: Vec3| floor.sdf(p.x, p.z) < -1.0 && [crate::consts::Team::Blue, crate::consts::Team::Red].iter().all(|t| t.fountain().distance(p) > FOUNTAIN_CLEAR);
        let ground = Vec3::new(over.x, 0.0, over.z);
        if self.rng.f32() < AT_CATS {
            let cats: Vec<Vec3> = self
                .gods
                .iter()
                .filter(|g| g.respawn_t <= 0.0 && g.skyfall.is_none() && g.pos.distance(ground) < DROP_SPREAD * 1.5)
                .map(|g| Vec3::new(g.pos.x, 0.0, g.pos.z))
                .collect();
            if !cats.is_empty() {
                let k = ((self.rng.f32() * cats.len() as f32) as usize).min(cats.len() - 1);
                if ok(cats[k]) {
                    return Some(cats[k]);
                }
            }
        }
        for _ in 0..10 {
            let a = self.rng.f32() * std::f32::consts::TAU;
            let r = DROP_SPREAD * self.rng.f32().sqrt();
            let p = ground + Vec3::new(a.cos() * r, 0.0, a.sin() * r);
            if ok(p) {
                return Some(p);
            }
        }
        None
    }

    /// Falling stars: count down, and burst on landing.
    pub fn starfalls_tick(&mut self, dt: f32) {
        if self.starfalls.is_empty() {
            return;
        }
        let mut landed = Vec::new();
        self.starfalls.retain_mut(|s| {
            s.left -= dt;
            if s.left <= 0.0 {
                landed.push(*s);
                return false;
            }
            true
        });
        for s in landed {
            self.events.push(Event::StarfallFx { at: s.at, radius: IMPACT_R });
            let near = |p: Vec3| Vec3::new(p.x - s.at.x, 0.0, p.z - s.at.z).length() < IMPACT_R;
            let cats: Vec<usize> = self.gods.iter().enumerate().filter(|(_, g)| g.respawn_t <= 0.0 && g.skyfall.is_none() && near(g.pos)).map(|(i, _)| i).collect();
            for gi in cats {
                self.deal_damage(TargetRef::God(gi), s.dmg, true, None, 0.0, 0.0);
            }
            // minions last, highest index first: a death removes one
            let mut hit: Vec<usize> = self.minions.iter().enumerate().filter(|(_, m)| m.hp > 0.0 && near(m.pos)).map(|(i, _)| i).collect();
            hit.sort_unstable_by(|a, b| b.cmp(a));
            for mi in hit {
                self.deal_damage(TargetRef::Minion(mi), s.dmg * MINION_SHARE, true, None, 0.0, 0.0);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_flight_leaves_the_well_and_comes_back_to_it() {
        crate::map::with(crate::map::MapId::Sky, || {
            let home = crate::sky::well_pos();
            assert!(pose(home, 0.0).0.distance(home) < 1e-3);
            assert!(pose(home, FLIGHT_SECS).0.distance(home) < 1e-3);
            // continuous, and it really goes somewhere
            let mut last = pose(home, 0.0).0;
            let (mut top, mut far, mut high, mut low) = (0.0f32, 0.0f32, f32::MIN, f32::MAX);
            let dt = 1.0 / 30.0;
            for k in 1..=(FLIGHT_SECS / dt) as usize {
                let (p, _, blend) = pose(home, k as f32 * dt);
                top = top.max((p - last).length() / dt);
                last = p;
                if blend > 0.99 {
                    far = far.max(Vec3::new(p.x, 0.0, p.z).length());
                    high = high.max(p.y);
                    low = low.min(p.y);
                    assert!(p.y >= crate::sky::terrain_height(p.x, p.z) + 8.0, "never through the ground: {p}");
                }
            }
            // the loop is ~1800 u long: about 50 u/s at its fastest, and a jump
            // (the first altitude rule stepped 30 u in a frame) shows as 200+
            assert!(top < 60.0, "no jumps: peak {top} u/s");
            assert!(far > 200.0, "over the whole map: {far}");
            assert!(high > 50.0, "up over the islands: {high}");
            assert!(low < -5.0, "and down into the chasms, below the bridges: {low}");
        });
    }

    #[test]
    fn stars_fall_only_out_on_the_loop() {
        assert!(!dropping(0.5) && !dropping(FLIGHT_SECS - 0.5));
        assert!(dropping(FLIGHT_SECS * 0.5));
    }

    #[test]
    fn a_falling_star_lands_on_its_mark() {
        let s = Starfall { at: Vec3::new(10.0, 0.0, 5.0), from: Vec3::new(0.0, 40.0, 0.0), left: TELEGRAPH, dmg: 1.0 };
        assert!(s.pos().distance(s.from) < 1e-3);
        assert!(Starfall { left: 0.0, ..s }.pos().distance(s.at) < 1e-3);
    }
}

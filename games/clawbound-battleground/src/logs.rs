//! logs — the river crossings are fallen logs, and logs roll.
//!
//! The wooden bridges were decoration over a causeway: `terrain_height`
//! filled the channel in wherever a lane came near, so there was no water
//! under them and nothing a bridge was for. Now the channel runs through
//! (`render::terrain_height` reads [`crossings`]) and a log lies across it.
//! On the log is dry and fast; beside it is wading, as everywhere else on the
//! river. Nothing is ever hard-blocked, so lanes and bots path as before —
//! a crossing whose log has gone is a ford, not a wall.
//!
//! ## The physics, and why it is this small
//!
//! A log across a stream can only go one interesting way: it ROLLS, sideways
//! to its own length, which here is along the river. So the state is one
//! coordinate `x` along `down`, its velocity, and the spin that rolling
//! without slipping implies. Three regimes:
//!
//! * **Seated.** It lies in a cradle the banks have worn: a spring back to
//!   the seat with damping. Walking across does nothing.
//! * **Worked loose.** Weight OFF the spine is a torque. A cat standing on
//!   the downstream shoulder of the log pushes it downstream; enough of that,
//!   or a blast beside it, carries it over the cradle's lip.
//! * **Free.** The current takes it. It rolls downstream, spinning, shedding
//!   whoever is on it once it turns faster than paws can follow, and lodges
//!   at the next snag — where it is a dry perch in mid-river and no longer a
//!   crossing on the lane.
//!
//! Simulation, not presentation: it decides who is wading. Deterministic.

use glam::Vec3;

use crate::consts::{self, Lane, Team};

/// Half-width of the cradle. Inside it a log always settles back.
pub const CRADLE_LIP: f32 = 0.9;
const CRADLE_SPRING: f32 = 16.0;
const CRADLE_DAMP: f32 = 5.0;
/// What the river does to a loose log, u/s².
const CURRENT: f32 = 2.4;
const ROLL_DRAG: f32 = 0.55;
/// A loose log lodges this far downstream.
pub const SNAG_AFTER: f32 = 24.0;
/// Rolling faster than this, nobody stays on top.
pub const SHED_SPEED: f32 = 2.2;
/// Torque per unit of weight per unit off the spine.
const LEAN: f32 = 7.5;
/// The mesh is `props::crossing_log`, built from THESE numbers, so what is
/// drawn and what is stood on cannot drift apart. A cylinder with an
/// elliptical section, 0.3 u voxels: it was `props::fallen_log` at 0.6 u --
/// half a cat per voxel, an ellipsoid whose top fell away toward both ends,
/// and one slab of moss over the whole of it.
pub const LOG_VOXEL: f32 = 0.3;
pub const LOG_HALF_LEN: f32 = 6.6;
/// Section half-width and half-height, in voxels.
pub const SECTION_A: f32 = 6.8;
pub const SECTION_B: f32 = 4.6;
/// Nominal rolling radius for the physics.
pub const LOG_RADIUS: f32 = 1.9;
/// Half the width of the channel cut through the causeway under a log.
/// Less than LOG_HALF_LEN by enough that both ends bear on the road.
pub const SLUICE_HALF: f32 = 3.6;
/// How far the top of a seated log stands above the lane.
pub const LOG_TOP: f32 = 0.55;

/// Height of the top FACE of the voxel column at lateral offset `off`,
/// above the section's axis. Mirrors `props::crossing_log`: voxel `j` of
/// column `k` is filled when `(j/B)^2 + (k/A)^2 <= 1` about the axis.
pub fn column_top(off: f32) -> f32 {
    let k = (off / LOG_VOXEL).round();
    let room = 1.0 - (k / SECTION_A) * (k / SECTION_A);
    if room <= 0.0 {
        return 0.0;
    }
    ((SECTION_B * room.sqrt()).floor() + 0.5) * LOG_VOXEL
}

/// Where a side lane meets its river: the point, and the lane's direction there.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Crossing {
    pub at: Vec3,
    pub along: Vec3,
}

/// The two crossings (Top lane x west river, Bot lane x east river).
///
/// Cached: `terrain_height` asks on every call, and that is the hottest
/// function in world building.
pub fn crossings() -> &'static [Crossing] {
    if crate::map::active() != crate::map::MapId::Jungle {
        return &[]; // the pass's streams are narrow enough to wade
    }
    static CACHE: std::sync::OnceLock<Vec<Crossing>> = std::sync::OnceLock::new();
    CACHE.get_or_init(|| [(-1.0f32, Lane::Top), (1.0, Lane::Bot)].into_iter().filter_map(|(s, l)| find_crossing(s, l)).collect())
}

/// Moved from `scene::bridge_crossing`, unchanged in what it picks: the
/// lane point nearest the river's centreline, clear of the lane's towers.
fn find_crossing(side: f32, lane: Lane) -> Option<Crossing> {
    let pts = lane.polyline();
    let mut best: Option<(Crossing, f32)> = None;
    for w in pts.windows(2) {
        let (a, b) = (w[0], w[1]);
        let ab = b - a;
        for i in 0..=48 {
            let p = a + ab * (i as f32 / 48.0);
            let diff = (p.x - consts::river_x(side, p.z)).abs();
            if diff > 9.0 {
                continue;
            }
            let near_tower = [Team::Blue, Team::Red].iter().any(|t| [false, true].iter().any(|inner| (consts::tower_pos(lane, *t, *inner) - p).length() < 13.0));
            if near_tower {
                continue;
            }
            if best.map_or(true, |(_, d)| diff < d) {
                best = Some((Crossing { at: p, along: ab.normalize_or_zero() }, diff));
            }
        }
    }
    best.map(|b| b.0)
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct RollingLog {
    /// Where it rests, on the ground plane.
    pub seat: Vec3,
    /// Unit, along the log: bank to bank, which is the lane's direction.
    pub axis: Vec3,
    /// Unit, the way it rolls when the river has it.
    pub down: Vec3,
    /// Displacement from the seat along `down`.
    pub x: f32,
    pub v: f32,
    /// Spin about its own axis, radians. Rolling without slipping.
    pub angle: f32,
    pub free: bool,
    /// Has left the lane for good and lodged downstream.
    pub lodged: bool,
}

/// Something standing on a log this tick.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Rider {
    /// How far off the spine, along `down`. Positive is the downstream shoulder.
    pub off: f32,
    pub weight: f32,
}

impl RollingLog {
    pub fn across(c: &Crossing) -> RollingLog {
        let axis = Vec3::new(c.along.x, 0.0, c.along.z).normalize_or_zero();
        // Perpendicular to the log; pick the sense that points downstream
        // (the rivers run north to south, toward -z).
        let mut down = Vec3::new(-axis.z, 0.0, axis.x);
        if down.z > 0.0 {
            down = -down;
        }
        RollingLog { seat: Vec3::new(c.at.x, 0.0, c.at.z), axis, down, x: 0.0, v: 0.0, angle: 0.0, free: false, lodged: false }
    }

    pub fn centre(&self) -> Vec3 {
        self.seat + self.down * self.x
    }

    /// Is `(x, z)` on top of the log? Returns how far off the spine it is.
    pub fn on_top(&self, x: f32, z: f32) -> Option<f32> {
        let rel = Vec3::new(x, 0.0, z) - self.centre();
        let (along, off) = (rel.dot(self.axis), rel.dot(self.down));
        (along.abs() <= LOG_HALF_LEN && off.abs() <= LOG_RADIUS * 0.85).then_some(off)
    }

    /// World Y of the log's upper surface under `(x, z)`, or `None` off it.
    ///
    /// Seated, this is the top face of the actual voxel column under the
    /// paw -- the same inequality the mesher fills by -- so a sole rests ON a
    /// voxel, not in it or over it. Turned, the voxels turn too, and it is
    /// the exact top of the rotated ellipse (the upper root of the line
    /// x = off against it).
    pub fn surface_y(&self, x: f32, z: f32) -> Option<f32> {
        let off = self.on_top(x, z)?;
        let (a, b) = (SECTION_A * LOG_VOXEL, SECTION_B * LOG_VOXEL);
        let axis_y = LOG_TOP - column_top(0.0);
        if self.angle.abs() < 0.02 {
            return Some(axis_y + column_top(off));
        }
        let (s, c) = self.angle.sin_cos();
        let qa = s * s / (a * a) + c * c / (b * b);
        let qb = 2.0 * off * s * c * (1.0 / (a * a) - 1.0 / (b * b));
        let qc = off * off * (c * c / (a * a) + s * s / (b * b)) - 1.0;
        let disc = qb * qb - 4.0 * qa * qc;
        (disc >= 0.0).then(|| axis_y + (-qb + disc.sqrt()) / (2.0 * qa))
    }

    /// Too fast to stand on.
    pub fn shedding(&self) -> bool {
        self.v.abs() > SHED_SPEED
    }

    /// A blast at `from`. Only the part of the push that lies along `down`
    /// can roll a log; a blast off one END of it does nothing, which is
    /// right — that is pushing a cylinder along its own length.
    pub fn shove(&mut self, from: Vec3, radius: f32, power: f32) {
        if self.lodged || !from.is_finite() || !(radius > 0.0) || !(power > 0.0) {
            return;
        }
        let rel = self.centre() - Vec3::new(from.x, 0.0, from.z);
        let dist = rel.length();
        if dist > radius + LOG_HALF_LEN {
            return;
        }
        let dir = if dist > 0.05 { rel / dist } else { self.down };
        let fall = (1.0 - dist / (radius + LOG_HALF_LEN)).clamp(0.0, 1.0);
        // 12, not 9: at 9 a radius-6 blast three units away gave 6.85 u/s, and
        // the cradle (w = 4, zeta = 0.625) turns that into a peak excursion of
        // 0.835 u -- under the 0.9 u lip. A log that shrugs off an explosion
        // beside it is not what was asked for.
        self.v += dir.dot(self.down) * power.min(1.5) * fall * 12.0;
    }

    /// Advance. `riders` are whoever [`Self::on_top`] says is aboard.
    pub fn tick(&mut self, dt: f32, riders: &[Rider]) {
        if self.lodged {
            self.v = 0.0;
            return;
        }
        let dt = if dt.is_finite() { dt.clamp(0.0, 0.05) } else { 0.0 };
        let lean: f32 = riders.iter().map(|r| r.weight.clamp(0.0, 4.0) * (r.off / LOG_RADIUS).clamp(-1.0, 1.0)).filter(|t| t.is_finite()).sum::<f32>() * LEAN;
        let a = if self.free { CURRENT + lean - ROLL_DRAG * self.v } else { -CRADLE_SPRING * self.x - CRADLE_DAMP * self.v + lean };
        self.v += a * dt;
        let dx = self.v * dt;
        self.x += dx;
        self.angle = (self.angle + dx / LOG_RADIUS) % std::f32::consts::TAU;
        if !self.free && self.x.abs() > CRADLE_LIP {
            self.free = true;
        }
        if self.free && self.x >= SNAG_AFTER {
            // Lodged: this is where it lives now.
            self.seat += self.down * SNAG_AFTER;
            self.x = 0.0;
            self.v = 0.0;
            self.free = false;
            self.lodged = true;
        }
    }
}

impl crate::sim::Game {
    /// Is this point on a crossing log (and so dry)?
    pub fn on_a_log(&self, x: f32, z: f32) -> bool {
        self.logs.iter().any(|l| l.on_top(x, z).is_some())
    }

    /// Blasts shove the logs, riders lean on them, the river takes the loose
    /// ones, and whoever is aboard goes where the log goes -- or off it.
    pub fn logs_tick(&mut self, dt: f32) {
        // Events pile up until the shell drains them; only look at new ones.
        if self.events.len() < self.logs_seen {
            self.logs_seen = 0;
        }
        for e in &self.events[self.logs_seen..] {
            let (pos, radius, power) = match e {
                crate::sim::Event::Explosion { pos, radius, .. } => (*pos, *radius, (*radius / 5.0).clamp(0.2, 1.2)),
                crate::sim::Event::NovaFx { pos, radius, .. } => (*pos, *radius * 0.7, (*radius / 9.0).clamp(0.2, 1.0)),
                _ => continue,
            };
            for l in &mut self.logs {
                l.shove(pos, radius, power);
            }
        }
        self.logs_seen = self.events.len();

        for li in 0..self.logs.len() {
            let log = self.logs[li];
            let mut riders = Vec::new();
            for g in self.gods.iter().filter(|g| g.respawn_t <= 0.0 && g.leap.is_none() && g.fall.is_none() && g.perch.is_none()) {
                if let Some(off) = log.on_top(g.pos.x, g.pos.z) {
                    riders.push(Rider { off, weight: 1.0 });
                }
            }
            for m in &self.minions {
                if let Some(off) = log.on_top(m.pos.x, m.pos.z) {
                    riders.push(Rider { off, weight: 0.5 });
                }
            }
            let before = log.centre();
            self.logs[li].tick(dt, &riders);
            let after = self.logs[li];
            let carried = after.centre() - before;
            if carried.length_squared() < 1e-10 {
                continue;
            }
            let shed = after.shedding();
            for (i, g) in self.gods.iter_mut().enumerate() {
                if g.respawn_t > 0.0 || g.leap.is_some() || g.fall.is_some() || g.perch.is_some() || log.on_top(g.pos.x, g.pos.z).is_none() {
                    continue;
                }
                if shed {
                    // Thrown off the UPSTREAM side, on its back -- and, being a
                    // cat, into the water on its feet. Upstream because the top
                    // of a rolling log moves forward at twice the log's speed:
                    // paws go with it and the cat goes off the back. I had this
                    // as downstream, where the log is GOING; it rolled over the
                    // cat, picked it up and threw it again, all the way down
                    // the river. The end-to-end test caught the juggling.
                    let sign = if i % 2 == 0 { 1.0 } else { -1.0 };
                    g.pos -= after.down * after.v.signum() * (LOG_RADIUS + 0.4);
                    g.pos.y = LOG_TOP;
                    g.fall = Some(crate::leap::Fall::new(LOG_TOP + 0.6, sign * 2.0));
                } else {
                    g.pos += carried;
                }
            }
            for m in self.minions.iter_mut().filter(|m| log.on_top(m.pos.x, m.pos.z).is_some()) {
                if !shed {
                    m.pos += carried;
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn log() -> RollingLog {
        RollingLog::across(&Crossing { at: Vec3::new(-75.0, 0.0, -40.0), along: Vec3::new(-0.6, 0.0, 0.8) })
    }
    fn run(l: &mut RollingLog, secs: f32, riders: &[Rider]) {
        for _ in 0..(secs * 60.0) as usize {
            l.tick(1.0 / 60.0, riders);
        }
    }

    #[test]
    /// The log has to SPAN. Water under its middle, road under both its
    /// ends, and a ford -- not a raft -- is what a screenshot showed instead.
    #[test]
    fn a_seated_log_has_water_under_its_middle_and_road_under_both_ends() {
        for c in crossings() {
            let l = RollingLog::across(c);
            assert!(consts::water_depth(c.at.x, c.at.z) > 1.0, "no water under the middle: {}", consts::water_depth(c.at.x, c.at.z));
            for end in [-1.0f32, 1.0] {
                let p = l.seat + l.axis * end * (LOG_HALF_LEN - 0.4);
                assert!(consts::water_depth(p.x, p.z) < 0.05, "end {end} of the log is over {} u of water, not on the road", consts::water_depth(p.x, p.z));
            }
            // And the gap is wet on BOTH sides of the log: water goes through.
            for side in [-1.0f32, 1.0] {
                let p = l.seat + l.down * side * (LOG_RADIUS + 2.0);
                assert!(consts::water_depth(p.x, p.z) > 0.8, "the sluice is dry on side {side}");
            }
        }
    }

    fn there_are_two_crossings_and_each_lies_on_its_lane_over_its_river() {
        let c = crossings();
        assert_eq!(c.len(), 2);
        for k in c {
            assert!(consts::lane_clearance(k.at.x, k.at.z) < 0.5, "on the lane: {}", k.at);
            assert!(consts::river_dist(k.at.x, k.at.z) < 9.0, "over the river: {}", consts::river_dist(k.at.x, k.at.z));
            assert!((k.along.length() - 1.0).abs() < 1e-3);
        }
        assert!(c[0].at.x < 0.0 && c[1].at.x > 0.0, "one each side");
    }

    /// A crossing has to survive being crossed. A file of minions down the
    /// spine, and even one cat stood on the shoulder, leave it seated.
    #[test]
    fn walking_across_does_not_move_it() {
        let mut l = log();
        let column: Vec<Rider> = (0..6).map(|i| Rider { off: if i % 2 == 0 { 0.15 } else { -0.15 }, weight: 0.5 }).collect();
        run(&mut l, 6.0, &column);
        assert!(!l.free && l.x.abs() < 0.05, "a minion wave moved it {}", l.x);
        run(&mut l, 6.0, &[Rider { off: 1.2, weight: 1.0 }]);
        assert!(!l.free, "one cat on the shoulder worked it loose (x = {})", l.x);
        assert!((0.1..CRADLE_LIP).contains(&l.x), "but it does LEAN under them: {}", l.x);
        run(&mut l, 3.0, &[]);
        assert!(l.x.abs() < 0.03, "and settles back when they step off");
    }

    /// Enough weight on one shoulder and it goes. This is the play: three
    /// cats crowding the downstream side, or a guardian and a friend.
    #[test]
    fn enough_weight_off_the_spine_works_it_loose_and_the_river_takes_it() {
        let mut l = log();
        let crowd = [Rider { off: 1.4, weight: 1.0 }; 3];
        // Measure from the SEAT. These riders are static test data and keep
        // leaning on a log that, in the game, would have thrown them the
        // moment it turned -- so by the end of this run it is already well
        // downstream, and measuring from there said "6 u".
        let start = l.centre();
        let mut t0 = 0.0;
        while !l.free {
            l.tick(1.0 / 60.0, &crowd);
            t0 += 1.0 / 60.0;
            assert!(t0 < 5.0, "three cats on the downstream shoulder did not shift it: x = {}", l.x);
        }
        let mut shed = false;
        let mut t = 0.0;
        while !l.lodged {
            l.tick(1.0 / 60.0, &[]);
            shed |= l.shedding();
            t += 1.0 / 60.0;
            assert!(t < 60.0, "a loose log must come to rest");
        }
        assert!(shed, "it never turned fast enough to throw anyone");
        let moved = l.centre() - start;
        assert!((moved.length() - SNAG_AFTER).abs() < 1.5 && moved.dot(l.down) > 0.0, "lodged {} downstream", moved.length());
        assert!(moved.z < 0.0, "downstream is south: {moved}");
        assert!((4.0..20.0).contains(&t), "took {t}s");
        // Lodged is final: nothing shifts it again.
        l.shove(l.centre() - l.down * 2.0, 8.0, 1.5);
        run(&mut l, 2.0, &crowd);
        assert!(l.lodged && l.v == 0.0);
    }

    /// A blast beside it rolls it; the same blast off its END does not.
    #[test]
    fn a_blast_rolls_a_log_sideways_and_cannot_push_it_lengthways() {
        let mut side = log();
        let upstream = side.centre() - side.down * 3.0;
        side.shove(upstream, 6.0, 1.0);
        assert!(side.v > 3.0, "a blast on the upstream side sends it downstream: v = {}", side.v);
        run(&mut side, 1.0, &[]);
        assert!(side.free);

        let mut end = log();
        let off_the_end = end.centre() + end.axis * (LOG_HALF_LEN + 2.0);
        end.shove(off_the_end, 6.0, 1.0);
        assert!(end.v.abs() < 0.05, "pushed along its own length: v = {}", end.v);

        let mut weak = log();
        weak.shove(upstream, 6.0, 0.15);
        run(&mut weak, 3.0, &[]);
        assert!(!weak.free && weak.x.abs() < 0.05, "a tap rocks it and it settles");
        let mut far = log();
        far.shove(far.centre() - far.down * 40.0, 6.0, 1.0);
        assert_eq!(far.v, 0.0);
    }

    /// Footing is read off the same shape the mesh is built from. Across a
    /// seated log every sampled paw position lands exactly on a voxel face;
    /// the spine is the high point; and a turned log still has a top.
    #[test]
    fn paws_land_on_voxel_faces_and_the_spine_is_the_high_point() {
        let l = log();
        let spine = l.surface_y(l.seat.x, l.seat.z).unwrap();
        assert!((spine - LOG_TOP).abs() < 1e-4, "the spine stands LOG_TOP above the lane: {spine}");
        let mut checked = 0;
        let mut last = f32::MAX;
        for i in 0..=16 {
            let off = i as f32 * 0.1;
            let p = l.seat + l.down * off;
            let y = l.surface_y(p.x, p.z).expect("on the log");
            let faces = (y - spine) / LOG_VOXEL;
            assert!((faces - faces.round()).abs() < 1e-3, "off {off}: surface {y} is not on a voxel face ({faces} voxels from the spine)");
            assert!(y <= last + 1e-5 && y <= spine + 1e-5, "it only falls away from the spine");
            last = y;
            checked += 1;
        }
        assert_eq!(checked, 17);
        assert!(spine - last > 0.25 && spine - last < 1.0, "the shoulder is lower, not a cliff: {}", spine - last);
        // Same height all the way along: a cylinder, not the old ellipsoid.
        let far = l.seat + l.axis * (LOG_HALF_LEN - 0.3);
        assert_eq!(l.surface_y(far.x, far.z), Some(spine));
        // Turned a quarter, the long side of the section is up.
        let mut t = l;
        t.angle = std::f32::consts::FRAC_PI_2;
        let up = t.surface_y(t.seat.x, t.seat.z).unwrap();
        assert!(up > spine + 0.4, "on its side it stands taller: {up} vs {spine}");
    }

    /// The other half of "drawn and stood-on cannot drift apart": build the
    /// REAL mesh and hold its top faces against `column_top`. Nothing in the
    /// suite built this mesh before (Scene needs a GPU), so an overflow in
    /// its bark hash and a half-voxel sideways offset both got through.
    #[test]
    fn the_mesh_that_is_drawn_has_its_top_where_footing_says() {
        let mesh = crate::props::crossing_log();
        // A hull: `Grid::mesh` no longer emits the inside. (Was > 2000, solid.)
        assert!(mesh.inst.len() > 1000, "{} voxels", mesh.inst.len());
        let ny = (SECTION_B as usize) * 2 + 3;
        let axis = ((ny / 2) as f32 + 0.5) * LOG_VOXEL;
        let mut checked = 0;
        for k in -5i32..=5 {
            let off = k as f32 * LOG_VOXEL;
            let top = mesh.inst.iter().filter(|v| (v.off_ao[2] - off).abs() < 0.01 && v.off_ao[0].abs() < 1.0).map(|v| v.off_ao[1] + LOG_VOXEL * 0.5).fold(f32::MIN, f32::max);
            assert!(top > f32::MIN, "no voxel column at lateral offset {off}: the mesh is not centred on a column");
            assert!((top - axis - column_top(off)).abs() < 1e-3, "column {k}: mesh top {} above the axis, footing says {}", top - axis, column_top(off));
            checked += 1;
        }
        assert_eq!(checked, 11);
        // Not a green slab: moss is a minority of what faces up.
        let up: Vec<_> = mesh.inst.iter().filter(|v| v.off_ao[1] > axis + LOG_VOXEL).collect();
        let mossy = up.iter().filter(|v| v.color[1] > v.color[0] * 1.5).count();
        assert!((mossy as f32) < up.len() as f32 * 0.1, "{mossy} of {} upward voxels are moss", up.len());
    }

    #[test]
    fn it_rolls_without_slipping_and_footing_follows_the_log() {
        let mut l = log();
        assert_eq!(l.on_top(l.seat.x, l.seat.z), Some(0.0));
        let end = l.seat + l.axis * (LOG_HALF_LEN - 0.1);
        assert!(l.on_top(end.x, end.z).is_some());
        let beside = l.seat + l.down * (LOG_RADIUS + 0.5);
        assert_eq!(l.on_top(beside.x, beside.z), None, "beside it is in the river");
        l.shove(l.centre() - l.down * 3.0, 6.0, 1.0);
        run(&mut l, 1.5, &[]);
        assert!((l.x / LOG_RADIUS - l.angle).abs() < 1e-3 || l.angle < l.x / LOG_RADIUS, "spin tracks travel");
        assert_eq!(l.on_top(l.seat.x, l.seat.z), None, "the lane is no longer under a log");
        let c = l.centre();
        assert!(l.on_top(c.x, c.z).is_some(), "the footing went with it");
        for bad in [f32::NAN, -1.0] {
            let mut g = log();
            g.shove(Vec3::splat(bad), bad, bad);
            g.tick(bad, &[Rider { off: bad, weight: bad }]);
            assert!(g.x.is_finite() && g.v.is_finite(), "garbage in: {g:?}");
        }
    }
}

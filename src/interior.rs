//! SHIP INTERIORS, generated from the room graph.
//!
//! `hub_plan.h` gives eight rooms, their kinds, their positions and which
//! doors connect to which — and that is a floor plan, not a menu. It was
//! ported as data in `content::HUB`; this turns it into somewhere you can
//! stand.
//!
//! The plan's coordinates span about 24 units, which is a diagram rather
//! than a ship. Everything here scales off the real hull length, so the same
//! eight rooms make a cramped lander and a carrier you get lost in. What
//! does NOT scale is clearance: a corridor is at least wide and tall enough
//! to drive a vehicle down, on every hull, because a ship whose spine you
//! can only walk is a ship whose hangar is decorative.
//!
//! The interior is carved rather than built. Start with solid hull, subtract
//! room volumes and the corridors between them, and the result is an SDF
//! that `BigBody` can already use — same `max(hull, -void)` trick as the
//! hangar bay, applied eight times. Nothing new has to understand it.
//!
//! Fixtures come from the room kind, because the kinds already say what each
//! place is for: a bridge gets the holo table and the forward glass, a
//! corridor gets viewports down both sides (the plan literally says
//! "Viewports on both sides"), the airlock gets an EVA platform on the OUTER
//! hull for free-flying, the trade deck gets stalls, the upgrade bay gets
//! rigs.

use glam::Vec3;

use crate::airlock::{Airlock, Door, Gauge, Kind};
use crate::content::{RoomKind, HUB};

/// Smallest corridor a vehicle can use, half-extents in metres. A skydribble
/// car is about 1.1 wide and 0.6 tall at `car.size` 1, so this is roughly
/// two-and-a-half car widths — enough to drive without scraping and to turn
/// at a junction.
pub const DRIVE_HALF_W: f32 = 2.8;
pub const DRIVE_HALF_H: f32 = 1.9;

/// What a room is for, and therefore what is in it.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Fixture {
    /// the planning table: a horizontal holographic plate you stand around
    HoloTable,
    /// glass. Big enough to watch a planet through.
    Viewport,
    /// chassis rigs and part racks
    UpgradeRig,
    /// a visitor selling something exotic
    VendorStall,
    /// crew bunks; where quests come from
    Bunk,
    /// an external platform, outside the pressure hull, for launching into
    /// vacuum as a flyer
    EvaPlatform,
    /// the drop door itself
    LaunchDoor,
}

impl Fixture {
    pub fn label(self) -> &'static str {
        match self {
            Fixture::HoloTable => "PLOTTING TABLE",
            Fixture::Viewport => "VIEWPORT",
            Fixture::UpgradeRig => "CHASSIS RIG",
            Fixture::VendorStall => "TRADE STALL",
            Fixture::Bunk => "BUNK",
            Fixture::EvaPlatform => "EVA PLATFORM",
            Fixture::LaunchDoor => "LAUNCH DOOR",
        }
    }
    /// Is it outside the pressure hull?
    pub fn exterior(self) -> bool {
        matches!(self, Fixture::EvaPlatform)
    }
}

fn fixtures_for(kind: RoomKind) -> &'static [Fixture] {
    match kind {
        RoomKind::Bridge => &[Fixture::HoloTable, Fixture::Viewport],
        RoomKind::Corridor => &[Fixture::Viewport],
        RoomKind::Upgrade => &[Fixture::UpgradeRig],
        RoomKind::Vendor => &[Fixture::VendorStall],
        RoomKind::Quest => &[Fixture::Bunk],
        RoomKind::Spacewalk => &[Fixture::EvaPlatform],
        RoomKind::Hangar => &[Fixture::LaunchDoor],
    }
}

/// A room, placed and sized in hull-local metres.
#[derive(Clone, Debug)]
pub struct Cell {
    pub id: &'static str,
    pub name: &'static str,
    pub kind: RoomKind,
    pub centre: Vec3,
    pub half: Vec3,
    pub fixtures: &'static [Fixture],
    pub blurb: &'static str,
}

/// A corridor between two rooms, as a box, with the door in it.
#[derive(Clone, Copy, Debug)]
pub struct Link {
    pub a: usize,
    pub b: usize,
    pub centre: Vec3,
    pub half: Vec3,
    /// every doorway has a door; a corridor with no door is a tube
    pub door: Door,
}

pub struct Interior {
    pub cells: Vec<Cell>,
    pub links: Vec<Link>,
    /// the hull this was fitted into
    pub hull_half: Vec3,
    /// where the deck is, in local Y
    pub deck_y: f32,
    /// false when the hull is too small to hold a corridor a vehicle fits
    /// down — a lander is walk-only and that is not a bug
    pub drive_clearance: bool,
    /// the ship's airlock, sited at the spacewalk room. This is the one
    /// door pair that is a mechanic rather than a passage.
    pub lock: Airlock,
}

fn axis_box(a: Vec3, b: Vec3, half_w: f32, half_h: f32) -> (Vec3, Vec3) {
    let centre = (a + b) * 0.5;
    let d = (b - a).abs() * 0.5;
    // corridors run along whichever axis separates the two rooms; the other
    // two get the clearance minimum
    let half = if d.x > d.z {
        Vec3::new(d.x.max(half_w), half_h, half_w)
    } else {
        Vec3::new(half_w, half_h, d.z.max(half_w))
    };
    (centre, half)
}

impl Interior {
    /// Lay the plan out inside a hull of this length.
    ///
    /// The plan's own coordinates span roughly 24 units nose to tail, so the
    /// scale factor is however many metres of ship there are per plan unit.
    /// Rooms get a share of the beam; corridors get whatever clearance is
    /// larger, the scaled one or the minimum a vehicle needs.
    pub fn generate(hull_length: f32) -> Interior {
        let hull_half = Vec3::new(hull_length * 0.16, hull_length * 0.09, hull_length * 0.5);
        // SCALE FROM THE SPACE LEFT OVER, not from the plan's own span. The
        // first version scaled so the bridge sat at 0.94 of the hull's half
        // length, which left three metres for a room that needed ten, and
        // every room poked out of the ship. Work out how far the furthest
        // room centre is in plan units, then choose a scale that leaves a
        // real share of the hull for the rooms themselves.
        let reach_x = HUB.iter().fold(0.0f32, |m, r| m.max(r.pos[0].abs())).max(1.0);
        let reach_z = HUB.iter().fold(0.0f32, |m, r| m.max(r.pos[2].abs())).max(1.0);
        let s = ((hull_half.x * 0.55) / reach_x).min((hull_half.z * 0.72) / reach_z);
        let deck_y = -hull_half.y * 0.25;

        // SMALL HULLS CANNOT BE DRIVEN, and saying so is better than
        // pretending. A 28 m lander is 5 m in the beam and 2.5 tall: a
        // corridor with vehicle clearance would be wider than the ship. So
        // clearance is a floor only where the hull can hold it, and where it
        // cannot the interior is honestly walk-only. The first version of
        // this clamped min against max and panicked, which was the hull
        // telling me the same thing less politely.
        // what is left between the outermost room centre and the skin
        let cap = Vec3::new(
            (hull_half.x - reach_x * s).max(0.4),
            hull_half.y * 0.6,
            (hull_half.z - reach_z * s).max(0.4),
        );
        let drive_clearance = cap.x >= DRIVE_HALF_W && cap.y >= DRIVE_HALF_H;
        let floor = if drive_clearance {
            Vec3::new(DRIVE_HALF_W, DRIVE_HALF_H, DRIVE_HALF_W)
        } else {
            cap * 0.55
        };
        let room_half = Vec3::new(
            (4.0 * s).max(floor.x).min(cap.x),
            (2.6 * s).max(floor.y).min(cap.y),
            (4.0 * s).max(floor.z).min(cap.z),
        );
        let cells: Vec<Cell> = HUB
            .iter()
            .map(|r| Cell {
                id: r.id,
                name: r.name,
                kind: r.kind,
                centre: Vec3::new(r.pos[0] * s, deck_y, r.pos[2] * s),
                // the bridge and hangar are the big rooms; corridors are thin
                half: match r.kind {
                    // the big rooms, still inside the skin
                    RoomKind::Bridge | RoomKind::Hangar => (room_half * 1.35).min(cap),
                    // a corridor is narrow, but never narrower than the
                    // corridors joining it — the first version made the
                    // spine of a 110 m ship 2.37 m half-wide while the links
                    // into it were 2.8, so you could drive between rooms and
                    // not through the one in the middle
                    RoomKind::Corridor => Vec3::new(
                        (room_half.x * 0.55).max(floor.x),
                        room_half.y,
                        room_half.z * 1.2,
                    )
                    .min(cap),
                    _ => room_half,
                },
                fixtures: fixtures_for(r.kind),
                blurb: r.blurb,
            })
            .collect();

        let cw = (room_half.x * 0.5).max(floor.x).min(cap.x);
        let ch = (room_half.y * 0.8).max(floor.y).min(cap.y);
        let mut links = Vec::new();
        for (i, r) in HUB.iter().enumerate() {
            for e in r.exits {
                let Some(j) = HUB.iter().position(|o| o.id == *e) else { continue };
                if j < i {
                    continue; // doors are two-way; carve each once
                }
                let (centre, half) = axis_box(cells[i].centre, cells[j].centre, cw, ch);
                // GAUGE FOLLOWS THE ROOMS IT JOINS. A door onto the hangar
                // has to pass what the hangar launches; a door into crew
                // quarters does not, and a corridor nobody could drive gets
                // a crew hatch regardless of what it connects.
                let touches = |k: RoomKind| cells[i].kind == k || cells[j].kind == k;
                let gauge = if !drive_clearance {
                    Gauge::Personnel
                } else if touches(RoomKind::Hangar) {
                    Gauge::Launch
                } else if touches(RoomKind::Upgrade) || touches(RoomKind::Corridor) {
                    Gauge::Vehicle
                } else {
                    Gauge::Personnel
                };
                let kind = if touches(RoomKind::Spacewalk) {
                    Kind::Bulkhead
                } else if touches(RoomKind::Hangar) {
                    Kind::Launch
                } else {
                    Kind::Hatch
                };
                let door = Door::new(kind, gauge, centre, half * 0.8);
                links.push(Link { a: i, b: j, centre, half, door });
            }
        }
        // the lock sits at the spacewalk room, outboard, and is sized to
        // what the ship can launch: a carrier's is a room you fly out of
        let air = cells.iter().find(|c| c.kind == RoomKind::Spacewalk);
        let lock = match air {
            Some(c) => Airlock::new(
                c.centre + Vec3::new(c.half.x, 0.0, 0.0),
                c.half * 0.8,
                if drive_clearance { Gauge::Vehicle } else { Gauge::Personnel },
            ),
            None => Airlock::new(Vec3::ZERO, Vec3::ONE, Gauge::Personnel),
        };
        Interior { cells, links, hull_half, deck_y, drive_clearance, lock }
    }

    fn box_sdf(p: Vec3, c: Vec3, h: Vec3) -> f32 {
        let q = (p - c).abs() - h;
        q.max(Vec3::ZERO).length() + q.max_element().min(0.0)
    }

    /// Signed distance in hull-local space: negative inside solid structure.
    ///
    /// Solid hull, minus every room and corridor. Same `max(hull, -void)`
    /// carve `BigBody` already does for the hangar bay, eight-plus times, so
    /// nothing downstream needs to learn a new shape.
    pub fn sdf_local(&self, p: Vec3) -> f32 {
        let q = p.abs() - self.hull_half;
        let mut d = q.max(Vec3::ZERO).length() + q.max_element().min(0.0);
        for c in &self.cells {
            d = d.max(-Self::box_sdf(p, c.centre, c.half));
        }
        for l in &self.links {
            // A SHUT DOOR IS HULL. The corridor is only carved as far as its
            // door has travelled, so walking into a closed one is walking
            // into structure rather than through a picture of a door.
            match l.door.void() {
                Some((c, h)) => {
                    d = d.max(-Self::box_sdf(p, l.centre, l.half.min(h * 4.0)));
                    let _ = c;
                }
                None => {}
            }
        }
        d
    }

    /// Open every internal door — the state a ship sits in at rest, and what
    /// the layout tests want to reason about.
    pub fn unseal(&mut self) {
        for l in self.links.iter_mut() {
            l.door.open();
            l.door.travel = 1.0;
            l.door.state = crate::airlock::State::Open;
        }
    }

    /// Cut power to everything. Hatches and bulkheads shut; the launch bay
    /// opens. That divergence is the whole point of the failure modes.
    pub fn depower(&mut self) {
        for l in self.links.iter_mut() {
            l.door.depower();
        }
        self.lock.inner.depower();
        self.lock.outer.depower();
    }

    pub fn step_doors(&mut self, dt: f32) {
        for l in self.links.iter_mut() {
            l.door.update(dt);
        }
        self.lock.update(dt);
    }

    /// Can something stand here? Inside the hull, but in open volume.
    pub fn walkable(&self, p: Vec3) -> bool {
        let inside_hull = (p.abs() - self.hull_half).max_element() < 0.0;
        inside_hull && self.sdf_local(p) > 0.0
    }

    /// Is there room to drive here — not merely to stand?
    ///
    /// Checks clearance on both sides and overhead rather than just the
    /// point, because a vehicle occupies volume and a gap it fits through
    /// on the centreline is not a gap it fits through.
    pub fn drivable(&self, p: Vec3) -> bool {
        if !self.walkable(p) {
            return false;
        }
        let probes = [
            Vec3::new(DRIVE_HALF_W * 0.9, 0.0, 0.0),
            Vec3::new(-DRIVE_HALF_W * 0.9, 0.0, 0.0),
            Vec3::new(0.0, DRIVE_HALF_H * 0.9, 0.0),
            Vec3::new(0.0, 0.0, DRIVE_HALF_W * 0.9),
            Vec3::new(0.0, 0.0, -DRIVE_HALF_W * 0.9),
        ];
        probes.iter().all(|o| self.walkable(p + *o))
    }

    pub fn cell(&self, id: &str) -> Option<&Cell> {
        self.cells.iter().find(|c| c.id == id)
    }

    /// Where a fixture sits, in hull-local metres.
    ///
    /// Viewports go on the outboard wall so there is something to look
    /// through. The EVA platform goes OUTSIDE the pressure hull, level with
    /// the airlock — that is the point of it: you cycle through and you are
    /// standing on the skin, free to launch as a flyer.
    pub fn fixture_pos(&self, cell: &Cell, f: Fixture) -> Vec3 {
        match f {
            Fixture::HoloTable => cell.centre,
            Fixture::Viewport => cell.centre + Vec3::new(cell.half.x * 0.95, 0.0, 0.0),
            Fixture::UpgradeRig => cell.centre + Vec3::new(0.0, 0.0, -cell.half.z * 0.6),
            Fixture::VendorStall => cell.centre + Vec3::new(-cell.half.x * 0.7, 0.0, 0.0),
            Fixture::Bunk => cell.centre + Vec3::new(0.0, 0.0, cell.half.z * 0.7),
            Fixture::LaunchDoor => cell.centre + Vec3::new(0.0, -cell.half.y, 0.0),
            // just proud of the hull skin, on the same side as the airlock
            Fixture::EvaPlatform => Vec3::new(
                self.hull_half.x + 1.5,
                cell.centre.y,
                cell.centre.z,
            ),
        }
    }

    /// Every fixture in the ship, with its place.
    pub fn all_fixtures(&self) -> Vec<(&Cell, Fixture, Vec3)> {
        let mut v = Vec::new();
        for c in &self.cells {
            for f in c.fixtures {
                v.push((c, *f, self.fixture_pos(c, *f)));
            }
        }
        v
    }

    /// Walk the door graph from a room; used to prove the carve did not
    /// separate anything the plan says is connected.
    pub fn reachable_from(&self, start: &str) -> Vec<&'static str> {
        let Some(s) = self.cells.iter().position(|c| c.id == start) else { return Vec::new() };
        let mut seen = vec![s];
        let mut i = 0;
        while i < seen.len() {
            let cur = seen[i];
            for l in &self.links {
                let other = if l.a == cur {
                    Some(l.b)
                } else if l.b == cur {
                    Some(l.a)
                } else {
                    None
                };
                if let Some(o) = other {
                    if !seen.contains(&o) {
                        seen.push(o);
                    }
                }
            }
            i += 1;
        }
        seen.into_iter().map(|i| self.cells[i].id).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_plan_survives_being_built() {
        let i = Interior::generate(110.0);
        assert_eq!(i.cells.len(), HUB.len());
        // every door in the plan is a carved corridor, counted once
        let doors: usize = HUB.iter().map(|r| r.exits.len()).sum();
        assert_eq!(i.links.len(), doors / 2, "doors and corridors disagree");
        // and nothing was separated by the carve
        assert_eq!(i.reachable_from("bridge").len(), HUB.len());
    }

    #[test]
    fn you_can_stand_in_every_room() {
        for len in [28.0, 110.0, 480.0, 3000.0] {
            let i = Interior::generate(len);
            for c in &i.cells {
                assert!(i.walkable(c.centre), "{} is solid on a {len} m hull", c.id);
            }
        }
    }

    #[test]
    fn you_can_drive_the_spine_of_a_ship_big_enough_to_hold_one() {
        // a hangar you cannot reach with a vehicle is decorative — but only
        // once the hull is wide enough for a corridor at all
        for len in [110.0, 480.0, 3000.0] {
            let i = Interior::generate(len);
            assert!(i.drive_clearance, "{len} m should be drivable");
            for id in ["bridge", "corridor_fore", "corridor_aft", "hangar"] {
                let c = i.cell(id).unwrap();
                assert!(i.drivable(c.centre), "{id} is too tight to drive on a {len} m hull");
            }
        }
    }

    #[test]
    fn a_lander_is_walk_only_and_says_so() {
        // 28 m long is about 5 m in the beam: a vehicle corridor would be
        // wider than the ship. Better to report it than to fake it.
        let i = Interior::generate(28.0);
        assert!(!i.drive_clearance);
        // and it is still a real interior you can walk
        for c in &i.cells {
            assert!(i.walkable(c.centre), "{} is solid even on foot", c.id);
        }
    }

    #[test]
    fn a_carrier_is_somewhere_you_could_get_lost() {
        let small = Interior::generate(28.0);
        let big = Interior::generate(3000.0);
        let sb = small.cell("bridge").unwrap();
        let bb = big.cell("bridge").unwrap();
        assert!(bb.half.z > sb.half.z * 10.0, "the carrier's bridge is a cupboard");
        // and the ship is longer end to end by roughly the length ratio
        let sl = small.cell("bridge").unwrap().centre.z - small.cell("airlock").unwrap().centre.z;
        let bl = big.cell("bridge").unwrap().centre.z - big.cell("airlock").unwrap().centre.z;
        assert!(bl > sl * 50.0, "{sl} -> {bl}");
    }

    #[test]
    fn everything_stays_inside_its_own_hull() {
        for len in [28.0, 480.0, 3000.0] {
            let i = Interior::generate(len);
            for c in &i.cells {
                let far = c.centre.abs() + c.half;
                assert!(far.x <= i.hull_half.x && far.y <= i.hull_half.y && far.z <= i.hull_half.z,
                    "{} pokes out of a {len} m hull: {far:?} vs {:?}", c.id, i.hull_half);
            }
        }
    }

    #[test]
    fn the_rooms_have_what_the_plan_says_they_are_for() {
        let i = Interior::generate(110.0);
        let has = |id: &str, f: Fixture| i.cell(id).unwrap().fixtures.contains(&f);
        assert!(has("bridge", Fixture::HoloTable), "nowhere to plot a course");
        assert!(has("bridge", Fixture::Viewport));
        assert!(has("corridor_fore", Fixture::Viewport), "the plan promises viewports on both sides");
        assert!(has("market", Fixture::VendorStall));
        assert!(has("upgrade_bay", Fixture::UpgradeRig));
        assert!(has("airlock", Fixture::EvaPlatform));
        assert!(has("hangar", Fixture::LaunchDoor));
    }

    #[test]
    fn the_eva_platform_is_outside_the_pressure_hull() {
        // the whole point of a spacewalk: cycle through and you are on the
        // skin, free to launch as a flyer
        let i = Interior::generate(480.0);
        let air = i.cell("airlock").unwrap();
        let p = i.fixture_pos(air, Fixture::EvaPlatform);
        assert!(p.x.abs() > i.hull_half.x, "the EVA platform is indoors");
        assert!(!i.walkable(p), "outside the hull should not read as interior");
        assert!(Fixture::EvaPlatform.exterior());
        // and everything else is inboard
        for (c, f, pos) in i.all_fixtures() {
            if f.exterior() {
                continue;
            }
            assert!(pos.abs().cmple(i.hull_half).all(), "{} {:?} is outside", c.id, f);
        }
    }

    #[test]
    fn a_shut_door_is_a_wall_and_an_open_one_is_a_way_through() {
        let mut i = Interior::generate(480.0);
        // find a corridor midpoint that is not inside either room it joins
        let l = i.links[0];
        let mid = l.centre;
        let shut = i.walkable(mid);
        i.unseal();
        let open = i.walkable(mid);
        assert!(open, "opening every door did not make the corridor passable");
        assert!(!shut || l.half.length() < 1.0,
            "a shut door was already walkable straight through");
    }

    #[test]
    fn losing_power_shuts_the_ship_and_opens_the_bay() {
        let mut i = Interior::generate(480.0);
        i.unseal();
        i.depower();
        for _ in 0..900 {
            i.step_doors(1.0 / 60.0);
        }
        // the launch door is the one that must NOT trap anything
        let bay = i.links.iter().find(|l| l.door.kind == Kind::Launch).unwrap();
        assert!(bay.door.passable(), "the launch bay sealed itself shut");
        // and the airlock is the one that must
        assert_eq!(i.lock.inner.state, crate::airlock::State::Shut);
        assert!(i.lock.interlock_held());
    }

    #[test]
    fn doors_are_gauged_for_what_has_to_pass_them() {
        let big = Interior::generate(480.0);
        assert!(big.drive_clearance);
        let bay = big.links.iter().find(|l| l.door.kind == Kind::Launch).unwrap();
        assert!(bay.door.admits(Gauge::Launch), "a flyer cannot leave the hangar");
        // a walk-only lander fits nothing but crew, and says so consistently
        let small = Interior::generate(28.0);
        assert!(!small.drive_clearance);
        assert!(small.links.iter().all(|l| !l.door.admits(Gauge::Vehicle)),
            "a walk-only ship has a vehicle door");
    }

    #[test]
    fn the_hull_between_rooms_is_still_solid() {
        // a carve that removes everything is not an interior, it is a shell
        let i = Interior::generate(480.0);
        let mut solid = 0;
        let mut open = 0;
        for a in 0..12 {
            for b in 0..12 {
                for c in 0..12 {
                    let p = Vec3::new(
                        (a as f32 / 11.0 - 0.5) * 2.0 * i.hull_half.x,
                        (b as f32 / 11.0 - 0.5) * 2.0 * i.hull_half.y,
                        (c as f32 / 11.0 - 0.5) * 2.0 * i.hull_half.z,
                    ) * 0.95;
                    if i.walkable(p) { open += 1 } else { solid += 1 }
                }
            }
        }
        assert!(solid > open, "the ship is mostly corridor: {open} open, {solid} solid");
        assert!(open > 20, "there is barely any interior: {open}");
    }
}

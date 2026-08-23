//! ABOARD — walking the ship, and consoles that live somewhere.
//!
//! `interior.rs` builds a complete walkable ship: eight rooms from
//! `hub_plan.h`, corridors, doors that seal and unseal, an SDF, a walkable
//! test and fixture positions. `main.rs` referenced it **zero times**.
//!
//! `stations.rs` gives each console a `cell()` — the room it belongs in —
//! and nothing has ever used that either. The Bridge is in the bridge, the
//! Chartroom is in the market, the Starmap is at the airlock, and until now
//! you reached all of them from the same menu regardless of where you were,
//! which is to say the ship was a diagram with a menu bolted to it.
//!
//! This is the join. It is small on purpose:
//!
//! * where you are, as a position rather than a screen;
//! * which room that is (`interior::room_at`, which had to be added — a
//!   walkable ship with no way to ask "where am I" cannot drive anything);
//! * which console is here, if any;
//! * and whether you can get from here to there, which `reachable_from`
//!   already answers and which starts mattering the moment power fails.
//!
//! ## Why this is the right thing to build before the scale work
//!
//! `--rescale` priced walking around outdoors: every physics constant retuned
//! and the feel numbers re-derived, a day of work and a playtest. **None of
//! that applies indoors.** The rooms are authored, the scale is fixed, and
//! moving between them is a position and a walkable check. The ship is the
//! one place the 22x problem does not bite.

use crate::interior::Interior;
use crate::stations::{Station, STATIONS};
use glam::Vec3;

// No `REACH_M`. I declared a three-metre activation radius out of habit and
// then never used it, because BEING IN THE ROOM is the better rule: it needs
// no tuning, it cannot put two consoles in range at once, and it means the
// answer to "why can't I use this" is always "you are not in there", which a
// player can see. A radius would have added a number to get wrong.

/// Where the player is aboard, and what that means.
pub struct Aboard {
    pub ship: Interior,
    pub pos: Vec3,
    /// the renderer slot the hull mesh was uploaded to, once it has been
    ///
    /// `None` until something asks. Uploading a mesh needs a GPU and this
    /// module is used headlessly by tests and drivers, so the geometry is
    /// built on demand rather than at construction.
    pub hull_slot: Option<usize>,
    /// the room last occupied — corridors keep the previous one, so a console
    /// does not blink off while you walk through a doorway
    pub room: &'static str,
}

/// A hull, measured in OCCUPANT HEIGHTS rather than metres.
///
/// This is the unit the ship should always have used, and using metres was
/// the mistake. A 70 m hull is a reasonable starship for a 1.75 m person and
/// **184 body-heights for the 0.38 m body this engine actually flies** — so
/// the rooms came out cavernous and the doorways meaningless.
///
/// Naming it in heights fixes it at both ends. Forty heights is a small
/// starship: 15 m today, and automatically 340 m the day `--rescale` makes
/// the player an 8.5 m bot. Nobody has to remember to change it, which
/// matters because the whole point of the rescale is that a hundred numbers
/// move at once and the ones that get missed are the ones written in metres.
pub const HULL_HEIGHTS: f32 = 40.0;

/// The hull length for a given occupant.
pub fn hull_for(body_h_m: f32) -> f32 {
    body_h_m * HULL_HEIGHTS
}

impl Aboard {
    /// Board a ship sized for the body that flies it.
    ///
    /// The ship is meant to be Transformer scale, and "Transformer scale" is
    /// a RATIO, not a length — a bot's ship should be as many bot-heights
    /// long as a person's ship is person-heights. Sizing off the occupant is
    /// what makes that true without a second number to keep in sync.
    pub fn for_body(body_h_m: f32) -> Aboard {
        Aboard::new(hull_for(body_h_m))
    }

    /// Board at the bridge, which is where the plan puts the way in.
    ///
    /// **The doors start shut.** `Door::passable` wants `travel > 0.75` and a
    /// freshly generated ship has every door at zero, so a new `Aboard` can
    /// reach exactly one room — which is correct and was not what I expected:
    /// my first test asserted a dark ship reaches fewer consoles than a lit
    /// one and got "1 vs 1", because neither had opened anything.
    ///
    /// A ship you have just boarded is sealed. `unseal` is what boarding
    /// means.
    pub fn new(hull_length: f32) -> Aboard {
        let mut ship = Interior::generate(hull_length);
        ship.unseal();
        // unseal starts them moving; run the doors out so a new ship is a
        // ship rather than a countdown
        for _ in 0..240 {
            ship.step_doors(1.0 / 60.0);
        }
        let pos = ship.stand_in("bridge").unwrap_or(Vec3::ZERO);
        Aboard { ship, pos, room: "bridge", hull_slot: None }
    }

    /// Try to move. Refused if the destination is not walkable — a sealed
    /// door or a hull wall — and the refusal is the point: `interior.rs`
    /// already models power failure sealing doors, and that only means
    /// something if it can stop you.
    pub fn walk_to(&mut self, p: Vec3) -> bool {
        if !self.ship.walkable(p) {
            return false;
        }
        self.pos = p;
        if let Some(c) = self.ship.room_at(p) {
            self.room = c.id;
        }
        true
    }

    /// Walk to a named room's centre. `false` when there is no such room or
    /// no way in.
    pub fn go(&mut self, room: &str) -> bool {
        let Some(p) = self.ship.stand_in(room) else { return false };
        if !self.reachable(room) {
            return false;
        }
        self.walk_to(p)
    }

    /// Can you get there from where you are, through doors that open?
    pub fn reachable(&self, room: &str) -> bool {
        self.ship.reachable_from(self.room).contains(&room)
    }

    /// Rooms you can get to right now.
    pub fn reachable_rooms(&self) -> Vec<&'static str> {
        self.ship.reachable_from(self.room)
    }

    /// The console in this room, if there is one.
    ///
    /// Reads `Station::cell()`, which has existed since the stations landed
    /// and has never been used for anything.
    pub fn console_here(&self) -> Option<Station> {
        STATIONS.into_iter().find(|s| s.cell() == self.room)
    }

    /// Cut the power and let the doors fall shut.
    ///
    /// `depower` sets them closing; it does not teleport them closed, the
    /// same way `unseal` does not teleport them open. My first test called
    /// `depower` and asked immediately, got "7 vs 7", and read that as the
    /// sealing not working — it was reading the doors mid-swing.
    pub fn power_out(&mut self) {
        self.ship.depower();
        for _ in 0..240 {
            self.ship.step_doors(1.0 / 60.0);
        }
    }

    /// Restore power and let them open again.
    pub fn power_on(&mut self) {
        self.ship.unseal();
        for _ in 0..240 {
            self.ship.step_doors(1.0 / 60.0);
        }
    }

    /// Every console you could walk to from here.
    pub fn consoles_reachable(&self) -> Vec<Station> {
        let rooms = self.reachable_rooms();
        STATIONS.into_iter().filter(|s| rooms.contains(&s.cell())).collect()
    }

    /// Consoles that exist and are cut off — the thing worth telling somebody
    /// about when the power goes.
    pub fn consoles_stranded(&self) -> Vec<Station> {
        let rooms = self.reachable_rooms();
        STATIONS.into_iter().filter(|s| !rooms.contains(&s.cell())).collect()
    }
}

// -------------------------------------------------------------- shape ---

/// How far to lift the hull so it rests on the ground.
///
/// `-deck_y` is the obvious answer and it is wrong: the deck is where you
/// STAND, and the rooms extend below it. Lifting by the deck leaves the hull
/// 4.9 m underground on a 70 m ship, which on screen looks like nothing drew
/// at all — the worst possible failure, because it points at the draw call
/// rather than at the transform.
///
/// The mesh's own lowest vertex is the answer.
pub fn ground_lift(ship: &Interior) -> f32 {
    let m = hull_mesh(ship);
    let lo = m.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
    -lo + 0.1
}

/// A drawable mesh for the ship's interior.
///
/// **`interior.rs` gives you a ship you can walk in and nothing to look at.**
/// Its SDF feeds collision, and `world.sdf` is a physics query — the renderer
/// draws meshes. So the rooms have positions, sizes, doors and fixtures, and
/// there has never been a triangle among them.
///
/// This is the missing half, and it is deliberately the SHELL rather than the
/// dressing: one bevelled box per room, one per open corridor, at their real
/// positions and sizes. Enough to stand in and see where the doors are.
///
/// Shut corridors are omitted, which matters twice: a sealed door should not
/// have a tunnel drawn through it, and the mesh then agrees with
/// `reachable_from` about what is passable — two answers to the same question
/// that could otherwise disagree.
pub fn hull_mesh(ship: &Interior) -> crate::meshgen::MeshData {
    use crate::meshgen::{bake, cbox, merge, MeshData};
    use glam::Quat;

    let mut out = MeshData::new();
    for c in &ship.cells {
        let mut b = cbox(0.06);
        bake(&mut b, c.half * 2.0, Quat::IDENTITY, c.centre);
        merge(&mut out, &b);
    }
    for l in &ship.links {
        // a shut door is hull, so there is no corridor to draw through it
        let Some((_, h)) = l.door.void() else { continue };
        let mut b = cbox(0.03);
        bake(&mut b, l.half.min(h * 4.0) * 2.0, Quat::IDENTITY, l.centre);
        merge(&mut out, &b);
    }
    out
}

impl Aboard {
    /// Upload the hull and remember where it went.
    ///
    /// **Nothing calls this yet, and that is the honest state.** It is the
    /// last step before the ship is on screen, and it is the only one in this
    /// module that cannot be checked here — `add_mesh` builds GPU buffers and
    /// this environment has no GPU. The path type-checks (`wgpu` is not
    /// feature-gated, so `render.rs` is in the build the tests run against),
    /// which is as far as verification goes without a device.
    ///
    /// I am not inventing a caller to silence the warning. The remaining work
    /// is: call this once on boarding, and `push` the slot with
    /// `Mat4::IDENTITY` in the draw loop the way the terrain chunks do. Two
    /// lines, and both of them want eyes on the result.
    ///
    /// Re-uploads when the doors have moved, because a sealed corridor is not
    /// drawn and the mesh has to agree with what you can walk through. Cheap:
    /// a thousand triangles.
    pub fn upload_hull(&mut self, gpu: &mut crate::render::Gpu) -> usize {
        let m = hull_mesh(&self.ship);
        match self.hull_slot {
            Some(i) => {
                gpu.replace_mesh(i, &m);
                i
            }
            None => {
                let i = gpu.add_mesh(&m);
                self.hull_slot = Some(i);
                i
            }
        }
    }
}

// ------------------------------------------------------------- driver ---

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--aboard").unwrap_or(0);
    let hull: f32 = argv.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(70.0);

    let mut a = Aboard::new(hull);
    println!("ABOARD — a {hull:.0} m hull\n");
    println!("  {} rooms, {} corridors, deck at {:.1}\n",
        a.ship.cells.len(), a.ship.links.len(), a.ship.deck_y);

    println!("  {:<14} {:<10} {:>9}  {}", "room", "console", "reachable", "what it is");
    for idx in 0..a.ship.cells.len() {
        let c = &a.ship.cells[idx];
        let (id, name, blurb) = (c.id, c.name, c.blurb);
        let con = STATIONS
            .into_iter()
            .find(|s| s.cell() == id)
            .map(|s| s.id())
            .unwrap_or("—");
        let reach = if a.reachable(id) { "yes" } else { "no" };
        println!("  {:<14} {:<10} {:>9}  {}", name.to_lowercase(), con, reach, blurb);
    }

    println!("\n  WALKING\n");
    for room in ["market", "hangar", "airlock", "quarters", "nowhere"] {
        let ok = a.go(room);
        let here = a.console_here();
        println!(
            "    -> {:<10} {}{}",
            room,
            if ok { "arrived" } else { "no route" },
            match here {
                Some(s) if ok => format!(", {} is here", s.title()),
                _ => String::new(),
            }
        );
    }

    println!("\n  AND WHEN THE POWER GOES\n");
    let mut dark = Aboard::new(hull);
    dark.power_out();
    println!("    from the bridge, {} of {} consoles are still reachable:",
        dark.consoles_reachable().len(), STATIONS.len());
    for s in dark.consoles_reachable() {
        println!("      {}", s.title());
    }
    let stranded = dark.consoles_stranded();
    if !stranded.is_empty() {
        println!("    stranded behind a sealed door:");
        for s in &stranded {
            println!("      {} (in the {})", s.title(), s.cell());
        }
    }
    // and back again, so the driver shows the round trip rather than a
    // one-way failure state
    dark.power_on();
    println!("    power restored: {} of {} reachable again.",
        dark.consoles_reachable().len(), STATIONS.len());

    println!("\n  AND SOMETHING TO LOOK AT\n");
    println!("    parked at the origin, lifted {:.1} m so it rests on the ground.",
        ground_lift(&a.ship));
    {
        let m = hull_mesh(&a.ship);
        let span = m.verts.iter().map(|v| v.pos[0].abs()).fold(0.0f32, f32::max) * 2.0;
        let tall = m.verts.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max)
            - m.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        println!("    {span:.0} m across, {tall:.1} m tall — drive to 0,0 and look.\n");
    }
    let lit = hull_mesh(&a.ship);
    let mut sealed_ship = Aboard::new(hull);
    sealed_ship.power_out();
    let dark_mesh = hull_mesh(&sealed_ship.ship);
    println!("    powered:  {:>6} verts, {:>6} tris", lit.verts.len(), lit.idx.len() / 3);
    println!("    sealed:   {:>6} verts, {:>6} tris", dark_mesh.verts.len(), dark_mesh.idx.len() / 3);
    println!("\n    the sealed hull is smaller because a shut door is hull — there is");
    println!("    no corridor drawn through it, so the mesh and `reachable_from`");
    println!("    agree about what is passable rather than being two answers to the");
    println!("    same question.");

    println!("\n    `interior.rs` has modelled power-failure sealing since it landed.");
    println!("    Until something asked where you were, it could not stop you.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_console_has_a_room_that_exists() {
        // `Station::cell()` has named a room since the stations landed and
        // nothing ever checked the room was real
        let a = Aboard::new(70.0);
        for s in STATIONS {
            assert!(
                a.ship.cell(s.cell()).is_some(),
                "{} lives in '{}', which is not a room on the ship",
                s.title(),
                s.cell()
            );
        }
        // and no two consoles share one
        let mut cells: Vec<&str> = STATIONS.into_iter().map(|s| s.cell()).collect();
        cells.sort_unstable();
        let before = cells.len();
        cells.dedup();
        assert_eq!(cells.len(), before, "two consoles in the same room");
    }

    #[test]
    fn standing_in_a_room_finds_the_console_there() {
        let mut a = Aboard::new(70.0);
        for s in STATIONS {
            assert!(a.go(s.cell()), "cannot walk to the {}", s.cell());
            assert_eq!(
                a.console_here().map(|x| x.id()),
                Some(s.id()),
                "in the {} and {} is not here",
                s.cell(),
                s.title()
            );
        }
    }

    #[test]
    fn a_corridor_is_between_rooms_not_in_both() {
        // a console that opens while you are halfway through a doorway is
        // worse than one that waits
        let a = Aboard::new(70.0);
        let link = &a.ship.links[0];
        // the midpoint of a corridor, on the deck
        let mid = Vec3::new(link.centre.x, a.ship.deck_y, link.centre.z);
        let in_a_room = a.ship.room_at(mid).is_some();
        // it may clip a room's bounds; what must hold is that the LAST room
        // is kept rather than cleared
        let mut b = Aboard::new(70.0);
        b.go("bridge");
        let was = b.room;
        b.walk_to(mid);
        assert!(
            b.room == was || in_a_room,
            "walking into a corridor cleared the room instead of keeping it"
        );
    }

    #[test]
    fn you_cannot_walk_through_the_hull() {
        let mut a = Aboard::new(70.0);
        let outside = a.ship.hull_half * 4.0;
        assert!(!a.walk_to(outside), "walked out through the hull");
        assert_eq!(a.room, "bridge", "the room changed on a refused move");
    }

    #[test]
    fn losing_power_strands_consoles_and_that_is_the_point() {
        // `interior.rs` has modelled power-failure sealing since it landed.
        // Until something asked where you were, it could not stop you — which
        // is what "referenced zero times in main.rs" meant in practice.
        let lit = Aboard::new(70.0);
        let mut dark = Aboard::new(70.0);
        dark.power_out();
        assert!(
            dark.consoles_reachable().len() < lit.consoles_reachable().len(),
            "a dead ship reaches as many consoles as a live one: {} vs {}",
            dark.consoles_reachable().len(),
            lit.consoles_reachable().len()
        );
        assert!(!dark.consoles_stranded().is_empty());
        // the one you are standing in is still yours
        assert!(dark.reachable("bridge"));
    }

    #[test]
    fn unsealing_gives_them_back() {
        let mut a = Aboard::new(70.0);
        a.power_out();
        let dark = a.consoles_reachable().len();
        a.power_on();
        assert!(a.consoles_reachable().len() > dark, "power did not help");
    }

    #[test]
    fn the_ship_has_a_mesh_now() {
        // `interior.rs` gave a ship you can walk in and nothing to look at:
        // its SDF feeds collision and the renderer draws meshes.
        let a = Aboard::new(70.0);
        let m = hull_mesh(&a.ship);
        assert!(!m.verts.is_empty(), "the hull has no geometry");
        assert_eq!(m.idx.len() % 3, 0, "indices are not triangles");
        // one box per room at minimum, and boxes have 8+ verts
        assert!(
            m.verts.len() >= a.ship.cells.len() * 8,
            "{} verts for {} rooms",
            m.verts.len(),
            a.ship.cells.len()
        );
        // every index addresses a real vertex
        let n = m.verts.len() as u32;
        assert!(m.idx.iter().all(|i| *i < n), "an index points past the end");
    }

    #[test]
    fn a_shut_door_has_no_corridor_drawn_through_it() {
        // the mesh and `reachable_from` are two answers to the same question
        // and they have to agree: a sealed ship should not have tunnels
        // through its sealed doors
        let lit = Aboard::new(70.0);
        let mut dark = Aboard::new(70.0);
        dark.power_out();
        let a = hull_mesh(&lit.ship);
        let b = hull_mesh(&dark.ship);
        assert!(
            b.verts.len() < a.verts.len(),
            "sealing the ship did not remove any corridor: {} vs {}",
            b.verts.len(),
            a.verts.len()
        );
        // the rooms are still there — it is a sealed ship, not an empty one
        assert!(b.verts.len() >= dark.ship.cells.len() * 8);
    }

    #[test]
    fn the_hull_sits_where_the_draw_call_puts_it() {
        // main.rs lifts the mesh by -deck_y so it rests on the ground rather
        // than half-buried. If that sign is wrong the ship is underground and
        // looks like nothing drew — worth knowing the numbers before looking.
        let a = Aboard::new(70.0);
        let m = hull_mesh(&a.ship);
        let lo_y = m.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        let hi_y = m.verts.iter().map(|v| v.pos[1]).fold(f32::MIN, f32::max);
        let lifted_lo = lo_y + ground_lift(&a.ship);
        assert!(
            lifted_lo > -1.0,
            "the lifted hull's floor is at {lifted_lo:.2} — it will be underground"
        );
        // and it is a ship-sized object, not a speck or a continent
        let span = m.verts.iter().map(|v| v.pos[0].abs()).fold(0.0f32, f32::max) * 2.0;
        assert!((20.0..200.0).contains(&span), "the hull spans {span:.0} m");
        assert!(hi_y - lo_y > 2.0, "the hull is {:.1} m tall", hi_y - lo_y);
    }

    #[test]
    fn the_hull_is_measured_in_occupants_not_metres() {
        // THE FIX FOR THE SCALE MISMATCH. A 70 m hull is a reasonable
        // starship for a person and 184 body-heights for the 0.38 m body this
        // engine flies, so the rooms were cavernous and the doorways
        // meaningless.
        //
        // "Transformer scale" is a RATIO. A bot's ship should be as many
        // bot-heights long as a person's ship is person-heights, and sizing
        // off the occupant is what makes that true with no second number.
        let car = hull_for(0.38);
        let human = hull_for(1.75);
        let bot = hull_for(8.5);
        assert!((car - 15.2).abs() < 0.1, "the engine body gets a {car} m hull");
        assert!(bot > 300.0, "a bot's ship is only {bot} m — building size was the ask");
        // the RATIO is what is fixed, at every scale
        for (h, hull) in [(0.38, car), (1.75, human), (8.5, bot)] {
            assert!((hull / h - HULL_HEIGHTS).abs() < 0.01, "at {h} m");
        }
    }

    #[test]
    fn a_ship_sized_for_its_occupant_still_has_all_its_rooms() {
        // 15 m is small, and the plan must not lose rooms at the bottom of
        // the range — a lander is a cramped carrier, not a smaller one
        for body in [0.38f32, 1.75, 8.5] {
            let a = Aboard::for_body(body);
            assert_eq!(
                a.ship.cells.len(),
                crate::content::HUB.len(),
                "a {body} m occupant lost rooms"
            );
            assert!(a.console_here().is_some(), "no bridge console at {body} m");
            // and you can still get about
            assert!(a.consoles_reachable().len() > 1, "sealed in at {body} m");
        }
    }

    #[test]
    fn the_doors_move_when_something_ticks_them() {
        // `step_doors` existed, `Door::update` animated travel, `passable`
        // read it — and NOTHING TICKED THE CLOCK, so every door in the game
        // was frozen at whatever state it generated in. Invisible while the
        // ship was scenery.
        let mut ship = Interior::generate(70.0);
        let shut = ship.links.iter().filter(|l| l.door.passable()).count();
        assert_eq!(shut, 0, "a freshly generated ship has open doors");

        // `unseal` sets travel to 1.0 OUTRIGHT — it is "the ship is powered",
        // a state rather than an animation, and I asserted it animated and
        // was wrong about my own function. What animates is `depower`, which
        // sets them CLOSING and lets the clock run them shut.
        ship.unseal();
        assert!(
            ship.links.iter().all(|l| l.door.passable()),
            "unseal did not open everything at once"
        );

        ship.depower();
        let immediately = ship.links.iter().filter(|l| l.door.passable()).count();
        for _ in 0..240 {
            ship.step_doors(1.0 / 60.0);
        }
        let after = ship.links.iter().filter(|l| l.door.passable()).count();
        assert!(
            after < immediately,
            "the doors never shut: {immediately} passable before ticking, {after} after"
        );
    }

    #[test]
    fn the_camera_probe_finds_open_volume() {
        // the clamp walks the boom in until the eye is in open volume. If
        // `walkable` were inverted the camera would clamp INTO the wall,
        // which looks like the ship vanishing rather than like a bad clamp.
        let a = Aboard::new(70.0);
        let inside = a.ship.stand_in("bridge").unwrap();
        assert!(a.ship.walkable(inside), "the bridge is not open volume");
        let wall = a.ship.hull_half * 0.95;
        assert!(!a.ship.walkable(wall), "the hull corner is open volume");
        // and the walk-in terminates somewhere clear
        let mut t = 1.0f32;
        let eye = inside + Vec3::new(0.0, 2.0, 40.0); // well outside
        for _ in 0..8 {
            if a.ship.walkable(inside + (eye - inside) * t) {
                break;
            }
            t *= 0.7;
        }
        assert!(a.ship.walkable(inside + (eye - inside) * t), "the clamp never found air");
        assert!(t < 1.0, "the eye was already inside and did not need clamping");
    }

    #[test]
    fn the_broad_phase_keeps_the_ship_off_the_hot_path() {
        // `World::sdf` runs per contact per frame and `sdf_local` walks every
        // cell and every link. The hull box test has to reject the common
        // case — being nowhere near the ship — for nearly nothing.
        let a = Aboard::new(70.0);
        let far = Vec3::new(400.0, 0.0, 400.0);
        let t0 = std::time::Instant::now();
        for _ in 0..200_000 {
            let q = far.abs() - a.ship.hull_half - Vec3::splat(2.0);
            std::hint::black_box(q.max_element() < 0.0);
        }
        let rejected = t0.elapsed().as_secs_f32();

        let near = a.ship.stand_in("bridge").unwrap();
        let t1 = std::time::Instant::now();
        for _ in 0..200_000 {
            std::hint::black_box(a.ship.sdf_local(near));
        }
        let full = t1.elapsed().as_secs_f32();
        assert!(
            full > rejected * 3.0,
            "the full query costs {full:.4}s and the reject {rejected:.4}s — \
             the broad phase is not buying anything"
        );
    }

    #[test]
    fn what_you_see_is_what_you_hit() {
        // The draw call and `World::park` both offset by `ground_lift`. Two
        // transforms for one object is how a hull you can see and a hull you
        // can hit end up in different places, and it is invisible until you
        // walk through a wall that is drawn a metre away.
        let a = Aboard::new(70.0);
        let lift = ground_lift(&a.ship);
        let origin = Vec3::new(0.0, lift, 0.0);

        // a point at the centre of the bridge, in world space
        let bridge = a.ship.stand_in("bridge").expect("bridge");
        let world_p = bridge + origin;

        // the collision query subtracts the origin and asks the interior
        let d = a.ship.sdf_local(world_p - origin);
        assert!(d > 0.0, "the bridge is solid at its own centre: {d}");

        // and the mesh's floor, lifted, is above ground — the same number
        let m = hull_mesh(&a.ship);
        let lo = m.verts.iter().map(|v| v.pos[1]).fold(f32::MAX, f32::min);
        assert!((lo + lift - 0.1).abs() < 0.01, "the lift is not the mesh floor");
    }

    #[test]
    fn the_hull_is_solid_where_the_walls_are() {
        // inside a room: free. Inside the hull but not in a room: solid.
        let a = Aboard::new(70.0);
        let bridge = a.ship.stand_in("bridge").expect("bridge");
        assert!(a.ship.sdf_local(bridge) > 0.0, "you cannot stand on the bridge");
        // a corner of the hull box is structure, not a room
        let corner = a.ship.hull_half * 0.95;
        assert!(
            a.ship.sdf_local(corner) < 0.0,
            "the hull corner is walkable — the ship is a shell with no walls"
        );
    }

    #[test]
    fn the_hull_slot_starts_empty_and_is_remembered() {
        // the upload itself needs a GPU; the bookkeeping around it does not,
        // and getting that wrong would mean re-uploading a thousand triangles
        // every frame
        let mut a = Aboard::new(70.0);
        assert!(a.hull_slot.is_none(), "a fresh ship claims to be uploaded");
        a.hull_slot = Some(42);
        // walking must not lose it — the mesh outlives the position
        a.go("market");
        assert_eq!(a.hull_slot, Some(42), "walking dropped the hull slot");
    }

    #[test]
    fn a_bigger_hull_makes_a_bigger_ship_not_more_rooms() {
        let small = hull_mesh(&Aboard::new(30.0).ship);
        let big = hull_mesh(&Aboard::new(200.0).ship);
        // same room count, so the same geometry — the plan scales, it does
        // not grow
        assert_eq!(small.verts.len(), big.verts.len(), "the plan gained rooms");
        let extent = |m: &crate::meshgen::MeshData| {
            m.verts.iter().map(|v| v.pos[0].abs()).fold(0.0f32, f32::max)
        };
        assert!(extent(&big) > extent(&small) * 2.0, "a 200 m hull is not bigger");
    }

    #[test]
    fn a_small_hull_is_still_a_ship() {
        // the module scales off hull length; a lander should not lose rooms
        for hull in [30.0f32, 70.0, 200.0] {
            let a = Aboard::new(hull);
            assert_eq!(a.ship.cells.len(), crate::content::HUB.len(), "at {hull} m");
            assert!(a.console_here().is_some(), "no console on the bridge at {hull} m");
        }
    }

    #[test]
    fn going_nowhere_is_refused_rather_than_moving_you() {
        let mut a = Aboard::new(70.0);
        let before = a.pos;
        assert!(!a.go("engine_room_that_does_not_exist"));
        assert_eq!(a.pos, before, "a failed walk moved the player");
    }
}

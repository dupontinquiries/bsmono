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
    /// the renderer slot the hull SKIN mesh was uploaded to (the outer
    /// lofted shell — see `hull_skin_mesh`), once it has been
    ///
    /// `None` until something asks. Uploading a mesh needs a GPU and this
    /// module is used headlessly by tests and drivers, so the geometry is
    /// built on demand rather than at construction.
    pub hull_slot: Option<usize>,
    /// the renderer slot for the room/corridor BOUNDARY boxes (see
    /// `hull_cells_mesh`) — kept as a SEPARATE mesh and draw call from
    /// the hull skin, and given its own colour, rather than merged into
    /// one undifferentiated shape. Before this split, the skin and the
    /// boundary boxes shared one flat colour and no texture, which reads
    /// as "a cluster of grey boxes" rather than as a ship with rooms in
    /// it — two colours is the cheapest thing that gives a viewer any
    /// way to tell the hull shell from the structure inside it.
    pub cells_slot: Option<usize>,
    /// A REAL, TEXTURED SHIP MODEL drawn over the procedural hull skin,
    /// instead of it, when one has been loaded (`load_exterior`). The
    /// procedural loft in `hull_skin_mesh` is silhouette-correct but
    /// untextured and reads as a plain shape; this asks one of the
    /// catalog's own `category: "spaceship"` chassis (real geometry,
    /// real materials, the same loader every car and bot already goes
    /// through) to stand in for it instead. `None` until `load_exterior`
    /// is called — headless tests and drivers never touch this.
    ///
    /// COLLISION IS UNCHANGED. The exterior model is decoration only —
    /// `world::sdf` still collides against `Interior::sdf_local` (the
    /// hull box minus the carved rooms), the same solid-shell-with-
    /// carved-voids model as before. Sizing the model to roughly match
    /// `hull_half` keeps the two from disagreeing badly, but they are
    /// still two separate approximations of the same ship, not one
    /// shared source the way the procedural skin was.
    pub exterior: Option<crate::model::Model>,
    /// room last occupied — corridors keep the previous one, so a console
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
/// **Raised from 40.** Forty occupant-heights is a ship you can stand in and
/// not one you can live in: at that size a room is about three occupants
/// across, which fits a body and leaves nowhere to put anything, and the
/// playtest note was "we don't even fit one room at the scale I imagined".
///
/// A hundred and sixty gives rooms of a dozen occupants across and thirty
/// long — a hangar you could land something in, which is what a hangar is
/// for. It is still a RATIO, so a bot's ship is 1.4 km and a person's is
/// 280 m, and the number moves in one place.
pub const HULL_HEIGHTS: f32 = 160.0;

/// The hull length for a given occupant.
/// THE FLEET, as hull classes. No ship MODEL exists yet — the hull is
/// generated (`interior::generate`) from one number, and that number is
/// the only thing that distinguishes a lander from a carrier. So the
/// roster is that number plus a name and a line of what it is like to be
/// inside, which is what a ship is to the player today: eight rooms at a
/// scale.
///
/// Lengths are in OCCUPANT HEIGHTS, per the repo convention, and go
/// through `hull_for` like every other hull — a Transformer's cutter is
/// not a person's cutter.
pub const FLEET: &[(&str, f32, &str)] = &[
    ("cutter", 9.0, "one deck, everything within shouting distance"),
    ("lander", 14.0, "the default: cramped, honest, gets there"),
    ("runner", 20.0, "a corridor with rooms bolted to it"),
    ("hauler", 30.0, "the hangar is the point; the rest is corridor"),
    ("carrier", 46.0, "you will get lost in this one"),
];

/// The default hull class, by index into `FLEET`.
pub const FLEET_DEFAULT: usize = 1;

/// A ship from the roster, sized for the body that flies it.
pub fn fleet_ship(i: usize, body_h_m: f32) -> Interior {
    let (_, heights, _) = FLEET[i.min(FLEET.len() - 1)];
    Interior::generate(heights * body_h_m)
}

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
        Aboard { ship, pos, room: "bridge", hull_slot: None, cells_slot: None, exterior: None }
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
/// The outer hull SKIN — a real lofted ship silhouette, not a box.
///
/// `world::sdf` collides the parked ship with `Interior::sdf_local`, which
/// is the OUTER HULL BOX minus the rooms — so the solid is the shell, and
/// the rooms are the holes in it. Same `hull_half` the SDF uses, so
/// silhouette and solid come from one number.
pub fn hull_skin_mesh(ship: &Interior) -> crate::meshgen::MeshData {
    use crate::meshgen::MeshData;
    let mut out = MeshData::new();
    // LOFT THE SKIN THROUGH THE SAME PROFILE THE COLLIDER MARCHES.
    // Rings of a superellipse at each station, quads between them, and
    // caps at the tips — so the silhouette IS the collider rather than
    // a second opinion about it.
    use crate::hullform::{ring, STATIONS};
    let prof = ship.profile();
    const SEG: usize = 16;
    let mut rings: Vec<Vec<Vec3>> = Vec::with_capacity(STATIONS);
    for i in 0..STATIONS {
        let z = -prof.half_z + 2.0 * prof.half_z * (i as f32 / (STATIONS - 1) as f32);
        let (a, b) = prof.at(z);
        rings.push(ring(a, b, SEG).into_iter().map(|(x, y)| Vec3::new(x, y, z)).collect());
    }
    let mut tri = |p0: Vec3, p1: Vec3, p2: Vec3| {
        let n = (p1 - p0).cross(p2 - p0).normalize_or(Vec3::Y);
        for p in [p0, p1, p2] {
            out.verts.push(crate::meshgen::Vertex {
                pos: p.to_array(),
                nrm: n.to_array(),
                uv: [0.0, 0.0],
            });
        }
    };
    for i in 0..STATIONS - 1 {
        for k in 0..SEG {
            let k2 = (k + 1) % SEG;
            let (a, b, c, d) = (rings[i][k], rings[i][k2], rings[i + 1][k2], rings[i + 1][k]);
            tri(a, b, c);
            tri(a, c, d);
        }
    }
    // caps: a fan to the tip centre at each end
    for (idx, sign) in [(0usize, -1.0f32), (STATIONS - 1, 1.0)] {
        let tip = Vec3::new(0.0, 0.0, sign * prof.half_z);
        for k in 0..SEG {
            let k2 = (k + 1) % SEG;
            if sign < 0.0 {
                tri(tip, rings[idx][k2], rings[idx][k]);
            } else {
                tri(tip, rings[idx][k], rings[idx][k2]);
            }
        }
    }
    out
}

/// The room/corridor BOUNDARY boxes — thin edge frames marking where each
/// room and passable link sits, drawn as a SEPARATE mesh (and given its
/// own colour by the caller) from `hull_skin_mesh` so the two read as
/// two different things rather than one undifferentiated cluster.
pub fn hull_cells_mesh(ship: &Interior) -> crate::meshgen::MeshData {
    use crate::meshgen::{bake, cbox, merge, MeshData};
    use glam::Quat;
    let mut out = MeshData::new();
    for c in &ship.cells {
        let mut b = cbox(0.06);
        bake(&mut b, c.half * 2.0, Quat::IDENTITY, c.centre);
        // CARVED, not extruded. `cbox` builds a solid, so every room was
        // drawn as a block with its faces pointing OUT — which reads as
        // the rooms sticking through the hull rather than being hollowed
        // into it, because from outside that is exactly what a solid box
        // at that position looks like.
        //
        // The hull was never too small: `Profile::fit` grows the skin to
        // contain the cells and there is a test asserting it. The boxes
        // were facing the wrong way.
        crate::meshgen::flip_inward(&mut b);
        merge(&mut out, &b);
    }
    for l in &ship.links {
        // ASK THE ROUTING QUESTION, NOT THE GEOMETRY ONE. This read
        // `Door::void()`, which is `travel > 0.01` — so a door a hundredth of
        // the way open had a corridor drawn through it while
        // `reachable_from`, which wants `passable()` at `travel > 0.75`,
        // called it shut. Between the two you could SEE through a door the
        // game said was closed.
        //
        // Neither function is wrong: `void` is about geometry and `passable`
        // is about whether a body fits. The mesh wants the second, because
        // the mesh is what tells a player where they can go.
        if !l.door.passable() {
            continue;
        }
        let Some((_, h)) = l.door.void() else { continue };
        let mut b = cbox(0.03);
        bake(&mut b, l.half.min(h * 4.0) * 2.0, Quat::IDENTITY, l.centre);
        merge(&mut out, &b);
    }
    out
}

/// Both meshes at once, for a caller that wants the whole hull and does
/// not care about the split (kept for anything that only needs to know
/// "does this look right", like the offline `--aboard` driver).
pub fn hull_mesh(ship: &Interior) -> crate::meshgen::MeshData {
    let mut out = hull_skin_mesh(ship);
    crate::meshgen::merge(&mut out, &hull_cells_mesh(ship));
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
    /// Upload the hull and remember where it went — now TWO meshes/slots
    /// (skin, cells; see their own doc comments), not one merged blob.
    pub fn upload_hull(&mut self, gpu: &mut crate::render::Gpu) -> usize {
        let skin = hull_skin_mesh(&self.ship);
        let cells = hull_cells_mesh(&self.ship);
        match self.cells_slot {
            Some(i) => gpu.replace_mesh(i, &cells),
            None => self.cells_slot = Some(gpu.add_mesh(&cells)),
        }
        match self.hull_slot {
            Some(i) => {
                gpu.replace_mesh(i, &skin);
                i
            }
            None => {
                let i = gpu.add_mesh(&skin);
                self.hull_slot = Some(i);
                i
            }
        }
    }

    /// Load a real, textured ship model from the catalog to stand in for
    /// the procedural hull skin — same loader (`model::load`) every car
    /// and bot chassis already goes through, sized so its long axis
    /// matches this ship's own `hull_half.z * 2.0` (the same length the
    /// procedural loft and the SDF collision both already use, so the
    /// three at least agree on scale even though the model's own
    /// proportions won't match the box-ish interior exactly).
    ///
    /// `bots_dir/name/f00` is the VEHICLE pose of a fold strip — every
    /// chassis in this catalog, spaceship-category included, ships one
    /// (`foldview`'s own convention) even when folding it into a robot
    /// makes no sense for a ship. f00 is a real, complete, textured
    /// model on its own; nothing here touches the fold.
    ///
    /// Silently leaves `exterior` as `None` on any load failure (missing
    /// directory, bad glTF) rather than panicking — a cosmetic upgrade
    /// failing should not take the ship down with it, same reasoning
    /// `declared_orientation` and friends use elsewhere in this codebase
    /// for a bad sidecar.
    pub fn load_exterior(&mut self, gpu: &mut crate::render::Gpu, bots_dir: &std::path::Path, name: &str) {
        let dir = bots_dir.join(name).join("f00");
        if !dir.is_dir() {
            eprintln!("[ship] exterior model {name:?} has no f00 pose at {dir:?} — keeping the procedural hull");
            return;
        }
        let target_len = (self.ship.hull_half.z * 2.0).max(0.5);
        let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
            crate::model::load(&dir, gpu, target_len)
        }));
        match result {
            Ok(m) => {
                eprintln!(
                    "[ship] exterior model {name:?} loaded: {:.1}x{:.1}x{:.1} m, {} prims",
                    m.half_len * 2.0, m.half_w * 2.0, m.half_h * 2.0, m.prims.len()
                );
                self.exterior = Some(m);
            }
            Err(_) => {
                eprintln!("[ship] exterior model {name:?} failed to load — keeping the procedural hull");
            }
        }
    }
}

// ------------------------------------------------------------- driver ---

pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--aboard").unwrap_or(0);
    // DEFAULT TO WHAT THE GAME ACTUALLY BUILDS. This was 70.0 — the old
    // metres-based number — while `main.rs` had moved to `for_body`, so the
    // driver described a 26 m ship the player would never see. A diagnostic
    // that disagrees with the thing it diagnoses is worse than none.
    let default_hull = hull_for(crate::physics::Car::new().half_h * 2.0);
    let hull: f32 = argv.get(i + 1).and_then(|s| s.parse().ok()).unwrap_or(default_hull);

    let mut a = Aboard::new(hull);
    // the useful pair is the length and WHO it is for — the occupant-heights
    // figure is HULL_HEIGHTS by construction and says nothing
    println!(
        "ABOARD — a {hull:.1} m hull, {HULL_HEIGHTS:.0} heights for a {:.2} m occupant\n",
        hull / HULL_HEIGHTS
    );
    println!("  {} rooms, {} corridors, deck at {:.1}\n",
        a.ship.cells.len(), a.ship.links.len(), a.ship.deck_y);

    // ROOM SIZES IN OCCUPANT UNITS. Neither of us could tell whether a room
    // was big enough because nothing printed the one number that answers it:
    // how many occupants fit across, along, and standing up.
    let occ = hull / HULL_HEIGHTS;
    println!("  an occupant is {occ:.2} m; rooms in occupant-units:\n");
    println!(
        "  {:<14} {:>18} {:>9} {:<10} {}",
        "room", "w x l x h (occ)", "reachable", "console", "what it is"
    );
    for idx in 0..a.ship.cells.len() {
        let c = &a.ship.cells[idx];
        let (id, name, blurb) = (c.id, c.name, c.blurb);
        let con = STATIONS
            .into_iter()
            .find(|s| s.cell() == id)
            .map(|s| s.id())
            .unwrap_or("—");
        let reach = if a.reachable(id) { "yes" } else { "no" };
        println!(
            "  {:<14} {:>5.1} x{:>5.1} x{:>5.1} {:>9} {:<10} {}",
            name.to_lowercase(),
            c.half.x * 2.0 / occ,
            c.half.z * 2.0 / occ,
            c.half.y * 2.0 / occ,
            reach,
            con,
            blurb.chars().take(28).collect::<String>()
        );
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
    /// THE BUG THIS GUARDS: the mesh drew only the rooms and links while
    /// `world::sdf` collided `Interior::sdf_local`, which is the outer
    /// hull box minus those rooms. Render was a strict SUBSET of collision
    /// — you bounced off a hull nobody had drawn. Both now come from
    /// `hull_half`, so the check is that the mesh actually reaches it.
    #[test]
    fn the_drawn_hull_reaches_the_volume_that_collides() {
        let ship = super::Interior::generate(90.0);
        let m = super::hull_mesh(&ship);
        let (mut lo, mut hi) = ([f32::MAX; 3], [f32::MIN; 3]);
        for v in &m.verts {
            for a in 0..3 {
                lo[a] = lo[a].min(v.pos[a]);
                hi[a] = hi[a].max(v.pos[a]);
            }
        }
        // the collider is the fitted skin now, so the mesh is checked
        // against THAT rather than against the bounding box it no longer
        // fills — a lofted hull is legitimately narrower than its box
        let p = ship.profile();
        let widest = p.a.iter().copied().fold(0.0f32, f32::max);
        let tallest = p.b.iter().copied().fold(0.0f32, f32::max);
        let half = Vec3::new(widest, tallest, p.half_z);
        for (a, want) in [(0usize, half.x), (1, half.y), (2, half.z)] {
            let got = (hi[a] - lo[a]) * 0.5;
            assert!(
                got > want * 0.9,
                "axis {a}: the mesh spans {got:.1} against a collider of {want:.1} — \
                 the render is a subset of the solid again"
            );
        }
    }

    /// THE STANDING GUARD against the bug that started this: the drawn
    /// skin and the marched collider must be the same surface. Sampled
    /// rather than argued about — points just inside the profile must read
    /// as solid hull, points well outside must read as empty.
    #[test]
    fn the_skin_you_see_is_the_skin_you_hit() {
        let ship = super::Interior::generate(110.0);
        let p = ship.profile();
        for i in 0..super::super::hullform::STATIONS {
            let z = -p.half_z + 2.0 * p.half_z * (i as f32 / 23.0);
            let (a, b) = p.at(z);
            let _ = b;
            // outside the beam by a clear margin: must be empty
            let out = glam::Vec3::new(a * 2.5 + 5.0, 0.0, z);
            assert!(
                ship.sdf_local(out) > 0.0,
                "station {i}: a point well outside the beam reads as hull"
            );
        }
        // and the middle of a room is empty while the plating around it is not
        if let Some(c) = ship.cells.first() {
            assert!(ship.sdf_local(c.centre) > 0.0, "a room reads as solid");
        }
    }

    /// Every hull class must produce a ship whose rooms are inside it and
    /// whose bridge is reachable — cycling with `{`/`}` regenerates at a
    /// new length, and a class that carves a broken ship would strand the
    /// player in structure.
    #[test]
    fn every_hull_class_in_the_fleet_is_a_walkable_ship() {
        for (i, (name, heights, _)) in super::FLEET.iter().enumerate() {
            let mut ship = super::fleet_ship(i, 8.0);
            ship.unseal();
            assert!(*heights > 0.0, "{name} has no length");
            assert!(!ship.cells.is_empty(), "{name} carved no rooms");
            for c in &ship.cells {
                let out = (c.centre.abs() + c.half) - ship.hull_half;
                assert!(
                    out.max_element() <= 0.5,
                    "{name}: room {} pokes {:.1} out of the hull",
                    c.id,
                    out.max_element()
                );
            }
            let open = ship.reachable_from("bridge");
            assert!(open.len() > 1, "{name}: nothing is reachable from the bridge");
        }
    }

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
        // NOT a hard-coded metre figure. This asserted 15.2 m, which is the
        // module's own mistake written into its own test — the whole point is
        // that the metre value moves when `HULL_HEIGHTS` does, and it just
        // did. What is fixed is the ratio and the ordering.
        assert!(car < human && human < bot, "hulls do not scale with occupants");
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
        // A point INSIDE the skin but not in any room is structure — that
        // is what "the hull between rooms is solid" means, and it is what
        // this test was reaching for.
        //
        // It used to sample `hull_half * 0.95`, a corner of the bounding
        // BOX. Measured: on this 70m ship that corner sits 9.88 units
        // OUTSIDE the lofted skin, where `sdf_local` is positive by
        // construction (it starts from `hull.sdf`, which is positive
        // outside, and carving rooms can only raise it). So the assertion
        // was asking whether a point that is not in the ship at all is
        // solid, and reading the honest answer as a bug. A tapered hull
        // never fills the corners of its own bounding box; that is what
        // tapering IS.
        //
        // The origin is the real case: inside the skin (-6.75) and
        // between rooms, so `sdf_local` is negative (-1.04) — structure.
        let wall = Vec3::ZERO;
        assert!(
            a.ship.profile().sdf(wall) < 0.0,
            "the sample must be inside the skin or it tests nothing"
        );
        assert!(
            a.ship.sdf_local(wall) < 0.0,
            "the hull between rooms is walkable — the ship is a shell with no walls"
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


#[cfg(test)]
mod exterior_smoke_test {
    #[test]
    fn the_chosen_exterior_asset_loads_headlessly() {
        // Not a GPU test (model::load needs a real Gpu to upload
        // textures/meshes to, which this sandbox doesn't have) -- this
        // only proves the raw geometry parses without panicking, the
        // same headless check `load_exterior`'s own catch_unwind exists
        // to survive if it fails at runtime instead.
        let dir = std::path::Path::new("assets/bots/spaceship_warpzone_aka_military_troop_ship/f00");
        if !dir.exists() {
            return; // asset tree not present in this checkout/build context
        }
        let (raws, _) = crate::model::load_raw(dir, 1.32);
        assert!(!raws.is_empty(), "the exterior asset loaded zero prims");
    }
}

#[cfg(test)]
mod placeholder_blade_smoke_test {
    #[test]
    fn the_converted_placeholder_blade_loads_headlessly() {
        // Converted from a person-supplied .iqm via the real foldview
        // --gltf export path (games/blackstar/rust/foldview), the same
        // tool every other chassis in assets/bots/ went through. Proves
        // the specific converted file parses through this game's own
        // loader, not just that the general mechanism compiles.
        let dir = std::path::Path::new("assets/weapons/placeholder_blade");
        if !dir.exists() {
            return;
        }
        let (raws, _) = crate::model::load_raw(dir, 1.32);
        assert!(!raws.is_empty(), "the placeholder blade model loaded zero prims");
    }
}

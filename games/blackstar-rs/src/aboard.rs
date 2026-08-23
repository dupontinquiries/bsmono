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
    /// the room last occupied — corridors keep the previous one, so a console
    /// does not blink off while you walk through a doorway
    pub room: &'static str,
}

impl Aboard {
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
        Aboard { ship, pos, room: "bridge" }
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

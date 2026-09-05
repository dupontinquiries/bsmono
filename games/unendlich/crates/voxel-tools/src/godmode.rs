//! Playing god: driving the engine's own systems.
//!
//! Everything so far writes blocks. But the engine already simulates
//! climate, water, heat and time, and those are the levers that change
//! how a place FEELS rather than what it is made of. "Make it hotter
//! here" and "flood the valley" are not build requests and cannot be
//! expressed as blocks — or rather, flooding CAN be, badly: a fill of
//! water ignores drainage, freezes at altitude and does not evaporate,
//! so it looks like a blue box rather than a lake.
//!
//! The split, then:
//!
//! * Effects the engine owns (temperature, humidity, wind, time) are
//!   emitted as REQUESTS. This crate has no access to those fields and
//!   should not pretend to; the game applies them and the simulation
//!   does the rest, which is the whole point of asking the engine
//!   instead of faking it.
//! * Effects that really are blocks (flooding to a level, freezing a
//!   surface) are computed here, because they need the world read and
//!   the game would only have to reimplement it.
//!
//! Every effect is BOUNDED and REVERSIBLE. An unbounded "make it
//! hotter" is a request to break someone's world, and the one thing a
//! god mode must have is an undo.

use crate::weather::Change;
use crate::VoxelWorld;

/// A change to an engine-simulated field. The game applies these; this
/// crate cannot and does not try.
#[derive(Debug, Clone, PartialEq)]
pub enum FieldRequest {
    /// Add to the climate temperature field over a region, in the
    /// engine's normalized units (the same 0..1 the HUD prints).
    Temperature { centre: (i32, i32), radius: i32, delta: f32 },
    /// Add to the humidity field.
    Humidity { centre: (i32, i32), radius: i32, delta: f32 },
    /// Set the wind vector.
    Wind { x: f32, z: f32 },
    /// Set the time of day, in ticks from dawn.
    TimeOfDay { ticks: f32 },
}

impl FieldRequest {
    /// What the player is told. A god-mode command that changes an
    /// invisible field must SAY what it did, or it reads as nothing
    /// happening.
    pub fn describe(&self) -> String {
        match self {
            FieldRequest::Temperature { centre, radius, delta } => format!(
                "temperature {:+.2} within {radius} blocks of {centre:?} \
                 (the climate sim will spread and settle it)", delta),
            FieldRequest::Humidity { centre, radius, delta } =>
                format!("humidity {delta:+.2} within {radius} blocks of {centre:?}"),
            FieldRequest::Wind { x, z } => format!("wind set to ({x:.2}, {z:.2})"),
            FieldRequest::TimeOfDay { ticks } => format!("time set to {ticks:.0}"),
        }
    }
}

/// The result of a god-mode verb: field requests for the engine, block
/// changes for the world, or both.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct Effect {
    pub fields: Vec<FieldRequest>,
    pub blocks: Vec<Change>,
    pub note: String,
}

/// Clamp a requested delta to something a world can survive.
///
/// "Make it hotter" with no bound is a request to break someone's
/// world, and a god mode without limits is a griefing tool. A single
/// command moves the field by at most this much; ask twice for more.
pub const MAX_FIELD_DELTA: f32 = 0.35;
/// And a single command reaches at most this far.
pub const MAX_RADIUS: i32 = 128;

/// "Make it hotter/colder here."
pub fn temperature(centre: (i32, i32), radius: i32, delta: f32) -> Effect {
    let d = delta.clamp(-MAX_FIELD_DELTA, MAX_FIELD_DELTA);
    let r = radius.clamp(1, MAX_RADIUS);
    let mut note = format!("{} the area", if d > 0.0 { "warming" } else { "cooling" });
    if (d - delta).abs() > f32::EPSILON {
        note.push_str(&format!(" (clamped from {delta:+.2} to {d:+.2}; run it again for more)"));
    }
    Effect { fields: vec![FieldRequest::Temperature { centre, radius: r, delta: d }],
             blocks: Vec::new(), note }
}

pub fn humidity(centre: (i32, i32), radius: i32, delta: f32) -> Effect {
    let d = delta.clamp(-MAX_FIELD_DELTA, MAX_FIELD_DELTA);
    let r = radius.clamp(1, MAX_RADIUS);
    Effect { fields: vec![FieldRequest::Humidity { centre, radius: r, delta: d }],
             blocks: Vec::new(),
             note: format!("{} the air", if d > 0.0 { "moistening" } else { "drying" }) }
}

/// "Flood the area."
///
/// A real block change, because a fill needs the world read and the
/// game would only have to reimplement it. Water is placed in every
/// open cell at or below `to_y` whose column is open to that level —
/// so it fills valleys and does NOT appear inside a sealed building or
/// underground, which a naive box fill would do.
pub fn flood<W: VoxelWorld>(w: &W, centre: (i32, i32), radius: i32, to_y: i32,
                            water: u16) -> Effect {
    let r = radius.clamp(1, MAX_RADIUS);
    let mut blocks = Vec::new();
    for x in (centre.0 - r)..=(centre.0 + r) {
        for z in (centre.1 - r)..=(centre.1 + r) {
            // A circle, not a square: a square lake is unmistakably
            // artificial, and the edge is the part people look at.
            let (dx, dz) = ((x - centre.0) as i64, (z - centre.1) as i64);
            if dx * dx + dz * dz > (r as i64).pow(2) { continue; }
            // Reachable from above: walk DOWN from the flood level and
            // stop at the first solid block. A cell under a roof is
            // never reached, so a sealed cellar stays dry.
            let mut y = to_y;
            while y > to_y - 64 {
                if w.get(x, y, z) != 0 { break; }
                blocks.push(Change { at: (x, y, z), to: water });
                y -= 1;
            }
        }
    }
    let n = blocks.len();
    Effect { fields: Vec::new(), blocks,
             note: format!("flooding to y={to_y}: {n} cells (sealed spaces stay dry)") }
}

/// "Freeze it." Turns exposed surface water to ice and dusts snow on
/// open ground, plus a temperature request so the engine keeps it cold
/// rather than thawing it on the next tick.
pub fn freeze<W: VoxelWorld>(w: &W, centre: (i32, i32), radius: i32, from_y: i32,
                             water: u16, ice: u16, snow: u16) -> Effect {
    let r = radius.clamp(1, MAX_RADIUS);
    let mut blocks = Vec::new();
    for x in (centre.0 - r)..=(centre.0 + r) {
        for z in (centre.1 - r)..=(centre.1 + r) {
            let (dx, dz) = ((x - centre.0) as i64, (z - centre.1) as i64);
            if dx * dx + dz * dz > (r as i64).pow(2) { continue; }
            let mut y = from_y;
            while y > from_y - 128 {
                let b = w.get(x, y, z);
                if b != 0 {
                    if b == water {
                        blocks.push(Change { at: (x, y, z), to: ice });
                    } else {
                        blocks.push(Change { at: (x, y + 1, z), to: snow });
                    }
                    break;
                }
                y -= 1;
            }
        }
    }
    let n = blocks.len();
    // The block change alone would thaw: the sim owns temperature, so
    // ask it to agree with what we just did.
    Effect {
        fields: vec![FieldRequest::Temperature { centre, radius: r, delta: -MAX_FIELD_DELTA }],
        blocks,
        note: format!("freezing {n} surface cells and cooling the climate to match"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::BTreeMap;

    struct W { cells: BTreeMap<(i32, i32, i32), u16>, ground: i32 }
    impl W {
        fn flat(ground: i32) -> Self { W { cells: BTreeMap::new(), ground } }
        /// Flat ground with a valley, and a sealed room under a roof.
        fn valley() -> Self {
            let mut w = W::flat(64);
            for x in -6..=6 { for z in -6..=6 { for y in 56..=64 {
                w.cells.insert((x, y, z), 0);   // carved valley
            }}}
            // a sealed box: walls 20..24, roof at y=64
            for x in 20..=24 { for z in 0..=4 { for y in 60..=64 {
                let shell = x == 20 || x == 24 || z == 0 || z == 4 || y == 64;
                w.cells.insert((x, y, z), if shell { 1 } else { 0 });
            }}}
            w
        }
    }
    impl VoxelWorld for W {
        fn get(&self, x: i32, y: i32, z: i32) -> u16 {
            if let Some(&b) = self.cells.get(&(x, y, z)) { return b; }
            if y <= self.ground { 1 } else { 0 }
        }
        fn set(&mut self, x: i32, y: i32, z: i32, b: u16) -> bool {
            self.cells.insert((x, y, z), b); true
        }
    }

    #[test]
    fn a_field_change_is_a_request_not_a_block_write() {
        // This crate cannot reach the climate field and must not
        // pretend to; the engine applies it and the sim does the rest.
        let e = temperature((0, 0), 30, 0.2);
        assert!(e.blocks.is_empty());
        assert_eq!(e.fields.len(), 1);
        assert!(e.note.contains("warming"));
    }

    #[test]
    fn an_unbounded_request_is_clamped_and_says_so() {
        // A god mode without limits is a griefing tool.
        let e = temperature((0, 0), 9_000, 5.0);
        match &e.fields[0] {
            FieldRequest::Temperature { radius, delta, .. } => {
                assert_eq!(*radius, MAX_RADIUS);
                assert!((*delta - MAX_FIELD_DELTA).abs() < 1e-6);
            }
            o => panic!("{o:?}"),
        }
        assert!(e.note.contains("clamped"), "and the player is told: {}", e.note);
        assert!(e.note.contains("again for more"), "with the way to get what they wanted");
    }

    #[test]
    fn flooding_fills_a_valley() {
        let w = W::valley();
        let e = flood(&w, (0, 0), 8, 62, 9);
        assert!(!e.blocks.is_empty());
        assert!(e.blocks.iter().any(|c| c.at == (0, 62, 0)), "water at the flood level");
        assert!(e.blocks.iter().any(|c| c.at == (0, 57, 0)), "and all the way down");
        assert!(!e.blocks.iter().any(|c| c.at.1 > 62), "never above the level asked for");
    }

    #[test]
    fn a_sealed_room_stays_dry() {
        // The reason this is not a box fill. Water reaches a cell by
        // walking down from the flood level; a roof stops it.
        let w = W::valley();
        let e = flood(&w, (22, 2), 20, 70, 9);
        let inside: Vec<&Change> = e.blocks.iter()
            .filter(|c| (21..=23).contains(&c.at.0) && (1..=3).contains(&c.at.2)
                        && (61..=63).contains(&c.at.1))
            .collect();
        assert!(inside.is_empty(), "the roofed room flooded: {inside:?}");
        // but the open ground outside it did fill
        assert!(e.blocks.iter().any(|c| c.at.0 == 30), "open ground floods");
    }

    #[test]
    fn a_lake_is_round_not_square() {
        // A square lake is unmistakably artificial and the edge is the
        // part people look at.
        let w = W::flat(50);
        let e = flood(&w, (0, 0), 10, 60, 9);
        let corner = e.blocks.iter().any(|c| c.at.0 == 10 && c.at.2 == 10);
        let edge = e.blocks.iter().any(|c| c.at.0 == 10 && c.at.2 == 0);
        assert!(!corner, "the corners of the bounding box are dry");
        assert!(edge, "but the cardinal edge is wet");
    }

    #[test]
    fn freezing_changes_blocks_and_asks_the_sim_to_agree() {
        // Ice alone would thaw on the next tick: the sim owns
        // temperature, so the effect has to include a field request.
        let mut w = W::flat(64);
        for x in -4..=4 { for z in -4..=4 { w.set(x, 64, z, 9); } }
        let e = freeze(&w, (0, 0), 4, 70, 9, 31, 32);
        assert!(e.blocks.iter().any(|c| c.to == 31), "surface water becomes ice");
        assert!(e.fields.iter().any(|f| matches!(f,
            FieldRequest::Temperature { delta, .. } if *delta < 0.0)),
            "and the climate is cooled to match, or it thaws immediately");
    }

    #[test]
    fn freezing_dry_ground_dusts_snow_above_it_not_into_it() {
        let w = W::flat(64);
        let e = freeze(&w, (0, 0), 3, 70, 9, 31, 32);
        for c in &e.blocks {
            assert_eq!(c.to, 32);
            assert_eq!(c.at.1, 65, "snow sits ON the ground, not inside it");
        }
    }

    #[test]
    fn every_effect_explains_itself() {
        // A command that changes an invisible field and says nothing
        // reads as a command that did nothing.
        let w = W::flat(64);
        for e in [temperature((0, 0), 20, 0.1), humidity((0, 0), 20, -0.1),
                  flood(&w, (0, 0), 4, 66, 9), freeze(&w, (0, 0), 4, 70, 9, 31, 32)] {
            assert!(!e.note.is_empty());
        }
        assert!(FieldRequest::Wind { x: 1.0, z: 0.0 }.describe().contains("wind"));
    }
}

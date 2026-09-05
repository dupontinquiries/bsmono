//! Verbs against a build that already exists.
//!
//! "Fortify my base" is not a shape and not a detail pass. It is a
//! PLAN — walls at the boundary, towers where walls meet, a gate on the
//! side the path comes from — composed from ops the IR already has,
//! with every coordinate derived from the workset's own box. The
//! player names the intent; the geometry is arithmetic.
//!
//! These are MACROS: each expands to IR text, which then runs through
//! the same parser, validator, preview and undo as anything else. Two
//! consequences worth having:
//!
//! * A model can emit one line (`fortify w3`) instead of nine, and a
//!   1B is far more likely to get one line right.
//! * The player sees the expansion in the preview and can edit it, so a
//!   macro is a starting point rather than a black box.
//!
//! Every verb here answers the question "what would a builder do to
//! THIS, given where it is" — which is why they take the box and the
//! site, not just a name.

use crate::land::Workset;

/// Which side of a box faces `toward`. Used to put the gate on the side
/// the path arrives from rather than on an arbitrary one.
fn facing(min: (i32, i32, i32), max: (i32, i32, i32), toward: (i32, i32)) -> &'static str {
    let cx = (min.0 + max.0) / 2;
    let cz = (min.2 + max.2) / 2;
    let (dx, dz) = (toward.0 - cx, toward.1 - cz);
    if dx.abs() >= dz.abs() {
        if dx >= 0 { "east" } else { "west" }
    } else if dz >= 0 { "south" } else { "north" }
}

/// Walls, corner towers, and a gate facing `gate_toward` (the path, the
/// player, the road). `margin` is how far outside the build the wall
/// stands; a wall touching the walls it protects is not a wall.
pub fn fortify(w: &Workset, margin: i32, wall_h: i32, block: &str, accent: &str,
               gate_toward: (i32, i32)) -> String {
    let m = margin.max(2);
    let (x0, x1) = (w.min.0 - m, w.max.0 + m);
    let (z0, z1) = (w.min.2 - m, w.max.2 + m);
    let h = wall_h.max(3);
    let tower_r = 2;
    let tower_h = h + 4;
    let cx = (x0 + x1) / 2;
    let cz = (z0 + z1) / 2;

    let mut ir = format!("# fortify {}\n", w.name);
    // The curtain wall.
    ir.push_str(&format!("enclose {x0},{z0} {x1},{z1} h={h} block={block}\n"));
    // Towers where walls meet: the corners are where a wall is weakest
    // and where the eye expects mass.
    for (tx, tz) in [(x0, z0), (x1, z0), (x0, z1), (x1, z1)] {
        ir.push_str(&format!("cylinder {tx},{tz} r={tower_r} h={tower_h} block={block}\n"));
    }
    // A gate on the side the path arrives from. Carved as air through
    // the wall, with an accent lintel so it reads as an entrance.
    let side = facing(w.min, w.max, gate_toward);
    let (gx, gz) = match side {
        "east" => (x1, cz), "west" => (x0, cz),
        "south" => (cx, z1), _ => (cx, z0),
    };
    ir.push_str(&format!("sphere {gx},{gz} r=1 block=air\n"));
    ir.push_str(&format!("fill {},{},{} {},{},{} block={accent}\n",
                         gx - 2, w.min.1 + 3, gz - 1, gx + 2, w.min.1 + 3, gz + 1));
    // A walkway along the top so the wall can be patrolled: a builder
    // never leaves a wall with nothing on it.
    ir.push_str(&format!("enclose {},{} {},{} h=1 y={} block={accent}\n",
                         x0 + 1, z0 + 1, x1 - 1, z1 - 1, w.min.1 + h));
    ir
}

/// Extend a build outward on one side by `by` blocks, matching its
/// height. "Make it bigger" without a direction grows the side with
/// the most room.
pub fn expand(w: &Workset, side: &str, by: i32, block: &str) -> String {
    let by = by.max(2);
    let h = (w.max.1 - w.min.1).max(3);
    let (min, max) = match side {
        "east" => ((w.max.0, w.min.2), (w.max.0 + by, w.max.2)),
        "west" => ((w.min.0 - by, w.min.2), (w.min.0, w.max.2)),
        "south" => ((w.min.0, w.max.2), (w.max.0, w.max.2 + by)),
        _ => ((w.min.0, w.min.2 - by), (w.max.0, w.min.2)),
    };
    format!("# expand {} {side}\n\
             enclose {},{} {},{} h={h} block={block}\n\
             sphere {},{} r=1 block=air\n",
            w.name, min.0, min.1, max.0, max.1,
            // an opening where the new wing meets the old
            (min.0 + max.0) / 2, (min.1 + max.1) / 2)
}

/// Lanterns along the top edge at intervals and one at the door.
/// A build with no light source reads as abandoned at night.
pub fn light(w: &Workset, spacing: i32, lantern: &str) -> String {
    let s = spacing.max(3);
    let top = w.max.1 + 1;
    let mut ir = format!("# light {}\n", w.name);
    let mut x = w.min.0;
    while x <= w.max.0 {
        ir.push_str(&format!("fill {x},{top},{} {x},{top},{} block={lantern}\n", w.min.2, w.min.2));
        ir.push_str(&format!("fill {x},{top},{} {x},{top},{} block={lantern}\n", w.max.2, w.max.2));
        x += s;
    }
    let mut z = w.min.2 + s;
    while z < w.max.2 {
        ir.push_str(&format!("fill {},{top},{z} {},{top},{z} block={lantern}\n", w.min.0, w.min.0));
        ir.push_str(&format!("fill {},{top},{z} {},{top},{z} block={lantern}\n", w.max.0, w.max.0));
        z += s;
    }
    ir
}

/// A path between two builds, bridging if there is a gap to cross.
pub fn connect(a: &Workset, b: &Workset, width: i32, block: &str, bridge: bool) -> String {
    let ac = ((a.min.0 + a.max.0) / 2, (a.min.2 + a.max.2) / 2);
    let bc = ((b.min.0 + b.max.0) / 2, (b.min.2 + b.max.2) / 2);
    let verb = if bridge { "bridge" } else { "path" };
    format!("# connect {} to {}\n{verb} {},{} {},{} w={} block={block}\n",
            a.name, b.name, ac.0, ac.1, bc.0, bc.1, width.max(1))
}

/// Undo weathering: restore the build's palette over its box. Damage
/// is not reversible without the original, so this restyles what is
/// there rather than pretending to rebuild what fell.
pub fn repair(w: &Workset, palette: &str) -> String {
    format!("# repair {}\nrestyle {},{},{} {},{},{} ruined {palette}\n\
             restyle {},{},{} {},{},{} darkwood {palette}\n",
            w.name, w.min.0, w.min.1, w.min.2, w.max.0, w.max.1, w.max.2,
            w.min.0, w.min.1, w.min.2, w.max.0, w.max.1, w.max.2)
}

/// Map a phrase to a macro, if it is one. Returns the verb and the
/// direction/argument it parsed, so the caller can expand it against
/// the right workset.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Verb {
    Fortify,
    Expand(String),
    Light,
    Connect,
    Repair,
}

pub fn recognize(prose: &str) -> Option<Verb> {
    let p = prose.to_lowercase();
    let has = |ws: &[&str]| ws.iter().any(|w| p.contains(w));
    if has(&["fortify", "defend", "walls around", "wall it", "protect my", "fortifications"]) {
        return Some(Verb::Fortify);
    }
    if has(&["light it", "lanterns", "light up", "lights on", "illuminate"]) {
        return Some(Verb::Light);
    }
    if has(&["repair", "restore", "fix it up", "fix the", "clean it up"]) {
        return Some(Verb::Repair);
    }
    if has(&["connect", "link", "a path between", "road between", "join the"]) {
        return Some(Verb::Connect);
    }
    if has(&["expand", "extend", "bigger", "larger", "add a wing", "enlarge"]) {
        let side = ["north", "south", "east", "west"].iter()
            .find(|d| p.contains(*d)).map(|d| d.to_string())
            .unwrap_or_else(|| "east".into());
        return Some(Verb::Expand(side));
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan, Severity};

    fn base() -> Workset {
        Workset { id: "w1".into(), name: "base".into(), owner: "kess".into(),
                  members: vec![], min: (10, 64, 10), max: (20, 70, 18), turns: vec![] }
    }

    #[test]
    fn fortify_expands_to_valid_ir_with_walls_towers_and_a_gate() {
        let ir = fortify(&base(), 3, 5, "stone", "dark_oak_planks", (40, 14));
        let (plan, issues) = parse_plan(&ir);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        let text = format!("{:?}", plan.ops);
        assert!(text.contains("Enclose"), "a curtain wall");
        assert_eq!(text.matches("Cylinder").count(), 4, "four corner towers");
        assert!(text.contains("Sphere"), "a gate carved as air");
        assert_eq!(plan.name, "fortify base");
    }

    #[test]
    fn the_wall_stands_outside_the_build_not_on_it() {
        let ir = fortify(&base(), 3, 5, "stone", "stone", (40, 14));
        // the enclose line's corners are the base's box plus the margin
        let wall = ir.lines().find(|l| l.starts_with("enclose")).unwrap();
        assert!(wall.contains("7,7") && wall.contains("23,21"), "{wall}");
    }

    #[test]
    fn the_gate_faces_where_the_path_comes_from() {
        let east = fortify(&base(), 3, 5, "stone", "stone", (60, 14));
        let west = fortify(&base(), 3, 5, "stone", "stone", (-40, 14));
        let gate = |ir: &str| ir.lines().find(|l| l.starts_with("sphere")).unwrap().to_string();
        assert!(gate(&east).contains("23,"), "gate on the east wall: {}", gate(&east));
        assert!(gate(&west).contains("7,"), "gate on the west wall: {}", gate(&west));
    }

    #[test]
    fn expand_grows_the_named_side_to_the_existing_height() {
        let ir = expand(&base(), "east", 6, "oak_planks");
        let (plan, issues) = parse_plan(&ir);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        let wing = ir.lines().find(|l| l.starts_with("enclose")).unwrap();
        assert!(wing.contains("20,10 26,18"), "from the east face outward: {wing}");
        assert!(wing.contains("h=6"), "matching the build's own height: {wing}");
        assert!(!plan.ops.is_empty());
    }

    #[test]
    fn light_places_lanterns_at_intervals_not_everywhere() {
        let ir = light(&base(), 4, "glow_crystal");
        let n = ir.lines().filter(|l| l.starts_with("fill")).count();
        let perimeter = 2 * (11 + 9);
        assert!(n < perimeter / 2, "{n} lanterns for a {perimeter}-block perimeter is too many");
        assert!(n >= 6, "and enough to matter: {n}");
        let (_, issues) = parse_plan(&ir);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
    }

    #[test]
    fn connect_runs_centre_to_centre_and_can_bridge() {
        let b = Workset { name: "mill".into(), min: (60, 64, 10), max: (66, 70, 16), ..base() };
        let p = connect(&base(), &b, 2, "cobblestone", false);
        assert!(p.contains("path 15,14 63,13"), "{p}");
        let br = connect(&base(), &b, 2, "oak_planks", true);
        assert!(br.contains("bridge 15,14 63,13"), "{br}");
    }

    #[test]
    fn phrases_map_to_verbs() {
        assert_eq!(recognize("fortify my base"), Some(Verb::Fortify));
        assert_eq!(recognize("put walls around it"), Some(Verb::Fortify));
        assert_eq!(recognize("make it bigger to the north"), Some(Verb::Expand("north".into())));
        assert_eq!(recognize("add a wing"), Some(Verb::Expand("east".into())),
                   "no direction given: a default, not a refusal");
        assert_eq!(recognize("light it up"), Some(Verb::Light));
        assert_eq!(recognize("repair the tower"), Some(Verb::Repair));
        assert_eq!(recognize("connect the mill to the barn"), Some(Verb::Connect));
        assert_eq!(recognize("a small hut"), None, "an ordinary build is not a macro");
    }

    #[test]
    fn every_macro_produces_ir_the_parser_accepts() {
        let b = Workset { name: "mill".into(), min: (60, 64, 10), max: (66, 70, 16), ..base() };
        for ir in [
            fortify(&base(), 3, 5, "stone", "dark_oak_planks", (40, 14)),
            expand(&base(), "north", 5, "stone"),
            light(&base(), 4, "glow_crystal"),
            connect(&base(), &b, 3, "cobblestone", false),
            repair(&base(), "stone"),
        ] {
            let (plan, issues) = parse_plan(&ir);
            let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "macro produced bad IR:\n{ir}\n{errs:?}");
            assert!(!plan.ops.is_empty(), "macro produced no ops:\n{ir}");
        }
    }
}

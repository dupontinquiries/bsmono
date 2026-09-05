//! Refinement: small passes after the big one.
//!
//! A build arrives in one shot — walls, roof, done — and then stops.
//! Everything that makes it look finished (block variation, trim,
//! settling into the ground, foliage creeping in) is a second, third
//! and fourth visit that never happens, because asking the model for
//! it means another 22-second round trip and another chance to get a
//! coordinate wrong.
//!
//! None of it needs the model. Every pass here is derived from the
//! build's own box and the site's own ambient reading, so a ladder of
//! them can run without a single token.
//!
//! WHY A LADDER RATHER THAN ONE BIG "FINISH" PASS
//!
//! * **Reviewable.** Each rung is one preview, one commit, one undo. A
//!   player who likes the speckle but hates the moss keeps the speckle.
//! * **Ordered.** Trim on top of speckle looks deliberate; speckle over
//!   trim eats it. Ground blend before foliage, or the foliage lands on
//!   bare ground and the spill covers it.
//! * **Bounded.** Each rung touches a fraction of the surface. Running
//!   all five is a different build; running two is a tidier one.
//!
//! The ladder is deliberately short. Five rungs a player can name beats
//! twenty they cannot predict.

use crate::ambient::Ambient;
use crate::land::Workset;

/// One rung: a name, what it does, and the IR that does it.
#[derive(Debug, Clone, PartialEq)]
pub struct Rung {
    pub name: &'static str,
    pub what: String,
    pub ir: String,
}

/// The ladder for a build, in the order the rungs must run.
///
/// Order is the content here. Trim after speckle so the edges read as
/// deliberate rather than as more noise; ground blend before foliage so
/// the foliage is not buried by the spill; weathering last because it
/// should act on the finished surface, including the trim.
pub fn ladder(w: &Workset, amb: &Ambient, seed: i32) -> Vec<Rung> {
    let s = amb.suggest();
    let (lo, hi) = (w.min, w.max);
    let mut out = Vec::new();

    out.push(Rung {
        name: "vary",
        what: "break up the flat faces with related blocks".into(),
        ir: format!("# refine vary\ndetail speckle {},{},{} {},{},{} amount=0.3 seed={seed}\n",
                    lo.0, lo.1, lo.2, hi.0, hi.1, hi.2),
    });
    out.push(Rung {
        name: "trim",
        what: format!("accent the edges in {}", block_name(s.accent)),
        ir: format!("# refine trim\ndetail trim {},{},{} {},{},{} block={} amount=0.55 seed={}\n",
                    lo.0, lo.1, lo.2, hi.0, hi.1, hi.2, block_name(s.accent), seed + 1),
    });
    out.push(Rung {
        name: "depth",
        what: "push a few wall cells out so the surface catches light".into(),
        ir: format!("# refine depth\ndetail relief {},{},{} {},{},{} amount=0.12 seed={}\n",
                    lo.0, lo.1, lo.2, hi.0, hi.1, hi.2, seed + 2),
    });
    out.push(Rung {
        name: "settle",
        what: "spill material around the foot so it does not look stamped".into(),
        ir: format!("# refine settle\ndetail blend {},{},{} {},{},{} block={} amount=0.35 seed={}\n",
                    lo.0, lo.1, lo.2, hi.0, lo.1, hi.2, block_name(s.structure), seed + 3),
    });
    // Foliage that suits the site, scattered just outside the footprint
    // rather than over it: a build with plants growing through its floor
    // reads as ruined, which is a different pass entirely.
    out.push(Rung {
        name: "green",
        what: format!("{} and ground cover around it", s.species),
        ir: format!("# refine green\nscatter {},{} {},{} block=flower clusters=4 per=5 \
                     spread=3 seed={}\ntree {},{} {} h=6\ntree {},{} {} h=7\n",
                    lo.0 - 6, lo.2 - 6, hi.0 + 6, hi.2 + 6, seed + 4,
                    lo.0 - 4, hi.2 + 4, s.species,
                    hi.0 + 4, lo.2 - 4, s.species),
    });
    out
}

/// The rung a player named, or None.
pub fn rung<'a>(rungs: &'a [Rung], name: &str) -> Option<&'a Rung> {
    rungs.iter().find(|r| r.name.eq_ignore_ascii_case(name.trim()))
}

/// Everything, as one plan. For "finish it" rather than "vary it".
pub fn all(rungs: &[Rung]) -> String {
    let mut s = String::from("# refine all\n");
    for r in rungs {
        for line in r.ir.lines() {
            if line.starts_with('#') { continue; }
            s.push_str(line);
            s.push('\n');
        }
    }
    s
}

/// The menu shown to the player.
pub fn menu(rungs: &[Rung]) -> String {
    let mut s = String::from("refinement passes (run in this order; /refine all for every one):\n");
    for (i, r) in rungs.iter().enumerate() {
        s.push_str(&format!("  {}. {:7} {}\n", i + 1, r.name, r.what));
    }
    s
}

fn block_name(b: u16) -> &'static str {
    match b {
        1 => "stone", 4 => "cobblestone", 58 => "stone_bricks", 57 => "oak_planks",
        86 => "dark_oak_planks", 48 => "sandstone", 24 => "red_sandstone",
        80 => "snow_block", 31 => "ice", 38 => "basalt", 17 => "oak_log",
        84 => "terracotta", 81 => "mossy_cobblestone", 37 => "marble", 42 => "calcite",
        _ => "stone",
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan, Severity};

    fn hut() -> Workset {
        Workset { id: "w1".into(), name: "hut".into(), owner: "kess".into(), members: vec![],
                  min: (10, 64, 10), max: (22, 72, 20), turns: vec![] }
    }

    #[test]
    fn every_rung_is_ir_the_parser_accepts() {
        let a = Ambient::default();
        for r in ladder(&hut(), &a, 3) {
            let (plan, issues) = parse_plan(&r.ir);
            let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "rung '{}' produced bad IR:\n{}\n{errs:?}", r.name, r.ir);
            assert!(!plan.ops.is_empty(), "rung '{}' produced no ops", r.name);
        }
    }

    #[test]
    fn the_order_is_the_content() {
        // Trim after speckle so the edges read as deliberate rather than
        // as more noise; settle before green so the foliage is not
        // buried by the spill.
        let names: Vec<&str> = ladder(&hut(), &Ambient::default(), 1)
            .iter().map(|r| r.name).collect();
        assert_eq!(names, ["vary", "trim", "depth", "settle", "green"]);
    }

    #[test]
    fn the_ladder_follows_the_site_not_a_constant() {
        // A hut on a glacier trims in ice and plants pines; the same hut
        // in a jungle does neither.
        let glacier = Ambient { surface_block: 32, temperature: 0.05, ..Default::default() };
        let jungle = Ambient { surface_block: 23, temperature: 0.85, humidity: 0.9,
                               ..Default::default() };
        let g = ladder(&hut(), &glacier, 1);
        let j = ladder(&hut(), &jungle, 1);
        assert_ne!(rung(&g, "trim").unwrap().ir, rung(&j, "trim").unwrap().ir,
                   "the accent follows the palette");
        assert!(rung(&g, "green").unwrap().ir.contains("pine"));
        assert!(rung(&j, "green").unwrap().ir.contains("jungle"));
    }

    #[test]
    fn each_rung_is_small_enough_to_review() {
        // The point of a ladder: a rung is one preview and one undo, not
        // a second build.
        for r in ladder(&hut(), &Ambient::default(), 1) {
            let (plan, _) = parse_plan(&r.ir);
            assert!(plan.ops.len() <= 3, "rung '{}' has {} ops", r.name, plan.ops.len());
        }
    }

    #[test]
    fn all_runs_every_rung_and_still_parses() {
        let rungs = ladder(&hut(), &Ambient::default(), 5);
        let ir = all(&rungs);
        let (plan, issues) = parse_plan(&ir);
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        let total: usize = rungs.iter()
            .map(|r| parse_plan(&r.ir).0.ops.len()).sum();
        assert_eq!(plan.ops.len(), total, "all = the sum of the rungs");
        assert!(!ir.contains("# refine vary"), "the per-rung headers are dropped");
    }

    #[test]
    fn the_settle_pass_works_at_ground_level_not_at_the_roof() {
        // Spill belongs at the foot. Passing the build's full height
        // would scatter it up the walls.
        let r = ladder(&hut(), &Ambient::default(), 1);
        let settle = rung(&r, "settle").unwrap();
        assert!(settle.ir.contains(",64,"), "the y range is the base: {}", settle.ir);
        assert!(!settle.ir.contains("72"), "not the roof: {}", settle.ir);
    }

    #[test]
    fn green_goes_around_the_build_not_through_it() {
        // A build with plants growing through its floor reads as ruined,
        // which is a different pass entirely.
        let r = ladder(&hut(), &Ambient::default(), 1);
        let green = rung(&r, "green").unwrap();
        assert!(green.ir.contains("4,4 28,26"), "footprint expanded outward: {}", green.ir);
    }

    #[test]
    fn the_menu_names_every_rung() {
        let rungs = ladder(&hut(), &Ambient::default(), 1);
        let m = menu(&rungs);
        for r in &rungs { assert!(m.contains(r.name), "{m}"); }
        assert!(m.contains("/refine all"));
    }
}

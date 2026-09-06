//! Finishing from the words.
//!
//! "A creepy stone statue" from a 1B came back as a pillar with a ball
//! on top. The word that carried all the intent -- creepy -- did
//! nothing, because a 1B does not know that creepy is `mood eerie`
//! plus `weather worn` plus gothic greebles, and would not have room
//! in its reply to say so if it did.
//!
//! So the adjectives are read HERE, after the plan, and each one adds
//! the finishing passes it implies over the plan's own box. The model
//! supplies the shape; the words supply the treatment. A 1B saying
//! `form statue @0,0 @6,6` and the player saying "creepy" together
//! produce what neither could alone.
//!
//! Everything added is a pass that already exists -- mood, weather,
//! greeble, detail, scatter, populate -- so nothing here is a new
//! capability. It is the connection between what the player said and
//! the passes that say it back.
//!
//! Only when the plan does not already contain them: a model that
//! wrote `mood eerie` itself is not second-guessed.

use crate::plan::Op;

/// One word's contribution, as IR templates with `{A}` and `{B}` for
/// the box corners (x,y,z) and `{FA}`/`{FB}` for the footprint (x,z).
struct Word {
    triggers: &'static [&'static str],
    lines: &'static [&'static str],
    /// A short reason, so the preview says why a pass was added.
    because: &'static str,
}

const WORDS: &[Word] = &[
    Word { triggers: &["creepy", "eerie", "spooky", "unsettling", "sinister", "ominous", "haunt"],
           lines: &["mood eerie {FA} {FB}", "weather {A} {B} worn seed=3",
                    "greeble {A} {B} gothic symmetric gens=10"],
           because: "creepy: eerie mood, a little decay, gothic detail" },
    // "old" is NOT here: "make the old tower taller" names a building,
    // not a treatment, and weathering it would be exactly wrong.
    Word { triggers: &["ancient", "ruined", "forgotten", "crumbling", "weathered", "aged", "decrepit"],
           lines: &["weather {A} {B} ancient seed=5"],
           because: "ancient: heavy weathering" },
    Word { triggers: &["overgrown", "mossy", "reclaimed"],
           lines: &["weather {A} {B} abandoned seed=7", "scatter {FA} {FB} block=moss clusters=6 per=6 spread=3"],
           because: "overgrown: moss and vines" },
    Word { triggers: &["ornate", "detailed", "intricate", "carved", "decorated", "elaborate"],
           lines: &["greeble {A} {B} gothic gens=14", "detail trim {A} {B} block=dark_oak_planks amount=0.5"],
           because: "ornate: gothic greebles and trim" },
    Word { triggers: &["industrial", "mechanical", "sci-fi", "scifi", "machine"],
           lines: &["greeble {A} {B} industrial gens=14", "detail speckle {A} {B} amount=0.25"],
           because: "industrial: studs, vents and variation" },
    Word { triggers: &["grand", "majestic", "imposing", "monumental", "regal"],
           lines: &["detail trim {A} {B} block=stone_bricks amount=0.6",
                    "greeble {A} {B} gothic symmetric gens=10"],
           because: "grand: trim and symmetry" },
    Word { triggers: &["cosy", "cozy", "homely", "warm", "welcoming", "quaint"],
           lines: &["mood cosy {FA} {FB}", "scatter {FA} {FB} block=flower clusters=4 per=5 spread=3"],
           because: "cosy: warm mood and flowers" },
    Word { triggers: &["sacred", "holy", "divine", "blessed"],
           lines: &["mood sacred {FA} {FB}", "detail trim {A} {B} block=marble amount=0.5"],
           because: "sacred: light and marble trim" },
    Word { triggers: &["fiery", "burning", "infernal", "volcanic", "lava"],
           lines: &["mood fiery {FA} {FB}", "weather {A} {B} burnt seed=2"],
           because: "fiery: heat, scorching" },
    Word { triggers: &["rustic", "rough", "crude", "primitive"],
           lines: &["detail speckle {A} {B} amount=0.35", "detail relief {A} {B} amount=0.15"],
           because: "rustic: uneven surfaces" },
];

/// Which ops count as "already finished" for a category, so a model's
/// own finishing is not duplicated.
fn already_has(ops: &[Op], line: &str) -> bool {
    let verb = line.split_whitespace().next().unwrap_or("");
    ops.iter().any(|o| match (verb, o) {
        ("mood", Op::Mood { .. }) => true,
        ("weather", Op::Weather { .. }) => true,
        ("greeble", Op::Greeble { .. }) => true,
        ("detail", Op::Detail { .. }) => true,
        ("scatter", Op::Scatter { .. }) => true,
        _ => false,
    })
}

/// How boxy a build is: the fraction of its bounding box that is
/// solid, and whether its skyline is flat. A box is the signature of a
/// generated building; a real one has a chimney, a turret, a wing --
/// something that breaks the outline against the sky.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Silhouette {
    /// solid cells / bounding box volume, 0..1
    pub fill: f32,
    /// how often a column's top differs from its neighbour's, 0..1.
    /// A flat lid is 0; a gable steps on every row; a chimney or turret
    /// adds edges. Counting DISTINCT heights was the first attempt, and
    /// a gable has only four across eighty columns.
    pub skyline_variety: f32,
    /// the footprint's aspect, longest over shortest
    pub aspect: f32,
}

impl Silhouette {
    /// Boxy: a full box with a flat top. The thresholds are where a
    /// shape stops reading as designed.
    pub fn is_boxy(&self) -> bool { self.fill > 0.55 && self.skyline_variety < 0.1 }
}

pub fn silhouette(placed: &[(i32, i32, i32)]) -> Option<Silhouette> {
    if placed.is_empty() { return None; }
    let lo = placed.iter().fold((i32::MAX, i32::MAX, i32::MAX), |a, p| (a.0.min(p.0), a.1.min(p.1), a.2.min(p.2)));
    let hi = placed.iter().fold((i32::MIN, i32::MIN, i32::MIN), |a, p| (a.0.max(p.0), a.1.max(p.1), a.2.max(p.2)));
    let vol = ((hi.0 - lo.0 + 1) as f32) * ((hi.1 - lo.1 + 1) as f32) * ((hi.2 - lo.2 + 1) as f32);
    let mut tops: std::collections::BTreeMap<(i32, i32), i32> = Default::default();
    for p in placed { let t = tops.entry((p.0, p.2)).or_insert(i32::MIN); *t = (*t).max(p.1); }
    let (mut pairs, mut steps) = (0usize, 0usize);
    for ((x, z), t) in &tops {
        for n in [(x + 1, *z), (*x, z + 1)] {
            if let Some(nt) = tops.get(&n) { pairs += 1; if nt != t { steps += 1; } }
        }
    }
    let (w, d) = ((hi.0 - lo.0 + 1) as f32, (hi.2 - lo.2 + 1) as f32);
    Some(Silhouette {
        fill: placed.len() as f32 / vol.max(1.0),
        skyline_variety: if pairs == 0 { 0.0 } else { steps as f32 / pairs as f32 },
        aspect: w.max(d) / w.min(d).max(1.0),
    })
}

/// IR that breaks a boxy silhouette: a corner turret or a chimney, and
/// trim along the roofline, sized to the box. Seeded, so two boxy
/// builds get different breakers.
pub fn break_silhouette(bounds: ((i32, i32, i32), (i32, i32, i32)), seed: u64) -> (String, String) {
    let (lo, hi) = bounds;
    let w = hi.0 - lo.0 + 1;
    let d = hi.2 - lo.2 + 1;
    let h = hi.1 - lo.1 + 1;
    let pick = (seed % 3) as u8;
    let corner = match seed / 3 % 4 { 0 => (lo.0, lo.2), 1 => (hi.0, lo.2), 2 => (lo.0, hi.2), _ => (hi.0, hi.2) };
    let mut ir = String::new();
    let why = match pick {
        0 => {
            // a corner turret, taller than the body, with a conical cap
            let r = (w.min(d) / 6).clamp(2, 4);
            ir.push_str(&format!("cylinder {},{} r={r} h={} block=stone_bricks hollow
", corner.0, corner.1, h + r + 2));
            ir.push_str(&format!("roof {},{} {},{} pyramid block=dark_oak_planks
",
                                 corner.0 - r, corner.1 - r, corner.0 + r, corner.1 + r));
            "a corner turret"
        }
        1 => {
            // a chimney stack off-centre, rising above the roof
            let cx = lo.0 + w / 3;
            let cz = lo.2 + d / 2;
            ir.push_str(&format!("fill {cx},{},{cz} {},{},{} block=stone_bricks
", hi.1 - 2, cx + 1, hi.1 + 3, cz + 1));
            "a chimney"
        }
        _ => {
            // a raised central bay: a second storey over the middle third
            let (bx0, bx1) = (lo.0 + w / 3, hi.0 - w / 3);
            ir.push_str(&format!("enclose {bx0},{} {bx1},{} h=3 y={} block=stone_bricks
", lo.2 + 1, hi.2 - 1, hi.1 + 1));
            ir.push_str(&format!("roof {bx0},{} {bx1},{} gable block=dark_oak_planks
", lo.2 + 1, hi.2 - 1));
            "a raised bay"
        }
    };
    (ir, format!("boxy silhouette: added {why}"))
}

/// The finishing lines the prose implies, over the box the plan built.
/// Empty when no word applies, the box is unknown, or the plan already
/// finishes itself. Returns (IR, reasons).
pub fn from_prose(prose: &str, ops: &[Op],
                  bounds: Option<((i32, i32, i32), (i32, i32, i32))>) -> (String, Vec<String>) {
    let Some((lo, hi)) = bounds else { return (String::new(), Vec::new()) };
    let p = prose.to_lowercase();
    let words: Vec<&str> = p.split(|c: char| !c.is_alphanumeric() && c != '-').filter(|w| !w.is_empty()).collect();
    let hit = |t: &str| words.iter().any(|w| *w == t || (w.len() > t.len() && w.starts_with(t) && t.len() >= 4));
    // Pad the box by one so passes reach the outer faces, and give the
    // footprint a little apron for scatter.
    let a = format!("{},{},{}", lo.0 - 1, lo.1, lo.2 - 1);
    let b = format!("{},{},{}", hi.0 + 1, hi.1 + 1, hi.2 + 1);
    let fa = format!("{},{}", lo.0 - 3, lo.2 - 3);
    let fb = format!("{},{}", hi.0 + 3, hi.2 + 3);
    let mut ir = String::new();
    let mut why = Vec::new();
    for w in WORDS {
        if !w.triggers.iter().any(|t| hit(t)) { continue; }
        let mut added = false;
        for line in w.lines {
            if already_has(ops, line) { continue; }
            let l = line.replace("{A}", &a).replace("{B}", &b).replace("{FA}", &fa).replace("{FB}", &fb);
            ir.push_str(&l); ir.push('\n');
            added = true;
        }
        if added { why.push(w.because.to_string()); }
    }
    (ir, why)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::plan::{parse_plan_at, Severity};

    fn ops(ir: &str) -> Vec<Op> { parse_plan_at(ir, Some((0, 65, 0))).0.ops }
    const BOX: Option<((i32, i32, i32), (i32, i32, i32))> = Some(((0, 65, 0), (6, 80, 6)));

    #[test]
    fn creepy_adds_mood_decay_and_gothic_detail() {
        let (ir, why) = from_prose("a creepy stone statue", &ops("form statue @0,0 @6,6\n"), BOX);
        assert!(ir.contains("mood eerie"), "{ir}");
        assert!(ir.contains("weather") && ir.contains("worn"), "{ir}");
        assert!(ir.contains("greeble") && ir.contains("gothic"), "{ir}");
        assert_eq!(why.len(), 1);
        assert!(why[0].starts_with("creepy"));
        // and it all parses as IR the run can execute
        let (plan, issues) = parse_plan_at(&ir, Some((0, 65, 0)));
        assert!(issues.iter().all(|i| i.severity != Severity::Error), "{issues:?}");
        assert_eq!(plan.ops.len(), 3);
    }

    #[test]
    fn plain_prose_adds_nothing() {
        let (ir, why) = from_prose("a stone statue", &ops("form statue @0,0 @6,6\n"), BOX);
        assert!(ir.is_empty() && why.is_empty());
    }

    #[test]
    fn a_plan_that_finishes_itself_is_not_second_guessed() {
        let (ir, _) = from_prose("a creepy statue",
                                 &ops("form statue @0,0 @6,6\nmood haunted @-4,-4 @10,10\n"), BOX);
        assert!(!ir.contains("mood"), "the model chose its own mood: {ir}");
        assert!(ir.contains("weather"), "but the rest still applies");
    }

    #[test]
    fn no_box_means_no_finishing() {
        let (ir, _) = from_prose("a creepy statue", &ops("form statue @0,0 @6,6\n"), None);
        assert!(ir.is_empty(), "nothing to finish without knowing where it is");
    }

    #[test]
    fn words_stack() {
        let (ir, why) = from_prose("an ancient overgrown temple", &ops("form house @0,0 @8,8\n"), BOX);
        assert!(why.len() == 2, "{why:?}");
        assert!(ir.matches("weather").count() >= 1);
        assert!(ir.contains("moss"));
    }

    #[test]
    fn stems_match_but_substrings_do_not() {
        // "haunted" matches "haunt"; "gold" must not match "old"
        assert!(!from_prose("a gold tower", &[], BOX).0.contains("weather"));
        assert!(from_prose("make the old tower taller", &[], BOX).0.is_empty(),
                "'old' names the building, not a treatment");
        assert!(from_prose("a wild west saloon", &[], BOX).0.is_empty());
        assert!(from_prose("a haunted tower", &[], BOX).0.contains("mood eerie"));
        assert!(from_prose("something bold and cold", &[], BOX).0.is_empty());
    }

    #[test]
    fn every_template_line_is_valid_ir() {
        // A word table that emits a bad line would fail every request
        // containing that word; the table is checked as a whole.
        let all: Vec<&str> = WORDS.iter().flat_map(|w| w.lines.iter().copied()).collect();
        for line in all {
            let l = line.replace("{A}", "0,65,0").replace("{B}", "6,80,6")
                        .replace("{FA}", "-3,-3").replace("{FB}", "9,9");
            let (_, issues) = parse_plan_at(&format!("{l}\n"), Some((0, 65, 0)));
            let errs: Vec<_> = issues.iter().filter(|i| i.severity == Severity::Error).collect();
            assert!(errs.is_empty(), "template {line:?}: {errs:?}");
        }
    }

    #[test]
    fn a_box_reads_as_boxy_and_a_house_does_not() {
        let mut solid = Vec::new();
        for x in 0..10 { for y in 0..6 { for z in 0..8 { solid.push((x, y, z)); } } }
        let s = silhouette(&solid).unwrap();
        assert!(s.is_boxy(), "{s:?}");
        // the same box with a pitched roof and a chimney: varied skyline
        let mut house = solid.clone();
        for x in 0..10i32 { for z in 0..8i32 { let ridge = 6 + (4 - (z - 4).abs()).max(0) / 2; for y in 6..ridge { house.push((x, y, z)); } } }
        for y in 6..12 { house.push((2, y, 4)); }
        let h = silhouette(&house).unwrap();
        assert!(!h.is_boxy(), "{h:?}");
    }

    #[test]
    fn breakers_are_valid_ir_and_differ_by_seed() {
        let b = ((0, 65, 0), (11, 70, 7));
        let mut seen = std::collections::BTreeSet::new();
        for seed in 0..12u64 {
            let (ir, why) = break_silhouette(b, seed);
            let (plan, issues) = parse_plan_at(&ir, Some((0, 65, 0)));
            assert!(issues.iter().all(|i| i.severity != Severity::Error), "seed {seed}: {issues:?}\n{ir}");
            assert!(!plan.ops.is_empty());
            assert!(why.contains("boxy"));
            seen.insert(why);
        }
        assert!(seen.len() >= 3, "turret, chimney and bay all appear: {seen:?}");
    }
}

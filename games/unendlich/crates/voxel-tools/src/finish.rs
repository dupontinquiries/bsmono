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
}

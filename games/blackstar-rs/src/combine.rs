//! COMBINE — several bots folding into one larger one.
//!
//! You spotted a walker tank carrying two models in one file that looked
//! right merged, and the reason it works is that a combiner IS the fold
//! system at a larger scale. `xform` moves parts along joint chains into
//! their place in a body; this moves BODIES along the same kind of chain
//! into their place in a bigger one. Nothing new is needed to animate it —
//! the member's own fold plays, and the combination is another set of
//! transforms on top.
//!
//! **What makes it a design rather than a gimmick is that not every set can
//! combine.** A leg wants something long and load-bearing; a torso wants
//! bulk; an arm wants reach. Those are proportions, and proportions are
//! measured — the same extents `partclass` already reads. So a squad
//! discovers whether it can merge by what its members ARE, and a squad of
//! five identical racers cannot, which is the interesting answer.
//!
//! Everything downstream falls out of what already exists. Combined mass and
//! reach go into `actuator`, so a gestalt folds slowly because it is moving
//! more metal. `idle` reads its size, so it breathes slowly. It is boss
//! scale by construction rather than by a multiplier someone typed.

/// Where a member sits in the larger body.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Slot {
    Torso,
    ArmL,
    ArmR,
    LegL,
    LegR,
}

pub const SLOTS: [Slot; 5] = [Slot::Torso, Slot::ArmL, Slot::ArmR, Slot::LegL, Slot::LegR];

impl Slot {
    pub fn label(self) -> &'static str {
        match self {
            Slot::Torso => "TORSO",
            Slot::ArmL => "LEFT ARM",
            Slot::ArmR => "RIGHT ARM",
            Slot::LegL => "LEFT LEG",
            Slot::LegR => "RIGHT LEG",
        }
    }
    /// Mirror pairs: an arm should be matched by the other arm, so a squad
    /// with one long body and one stubby one reads as lopsided and scores
    /// worse than two mediocre matched ones.
    pub fn partner(self) -> Option<Slot> {
        match self {
            Slot::ArmL => Some(Slot::ArmR),
            Slot::ArmR => Some(Slot::ArmL),
            Slot::LegL => Some(Slot::LegR),
            Slot::LegR => Some(Slot::LegL),
            Slot::Torso => None,
        }
    }
    /// What the role wants: (elongation, bulk), each 0..1.
    ///
    /// A leg is long and heavy — it carries everything. An arm is long and
    /// light. A torso is neither long nor light; it is the thing the others
    /// hang off.
    pub fn wants(self) -> (f32, f32) {
        match self {
            Slot::Torso => (0.25, 1.00),
            Slot::ArmL | Slot::ArmR => (0.85, 0.30),
            Slot::LegL | Slot::LegR => (0.75, 0.75),
        }
    }
}

/// A candidate member, as measured rather than as labelled.
#[derive(Clone, Copy, Debug)]
pub struct Member {
    pub name: &'static str,
    /// longest / shortest extent, normalised to 0..1 by an elongation of 4
    pub elongation: f32,
    /// mass relative to the squad's largest, 0..1
    pub bulk: f32,
    /// metres, longest dimension — sets the gestalt's scale
    pub reach_m: f32,
    /// kg
    pub mass: f32,
}

/// How well this body suits this role, 0 useless .. 1 made for it.
pub fn fitness(m: &Member, s: Slot) -> f32 {
    let (we, wb) = s.wants();
    let de = (m.elongation.clamp(0.0, 1.0) - we).abs();
    let db = (m.bulk.clamp(0.0, 1.0) - wb).abs();
    (1.0 - (de * 0.55 + db * 0.45)).clamp(0.0, 1.0)
}

/// A combination that works.
#[derive(Clone, Debug)]
pub struct Gestalt {
    pub members: Vec<(Slot, Member)>,
    /// mean fitness across the five roles
    pub coherence: f32,
    /// how alike the mirrored pairs are, 0 lopsided .. 1 matched
    pub symmetry: f32,
    pub total_mass: f32,
    pub reach_m: f32,
}

impl Gestalt {
    /// Scale against an ordinary bot, for the renderer and for `idle`.
    pub fn scale_vs(&self, bot_reach_m: f32) -> f32 {
        (self.reach_m / bot_reach_m.max(0.1)).max(1.0)
    }
}

/// The bar a squad has to clear. Below this it reads as five bots stuck
/// together rather than one machine.
pub const MIN_COHERENCE: f32 = 0.62;
pub const MIN_SYMMETRY: f32 = 0.55;

/// Assign members to roles, best-fit first.
///
/// Greedy on the strongest pairing rather than optimal: five members into
/// five slots is small enough that the difference rarely matters, and greedy
/// is inspectable — you can see WHY a body got a role, which a Hungarian
/// solve does not give you.
pub fn assign(members: &[Member]) -> Option<Gestalt> {
    if members.len() != SLOTS.len() {
        return None;
    }
    let mut free_m: Vec<usize> = (0..members.len()).collect();
    let mut free_s: Vec<Slot> = SLOTS.to_vec();
    let mut out: Vec<(Slot, Member)> = Vec::new();
    while !free_s.is_empty() {
        let mut best: Option<(f32, usize, usize)> = None;
        for (si, s) in free_s.iter().enumerate() {
            for (mi, m) in free_m.iter().enumerate() {
                let f = fitness(&members[*m], *s);
                if best.map_or(true, |(bf, _, _)| f > bf) {
                    best = Some((f, si, mi));
                }
            }
        }
        let (_, si, mi) = best?;
        out.push((free_s[si], members[free_m[mi]]));
        free_s.remove(si);
        free_m.remove(mi);
    }
    // THE WORST LIMB, NOT THE MEAN. Averaging let five identical racers
    // score 0.73 — each is mediocre at every role and mediocre five times
    // averages to acceptable, which is exactly wrong. A gestalt with one
    // limb that does not fit reads as five bots stuck together, however good
    // the other four are, so the minimum is the honest measure.
    let coherence = out
        .iter()
        .map(|(s, m)| fitness(m, *s))
        .fold(1.0f32, f32::min);
    // mirrored pairs should resemble each other
    let mut sym = Vec::new();
    for (s, m) in &out {
        if let Some(p) = s.partner() {
            if let Some((_, o)) = out.iter().find(|(x, _)| *x == p) {
                let a = m.reach_m.max(o.reach_m);
                let b = m.reach_m.min(o.reach_m);
                sym.push(b / a.max(1e-6));
            }
        }
    }
    // same reasoning: one lopsided pair is lopsided. Averaging a mismatched
    // arm pair against a matched leg pair scored 0.65 and passed, when the
    // thing has one arm a third the length of the other.
    let symmetry = sym.iter().copied().fold(1.0f32, f32::min);
    let total_mass = out.iter().map(|(_, m)| m.mass).sum();
    // the gestalt stands as tall as its legs plus its torso
    let leg = out
        .iter()
        .filter(|(s, _)| matches!(s, Slot::LegL | Slot::LegR))
        .map(|(_, m)| m.reach_m)
        .fold(0.0f32, f32::max);
    let torso = out
        .iter()
        .find(|(s, _)| *s == Slot::Torso)
        .map(|(_, m)| m.reach_m)
        .unwrap_or(0.0);
    Some(Gestalt { members: out, coherence, symmetry, total_mass, reach_m: leg + torso })
}

/// Can this squad merge at all?
pub fn viable(g: &Gestalt) -> bool {
    g.coherence >= MIN_COHERENCE && g.symmetry >= MIN_SYMMETRY
}

/// How long the combination takes, from the actuator model rather than a
/// typed duration. A gestalt is slow because it is moving more metal, which
/// is the same reason a heavy chassis folds slowly.
pub fn combine_time_s(g: &Gestalt, power_per_member: f32) -> f32 {
    let inertia = g.total_mass * (g.reach_m * 0.5).powi(2);
    let act = crate::actuator::fold_joint(power_per_member * g.members.len() as f32, inertia);
    crate::actuator::fold_time(&act, 2.2, inertia, 0.0, 1.0 / 120.0, 60.0).unwrap_or(60.0)
}

#[cfg(test)]
mod driver_tests {
    use super::*;

    #[test]
    fn the_examples_demonstrate_each_rule_exactly_once() {
        // A driver whose examples all fail the same way teaches one rule and
        // implies the other is decoration. `MIN_SYMMETRY` went undemonstrated
        // until this test existed: the "lopsided" squad failed on COHERENCE,
        // because a tank makes a bad arm before it makes a mismatched one.
        let ex = examples();
        assert_eq!(ex.len(), 3);

        let g = assign(&ex[0].1).expect("the built-for-it squad assigns");
        assert!(viable(&g), "coherence {} symmetry {}", g.coherence, g.symmetry);

        let g = assign(&ex[1].1).expect("identical racers still assign");
        assert!(!viable(&g));
        assert!(g.coherence < MIN_COHERENCE, "should fail on shape: {}", g.coherence);
        assert!(g.symmetry >= MIN_SYMMETRY, "identical bodies are perfectly symmetric");

        let g = assign(&ex[2].1).expect("the mismatched-arm squad assigns");
        assert!(!viable(&g));
        assert!(
            g.coherence >= MIN_COHERENCE,
            "every body should suit its role here: {}",
            g.coherence
        );
        assert!(g.symmetry < MIN_SYMMETRY, "should fail on symmetry: {}", g.symmetry);
    }

    #[test]
    fn a_gestalt_is_a_rung_on_the_size_ladder() {
        // the connection that made this wireable: a combined body has a
        // HEIGHT, and since `scale.rs` exists that height means something —
        // it frames like any other body rather than needing a multiplier
        // somebody typed.
        let g = assign(&examples()[0].1).unwrap();
        let bot = crate::scale::sized("bot").unwrap();
        let h = g.scale_vs(bot.height_m) * bot.height_m;
        assert!(h > bot.height_m, "a gestalt should be taller than its members");
        let (d, _) = crate::scale::camera_for(crate::scale::Class::Bot, h);
        let (bd, _) = crate::scale::camera_for(crate::scale::Class::Bot, bot.height_m);
        assert!(d > bd, "and should be framed further out");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn m(name: &'static str, elong: f32, bulk: f32, reach: f32, mass: f32) -> Member {
        Member { name, elongation: elong, bulk, reach_m: reach, mass }
    }

    /// a squad that should work: one bulky, two long light, two long heavy
    fn good() -> Vec<Member> {
        vec![
            m("HULK", 0.20, 0.98, 5.0, 4_200.0),
            m("REACH-L", 0.88, 0.28, 4.4, 1_500.0),
            m("REACH-R", 0.86, 0.31, 4.3, 1_550.0),
            m("STRIDE-L", 0.74, 0.78, 5.2, 3_100.0),
            m("STRIDE-R", 0.76, 0.72, 5.1, 3_050.0),
        ]
    }

    #[test]
    fn a_body_gets_the_role_its_proportions_suit() {
        let g = assign(&good()).unwrap();
        let by = |n: &str| g.members.iter().find(|(_, m)| m.name == n).unwrap().0;
        assert_eq!(by("HULK"), Slot::Torso, "the bulky one should be the torso");
        assert!(matches!(by("REACH-L"), Slot::ArmL | Slot::ArmR));
        assert!(matches!(by("STRIDE-L"), Slot::LegL | Slot::LegR));
        assert!(viable(&g), "coherence {} symmetry {}", g.coherence, g.symmetry);
    }

    #[test]
    fn five_identical_racers_cannot_combine() {
        // the interesting answer, and the reason this is a design rather
        // than a gimmick: a squad discovers whether it can merge from what
        // its members ARE
        let same = vec![m("RACER", 0.55, 0.5, 4.0, 1_200.0); 5];
        let g = assign(&same).unwrap();
        assert!(!viable(&g), "coherence {} should be under {MIN_COHERENCE}", g.coherence);
    }

    #[test]
    fn a_lopsided_pair_is_refused_even_when_every_body_fits_its_role() {
        // one long arm and one stubby one reads wrong however well each
        // suits "arm" on its own
        let mut sq = good();
        sq[2].reach_m = 1.4; // the right arm is a third of the left
        let g = assign(&sq).unwrap();
        assert!(g.coherence >= MIN_COHERENCE, "the roles still fit: {}", g.coherence);
        assert!(g.symmetry < MIN_SYMMETRY, "symmetry {}", g.symmetry);
        assert!(!viable(&g));
    }

    #[test]
    fn a_gestalt_is_boss_scale_by_construction() {
        // rather than by a multiplier someone typed
        let g = assign(&good()).unwrap();
        assert!(g.reach_m > 9.0, "leg plus torso: {}", g.reach_m);
        assert!(g.scale_vs(4.0) > 2.4, "{}", g.scale_vs(4.0));
        assert!(g.total_mass > 13_000.0);
    }

    #[test]
    fn it_folds_slowly_because_it_is_moving_more_metal() {
        // the actuator model, not a typed duration
        let g = assign(&good()).unwrap();
        let t = combine_time_s(&g, 9_000.0);
        assert!(t > 0.5, "a gestalt should not snap together: {t}");
        // and a lighter squad merges faster on the same actuators
        let mut light = good();
        for x in light.iter_mut() {
            x.mass *= 0.25;
        }
        let lg = assign(&light).unwrap();
        assert!(combine_time_s(&lg, 9_000.0) < t, "less metal should be quicker");
    }

    #[test]
    fn the_wrong_number_of_members_is_refused_rather_than_padded() {
        assert!(assign(&good()[..4]).is_none());
        assert!(assign(&[]).is_none());
        let mut six = good();
        six.push(m("SPARE", 0.5, 0.5, 4.0, 1_000.0));
        assert!(assign(&six).is_none());
    }

    #[test]
    fn every_role_is_filled_exactly_once() {
        let g = assign(&good()).unwrap();
        for s in SLOTS {
            assert_eq!(g.members.iter().filter(|(x, _)| *x == s).count(), 1, "{}", s.label());
        }
    }
}

// ------------------------------------------------------------- driver ---

/// `--combine` — does this squad merge, and how big is what comes out?
///
/// The module shipped with none of this reachable: `assign`, `viable`,
/// `combine_time_s` and `scale_vs` existed, were tested, and were called by
/// nothing. `curriculum.rs` has been listing `combine` as `built: false`
/// beside `clinch` ever since the ledger landed, which was accurate and did
/// not fix it.
///
/// It is worth a driver rather than a shrug because the interesting claim is
/// checkable without a screen: **not every squad can combine.** A squad of
/// five identical racers should fail, and it should fail for a legible
/// reason rather than a score. Printing the fitness grid shows why a body got
/// a role, which is the whole argument for the greedy assignment.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--combine").unwrap_or(0);
    let which = argv.get(i + 1).map(String::as_str).unwrap_or("squads");
    match which {
        "squads" => squads(),
        "grid" => grid(),
        other => {
            println!("unknown: --combine {other}");
            println!("try: squads | grid");
        }
    }
}

/// A mixed squad that should work, and a uniform one that should not.
fn examples() -> Vec<(&'static str, Vec<Member>)> {
    let m = |name, elongation, bulk, reach_m, mass| Member {
        name,
        elongation,
        bulk,
        reach_m,
        mass,
    };
    vec![
        (
            "a squad built for it",
            vec![
                m("bulwark", 0.20, 0.95, 6.0, 14_000.0),
                m("lance", 0.90, 0.28, 5.2, 6_000.0),
                m("pike", 0.88, 0.32, 5.1, 6_400.0),
                m("stride", 0.74, 0.72, 5.8, 9_000.0),
                m("stanchion", 0.76, 0.78, 5.9, 9_400.0),
            ],
        ),
        (
            "five identical racers",
            vec![
                m("racer i", 0.70, 0.25, 4.4, 3_000.0),
                m("racer ii", 0.70, 0.25, 4.4, 3_000.0),
                m("racer iii", 0.70, 0.25, 4.4, 3_000.0),
                m("racer iv", 0.70, 0.25, 4.4, 3_000.0),
                m("racer v", 0.70, 0.25, 4.4, 3_000.0),
            ],
        ),
        (
            // Two GOOD arms of wildly different length. Every body is shaped
            // for its role, so coherence passes; the thing that stops it is
            // that one arm is a third the length of the other, which is what
            // `symmetry` measures and nothing else in the model catches.
            //
            // The first version of this example was "one arm is a tank",
            // which failed on COHERENCE instead — a tank is a bad arm — so
            // the symmetry rule was never the deciding factor in any example
            // the driver printed, and a reader would have had no idea what it
            // was for.
            "mismatched arms: both good, one enormous",
            vec![
                m("bulwark", 0.20, 0.95, 6.0, 14_000.0),
                m("lance", 0.90, 0.28, 2.0, 3_000.0),
                m("greatlance", 0.90, 0.28, 8.0, 9_000.0),
                m("stride", 0.74, 0.72, 5.8, 9_000.0),
                m("stanchion", 0.76, 0.78, 5.9, 9_400.0),
            ],
        ),
    ]
}

fn squads() {
    println!("COMBINATION — not every squad can\n");
    for (label, members) in examples() {
        println!("  {label}");
        match assign(&members) {
            None => println!("      will not assign at all\n"),
            Some(g) => {
                for (slot, m) in &g.members {
                    println!("      {:<12} {}", slot.label(), m.name);
                }
                println!(
                    "      coherence {:.2} (needs {MIN_COHERENCE:.2}), symmetry {:.2} (needs {MIN_SYMMETRY:.2})",
                    g.coherence, g.symmetry
                );
                if viable(&g) {
                    // the scale connection: a gestalt is a new rung on the
                    // ladder, and `scale.rs` frames it like any other body
                    let bot = crate::scale::sized("bot").unwrap();
                    let h = g.scale_vs(bot.height_m) * bot.height_m;
                    let (d, up) = crate::scale::camera_for(crate::scale::Class::Bot, h);
                    println!(
                        "      COMBINES: {:.1} m, {:.0} t, {:.0} s to fold",
                        h,
                        g.total_mass / 1000.0,
                        combine_time_s(&g, 1.0)
                    );
                    println!(
                        "      camera {d:.0} m back, {up:.0} m up ({:.1}x a standard frame)",
                        g.scale_vs(bot.height_m)
                    );
                } else {
                    let why = if g.coherence < MIN_COHERENCE {
                        "the bodies are not shaped for the roles"
                    } else {
                        "every body suits its role; the mirrored pair does not match"
                    };
                    println!("      will not combine: {why}");
                }
                println!();
            }
        }
    }
    println!("  a squad of five identical bodies cannot merge, which is the");
    println!("  interesting answer: roles want proportions, and proportions are");
    println!("  measured off the models rather than assigned in a table.");
}

fn grid() {
    println!("FITNESS — why a body gets a role\n");
    let (_, members) = examples().into_iter().next().unwrap();
    print!("  {:<12}", "");
    for s in SLOTS {
        print!(" {:>10}", s.label());
    }
    println!();
    for m in &members {
        print!("  {:<12}", m.name);
        for s in SLOTS {
            print!(" {:>10.2}", fitness(m, s));
        }
        println!();
    }
    println!("\n  the assignment is greedy on the strongest pairing, so the grid");
    println!("  explains the result. A Hungarian solve would be marginally better");
    println!("  and would not tell you why.");
}

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

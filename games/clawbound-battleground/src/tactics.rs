//! tactics -- when a bot commits to a fight, how far it chases, when it
//! lets go.
//!
//! The old brain had one fighting state: orbit the target at attack range,
//! strafing. Two cats of equal health circled each other until one crossed
//! 30% and ran home; a cat that was ahead chased its target under a tower.
//! The strafing (the sidestep that makes bots hard to hit) was good and is
//! kept; what changes is the decision around it.
//!
//! A bot reads its fight as numbers ([`Read`]) and picks a [`Stance`]:
//!
//! * **Dive** -- commit: close to melee, sidestep less, spend the engage
//!   abilities (dash, blink, pull). Taken when the local fight is won on
//!   paper (`edge >= commit`), and HELD for a few seconds even as the edge
//!   wobbles, so a dive is not abandoned at the first bite.
//! * **Hold** -- the even fight: keep at range (`reach` x the cat's ideal),
//!   sidestep, trade. Patience lowers the commit bar the longer it lasts,
//!   so an even fight resolves instead of circling forever.
//! * **Regroup** -- behind on paper, or the chase ran past the leash, or
//!   the target sits under an enemy tower: back off toward the allies (or
//!   the anchor the fight started at), still sidestepping, still poking.
//!   Sticky for a moment so it does not flip straight back.
//! * **Retreat** -- low health: go home. A kill in reach overrides it.
//!
//! The edge is Lanchester's square law in miniature: a side's fighting
//! strength is (total health) x (total damage), so the ratio of two sides
//! is the square root of the ratio of those products. Enemy towers count
//! on the other side, scaled by how much this style respects them.
//!
//! How a team leans on these thresholds is its [`Genes`] -- one page of
//! the playbook (`playbook`) -- and the numbers are what the arena
//! (`arena`) mutates. Headless and pure: [`decide`] is tested without a
//! game.

use crate::sim::Rng;

/// What a bot is doing in a fight.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Default)]
pub enum Stance {
    Dive,
    #[default]
    Hold,
    Regroup,
    Retreat,
}

impl Stance {
    pub fn label(self) -> &'static str {
        match self {
            Stance::Dive => "dive",
            Stance::Hold => "hold",
            Stance::Regroup => "regroup",
            Stance::Retreat => "retreat",
        }
    }
}

/// One team's temperament: the numbers a playbook page sets and the arena
/// evolves. Every field is a plain f32 inside [`RANGES`] so a mutation is
/// a nudge and a clamp.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Genes {
    /// Edge needed to dive.
    pub commit: f32,
    /// Edge below which it regroups.
    pub give: f32,
    /// Health fraction that sends it home.
    pub retreat: f32,
    /// How far (u) it chases from where the fight started.
    pub leash: f32,
    /// Weight of an enemy tower on the other side of the scale.
    pub tower: f32,
    /// Sidestep weight while fighting (the dodging).
    pub strafe: f32,
    /// Preferred fighting distance, x the cat's ideal range.
    pub reach: f32,
    /// 1 = hunt the weakest target, 0 = the nearest.
    pub focus: f32,
    /// How much the team stays together (lanes, regroup pull).
    pub group: f32,
    /// How readily it joins a teammate's dive.
    pub rally: f32,
    /// Pull of bosses and structures over fights.
    pub objective: f32,
    /// Seconds a dive is held through a wobbling edge.
    pub hold: f32,
}

pub const GENE_COUNT: usize = 12;

/// (min, max) of each gene, in [`Genes::to_array`] order.
pub const RANGES: [(f32, f32); GENE_COUNT] = [
    (0.8, 1.9),   // commit
    (0.35, 0.95), // give
    (0.10, 0.45), // retreat
    (6.0, 32.0),  // leash
    (0.2, 1.5),   // tower
    (0.2, 1.3),   // strafe
    (0.5, 1.6),   // reach
    (0.0, 1.0),   // focus
    (0.0, 1.0),   // group
    (0.0, 1.0),   // rally
    (0.0, 1.0),   // objective
    (0.5, 4.0),   // hold
];

pub const GENE_NAMES: [&str; GENE_COUNT] =
    ["commit", "give", "retreat", "leash", "tower", "strafe", "reach", "focus", "group", "rally", "objective", "hold"];

impl Default for Genes {
    /// The middle of the road: roughly what the old brain did, minus the
    /// endless circling.
    fn default() -> Genes {
        Genes {
            commit: 1.25,
            give: 0.7,
            retreat: 0.26,
            leash: 16.0,
            tower: 1.0,
            strafe: 0.8,
            reach: 1.0,
            focus: 0.5,
            group: 0.4,
            rally: 0.5,
            objective: 0.5,
            hold: 2.0,
        }
    }
}

impl Genes {
    pub fn to_array(&self) -> [f32; GENE_COUNT] {
        [
            self.commit, self.give, self.retreat, self.leash, self.tower, self.strafe, self.reach, self.focus, self.group,
            self.rally, self.objective, self.hold,
        ]
    }

    /// From an array, each value clamped into its range.
    pub fn from_array(a: [f32; GENE_COUNT]) -> Genes {
        let c = |i: usize| a[i].clamp(RANGES[i].0, RANGES[i].1);
        let mut g = Genes {
            commit: c(0),
            give: c(1),
            retreat: c(2),
            leash: c(3),
            tower: c(4),
            strafe: c(5),
            reach: c(6),
            focus: c(7),
            group: c(8),
            rally: c(9),
            objective: c(10),
            hold: c(11),
        };
        // giving up must stay below committing, or the bot flips every tick
        g.give = g.give.min(g.commit - 0.2);
        g
    }

    /// A nudge of up to `amount` of each gene's range on about a third of
    /// the genes (at least one).
    pub fn mutate(&self, rng: &mut Rng, amount: f32) -> Genes {
        let mut a = self.to_array();
        let forced = (rng.next_u() % GENE_COUNT as u64) as usize;
        for (i, v) in a.iter_mut().enumerate() {
            if i == forced || rng.f32() < 0.33 {
                let (lo, hi) = RANGES[i];
                *v += (rng.f32() * 2.0 - 1.0) * amount * (hi - lo);
            }
        }
        Genes::from_array(a)
    }

    /// Distance between two temperaments, each gene normalised to its range.
    pub fn distance(&self, o: &Genes) -> f32 {
        let (a, b) = (self.to_array(), o.to_array());
        (0..GENE_COUNT).map(|i| ((a[i] - b[i]) / (RANGES[i].1 - RANGES[i].0)).powi(2)).sum::<f32>().sqrt()
    }
}

/// A bot's reading of its fight.
#[derive(Clone, Copy, Debug)]
pub struct Read {
    /// Own health fraction.
    pub my_hp: f32,
    /// Our side near the fight: (total health, total damage).
    pub ours: (f32, f32),
    /// Their side (gods and minions near the fight).
    pub theirs: (f32, f32),
    /// An enemy tower over the fight: its (health, damage) equivalent, 0
    /// when none or when our minions are tanking it.
    pub tower: (f32, f32),
    /// The target's health fraction.
    pub target_hp: f32,
    /// Distance to the target, and the cat's ideal fighting distance.
    pub target_dist: f32,
    pub ideal: f32,
    /// Distance from where this fight started.
    pub from_anchor: f32,
    /// Fraction of the damaging abilities that are ready.
    pub ready: f32,
    /// A teammate dived on this target moments ago.
    pub called: bool,
    /// Current stance and how long it has lasted.
    pub stance: Stance,
    pub stance_t: f32,
}

impl Read {
    /// The Lanchester edge (>1 = ahead), with the tower weighed by `tower`.
    pub fn edge(&self, g: &Genes) -> f32 {
        let ours = self.ours.0.max(1.0) * self.ours.1.max(0.1);
        let th = (self.theirs.0 + self.tower.0 * g.tower).max(1.0) * (self.theirs.1 + self.tower.1 * g.tower).max(0.1);
        (ours / th).sqrt()
    }
}

/// Seconds of an even Hold after which the commit bar has dropped to 1.
pub const PATIENCE: f32 = 6.0;
/// A Regroup lasts at least this long unless the edge turns decisively.
pub const REGROUP_MIN: f32 = 1.2;

/// The stance for this reading. Pure.
pub fn decide(r: &Read, g: &Genes) -> Stance {
    let mut edge = r.edge(g);
    if r.called {
        edge += 0.35 * g.rally;
    }
    // a low target is worth more than its health says: it is about to stop
    // hitting back
    edge += (0.35 - r.target_hp).max(0.0);
    let kill = r.target_hp < 0.22 && r.target_dist < r.ideal * 1.8 + 2.0 && r.my_hp > g.retreat * 0.45;

    // 1. home -- unless the kill is right there
    if r.my_hp < g.retreat && !kill {
        return Stance::Retreat;
    }

    // 2. the leash: chased too far from where the fight began
    let leashed = r.from_anchor > g.leash && r.target_dist > r.ideal + 2.0 && !kill;
    if leashed {
        return Stance::Regroup;
    }

    // 3. patience: the longer an even fight holds, the lower the bar
    let bar = if r.stance == Stance::Hold {
        let p = (r.stance_t / PATIENCE).min(1.0);
        g.commit + (1.0f32.min(g.commit) - g.commit) * p
    } else {
        g.commit
    };

    // 4. a dive is held through a wobble
    if r.stance == Stance::Dive && r.stance_t < g.hold && edge > bar * 0.7 {
        return Stance::Dive;
    }
    // 5. regroup is sticky too
    if r.stance == Stance::Regroup && r.stance_t < REGROUP_MIN && edge < bar * 1.15 {
        return Stance::Regroup;
    }

    if edge >= bar && (r.ready >= 0.34 || kill || r.stance == Stance::Dive) {
        Stance::Dive
    } else if kill {
        Stance::Dive
    } else if edge < g.give {
        Stance::Regroup
    } else {
        Stance::Hold
    }
}

/// Where a stance wants to stand relative to the target, x `ideal`.
pub fn distance_for(stance: Stance, g: &Genes) -> f32 {
    match stance {
        Stance::Dive => 0.55,
        Stance::Hold => g.reach,
        Stance::Regroup => g.reach.max(1.0) * 1.35,
        Stance::Retreat => 3.0,
    }
}

/// Sidestep weight for a stance: a diving cat still weaves, less.
pub fn strafe_for(stance: Stance, g: &Genes) -> f32 {
    match stance {
        Stance::Dive => g.strafe * 0.35,
        Stance::Hold => g.strafe,
        Stance::Regroup => g.strafe * 0.8,
        Stance::Retreat => 0.0,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn even() -> Read {
        Read {
            my_hp: 0.8,
            ours: (1000.0, 100.0),
            theirs: (1000.0, 100.0),
            tower: (0.0, 0.0),
            target_hp: 0.8,
            target_dist: 5.0,
            ideal: 5.0,
            from_anchor: 0.0,
            ready: 1.0,
            called: false,
            stance: Stance::Hold,
            stance_t: 0.0,
        }
    }

    #[test]
    fn the_edge_is_lanchester() {
        let g = Genes::default();
        let mut r = even();
        assert!((r.edge(&g) - 1.0).abs() < 1e-4);
        // two cats against one of the same: health and damage both double
        r.ours = (2000.0, 200.0);
        assert!((r.edge(&g) - 2.0).abs() < 1e-3, "{}", r.edge(&g));
        // a tower counts against us, and a style that respects towers less
        // counts it less
        r.tower = (1000.0, 100.0);
        let bold = Genes { tower: 0.3, ..g };
        assert!(r.edge(&bold) > r.edge(&g) + 0.2);
    }

    #[test]
    fn ahead_dives_behind_regroups_even_holds() {
        let g = Genes::default();
        let mut r = even();
        assert_eq!(decide(&r, &g), Stance::Hold);
        r.ours = (2000.0, 200.0);
        assert_eq!(decide(&r, &g), Stance::Dive);
        r.ours = (500.0, 50.0);
        assert_eq!(decide(&r, &g), Stance::Regroup);
    }

    /// The circle-chasing fix: an even Hold does not last forever.
    #[test]
    fn an_even_fight_resolves_with_patience() {
        let g = Genes::default();
        let mut r = even();
        r.ours = (1080.0, 108.0); // a hair ahead: below the commit bar
        assert_eq!(decide(&r, &g), Stance::Hold);
        r.stance_t = PATIENCE;
        assert_eq!(decide(&r, &g), Stance::Dive, "after {PATIENCE}s of trading it commits");
        // but not when behind: patience lowers the bar to 1, no further
        r.ours = (900.0, 90.0);
        assert_ne!(decide(&r, &g), Stance::Dive);
    }

    /// The going-too-deep fix: the leash and the tower.
    #[test]
    fn a_chase_stops_at_the_leash_and_the_tower() {
        let g = Genes::default();
        let mut r = even();
        r.ours = (2000.0, 200.0);
        r.target_dist = 12.0;
        r.from_anchor = g.leash + 1.0;
        assert_eq!(decide(&r, &g), Stance::Regroup, "past the leash, a fleeing target is let go");
        r.target_hp = 0.1;
        r.target_dist = 6.0;
        assert_eq!(decide(&r, &g), Stance::Dive, "unless the kill is right there");
        let mut r = even();
        r.ours = (1600.0, 160.0);
        assert_eq!(decide(&r, &g), Stance::Dive);
        r.tower = (2500.0, 250.0);
        assert_eq!(decide(&r, &g), Stance::Regroup, "ahead in cats, behind with the tower");
    }

    /// The fleeing fix: a dive is held through a wobble, and low health
    /// only sends a cat home when there is no kill in reach.
    #[test]
    fn a_dive_is_held_and_a_kill_beats_going_home() {
        let g = Genes::default();
        let mut r = even();
        r.stance = Stance::Dive;
        r.stance_t = 0.5;
        r.ours = (950.0, 95.0); // edge fell below the bar but above 70% of it
        assert_eq!(decide(&r, &g), Stance::Dive);
        r.stance_t = g.hold + 0.1;
        assert_ne!(decide(&r, &g), Stance::Dive, "held for `hold` seconds, not forever");
        let mut r = even();
        r.my_hp = g.retreat - 0.05;
        assert_eq!(decide(&r, &g), Stance::Retreat);
        r.target_hp = 0.1;
        assert_eq!(decide(&r, &g), Stance::Dive, "the kill is worth the risk");
        r.my_hp = 0.05;
        assert_eq!(decide(&r, &g), Stance::Retreat, "but not at death's door");
    }

    #[test]
    fn a_teammates_call_tips_an_even_fight() {
        let g = Genes { rally: 1.0, ..Genes::default() };
        let mut r = even();
        r.ours = (1100.0, 110.0);
        assert_eq!(decide(&r, &g), Stance::Hold);
        r.called = true;
        assert_eq!(decide(&r, &g), Stance::Dive);
        let loner = Genes { rally: 0.0, ..g };
        assert_eq!(decide(&r, &loner), Stance::Hold);
    }

    #[test]
    fn genes_stay_in_range_and_give_stays_below_commit() {
        let mut rng = Rng::new(9);
        let mut g = Genes::default();
        let mut moved = 0;
        for _ in 0..500 {
            let n = g.mutate(&mut rng, 0.4);
            if n != g {
                moved += 1;
            }
            g = n;
            let a = g.to_array();
            for i in 0..GENE_COUNT {
                assert!(a[i] >= RANGES[i].0 - 1e-6 && a[i] <= RANGES[i].1 + 1e-6, "{} = {}", GENE_NAMES[i], a[i]);
            }
            assert!(g.give <= g.commit - 0.2 + 1e-6);
        }
        assert!(moved > 450, "a mutation changes something: {moved}");
    }
}

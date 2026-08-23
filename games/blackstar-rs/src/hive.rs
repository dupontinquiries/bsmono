//! THE HIVE — a swarm that decides by proximity rather than by orders.
//!
//! Ported from the C engine's `hive_ai.c`. It shares the command vocabulary
//! with `brain::Fsm` and sits behind the same `Brain` trait, so the game
//! cannot tell which is issuing orders — but it reaches those commands by a
//! completely different policy, and the C header is emphatic that this is the
//! point:
//!
//! > Deliberately DIFFERENT IN KIND from commander_ai's military doctrine
//! > FSMs. A commander issues open, top-down squad orders. The hive does the
//! > opposite: it is patient and indirect.
//!
//! `corruption.rs` in this tree models what corruption *is*. It has never
//! modelled how a corrupted swarm *decides*, and that gap is why the infected
//! have been fighting like soldiers with different stats.
//!
//! ## Crawl, dance, feed
//!
//! Three states, borrowed from bees rather than from armies.
//!
//! **CRAWL** — below critical mass the infected stay dispersed and creep,
//! massing quietly instead of charging, so a lone scout does not give the
//! swarm away.
//!
//! **DANCE** — an infected that gets close to prey emits a waggle dance that
//! RECRUITS nearby infected toward that point. Excitement spreads through
//! proximity — stigmergic, not a broadcast order. Nothing commands anything;
//! the swarm self-organises because each individual reacts to what is near it.
//!
//! **FEED** — once enough have been recruited, the hive tips and converges.
//!
//! The interesting consequence is that **killing the scout works**. There is
//! no commander to decapitate, but there is a dance to interrupt, and if the
//! prey slips outside the dance radius the whole thing subsides back to
//! crawling. A player who learns that has learned something the enemy's
//! statistics never told them.
//!
//! ## Patience is the strain's whole personality
//!
//! The gate is `dance_radius * (1 - 0.4 * patience)`, so a patient strain will
//! not dance until a scout is well inside range — it refuses to tip its hand.
//! An impatient one dances from the edge and gives itself away. Same skeleton,
//! four temperaments.

use crate::brain::{Brain, Command, Orders, Snapshot, Verb, ZONE_ALL};
use glam::Vec3;

/// The collective posture.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hive {
    /// dispersed, creeping, massing quietly
    Crawl,
    /// a scout found prey and is recruiting
    Dance,
    /// critical mass; converge
    Feed,
}

/// Distinct sneaky styles. All share crawl/dance/feed and differ in how they
/// lurk and swarm — the same relationship archetypes have to commanders.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Strain {
    /// patient ambusher: lurks long, tight pack, sudden feed
    Stalker,
    /// slow encircler: spreads wide, wants a big mass before committing
    Creeper,
    /// a few dance early and from range to draw prey in, then the pack tips
    Lure,
    /// impatient: short lurk, small mass, barely sneaky — they cannot help it
    Ravener,
}

/// What a strain wants.
#[derive(Clone, Copy, Debug)]
pub struct Doctrine {
    /// recruited count that tips crawl/dance into feed
    pub feed_mass: usize,
    /// how close a scout must be to prey to start dancing
    pub dance_radius: f32,
    /// how near an infected must be to a dance to be recruited
    pub recruit_radius: f32,
    /// 0..1 — higher crawls longer before daring to dance
    pub patience: f32,
}

impl Strain {
    pub fn all() -> [Strain; 4] {
        [Strain::Stalker, Strain::Creeper, Strain::Lure, Strain::Ravener]
    }
    pub fn name(self) -> &'static str {
        match self {
            Strain::Stalker => "stalker",
            Strain::Creeper => "creeper",
            Strain::Lure => "lure",
            Strain::Ravener => "ravener",
        }
    }
    pub fn parse(s: &str) -> Option<Strain> {
        Strain::all().into_iter().find(|x| x.name() == s)
    }
    pub fn doctrine(self) -> Doctrine {
        match self {
            // lurks long, so it only dances when a scout is nearly on top of
            // prey; compact pack, sudden feed
            Strain::Stalker => {
                Doctrine { feed_mass: 4, dance_radius: 20.0, recruit_radius: 11.0, patience: 0.85 }
            }
            // dances from farther out and waits for a BIG mass spread wide.
            // Surrounds before it feeds.
            Strain::Creeper => {
                Doctrine { feed_mass: 6, dance_radius: 30.0, recruit_radius: 24.0, patience: 0.25 }
            }
            // a few dance early and from range to draw prey toward the pack;
            // small feed mass, so once the prey is close the rest tip fast
            Strain::Lure => {
                Doctrine { feed_mass: 3, dance_radius: 34.0, recruit_radius: 18.0, patience: 0.15 }
            }
            Strain::Ravener => {
                Doctrine { feed_mass: 2, dance_radius: 24.0, recruit_radius: 14.0, patience: 0.10 }
            }
        }
    }
}

/// How often the hive re-considers. Faster than a commander's 1.25 s: the
/// swarm has no deliberation to do, only a threshold to notice.
pub const THINK_INTERVAL_S: f32 = 0.5;

/// Distance is measured on the ground plane. Height differences between an
/// infected and its prey are terrain, not intent, and counting them would let
/// a swarm below a ledge decide it was far away.
fn ground_dist(a: Vec3, b: Vec3) -> f32 {
    Vec3::new(a.x - b.x, 0.0, a.z - b.z).length()
}

pub struct Swarm {
    pub strain: Strain,
    pub doctrine: Doctrine,
    pub state: Hive,
    pub recruited: usize,
    pub dance_point: Vec3,
    think_cd: f32,
    pub state_t: f32,
}

impl Swarm {
    pub fn new(strain: Strain) -> Swarm {
        Swarm {
            strain,
            doctrine: strain.doctrine(),
            state: Hive::Crawl,
            recruited: 0,
            dance_point: Vec3::ZERO,
            think_cd: 0.0,
            state_t: 0.0,
        }
    }

    /// The nearest living infected to the prey, and how many there are.
    ///
    /// Every live unit counts as infected here. The C filtered on
    /// `kind == EX_ENEMY_CORRUPTED`; this tree has no kind on a snapshot unit,
    /// so a `Swarm` is given a snapshot that already contains only its own —
    /// stated because the alternative is a swarm quietly recruiting the
    /// player's allies.
    fn scout<'a>(&self, s: &'a Snapshot) -> Option<(&'a crate::brain::Unit, f32, usize)> {
        let n = s.live().count();
        s.live()
            .map(|u| (u, ground_dist(u.pos, s.player_pos)))
            .min_by(|a, b| a.1.partial_cmp(&b.1).unwrap())
            .map(|(u, d)| (u, d, n))
    }

    fn count_near(&self, s: &Snapshot, at: Vec3) -> usize {
        s.live().filter(|u| ground_dist(u.pos, at) <= self.doctrine.recruit_radius).count()
    }

    /// The distance at which this strain will risk dancing.
    ///
    /// Patience shrinks it: a patient hive holds until a scout is well inside
    /// range rather than tipping its hand at the edge.
    pub fn dance_gate(&self) -> f32 {
        self.doctrine.dance_radius * (1.0 - 0.4 * self.doctrine.patience)
    }

    fn advance(&mut self, s: &Snapshot) {
        let Some((scout, scout_d, n)) = self.scout(s) else {
            self.state = Hive::Crawl;
            self.recruited = 0;
            return;
        };
        // SEQUENTIAL, NOT EXCLUSIVE. The C uses three separate `if`s rather
        // than `else if`, so the tick a scout gets close the hive enters
        // DANCE and immediately falls through to count recruits — and if the
        // pack is already gathered, tips to FEED in that same tick. A `match`
        // here looked equivalent and was not: it cost a tick per transition,
        // so a swarm the player walked into took a second to notice. Caught
        // by the recruitment test reading zero on both sides of a comparison.
        if self.state == Hive::Crawl {
            if scout_d <= self.dance_gate() {
                self.state = Hive::Dance;
                self.state_t = 0.0;
                self.dance_point = scout.pos;
            }
        }
        match self.state {
            Hive::Crawl => {}
            Hive::Dance => {
                // the dance follows the scout; recruits accumulate near it
                self.dance_point = scout.pos;
                self.recruited = self.count_near(s, self.dance_point);
                if self.recruited >= self.doctrine.feed_mass {
                    self.state = Hive::Feed;
                    self.state_t = 0.0;
                } else if scout_d > self.doctrine.dance_radius * 1.5 {
                    // the prey slipped away; subside rather than chase
                    self.state = Hive::Crawl;
                    self.state_t = 0.0;
                    self.recruited = 0;
                }
            }
            Hive::Feed => {
                // thinned below mass, and it comes apart again
                if n < self.doctrine.feed_mass {
                    self.state = Hive::Crawl;
                    self.state_t = 0.0;
                    self.recruited = 0;
                }
            }
        }
    }

    fn order(&self, s: &Snapshot) -> Orders {
        let mut c = match self.state {
            // stay dispersed and creep: a loose hold, deliberately quiet
            Hive::Crawl => {
                let mut c = Command::new(Verb::Hold, ZONE_ALL, s.player_pos);
                c.param0 = self.doctrine.patience;
                c
            }
            // recruit toward the scout on the prey
            Hive::Dance => {
                let mut c = Command::new(Verb::FlockTo, ZONE_ALL, self.dance_point);
                c.param0 = 8.0;
                c.param1 = self.recruited as f32;
                c
            }
            // critical mass
            Hive::Feed => {
                let mut c = Command::new(Verb::Focus, ZONE_ALL, s.player_pos);
                c.param0 = 1.0;
                c
            }
        };
        c.point.y = 0.0;
        Orders { tick: s.tick, speaker: self.strain.name().to_string(), cmds: vec![c] }.sanitised()
    }
}

impl Brain for Swarm {
    fn think(&mut self, snap: &Snapshot, dt: f32) -> Option<Orders> {
        self.state_t += dt;
        self.think_cd -= dt;
        if self.think_cd > 0.0 {
            return None;
        }
        self.think_cd = THINK_INTERVAL_S;
        self.advance(snap);
        Some(self.order(snap))
    }
    fn who(&self) -> &str {
        self.strain.name()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::brain::{Signals, Unit};

    /// `n` infected in a clump `dist` from the player.
    fn swarm_at(dist: f32, n: usize, spread: f32) -> Snapshot {
        let units = (0..n)
            .map(|i| Unit {
                id: i as i32,
                pos: Vec3::new(i as f32 * spread, 0.0, dist),
                hp: 100.0,
                max_hp: 100.0,
                zone: 1,
                alive: true,
                is_commander: false,
                squad: -1,
                scattering: false,
            })
            .collect();
        Snapshot {
            tick: 1,
            player_pos: Vec3::ZERO,
            in_vehicle: false,
            signals: Signals { enemies_alive: n as i32, ..Default::default() },
            units,
            allies: Vec::new(),
        }
    }

    #[test]
    fn a_distant_swarm_crawls_and_says_nothing_loud() {
        let mut h = Swarm::new(Strain::Stalker);
        let o = h.think(&swarm_at(200.0, 8, 2.0), 99.0).unwrap();
        assert_eq!(h.state, Hive::Crawl);
        assert_eq!(o.cmds[0].verb, Verb::Hold, "a crawling hive should not charge");
    }

    #[test]
    fn dance_is_a_real_phase_when_the_pack_is_not_gathered_yet() {
        // A stalker wants four recruited within eleven metres. Spread the
        // infected out and a scout can reach the prey without the pack being
        // there — so the hive dances, and keeps dancing, waiting for numbers
        // it does not have. That waiting IS the mechanic.
        let mut h = Swarm::new(Strain::Stalker);
        for _ in 0..5 {
            h.think(&swarm_at(6.0, 5, 30.0), 99.0);
        }
        assert_eq!(h.state, Hive::Dance, "recruited {}", h.recruited);
        assert!(h.recruited < h.doctrine.feed_mass);

        // bring them together and it tips
        h.think(&swarm_at(6.0, 5, 2.0), 99.0);
        assert_eq!(h.state, Hive::Feed);
    }

    #[test]
    fn a_gathered_pack_tips_the_instant_a_scout_arrives() {
        // and the other half: the C's state checks are SEQUENTIAL, so a hive
        // that is already massed goes crawl -> dance -> feed within a single
        // tick rather than taking three. A swarm the player walks into does
        // not need a second to notice them.
        let mut h = Swarm::new(Strain::Ravener);
        h.think(&swarm_at(200.0, 6, 2.0), 99.0);
        assert_eq!(h.state, Hive::Crawl);
        h.think(&swarm_at(6.0, 6, 2.0), 99.0);
        assert_eq!(h.state, Hive::Feed, "one tick from crawling to feeding");
    }

    #[test]
    fn patience_is_the_difference_between_the_strains() {
        // the gate shrinks with patience: a patient hive refuses to tip its
        // hand at the edge of its own dance radius
        let stalker = Swarm::new(Strain::Stalker);
        let lure = Swarm::new(Strain::Lure);
        assert!(
            stalker.dance_gate() < lure.dance_gate(),
            "the patient strain should hold longer: {} vs {}",
            stalker.dance_gate(),
            lure.dance_gate()
        );
        // and each strain has its own answer at the same distance
        let s = swarm_at(22.0, 6, 2.0);
        let mut states = std::collections::BTreeSet::new();
        for st in Strain::all() {
            let mut h = Swarm::new(st);
            h.think(&s, 99.0);
            states.insert(format!("{:?}", h.state));
        }
        assert!(states.len() >= 2, "every strain behaved identically: {states:?}");
    }

    #[test]
    fn the_dance_subsides_when_the_prey_gets_away() {
        // there is no commander to kill, but there IS a dance to interrupt —
        // and a player who notices that has learned something real
        let mut h = Swarm::new(Strain::Stalker);
        h.think(&swarm_at(8.0, 3, 2.0), 99.0);
        assert_eq!(h.state, Hive::Dance, "should be recruiting");
        h.think(&swarm_at(400.0, 3, 2.0), 99.0);
        assert_eq!(h.state, Hive::Crawl, "the hive should subside, not chase");
        assert_eq!(h.recruited, 0);
    }

    #[test]
    fn recruitment_is_by_proximity_not_by_headcount() {
        // stigmergy: the same number of infected, spread out, does not tip.
        // Nothing is broadcasting — each one reacts to what is near it.
        let mut tight = Swarm::new(Strain::Stalker);
        tight.think(&swarm_at(8.0, 5, 2.0), 99.0);
        let mut spread = Swarm::new(Strain::Stalker);
        spread.think(&swarm_at(8.0, 5, 40.0), 99.0);
        assert!(
            tight.recruited > spread.recruited,
            "a clump should recruit more than a line: {} vs {}",
            tight.recruited,
            spread.recruited
        );
    }

    #[test]
    fn a_swarm_thinned_below_mass_comes_apart() {
        let mut h = Swarm::new(Strain::Creeper);
        for _ in 0..3 {
            h.think(&swarm_at(6.0, 8, 2.0), 99.0);
        }
        assert_eq!(h.state, Hive::Feed);
        h.think(&swarm_at(6.0, 2, 2.0), 99.0);
        assert_eq!(h.state, Hive::Crawl, "two left should not still be feeding");
    }

    #[test]
    fn it_speaks_the_same_vocabulary_as_a_commander() {
        // the claim that makes it a Brain at all: the game downstream cannot
        // tell a hive from a squad commander
        use crate::brain::read_orders;
        use crate::brain::write_orders;
        for st in Strain::all() {
            for d in [200.0f32, 20.0, 4.0] {
                let mut h = Swarm::new(st);
                let o = h.think(&swarm_at(d, 8, 2.0), 99.0).unwrap();
                assert_eq!(o.cmds.len(), 1);
                assert!(o.cmds[0].sane(), "{} emitted an unusable command", st.name());
                assert!(read_orders(&write_orders(&o)).is_some(), "did not round-trip");
            }
        }
    }

    #[test]
    fn an_empty_field_does_not_panic_or_keep_feeding() {
        let mut h = Swarm::new(Strain::Ravener);
        h.think(&swarm_at(4.0, 6, 1.0), 99.0);
        let empty = Snapshot { tick: 2, ..Default::default() };
        let o = h.think(&empty, 99.0).unwrap();
        assert_eq!(h.state, Hive::Crawl);
        assert_eq!(o.cmds[0].verb, Verb::Hold);
    }

    #[test]
    fn height_is_terrain_and_does_not_make_the_prey_far_away() {
        // a swarm under a ledge should not decide the player is out of reach
        let mut flat = swarm_at(6.0, 6, 2.0);
        let mut high = flat.clone();
        high.player_pos = Vec3::new(0.0, 40.0, 0.0);
        let mut a = Swarm::new(Strain::Ravener);
        let mut b = Swarm::new(Strain::Ravener);
        a.think(&flat, 99.0);
        b.think(&high, 99.0);
        assert_eq!(a.state, b.state, "forty metres of altitude changed the hive's mind");
        flat.tick += 1;
    }
}

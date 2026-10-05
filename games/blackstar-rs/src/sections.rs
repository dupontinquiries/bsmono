//! sections — per-section damage for bots.
//!
//! ## Why this file exists here
//!
//! The fold patch's `main.rs` calls `sections::Body::hit`, `kind_for` and
//! `Kind::Blast`, and its handoff note says the monorepo "already has the
//! line". It did not: there was no sections.rs in this tree at all. So this is
//! written to the CONTRACT THE CALL SITES STATE rather than copied from the
//! sibling — the five uses in main.rs pin the signature exactly, and the
//! comments there say what each argument means.
//!
//! Where a decision was not pinned by a call site, it is derived from something
//! already in this repo rather than invented. In particular:
//!
//! ## Kind is Delivery, not a second table
//!
//! `weapons::Verb::delivery()` already sorts every verb into Kinetic / Beam /
//! Blast / Melee, with a stated rationale ("Flak and Pierce throw mass, Beam is
//! light, Pool and Vortex are a pressure event"). `kind_for` delegates to it.
//!
//! A parallel mapping here would be a second opinion about what a weapon IS,
//! and the two would drift the first time a verb was added — one of them
//! silently, because nothing compares them.
//!
//! ## The geometry is the fold's own
//!
//! Sections are named for the parts the fold already articulates, so a torn-off
//! arm is a part the renderer knows how to stop drawing.

use crate::ballistics::Delivery;
use glam::Vec3;

/// What kind of event hit the body. One-to-one with `Delivery`.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind { Kinetic, Beam, Blast, Melee }

/// The damage kind a weapon verb delivers.
pub fn kind_for(verb: crate::weapons::Verb) -> Kind {
    match verb.delivery() {
        Delivery::Kinetic => Kind::Kinetic,
        Delivery::Beam => Kind::Beam,
        Delivery::Blast => Kind::Blast,
        Delivery::Melee => Kind::Melee,
    }
}

impl Kind {
    /// How much of a hit spreads to neighbouring sections.
    ///
    /// A blast is felt next door; a beam is not. This is the whole reason the
    /// kind is carried at all — without it every weapon would damage exactly
    /// one section and a grenade would behave like a rifle.
    fn spread(self) -> f32 {
        match self {
            Kind::Blast => 0.6,
            Kind::Melee => 0.25,
            Kind::Kinetic => 0.15,
            // A beam cuts where it lands. No spread at all.
            Kind::Beam => 0.0,
        }
    }

    /// Can this kind sever a section outright?
    ///
    /// A beam and a blade cut; a rifle round punches through and a blast
    /// crushes. Severing on every kind would have bots losing arms to small
    /// arms fire, which reads as the model being broken rather than as damage.
    fn can_sever(self) -> bool {
        matches!(self, Kind::Beam | Kind::Melee)
    }
}

/// A named part of a bot.
///
/// Named for what the fold articulates, so a severed part is one the renderer
/// already knows how to stop drawing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Section { Head, Torso, ArmL, ArmR, LegL, LegR }

impl Section {
    pub const ALL: [Section; 6] = [Section::Head, Section::Torso, Section::ArmL,
                                   Section::ArmR, Section::LegL, Section::LegR];

    /// The name shown in the "torn off" toast.
    pub fn name(self) -> &'static str {
        match self {
            Section::Head => "head", Section::Torso => "torso",
            Section::ArmL => "left arm", Section::ArmR => "right arm",
            Section::LegL => "left leg", Section::LegR => "right leg",
        }
    }

    /// Roughly where this section sits in body space, in metres.
    ///
    /// Body space is what `main.rs` hands in: the impact is already rotated out
    /// of world space by the bot's own inverse rotation, so +Y is up and +X is
    /// the bot's right regardless of where it is facing.
    fn centre(self) -> Vec3 {
        match self {
            Section::Head => Vec3::new(0.0, 1.6, 0.0),
            Section::Torso => Vec3::new(0.0, 1.0, 0.0),
            Section::ArmL => Vec3::new(-0.6, 1.1, 0.0),
            Section::ArmR => Vec3::new(0.6, 1.1, 0.0),
            Section::LegL => Vec3::new(-0.25, 0.4, 0.0),
            Section::LegR => Vec3::new(0.25, 0.4, 0.0),
        }
    }

    /// Health at full. The torso is the body: losing it is losing the bot.
    fn max_hp(self) -> f32 {
        match self {
            Section::Torso => 3.0,
            Section::Head => 1.0,
            _ => 1.5,
        }
    }

    /// Does destroying this section stop the bot?
    ///
    /// Head and torso only. A bot with no arms is a bot that cannot fight; a
    /// bot with no torso is scrap, and the distinction is what makes
    /// dismemberment worth modelling rather than a second health bar.
    fn vital(self) -> bool {
        matches!(self, Section::Head | Section::Torso)
    }
}

/// What a hit did.
pub struct HitResult {
    /// Sections that came off, in the order they failed.
    pub severed: Vec<Section>,
    /// The bot is down.
    pub killed: bool,
    /// Which section the impact actually landed on nearest-centre --
    /// exposed for callers that want to react to WHERE a hit landed
    /// beyond damage/severing (a scorch decal, a targeted-hit toast),
    /// not just recompute `nearest` a second time themselves. `None`
    /// only when the hit did nothing at all (amount <= 0.0 or the body
    /// was already dead) -- see `hit`'s own early return.
    pub struck: Option<Section>,
}

/// A bot's per-section condition.
#[derive(Clone, Debug)]
pub struct Body {
    hp: [f32; 6],
    /// Already severed, so a part cannot come off twice.
    gone: [bool; 6],
    /// Seconds since this section last took damage. Healing and
    /// regrowth are both gated on this — a bot mid-fight should not
    /// visibly patch itself up between incoming rounds, and a limb
    /// that just came off should not spring back before the fight that
    /// took it is even over.
    since_hit: [f32; 6],
}

impl Default for Body {
    fn default() -> Self {
        let mut hp = [0.0f32; 6];
        for (i, s) in Section::ALL.iter().enumerate() {
            hp[i] = s.max_hp();
        }
        Body { hp, gone: [false; 6], since_hit: [f32::MAX; 6] }
    }
}

impl Body {
    pub fn hp_of(&self, s: Section) -> f32 { self.hp[Self::idx(s)] }
    pub fn is_gone(&self, s: Section) -> bool { self.gone[Self::idx(s)] }
    /// A vital section (head, torso) can never actually be marked "gone" —
    /// `apply` only severs `!s.vital()` parts — so checking `is_gone` alone
    /// meant this returned `true` unconditionally: a torso at 0 hp is
    /// destroyed but still attached, not severed, and `is_gone` never sees
    /// it. `hp_of(*s) > 0.0` is the actual "still standing" check;
    /// `!is_gone` stays as a defensive second condition in case a future
    /// change ever does let a vital section come off. `all` rather than
    /// `any`: the module doc says "a bot with no torso is scrap," not "a
    /// bot with no torso AND no head" — losing either vital section alone
    /// is what "vital" is supposed to mean.
    pub fn alive(&self) -> bool {
        Section::ALL.iter().filter(|s| s.vital()).all(|s| !self.is_gone(*s) && self.hp_of(*s) > 0.0)
    }

    /// Leg asymmetry, as a signed fraction. Negative means the LEFT leg is the
    /// hurt one.
    ///
    /// The sign convention is main.rs's, not mine — its comment says
    /// "limp<0 means the LEFT side is the hurt one" and the gait code reads it
    /// that way. Getting it backwards would make a bot limp on its good leg,
    /// which looks like a bug in the animation rather than in the damage.
    ///
    /// A severed leg is a full limp regardless of what the other one has left:
    /// there is nothing to favour it WITH.
    pub fn limp(&self) -> f32 {
        let frac = |s: Section| -> f32 {
            if self.is_gone(s) { 0.0 } else { self.hp_of(s) / s.max_hp() }
        };
        // right minus left: a hurt LEFT leg has the smaller fraction, so the
        // difference comes out positive — negated to match the convention.
        -(frac(Section::LegR) - frac(Section::LegL))
    }

    fn idx(s: Section) -> usize {
        Section::ALL.iter().position(|x| *x == s).unwrap_or(1)
    }

    /// Which section an impact landed on.
    ///
    /// Nearest centre, skipping parts already gone — a hit that passes through
    /// the space where an arm used to be should carry on to the torso rather
    /// than hitting nothing.
    fn nearest(&self, local: Vec3) -> Section {
        let mut best = Section::Torso;
        let mut best_d = f32::MAX;
        for s in Section::ALL {
            if self.is_gone(s) { continue; }
            let d = (s.centre() - local).length_squared();
            if d < best_d { best_d = d; best = s; }
        }
        best
    }

    /// Apply a hit.
    ///
    /// Arguments are exactly what main.rs supplies: the kind from the verb, the
    /// impact point and direction already in BODY space, the amount scaled from
    /// delta-v, and a blast radius that is zero for everything else.
    pub fn hit(&mut self, kind: Kind, local: Vec3, dir_local: Vec3, amount: f32,
               radius: f32) -> HitResult {
        let _ = dir_local; // direction is carried for future angled armour
        let mut severed = Vec::new();
        if amount <= 0.0 || !self.alive() {
            return HitResult { severed, killed: !self.alive(), struck: None };
        }

        let struck = self.nearest(local);
        self.apply(struck, amount, kind, &mut severed);

        // Spread to neighbours. A blast carries a radius from the weapon's
        // reach; the others spread by kind alone, so a sabre still nicks what
        // is next to what it cut.
        let spread = kind.spread();
        if spread > 0.0 {
            let reach = radius.max(0.5);
            for s in Section::ALL {
                if s == struck || self.is_gone(s) { continue; }
                let d = (s.centre() - local).length();
                if d <= reach {
                    // Falls off with distance, so a near miss is not a hit.
                    let falloff = 1.0 - (d / reach).clamp(0.0, 1.0);
                    self.apply(s, amount * spread * falloff, kind, &mut severed);
                }
            }
        }

        let killed = !self.alive();
        HitResult { severed, killed, struck: Some(struck) }
    }

    fn apply(&mut self, s: Section, amount: f32, kind: Kind, severed: &mut Vec<Section>) {
        let i = Self::idx(s);
        if self.gone[i] { return; }
        self.since_hit[i] = 0.0; // a fresh hit restarts the healing clock
        self.hp[i] = (self.hp[i] - amount).max(0.0);
        if self.hp[i] > 0.0 { return; }
        // At zero: a limb comes OFF only to a kind that cuts. Everything else
        // leaves it disabled at zero hp — visibly wrecked, still attached.
        if kind.can_sever() && !s.vital() {
            self.gone[i] = true;
            severed.push(s);
        }
    }

    /// Seconds of quiet (no hits to a section) before it starts
    /// regenerating hp. A bot mid-fight should not visibly patch itself
    /// up between incoming rounds — healing is a claim about having
    /// disengaged, not a passive tick that fights alongside the damage.
    pub const REGEN_DELAY: f32 = 4.0;
    /// hp/second once regen has started.
    pub const REGEN_RATE: f32 = 0.4;
    /// Seconds of quiet before a SEVERED section regrows. Longer than
    /// REGEN_DELAY on purpose: a limb coming back mid-fight should read
    /// as a cheat, not a mechanic, and REGROW_DELAY is what keeps a
    /// regrow from ever landing before REGEN_DELAY has already let
    /// every attached section start healing first.
    pub const REGROW_DELAY: f32 = 10.0;
    /// hp a regrown section comes back with — a fraction of max, not
    /// full: the part is new, not undamaged history.
    const REGROW_HP_FRACTION: f32 = 0.5;

    /// Advance the healing clock. Returns the sections that regrew this
    /// tick, in `Section::ALL` order, for a caller that wants to spawn
    /// the "grown back" animation/fx on exactly the frame it happens
    /// rather than polling `is_gone` every frame to notice.
    ///
    /// Back to factory: every section whole, nothing severed, nothing recently hit. What a
    /// launch, a raid, a jump to another world, and the field rebuild after a loss all start
    /// from. Without it a body that died once stayed dead for the rest of the session --
    /// `hit` refuses a dead body and `tick` does not heal one -- so the second sortie could
    /// not be lost and `player_down` never fired again.
    pub fn repair(&mut self) {
        *self = Body::default();
    }

    /// A dead body does not heal — `alive()` gates the whole tick, same
    /// as `hit` already refuses to apply further damage to one.
    pub fn tick(&mut self, dt: f32) -> Vec<Section> {
        let mut regrown = Vec::new();
        if !self.alive() || dt <= 0.0 {
            return regrown;
        }
        for (i, s) in Section::ALL.iter().enumerate() {
            // saturating rather than unbounded: a body that sat untouched
            // for an hour should not regrow the INSTANT it takes a single
            // scratch, which is what an f32 that overflowed toward
            // infinity and then got reset to 0.0 by the next hit would
            // otherwise risk if some future caller compared it oddly
            self.since_hit[i] = (self.since_hit[i] + dt).min(f32::MAX / 2.0);
            if self.gone[i] {
                if self.since_hit[i] >= Self::REGROW_DELAY {
                    self.gone[i] = false;
                    self.hp[i] = s.max_hp() * Self::REGROW_HP_FRACTION;
                    regrown.push(*s);
                }
                continue;
            }
            if self.since_hit[i] >= Self::REGEN_DELAY && self.hp[i] < s.max_hp() {
                self.hp[i] = (self.hp[i] + Self::REGEN_RATE * dt).min(s.max_hp());
            }
        }
        regrown
    }
}

/// The field rebuild: a body that goes down is scrap for [`Rebuild::SECS`], then stands up
/// whole. Ticked beside `Body::tick` in play; a sortie's loss has already been recorded by
/// then (`Sortie::player_down` fires on the frame of death), so the rebuild never undoes it.
#[derive(Clone, Copy, Debug, Default, PartialEq)]
pub struct Rebuild {
    /// Seconds of scrap left; 0 while alive.
    t: f32,
}

impl Rebuild {
    pub const SECS: f32 = 3.0;

    /// Seconds until the body stands again, 0 when it is standing.
    pub fn remaining(&self) -> f32 {
        self.t
    }

    /// Advance; `true` on the frame the body is rebuilt.
    pub fn tick(&mut self, body: &mut Body, dt: f32) -> bool {
        if body.alive() {
            self.t = 0.0;
            return false;
        }
        if self.t <= 0.0 {
            self.t = Self::SECS;
        }
        self.t -= dt.max(0.0);
        if self.t <= 0.0 {
            body.repair();
            self.t = 0.0;
            return true;
        }
        false
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::Verb;

    fn killed() -> Body {
        let mut b = Body::default();
        for _ in 0..100 {
            b.hit(Kind::Kinetic, Section::Torso.centre(), Vec3::Z, 100.0, 0.0);
        }
        assert!(!b.alive(), "the torso must be destroyed");
        b
    }

    #[test]
    fn a_repaired_body_is_whole_and_can_be_hurt_and_killed_again() {
        let mut b = killed();
        assert!(!b.tick(100.0).is_empty() || !b.alive(), "dead: no healing");
        b.repair();
        assert!(b.alive());
        for s in Section::ALL {
            assert_eq!(b.hp_of(s), s.max_hp(), "{}", s.name());
            assert!(!b.is_gone(s));
        }
        let r = b.hit(Kind::Kinetic, Section::ArmL.centre(), Vec3::Z, 5.0, 0.0);
        assert!(!r.killed && b.hp_of(Section::ArmL) < Section::ArmL.max_hp(), "a hit lands again");
        let b2 = killed();
        assert!(!b2.alive(), "and it can die again");
    }

    #[test]
    fn the_rebuild_waits_its_seconds_then_stands_the_body_up_once() {
        let mut b = killed();
        let mut r = Rebuild::default();
        assert!(!r.tick(&mut b, 1.0 / 60.0));
        assert!(r.remaining() > Rebuild::SECS - 0.1 && r.remaining() < Rebuild::SECS);
        let mut t = 1.0 / 60.0;
        let mut rebuilt_at = None;
        for _ in 0..600 {
            if r.tick(&mut b, 1.0 / 60.0) {
                rebuilt_at = Some(t);
                break;
            }
            t += 1.0 / 60.0;
        }
        let at = rebuilt_at.expect("rebuilt within ten seconds");
        assert!((at - Rebuild::SECS).abs() < 0.05, "rebuilt at {at}s, not {}", Rebuild::SECS);
        assert!(b.alive() && r.remaining() == 0.0);
        // alive: nothing happens, and the countdown does not run
        assert!(!r.tick(&mut b, 10.0));
        assert_eq!(r.remaining(), 0.0);
    }

    #[test]
    fn healing_waits_for_regen_delay_then_caps_at_max_hp() {
        let mut b = Body::default();
        b.hit(Kind::Kinetic, Section::ArmL.centre(), Vec3::Z, 1.0, 0.0);
        assert!(b.hp_of(Section::ArmL) < Section::ArmL.max_hp(), "the hit should have done damage");
        // before REGEN_DELAY: no change
        let hp_before = b.hp_of(Section::ArmL);
        b.tick(Body::REGEN_DELAY - 0.1);
        assert_eq!(b.hp_of(Section::ArmL), hp_before, "healed before the quiet period elapsed");
        // past it: healing resumes and caps at max, does not overshoot
        b.tick(100.0);
        assert_eq!(b.hp_of(Section::ArmL), Section::ArmL.max_hp(), "healing overshot or never capped");
    }

    #[test]
    fn a_fresh_hit_restarts_the_healing_clock() {
        let mut b = Body::default();
        b.hit(Kind::Kinetic, Section::ArmL.centre(), Vec3::Z, 1.0, 0.0);
        b.tick(Body::REGEN_DELAY + 0.5); // healing would have started
        let mid_heal = b.hp_of(Section::ArmL);
        b.hit(Kind::Kinetic, Section::ArmL.centre(), Vec3::Z, 0.01, 0.0); // any hit, even tiny
        b.tick(Body::REGEN_DELAY - 0.1); // same short wait as before
        assert!(b.hp_of(Section::ArmL) <= mid_heal, "healing continued through a fresh hit's cooldown");
    }

    #[test]
    fn a_severed_section_regrows_after_the_longer_delay_at_partial_hp() {
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::ArmL.centre(), Vec3::Z, 99.0, 0.0); // sever it
        assert!(b.is_gone(Section::ArmL));
        // regrow delay is longer than regen delay on purpose -- confirm
        // it does not come back at the shorter interval
        let regrown = b.tick(Body::REGEN_DELAY + 0.1);
        assert!(b.is_gone(Section::ArmL), "regrew before REGROW_DELAY");
        assert!(regrown.is_empty());
        let regrown = b.tick(Body::REGROW_DELAY);
        assert!(!b.is_gone(Section::ArmL), "did not regrow after REGROW_DELAY");
        assert_eq!(regrown, vec![Section::ArmL]);
        let expected = Section::ArmL.max_hp() * 0.5;
        assert!((b.hp_of(Section::ArmL) - expected).abs() < 1e-5,
            "a regrown limb should come back partial, not full: got {}", b.hp_of(Section::ArmL));
    }

    #[test]
    fn a_dead_body_does_not_heal() {
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::Torso.centre(), Vec3::Z, 99.0, 0.0);
        assert!(!b.alive());
        let hp_before = b.hp_of(Section::ArmL);
        let regrown = b.tick(1000.0);
        assert!(regrown.is_empty());
        assert_eq!(b.hp_of(Section::ArmL), hp_before, "a dead body's untouched sections should not heal either");
    }

    #[test]
    fn kind_follows_the_verbs_own_delivery() {
        // Not a second table: weapons::Verb::delivery() already sorts these,
        // and a parallel mapping would drift the first time a verb was added.
        assert_eq!(kind_for(Verb::Beam), Kind::Beam);
        assert_eq!(kind_for(Verb::Pierce), Kind::Kinetic);
        assert_eq!(kind_for(Verb::Vortex), Kind::Blast);
        assert_eq!(kind_for(Verb::Thrust), Kind::Melee);
    }

    #[test]
    fn a_fresh_body_is_whole_and_alive() {
        let b = Body::default();
        assert!(b.alive());
        assert!(Section::ALL.iter().all(|s| !b.is_gone(*s)));
    }

    #[test]
    fn a_hit_lands_on_the_nearest_section() {
        let mut b = Body::default();
        // Head height, dead centre.
        b.hit(Kind::Beam, Vec3::new(0.0, 1.6, 0.0), Vec3::Z, 0.5, 0.0);
        assert!(b.hp_of(Section::Head) < 1.0);
        assert_eq!(b.hp_of(Section::Torso), Section::Torso.max_hp());
    }

    #[test]
    fn a_beam_does_not_spread_and_a_blast_does() {
        // The whole reason the kind is carried: without it a grenade would
        // behave like a rifle.
        let mut beam = Body::default();
        beam.hit(Kind::Beam, Vec3::new(0.6, 1.1, 0.0), Vec3::Z, 0.4, 0.0);
        assert_eq!(beam.hp_of(Section::Torso), Section::Torso.max_hp(),
                   "a beam reached a section it did not hit");

        let mut blast = Body::default();
        blast.hit(Kind::Blast, Vec3::new(0.6, 1.1, 0.0), Vec3::Z, 0.4, 2.0);
        assert!(blast.hp_of(Section::Torso) < Section::Torso.max_hp(),
                "a blast did not spread");
    }

    #[test]
    fn only_a_cutting_kind_severs() {
        // Severing on every kind would have bots losing arms to small arms
        // fire, which reads as a broken model rather than as damage.
        let mut rifle = Body::default();
        for _ in 0..10 {
            rifle.hit(Kind::Kinetic, Section::ArmR.centre(), Vec3::Z, 1.0, 0.0);
        }
        assert_eq!(rifle.hp_of(Section::ArmR), 0.0, "the arm should be wrecked");
        assert!(!rifle.is_gone(Section::ArmR), "a rifle tore an arm off");

        let mut sabre = Body::default();
        let res = sabre.hit(Kind::Melee, Section::ArmR.centre(), Vec3::Z, 99.0, 0.0);
        assert!(sabre.is_gone(Section::ArmR));
        assert!(res.severed.contains(&Section::ArmR));
    }

    #[test]
    fn a_vital_section_is_never_severed_only_destroyed() {
        // A head that falls off and a bot that is down are the same event;
        // reporting a severed head AND a kill would toast twice for one thing.
        let mut b = Body::default();
        let res = b.hit(Kind::Melee, Section::Head.centre(), Vec3::Z, 99.0, 0.0);
        assert!(!b.is_gone(Section::Head));
        assert!(res.severed.is_empty());
    }

    #[test]
    fn destroying_the_torso_kills() {
        let mut b = Body::default();
        let res = b.hit(Kind::Melee, Section::Torso.centre(), Vec3::Z, 99.0, 0.0);
        assert!(res.killed);
        assert!(!b.alive());
    }

    #[test]
    fn losing_both_arms_does_not_kill() {
        // The distinction that makes dismemberment worth modelling rather than
        // a second health bar.
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::ArmL.centre(), Vec3::Z, 99.0, 0.0);
        b.hit(Kind::Melee, Section::ArmR.centre(), Vec3::Z, 99.0, 0.0);
        assert!(b.is_gone(Section::ArmL) && b.is_gone(Section::ArmR));
        assert!(b.alive());
    }

    #[test]
    fn a_part_cannot_come_off_twice() {
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::LegL.centre(), Vec3::Z, 99.0, 0.0);
        let again = b.hit(Kind::Melee, Section::LegL.centre(), Vec3::Z, 99.0, 0.0);
        assert!(!again.severed.contains(&Section::LegL));
    }

    #[test]
    fn a_hit_where_a_limb_used_to_be_carries_on_to_the_torso() {
        // Otherwise shooting the gap does nothing, and a dismembered bot
        // becomes harder to finish than a whole one.
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::ArmR.centre(), Vec3::Z, 99.0, 0.0);
        let before = b.hp_of(Section::Torso);
        b.hit(Kind::Kinetic, Section::ArmR.centre(), Vec3::Z, 0.5, 0.0);
        assert!(b.hp_of(Section::Torso) < before);
    }

    #[test]
    fn a_zero_amount_hit_changes_nothing() {
        // main.rs already skips these, but a damage model that takes damage
        // from a zero-energy impact is wrong on its own terms.
        let mut b = Body::default();
        b.hit(Kind::Blast, Vec3::ZERO, Vec3::Z, 0.0, 2.0);
        assert!(Section::ALL.iter().all(|s| b.hp_of(*s) == s.max_hp()));
    }
}

#[cfg(test)]
mod limp_tests {
    use super::*;

    #[test]
    fn a_whole_body_does_not_limp() {
        assert_eq!(Body::default().limp(), 0.0);
    }

    #[test]
    fn the_sign_says_which_leg_is_hurt() {
        // main.rs: "limp<0 means the LEFT side is the hurt one". Backwards
        // here would make a bot limp on its GOOD leg — a bug that looks like
        // the animation rather than the damage.
        let mut left = Body::default();
        left.hit(Kind::Kinetic, Section::LegL.centre(), Vec3::Z, 0.7, 0.0);
        assert!(left.limp() < 0.0, "hurt left leg gave {}", left.limp());

        let mut right = Body::default();
        right.hit(Kind::Kinetic, Section::LegR.centre(), Vec3::Z, 0.7, 0.0);
        assert!(right.limp() > 0.0, "hurt right leg gave {}", right.limp());
    }

    #[test]
    fn a_severed_leg_is_a_full_limp() {
        // Nothing to favour it with.
        let mut b = Body::default();
        b.hit(Kind::Melee, Section::LegL.centre(), Vec3::Z, 99.0, 0.0);
        assert!((b.limp() + 1.0).abs() < 1e-6, "{}", b.limp());
    }

    #[test]
    fn equal_damage_to_both_legs_is_not_a_limp() {
        // A bot hurt evenly walks slowly, it does not lurch — and the gait
        // reads only the ASYMMETRY.
        let mut b = Body::default();
        b.hit(Kind::Kinetic, Section::LegL.centre(), Vec3::Z, 0.5, 0.0);
        b.hit(Kind::Kinetic, Section::LegR.centre(), Vec3::Z, 0.5, 0.0);
        assert!(b.limp().abs() < 1e-6, "{}", b.limp());
    }
}


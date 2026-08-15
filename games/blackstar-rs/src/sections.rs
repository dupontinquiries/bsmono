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
}

/// A bot's per-section condition.
#[derive(Clone, Debug)]
pub struct Body {
    hp: [f32; 6],
    /// Already severed, so a part cannot come off twice.
    gone: [bool; 6],
}

impl Default for Body {
    fn default() -> Self {
        let mut hp = [0.0f32; 6];
        for (i, s) in Section::ALL.iter().enumerate() {
            hp[i] = s.max_hp();
        }
        Body { hp, gone: [false; 6] }
    }
}

impl Body {
    pub fn hp_of(&self, s: Section) -> f32 { self.hp[Self::idx(s)] }
    pub fn is_gone(&self, s: Section) -> bool { self.gone[Self::idx(s)] }
    pub fn alive(&self) -> bool {
        Section::ALL.iter().filter(|s| s.vital()).any(|s| !self.is_gone(*s))
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
            return HitResult { severed, killed: !self.alive() };
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
        HitResult { severed, killed }
    }

    fn apply(&mut self, s: Section, amount: f32, kind: Kind, severed: &mut Vec<Section>) {
        let i = Self::idx(s);
        if self.gone[i] { return; }
        self.hp[i] = (self.hp[i] - amount).max(0.0);
        if self.hp[i] > 0.0 { return; }
        // At zero: a limb comes OFF only to a kind that cuts. Everything else
        // leaves it disabled at zero hp — visibly wrecked, still attached.
        if kind.can_sever() && !s.vital() {
            self.gone[i] = true;
            severed.push(s);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::weapons::Verb;

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


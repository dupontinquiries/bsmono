//! STEALTH — two ways to be found, and they want opposite things from you.
//!
//! Ported from `game.c`'s sneak / awareness / assassination block. The C
//! header sells it as Halo-style assassinations, and that is the payoff; the
//! design underneath is better than the pitch.
//!
//! ## Two channels, and this is the whole idea
//!
//! **SIGHT.** Range, a facing cone, line of sight. Beaten by distance, by
//! cover, by being behind someone, and by being small — a human is spotted at
//! forty-five metres where a bot is spotted at ninety-five.
//!
//! **SPARK RESONANCE.** The Corrupted and the apex frames do not hunt by
//! looking. They feel a spark working: firing, sprinting, using abilities.
//! Sensed **through walls**, with no facing and no line of sight, at a range
//! that grows with how loud you are.
//!
//! The two have **opposite counters**, and that is the mechanic. Sight is
//! beaten by geometry — get behind something. Resonance is beaten by
//! *stopping*. So the instinct that saves you from a sentry (duck behind a
//! wall and keep shooting) is the one that lights you up for a hunter, and a
//! player who has learned one enemy has learned the wrong lesson about the
//! other.
//!
//! `perception.rs` models the first channel and has never had the second.
//! This adds it, and the composition is the point: `sight_rate` delegates to
//! `perception::rate` rather than reimplementing a falloff.
//!
//! ## Awareness has hysteresis, and it must
//!
//! Once found, an enemy stays found for `LOSE_S` after it stops sensing you.
//! Without that, stepping behind a pillar for a single frame resets a hunt,
//! and the whole thing becomes a strobe rather than a chase — the same
//! failure the amalgam's sweep had for the same reason.
//!
//! ## Assassination
//!
//! An UNAWARE enemy can be killed at any health from close. Apex frames are
//! too vast: the strike wounds them instead. Which animation plays is decided
//! by the approach — descending is aerial, sneaking is a ghost kill nobody
//! hears, otherwise a backbreaker — and the ghost is the only one that does
//! not make noise, which folds straight back into the resonance channel.

use crate::perception::{rate, Cover, Watcher};
use glam::Vec3;

/// How far a bot is visible in the open.
pub const SIGHT_M: f32 = 95.0;
/// A human is small and hard to spot.
pub const SIGHT_HUMAN_M: f32 = 45.0;
/// Low and slow: seen only up close.
pub const SIGHT_SNEAK_M: f32 = 42.0;
/// Forward cone, as a dot product against the heading.
pub const FACING_DOT: f32 = 0.25;
/// Resonance floor: felt this far even when barely active.
pub const RESONANCE_BASE_M: f32 = 40.0;
/// And how much further a fully lit spark carries.
pub const RESONANCE_GAIN_M: f32 = 90.0;
/// Loud enough to be felt at all.
pub const RESONANCE_FLOOR: f32 = 0.15;
/// How long an enemy stays alert after losing you.
pub const LOSE_S: f32 = 3.5;
/// Assassination reach.
pub const REACH_M: f32 = 7.5;
/// What a strike takes off an apex frame instead of killing it.
pub const APEX_WOUND: f32 = 0.45;

/// What the player is doing to their own spark.
#[derive(Clone, Copy, Debug, Default)]
pub struct Spark {
    /// 0..1, decays
    pub loud: f32,
    pub sneaking: bool,
    /// the frontier cloak: a stealth drape, sensor-dim
    pub cloaked: bool,
}

impl Spark {
    /// Firing, sprinting, abilities. The things that make you findable
    /// through a wall.
    pub fn act(&mut self, amount: f32) {
        self.loud = (self.loud + amount).clamp(0.0, 1.0);
    }
    pub fn decay(&mut self, dt: f32) {
        // the weave damps resonance: quiet comes quicker, and sneaking
        // quiets faster still
        let r = 0.9 * if self.cloaked { 1.5 / 0.9 } else { 1.0 }
            * if self.sneaking { 1.6 } else { 1.0 };
        self.loud = (self.loud - r * dt).max(0.0);
    }
    /// How far this spark is felt, in metres. Zero when quiet enough.
    pub fn felt_m(&self) -> f32 {
        if self.loud <= RESONANCE_FLOOR {
            return 0.0;
        }
        let mut s = RESONANCE_BASE_M + self.loud * RESONANCE_GAIN_M;
        if self.cloaked {
            s *= 0.72;
        }
        if self.sneaking {
            s *= 0.62;
        }
        s
    }
}

/// What kind of thing is hunting.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hunter {
    /// hunts by looking
    Sighted,
    /// hunts by feeling a spark: the Corrupted
    Resonant,
    /// too vast to assassinate
    Apex,
}

impl Hunter {
    pub fn feels_resonance(self) -> bool {
        matches!(self, Hunter::Resonant | Hunter::Apex)
    }
}

/// How far this observer can see the player, given who the player is.
pub fn sight_range_m(human_scale: bool, sneaking: bool, storm: f32) -> f32 {
    let mut r = if human_scale { SIGHT_HUMAN_M } else { SIGHT_M };
    if sneaking {
        r = r.min(SIGHT_SNEAK_M);
    }
    // the storm eats sightlines
    if storm > 0.25 {
        r = r.min(SIGHT_M - 62.0 * storm);
    }
    r.max(1.0)
}

/// Is the player inside the observer's cone and unblocked?
pub fn in_sight(
    observer: Vec3,
    heading: Vec3,
    player: Vec3,
    blocked: bool,
    range_m: f32,
) -> bool {
    let to = player - observer;
    let d = Vec3::new(to.x, 0.0, to.z);
    let dist = d.length();
    if dist > range_m || blocked {
        return false;
    }
    if dist < 0.5 {
        return true;
    }
    d.normalize().dot(Vec3::new(heading.x, 0.0, heading.z).normalize_or_zero()) > FACING_DOT
}

/// How fast an observer builds certainty visually, via `perception`.
///
/// Delegated rather than reimplemented: the inverse-square falloff and the
/// motion factor belong to one module, and a second copy would drift.
pub fn sight_rate(w: &Watcher, dist_m: f32, cover: Cover, moving: bool) -> f32 {
    rate(w, dist_m, cover, moving)
}

/// One enemy's knowledge of the player.
#[derive(Clone, Copy, Debug, Default)]
pub struct Awareness {
    pub aware: bool,
    /// seconds since it last sensed anything
    pub losing: f32,
    /// set on the tick it first notices
    pub startled: bool,
}

impl Awareness {
    /// `sensed` is either channel firing this tick.
    pub fn step(&mut self, dt: f32, sensed: bool) {
        self.startled = false;
        if sensed {
            if !self.aware {
                self.startled = true;
            }
            self.aware = true;
            self.losing = 0.0;
        } else if self.aware {
            self.losing += dt;
            if self.losing > LOSE_S {
                self.aware = false;
                self.losing = 0.0;
            }
        }
    }
}

/// The full check for one observer.
pub struct Observer {
    pub kind: Hunter,
    pub pos: Vec3,
    pub heading: Vec3,
    pub awareness: Awareness,
    /// visual certainty, 0..1; reaching 1 is having seen you
    pub certainty: f32,
}

impl Observer {
    /// How keenly this kind looks. An apex frame is enormous and attentive;
    /// an ordinary trooper is doing a job.
    pub fn eyes(&self) -> Watcher {
        match self.kind {
            Hunter::Apex => Watcher { range_m: SIGHT_M, attention: 0.9, relentless: true },
            Hunter::Resonant => Watcher { range_m: SIGHT_M, attention: 0.5, relentless: true },
            Hunter::Sighted => Watcher { range_m: SIGHT_M, attention: 0.6, relentless: false },
        }
    }
}

/// Why an observer knows about you, which the player needs to be told.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Sensed {
    No,
    /// saw you
    Sight,
    /// felt your spark, through whatever was in the way
    Resonance,
}

impl Observer {
    pub fn sense(
        &mut self,
        dt: f32,
        player: Vec3,
        spark: &Spark,
        blocked: bool,
        human_scale: bool,
        storm: f32,
    ) -> Sensed {
        let dist = Vec3::new(self.pos.x - player.x, 0.0, self.pos.z - player.z).length();

        // RESONANCE FIRST, and it does not care about the wall.
        if self.kind.feels_resonance() {
            let felt = spark.felt_m();
            if felt > 0.0 && dist < felt {
                self.awareness.step(dt, true);
                return Sensed::Resonance;
            }
        }
        // SIGHT IS A RATE, NOT A BOOLEAN. Being inside the cone and unblocked
        // is permission to start noticing, not the noticing itself — and how
        // fast that goes is `perception`'s question, which already answers it
        // with an inverse-square falloff and a motion term. The first version
        // of this treated in-cone as instantly seen, which made `sight_rate`
        // an unused function and the claim that these two modules compose a
        // decoration.
        let r = sight_range_m(human_scale, spark.sneaking, storm);
        if !in_sight(self.pos, self.heading, player, blocked, r) {
            self.awareness.step(dt, false);
            return Sensed::No;
        }
        let cover = if spark.sneaking { Cover::PARTIAL } else { Cover::OPEN };
        let gain = sight_rate(&self.eyes(), dist, cover, true) * dt;
        self.certainty = (self.certainty + gain).min(2.0);
        let seen = self.certainty >= 1.0;
        self.awareness.step(dt, seen);
        if seen {
            Sensed::Sight
        } else {
            Sensed::No
        }
    }
}

/// How the kill plays, decided by the approach.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Style {
    /// dropping onto them
    Aerial,
    /// sneaking: fully silent, the pack never knows
    Ghost,
    Backbreaker,
}

impl Style {
    /// Does it make noise? Only the ghost does not — which feeds straight
    /// back into the resonance channel, so a silent kill does not hand you to
    /// the next hunter.
    pub fn silent(self) -> bool {
        self == Style::Ghost
    }
}

/// What a strike did.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Strike {
    /// not in reach, or they already know
    Refused(&'static str),
    Killed(Style),
    /// apex frames are too vast: a fraction of their health instead
    Wounded(f32),
}

/// Attempt an assassination.
pub fn assassinate(
    target: &Observer,
    dist_m: f32,
    descending: bool,
    spark: &Spark,
) -> Strike {
    if target.awareness.aware {
        return Strike::Refused("they know you are there");
    }
    if dist_m > REACH_M {
        return Strike::Refused("too far");
    }
    if target.kind == Hunter::Apex {
        return Strike::Wounded(APEX_WOUND);
    }
    Strike::Killed(if descending {
        Style::Aerial
    } else if spark.sneaking {
        Style::Ghost
    } else {
        Style::Backbreaker
    })
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("STEALTH — two ways to be found, wanting opposite things\n");

    println!("  SIGHT: range, a cone, and whatever is in the way.");
    for (who, human, sneak) in
        [("a bot, walking", false, false), ("a bot, sneaking", false, true), ("a human", true, false)]
    {
        println!("    {:<18} seen at {:.0} m", who, sight_range_m(human, sneak, 0.0));
    }
    println!("    {:<18} seen at {:.0} m   (the storm eats sightlines)", "a bot, in a storm", sight_range_m(false, false, 0.9));

    println!("\n  RESONANCE: the Corrupted feel a spark working. No cone, no line of");
    println!("  sight, through walls.");
    for (label, loud, sneak, cloak) in [
        ("still and quiet", 0.05f32, false, false),
        ("moving", 0.35, false, false),
        ("firing", 0.80, false, false),
        ("firing, sneaking", 0.80, true, false),
        ("firing, cloaked+sneaking", 0.80, true, true),
    ] {
        let s = Spark { loud, sneaking: sneak, cloaked: cloak };
        let f = s.felt_m();
        println!(
            "    {:<26} felt at {}",
            label,
            if f <= 0.0 { "— nothing".to_string() } else { format!("{f:.0} m") }
        );
    }

    println!("\n  THE TRAP: behind a wall, still firing.\n");
    let spark = Spark { loud: 0.8, ..Default::default() };
    for (kind, name) in [(Hunter::Sighted, "a sentry"), (Hunter::Resonant, "the corrupted")] {
        let mut o = Observer {
            kind,
            pos: Vec3::new(50.0, 0.0, 0.0),
            heading: Vec3::new(-1.0, 0.0, 0.0),
            awareness: Awareness::default(), certainty: 0.0,
        };
        let got = o.sense(1.0 / 60.0, Vec3::ZERO, &spark, true, false, 0.0);
        println!("    {:<16} {:?}", name, got);
    }
    println!("\n  the geometry that beats a sentry is nothing to a hunter, and the");
    println!("  stillness that beats a hunter leaves you in plain view. A player who");
    println!("  has learned one enemy has learned the wrong lesson about the other.");

    // The static table above is the arithmetic; this is the loop. A spark is
    // something you spend and wait out, and the waiting is the gameplay.
    println!("\n  GOING QUIET — one burst, then stop, 30 m from a corrupted\n");
    for (label, sneak, cloak) in
        [("plain", false, false), ("sneaking", true, false), ("cloaked+sneaking", true, true)]
    {
        let mut sp = Spark { sneaking: sneak, cloaked: cloak, ..Default::default() };
        sp.act(0.9);
        let mut o = Observer {
            kind: Hunter::Resonant,
            pos: Vec3::new(30.0, 0.0, 0.0),
            heading: Vec3::X, // looking away: resonance only
            awareness: Awareness::default(),
            certainty: 0.0,
        };
        let (mut t, mut clear_at) = (0.0f32, None);
        for _ in 0..(60 * 15) {
            t += 1.0 / 60.0;
            sp.decay(1.0 / 60.0);
            let felt = o.sense(1.0 / 60.0, Vec3::ZERO, &sp, true, false, 0.0) == Sensed::Resonance;
            if !felt && clear_at.is_none() {
                clear_at = Some(t);
            }
            if clear_at.is_some() && !o.awareness.aware {
                break;
            }
        }
        println!(
            "    {:<20} unfelt after {:>4.1}s, stops hunting after {:>4.1}s",
            label,
            clear_at.unwrap_or(f32::NAN),
            t
        );
    }
    println!("\n  the {LOSE_S:.1}s tail is why stopping is not enough on its own: you go");
    println!("  quiet and it keeps coming to where you were.");

    println!("\n  ASSASSINATION — unaware, within {REACH_M:.1} m, at any health\n");
    for (label, kind, aware, dist, descending, sneaking) in [
        ("from above", Hunter::Sighted, false, 3.0f32, true, false),
        ("sneaking up", Hunter::Sighted, false, 3.0, false, true),
        ("walking up", Hunter::Sighted, false, 3.0, false, false),
        ("they turned round", Hunter::Sighted, true, 3.0, false, true),
        ("too far", Hunter::Sighted, false, 20.0, false, true),
        ("an apex frame", Hunter::Apex, false, 3.0, false, true),
    ] {
        let t = Observer {
            kind,
            pos: Vec3::ZERO,
            heading: Vec3::X,
            awareness: Awareness { aware, losing: 0.0, startled: false }, certainty: 0.0,
        };
        let s = Spark { sneaking, ..Default::default() };
        let r = assassinate(&t, dist, descending, &s);
        let note = match r {
            Strike::Killed(st) if st.silent() => "  (silent — the pack never knows)",
            _ => "",
        };
        println!("    {:<20} {:?}{note}", label, r);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Tick until this observer sees the player, or give up. Returns seconds.
    fn time_to_see(o: &mut Observer, player: Vec3, spark: &Spark, blocked: bool) -> Option<f32> {
        for i in 0..(60 * 30) {
            if o.sense(1.0 / 60.0, player, spark, blocked, false, 0.0) == Sensed::Sight {
                return Some(i as f32 / 60.0);
            }
        }
        None
    }

    fn watcher(kind: Hunter, at: Vec3, facing: Vec3) -> Observer {
        Observer { kind, pos: at, heading: facing, awareness: Awareness::default(), certainty: 0.0 }
    }

    #[test]
    fn a_wall_beats_sight_and_does_nothing_against_resonance() {
        // THE WHOLE POINT. Same position, same noise, opposite outcomes.
        let loud = Spark { loud: 0.8, ..Default::default() };
        let mut sentry = watcher(Hunter::Sighted, Vec3::new(50.0, 0.0, 0.0), -Vec3::X);
        let mut corrupted = watcher(Hunter::Resonant, Vec3::new(50.0, 0.0, 0.0), -Vec3::X);
        assert_eq!(sentry.sense(0.016, Vec3::ZERO, &loud, true, false, 0.0), Sensed::No);
        assert_eq!(
            corrupted.sense(0.016, Vec3::ZERO, &loud, true, false, 0.0),
            Sensed::Resonance,
            "a wall should not stop a thing that hunts by feel"
        );
    }

    #[test]
    fn going_quiet_beats_resonance_and_does_nothing_against_sight() {
        // and the mirror of it
        let quiet = Spark { loud: 0.0, ..Default::default() };
        let mut corrupted = watcher(Hunter::Resonant, Vec3::new(30.0, 0.0, 0.0), -Vec3::X);
        let mut sentry = watcher(Hunter::Sighted, Vec3::new(30.0, 0.0, 0.0), -Vec3::X);
        assert!(
            time_to_see(&mut corrupted, Vec3::ZERO, &quiet, false).is_some(),
            "a resonant hunter still has eyes"
        );
        // ... but it did not feel anything; it merely looked
        let mut deaf = watcher(Hunter::Resonant, Vec3::new(30.0, 0.0, 0.0), -Vec3::X);
        assert_eq!(deaf.sense(0.016, Vec3::ZERO, &quiet, true, false, 0.0), Sensed::No,
            "behind a wall and silent should be invisible to everything");
        assert!(time_to_see(&mut sentry, Vec3::ZERO, &quiet, false).is_some());
    }

    #[test]
    fn being_seen_takes_time_and_less_of_it_up_close() {
        // Sight is a RATE, delegated to `perception` — being inside the cone
        // is permission to start noticing, not the noticing. The first
        // version treated in-cone as instantly seen, which made the whole
        // claim that these two modules compose into a decoration.
        let quiet = Spark::default();
        let mut near = watcher(Hunter::Sighted, Vec3::new(12.0, 0.0, 0.0), -Vec3::X);
        let mut far = watcher(Hunter::Sighted, Vec3::new(70.0, 0.0, 0.0), -Vec3::X);
        let tn = time_to_see(&mut near, Vec3::ZERO, &quiet, false).expect("seen up close");
        let tf = time_to_see(&mut far, Vec3::ZERO, &quiet, false).expect("seen eventually");
        assert!(tn > 0.0, "it was instant, so nothing is being modelled");
        assert!(tf > tn * 4.0, "distance should cost far more: {tf} vs {tn}");

        // and sneaking buys time even inside the cone
        let sneak = Spark { sneaking: true, ..Default::default() };
        let mut a = watcher(Hunter::Sighted, Vec3::new(20.0, 0.0, 0.0), -Vec3::X);
        let mut b = watcher(Hunter::Sighted, Vec3::new(20.0, 0.0, 0.0), -Vec3::X);
        let walk = time_to_see(&mut a, Vec3::ZERO, &quiet, false).unwrap();
        let creep = time_to_see(&mut b, Vec3::ZERO, &sneak, false).unwrap();
        assert!(creep > walk, "sneaking should buy time: {creep} vs {walk}");
    }

    #[test]
    fn facing_matters_to_eyes_and_not_to_a_spark() {
        let loud = Spark { loud: 0.9, ..Default::default() };
        // looking the other way
        let mut behind = watcher(Hunter::Sighted, Vec3::new(30.0, 0.0, 0.0), Vec3::X);
        assert_eq!(behind.sense(0.016, Vec3::ZERO, &loud, false, false, 0.0), Sensed::No);
        let mut felt = watcher(Hunter::Resonant, Vec3::new(30.0, 0.0, 0.0), Vec3::X);
        assert_eq!(felt.sense(0.016, Vec3::ZERO, &loud, false, false, 0.0), Sensed::Resonance);
    }

    #[test]
    fn sneaking_and_cloak_shrink_both_channels() {
        assert!(sight_range_m(false, true, 0.0) < sight_range_m(false, false, 0.0));
        let plain = Spark { loud: 0.8, ..Default::default() };
        let sneak = Spark { loud: 0.8, sneaking: true, cloaked: false };
        let both = Spark { loud: 0.8, sneaking: true, cloaked: true };
        assert!(sneak.felt_m() < plain.felt_m());
        assert!(both.felt_m() < sneak.felt_m());
        // and a human is harder to see than a bot
        assert!(sight_range_m(true, false, 0.0) < sight_range_m(false, false, 0.0));
    }

    #[test]
    fn a_quiet_enough_spark_is_felt_by_nobody() {
        let s = Spark { loud: RESONANCE_FLOOR * 0.5, ..Default::default() };
        assert_eq!(s.felt_m(), 0.0);
        let mut o = watcher(Hunter::Apex, Vec3::new(1.0, 0.0, 0.0), -Vec3::X);
        // right next to an apex frame, behind a wall, doing nothing
        assert_eq!(o.sense(0.016, Vec3::ZERO, &s, true, false, 0.0), Sensed::No);
    }

    #[test]
    fn acting_lights_you_up_and_stillness_puts_it_out() {
        let mut s = Spark::default();
        s.act(0.9);
        assert!(s.felt_m() > 100.0, "firing should carry: {}", s.felt_m());
        for _ in 0..60 {
            s.decay(1.0 / 60.0);
        }
        assert!(s.felt_m() < 90.0, "a second of quiet should help");
        // sneaking quiets faster
        let mut a = Spark { loud: 1.0, ..Default::default() };
        let mut b = Spark { loud: 1.0, sneaking: true, ..Default::default() };
        for _ in 0..30 {
            a.decay(1.0 / 60.0);
            b.decay(1.0 / 60.0);
        }
        assert!(b.loud < a.loud, "a creeping frame runs cold");
    }

    #[test]
    fn awareness_has_hysteresis_or_a_pillar_resets_the_hunt() {
        // stepping out of sight for one frame must not clear an alert, or the
        // chase becomes a strobe — the same failure the amalgam's sweep had
        let mut a = Awareness::default();
        a.step(0.016, true);
        assert!(a.aware && a.startled, "it should be startled the first time");
        a.step(0.016, true);
        assert!(!a.startled, "startled is one tick, not a state");
        for _ in 0..60 {
            a.step(1.0 / 60.0, false);
        }
        assert!(a.aware, "one second behind a pillar cleared the hunt");
        for _ in 0..((LOSE_S + 0.5) * 60.0) as i32 {
            a.step(1.0 / 60.0, false);
        }
        assert!(!a.aware, "it never went back to searching");
    }

    #[test]
    fn you_cannot_assassinate_someone_who_knows() {
        let mut t = watcher(Hunter::Sighted, Vec3::ZERO, Vec3::X);
        t.awareness.aware = true;
        let s = Spark { sneaking: true, ..Default::default() };
        assert!(matches!(assassinate(&t, 2.0, false, &s), Strike::Refused(_)));
        t.awareness.aware = false;
        assert!(matches!(assassinate(&t, 2.0, false, &s), Strike::Killed(_)));
        assert!(matches!(assassinate(&t, REACH_M + 1.0, false, &s), Strike::Refused(_)));
    }

    #[test]
    fn the_approach_picks_the_style_and_only_the_ghost_is_silent() {
        let t = watcher(Hunter::Sighted, Vec3::ZERO, Vec3::X);
        let sneaking = Spark { sneaking: true, ..Default::default() };
        let walking = Spark::default();
        assert_eq!(assassinate(&t, 2.0, true, &walking), Strike::Killed(Style::Aerial));
        assert_eq!(assassinate(&t, 2.0, false, &sneaking), Strike::Killed(Style::Ghost));
        assert_eq!(assassinate(&t, 2.0, false, &walking), Strike::Killed(Style::Backbreaker));
        // descending wins over sneaking: you are in the air, not creeping
        assert_eq!(assassinate(&t, 2.0, true, &sneaking), Strike::Killed(Style::Aerial));

        assert!(Style::Ghost.silent());
        assert!(!Style::Aerial.silent() && !Style::Backbreaker.silent());
    }

    #[test]
    fn an_apex_frame_is_wounded_rather_than_killed() {
        let t = watcher(Hunter::Apex, Vec3::ZERO, Vec3::X);
        let s = Spark { sneaking: true, ..Default::default() };
        match assassinate(&t, 2.0, false, &s) {
            Strike::Wounded(f) => assert!((f - APEX_WOUND).abs() < 1e-6),
            other => panic!("you cannot assassinate a god: {other:?}"),
        }
        // and it still refuses when they know
        let mut awake = t;
        awake.awareness.aware = true;
        assert!(matches!(assassinate(&awake, 2.0, false, &s), Strike::Refused(_)));
    }
}

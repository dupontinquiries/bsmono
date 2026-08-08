//! THE CLINCH — being grabbed out of the air, and what you do about it.
//!
//! The C engine's melee is `ex_enemy_melee_damage` and a flag for whether it
//! transmits corruption. There is no grab to port, so this is a design, and
//! the brief was that grabs should not feel random. That word is worth
//! taking seriously, because "random" almost never means "the RNG got me" —
//! it means **I could not have seen it coming and I could not have done
//! anything about it**. So every condition below is something the player can
//! observe, and every escape is something the player does rather than rolls.
//!
//! FOUR THINGS MUST ALL BE TRUE for a grab to land:
//!
//!   1. **Both airborne.** A grab is a thing that happens in the sky. On the
//!      ground you are in a fist fight, which is a different problem.
//!   2. **Close enough**, and it is a hard radius rather than a probability.
//!   3. **Closing.** Someone drifting alongside you cannot grab you; they
//!      have to commit to the intercept, which means they are visibly
//!      accelerating toward you before it happens.
//!   4. **The tell has played.** The attacker TRANSFORMS to grab — it needs
//!      hands — and the fold takes `TELL_SEC`. That is the whole answer to
//!      "it came from nowhere": something the size of a car unfolded into a
//!      robot in front of you first, and if you broke the range or the
//!      closing during the fold, it whiffs.
//!
//! Then the choice, which is the actual mechanic:
//!
//!   * **Fly on.** Keep your thrust, wear the damage, and SHAKE them off by
//!     rolling. Alternating roll builds `shake`; holding one direction does
//!     not, because a sustained roll is something they can brace against and
//!     a reversal is not. It is a rhythm, so it rewards a player who has
//!     learned the rate.
//!   * **Transform and fight.** Fold to robot mid-air and brawl in
//!     quicktimes. Each read costs grip and hurts them; each miss hurts you.
//!     Faster than shaking, and worse if you are bad at it.
//!
//! Both end the same three ways: grip broken, someone out of health, or the
//! ground arrives — and the ground does not care who was winning.

/// How long the attacker's fold takes. This is the tell, and it is generous
/// on purpose: a grab you cannot react to is the thing being fixed.
pub const TELL_SEC: f32 = 0.55;
/// Hard grab radius, metres.
pub const REACH: f32 = 6.5;
/// Minimum closing speed, m/s. Drifting alongside is not an attack.
pub const CLOSING_MIN: f32 = 4.0;
/// A roll reversal only counts as a shake if it comes within this window of
/// the last one — that is what makes it a rhythm rather than a mash.
pub const SHAKE_WINDOW: f32 = 0.42;
/// Grip is the health of the hold.
pub const GRIP_MAX: f32 = 1.0;
/// Damage per second while you keep flying with someone attached.
pub const DRAG_DPS: f32 = 9.0;
/// How long a quicktime prompt stays open.
pub const QTE_WINDOW: f32 = 0.75;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// nothing is happening
    Free,
    /// the attacker is folding — this is the window to break it up
    Telegraphing,
    /// held, and the player has not chosen yet
    Held,
    /// flying on, taking damage, trying to roll them off
    Shaking,
    /// folded to robot, trading quicktimes
    Brawling,
    /// over
    Broken,
}

/// Which way a quicktime wants you to move. Four directions, so it reads at
/// a glance and maps to a stick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Cue {
    Left,
    Right,
    Up,
    Down,
}

impl Cue {
    fn from_seed(s: u64) -> Cue {
        match s % 4 {
            0 => Cue::Left,
            1 => Cue::Right,
            2 => Cue::Up,
            _ => Cue::Down,
        }
    }
}

/// What happened this tick, for the caller to turn into effects and damage.
#[derive(Default, Debug, Clone, Copy)]
pub struct Tick {
    /// damage to the player
    pub player_dmg: f32,
    /// damage to the attacker
    pub enemy_dmg: f32,
    /// a quicktime just opened
    pub cue: Option<Cue>,
    /// the hold just ended, and whether the player got out on their own
    pub released: bool,
    pub player_escaped: bool,
    /// a shake reversal registered — worth a lurch and a sound
    pub shook: bool,
}

pub struct Clinch {
    pub phase: Phase,
    pub grip: f32,
    /// 0..1 toward breaking free by rolling
    pub shake: f32,
    timer: f32,
    /// sign of the last roll input that counted
    last_roll: f32,
    since_reversal: f32,
    /// live quicktime, if any
    pub cue: Option<Cue>,
    cue_t: f32,
    rng: u64,
}

impl Default for Clinch {
    fn default() -> Clinch {
        Clinch::new()
    }
}

impl Clinch {
    pub fn new() -> Clinch {
        Clinch {
            phase: Phase::Free,
            grip: GRIP_MAX,
            shake: 0.0,
            timer: 0.0,
            last_roll: 0.0,
            since_reversal: 0.0,
            cue: None,
            cue_t: 0.0,
            rng: 0x2545_F491_4F6C_DD1D,
        }
    }

    fn rand(&mut self) -> u64 {
        self.rng ^= self.rng << 13;
        self.rng ^= self.rng >> 7;
        self.rng ^= self.rng << 17;
        self.rng
    }

    /// EVERY CONDITION FOR A GRAB, in one place so it can be read.
    ///
    /// Returns why not, rather than a bare bool, because "it came from
    /// nowhere" is usually a UI failure and the reason is what the HUD
    /// should be showing.
    pub fn can_attempt(
        &self,
        player_airborne: bool,
        enemy_airborne: bool,
        dist: f32,
        closing: f32,
    ) -> Result<(), &'static str> {
        if self.phase != Phase::Free {
            return Err("already engaged");
        }
        if !player_airborne || !enemy_airborne {
            return Err("a grab is an air move");
        }
        if dist > REACH {
            return Err("out of reach");
        }
        if closing < CLOSING_MIN {
            return Err("not closing");
        }
        Ok(())
    }

    /// Begin the tell. The attacker folds; the player has TELL_SEC to break
    /// range or stop the closing.
    pub fn telegraph(
        &mut self,
        player_airborne: bool,
        enemy_airborne: bool,
        dist: f32,
        closing: f32,
    ) -> bool {
        if self.can_attempt(player_airborne, enemy_airborne, dist, closing).is_err() {
            return false;
        }
        self.phase = Phase::Telegraphing;
        self.timer = TELL_SEC;
        self.grip = GRIP_MAX;
        self.shake = 0.0;
        true
    }

    /// The player commits to riding it out and rolling them off.
    pub fn choose_shake(&mut self) -> bool {
        if self.phase != Phase::Held {
            return false;
        }
        self.phase = Phase::Shaking;
        true
    }

    /// The player folds to robot and fights.
    pub fn choose_brawl(&mut self) -> bool {
        if self.phase != Phase::Held {
            return false;
        }
        self.phase = Phase::Brawling;
        self.cue = None;
        self.cue_t = 0.0;
        true
    }

    /// Feed the roll axis, -1..1. Only REVERSALS count, and only if they
    /// come inside the window: a held roll is something they can brace
    /// against, and a mash in one direction is not a shake.
    pub fn roll(&mut self, axis: f32) -> bool {
        if self.phase != Phase::Shaking {
            return false;
        }
        let s = if axis > 0.35 {
            1.0
        } else if axis < -0.35 {
            -1.0
        } else {
            return false;
        };
        if self.last_roll == 0.0 {
            self.last_roll = s;
            self.since_reversal = 0.0;
            return false;
        }
        if s == self.last_roll {
            return false;
        }
        let in_rhythm = self.since_reversal <= SHAKE_WINDOW;
        self.last_roll = s;
        self.since_reversal = 0.0;
        // a reversal always helps a little; one in rhythm helps three times
        // as much, so there is something to get good at
        self.shake = (self.shake + if in_rhythm { 0.16 } else { 0.05 }).min(1.5);
        true
    }

    /// Answer a quicktime.
    ///
    /// Right and in time: their grip goes and they take it. Wrong, or after
    /// the window, and you eat one — brawling is faster than shaking and
    /// worse if you are bad at it, which is the trade.
    pub fn answer(&mut self, c: Cue) -> Option<Tick> {
        if self.phase != Phase::Brawling {
            return None;
        }
        let want = self.cue?;
        let mut t = Tick::default();
        if c == want {
            self.grip -= 0.30;
            t.enemy_dmg = 11.0;
        } else {
            self.grip = (self.grip + 0.06).min(GRIP_MAX);
            t.player_dmg = 7.0;
        }
        self.cue = None;
        self.cue_t = 0.0;
        Some(t)
    }

    /// Advance. `grounded` ends everything — the ground does not care who
    /// was winning.
    pub fn update(&mut self, dt: f32, grounded: bool, still_in_reach: bool) -> Tick {
        let mut out = Tick::default();
        self.since_reversal += dt;

        if self.phase == Phase::Free || self.phase == Phase::Broken {
            return out;
        }

        if grounded {
            // impact ends it, and both of them wear the landing
            out.player_dmg += 6.0;
            out.enemy_dmg += 6.0;
            out.released = true;
            self.phase = Phase::Broken;
            return out;
        }

        match self.phase {
            Phase::Telegraphing => {
                self.timer -= dt;
                if !still_in_reach {
                    // broke the range during the fold: it whiffs, and that is
                    // the reward for reading the tell
                    self.phase = Phase::Broken;
                    out.released = true;
                    out.player_escaped = true;
                } else if self.timer <= 0.0 {
                    self.phase = Phase::Held;
                }
            }
            Phase::Held => {
                // undecided still costs you: hesitation is a choice
                out.player_dmg += DRAG_DPS * 0.5 * dt;
            }
            Phase::Shaking => {
                out.player_dmg += DRAG_DPS * dt;
                // shake decays, so stopping loses ground
                self.shake = (self.shake - dt * 0.35).max(0.0);
                if self.shake >= self.grip {
                    self.phase = Phase::Broken;
                    out.released = true;
                    out.player_escaped = true;
                    out.shook = true;
                }
            }
            Phase::Brawling => {
                // BREAK FIRST. Checking the grip after issuing a prompt let
                // the last winning read open a cue and end the fight in the
                // same tick, so the player was answering something that no
                // longer existed.
                if self.grip <= 0.0 {
                    self.cue = None;
                    self.phase = Phase::Broken;
                    out.released = true;
                    out.player_escaped = true;
                    return out;
                }
                if self.cue.is_none() {
                    self.cue_t -= dt;
                    if self.cue_t <= 0.0 {
                        let s = self.rand();
                        let c = Cue::from_seed(s >> 17);
                        self.cue = Some(c);
                        self.cue_t = QTE_WINDOW;
                        out.cue = Some(c);
                    }
                } else {
                    self.cue_t -= dt;
                    if self.cue_t <= 0.0 {
                        // missed it entirely
                        self.cue = None;
                        self.cue_t = 0.25;
                        out.player_dmg += 7.0;
                        self.grip = (self.grip + 0.06).min(GRIP_MAX);
                    }
                }
            }
            _ => {}
        }
        out
    }

    /// Ready for another engagement.
    pub fn reset(&mut self) {
        *self = Clinch::new();
    }

    /// Is the player's own flight being dragged right now?
    pub fn dragging(&self) -> bool {
        matches!(self.phase, Phase::Held | Phase::Shaking | Phase::Brawling)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn engaged() -> Clinch {
        let mut c = Clinch::new();
        assert!(c.telegraph(true, true, 4.0, 9.0));
        for _ in 0..((TELL_SEC * 60.0) as i32 + 2) {
            c.update(1.0 / 60.0, false, true);
        }
        assert_eq!(c.phase, Phase::Held);
        c
    }

    #[test]
    fn a_grab_needs_all_four_conditions_and_says_which_is_missing() {
        let c = Clinch::new();
        assert_eq!(c.can_attempt(false, true, 3.0, 9.0).unwrap_err(), "a grab is an air move");
        assert_eq!(c.can_attempt(true, false, 3.0, 9.0).unwrap_err(), "a grab is an air move");
        assert_eq!(c.can_attempt(true, true, REACH + 1.0, 9.0).unwrap_err(), "out of reach");
        assert_eq!(c.can_attempt(true, true, 3.0, 1.0).unwrap_err(), "not closing");
        assert!(c.can_attempt(true, true, 3.0, 9.0).is_ok());
    }

    #[test]
    fn drifting_alongside_is_not_an_attack() {
        // the specific complaint: something appears next to you and suddenly
        // has you. Closing speed is what makes it a commitment.
        let mut c = Clinch::new();
        assert!(!c.telegraph(true, true, 2.0, 0.5));
        assert_eq!(c.phase, Phase::Free);
    }

    #[test]
    fn reading_the_tell_makes_it_whiff() {
        let mut c = Clinch::new();
        c.telegraph(true, true, 4.0, 9.0);
        assert_eq!(c.phase, Phase::Telegraphing);
        // break the range while it is folding
        let t = c.update(0.1, false, false);
        assert_eq!(c.phase, Phase::Broken);
        assert!(t.player_escaped, "outrunning the tell did not save you");
        assert_eq!(t.player_dmg, 0.0);
    }

    #[test]
    fn the_tell_is_long_enough_to_react_to() {
        // it is only a fair mechanic if a human can act inside it
        assert!(TELL_SEC > 0.4, "the tell is shorter than a reaction time");
    }

    #[test]
    fn hesitating_costs_you_but_less_than_committing_wrong() {
        let mut c = engaged();
        let mut held = 0.0;
        for _ in 0..60 {
            held += c.update(1.0 / 60.0, false, true).player_dmg;
        }
        assert!(held > 0.0, "standing still in a hold is free");
        assert!(held < DRAG_DPS, "hesitating hurts as much as flying on");
    }

    #[test]
    fn alternating_rolls_shake_them_off_and_mashing_does_not() {
        let mut c = engaged();
        c.choose_shake();
        // holding one direction: nothing, however long you hold it
        for _ in 0..120 {
            c.roll(1.0);
            c.update(1.0 / 60.0, false, true);
        }
        assert!(c.shake < 0.05, "a held roll shook them: {}", c.shake);

        // now alternate, in rhythm
        let mut d = engaged();
        d.choose_shake();
        let mut escaped = false;
        let mut dir = 1.0;
        for i in 0..600 {
            if i % 12 == 0 {
                d.roll(dir);
                dir = -dir;
            }
            if d.update(1.0 / 60.0, false, true).player_escaped {
                escaped = true;
                break;
            }
        }
        assert!(escaped, "rhythmic rolling never broke the grip: {}", d.shake);
    }

    #[test]
    fn shaking_decays_so_stopping_loses_ground() {
        let mut c = engaged();
        c.choose_shake();
        c.roll(1.0);
        c.roll(-1.0);
        let peak = c.shake;
        assert!(peak > 0.0);
        for _ in 0..120 {
            c.update(1.0 / 60.0, false, true);
        }
        assert!(c.shake < peak, "shake did not decay while idle");
    }

    #[test]
    fn brawling_is_faster_than_shaking_if_you_read_it() {
        let mut c = engaged();
        c.choose_brawl();
        let mut frames = 0;
        let mut out = false;
        for _ in 0..2000 {
            frames += 1;
            let t = c.update(1.0 / 60.0, false, true);
            if let Some(cue) = t.cue {
                let r = c.answer(cue).unwrap();
                assert!(r.enemy_dmg > 0.0 && r.player_dmg == 0.0, "a correct read hurt me");
            }
            if c.phase == Phase::Broken {
                out = true;
                break;
            }
        }
        assert!(out, "perfect play never escaped the brawl");
        assert!(frames < 600, "a clean brawl took {frames} frames");
    }

    #[test]
    fn getting_the_quicktime_wrong_makes_it_worse() {
        let mut c = engaged();
        c.choose_brawl();
        let mut hurt = 0.0;
        for _ in 0..600 {
            let t = c.update(1.0 / 60.0, false, true);
            hurt += t.player_dmg;
            if let Some(cue) = t.cue {
                // answer the opposite every time
                let wrong = if cue == Cue::Left { Cue::Right } else { Cue::Left };
                hurt += c.answer(wrong).unwrap().player_dmg;
            }
        }
        assert!(hurt > 20.0, "fumbling every prompt was painless: {hurt}");
        assert_eq!(c.phase, Phase::Brawling, "bad play still escaped");
        assert!(c.grip >= GRIP_MAX - 1e-3, "grip loosened despite missing everything");
    }

    #[test]
    fn ignoring_the_prompts_entirely_still_costs() {
        let mut c = engaged();
        c.choose_brawl();
        let mut hurt = 0.0;
        for _ in 0..600 {
            hurt += c.update(1.0 / 60.0, false, true).player_dmg;
        }
        assert!(hurt > 15.0, "letting every prompt lapse was free: {hurt}");
    }

    #[test]
    fn the_ground_ends_it_regardless_of_who_was_winning() {
        for choose_brawl in [false, true] {
            let mut c = engaged();
            if choose_brawl {
                c.choose_brawl();
            } else {
                c.choose_shake();
            }
            let t = c.update(1.0 / 60.0, true, true);
            assert_eq!(c.phase, Phase::Broken);
            assert!(t.released);
            // both of them wear the landing
            assert!(t.player_dmg > 0.0 && t.enemy_dmg > 0.0);
            assert!(!t.player_escaped, "hitting the ground counted as an escape");
        }
    }

    #[test]
    fn you_cannot_choose_before_you_are_held_or_twice() {
        let mut c = Clinch::new();
        assert!(!c.choose_shake(), "chose a response with nobody holding me");
        c.telegraph(true, true, 4.0, 9.0);
        assert!(!c.choose_brawl(), "chose during the tell");
        let mut c = engaged();
        assert!(c.choose_shake());
        assert!(!c.choose_brawl(), "switched stance mid-clinch");
    }

    #[test]
    fn a_second_grab_cannot_start_while_one_is_running() {
        let c = engaged();
        assert_eq!(c.can_attempt(true, true, 2.0, 9.0).unwrap_err(), "already engaged");
    }
}

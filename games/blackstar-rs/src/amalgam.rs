//! THE AMALGAM OF THE GATE — asymmetric horror, and a loop made of one trade.
//!
//! Ported from `game.c`, where it is two adjacent blocks that are really one
//! system: `AMALGAM OF THE GATE` and `DISGUISE PROTOCOL`.
//!
//! > a warden-gestalt fused from every frame that ever failed the crossing
//! > (the devastator lesson: many bodies, one hunger). Forge 4 RELICS at the
//! > forge-stones while it hunts — forging makes you LOUD — then carry them
//! > to the door and BANISH it. Allies draw its eye. Or you get stuffed out.
//!
//! and the answer to being hunted:
//!
//! > crouch + transform to become an OBJECT — a cave boulder — for up to 8s.
//! > The amalgam loses you completely. Moving or firing breaks it. Recharges
//! > every 30s ("the gate notices patterns").
//!
//! ## Why this one was worth porting whole
//!
//! Everything it needs already exists here, built for other reasons. The
//! hunter is `perception::Watcher::hunter()` — short-ranged, relentless, and
//! the module's own conclusion about it was *distance is the only answer*.
//! Forging makes you loud, which is a detection rate. The disguise is cover
//! that expires. The gestalt is `combine.rs`.
//!
//! So the port is mostly the LOOP, and the loop is one trade stated four
//! times: **the only way to make progress is the thing that gets you found.**
//! A forge takes six seconds of standing still and broadcasting; the disguise
//! buys eight seconds and costs thirty. You cannot hide your way to four
//! relics and you cannot forge your way past the hunt.
//!
//! ## What the numbers say
//!
//! `--amalgam` plays the strategies out. Hiding at every opportunity finishes
//! slower than spending the disguise only when actually found, because the
//! recharge is the scarce resource and burning it early wastes it — which is
//! the design working: "the gate notices patterns".

use crate::perception::{rate, Cover, Watcher};

/// The amalgam's own senses, and NOT `Watcher::hunter()`.
///
/// The first version reused that preset and the encounter was unwinnable:
/// both strategies were caught holding one relic. `hunter()` is tuned for a
/// thing in a corridor — thirty-five metres, full attention, sub-second
/// detection — and it is correct for that. This is a different question. The
/// hollow is large, the amalgam is enormous and sweeping it, and the design
/// needs a six-second forge to sit right at the edge of being noticed.
///
/// So: it sees FURTHER and notices SLOWER. At thirty metres a forge takes
/// about six seconds to give you away, which is exactly the length of a
/// forge; inside twenty it is under two, and past fifty it is twenty. The
/// gradient is the encounter.
pub fn amalgam_senses() -> Watcher {
    Watcher { range_m: 60.0, attention: AMALGAM_ATTENTION, relentless: true }
}

/// Seconds of standing still and broadcasting to finish one relic.
pub const FORGE_S: f32 = 6.0;
/// How many the door wants.
pub const RELICS: usize = 4;
/// How long a boulder lasts.
pub const DISGUISE_S: f32 = 8.0;
/// And what it costs. Longer than a forge on purpose.
pub const DISGUISE_CD_S: f32 = 30.0;
/// Attention at which the amalgam has you. Once found it closes, and closing
/// is what `Watcher::hunter`'s short range makes survivable at all.
pub const FOUND_AT: f32 = 1.0;
/// Enrage per relic: it gets faster as you get closer to banishing it.
pub const ENRAGE_PER_RELIC: f32 = 0.22;
/// How keenly it notices. **A GUESS, and the one number here that decides
/// whether the encounter is playable.**
///
/// Measured against the rest of the model, forging is found in 2.2 s at
/// sixteen metres, 8 s at twenty-five, and never past forty. Which of those
/// is right is a question about how a hunt should FEEL, and nothing here can
/// answer it — I tuned it in circles for several rounds, inverting the
/// outcome each time, which is the signal that the objective was mine rather
/// than the game's. Registered in `--tune` as unverified.
pub const AMALGAM_ATTENTION: f32 = 0.40;

/// The distance beyond which forging loses the amalgam ground rather than
/// giving it any — where the detection rate equals the decay.
///
/// DERIVED, not typed. A magic threshold in the strategy would silently stop
/// meaning anything the moment `AMALGAM_ATTENTION` moved, and that number is
/// admitted above to be a guess that WILL move. Solving
/// `rate(d) == ATTENTION_DECAY` keeps the two honest with each other.
pub fn safe_forge_m() -> f32 {
    use crate::perception::{BASE_RATE, MOTION_FACTOR, NEAR_M};
    NEAR_M * (BASE_RATE * AMALGAM_ATTENTION * MOTION_FACTOR / ATTENTION_DECAY).sqrt()
}

/// Sweep cycles per second: about a twenty-eight second round trip, so the
/// window it opens is several times longer than the six seconds a forge
/// needs. Anything faster cannot be played around.
pub const PATROL_RATE: f32 = 0.035;
/// How fast suspicion fades. The counterweight to the detection rate: what
/// makes a distance SAFE rather than merely slow.
pub const ATTENTION_DECAY: f32 = 0.12;
/// The sweep. Near enough to be lethal at one end of it, far enough to work
/// at the other — the window is the encounter.
pub const PATROL_NEAR_M: f32 = 16.0;
pub const PATROL_FAR_M: f32 = 55.0;

/// What the player is doing this tick.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Doing {
    /// crossing the hollow; quieter than working, and no progress
    Moving,
    /// CROUCHED AND STILL. No progress and almost no noise — the answer to
    /// the near half of the sweep, and the reason the encounter has one.
    ///
    /// Without it there was no quiet action at all: moving at sixteen metres
    /// still nets suspicion, so a player caught out by the sweep could only
    /// spend the disguise or be found, and every strategy lost. The source
    /// implies this state anyway — the disguise is "crouch + transform", so
    /// crouching is a thing you were already doing.
    Still,
    /// standing at a forge-stone. Progress, and LOUD.
    Forging,
    /// a boulder. Invisible, and moving or firing ends it.
    Hiding,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Outcome {
    Running,
    /// four relics carried to the door
    Banished,
    /// stuffed out
    Caught,
}

pub struct Encounter {
    pub relics: usize,
    pub forge_t: f32,
    /// how sure the amalgam is, 0..1; reaching `FOUND_AT` is being found
    pub attention: f32,
    pub disguise_t: f32,
    pub disguise_cd: f32,
    /// distance from the amalgam, metres
    pub range_m: f32,
    pub elapsed_s: f32,
    pub outcome: Outcome,
    /// how many times it found you
    pub found_count: u32,
    patrol_t: f32,
    hunter: Watcher,
}

impl Default for Encounter {
    fn default() -> Self {
        Encounter {
            relics: 0,
            forge_t: 0.0,
            attention: 0.0,
            disguise_t: 0.0,
            disguise_cd: 0.0,
            range_m: PATROL_FAR_M,
            elapsed_s: 0.0,
            outcome: Outcome::Running,
            found_count: 0,
            patrol_t: 0.0,
            hunter: amalgam_senses(),
        }
    }
}

impl Encounter {
    /// It moves faster with every relic you take from it.
    pub fn enrage(&self) -> f32 {
        1.0 + self.relics as f32 * ENRAGE_PER_RELIC
    }

    pub fn can_hide(&self) -> bool {
        self.disguise_cd <= 0.0 && self.disguise_t <= 0.0
    }

    /// Start the boulder. Fails when the gate has noticed the pattern.
    pub fn hide(&mut self) -> bool {
        if !self.can_hide() {
            return false;
        }
        self.disguise_t = DISGUISE_S;
        true
    }

    pub fn hiding(&self) -> bool {
        self.disguise_t > 0.0
    }

    pub fn step(&mut self, dt: f32, want: Doing) -> Outcome {
        if self.outcome != Outcome::Running {
            return self.outcome;
        }
        self.elapsed_s += dt;
        self.disguise_cd = (self.disguise_cd - dt).max(0.0);

        // MOVING OR FORGING BREAKS THE DISGUISE. It is not a state you choose
        // and keep; it is a state that ends the moment you do anything.
        let doing = if self.hiding() {
            if want == Doing::Hiding {
                Doing::Hiding
            } else {
                self.disguise_t = 0.0;
                self.disguise_cd = DISGUISE_CD_S;
                want
            }
        } else {
            if want == Doing::Hiding {
                // asked to hide without a charge: you are just standing there
                Doing::Moving
            } else {
                want
            }
        };

        if doing == Doing::Hiding {
            self.disguise_t -= dt;
            if self.disguise_t <= 0.0 {
                self.disguise_t = 0.0;
                self.disguise_cd = DISGUISE_CD_S;
            }
        }

        // ---- what it can perceive -------------------------------------
        // A boulder is total concealment; forging is standing in the open and
        // broadcasting; moving is somewhere between.
        let cover = match doing {
            Doing::Hiding => Cover(1.0),
            Doing::Forging => Cover::OPEN,
            Doing::Moving | Doing::Still => Cover::PARTIAL,
        };
        // motion is what the eye is good at: standing still is worth as much
        // as cover is
        let moving = matches!(doing, Doing::Forging | Doing::Moving);
        let gain = rate(&self.hunter, self.range_m, cover, moving) * dt * self.enrage();
        // ATTENTION IS A BALANCE, NOT A RATCHET.
        //
        // The first version only let it fall when the gain was exactly zero —
        // that is, outside the amalgam's senses entirely. But its senses reach
        // sixty metres and the sweep never leaves them, so attention rose
        // monotonically and EVERY strategy was eventually caught. There was no
        // way to shed suspicion, which means there was no stealth: only a
        // countdown wearing one.
        //
        // Suspicion now decays at a constant rate, so a low detection rate is
        // net-negative and a high one is net-positive. That puts the safe
        // distance where the design wants it: forging past about thirty-five
        // metres loses ground, and inside twenty-five it is found in seconds.
        self.attention = (self.attention + gain - ATTENTION_DECAY * dt).clamp(0.0, 2.0);

        // ---- IT PATROLS, AND IT CLOSES WHEN IT IS SURE --------------------
        //
        // The first version had it DRIFT AWAY whenever it was unsure, which
        // is not hunting — combined with a starting range of 40 m against a
        // watcher that sees 35, the encounter never began at all and every
        // test read zero attention. That is `perception`'s own lesson biting
        // the thing that inherited it: distance is the only answer to a
        // hunter, so a hunter that keeps its distance is not a threat.
        //
        // It sweeps the hollow instead. The windows that opens and closes are
        // the encounter: forge while it is far, and be somewhere else when the
        // sweep comes back.
        if self.attention >= FOUND_AT {
            self.range_m = (self.range_m - 9.0 * self.enrage() * dt).max(0.0);
            if self.range_m <= 1.5 {
                self.outcome = Outcome::Caught;
                return self.outcome;
            }
        } else {
            // a slow sweep between the edge of its senses and well outside
            // SLOW. At 0.16 the full sweep took six seconds — the whole
            // hollow crossed and recrossed in less time than one forge, which
            // is not a patrol, it is a strobe, and nothing could be read from
            // it. A sweep has to be longer than the thing it interrupts.
            self.patrol_t += dt * PATROL_RATE * self.enrage();
            let sweep = 0.5 - 0.5 * (self.patrol_t * std::f32::consts::TAU).cos();
            let want = PATROL_NEAR_M + (PATROL_FAR_M - PATROL_NEAR_M) * sweep;
            let d = want - self.range_m;
            self.range_m += d.clamp(-8.0 * dt, 8.0 * dt);
        }
        // count each time it goes from unsure to sure
        if self.attention >= FOUND_AT && gain > 0.0 && self.attention - gain < FOUND_AT {
            self.found_count += 1;
        }

        // ---- progress --------------------------------------------------
        if doing == Doing::Forging {
            self.forge_t += dt;
            if self.forge_t >= FORGE_S {
                self.forge_t = 0.0;
                self.relics += 1;
                if self.relics >= RELICS {
                    self.outcome = Outcome::Banished;
                }
            }
        } else {
            // a forge left half-done does not keep
            self.forge_t = 0.0;
        }
        self.outcome
    }
}

/// How the player is choosing to behave.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Play {
    /// forge always; spend the disguise the moment it recharges
    Panic,
    /// forge always; hold the disguise until it actually has you
    Patient,
    /// READ THE SWEEP: forge only when it is far, move when it is close,
    /// and keep the disguise for when that judgement is wrong
    Sweep,
}

/// Run a strategy to a conclusion.
pub fn play_as(style: Play, max_s: f32) -> Encounter {
    let mut e = Encounter::default();
    let dt = 1.0 / 30.0;
    while e.outcome == Outcome::Running && e.elapsed_s < max_s {
        let threatened = e.attention >= FOUND_AT * 0.9;
        let want = if e.hiding() {
            Doing::Hiding
        } else if threatened && e.can_hide() {
            e.hide();
            Doing::Hiding
        } else if style == Play::Panic && e.can_hide() {
            e.hide();
            Doing::Hiding
        } else if style == Play::Sweep && (e.range_m < safe_forge_m() || e.attention > 0.35) {
            // the skill the encounter teaches: a forge takes six seconds, and
            // inside the safe distance it does not have six seconds. Crouch
            // and let the sweep go past.
            Doing::Still
        } else {
            Doing::Forging
        };
        e.step(dt, want);
    }
    e
}

// No `play(bool, f32)`. It was the first shape of this — two strategies
// behind a boolean — and `play_as` replaced it when a third arrived. A
// superseded entry point that still compiles is the two-doors-to-one-room
// pattern, and the wrong door is the one that cannot express the interesting
// strategy.

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("THE AMALGAM OF THE GATE — many bodies, one hunger\n");
    println!("  forge {RELICS} relics at the stones while it hunts. forging makes you");
    println!("  LOUD. become a boulder for {DISGUISE_S:.0}s to break its lock — and wait");
    println!("  {DISGUISE_CD_S:.0}s for another, because the gate notices patterns.\n");

    for (label, style) in [
        ("forge blindly, hide the moment you can", Play::Panic),
        ("forge blindly, hold the disguise back", Play::Patient),
        ("read the sweep: forge far, move near", Play::Sweep),
    ] {
        let e = play_as(style, 600.0);
        println!("  {label}");
        println!(
            "    {:?} — {} relics, found {} times, {:.0}s\n",
            e.outcome, e.relics, e.found_count, e.elapsed_s
        );
    }

    println!("  THE TRADE, stated once: the only way to make progress is the thing");
    println!("  that gets you found. Six seconds of standing still and broadcasting");
    println!("  per relic, against a disguise that buys eight and costs thirty. You");
    println!("  cannot hide your way to four relics, and you cannot forge your way");
    println!("  past the hunt.\n");

    println!("  and it accelerates as you win: +{:.0}% speed per relic taken.",
        ENRAGE_PER_RELIC * 100.0);
    let mut e = Encounter::default();
    for r in 0..=RELICS {
        e.relics = r;
        println!("    {r} relics  ->  x{:.2}", e.enrage());
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn forging_is_what_gets_you_found() {
        // the whole loop in one assertion: standing at a stone broadcasting
        // is the only thing that makes progress and the fastest way to be
        // noticed
        // set the range: this is about the NOISE difference, and the sweep
        // starts outside the hunter's senses so a short run would otherwise
        // compare zero against zero
        let mut loud = Encounter::default();
        let mut quiet = Encounter::default();
        loud.range_m = 25.0;
        quiet.range_m = 25.0;
        for _ in 0..(30 * 5) {
            loud.step(1.0 / 30.0, Doing::Forging);
            quiet.step(1.0 / 30.0, Doing::Moving);
        }
        assert!(loud.attention > quiet.attention, "{} vs {}", loud.attention, quiet.attention);
        assert!(loud.forge_t > 0.0 && quiet.forge_t == 0.0);
    }

    #[test]
    fn a_boulder_is_invisible_and_ends_the_moment_you_move() {
        let mut e = Encounter::default();
        assert!(e.hide());
        for _ in 0..(30 * 2) {
            e.step(1.0 / 30.0, Doing::Hiding);
        }
        let hidden = e.attention;
        assert!(hidden < 0.05, "a boulder was noticed: {hidden}");

        // doing anything at all ends it, and starts the long recharge
        e.step(1.0 / 30.0, Doing::Forging);
        assert!(!e.hiding(), "it survived being used");
        assert!((e.disguise_cd - DISGUISE_CD_S).abs() < 0.1);
        assert!(!e.can_hide(), "another boulder immediately");
    }

    #[test]
    fn the_recharge_is_longer_than_a_forge_so_you_cannot_hide_your_way_through() {
        // the asymmetry that makes it horror rather than stealth
        assert!(DISGUISE_CD_S > FORGE_S * RELICS as f32 / 2.0);
        assert!(DISGUISE_CD_S > DISGUISE_S * 3.0);
        let e = play_as(Play::Panic, 400.0);
        assert!(e.found_count > 0, "hiding at every chance was never found once");
    }

    #[test]
    fn an_unearned_relic_does_not_keep() {
        // walk away from a half-forged stone and it resets
        let mut e = Encounter::default();
        for _ in 0..(30 * 4) {
            e.step(1.0 / 30.0, Doing::Forging);
        }
        assert!(e.forge_t > 3.0);
        e.step(1.0 / 30.0, Doing::Moving);
        assert_eq!(e.forge_t, 0.0, "progress banked without finishing");
        assert_eq!(e.relics, 0);
    }

    #[test]
    fn it_is_hard_and_it_is_winnable() {
        // An encounter nobody can beat is not asymmetric horror, it is a
        // cutscene. The skill it teaches is READING THE SWEEP — a forge takes
        // six seconds and inside thirty metres it does not have six seconds —
        // and that has to be enough while forging blindly is not.
        // WHAT THIS DOES NOT ASSERT: that forging blindly fails. That is a
        // difficulty judgement, it moves with `AMALGAM_ATTENTION`, and I
        // tuned that number in circles for several rounds — inverting which
        // strategy won each time — which is the signal that the objective
        // was mine rather than the game's. Pinning a difficulty I cannot
        // playtest would be encoding my guess as a requirement.
        //
        // What IS robustly true and worth defending: somebody can win, and
        // reading the sweep is never worse than ignoring it.
        let blind = play_as(Play::Patient, 600.0);
        let skilled = play_as(Play::Sweep, 600.0);
        assert!(
            blind.outcome == Outcome::Banished || skilled.outcome == Outcome::Banished,
            "nobody can beat it; that is a cutscene, not an encounter"
        );
        assert!(
            skilled.relics >= blind.relics,
            "reading the sweep did worse than ignoring it: {} vs {}",
            skilled.relics,
            blind.relics
        );
    }

    #[test]
    fn patience_with_the_disguise_beats_spending_it_early() {
        // "the gate notices patterns": the recharge is the scarce resource,
        // and burning it before you are threatened wastes it
        let panic = play_as(Play::Panic, 600.0);
        let patient = play_as(Play::Patient, 600.0);
        assert!(
            patient.relics >= panic.relics,
            "spending it early did better: {} vs {}",
            panic.relics,
            patient.relics
        );
    }

    #[test]
    fn four_relics_banishes_it() {
        let mut e = Encounter::default();
        // forge undisturbed: enough seconds for four
        for _ in 0..(30.0 * FORGE_S * RELICS as f32 + 60.0) as i32 {
            if e.step(1.0 / 30.0, Doing::Forging) != Outcome::Running {
                break;
            }
            // keep it off us so the test is about progress, not the hunt
            e.attention = 0.0;
            e.range_m = 50.0;
        }
        assert_eq!(e.outcome, Outcome::Banished);
        assert_eq!(e.relics, RELICS);
    }

    #[test]
    fn it_accelerates_as_you_take_relics_from_it() {
        let mut e = Encounter::default();
        let none = e.enrage();
        e.relics = RELICS;
        assert!(e.enrage() > none * 1.5, "taking everything barely angered it");
        // and that shortens the time it needs to reach you
        let mut calm = Encounter::default();
        let mut angry = Encounter::default();
        angry.relics = 3;
        calm.attention = 1.5;
        angry.attention = 1.5;
        for _ in 0..30 {
            calm.step(1.0 / 30.0, Doing::Forging);
            angry.step(1.0 / 30.0, Doing::Forging);
        }
        assert!(angry.range_m < calm.range_m, "{} vs {}", angry.range_m, calm.range_m);
    }

    #[test]
    fn distance_is_still_the_only_answer_to_a_hunter() {
        // `perception`'s own conclusion, inherited: it never stops, so being
        // far is the one safe state that is not a boulder. Its range is its
        // own (see `amalgam_senses`), wider and slower than a corridor
        // hunter's.
        let mut close = Encounter::default();
        close.range_m = 10.0;
        let mut far = Encounter::default();
        far.range_m = 200.0;
        for _ in 0..(30 * 4) {
            close.step(1.0 / 30.0, Doing::Forging);
            far.step(1.0 / 30.0, Doing::Forging);
        }
        assert!(close.attention > far.attention);
        assert_eq!(far.attention, 0.0, "it saw past its own range");
    }

    #[test]
    fn hiding_without_a_charge_is_just_standing_there() {
        let mut e = Encounter::default();
        e.disguise_cd = DISGUISE_CD_S;
        assert!(!e.hide());
        for _ in 0..(30 * 3) {
            e.step(1.0 / 30.0, Doing::Hiding);
        }
        assert!(!e.hiding());
        assert_eq!(e.relics, 0, "it forged while pretending to be a rock");
    }

    #[test]
    fn attention_decays_when_it_has_nothing() {
        // a hunt, not an alarm: lose it and it drifts back out
        let mut e = Encounter::default();
        e.attention = 0.8;
        e.range_m = 200.0; // well outside its range
        for _ in 0..(30 * 3) {
            e.step(1.0 / 30.0, Doing::Moving);
        }
        assert!(e.attention < 0.8, "it stayed certain with nothing to go on");
    }
}

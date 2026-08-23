//! HAZARDS — things already on their way.
//!
//! Ported from the C engine before it is retired (`exengine/warp_wave.c`,
//! `exengine/spearcon.c`). Both came out of the film scripts rather than out
//! of the code, and both are the same idea: something is going to happen at a
//! place, at a time, and nobody can call it off. What differs is the fiction
//! and the plurality, so the timing is shared and the two mechanics are thin.
//!
//! ## The warp wave
//!
//! The Ice-Clone quantum leap's *consequence*. The jump itself is silent and
//! undetectable — one moment the bearer is not there, then they are. The only
//! evidence anything happened is a gravity wave that arrives SECONDS LATER at
//! the landing point. The jumper knows it is coming and can warn allies, but
//! cannot stop or shape it, **and it is indifferent to who is standing where
//! it lands.**
//!
//! That last clause is the mechanic. A consequence you cannot aim, arriving
//! after the fact, is an unusual thing for a game to hand a player, and it is
//! exactly the Watch-bearer's register: the power is real and it is not
//! obedient.
//!
//! ## Spearcons
//!
//! Cryotek's lieutenants' signature weapon. Sticky spears that cling where
//! they land and count down — the script calls it "ticking-bomb tension that
//! forces everyone to keep moving". Same clock, many of them, and they are
//! placed rather than aimed.
//!
//! ## What changed in the port
//!
//! The C `ex_spearcon_update` writes detonations into a caller-supplied buffer
//! and returns how many went off — but it increments that count even when the
//! buffer is full, so a caller iterating `0..n` over a shorter buffer reads
//! whatever was in memory. Returning a `Vec` removes the failure mode rather
//! than fixing it, which is the right trade in a language that can.

use glam::Vec3;

/// A single thing on a clock. Neither mechanic can cancel one; the only
/// operations are start it, advance it, and ask how close it is.
#[derive(Clone, Copy, Debug, Default)]
pub struct Fuse {
    armed: bool,
    remaining_s: f32,
    total_s: f32,
    pub at: Vec3,
}

/// The shortest fuse anything can be given. A zero-length fuse fires on the
/// frame it is armed, which reads as an instant hit and loses the entire
/// point of both mechanics.
pub const MIN_FUSE_S: f32 = 0.05;

impl Fuse {
    /// Start the clock. Overwrites whatever was pending.
    pub fn arm(&mut self, at: Vec3, delay_s: f32) {
        self.armed = true;
        self.total_s = delay_s.max(MIN_FUSE_S);
        self.remaining_s = self.total_s;
        self.at = at;
    }

    /// Advance. `true` on the SINGLE tick it fires, and it disarms itself.
    pub fn tick(&mut self, dt: f32) -> bool {
        if !self.armed {
            return false;
        }
        self.remaining_s -= dt;
        if self.remaining_s <= 0.0 {
            self.armed = false;
            self.remaining_s = 0.0;
            return true;
        }
        false
    }

    pub fn armed(&self) -> bool {
        self.armed
    }

    /// 0 the instant it is armed, 1 as it fires; 0 when nothing is pending.
    /// Drives the warning ring — a telegraph that tightens as it closes.
    pub fn progress(&self) -> f32 {
        if !self.armed {
            return 0.0;
        }
        (1.0 - self.remaining_s / self.total_s).clamp(0.0, 1.0)
    }

    pub fn remaining_s(&self) -> f32 {
        if self.armed {
            self.remaining_s
        } else {
            0.0
        }
    }
}

/// The gravity wave following a quantum leap.
///
/// One at a time, and a new leap resets the clock — the C original overwrites
/// on re-arm and that is right: you cannot have two of your own arrivals
/// pending, because there is only one of you.
#[derive(Clone, Copy, Debug, Default)]
pub struct WarpWave(Fuse);

/// How long after a leap the wave lands. From the script's "seconds later";
/// long enough to warn somebody and too short to move a fight.
pub const WARP_DELAY_S: f32 = 4.0;

impl WarpWave {
    pub fn leap(&mut self, to: Vec3) {
        self.0.arm(to, WARP_DELAY_S);
    }
    /// Arm with a non-default delay, for a bearer whose leap is degraded.
    pub fn leap_after(&mut self, to: Vec3, delay_s: f32) {
        self.0.arm(to, delay_s);
    }
    /// `Some(where)` on the tick it lands.
    pub fn tick(&mut self, dt: f32) -> Option<Vec3> {
        let at = self.0.at;
        self.0.tick(dt).then_some(at)
    }
    pub fn inbound(&self) -> bool {
        self.0.armed()
    }
    pub fn progress(&self) -> f32 {
        self.0.progress()
    }
    pub fn landing(&self) -> Option<Vec3> {
        self.0.armed().then_some(self.0.at)
    }
}

/// How many spears can be in the air and stuck at once. From the C pool.
pub const SPEARCON_MAX: usize = 16;

/// The lieutenants' planted charges.
#[derive(Clone, Debug, Default)]
pub struct Spearcons {
    live: Vec<Fuse>,
}

impl Spearcons {
    /// Plant a charge. `false` when the pool is full — a refusal the caller
    /// should show, because a spear that silently does not exist is worse
    /// than one that visibly bounces off.
    pub fn plant(&mut self, at: Vec3, fuse_s: f32) -> bool {
        if self.live.len() >= SPEARCON_MAX {
            return false;
        }
        let mut f = Fuse::default();
        f.arm(at, fuse_s);
        self.live.push(f);
        true
    }

    /// Advance every charge; returns where each one that went off this tick
    /// was planted.
    ///
    /// A `Vec` rather than the C signature's caller-supplied buffer and
    /// count: that version incremented the count even when the buffer was
    /// full, so a caller reading `n` entries out of a shorter array read
    /// past it. Not a bug worth porting.
    pub fn tick(&mut self, dt: f32) -> Vec<Vec3> {
        let mut fired = Vec::new();
        for f in &mut self.live {
            if f.tick(dt) {
                fired.push(f.at);
            }
        }
        self.live.retain(|f| f.armed());
        fired
    }

    pub fn count(&self) -> usize {
        self.live.len()
    }
    pub fn full(&self) -> bool {
        self.live.len() >= SPEARCON_MAX
    }
    /// Live charges, for the telegraph: where, and how close.
    pub fn telegraph(&self) -> impl Iterator<Item = (Vec3, f32)> + '_ {
        self.live.iter().map(|f| (f.at, f.progress()))
    }
    pub fn clear(&mut self) {
        self.live.clear();
    }
}

// ------------------------------------------------------------- driver ---

/// `--hazard` — watch a fuse burn.
///
/// Both mechanics are about a delay you can see coming, and the only thing
/// worth checking without a screen is the SHAPE of that delay: how long the
/// telegraph reads for, and whether it fires once.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--hazard").unwrap_or(0);
    match argv.get(i + 1).map(String::as_str).unwrap_or("all") {
        "wave" => wave(),
        "spears" => spears(),
        "all" => {
            wave();
            println!();
            spears();
        }
        other => println!("unknown: --hazard {other}\ntry: wave | spears | all"),
    }
}

fn wave() {
    println!("THE WARP WAVE — the leap's consequence\n");
    let mut w = WarpWave::default();
    w.leap(Vec3::new(12.0, 0.0, -30.0));
    println!("  the bearer is gone and then there. nothing is detectable yet.");
    println!("  {:.1} s until the gravity wave reaches where they landed.\n", WARP_DELAY_S);
    let dt = 1.0 / 60.0;
    let mut t = 0.0;
    let mut ticks = 0;
    loop {
        t += dt;
        ticks += 1;
        let landed = w.tick(dt);
        if ticks % 30 == 0 && landed.is_none() {
            let bar = "#".repeat((w.progress() * 24.0) as usize);
            println!("  t+{t:>4.1}s  [{bar:<24}] {:.0}%", w.progress() * 100.0);
        }
        if let Some(at) = landed {
            println!("  t+{t:>4.1}s  IT LANDS at {:.0},{:.0},{:.0}", at.x, at.y, at.z);
            break;
        }
        if t > 30.0 {
            println!("  never landed — the scheduler is broken");
            break;
        }
    }
    println!("\n  the jumper knew it was coming and could warn people. they could not");
    println!("  stop it, aim it, or move it, and it does not care who is standing there.");
}

fn spears() {
    println!("SPEARCONS — ticking-bomb tension\n");
    let mut p = Spearcons::default();
    // a lieutenant walking fire across a room: same weapon, staggered fuses
    for (n, fuse) in [1.2f32, 2.0, 2.8].into_iter().enumerate() {
        p.plant(Vec3::new(n as f32 * 4.0, 0.0, 0.0), fuse);
    }
    println!("  three planted, fuses 1.2 / 2.0 / 2.8 s. {} live.\n", p.count());
    let dt = 1.0 / 60.0;
    let mut t = 0.0;
    while p.count() > 0 && t < 10.0 {
        t += dt;
        for at in p.tick(dt) {
            println!("  t+{t:>4.1}s  detonates at x={:.0}   ({} still live)", at.x, p.count());
        }
    }
    println!("\n  they go off in the order they were planted, which is what makes the");
    println!("  room readable: you can see where you must not be, and when.");
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_fuse_fires_exactly_once() {
        let mut f = Fuse::default();
        f.arm(Vec3::ZERO, 1.0);
        let mut fired = 0;
        for _ in 0..600 {
            if f.tick(1.0 / 60.0) {
                fired += 1;
            }
        }
        assert_eq!(fired, 1, "a delayed consequence must not repeat");
        assert!(!f.armed());
        // and ticking a spent fuse is safe and silent
        assert!(!f.tick(1.0));
    }

    #[test]
    fn the_telegraph_runs_from_nothing_to_everything() {
        let mut f = Fuse::default();
        assert_eq!(f.progress(), 0.0, "an unarmed fuse shows no warning");
        f.arm(Vec3::ZERO, 2.0);
        assert_eq!(f.progress(), 0.0, "and it starts at nothing the instant it is armed");
        let mut prev = 0.0;
        for _ in 0..119 {
            f.tick(1.0 / 60.0);
            let p = f.progress();
            assert!(p >= prev, "the warning must not run backwards");
            prev = p;
        }
        assert!(prev > 0.95, "and it should be nearly full just before it fires: {prev}");
        // spent, the ring goes away rather than sticking at 1
        while f.armed() {
            f.tick(1.0 / 60.0);
        }
        assert_eq!(f.progress(), 0.0);
    }

    #[test]
    fn no_fuse_is_instant() {
        // a zero fuse fires on the frame it is armed, which reads as a hit
        // and loses the mechanic entirely
        let mut f = Fuse::default();
        f.arm(Vec3::ZERO, 0.0);
        assert!(!f.tick(1.0 / 60.0), "a zero fuse detonated immediately");
        assert!(f.remaining_s() > 0.0);
    }

    #[test]
    fn a_second_leap_resets_the_clock_rather_than_stacking() {
        // there is only one of you, so there is only one arrival pending
        let mut w = WarpWave::default();
        w.leap(Vec3::new(1.0, 0.0, 0.0));
        for _ in 0..120 {
            w.tick(1.0 / 60.0);
        }
        let part_way = w.progress();
        assert!(part_way > 0.0);

        w.leap(Vec3::new(50.0, 0.0, 0.0));
        assert_eq!(w.progress(), 0.0, "the new leap should restart the telegraph");
        assert_eq!(w.landing(), Some(Vec3::new(50.0, 0.0, 0.0)), "and move the landing");

        let mut lands = Vec::new();
        for _ in 0..600 {
            if let Some(at) = w.tick(1.0 / 60.0) {
                lands.push(at);
            }
        }
        assert_eq!(lands, vec![Vec3::new(50.0, 0.0, 0.0)], "only the latest leap arrives");
    }

    #[test]
    fn the_wave_lands_where_the_bearer_did_not_where_they_are() {
        // "indifferent to who is standing where it lands" — the position is
        // fixed at leap time and nothing afterwards moves it
        let mut w = WarpWave::default();
        let landing = Vec3::new(7.0, 0.0, -3.0);
        w.leap(landing);
        for _ in 0..600 {
            if let Some(at) = w.tick(1.0 / 60.0) {
                assert_eq!(at, landing);
                return;
            }
        }
        panic!("the wave never arrived");
    }

    #[test]
    fn spears_go_off_in_fuse_order_not_plant_order() {
        let mut p = Spearcons::default();
        p.plant(Vec3::new(0.0, 0.0, 0.0), 2.0); // planted first, slow fuse
        p.plant(Vec3::new(9.0, 0.0, 0.0), 0.5); // planted second, quick
        let mut order = Vec::new();
        for _ in 0..600 {
            for at in p.tick(1.0 / 60.0) {
                order.push(at.x as i32);
            }
        }
        assert_eq!(order, vec![9, 0], "the short fuse should go first");
        assert_eq!(p.count(), 0, "spent charges should leave the pool");
    }

    #[test]
    fn the_pool_refuses_rather_than_silently_dropping() {
        let mut p = Spearcons::default();
        for i in 0..SPEARCON_MAX {
            assert!(p.plant(Vec3::new(i as f32, 0.0, 0.0), 1.0), "slot {i} should take");
        }
        assert!(p.full());
        assert!(!p.plant(Vec3::ZERO, 1.0), "the pool should say no when it is full");
        assert_eq!(p.count(), SPEARCON_MAX);
    }

    #[test]
    fn every_detonation_is_reported_even_when_many_land_together() {
        // THE C BUG, as a test. `ex_spearcon_update` wrote into a
        // caller-supplied buffer but incremented its return count whether or
        // not it had room, so a caller reading `n` entries out of a shorter
        // array read past the end. Returning a Vec makes the failure
        // unrepresentable; this pins that all sixteen are accounted for.
        let mut p = Spearcons::default();
        for i in 0..SPEARCON_MAX {
            p.plant(Vec3::new(i as f32, 0.0, 0.0), 1.0);
        }
        let mut fired = Vec::new();
        for _ in 0..600 {
            fired.extend(p.tick(1.0 / 60.0));
        }
        assert_eq!(fired.len(), SPEARCON_MAX, "some detonations went unreported");
        let mut xs: Vec<i32> = fired.iter().map(|v| v.x as i32).collect();
        xs.sort_unstable();
        assert_eq!(xs, (0..SPEARCON_MAX as i32).collect::<Vec<_>>());
        assert_eq!(p.count(), 0);
    }

    #[test]
    fn the_telegraph_lists_every_live_charge() {
        let mut p = Spearcons::default();
        p.plant(Vec3::new(1.0, 0.0, 0.0), 1.0);
        p.plant(Vec3::new(2.0, 0.0, 0.0), 4.0);
        for _ in 0..30 {
            p.tick(1.0 / 60.0);
        }
        let t: Vec<(Vec3, f32)> = p.telegraph().collect();
        assert_eq!(t.len(), 2);
        assert!(t[0].1 > t[1].1, "the shorter fuse should read as closer");
    }
}

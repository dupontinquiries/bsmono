//! CLING — holding onto a wall, and the meter that says how long.
//!
//! Ported from `game.c`. The design line:
//!
//! > SAFETY is a grip meter: vertical faces drain slow, UNDERSIDES drain
//! > fast, heavier builds drain faster, crawler alt-modes (spider / insect /
//! > scorpion / crab) grip far better, and ice is slick. Empty grip = you
//! > fall.
//!
//! Four multiplied terms and a clock. What makes it worth porting rather than
//! reinventing is that every term already has a home here: orientation is the
//! surface you latched to, build is `chassis_traits`' bulk, alt-mode is the
//! chassis category, and the surface is `terrain::Material` — the module that
//! already knows ice from basalt because `mapsim` needed it to decide where
//! dunes form.
//!
//! ## The shape of the decision
//!
//! A wall is a place you can wait. An underside is not: at the shipped rates
//! a light frame gets about half a minute on a vertical face and **eight
//! seconds hanging beneath one**. That asymmetry is the whole mechanic —
//! crossing a ceiling is a committed move with a countdown, and crossing a
//! wall is a route.
//!
//! ## What the port changes
//!
//! The C reads `g_surface_grip` and computes `drain *= (2 - grip)`, which is
//! a number the map sets. Here the surface is a `terrain::Material`, so ice
//! being slick is derived from the same table that decides everything else
//! about a material rather than being a second opinion about it.

use crate::terrain::Material;

/// Drain per second on a vertical face, for a reference frame on rock.
pub const WALL_DRAIN: f32 = 0.045;
/// And hanging underneath one. Undersides are brutal.
pub const UNDER_DRAIN: f32 = 0.20;
/// Crawler alt-modes are born to cling.
pub const CRAWLER_MULT: f32 = 0.40;
/// Grip recovered per second with both feet on the ground.
pub const RECOVER: f32 = 0.5;
/// After a fall, before you can latch again.
pub const REGRAB_LOCKOUT_S: f32 = 1.2;

/// What you are holding onto.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Hold {
    /// standing on it
    Ground,
    /// a vertical face
    Wall,
    /// hanging beneath. The committed one.
    Underside,
}

/// How well a surface takes a grip, 0..1.
///
/// Derived from the material rather than set per map. `terrain::Material`
/// already distinguishes ice from rock for dune formation and talus; a second
/// per-map "grip" number would be a chance for the two to disagree about what
/// a surface is.
pub fn surface_grip(m: Material) -> f32 {
    match m {
        // worked metal: purpose-built handholds everywhere
        Material::Metal => 1.0,
        // porous, sharp, and it takes an edge
        Material::Basalt => 0.95,
        Material::Sandstone => 0.9,
        // loose over rock — it comes away in your hand
        Material::Regolith => 0.65,
        Material::Ice => 0.35,
        // it deforms under load, which is worse than slick
        Material::SoftIce => 0.2,
    }
}

/// The chassis facts grip cares about.
#[derive(Clone, Copy, Debug)]
pub struct Frame {
    /// `chassis_traits`' bulk, 0..1
    pub bulk: f32,
    /// spider, insect, scorpion, crab
    pub crawler: bool,
}

impl Frame {
    pub fn light() -> Frame {
        Frame { bulk: 0.25, crawler: false }
    }
    pub fn heavy() -> Frame {
        Frame { bulk: 0.95, crawler: false }
    }
    pub fn crawler() -> Frame {
        Frame { bulk: 0.5, crawler: true }
    }
}

/// Grip lost per second in this situation. Zero on the ground.
pub fn drain_rate(hold: Hold, f: Frame, m: Material) -> f32 {
    if hold == Hold::Ground {
        return 0.0;
    }
    let mut d = if hold == Hold::Underside { UNDER_DRAIN } else { WALL_DRAIN };
    // heavy frames struggle: 0.55 at nothing, 1.10 at full bulk
    d *= 0.55 + f.bulk.clamp(0.0, 1.0) * 0.55;
    if f.crawler {
        d *= CRAWLER_MULT;
    }
    // the C's `2 - grip`: a perfect surface still costs full rate, a bad one
    // costs nearly double
    d * (2.0 - surface_grip(m))
}

/// How long you can hold on from full, in seconds. `None` on the ground.
pub fn hang_time_s(hold: Hold, f: Frame, m: Material) -> Option<f32> {
    let d = drain_rate(hold, f, m);
    (d > 0.0).then(|| 1.0 / d)
}

pub struct Grip {
    /// 0..1
    pub level: f32,
    pub hold: Hold,
    /// >0 while you cannot latch again
    pub regrab: f32,
    /// set on the tick grip ran out
    pub fell: bool,
}

impl Default for Grip {
    fn default() -> Self {
        Grip { level: 1.0, hold: Hold::Ground, regrab: 0.0, fell: false }
    }
}

impl Grip {
    pub fn can_latch(&self) -> bool {
        self.regrab <= 0.0
    }

    /// Try to take hold. Refused during the lockout after a fall.
    pub fn latch(&mut self, to: Hold) -> bool {
        if !self.can_latch() || to == Hold::Ground {
            return false;
        }
        self.hold = to;
        true
    }

    pub fn step(&mut self, dt: f32, f: Frame, m: Material) {
        self.fell = false;
        self.regrab = (self.regrab - dt).max(0.0);
        match self.hold {
            Hold::Ground => self.level = (self.level + RECOVER * dt).min(1.0),
            _ => {
                self.level -= drain_rate(self.hold, f, m) * dt;
                if self.level <= 0.0 {
                    self.level = 0.0;
                    self.hold = Hold::Ground;
                    self.regrab = REGRAB_LOCKOUT_S;
                    self.fell = true;
                }
            }
        }
    }
}

// ------------------------------------------------------------- driver ---

pub fn run() {
    println!("GRIP — a wall is a route, a ceiling is a countdown\n");
    let frames = [("light", Frame::light()), ("heavy", Frame::heavy()), ("crawler", Frame::crawler())];
    let mats = [
        ("metal", Material::Metal),
        ("basalt", Material::Basalt),
        ("regolith", Material::Regolith),
        ("ice", Material::Ice),
        ("soft ice", Material::SoftIce),
    ];
    for (hold, label) in [(Hold::Wall, "ON A WALL"), (Hold::Underside, "UNDER AN OVERHANG")] {
        println!("  {label}   (seconds from a full meter)\n");
        print!("    {:<10}", "");
        for (mn, _) in &mats {
            print!(" {:>9}", mn);
        }
        println!();
        for (fname, f) in &frames {
            print!("    {:<10}", fname);
            for (_, m) in &mats {
                print!(" {:>9.1}", hang_time_s(hold, *f, *m).unwrap_or(0.0));
            }
            println!();
        }
        println!();
    }
    let l = hang_time_s(Hold::Wall, Frame::light(), Material::Basalt).unwrap();
    let u = hang_time_s(Hold::Underside, Frame::light(), Material::Basalt).unwrap();
    println!("  the asymmetry is the mechanic: {l:.0}s on the face, {u:.0}s beneath it.");
    println!("  a wall is somewhere you can stop and think. a ceiling is a move you");
    println!("  have already committed to.\n");
    println!("  and ice is not a modifier somebody set per map — it is");
    println!("  `terrain::Material`, the same table that decides where dunes form.");

    // The table above is the arithmetic. This is the mechanic: a traverse
    // where the meter runs out, and what the recovery costs.
    println!("\n  A CROSSING — 9 m of ice overhang at 1.4 m/s\n");
    for (who, f) in [("heavy", Frame::heavy()), ("crawler", Frame::crawler())] {
        println!("    {who}:");
        crossing(f, Material::Ice, 9.0);
    }
    println!("\n  the meter is not a resource you spend, it is a distance you can");
    println!("  reach. a span longer than your hang time is not hard, it is SHUT —");
    println!("  and the answer is a different frame, not a better attempt.");
}

/// Cross `span` metres of overhang, retrying after every fall.
fn crossing(f: Frame, m: Material, span: f32) {
    let mut g = Grip::default();
    g.latch(Hold::Underside);
    let (mut t, mut covered, mut falls) = (0.0f32, 0.0f32, 0);
    let dt = 1.0 / 30.0;
    while covered < span && t < 60.0 {
        t += dt;
        let before = g.hold;
        g.step(dt, f, m);
        if before == Hold::Underside && g.hold == Hold::Underside {
            covered += 1.4 * dt;
        }
        if g.fell {
            falls += 1;
            if falls <= 2 {
                println!("      t+{t:>5.1}s  GRIP FAILED at {covered:.1} m");
            }
            covered = 0.0; // back to the start of the span
        }
        if g.hold == Hold::Ground && g.can_latch() && g.level > 0.85 {
            g.latch(Hold::Underside);
        }
    }
    if covered >= span {
        println!("      t+{t:>5.1}s  across, after {falls} fall(s)");
    } else {
        let reach = hang_time_s(Hold::Underside, f, m).unwrap_or(0.0) * 1.4;
        println!(
            "      gave up after {falls} identical falls — it reaches {reach:.1} m of a {span:.0} m span"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn undersides_are_brutal() {
        // the asymmetry the whole mechanic rests on
        let (f, m) = (Frame::light(), Material::Basalt);
        let wall = hang_time_s(Hold::Wall, f, m).unwrap();
        let under = hang_time_s(Hold::Underside, f, m).unwrap();
        assert!(wall > under * 3.0, "wall {wall}s vs underside {under}s");
        assert!(under < 15.0, "you can loiter on a ceiling for {under}s");
        assert!(wall > 15.0, "a wall should be somewhere you can stop: {wall}s");
    }

    #[test]
    fn a_heavy_frame_struggles_and_a_crawler_does_not() {
        let m = Material::Basalt;
        let light = hang_time_s(Hold::Wall, Frame::light(), m).unwrap();
        let heavy = hang_time_s(Hold::Wall, Frame::heavy(), m).unwrap();
        let crawl = hang_time_s(Hold::Wall, Frame::crawler(), m).unwrap();
        assert!(heavy < light, "bulk should cost: {heavy} vs {light}");
        assert!(crawl > light, "a crawler should out-hang a light frame: {crawl} vs {light}");
        // and a crawler beats a heavy frame by a wide margin, since the
        // multiplier compounds with its middling bulk
        assert!(crawl > heavy * 2.0);
    }

    #[test]
    fn ice_is_slick_and_soft_ice_is_worse() {
        let f = Frame::light();
        let rock = hang_time_s(Hold::Wall, f, Material::Basalt).unwrap();
        let ice = hang_time_s(Hold::Wall, f, Material::Ice).unwrap();
        let soft = hang_time_s(Hold::Wall, f, Material::SoftIce).unwrap();
        assert!(ice < rock, "ice should be worse than rock");
        assert!(soft < ice, "soft ice deforms under load, which is worse than slick");
        // metal is the best surface: it was built with handholds
        assert!(hang_time_s(Hold::Wall, f, Material::Metal).unwrap() > rock);
    }

    #[test]
    fn every_material_can_be_held_at_all() {
        // a surface with an infinite drain is a surface the level cannot use,
        // and terrain hands out all six
        for m in [
            Material::Metal,
            Material::Basalt,
            Material::Sandstone,
            Material::Regolith,
            Material::Ice,
            Material::SoftIce,
        ] {
            let t = hang_time_s(Hold::Wall, Frame::heavy(), m).unwrap();
            assert!(t > 3.0, "{m:?} gives a heavy frame only {t}s on a wall");
            assert!((0.0..=1.0).contains(&surface_grip(m)));
        }
    }

    #[test]
    fn the_ground_costs_nothing_and_gives_it_back() {
        let mut g = Grip::default();
        g.level = 0.2;
        for _ in 0..60 {
            g.step(1.0 / 60.0, Frame::heavy(), Material::SoftIce);
        }
        assert!(g.level > 0.6, "a second on the ground should recover half: {}", g.level);
        assert_eq!(drain_rate(Hold::Ground, Frame::heavy(), Material::SoftIce), 0.0);
        assert!(hang_time_s(Hold::Ground, Frame::light(), Material::Metal).is_none());
    }

    #[test]
    fn running_out_drops_you_and_locks_you_out() {
        let mut g = Grip::default();
        assert!(g.latch(Hold::Underside));
        let mut fell_at = None;
        for i in 0..(60 * 60) {
            g.step(1.0 / 60.0, Frame::heavy(), Material::Ice);
            if g.fell {
                fell_at = Some(i as f32 / 60.0);
                break;
            }
        }
        let t = fell_at.expect("a heavy frame under an ice overhang should fall");
        assert!(t < 6.0, "it held for {t}s");
        assert_eq!(g.hold, Hold::Ground);
        assert!(!g.can_latch(), "it could grab again the instant it fell");

        // and the lockout does expire
        for _ in 0..((REGRAB_LOCKOUT_S + 0.2) * 60.0) as i32 {
            g.step(1.0 / 60.0, Frame::heavy(), Material::Ice);
        }
        assert!(g.can_latch());
    }

    #[test]
    fn letting_go_is_latching_to_the_ground_and_not_a_third_verb() {
        // A `release()` lived here and nothing called it: dropping off a wall
        // is the same event as arriving on the floor, and giving it its own
        // method invites the two to disagree about the regrab lockout. In the
        // C, letting go is a push-off that sets velocity — a movement fact,
        // not a grip one.
        let mut g = Grip::default();
        g.latch(Hold::Wall);
        for _ in 0..30 {
            g.step(1.0 / 60.0, Frame::light(), Material::Basalt);
        }
        assert!(g.level < 1.0, "a wall should cost something");
        g.hold = Hold::Ground;
        g.step(1.0 / 60.0, Frame::light(), Material::Basalt);
        assert!(g.can_latch(), "letting go voluntarily should not lock you out");
    }

    #[test]
    fn you_cannot_latch_onto_the_floor() {
        let mut g = Grip::default();
        assert!(!g.latch(Hold::Ground), "latched onto the ground");
        assert_eq!(g.hold, Hold::Ground);
    }

    #[test]
    fn the_worst_case_is_survivable_for_a_crawler() {
        // a crawler under a soft-ice overhang is the hardest thing the model
        // can be asked, and it should still be a move rather than a refusal
        let t = hang_time_s(Hold::Underside, Frame::crawler(), Material::SoftIce).unwrap();
        assert!(t > 2.0, "the hardest crossing gives only {t}s");
        // ... and strictly worse than the same crawler on metal
        assert!(t < hang_time_s(Hold::Underside, Frame::crawler(), Material::Metal).unwrap());
    }
}

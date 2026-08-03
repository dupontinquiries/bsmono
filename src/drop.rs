//! THE DROP — leaving a moving ship on purpose.
//!
//! A raid should not begin at a loading screen. `hub_plan.h` already says
//! the hangar is "Drop bay. Launches and landings come through here", and
//! everything needed is built: a hangar door with a launch gauge, a hull
//! whose frame carries velocity, a transform, and a fall gait. This is the
//! twenty seconds that joins them.
//!
//! THE PHYSICS IS THE POINT. You do not spawn at the LZ; you leave through
//! the door with the ship's velocity at that door — `BigBody::carry_at`,
//! including the rotational term — so a fast low pass flings you forward and
//! a banking ship throws you sideways. Where you land is decided before you
//! jump, by the pilot's line and by when you let go, and that is what makes
//! a drop something you can be good at.
//!
//! THE TRADE IS SPEED AGAINST CONTROL, and it is the whole mechanic:
//!
//!   * **Vehicle mode** is a dart. Small frontal area, high terminal
//!     velocity, almost no steering. You are down fast, which means less
//!     time in anyone's sights, and you arrive hard.
//!   * **Robot mode** is a parachute. Limbs out, terminal velocity halved,
//!     real lateral authority. You can fix a bad exit — and you hang in the
//!     air twice as long while an outpost shoots at you.
//!
//! And the last second decides the landing. Fold late and you hit at vehicle
//! speed in a body that can take it: the impact craters, and anything
//! standing near the LZ wears it. That is the hero landing, and it is
//! genuinely the optimal play — it is just a very small window, and missing
//! it means eating the impact yourself.

use glam::Vec3;

/// Vehicle-mode terminal velocity, m/s at 1 g. A dart.
pub const TERM_VEHICLE: f32 = 92.0;
/// Robot-mode terminal velocity. Limbs out.
pub const TERM_ROBOT: f32 = 41.0;
/// Lateral acceleration available in robot mode, m/s².
pub const STEER_ROBOT: f32 = 14.0;
/// And in vehicle mode. Nearly nothing: you committed at the door.
pub const STEER_VEHICLE: f32 = 2.2;
/// Below this altitude a fold counts as the hero landing.
pub const HERO_WINDOW_M: f32 = 34.0;
/// Impact speed a robot can absorb without hurting itself.
pub const SAFE_IMPACT: f32 = 46.0;
/// Radius of the crater a hero landing makes.
pub const HERO_RADIUS_M: f32 = 12.0;
/// Anti-air damage per second while visible over a defended LZ.
pub const AA_DPS: f32 = 5.5;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Mode {
    /// folded, falling fast, barely steerable
    Vehicle,
    /// limbs out, slow, steerable
    Robot,
}

impl Mode {
    pub fn terminal(self, g: f32) -> f32 {
        // terminal velocity goes as sqrt(g) for fixed drag and mass
        let s = (g / 9.81).max(0.01).sqrt();
        match self {
            Mode::Vehicle => TERM_VEHICLE * s,
            Mode::Robot => TERM_ROBOT * s,
        }
    }
    pub fn steer(self) -> f32 {
        match self {
            Mode::Vehicle => STEER_VEHICLE,
            Mode::Robot => STEER_ROBOT,
        }
    }
    /// How readable you are to an anti-air gun. A slow wide robot is an
    /// easier shot than a folded dart, so the safe descent is the dangerous
    /// one.
    pub fn exposure(self) -> f32 {
        match self {
            Mode::Vehicle => 0.45,
            Mode::Robot => 1.0,
        }
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    /// aboard, waiting on the door and the line
    Ready,
    /// out, falling
    Falling,
    /// on the ground
    Down,
}

/// What the drop did, for the caller to apply.
#[derive(Default, Debug, Clone, Copy)]
pub struct Landing {
    pub impact_speed: f32,
    /// damage to the dropper
    pub self_dmg: f32,
    /// area damage dealt on arrival, and its radius
    pub shock: f32,
    pub shock_radius: f32,
    /// metres from the intended landing zone
    pub miss_distance: f32,
    /// total anti-air damage taken on the way down
    pub aa_taken: f32,
    /// did the last-second fold land in the window?
    pub hero: bool,
}

pub struct Drop {
    pub phase: Phase,
    pub mode: Mode,
    pub pos: Vec3,
    pub vel: Vec3,
    /// where you are trying to land
    pub lz: Vec3,
    /// is the LZ shooting back?
    pub defended: bool,
    aa_taken: f32,
    /// altitude at which the last fold happened, if any
    fold_alt: Option<f32>,
}

impl Drop {
    pub fn new(lz: Vec3, defended: bool) -> Drop {
        Drop {
            phase: Phase::Ready,
            mode: Mode::Vehicle,
            pos: Vec3::ZERO,
            vel: Vec3::ZERO,
            lz,
            defended,
            aa_taken: 0.0,
            fold_alt: None,
        }
    }

    /// CAN YOU GO? Refuses with a reason, because a jump button that does
    /// nothing over a hot LZ is the worst possible moment for silence.
    ///
    /// The door has to be genuinely open — `airlock::Door::passable`, which
    /// is 75% travelled, not merely "opening" — and the ship has to be off
    /// the ground. A hangar cracking open at four metres is not a drop, it
    /// is a ramp.
    pub fn can_jump(
        &self,
        door_passable: bool,
        ship_airborne: bool,
        altitude: f32,
    ) -> Result<(), &'static str> {
        if self.phase != Phase::Ready {
            return Err("already out");
        }
        if !door_passable {
            return Err("hangar door is not open");
        }
        if !ship_airborne {
            return Err("ship is on the ground");
        }
        if altitude < 25.0 {
            return Err("too low to drop");
        }
        Ok(())
    }

    /// Step out. `door_pos` and `carry` come from the ship's BigBody — the
    /// velocity AT THE DOOR, rotational term included, so a banking ship
    /// throws you sideways whether you meant it or not.
    pub fn jump(&mut self, door_pos: Vec3, carry: Vec3) -> bool {
        self.phase = Phase::Falling;
        self.pos = door_pos;
        self.vel = carry;
        self.fold_alt = None;
        true
    }

    /// Fold, mid-air. Doing it late is the hero landing; doing it early is
    /// the safe one and costs you time in the open.
    pub fn transform(&mut self, altitude: f32) {
        if self.phase != Phase::Falling {
            return;
        }
        self.mode = match self.mode {
            Mode::Vehicle => Mode::Robot,
            Mode::Robot => Mode::Vehicle,
        };
        if self.mode == Mode::Robot {
            self.fold_alt = Some(altitude);
        }
    }

    /// Advance. `steer` is the lateral stick, `ground` the world's height at
    /// a point, `g` the gravity magnitude.
    pub fn update(
        &mut self,
        dt: f32,
        steer: Vec3,
        g: f32,
        ground_y: f32,
    ) -> Option<Landing> {
        if self.phase != Phase::Falling {
            return None;
        }
        let alt = (self.pos.y - ground_y).max(0.0);

        // anti-air: only while the LZ is defended and you are visible, and
        // scaled by how readable this mode is
        if self.defended && alt > 3.0 {
            self.aa_taken += AA_DPS * self.mode.exposure() * dt;
        }

        // lateral authority is the difference between the two modes
        let lat = Vec3::new(steer.x, 0.0, steer.z);
        let lat = if lat.length() > 1.0 { lat.normalize() } else { lat };
        self.vel += lat * self.mode.steer() * dt;

        // DRAG APPROACHES TERMINAL, it does not snap to it. The first
        // version clamped hard, which meant folding at 30 m instantly
        // dropped you from 92 m/s to 41 and the hero landing could never
        // happen — the mechanic deleted itself. An exponential approach also
        // reads better: opening up at speed is a decelerating lurch you feel
        // over half a second rather than a teleport.
        let term = self.mode.terminal(g);
        self.vel.y -= g * dt;
        if self.vel.y < -term {
            const DRAG_K: f32 = 2.4;
            self.vel.y += (-term - self.vel.y) * (1.0 - (-DRAG_K * dt).exp());
        }
        // horizontal bleeds off; you cannot glide forever on the exit throw
        let drag = if self.mode == Mode::Robot { 0.55 } else { 0.18 };
        self.vel.x -= self.vel.x * drag * dt;
        self.vel.z -= self.vel.z * drag * dt;

        self.pos += self.vel * dt;

        if self.pos.y <= ground_y {
            self.pos.y = ground_y;
            return Some(self.touchdown(ground_y));
        }
        None
    }

    fn touchdown(&mut self, ground_y: f32) -> Landing {
        self.phase = Phase::Down;
        let speed = self.vel.length();
        let mut l = Landing {
            impact_speed: speed,
            miss_distance: (Vec3::new(self.pos.x, ground_y, self.pos.z)
                - Vec3::new(self.lz.x, ground_y, self.lz.z))
            .length(),
            aa_taken: self.aa_taken,
            ..Landing::default()
        };
        // THE HERO LANDING. Folded inside the window, arriving at speed, in
        // a body that can take it: the impact goes into the ground instead
        // of into you, and anything standing near the LZ wears it.
        let in_window = self.fold_alt.map_or(false, |a| a <= HERO_WINDOW_M);
        if self.mode == Mode::Robot && in_window && speed > SAFE_IMPACT {
            l.hero = true;
            l.shock = (speed - SAFE_IMPACT) * 0.9;
            l.shock_radius = HERO_RADIUS_M;
        } else if speed > SAFE_IMPACT {
            // arrived too fast without setting up for it
            l.self_dmg = (speed - SAFE_IMPACT) * 0.75;
        }
        l
    }

    /// Where you are currently going to land, if nothing changes. The line
    /// the HUD should be drawing — a drop is only skill-based if you can see
    /// the answer while there is still time to change it.
    pub fn predicted(&self, g: f32, ground_y: f32) -> Vec3 {
        let alt = (self.pos.y - ground_y).max(0.0);
        let term = self.mode.terminal(g);
        // time to fall the rest of the way at roughly terminal
        let t = (alt / term.max(1.0)).max(0.0);
        Vec3::new(self.pos.x + self.vel.x * t, ground_y, self.pos.z + self.vel.z * t)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fall(d: &mut Drop, steer: Vec3, secs: f32) -> Option<Landing> {
        for _ in 0..(secs * 60.0) as i32 {
            if let Some(l) = d.update(1.0 / 60.0, steer, 9.81, 0.0) {
                return Some(l);
            }
        }
        None
    }

    #[test]
    fn you_cannot_jump_through_a_shut_door() {
        let d = Drop::new(Vec3::ZERO, false);
        assert_eq!(d.can_jump(false, true, 900.0).unwrap_err(), "hangar door is not open");
        assert_eq!(d.can_jump(true, false, 900.0).unwrap_err(), "ship is on the ground");
        assert_eq!(d.can_jump(true, true, 8.0).unwrap_err(), "too low to drop");
        assert!(d.can_jump(true, true, 900.0).is_ok());
    }

    #[test]
    fn you_leave_with_the_ships_momentum() {
        // the thing that makes the pilot's line matter
        let mut d = Drop::new(Vec3::ZERO, false);
        let carry = Vec3::new(0.0, 0.0, -180.0);
        d.jump(Vec3::new(0.0, 800.0, 0.0), carry);
        assert_eq!(d.vel, carry);
        let l = fall(&mut d, Vec3::ZERO, 60.0).expect("never landed");
        // a fast pass throws you a long way down-track
        assert!(d.pos.z < -300.0, "the exit throw did nothing: {}", d.pos.z);
        assert!(l.miss_distance > 300.0);
    }

    #[test]
    fn a_banking_ship_throws_you_sideways_whether_you_meant_it_or_not() {
        let mut d = Drop::new(Vec3::ZERO, false);
        // carry_at on a rolling hull has a lateral component
        d.jump(Vec3::new(0.0, 700.0, 0.0), Vec3::new(60.0, 0.0, -100.0));
        fall(&mut d, Vec3::ZERO, 60.0);
        assert!(d.pos.x > 40.0, "the rotational term was lost: {}", d.pos.x);
    }

    #[test]
    fn a_dart_falls_twice_as_fast_as_a_parachute() {
        assert!(Mode::Vehicle.terminal(9.81) > Mode::Robot.terminal(9.81) * 2.0);
        // and terminal scales with gravity, so a low-g world is a slow drop
        assert!(Mode::Robot.terminal(1.6) < Mode::Robot.terminal(9.81));

        let mut fast = Drop::new(Vec3::ZERO, false);
        fast.jump(Vec3::new(0.0, 900.0, 0.0), Vec3::ZERO);
        let mut slow = Drop::new(Vec3::ZERO, false);
        slow.jump(Vec3::new(0.0, 900.0, 0.0), Vec3::ZERO);
        slow.transform(900.0);
        assert_eq!(slow.mode, Mode::Robot);

        let mut tf = 0;
        let mut ts = 0;
        while fast.phase == Phase::Falling && tf < 6000 {
            fast.update(1.0 / 60.0, Vec3::ZERO, 9.81, 0.0);
            tf += 1;
        }
        while slow.phase == Phase::Falling && ts < 6000 {
            slow.update(1.0 / 60.0, Vec3::ZERO, 9.81, 0.0);
            ts += 1;
        }
        // not exactly 2x: the dart spends the first seconds accelerating to
        // terminal, so the ratio of TOTAL times is smaller than the ratio of
        // terminal speeds
        assert!(ts as f32 > tf as f32 * 1.5, "the parachute was not slower: {tf} vs {ts}");
    }

    #[test]
    fn only_a_robot_can_fix_a_bad_exit() {
        // dropped 200 m off the LZ, in each mode, steering hard toward it
        let mk = |robot: bool| {
            let mut d = Drop::new(Vec3::ZERO, false);
            d.jump(Vec3::new(200.0, 900.0, 0.0), Vec3::ZERO);
            if robot {
                d.transform(900.0);
            }
            d
        };
        // steer PROPORTIONALLY toward the LZ, the way a player does — hard
        // over at first, easing as it closes. Holding full lock the whole
        // way just overshoots, which the first version of this test did and
        // then blamed the robot for.
        let guide = |d: &mut Drop| -> Landing {
            loop {
                let err = Vec3::new(d.lz.x - d.pos.x, 0.0, d.lz.z - d.pos.z);
                let want = (err * 0.05).clamp_length_max(1.0);
                if let Some(l) = d.update(1.0 / 60.0, want, 9.81, 0.0) {
                    return l;
                }
            }
        };
        let mut v = mk(false);
        let mut r = mk(true);
        let lv = guide(&mut v);
        let lr = guide(&mut r);
        assert!(lr.miss_distance < lv.miss_distance * 0.5,
            "steering did nothing: robot {} vs vehicle {}", lr.miss_distance, lv.miss_distance);
    }

    #[test]
    fn the_safe_descent_is_the_dangerous_one() {
        // a slow wide robot is an easier shot, so hanging under a defended
        // LZ costs more than diving at it
        assert!(Mode::Robot.exposure() > Mode::Vehicle.exposure() * 2.0);
        let mk = |robot: bool| {
            let mut d = Drop::new(Vec3::ZERO, true);
            d.jump(Vec3::new(0.0, 800.0, 0.0), Vec3::ZERO);
            if robot {
                d.transform(800.0);
            }
            d
        };
        let mut v = mk(false);
        let mut r = mk(true);
        let lv = fall(&mut v, Vec3::ZERO, 90.0).unwrap();
        let lr = fall(&mut r, Vec3::ZERO, 90.0).unwrap();
        assert!(lr.aa_taken > lv.aa_taken * 2.0,
            "the slow drop was not punished: {} vs {}", lr.aa_taken, lv.aa_taken);
    }

    #[test]
    fn an_undefended_lz_never_shoots_at_you() {
        let mut d = Drop::new(Vec3::ZERO, false);
        d.jump(Vec3::new(0.0, 800.0, 0.0), Vec3::ZERO);
        let l = fall(&mut d, Vec3::ZERO, 90.0).unwrap();
        assert_eq!(l.aa_taken, 0.0);
    }

    #[test]
    fn folding_at_the_last_moment_is_the_hero_landing() {
        let mut d = Drop::new(Vec3::ZERO, false);
        d.jump(Vec3::new(0.0, 600.0, 0.0), Vec3::ZERO);
        // dive all the way down, then fold inside the window
        let mut folded = false;
        let mut landing = None;
        for _ in 0..6000 {
            let alt = d.pos.y;
            if !folded && alt < HERO_WINDOW_M * 0.8 {
                d.transform(alt);
                folded = true;
            }
            if let Some(l) = d.update(1.0 / 60.0, Vec3::ZERO, 9.81, 0.0) {
                landing = Some(l);
                break;
            }
        }
        let l = landing.expect("never landed");
        assert!(l.hero, "a last-second fold was not a hero landing");
        assert!(l.shock > 0.0 && l.shock_radius > 0.0, "the crater did nothing");
        assert_eq!(l.self_dmg, 0.0, "the hero landing hurt the hero");
    }

    #[test]
    fn folding_too_early_is_safe_and_unremarkable() {
        let mut d = Drop::new(Vec3::ZERO, false);
        d.jump(Vec3::new(0.0, 600.0, 0.0), Vec3::ZERO);
        d.transform(600.0);
        let l = fall(&mut d, Vec3::ZERO, 90.0).unwrap();
        assert!(!l.hero, "folding at 600 m counted as a hero landing");
        assert_eq!(l.self_dmg, 0.0, "a parachute descent hurt");
        assert_eq!(l.shock, 0.0);
    }

    #[test]
    fn never_folding_at_all_means_you_wear_the_impact() {
        let mut d = Drop::new(Vec3::ZERO, false);
        d.jump(Vec3::new(0.0, 600.0, 0.0), Vec3::ZERO);
        let l = fall(&mut d, Vec3::ZERO, 90.0).unwrap();
        assert!(!l.hero);
        assert!(l.self_dmg > 20.0, "hitting at terminal in vehicle mode was free: {:?}", l);
        assert_eq!(l.shock, 0.0, "a face-first crash still cratered");
    }

    #[test]
    fn the_predicted_landing_moves_when_you_steer() {
        // a drop is only skill-based if you can see the answer in time
        let mut d = Drop::new(Vec3::ZERO, false);
        d.jump(Vec3::new(0.0, 900.0, 0.0), Vec3::new(0.0, 0.0, -120.0));
        d.transform(900.0);
        let before = d.predicted(9.81, 0.0);
        for _ in 0..120 {
            d.update(1.0 / 60.0, Vec3::new(1.0, 0.0, 0.0), 9.81, 0.0);
        }
        let after = d.predicted(9.81, 0.0);
        assert!(after.x > before.x + 1.0, "the marker did not follow the stick");
    }
}

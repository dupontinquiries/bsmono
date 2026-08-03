//! DOORS AND AIRLOCKS.
//!
//! A door is not a texture that slides. It is three things at once, and each
//! of them is a mechanic:
//!
//!   * **A hole that opens and closes.** The interior is an SDF carve; a
//!     door is a piece of that carve that comes and goes, so a closed door
//!     is solid hull and you cannot walk through it. Same `max(hull, -void)`
//!     as everything else.
//!   * **A size class.** A personnel door does not admit a car. `interior.rs`
//!     already worries about whether a corridor is drivable; a door is where
//!     that gets decided in practice, and "I drove the spine and could not
//!     get into the bay" is exactly the kind of thing that should be a rule
//!     rather than a surprise.
//!   * **A pressure boundary.** Which is where it stops being scenery.
//!
//! THE AIRLOCK IS AN INTERLOCK, and that is the whole design. Two doors that
//! can never both be open. Not as a warning, not as a scripted sequence — as
//! a refusal: ask for the outer door while the inner is anything but shut
//! and it says no. Everything interesting falls out of that one rule. You
//! cannot rush a cycle. A fight in an airlock is a fight over a switch. And
//! if someone shoots the outer door while the lock is up to pressure, the
//! air leaves, and it takes with it everything that is not bolted down.
//!
//! DECOMPRESSION IS A FORCE, not an effect. `breach` returns the gust, and
//! it points out of the hole, and it is proportional to the pressure that
//! was there. Venting a lock first is what makes it safe, which means the
//! two-minute cycle you were impatient about is the thing that saves you.
//!
//! FAILURE MODES ARE OPPOSITE ON PURPOSE. Lose power and an airlock fails
//! CLOSED, because a lock that opens when the lights go out is a way to kill
//! a crew. A hangar door fails OPEN, because a launch bay that traps its
//! own craft is worse than one left gaping. When the ship is coming down —
//! `bigbody::Drive::Falling` — that difference is the difference between
//! getting out and not.

use glam::Vec3;

/// What can fit through.
#[derive(Clone, Copy, PartialEq, Eq, Debug, PartialOrd, Ord)]
pub enum Gauge {
    /// a bot on foot
    Personnel,
    /// a car, a tank, a hovercar
    Vehicle,
    /// a flyer, or a bay big enough to launch one
    Launch,
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Kind {
    /// ordinary interior door
    Hatch,
    /// holds pressure between sections; fails closed
    Bulkhead,
    /// the inboard half of an airlock
    Inner,
    /// the outboard half — vacuum on the far side
    Outer,
    /// the hangar's own door; fails open
    Launch,
}

impl Kind {
    /// With no power, does it end up open or shut?
    ///
    /// Opposite by design. A lock that opens when the lights go out kills a
    /// crew; a launch bay that traps its own craft is worse than one left
    /// gaping.
    pub fn fails_open(self) -> bool {
        matches!(self, Kind::Launch)
    }
    /// Does it separate pressure from vacuum?
    pub fn seals(self) -> bool {
        matches!(self, Kind::Bulkhead | Kind::Inner | Kind::Outer)
    }
}

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum State {
    Shut,
    Opening,
    Open,
    Closing,
    /// bent, shot or unpowered — it can still be forced, slowly
    Jammed,
}

/// Seconds for a powered door to travel, by gauge. Big doors are slow, which
/// is why a hangar is a commitment and a hatch is not.
fn travel_sec(g: Gauge) -> f32 {
    match g {
        Gauge::Personnel => 0.6,
        Gauge::Vehicle => 1.6,
        Gauge::Launch => 4.5,
    }
}

/// Forcing one by hand takes about six times as long.
pub const FORCE_MUL: f32 = 6.0;

#[derive(Clone, Copy, Debug)]
pub struct Door {
    pub kind: Kind,
    pub gauge: Gauge,
    pub state: State,
    /// 0 shut .. 1 open
    pub travel: f32,
    /// centre of the passage, in hull-local metres
    pub centre: Vec3,
    /// half extents of the hole it makes when open
    pub half: Vec3,
    pub powered: bool,
    forcing: bool,
}

impl Door {
    pub fn new(kind: Kind, gauge: Gauge, centre: Vec3, half: Vec3) -> Door {
        Door { kind, gauge, state: State::Shut, travel: 0.0, centre, half, powered: true, forcing: false }
    }

    /// Will this fit through?
    ///
    /// Gauge is ordered, so a launch door passes everything and a personnel
    /// door passes only what walks.
    pub fn admits(&self, g: Gauge) -> bool {
        g <= self.gauge
    }

    pub fn open(&mut self) -> bool {
        if matches!(self.state, State::Open | State::Opening) {
            return true;
        }
        if !self.powered && !self.forcing {
            return false;
        }
        self.state = State::Opening;
        true
    }

    pub fn close(&mut self) -> bool {
        if matches!(self.state, State::Shut | State::Closing) {
            return true;
        }
        if !self.powered && !self.forcing {
            return false;
        }
        self.state = State::Closing;
        true
    }

    /// Crank it by hand. Works with no power and works on a jam, and takes
    /// FORCE_MUL times as long — long enough that doing it while something
    /// is shooting at you is a real decision.
    pub fn force(&mut self, opening: bool) {
        self.forcing = true;
        self.state = if opening { State::Opening } else { State::Closing };
    }

    /// Cut the power. The door goes where its kind says it goes.
    pub fn depower(&mut self) {
        self.powered = false;
        if self.state == State::Opening || self.state == State::Closing {
            self.state = if self.kind.fails_open() { State::Opening } else { State::Closing };
        } else if self.kind.fails_open() && self.state == State::Shut {
            self.state = State::Opening;
        } else if !self.kind.fails_open() && self.state == State::Open {
            self.state = State::Closing;
        }
    }

    pub fn jam(&mut self) {
        self.state = State::Jammed;
        self.forcing = false;
    }

    pub fn update(&mut self, dt: f32) {
        let rate = 1.0 / (travel_sec(self.gauge) * if self.forcing { FORCE_MUL } else { 1.0 });
        match self.state {
            State::Opening => {
                self.travel = (self.travel + rate * dt).min(1.0);
                if self.travel >= 1.0 {
                    self.state = State::Open;
                    self.forcing = false;
                }
            }
            State::Closing => {
                self.travel = (self.travel - rate * dt).max(0.0);
                if self.travel <= 0.0 {
                    self.state = State::Shut;
                    self.forcing = false;
                }
            }
            _ => {}
        }
    }

    /// Is the hole passable enough to get through? Half-open is not a door
    /// you walk through, it is a door you wait for.
    pub fn passable(&self) -> bool {
        self.travel > 0.75
    }

    /// The void this door contributes to the interior carve, if any. A shut
    /// door contributes nothing, which is what makes it solid.
    pub fn void(&self) -> Option<(Vec3, Vec3)> {
        if self.travel <= 0.01 {
            return None;
        }
        // the hole grows along its short axis as the door slides back
        let mut h = self.half;
        h.y *= self.travel;
        Some((self.centre, h))
    }
}

/// The gust that leaves when a pressure boundary opens onto vacuum.
#[derive(Clone, Copy, Debug, Default)]
pub struct Gust {
    /// metres per second of outrush at the hole
    pub speed: f32,
    /// which way it blows
    pub dir: Vec3,
    /// seconds until it dies
    pub life: f32,
}

/// Two doors and the volume between them.
pub struct Airlock {
    pub inner: Door,
    pub outer: Door,
    /// 0 vacuum .. 1 shipboard
    pub pressure: f32,
    /// volume of the chamber, m³ — a big lock takes longer to cycle
    pub volume: f32,
    pub gust: Option<Gust>,
}

/// Pumping air back in is slow. Dumping it is not — you open a valve and
/// physics does the rest, which is why venting is the fast direction and
/// impatience pushes you toward the safe one.
pub const VENT_RATE: f32 = 0.55;
pub const PRESSURISE_RATE: f32 = 0.16;

impl Airlock {
    pub fn new(centre: Vec3, half: Vec3, gauge: Gauge) -> Airlock {
        let along = Vec3::new(0.0, 0.0, half.z);
        Airlock {
            inner: Door::new(Kind::Inner, gauge, centre - along, half * 0.9),
            outer: Door::new(Kind::Outer, gauge, centre + along, half * 0.9),
            pressure: 1.0,
            volume: (half.x * half.y * half.z * 8.0).max(1.0),
            gust: None,
        }
    }

    /// THE INTERLOCK. This one function is the mechanic.
    ///
    /// Refuses rather than warns, and says which rule stopped it, because a
    /// player who is told "the inner door is open" learns the system and a
    /// player whose button does nothing learns that the game is broken.
    pub fn request(&mut self, outer: bool, open: bool) -> Result<(), &'static str> {
        if !open {
            let d = if outer { &mut self.outer } else { &mut self.inner };
            d.close();
            return Ok(());
        }
        if outer {
            if self.inner.state != State::Shut {
                return Err("inner door is not shut");
            }
            if self.pressure > 0.05 {
                return Err("chamber is still pressurised");
            }
            self.outer.open();
        } else {
            if self.outer.state != State::Shut {
                return Err("outer door is not shut");
            }
            if self.pressure < 0.95 {
                return Err("chamber is not pressurised");
            }
            self.inner.open();
        }
        Ok(())
    }

    /// Dump the air. Fast, and the honest way out.
    pub fn vent(&mut self) {
        if self.outer.state == State::Shut {
            self.pressure = (self.pressure - 1.0).max(0.0);
        }
    }

    /// SHOOT THE DOOR. The other way to open an airlock, and the reason the
    /// slow one exists.
    ///
    /// A breach onto vacuum with the chamber up to pressure throws
    /// everything loose out of the hole. Breach an already-vented lock and
    /// nothing happens, because there was nothing in it — venting first is
    /// what makes it safe.
    pub fn breach(&mut self, which_outer: bool) -> Gust {
        let d = if which_outer { &mut self.outer } else { &mut self.inner };
        d.jam();
        d.travel = 1.0;
        let p = self.pressure;
        self.pressure = 0.0;
        let g = Gust {
            speed: p * 42.0,
            dir: if which_outer { Vec3::Z } else { -Vec3::Z },
            life: p * 2.5,
        };
        self.gust = if p > 0.02 { Some(g) } else { None };
        self.gust.unwrap_or_default()
    }

    /// Force on something of this mass sitting in the outrush.
    pub fn gust_force(&self, mass: f32) -> Vec3 {
        match self.gust {
            Some(g) => g.dir * (g.speed * g.speed * 0.02 / mass.max(1.0)),
            None => Vec3::ZERO,
        }
    }

    pub fn update(&mut self, dt: f32) {
        self.inner.update(dt);
        self.outer.update(dt);
        // scale by volume: a launch-gauge lock is a room, not a cupboard
        let vscale = (12.0 / self.volume).clamp(0.15, 1.0);
        if self.outer.travel > 0.01 {
            self.pressure = (self.pressure - VENT_RATE * vscale * dt * 4.0).max(0.0);
        } else if self.inner.travel > 0.01 {
            self.pressure = (self.pressure + PRESSURISE_RATE * vscale * dt * 4.0).min(1.0);
        }
        if let Some(g) = &mut self.gust {
            g.life -= dt;
            if g.life <= 0.0 {
                self.gust = None;
            }
        }
    }

    /// Both open at once should be impossible. Exposed so a test — and an
    /// assertion in a debug build — can say so out loud.
    pub fn interlock_held(&self) -> bool {
        !(self.inner.travel > 0.01 && self.outer.travel > 0.01)
    }

    /// Seconds to get from here to standing outside, cycling properly.
    pub fn cycle_estimate(&self) -> f32 {
        let vscale = (12.0 / self.volume).clamp(0.15, 1.0);
        let vent = self.pressure / (VENT_RATE * vscale * 4.0);
        vent + travel_sec(self.outer.gauge) * 2.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn lock() -> Airlock {
        Airlock::new(Vec3::ZERO, Vec3::new(2.0, 2.0, 3.0), Gauge::Personnel)
    }
    fn run(a: &mut Airlock, secs: f32) {
        for _ in 0..(secs * 60.0) as i32 {
            a.update(1.0 / 60.0);
        }
    }

    #[test]
    fn both_doors_can_never_be_open_at_once() {
        let mut a = lock();
        a.request(false, true).unwrap(); // inner, chamber is pressurised
        run(&mut a, 2.0);
        assert!(a.inner.passable());
        // the outer door must refuse, and say why
        assert_eq!(a.request(true, true).unwrap_err(), "inner door is not shut");
        // even after shutting it, pressure still blocks
        a.request(false, false).unwrap();
        run(&mut a, 2.0);
        assert_eq!(a.request(true, true).unwrap_err(), "chamber is still pressurised");
        assert!(a.interlock_held());
    }

    #[test]
    fn cycling_properly_gets_you_outside() {
        let mut a = lock();
        a.request(false, true).unwrap();
        run(&mut a, 2.0);
        a.request(false, false).unwrap();
        run(&mut a, 2.0);
        a.vent();
        a.request(true, true).unwrap();
        run(&mut a, 3.0);
        assert!(a.outer.passable(), "the outer door never opened");
        assert!(a.pressure < 0.05);
        assert!(a.interlock_held());
    }

    #[test]
    fn venting_is_faster_than_pressurising() {
        // impatience should push you toward the safe direction
        assert!(VENT_RATE > PRESSURISE_RATE * 2.0);
        let mut a = lock();
        a.outer.travel = 0.5;
        let t0 = a.pressure;
        run(&mut a, 1.0);
        let vented = t0 - a.pressure;
        let mut b = lock();
        b.pressure = 0.0;
        b.outer.travel = 0.0;
        b.inner.travel = 0.5;
        run(&mut b, 1.0);
        assert!(vented > b.pressure, "pumping in beat dumping out");
    }

    #[test]
    fn a_big_lock_takes_longer_to_cycle() {
        let small = Airlock::new(Vec3::ZERO, Vec3::new(2.0, 2.0, 3.0), Gauge::Personnel);
        let big = Airlock::new(Vec3::ZERO, Vec3::new(9.0, 6.0, 12.0), Gauge::Launch);
        assert!(big.cycle_estimate() > small.cycle_estimate() * 2.0,
            "{} vs {}", small.cycle_estimate(), big.cycle_estimate());
    }

    #[test]
    fn shooting_a_pressurised_lock_blows_everything_out() {
        let mut a = lock();
        assert!(a.pressure > 0.9);
        let g = a.breach(true);
        assert!(g.speed > 30.0, "the air left politely: {}", g.speed);
        assert!(g.dir.z > 0.0, "it blew inboard");
        assert_eq!(a.pressure, 0.0);
        assert_eq!(a.outer.state, State::Jammed, "a shot door still works");
        // and a light thing is thrown harder than a heavy one
        let light = a.gust_force(1.0).length();
        let heavy = a.gust_force(40.0).length();
        assert!(light > heavy * 10.0);
    }

    #[test]
    fn venting_first_is_what_makes_a_breach_survivable() {
        let mut a = lock();
        a.vent();
        assert_eq!(a.pressure, 0.0);
        let g = a.breach(true);
        assert_eq!(g.speed, 0.0, "an empty lock still had a gale in it");
        assert!(a.gust.is_none());
        assert_eq!(a.gust_force(1.0), Vec3::ZERO);
    }

    #[test]
    fn the_gust_dies_rather_than_blowing_forever() {
        let mut a = lock();
        a.breach(true);
        assert!(a.gust.is_some());
        run(&mut a, 5.0);
        assert!(a.gust.is_none(), "still venting five seconds later");
    }

    #[test]
    fn power_loss_shuts_a_lock_and_opens_a_hangar() {
        // opposite on purpose, and this is the difference between getting
        // out of a falling ship and not
        let mut inner = Door::new(Kind::Inner, Gauge::Personnel, Vec3::ZERO, Vec3::ONE);
        inner.open();
        for _ in 0..60 {
            inner.update(1.0 / 60.0);
        }
        assert_eq!(inner.state, State::Open);
        inner.depower();
        for _ in 0..300 {
            inner.update(1.0 / 60.0);
        }
        assert_eq!(inner.state, State::Shut, "an unpowered airlock stayed open");

        let mut bay = Door::new(Kind::Launch, Gauge::Launch, Vec3::ZERO, Vec3::ONE);
        assert_eq!(bay.state, State::Shut);
        bay.depower();
        for _ in 0..600 {
            bay.update(1.0 / 60.0);
        }
        assert_eq!(bay.state, State::Open, "an unpowered launch bay trapped its craft");
    }

    #[test]
    fn an_unpowered_door_can_still_be_forced_but_slowly() {
        let mut d = Door::new(Kind::Hatch, Gauge::Personnel, Vec3::ZERO, Vec3::ONE);
        d.powered = false;
        assert!(!d.open(), "an unpowered door opened itself");
        d.force(true);
        // it does move, eventually
        for _ in 0..30 {
            d.update(1.0 / 60.0);
        }
        assert!(d.travel > 0.0 && d.travel < 1.0, "forcing was instant or nothing");
        for _ in 0..600 {
            d.update(1.0 / 60.0);
        }
        assert_eq!(d.state, State::Open);
    }

    #[test]
    fn a_personnel_door_will_not_pass_a_tank() {
        let hatch = Door::new(Kind::Hatch, Gauge::Personnel, Vec3::ZERO, Vec3::ONE);
        let bay = Door::new(Kind::Launch, Gauge::Launch, Vec3::ZERO, Vec3::ONE);
        assert!(hatch.admits(Gauge::Personnel));
        assert!(!hatch.admits(Gauge::Vehicle), "a car fitted through a crew hatch");
        assert!(!hatch.admits(Gauge::Launch));
        // a launch door passes everything
        for g in [Gauge::Personnel, Gauge::Vehicle, Gauge::Launch] {
            assert!(bay.admits(g));
        }
    }

    #[test]
    fn a_shut_door_is_solid_and_a_half_open_one_is_not_a_doorway() {
        let mut d = Door::new(Kind::Bulkhead, Gauge::Vehicle, Vec3::ZERO, Vec3::new(2.0, 2.0, 0.5));
        assert!(d.void().is_none(), "a shut door left a hole in the hull");
        assert!(!d.passable());
        d.open();
        d.update(0.4);
        assert!(d.void().is_some());
        assert!(!d.passable(), "you walked through a door that was 25% open");
        d.update(2.0);
        assert!(d.passable());
    }

    #[test]
    fn a_bigger_door_takes_longer_to_move() {
        assert!(travel_sec(Gauge::Launch) > travel_sec(Gauge::Vehicle));
        assert!(travel_sec(Gauge::Vehicle) > travel_sec(Gauge::Personnel));
    }
}

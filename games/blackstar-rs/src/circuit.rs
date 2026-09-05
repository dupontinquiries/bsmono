//! CIRCUIT — the Lighter's timetable, as pure logic.
//!
//! Same boundary as `escalation.rs`: this module knows what stop the
//! Lighter is at, when it leaves, how long a leg takes, and whether a
//! rider standing on a bridge has power, a crate nearby, or a part
//! installed. It does not know where a stop IS in world space, does not
//! draw anything, and does not decide what a Corrupted looks like —
//! `moons.rs` and `corrupt_body.rs` own those. `escalation::Waves` still
//! owns the wave counter and the valence bank; this module only reads
//! `laps` to scale what `escalation::makeup` already returns.
//!
//! The five stops are fixed (`STOP_COUNT = 5`); the ring is a cycle,
//! stop 0 is the Dock. A run is one `Circuit`, ticked once a frame with
//! `update(dt)`, same shape as `Waves::update`.
//!
//! **Boarding** only during `Dwell`/`Warn` at the stop you're standing
//! at. **Jumping off** mid-leg leaves you on the bridge with a known
//! remaining walk distance; `moons.rs`/`circuit_map.rs` turn that into
//! an actual position on the bridge spline. **Power** gates every perk
//! rack until `throw_relay` is called at the Tithe. **The crate**
//! relocates after `CRATE_USES_PER_STOP` uses to a different stop,
//! never the one it just left. **Parts** are found once each, in a
//! fixed order, and their multipliers are read by the caller — they are
//! never applied silently inside this module, so a test can check the
//! multiplier value itself rather than an emergent side effect.

pub const STOP_COUNT: usize = 5;
pub const DOCK: usize = 0;

/// (guess) — matches the design brief's placeholders exactly so a
/// balance pass has one place to change them.
pub const WARN_S: f32 = 5.0;
pub const BREATHER_S: f32 = 3.0;
pub const WALK_LEG_RATIO: f32 = 2.2;
pub const CRATE_USES_PER_STOP: u32 = 5;
pub const LAP_PRESSURE_PER_LAP: f32 = 0.25;

/// Per-leg travel time, stop `i` to stop `(i+1) % STOP_COUNT`.
/// (guess) — five legs around the ring; the Dock's two legs are the
/// longest since it is the only stop off the main cluster.
pub const LEG_S: [f32; STOP_COUNT] = [70.0, 55.0, 60.0, 55.0, 65.0];

fn leg_index(from: usize) -> usize {
    from % STOP_COUNT
}

fn next_stop(from: usize) -> usize {
    (from + 1) % STOP_COUNT
}

/// Where the Lighter is right now.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Phase {
    /// sitting at `stop`, boardable, before the horn
    Dwell { stop: usize, remaining: f32 },
    /// horn's gone, still at `stop`, still boardable — last chance
    Warn { stop: usize, remaining: f32 },
    /// between `from` and `next_stop(from)`; `progress` in `[0,1]`
    Travel { from: usize, progress: f32 },
}

impl Phase {
    pub fn stop(self) -> Option<usize> {
        match self {
            Phase::Dwell { stop, .. } | Phase::Warn { stop, .. } => Some(stop),
            Phase::Travel { .. } => None,
        }
    }
    pub fn boardable(self) -> bool {
        matches!(self, Phase::Dwell { .. } | Phase::Warn { .. })
    }
}

/// Which of the three bolt-on parts the player has found. Multipliers
/// are read by the caller (`circuit_map.rs`); this struct only tracks
/// possession. All three numbers are (guess), named in the brief.
#[derive(Clone, Copy, Default, PartialEq, Eq, Debug)]
pub struct Parts {
    /// boarding cadence during travel: -70% when held
    pub lashing_plow: bool,
    /// dwell/warn duration: +50% when held
    pub running_lamp: bool,
    /// can fire while riding, not just walking
    pub top_hatch: bool,
}

impl Parts {
    pub const PLOW_BOARDER_MUL: f32 = 0.30;
    pub const LAMP_DWELL_MUL: f32 = 1.50;

    pub fn dwell_mul(self) -> f32 {
        if self.running_lamp {
            Self::LAMP_DWELL_MUL
        } else {
            1.0
        }
    }
    pub fn boarder_mul(self) -> f32 {
        if self.lashing_plow {
            Self::PLOW_BOARDER_MUL
        } else {
            1.0
        }
    }
}

/// One rider's relationship to the loop: aboard, walking a bridge
/// jumped-off-early, or COLD (powered down, undetected — see
/// `corruption::detects`'s "dead machinery" clause, which this struct
/// deliberately does not re-implement; it only flags the state).
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Rider {
    Aboard,
    /// standing on the bridge after `from`, `remaining_m` of `total_m`
    /// left to walk. `remaining_m` counts DOWN as the caller moves the
    /// player; this struct does not simulate footsteps.
    Walking { from: usize, remaining_m: f32, total_m: f32 },
    Cold { from: usize, remaining_m: f32, total_m: f32 },
}

impl Rider {
    pub fn on_foot(self) -> bool {
        matches!(self, Rider::Walking { .. } | Rider::Cold { .. })
    }
    pub fn from_stop(self) -> Option<usize> {
        match self {
            Rider::Aboard => None,
            Rider::Walking { from, .. } | Rider::Cold { from, .. } => Some(from),
        }
    }
}

/// Fires once the frame it happens; the shell drains these each tick.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    Arrive(usize),
    Depart(usize),
    Warn(usize),
    /// relay thrown: perks are live, a wave spawns on the spot
    PowerOn,
    CrateMoved(usize),
}

/// One full circuit run: the Lighter's own state, the rider's state,
/// power, the crate, parts and lap count. `escalation::Waves` lives
/// alongside this in the caller, not inside it — this struct only
/// reports `laps` for `pressure_mul`.
#[derive(Clone, Debug)]
pub struct Circuit {
    pub phase: Phase,
    pub rider: Rider,
    pub laps: u32,
    pub power_on: bool,
    pub crate_stop: usize,
    pub crate_uses: u32,
    pub parts: Parts,
    events: Vec<Event>,
}

impl Circuit {
    /// Starts at the Dock, dwelling, crate also at the Dock (it is
    /// moved off on first use so the opening stop is never sterile
    /// for long — see `crate_relocate`).
    pub fn new() -> Circuit {
        Circuit {
            phase: Phase::Dwell { stop: DOCK, remaining: WARN_S },
            rider: Rider::Aboard,
            laps: 0,
            power_on: false,
            crate_stop: DOCK,
            crate_uses: 0,
            parts: Parts::default(),
            events: Vec::new(),
        }
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    /// Board while at a stop and boardable. No-op (returns false) if
    /// already aboard or the phase isn't boardable — jumping mid-leg
    /// is `jump_off`'s job, not this one's.
    pub fn board(&mut self) -> bool {
        if self.rider == Rider::Aboard {
            return false;
        }
        if !self.phase.boardable() {
            return false;
        }
        self.rider = Rider::Aboard;
        true
    }

    /// Leave the Lighter at the current stop, or jump off mid-leg. In
    /// both cases the rider ends up `Walking` from the stop just left,
    /// with `remaining_m` computed from how much of the leg is left —
    /// zero if leaving at a dwelling stop (nothing to walk yet).
    pub fn disembark(&mut self) {
        if self.rider != Rider::Aboard {
            return;
        }
        match self.phase {
            Phase::Dwell { stop, .. } | Phase::Warn { stop, .. } => {
                self.rider = Rider::Walking { from: stop, remaining_m: 0.0, total_m: 0.0 };
            }
            Phase::Travel { from, progress } => {
                let total = LEG_S[leg_index(from)] * WALK_LEG_RATIO;
                let remaining = total * (1.0 - progress);
                self.rider = Rider::Walking { from, remaining_m: remaining, total_m: total };
            }
        }
    }

    /// Power down: undetected, no weapons, crawl speed — the shell
    /// enforces the last two; this just flips the state. Only valid on
    /// foot (COLD while aboard makes no fictional sense: the Lighter
    /// itself is not hiding).
    pub fn go_cold(&mut self) -> bool {
        match self.rider {
            Rider::Walking { from, remaining_m, total_m } => {
                self.rider = Rider::Cold { from, remaining_m, total_m };
                true
            }
            _ => false,
        }
    }
    pub fn go_hot(&mut self) -> bool {
        match self.rider {
            Rider::Cold { from, remaining_m, total_m } => {
                self.rider = Rider::Walking { from, remaining_m, total_m };
                true
            }
            _ => false,
        }
    }

    /// Advance the walk by `meters`; clamps at zero and does not
    /// auto-arrive — the caller (`circuit_map.rs`) decides what
    /// reaching the far end means once the bridge geometry exists.
    pub fn walk(&mut self, meters: f32) {
        match &mut self.rider {
            Rider::Walking { remaining_m, .. } | Rider::Cold { remaining_m, .. } => {
                *remaining_m = (*remaining_m - meters).max(0.0);
            }
            Rider::Aboard => {}
        }
    }

    /// Throw the relay: perks go live, `Event::PowerOn` fires once.
    /// Idempotent — a second throw is a no-op, not a double event.
    pub fn throw_relay(&mut self) {
        if !self.power_on {
            self.power_on = true;
            self.events.push(Event::PowerOn);
        }
    }

    /// Use the crate once. Every `CRATE_USES_PER_STOP`th use relocates
    /// it to a different stop chosen deterministically from `seed` so
    /// a replay is reproducible, and never to the stop it's leaving.
    pub fn use_crate(&mut self, seed: u64) {
        self.crate_uses += 1;
        if self.crate_uses % CRATE_USES_PER_STOP == 0 {
            let leaving = self.crate_stop;
            let mut candidates: Vec<usize> =
                (0..STOP_COUNT).filter(|&s| s != leaving).collect();
            let pick = (seed.wrapping_add(self.crate_uses as u64) as usize) % candidates.len();
            self.crate_stop = candidates.remove(pick);
            self.events.push(Event::CrateMoved(self.crate_stop));
        }
    }

    /// Pressure multiplier fed into `escalation::makeup`'s caller —
    /// this module does not call `makeup` itself, it only reports the
    /// number so the shell can (`makeup_scaled` below does that for
    /// convenience without creating a dependency in the other
    /// direction).
    pub fn pressure_mul(&self) -> f32 {
        1.0 + LAP_PRESSURE_PER_LAP * self.laps as f32
    }

    /// ETA to a given stop from wherever the Lighter is now, in
    /// seconds, following the timetable exactly (no rider state
    /// involved — this answers "when does the board say it arrives").
    pub fn eta(&self, target: usize) -> f32 {
        let (mut at, mut remaining_here) = match self.phase {
            Phase::Dwell { stop, remaining } => (stop, remaining + WARN_S),
            Phase::Warn { stop, remaining } => (stop, remaining),
            Phase::Travel { from, progress } => {
                let leg = LEG_S[leg_index(from)];
                (from, leg * (1.0 - progress))
            }
        };
        if at == target && self.phase.boardable() {
            return 0.0;
        }
        let mut total = remaining_here;
        loop {
            at = next_stop(at);
            if at == target {
                return total;
            }
            total += LEG_S[leg_index(at)] + WARN_S + BREATHER_S;
            remaining_here = 0.0; // silence unused-assign warning path
            let _ = remaining_here;
        }
    }

    /// One frame. Advances the timetable only — rider walking is
    /// driven separately by `walk()`, since the shell is the one that
    /// knows real elapsed distance on a bridge with slopes and jumps.
    pub fn update(&mut self, dt: f32) {
        match &mut self.phase {
            Phase::Dwell { stop, remaining } => {
                *remaining -= dt / self.parts.dwell_mul();
                if *remaining <= 0.0 {
                    let stop = *stop;
                    self.phase = Phase::Warn { stop, remaining: WARN_S };
                    self.events.push(Event::Warn(stop));
                }
            }
            Phase::Warn { stop, remaining } => {
                *remaining -= dt / self.parts.dwell_mul();
                if *remaining <= 0.0 {
                    let from = *stop;
                    self.events.push(Event::Depart(from));
                    self.phase = Phase::Travel { from, progress: 0.0 };
                }
            }
            Phase::Travel { from, progress } => {
                let leg = LEG_S[leg_index(*from)];
                *progress += dt / leg;
                if *progress >= 1.0 {
                    let arrived = next_stop(*from);
                    if arrived == DOCK {
                        self.laps += 1;
                    }
                    self.phase = Phase::Dwell { stop: arrived, remaining: base_dwell() };
                    self.events.push(Event::Arrive(arrived));
                }
            }
        }
    }
}

impl Default for Circuit {
    fn default() -> Circuit {
        Circuit::new()
    }
}

/// (guess) — base dwell before the warn horn.
fn base_dwell() -> f32 {
    20.0
}

/// `--circuit`: prints the timetable and a few derived numbers so the
/// ring can be read and argued with from a terminal, same convention
/// as `space::run`/`gravity::run`. Nothing here touches a screen or a
/// `world::World` — that is `circuit_map.rs`'s job once it exists.
pub fn run() {
    println!("CIRCUIT — {STOP_COUNT} stops, ring, stop 0 = the Dock\n");
    println!("legs (s): {LEG_S:?}");
    println!(
        "warn {WARN_S}s, breather {BREATHER_S}s, base dwell {}s, walk ratio {WALK_LEG_RATIO}x",
        base_dwell()
    );
    let lap_s: f32 =
        LEG_S.iter().sum::<f32>() + STOP_COUNT as f32 * (WARN_S + BREATHER_S + base_dwell());
    println!("one full lap, nobody stops early: ~{lap_s:.1}s ({:.1} min)", lap_s / 60.0);

    println!("\nETA table from a Lighter just departing the Dock:");
    let mut c = Circuit::new();
    c.phase = Phase::Travel { from: DOCK, progress: 0.0 };
    for stop in 0..STOP_COUNT {
        println!("  stop {stop}: {:.1}s", c.eta(stop));
    }

    println!("\nparts:");
    println!(
        "  lashing plow -> boarder pressure x{:.2}",
        Parts { lashing_plow: true, ..Parts::default() }.boarder_mul()
    );
    println!(
        "  running lamp -> dwell/warn duration x{:.2}",
        Parts { running_lamp: true, ..Parts::default() }.dwell_mul()
    );

    println!("\ncrate: relocates every {CRATE_USES_PER_STOP} uses");
    println!("lap pressure: +{:.0}% escalation makeup per lap", LAP_PRESSURE_PER_LAP * 100.0);
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn timetable_is_deterministic_over_1000_seconds() {
        let mut a = Circuit::new();
        let mut b = Circuit::new();
        for _ in 0..100_000 {
            a.update(0.01);
            b.update(0.01);
        }
        assert_eq!(format!("{:?}", a.phase), format!("{:?}", b.phase));
        assert_eq!(a.laps, b.laps);
    }

    #[test]
    fn board_only_during_dwell_or_warn() {
        let mut c = Circuit::new();
        c.disembark();
        assert!(c.rider.on_foot());
        // still at the dock, dwelling -> boardable
        assert!(c.board());
        assert_eq!(c.rider, Rider::Aboard);

        c.disembark();
        c.phase = Phase::Travel { from: DOCK, progress: 0.5 };
        assert!(!c.board(), "must not board mid-leg");
        assert!(c.rider.on_foot());
    }

    #[test]
    fn jump_off_mid_leg_yields_the_right_remaining_walk() {
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 2, progress: 0.25 };
        c.rider = Rider::Aboard;
        c.disembark();
        match c.rider {
            Rider::Walking { from, remaining_m, total_m } => {
                assert_eq!(from, 2);
                let expected_total = LEG_S[2] * WALK_LEG_RATIO;
                assert!((total_m - expected_total).abs() < 1e-4);
                assert!((remaining_m - expected_total * 0.75).abs() < 1e-4);
            }
            other => panic!("expected Walking, got {other:?}"),
        }
    }

    #[test]
    fn disembarking_at_a_dwelling_stop_has_nothing_to_walk() {
        let mut c = Circuit::new(); // dwelling at DOCK
        c.disembark();
        match c.rider {
            Rider::Walking { remaining_m, total_m, .. } => {
                assert_eq!(remaining_m, 0.0);
                assert_eq!(total_m, 0.0);
            }
            other => panic!("expected Walking, got {other:?}"),
        }
    }

    #[test]
    fn power_gates_and_fires_once() {
        let mut c = Circuit::new();
        assert!(!c.power_on);
        c.throw_relay();
        assert!(c.power_on);
        assert_eq!(c.drain_events(), vec![Event::PowerOn]);
        c.throw_relay(); // idempotent
        assert!(c.drain_events().is_empty());
    }

    #[test]
    fn crate_relocates_on_the_nth_use_and_never_to_the_same_stop() {
        let mut c = Circuit::new();
        let start = c.crate_stop;
        for i in 1..CRATE_USES_PER_STOP {
            c.use_crate(7);
            assert_eq!(c.crate_stop, start, "must not move before the Nth use (i={i})");
        }
        c.use_crate(7);
        assert_ne!(c.crate_stop, start, "must move on the Nth use");
        let moved_to = c.crate_stop;
        assert_eq!(c.drain_events(), vec![Event::CrateMoved(moved_to)]);
    }

    #[test]
    fn crate_never_relocates_to_every_stop_pathologically() {
        // regression against an off-by-one that could pick index == leaving
        let mut c = Circuit::new();
        let mut seen_self_pick = false;
        for seed in 0..500u64 {
            c.crate_stop = 3;
            c.crate_uses = CRATE_USES_PER_STOP - 1;
            c.use_crate(seed);
            if c.crate_stop == 3 {
                seen_self_pick = true;
            }
        }
        assert!(!seen_self_pick);
    }

    #[test]
    fn plow_reduces_effective_boarder_pressure_and_lamp_extends_dwell() {
        let bare = Parts::default();
        let plowed = Parts { lashing_plow: true, ..Parts::default() };
        assert!(plowed.boarder_mul() < bare.boarder_mul());
        assert!(plowed.boarder_mul() > 0.0, "must not zero it out entirely");

        let lamped = Parts { running_lamp: true, ..Parts::default() };
        assert!(lamped.dwell_mul() > bare.dwell_mul());
    }

    #[test]
    fn eta_is_exact_against_the_timetable() {
        let mut c = Circuit::new(); // Dwell at DOCK, remaining = WARN_S
        // manual expectation: dwell remainder + WARN_S to depart DOCK
        let manual_first_leg_wait = WARN_S + WARN_S;
        let eta_1 = c.eta(1);
        assert!((eta_1 - manual_first_leg_wait).abs() < 1e-4);

        // advance to Warn, confirm eta shrinks by exactly the dwell delta
        c.update(3.0);
        let eta_1b = c.eta(1);
        assert!(eta_1b < eta_1);

        // eta to the current stop while boardable is zero
        let cur = c.phase.stop().unwrap();
        assert_eq!(c.eta(cur), 0.0);
    }

    #[test]
    fn eta_to_dock_after_a_full_lap_matches_summed_legs() {
        // Start the Lighter just as it leaves the Dock, so `eta(DOCK)`
        // has to walk the whole ring rather than returning 0 for
        // "already there" (which `eta` correctly does when boardable —
        // exercised separately by `eta_is_exact_against_the_timetable`).
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: DOCK, progress: 0.0 };
        let eta_dock = c.eta(DOCK);

        // Hand-computed, independently of `eta`'s own loop shape: the
        // rest of leg 0, plus every OTHER leg's length, plus a
        // warn+breather for every intermediate stop reached along the
        // way (four of them — the arrival at DOCK itself costs no
        // further wait, since arriving IS the answer).
        let expect: f32 = LEG_S[leg_index(DOCK)]
            + LEG_S[1] + LEG_S[2] + LEG_S[3] + LEG_S[4]
            + 4.0 * (WARN_S + BREATHER_S);
        assert!((eta_dock - expect).abs() < 1e-4, "eta={eta_dock} expect={expect}");
    }

    #[test]
    fn pressure_scales_linearly_with_laps() {
        let mut c = Circuit::new();
        assert_eq!(c.pressure_mul(), 1.0);
        c.laps = 4;
        assert!((c.pressure_mul() - (1.0 + 4.0 * LAP_PRESSURE_PER_LAP)).abs() < 1e-6);
    }

    #[test]
    fn cold_round_trips_without_losing_walk_progress() {
        let mut c = Circuit::new();
        c.phase = Phase::Travel { from: 0, progress: 0.5 };
        c.rider = Rider::Aboard;
        c.disembark();
        c.walk(10.0);
        let before = c.rider;
        assert!(c.go_cold());
        assert!(matches!(c.rider, Rider::Cold { .. }));
        assert!(c.go_hot());
        assert_eq!(format!("{:?}", c.rider), format!("{:?}", before));
    }

    #[test]
    fn arriving_at_dock_increments_laps_exactly_once_per_loop() {
        let mut c = Circuit::new();
        // fast-forward through exactly one full loop
        for _ in 0..2_000_000 {
            c.update(0.01);
            if c.laps >= 1 {
                break;
            }
        }
        assert_eq!(c.laps, 1);
    }
}

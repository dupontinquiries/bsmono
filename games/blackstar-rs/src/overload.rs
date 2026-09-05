//! OVERLOAD — the mode's own name, and the flow that gets you into it.
//!
//! This is the INNER flow: it starts once the top-level `flow.rs` has
//! already decided the player is entering OVERLOAD specifically (from
//! the character hub, or wherever else a future path reaches it) —
//! `flow.rs` owns Boot, Loading, the first-time intro, and the hub;
//! this module owns nothing before `RegionSelect`. Everything from the
//! hologram projector to the first shot fired is one ordered `Stage`,
//! ticked by the shell exactly like `circuit::Circuit` and
//! `escalation::Waves` are: `update(dt)` advances time-based stages,
//! explicit calls (`confirm_region`, `enter_facility`, ...) advance
//! choice-based ones, and `drain_events()` hands the shell what just
//! happened. No stage can be skipped by calling the wrong method at the
//! wrong time — every advance method checks the current stage first and
//! is a no-op (not a panic) otherwise, same contract as
//! `circuit::Circuit::board`.
//!
//! **What each stage actually is, and what already exists for it:**
//!
//! * `RegionSelect` — the **hologram projector**: browsing the five
//!   `moons::stops()` as a projected ring. `holomap.rs`'s `Holo`/`Skin`/
//!   `Glyph` machinery is the right renderer for this (a plot table
//!   that draws bodies on rings and a sweep) but its `CHART` is the
//!   real solar system, machine-checked against `patrol::ROUTE`
//!   (`every_route_world_is_on_the_chart`) — wiring the moon ring into
//!   THAT table would either fake five AU-scale orbits for bodies that
//!   don't have any, or weaken a test that exists specifically to keep
//!   the chart honest. So `RegionSelect` here is its own small
//!   projector state (which moon is highlighted, which skin) that a
//!   render layer can point at `Holo`'s draw primitives without
//!   touching its data.
//! * `Loading` — the **planet view**, ported in spirit from the C
//!   engine's planetary lighting (`sun.rs` is that actual port) and
//!   `planetlook::look()`'s derived palette for the target moon. A
//!   fixed-duration beat; `progress()` is what a loading-screen shader
//!   reads.
//! * `Drop` — hands off to `drop::Drop` for the real descent; this
//!   stage only tracks whether the fall has resolved, not the fall
//!   itself (no physics duplicated here).
//! * `Bridge` — the floating rubble bridge from the LZ to the facility.
//!   Reuses `moons::Bridge`'s length for the stop the player is
//!   entering FROM in `circuit.rs` proper, but the approach bridge here
//!   is short and one-way (LZ → facility door), tracked as its own
//!   `progress` rather than reusing `circuit::Rider` (which is for the
//!   ring's own bridges, mid-run — a different lifecycle).
//! * `FacilityEntry` — the door: `airlock::Door::passable` is the real
//!   gate once wired to a real `Door`; this stage is the beat before
//!   and after that check.
//! * `Gameplay` — a fresh `circuit_match::CircuitMatch` is constructed
//!   the moment `enter_facility` succeeds and lives in `Overload::run`
//!   from then on; `overload.rs` still owns the *stage transition* but
//!   the ring's own timetable, boarding, economy, relay, and spawned
//!   waves are entirely `CircuitMatch`'s (which itself wraps
//!   `circuit::Circuit`) from here — this module never reaches into
//!   `run`'s fields itself, only creates and holds it.

use crate::moons;

/// (guess) — a loading screen shorter than a real load would need
/// costs nothing here and is easy to tune from one place.
pub const LOADING_S: f32 = 4.0;
/// (guess) — the LZ-to-facility bridge, short by design: the design
/// doc's floating bridges belong to the ring's own crossings
/// (`moons::Bridge`), not to every mission's opening walk.
pub const ENTRY_BRIDGE_M: f32 = 45.0;

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Stage {
    /// `target` is always a valid `moons::stops()` index — there is no
    /// "nothing selected" state, matching the projector always
    /// highlighting SOMETHING as soon as it's open.
    RegionSelect { target: usize, holo_skin: usize },
    Loading { target: usize, t: f32 },
    Drop { target: usize, landed: bool },
    Bridge { target: usize, progress_m: f32 },
    FacilityEntry { target: usize },
    Gameplay { target: usize },
}

#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    RegionLocked(usize),
    LoadingDone(usize),
    Landed(usize),
    BridgeCrossed(usize),
    FacilityEntered(usize),
    GameplayStarted(usize),
}

pub struct Overload {
    pub stage: Stage,
    /// the live ring run, once `Gameplay` is reached. `None` at every
    /// earlier stage — constructed fresh exactly once, in
    /// `enter_facility`, so a caller can never read a stale run from a
    /// previous attempt at the same facility. Holds the full
    /// `CircuitMatch` (timetable + economy + spawned waves), not the
    /// bare `circuit::Circuit` — accessible at `run.circuit` for the
    /// timetable/boarding/rider API `circuit::Circuit` already exposes.
    pub run: Option<crate::circuit_match::CircuitMatch>,
    events: Vec<Event>,
}

impl Overload {
    /// Opens straight on the projector, highlighting the first
    /// non-Dock stop — `flow.rs` is what decided the player should be
    /// here at all.
    pub fn new() -> Overload {
        let first = Self::first_mission_target();
        Overload {
            stage: Stage::RegionSelect { target: first, holo_skin: 0 },
            run: None,
            events: Vec::new(),
        }
    }

    pub fn drain_events(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.events)
    }

    fn first_mission_target() -> usize {
        (crate::circuit::DOCK + 1) % crate::circuit::STOP_COUNT
    }

    /// Cycle the highlighted moon on the projector. Wraps, and always
    /// skips the Dock — you board the Lighter at the Dock, you don't
    /// select it as a destination.
    pub fn region_cycle(&mut self, d: i32) {
        if let Stage::RegionSelect { target, holo_skin } = self.stage {
            let n = crate::circuit::STOP_COUNT as i32;
            let mut next = (target as i32 + d).rem_euclid(n);
            if next as usize == crate::circuit::DOCK {
                next = (next + d.signum().max(1)).rem_euclid(n);
            }
            self.stage = Stage::RegionSelect { target: next as usize, holo_skin };
        }
    }

    /// The projector's own control, independent of the selected moon —
    /// same one-control-both-directions shape as `holomap::Holo::cycle_skin`.
    pub fn region_cycle_skin(&mut self, d: i32) {
        if let Stage::RegionSelect { target, holo_skin } = self.stage {
            self.stage =
                Stage::RegionSelect { target, holo_skin: (holo_skin as i32 + d).rem_euclid(3) as usize };
        }
    }

    /// Lock the highlighted moon and start the loading beat.
    pub fn confirm_region(&mut self) {
        if let Stage::RegionSelect { target, .. } = self.stage {
            self.events.push(Event::RegionLocked(target));
            self.stage = Stage::Loading { target, t: 0.0 };
        }
    }

    // Cancel back out of OVERLOAD entirely goes through `flow.rs`, not
    // this module: dropping this `Overload` and letting the top-level
    // flow pick the next stage (the character hub) is the whole
    // mechanism, so there is no `cancel_region` method here.

    /// `Drop{landed:true} -> Bridge`. Split from `update` because
    /// landing is reported by the drop system's own resolution
    /// (`drop::Landing`), not by elapsed time.
    pub fn report_landed(&mut self) {
        if let Stage::Drop { target, .. } = self.stage {
            self.events.push(Event::Landed(target));
            self.stage = Stage::Drop { target, landed: true };
        }
    }

    /// Step onto the bridge once the drop has resolved. No-op if the
    /// drop hasn't reported landed yet — the facility is not reachable
    /// mid-fall.
    pub fn begin_bridge(&mut self) {
        if let Stage::Drop { target, landed: true } = self.stage {
            self.stage = Stage::Bridge { target, progress_m: 0.0 };
        }
    }

    /// Walk the entry bridge. Clamped, not auto-advancing — same shape
    /// as `circuit::Circuit::walk`.
    pub fn cross_bridge(&mut self, meters: f32) {
        if let Stage::Bridge { target, progress_m } = self.stage {
            let p = (progress_m + meters).min(ENTRY_BRIDGE_M);
            if p >= ENTRY_BRIDGE_M && progress_m < ENTRY_BRIDGE_M {
                self.events.push(Event::BridgeCrossed(target));
                self.stage = Stage::FacilityEntry { target };
            } else {
                self.stage = Stage::Bridge { target, progress_m: p };
            }
        }
    }

    /// The threshold. No-op unless the bridge has actually been
    /// crossed (`FacilityEntry` is only reached that way). Spawns a
    /// fresh `circuit_match::CircuitMatch` — the ring run starts here,
    /// not at `Stage::Gameplay`'s construction, so `self.run` and
    /// `self.stage` always change together and can never disagree
    /// about whether a run exists. Seeded from the target moon's own
    /// index, so the same destination always starts the same run
    /// (`docs`'s own "deterministic seeds" requirement) — not from
    /// anything time-based, which would make two launches into the
    /// same facility diverge for no reason a player could see.
    pub fn enter_facility(&mut self) {
        if let Stage::FacilityEntry { target } = self.stage {
            self.events.push(Event::FacilityEntered(target));
            self.run = Some(crate::circuit_match::CircuitMatch::new(target as u64));
            self.stage = Stage::Gameplay { target };
            self.events.push(Event::GameplayStarted(target));
        }
    }

    /// Time-based advance: only `Loading` moves on its own; every
    /// other stage waits on an explicit call, same split `circuit.rs`
    /// makes between `update` (timetable) and `walk` (rider).
    pub fn update(&mut self, dt: f32) {
        if let Stage::Loading { target, t } = self.stage {
            let t2 = t + dt;
            if t2 >= LOADING_S {
                self.events.push(Event::LoadingDone(target));
                self.stage = Stage::Drop { target, landed: false };
            } else {
                self.stage = Stage::Loading { target, t: t2 };
            }
        }
    }

    /// `0..1` loading-screen progress, for a shader or a bar. `1.0`
    /// outside `Loading` rather than panicking — a caller that reads
    /// this a frame late during the transition gets a sane number.
    pub fn loading_progress(&self) -> f32 {
        match self.stage {
            Stage::Loading { t, .. } => (t / LOADING_S).clamp(0.0, 1.0),
            _ => 1.0,
        }
    }

    /// The moon currently targeted — every stage has one now that
    /// `RegionSelect` is the starting stage (`flow.rs` owns whatever
    /// comes before that has no target yet).
    pub fn target(&self) -> usize {
        match self.stage {
            Stage::RegionSelect { target, .. }
            | Stage::Loading { target, .. }
            | Stage::Drop { target, .. }
            | Stage::Bridge { target, .. }
            | Stage::FacilityEntry { target }
            | Stage::Gameplay { target } => target,
        }
    }

    pub fn target_spec(&self) -> moons::MoonSpec {
        moons::stops()[self.target()]
    }
}

impl Default for Overload {
    fn default() -> Overload {
        Overload::new()
    }
}

/// `--overload`: walks the flow once RegionSelect is reached, printing
/// every stage and the moon it lands on, so the sequence can be read
/// without a screen — same convention as `--circuit`. The Boot/hub
/// portion before this is `flow.rs`'s own driver's job now.
pub fn run() {
    let mut o = Overload::new();
    println!("OVERLOAD — region select to first area\n");
    println!("{:?}  -> {}", o.stage, o.target_spec().name);

    for _ in 0..2 {
        o.region_cycle(1);
    }
    println!("{:?}  -> {}", o.stage, o.target_spec().name);

    o.confirm_region();
    println!("{:?}", o.stage);
    while !matches!(o.stage, Stage::Drop { .. }) {
        o.update(0.5);
    }
    println!("{:?}", o.stage);

    o.report_landed();
    o.begin_bridge();
    println!("{:?}", o.stage);

    o.cross_bridge(ENTRY_BRIDGE_M);
    println!("{:?}", o.stage);

    o.enter_facility();
    println!("{:?}", o.stage);
    println!("\nevents: {:?}", o.drain_events());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn starts_on_region_select_never_highlighting_the_dock() {
        let o = Overload::new();
        assert!(matches!(o.stage, Stage::RegionSelect { .. }));
        assert_ne!(o.target(), crate::circuit::DOCK);
    }

    #[test]
    fn region_select_never_highlights_the_dock() {
        let mut o = Overload::new();
        for _ in 0..(crate::circuit::STOP_COUNT * 3) {
            o.region_cycle(1);
            assert_ne!(o.target(), crate::circuit::DOCK);
        }
        for _ in 0..(crate::circuit::STOP_COUNT * 3) {
            o.region_cycle(-1);
            assert_ne!(o.target(), crate::circuit::DOCK);
        }
    }

    #[test]
    fn confirm_region_locks_the_target_through_every_later_stage() {
        let mut o = Overload::new();
        o.region_cycle(2);
        let picked = o.target();
        o.confirm_region();
        assert_eq!(o.target(), picked);
        for _ in 0..1000 {
            o.update(0.1);
        }
        assert_eq!(o.target(), picked, "loading must not change the target");
        o.report_landed();
        o.begin_bridge();
        assert_eq!(o.target(), picked);
        o.cross_bridge(ENTRY_BRIDGE_M);
        assert_eq!(o.target(), picked);
        o.enter_facility();
        assert_eq!(o.target(), picked, "must survive into Gameplay");
    }

    #[test]
    fn loading_progress_is_monotonic_and_clamped() {
        let mut o = Overload::new();
        o.confirm_region();
        let mut last = -1.0f32;
        for _ in 0..100 {
            o.update(LOADING_S / 50.0);
            let p = o.loading_progress();
            assert!(p >= last, "progress went backwards: {last} -> {p}");
            assert!((0.0..=1.0).contains(&p));
            last = p;
        }
        assert_eq!(last, 1.0);
    }

    #[test]
    fn loading_fires_the_done_event_exactly_once() {
        let mut o = Overload::new();
        o.confirm_region();
        let target = o.target();
        let mut done_count = 0;
        for _ in 0..200 {
            o.update(LOADING_S / 20.0);
            for e in o.drain_events() {
                if e == Event::LoadingDone(target) {
                    done_count += 1;
                }
            }
        }
        assert_eq!(done_count, 1);
    }

    #[test]
    fn bridge_cannot_be_entered_before_landing_is_reported() {
        let mut o = Overload::new();
        o.confirm_region();
        while !matches!(o.stage, Stage::Drop { .. }) {
            o.update(1.0);
        }
        o.begin_bridge(); // must be a no-op: not landed yet
        assert!(matches!(o.stage, Stage::Drop { landed: false, .. }));
        o.report_landed();
        o.begin_bridge();
        assert!(matches!(o.stage, Stage::Bridge { .. }));
    }

    #[test]
    fn crossing_the_bridge_is_clamped_and_fires_once_at_the_far_end() {
        let mut o = Overload::new();
        o.confirm_region();
        while !matches!(o.stage, Stage::Drop { .. }) {
            o.update(1.0);
        }
        o.report_landed();
        o.begin_bridge();

        o.cross_bridge(ENTRY_BRIDGE_M * 10.0); // overshoot
        assert!(matches!(o.stage, Stage::FacilityEntry { .. }), "overshoot must still land exactly at the door");
        let events = o.drain_events();
        let crossed = events.iter().filter(|e| matches!(e, Event::BridgeCrossed(_))).count();
        assert_eq!(crossed, 1);
    }

    #[test]
    fn facility_entry_requires_a_crossed_bridge() {
        let mut o = Overload::new();
        o.enter_facility(); // way too early
        assert!(matches!(o.stage, Stage::RegionSelect { .. }), "must not skip ahead");
    }

    #[test]
    fn full_run_reaches_gameplay_with_the_right_events_in_order() {
        let mut o = Overload::new();
        o.region_cycle(3);
        let target = o.target();
        o.confirm_region();
        while !matches!(o.stage, Stage::Drop { .. }) {
            o.update(0.25);
        }
        o.report_landed();
        o.begin_bridge();
        o.cross_bridge(ENTRY_BRIDGE_M);
        o.enter_facility();
        assert_eq!(o.stage, Stage::Gameplay { target });

        // re-check the whole event log in one pass from a fresh run,
        // since `drain_events` was called mid-flow above and would
        // otherwise hide ordering bugs between drains
        let mut o2 = Overload::new();
        o2.region_cycle(3);
        let target2 = o2.target();
        o2.confirm_region();
        while !matches!(o2.stage, Stage::Drop { .. }) {
            o2.update(0.25);
        }
        o2.report_landed();
        o2.begin_bridge();
        o2.cross_bridge(ENTRY_BRIDGE_M);
        o2.enter_facility();
        let events = o2.drain_events();
        assert_eq!(
            events,
            vec![
                Event::RegionLocked(target2),
                Event::LoadingDone(target2),
                Event::Landed(target2),
                Event::BridgeCrossed(target2),
                Event::FacilityEntered(target2),
                Event::GameplayStarted(target2),
            ]
        );
    }

    #[test]
    fn target_spec_matches_the_selected_moon() {
        let mut o = Overload::new();
        o.region_cycle(2);
        let idx = o.target();
        let expect_name = moons::stops()[idx].name;
        assert_eq!(o.target_spec().name, expect_name);
    }

    #[test]
    fn run_is_none_until_gameplay_and_fresh_once_it_exists() {
        let mut o = Overload::new();
        assert!(o.run.is_none());
        o.confirm_region();
        assert!(o.run.is_none());
        while !matches!(o.stage, Stage::Drop { .. }) {
            o.update(0.5);
        }
        assert!(o.run.is_none());
        o.report_landed();
        o.begin_bridge();
        assert!(o.run.is_none(), "must not exist before the bridge is crossed");
        o.cross_bridge(ENTRY_BRIDGE_M);
        assert!(o.run.is_none(), "must not exist before the door");
        o.enter_facility();
        let run = o.run.as_ref().expect("Gameplay must carry a live run");
        assert_eq!(run.circuit.laps, 0);
        assert_eq!(run.circuit.phase, crate::circuit::Phase::Dwell {
            stop: crate::circuit::DOCK,
            remaining: crate::circuit::WARN_S,
        });
    }

    #[test]
    fn calling_enter_facility_again_in_gameplay_does_not_replace_the_run() {
        let mut o = Overload::new();
        o.confirm_region();
        while !matches!(o.stage, Stage::Drop { .. }) {
            o.update(0.5);
        }
        o.report_landed();
        o.begin_bridge();
        o.cross_bridge(ENTRY_BRIDGE_M);
        o.enter_facility();
        // advance the run a little so a replacement would be detectable
        o.run.as_mut().unwrap().update(5.0);
        let progressed_phase = o.run.as_ref().unwrap().circuit.phase;
        o.enter_facility(); // already in Gameplay: must be a no-op
        assert_eq!(o.run.as_ref().unwrap().circuit.phase, progressed_phase);
    }
}

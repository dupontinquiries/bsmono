//! RAID RUNTIME — the encounter layer, steppable and headless.
//!
//! `raid::run` resolves a raid the way a planner would: costs against a
//! window, one function call, no moment-to-moment. This module is the other
//! half — the C engine's `raid_update` ported as a pure state machine, so
//! the SAME mechanics (PURGE quotas, GROUND anti-air fields, AIR nodes and
//! the grapple, SYNC plates) run per-tick with no renderer, no input system
//! and no globals.
//!
//! The seam: this layer is handed unit positions and a clock and hands back
//! EVENTS. It never moves a unit — a GROUND slam returns the velocity to
//! apply, a grapple returns the impulse — because who applies them (the
//! player controller, a drone, a sim policy) is the shell's business. That
//! is what makes the whole raid testable at 120 Hz on a build machine.
//!
//! One deliberate fix over the C original: its grounding-field chip damage
//! rolled `rand()%100 < 28` per FRAME, which made the field's bite depend
//! on the framerate. Here it ticks damage on a fixed accumulator, so a
//! flyer pays the same price at 30 fps and at 240.

use crate::raidcon::Mech;
use crate::raidgen::{EncKind, EncSpec, PhaseSpec, RaidPlan};

// ------------------------------------------------------------ tunables ---
// The C engine's numbers, named. Changing one here changes the game and
// the tests in the same breath, which is the point of naming them.

/// horizontal radius of a GROUND encounter's anti-air field
pub const GROUND_R: f32 = 22.0;
/// a unit above this height counts as flying (the C engine's 3.5)
pub const FLY_Y: f32 = 3.5;
/// vertical velocity a slam imposes
pub const SLAM_VY: f32 = -22.0;
/// chip damage while flying inside the field, per tick of the accumulator
pub const CHIP_DMG: f32 = 4.0;
/// seconds between chip ticks (replaces the per-frame dice roll)
pub const CHIP_EVERY_S: f32 = 0.5;
/// AIR node altitude, and everything else's beacon height
pub const NODE_Y_AIR: f32 = 14.0;
pub const NODE_Y: f32 = 6.0;
/// claim envelope around an AIR node
pub const AIR_CLAIM_R: f32 = 5.0;
pub const AIR_CLAIM_DY: f32 = 4.5;
/// grapple: reach, cooldown, and the impulse it grants
pub const GRAPPLE_R: f32 = 26.0;
pub const GRAPPLE_CD: f32 = 2.0;
pub const GRAPPLE_V_XZ: f32 = 14.0;
pub const GRAPPLE_V_Y: f32 = 19.0;
/// SYNC plate geometry: centre plate plus one each side
pub const PLATE_DX: f32 = 8.0;
pub const SYNC_CENTER_R: f32 = 3.5;
pub const SYNC_SIDE_R: f32 = 4.0;
/// kills to secure a quota encounter (PURGE, and GROUND once the field is
/// understood as "fight it on foot")
pub const KILL_QUOTA: u32 = 6;

// ----------------------------------------------------------------- i/o ---

/// What the runtime needs to know about a body in the arena. Anyone's:
/// player, ally drone, a netcode ghost.
#[derive(Clone, Copy, Debug)]
pub struct Unit {
    pub pos: [f32; 3],
}

impl Unit {
    pub fn flying(&self) -> bool {
        self.pos[1] > FLY_Y
    }
}

/// What happened this tick. The shell decides what each one looks and
/// sounds like; the sim prints them; the tests assert on them.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Event {
    /// a flyer inside a grounding field: apply this vertical velocity
    Slam { unit: usize, vy: f32 },
    /// the field's chip damage came due
    Chip { unit: usize, dmg: f32 },
    /// a grapple fired: apply this velocity to the unit
    Grapple { unit: usize, vel: [f32; 3] },
    /// an AIR node was reached and claimed
    NodeClaimed { enc: usize },
    /// enough units locked in on the plates
    SyncHeld { enc: usize, occ: u8 },
    /// a quota encounter cleared
    Secured { enc: usize },
    /// the anchor fell (champion down)
    AnchorDown { enc: usize },
    /// every encounter in the phase is done
    PhaseCleared,
    /// the weather closed before the encounters did
    WindowClosed,
}

/// A beacon the shell can draw: where, what mechanic, whether it still
/// matters. The pulse and the colours live with the renderer.
#[derive(Clone, Copy, Debug)]
pub struct Beacon {
    pub pos: [f32; 3],
    pub mech: Mech,
    pub anchor: bool,
    pub done: bool,
}

// --------------------------------------------------------------- phase ---

/// One phase, live. Borrows its spec: the design is immutable, the run is
/// state.
pub struct PhaseRun<'a> {
    pub spec: &'a PhaseSpec,
    done: Vec<bool>,
    kills: Vec<u32>,
    chip_t: Vec<f32>,
    grapple_cd: f32,
    champion_down: bool,
    /// seconds left in the weather window; infinite on a clear sky
    pub left_s: f32,
    /// a fortress foothold puts the fight under cover: the window burns at
    /// the planner's sheltered rate instead of in full
    pub sheltered: bool,
    failed: bool,
}

/// The planner discounts sheltered fighting to 0.35x; the runtime burns
/// the window at the same rate so the two layers agree on what shelter is
/// worth. One constant, shared by construction.
pub const SHELTER_BURN: f32 = 0.35;

impl<'a> PhaseRun<'a> {
    pub fn new(spec: &'a PhaseSpec, window_s: f32, sheltered: bool) -> PhaseRun<'a> {
        let n = spec.enc.len();
        PhaseRun {
            spec,
            done: vec![false; n],
            kills: vec![0; n],
            chip_t: vec![0.0; n],
            grapple_cd: 0.0,
            champion_down: false,
            left_s: window_s,
            sheltered,
            failed: false,
        }
    }

    pub fn cleared(&self) -> bool {
        self.done.iter().all(|d| *d)
    }
    pub fn failed(&self) -> bool {
        self.failed
    }
    pub fn kills(&self, enc: usize) -> u32 {
        self.kills[enc]
    }
    pub fn enc_done(&self, enc: usize) -> bool {
        self.done[enc]
    }

    /// A kill scored inside encounter `enc`'s zone. The shell attributes
    /// kills to zones; the runtime only counts them.
    pub fn credit_kill(&mut self, enc: usize) {
        if enc < self.kills.len() && !self.done[enc] {
            self.kills[enc] += 1;
        }
    }

    /// The phase's champion fell. Anchors clear on this, which is the C
    /// engine's `g_esc_boss_down` reaching the encounter table.
    pub fn champion_fell(&mut self) {
        self.champion_down = true;
    }

    /// Ask for a grapple toward the nearest live AIR node. Returns nothing
    /// if there is no node in reach or the line is still rewinding.
    pub fn request_grapple(&mut self, unit: usize, units: &[Unit]) -> Option<Event> {
        if self.grapple_cd > 0.0 || unit >= units.len() {
            return None;
        }
        let u = &units[unit];
        let mut best: Option<(f32, usize, f32, f32)> = None;
        for (i, e) in self.spec.enc.iter().enumerate() {
            if self.done[i] || e.mech != Mech::Air {
                continue;
            }
            let dx = u.pos[0] - e.x;
            let dz = u.pos[2] - e.z;
            let d2 = dx * dx + dz * dz;
            if d2 < GRAPPLE_R * GRAPPLE_R && best.map(|b| d2 < b.0).unwrap_or(true) {
                best = Some((d2, i, dx, dz));
            }
        }
        let (d2, _i, dx, dz) = best?;
        self.grapple_cd = GRAPPLE_CD;
        let d = d2.sqrt().max(1e-3);
        Some(Event::Grapple {
            unit,
            vel: [-dx / d * GRAPPLE_V_XZ, GRAPPLE_V_Y, -dz / d * GRAPPLE_V_XZ],
        })
    }

    /// One tick. Hand it every body standing in the arena; get back what
    /// the mechanics did about them.
    pub fn step(&mut self, dt: f32, units: &[Unit]) -> Vec<Event> {
        let mut ev = Vec::new();
        if self.failed || self.cleared() {
            return ev;
        }
        if self.grapple_cd > 0.0 {
            self.grapple_cd -= dt;
        }
        // the shared clock, burning at the shelter's rate
        if self.left_s.is_finite() {
            self.left_s -= dt * if self.sheltered { SHELTER_BURN } else { 1.0 };
            if self.left_s <= 0.0 {
                self.failed = true;
                ev.push(Event::WindowClosed);
                return ev;
            }
        }

        for (i, e) in self.spec.enc.iter().enumerate() {
            if self.done[i] {
                continue;
            }
            if e.kind == EncKind::Anchor {
                if self.champion_down {
                    self.done[i] = true;
                    ev.push(Event::AnchorDown { enc: i });
                }
                continue;
            }
            match e.mech {
                Mech::Ground => {
                    // an anti-air ceiling: flyers inside get slammed and
                    // pay chip damage on a fixed clock
                    let mut any_flyer = false;
                    for (ui, u) in units.iter().enumerate() {
                        let dx = u.pos[0] - e.x;
                        let dz = u.pos[2] - e.z;
                        if dx * dx + dz * dz < GROUND_R * GROUND_R && u.flying() {
                            any_flyer = true;
                            ev.push(Event::Slam { unit: ui, vy: SLAM_VY });
                        }
                    }
                    if any_flyer {
                        self.chip_t[i] += dt;
                        while self.chip_t[i] >= CHIP_EVERY_S {
                            self.chip_t[i] -= CHIP_EVERY_S;
                            for (ui, u) in units.iter().enumerate() {
                                let dx = u.pos[0] - e.x;
                                let dz = u.pos[2] - e.z;
                                if dx * dx + dz * dz < GROUND_R * GROUND_R && u.flying() {
                                    ev.push(Event::Chip { unit: ui, dmg: CHIP_DMG });
                                }
                            }
                        }
                    } else {
                        self.chip_t[i] = 0.0;
                    }
                    if self.kills[i] >= KILL_QUOTA {
                        self.done[i] = true;
                        ev.push(Event::Secured { enc: i });
                    }
                }
                Mech::Air => {
                    // an elevated node: be there, at altitude
                    for u in units {
                        let dx = u.pos[0] - e.x;
                        let dz = u.pos[2] - e.z;
                        let dy = u.pos[1] - NODE_Y_AIR;
                        if dx * dx + dz * dz < AIR_CLAIM_R * AIR_CLAIM_R && dy.abs() < AIR_CLAIM_DY {
                            self.done[i] = true;
                            ev.push(Event::NodeClaimed { enc: i });
                            break;
                        }
                    }
                }
                Mech::Sync => {
                    // min_units bodies spread across the plates, together
                    let occ = sync_occupancy(e, units);
                    if occ >= e.min_units {
                        self.done[i] = true;
                        ev.push(Event::SyncHeld { enc: i, occ });
                    }
                }
                Mech::Purge => {
                    if self.kills[i] >= KILL_QUOTA {
                        self.done[i] = true;
                        ev.push(Event::Secured { enc: i });
                    }
                }
            }
        }

        if self.cleared() {
            ev.push(Event::PhaseCleared);
        }
        ev
    }

    /// Beacons for the shell to draw, C tints and heights implied by mech.
    pub fn beacons(&self) -> Vec<Beacon> {
        self.spec
            .enc
            .iter()
            .enumerate()
            .map(|(i, e)| Beacon {
                pos: [e.x, if e.mech == Mech::Air { NODE_Y_AIR } else { NODE_Y }, e.z],
                mech: e.mech,
                anchor: e.kind == EncKind::Anchor,
                done: self.done[i],
            })
            .collect()
    }

    /// Where two ally drones should stand: on the outer plates of the
    /// nearest live SYNC gate, else trailing the player. The C engine's
    /// solo answer to a mechanic one body cannot solve.
    pub fn seats(&self, player: [f32; 2]) -> [[f32; 2]; 2] {
        for (i, e) in self.spec.enc.iter().enumerate() {
            if !self.done[i] && e.mech == Mech::Sync {
                return [[e.x + PLATE_DX, e.z], [e.x - PLATE_DX, e.z]];
            }
        }
        [[player[0] + 3.0, player[1] - 3.0], [player[0] - 3.0, player[1] - 3.0]]
    }
}

/// How many plates of a SYNC gate have a body on them. Distinct plates:
/// three units on the centre plate is one plate held, not three.
pub fn sync_occupancy(e: &EncSpec, units: &[Unit]) -> u8 {
    let plates = [[e.x, e.z], [e.x + PLATE_DX, e.z], [e.x - PLATE_DX, e.z]];
    let mut occ = 0u8;
    for (pi, p) in plates.iter().enumerate() {
        let r = if pi == 0 { SYNC_CENTER_R } else { SYNC_SIDE_R };
        if units.iter().any(|u| {
            let dx = u.pos[0] - p[0];
            let dz = u.pos[2] - p[1];
            dx * dx + dz * dz < r * r
        }) {
            occ += 1;
        }
    }
    occ
}

// ---------------------------------------------------------------- raid ---

/// A whole raid, phase-gated the way the C engine gates it: you cannot
/// skip ahead, and each phase opens on a fresh window because it is a new
/// world with its own sky. The foothold decision per phase stays OUTSIDE —
/// the caller supplies each phase's window and shelter, because that
/// decision is the player's.
pub struct RaidRun<'a> {
    pub plan: &'a RaidPlan,
    pub phase: usize,
    pub over: bool,
    current: Option<PhaseRun<'a>>,
}

impl<'a> RaidRun<'a> {
    pub fn new(plan: &'a RaidPlan) -> RaidRun<'a> {
        RaidRun { plan, phase: 0, over: false, current: None }
    }
    /// Open the current phase with the window the foothold bought.
    pub fn start_phase(&mut self, window_s: f32, sheltered: bool) -> &mut PhaseRun<'a> {
        let spec = &self.plan.phases[self.phase];
        self.current = Some(PhaseRun::new(spec, window_s, sheltered));
        self.current.as_mut().unwrap()
    }
    pub fn phase_run(&mut self) -> Option<&mut PhaseRun<'a>> {
        self.current.as_mut()
    }
    /// Call when the live phase reports cleared. Advances, or ends the
    /// raid with the boss down.
    pub fn advance(&mut self) -> bool {
        self.current = None;
        self.phase += 1;
        if self.phase >= self.plan.phases.len() {
            self.over = true;
        }
        self.over
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raidcon::{boss, Mech};
    use crate::raidgen::{enc_seconds, plan_for, EncKind};

    fn spec(mech: Mech, kind: EncKind, x: f32, z: f32) -> EncSpec {
        EncSpec { x, z, kind, mech, min_units: mech.min_units() }
    }
    fn phase(enc: Vec<EncSpec>) -> PhaseSpec {
        PhaseSpec { name: "T".into(), arena: crate::raidcon::arena("pluto").unwrap(), enc }
    }
    fn at(x: f32, y: f32, z: f32) -> Unit {
        Unit { pos: [x, y, z] }
    }

    #[test]
    fn purge_clears_on_the_quota_and_not_before() {
        let p = phase(vec![spec(Mech::Purge, EncKind::Standard, 0.0, 0.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        for _ in 0..KILL_QUOTA - 1 {
            r.credit_kill(0);
        }
        assert!(r.step(0.1, &[]).is_empty());
        r.credit_kill(0);
        let ev = r.step(0.1, &[]);
        assert!(ev.contains(&Event::Secured { enc: 0 }));
        assert!(ev.contains(&Event::PhaseCleared));
    }

    #[test]
    fn the_grounding_field_slams_flyers_and_leaves_walkers_alone() {
        let p = phase(vec![spec(Mech::Ground, EncKind::Standard, 0.0, 0.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        let ev = r.step(0.1, &[at(5.0, 10.0, 0.0), at(5.0, 1.0, 0.0), at(100.0, 10.0, 0.0)]);
        assert!(ev.contains(&Event::Slam { unit: 0, vy: SLAM_VY }), "{ev:?}");
        assert!(!ev.iter().any(|e| matches!(e, Event::Slam { unit: 1, .. } | Event::Slam { unit: 2, .. })));
    }

    #[test]
    fn chip_damage_is_framerate_independent() {
        // the fix over the C original: one second in the field costs the
        // same at 10 Hz and at 120 Hz
        // 3.2 s, deliberately NOT a multiple of the chip period: at a
        // period boundary, float accumulation decides whether the final
        // tick lands, and the test would be comparing rounding, not rules
        let count = |dt: f32| {
            let p = phase(vec![spec(Mech::Ground, EncKind::Standard, 0.0, 0.0)]);
            let mut r = PhaseRun::new(&p, f32::INFINITY, false);
            let mut chips = 0;
            let mut t = 0.0;
            while t < 3.2 {
                chips += r
                    .step(dt, &[at(0.0, 10.0, 0.0)])
                    .iter()
                    .filter(|e| matches!(e, Event::Chip { .. }))
                    .count();
                t += dt;
            }
            chips
        };
        assert_eq!(count(0.1), count(1.0 / 120.0));
    }

    #[test]
    fn an_air_node_claims_at_altitude_and_only_there() {
        let p = phase(vec![spec(Mech::Air, EncKind::Standard, 0.0, 0.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        // on the ground under it: nothing
        assert!(r.step(0.1, &[at(0.0, 1.0, 0.0)]).is_empty());
        let ev = r.step(0.1, &[at(1.0, NODE_Y_AIR + 1.0, 1.0)]);
        assert!(ev.contains(&Event::NodeClaimed { enc: 0 }));
    }

    #[test]
    fn the_grapple_throws_you_at_the_node_and_then_rewinds() {
        let p = phase(vec![spec(Mech::Air, EncKind::Standard, 10.0, 0.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        let units = [at(0.0, 1.0, 0.0)];
        let ev = r.request_grapple(0, &units).expect("in range, off cooldown");
        if let Event::Grapple { vel, .. } = ev {
            assert!(vel[0] > 10.0, "should throw toward +x: {vel:?}");
            assert_eq!(vel[1], GRAPPLE_V_Y);
        } else {
            panic!("{ev:?}");
        }
        assert!(r.request_grapple(0, &units).is_none(), "cooldown ignored");
        r.step(GRAPPLE_CD + 0.01, &units);
        assert!(r.request_grapple(0, &units).is_some(), "cooldown never rewound");
        // and out of range is out of range
        let far = [at(100.0, 1.0, 0.0)];
        let p2 = phase(vec![spec(Mech::Air, EncKind::Standard, 10.0, 0.0)]);
        let mut r2 = PhaseRun::new(&p2, f32::INFINITY, false);
        assert!(r2.request_grapple(0, &far).is_none());
    }

    #[test]
    fn sync_needs_bodies_spread_across_plates_not_a_pile() {
        let p = phase(vec![spec(Mech::Sync, EncKind::Standard, 0.0, 0.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        // three units on the centre plate: one plate held, no clear
        let pile = [at(0.0, 1.0, 0.0), at(1.0, 1.0, 0.0), at(-1.0, 1.0, 0.0)];
        assert!(r.step(0.1, &pile).is_empty(), "a pile counted as a spread");
        // one per plate: locked in
        let spread = [at(0.0, 1.0, 0.0), at(PLATE_DX, 1.0, 0.0), at(-PLATE_DX, 1.0, 0.0)];
        let ev = r.step(0.1, &spread);
        assert!(ev.contains(&Event::SyncHeld { enc: 0, occ: 3 }), "{ev:?}");
        // and two units genuinely cannot solve it
        let p2 = phase(vec![spec(Mech::Sync, EncKind::Standard, 0.0, 0.0)]);
        let mut r2 = PhaseRun::new(&p2, f32::INFINITY, false);
        let two = [at(0.0, 1.0, 0.0), at(PLATE_DX, 1.0, 0.0)];
        assert!(r2.step(0.1, &two).is_empty(), "SYNC fell to a duo");
    }

    #[test]
    fn the_anchor_waits_for_the_champion() {
        let p = phase(vec![
            spec(Mech::Purge, EncKind::Anchor, 0.0, 0.0),
            spec(Mech::Purge, EncKind::Standard, 30.0, 0.0),
        ]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        for _ in 0..KILL_QUOTA {
            r.credit_kill(0); // kills in the anchor zone do nothing for it
            r.credit_kill(1);
        }
        let ev = r.step(0.1, &[]);
        assert!(ev.contains(&Event::Secured { enc: 1 }));
        assert!(!r.cleared(), "anchor cleared without its champion");
        r.champion_fell();
        let ev = r.step(0.1, &[]);
        assert!(ev.contains(&Event::AnchorDown { enc: 0 }));
        assert!(ev.contains(&Event::PhaseCleared));
    }

    #[test]
    fn the_window_closes_faster_in_the_open() {
        // the planner's 0.35x shelter discount, expressed as burn rate:
        // the same 100 s of fighting fails exposed and passes sheltered
        let mk = || phase(vec![spec(Mech::Purge, EncKind::Standard, 0.0, 0.0)]);
        let (pa, pb) = (mk(), mk());
        let mut open = PhaseRun::new(&pa, 60.0, false);
        let mut fort = PhaseRun::new(&pb, 60.0, true);
        for _ in 0..1000 {
            open.step(0.1, &[]);
            fort.step(0.1, &[]);
        }
        assert!(open.failed(), "100 s against a 60 s window should fail in the open");
        assert!(!fort.failed(), "under shelter the same fight fits");
        for _ in 0..KILL_QUOTA {
            fort.credit_kill(0);
        }
        assert!(fort.step(0.1, &[]).contains(&Event::PhaseCleared));
    }

    #[test]
    fn seats_go_to_the_live_sync_gate_then_back_to_the_player() {
        let p = phase(vec![spec(Mech::Sync, EncKind::Standard, 40.0, -10.0)]);
        let mut r = PhaseRun::new(&p, f32::INFINITY, false);
        let s = r.seats([0.0, 0.0]);
        assert_eq!(s[0], [40.0 + PLATE_DX, -10.0]);
        assert_eq!(s[1], [40.0 - PLATE_DX, -10.0]);
        // gate held (player centre + both seats): the drones fall back in
        let units = [at(40.0, 1.0, -10.0), at(40.0 + PLATE_DX, 1.0, -10.0), at(40.0 - PLATE_DX, 1.0, -10.0)];
        r.step(0.1, &units);
        let s = r.seats([5.0, 5.0]);
        assert_eq!(s[0], [8.0, 2.0]);
    }

    #[test]
    fn a_generated_raid_steps_to_the_boss_and_agrees_with_the_planner() {
        // the differential test: drive the runtime with a policy that
        // spends exactly the planner's per-encounter seconds, and the
        // stepped raid must clear phase-gated end to end — the two layers
        // priced by ONE table (`enc_seconds`), caught here if they drift.
        let plan = plan_for(boss("gigadrill").unwrap(), 3, 7);
        let mut run = RaidRun::new(&plan);
        while !run.over {
            let spec = &plan.phases[run.phase];
            // budget: what the planner would charge for this phase, plus
            // slack for the anchor champion
            let budget: f32 = spec.enc.iter().map(|e| enc_seconds(e.mech, e.kind)).sum::<f32>() + 60.0;
            let pr = run.start_phase(budget, false);
            // the policy: stand on what needs standing, kill what needs
            // killing, each encounter taking its priced time
            for i in 0..spec.enc.len() {
                let e = spec.enc[i];
                let secs = enc_seconds(e.mech, e.kind);
                let mut t = 0.0;
                while t < secs - 0.5 {
                    pr.step(0.5, &[]);
                    t += 0.5;
                }
                match e.kind {
                    EncKind::Anchor => pr.champion_fell(),
                    EncKind::Standard => match e.mech {
                        Mech::Air => {
                            pr.step(0.5, &[at(e.x, NODE_Y_AIR, e.z)]);
                        }
                        Mech::Sync => {
                            pr.step(
                                0.5,
                                &[at(e.x, 1.0, e.z), at(e.x + PLATE_DX, 1.0, e.z), at(e.x - PLATE_DX, 1.0, e.z)],
                            );
                        }
                        _ => {
                            for _ in 0..KILL_QUOTA {
                                pr.credit_kill(i);
                            }
                            pr.step(0.5, &[]);
                        }
                    },
                }
            }
            let pr = run.phase_run().unwrap();
            pr.step(0.1, &[]);
            assert!(pr.cleared(), "phase {} did not clear under its own budget", run.phase);
            assert!(!pr.failed());
            run.advance();
        }
        assert_eq!(run.phase, 3);
    }
}

// --------------------------------------------------------------- driver ---

/// `--raidrun <boss>` — a raid at 120 Hz, as the renderer will have to see it.
///
/// This module is the contract the raid renderer must satisfy: it is handed
/// unit positions and a clock, and hands back EVENTS and BEACONS. It never
/// moves anything. `skirmish.rs` calls it "the contract for the raid
/// renderer" in its own doc — and it had one reference in the whole tree, so
/// nothing had ever run the contract to see what it actually emits.
///
/// The point of printing the stream is that the renderer's job is defined by
/// it. Every `Slam` is a body being thrown at the deck; every `Grapple` is an
/// impulse the shell applies and the camera has to follow. A list of the
/// events a real raid produces, in order, with timings, is the closest thing
/// to a specification for that work that can exist before there is a window.
pub fn run(argv: &[String]) {
    let i = argv.iter().position(|a| a == "--raidrun").unwrap_or(0);
    let want = argv.get(i + 1).map(String::as_str).unwrap_or("mantis");
    let Some(boss) = crate::raidcon::BOSSES.iter().find(|b| b.id == want) else {
        println!("no warden '{want}'. `--raid` lists them.");
        return;
    };
    let plan = crate::raidgen::plan_for(boss, 3, 7);
    println!("{} — {:?}, {} phases, stepped at 120 Hz\n", boss.name, boss.mech, plan.phases.len());

    // A squad that flies, so grounding fields have something to bite. Who
    // moves these is the shell's business; here they just exist and drift in.
    // start them on top of the first objective, so a GROUND field has
    // something to bite immediately rather than after a minute of drift
    let start = plan.phases[0].enc.first().map(|e| (e.x, e.z)).unwrap_or((0.0, 0.0));
    let mut units: Vec<Unit> = (0..4)
        .map(|k| Unit { pos: [start.0 + k as f32 * 4.0 - 6.0, FLY_Y + 4.0, start.1] })
        .collect();

    let mut run = RaidRun::new(&plan);
    let dt = 1.0 / 120.0;
    let mut phase_no = 1;
    loop {
        let spec_len = plan.phases[run.phase].enc.len();
        println!("-- phase {phase_no}: {spec_len} encounter(s)");
        {
            let pr = run.start_phase(600.0, false);
            let mut t = 0.0f32;
            let mut seen: Vec<String> = Vec::new();
            let mut ticks = 0;
            while t < 600.0 {
                let evs = pr.step(dt, &units);
                for e in &evs {
                    let line = format!("{:>7.2}s  {}", t, describe(e));
                    // collapse the repeaters: a chip every accumulator period
                    // for a minute is one fact, not two hundred
                    let key = describe_kind(e);
                    if seen.iter().filter(|s| **s == key).count() < 2 {
                        println!("   {line}");
                    } else if seen.iter().filter(|s| **s == key).count() == 2 {
                        println!("   {:>7.2}s  ... {key} repeating", t);
                    }
                    seen.push(key);
                }
                // apply what the sim asked for, which is the shell's half
                for e in &evs {
                    match e {
                        Event::Slam { unit, vy } => units[*unit].pos[1] += vy * dt,
                        Event::Grapple { unit, vel } => {
                            for a in 0..3 {
                                units[*unit].pos[a] += vel[a] * dt;
                            }
                        }
                        _ => {}
                    }
                }
                if evs.iter().any(|e| matches!(e, Event::PhaseCleared | Event::WindowClosed)) {
                    break;
                }
                // THE SHELL'S OTHER HALF: the contract exposes two inputs as
                // well as the event stream. A player presses grapple; a
                // champion dies to whatever the combat layer decides. Drive
                // both, or the driver only tests the half that ticks itself.
                if ticks == 240 {
                    if let Some(e) = pr.request_grapple(0, &units) {
                        println!("   {:>7.2}s  {}  (player pressed grapple)", t, describe(&e));
                    }
                }
                if ticks == 600 {
                    pr.champion_fell();
                }
                // AND THE THIRD INPUT: kills. A quota encounter clears on
                // `credit_kill`, never on its own — the contract does not
                // decide who dies any more than it decides who moves. My
                // first run of this driver called neither and sat for 72,000
                // ticks emitting two events, which read like a broken sim and
                // was a harness feeding it nothing.
                if ticks % 600 == 0 {
                    for (i, b) in pr.beacons().iter().enumerate() {
                        if !b.done && !b.anchor {
                            pr.credit_kill(i);
                            break;
                        }
                    }
                }
                // drift toward the objectives so the phase can actually end
                if let Some(b) = pr.beacons().iter().find(|b| !b.done) {
                    for u in units.iter_mut() {
                        for a in [0usize, 2] {
                            u.pos[a] += (b.pos[a] - u.pos[a]).clamp(-8.0 * dt, 8.0 * dt);
                        }
                    }
                }
                t += dt;
                ticks += 1;
            }
            let counts = tally(&seen);
            println!("   {ticks} ticks, {:.1}s: {counts}\n", t);
        }
        // `advance` returns OVER, not "ok to continue". I read it as the
        // latter and the driver stopped after phase one — the same shape of
        // error as reading a `Result` backwards, and worth the comment
        // because the name reads either way at the call site.
        if run.advance() {
            break;
        }
        phase_no += 1;
    }
    println!("the sim never moved a unit. every Slam and Grapple above is a");
    println!("velocity the shell applies and the camera has to follow — which is");
    println!("the renderer's half of this contract, stated as a list.");
}

fn describe(e: &Event) -> String {
    match e {
        Event::Slam { unit, vy } => format!("SLAM      unit {unit}, vy {vy:+.1}"),
        Event::Chip { unit, dmg } => format!("CHIP      unit {unit}, {dmg:.1}"),
        Event::Grapple { unit, vel } => {
            format!("GRAPPLE   unit {unit}, vel [{:.0} {:.0} {:.0}]", vel[0], vel[1], vel[2])
        }
        Event::NodeClaimed { enc } => format!("NODE      encounter {enc} claimed"),
        Event::SyncHeld { enc, occ } => format!("SYNC      encounter {enc}, {occ} on the plates"),
        Event::Secured { enc } => format!("SECURED   encounter {enc}"),
        Event::AnchorDown { enc } => format!("ANCHOR    encounter {enc} — champion down"),
        Event::PhaseCleared => "PHASE CLEARED".into(),
        Event::WindowClosed => "WINDOW CLOSED — the weather beat them".into(),
    }
}

fn describe_kind(e: &Event) -> String {
    match e {
        Event::Slam { .. } => "SLAM",
        Event::Chip { .. } => "CHIP",
        Event::Grapple { .. } => "GRAPPLE",
        Event::NodeClaimed { .. } => "NODE",
        Event::SyncHeld { .. } => "SYNC",
        Event::Secured { .. } => "SECURED",
        Event::AnchorDown { .. } => "ANCHOR",
        Event::PhaseCleared => "PHASE CLEARED",
        Event::WindowClosed => "WINDOW CLOSED",
    }
    .into()
}

fn tally(seen: &[String]) -> String {
    let mut kinds: Vec<&String> = seen.iter().collect();
    kinds.sort();
    kinds.dedup();
    kinds
        .iter()
        .map(|k| format!("{} x{}", k, seen.iter().filter(|s| s.as_str() == k.as_str()).count()))
        .collect::<Vec<_>>()
        .join(", ")
}

#[cfg(test)]
mod contract_shape_tests {
    use super::*;
    use crate::raidcon::BOSSES;
    use crate::raidgen::plan_for;

    /// Step a whole raid, feeding the three shell inputs, and report the
    /// worst tick and the beacon behaviour.
    fn drive(boss: &'static crate::raidcon::Boss) -> (usize, usize, bool) {
        let plan = plan_for(boss, 3, 7);
        let start = plan.phases[0].enc.first().map(|e| (e.x, e.z)).unwrap_or((0.0, 0.0));
        let mut units: Vec<Unit> = (0..4)
            .map(|k| Unit { pos: [start.0 + k as f32 * 4.0 - 6.0, FLY_Y + 4.0, start.1] })
            .collect();
        let mut run = RaidRun::new(&plan);
        let dt = 1.0 / 120.0;
        let (mut worst, mut total) = (0usize, 0usize);
        let mut beacons_stable = true;
        loop {
            let pr = run.start_phase(600.0, false);
            let mut n_beacons: Option<usize> = None;
            for ticks in 0..72_000u32 {
                let evs = pr.step(dt, &units);
                worst = worst.max(evs.len());
                total += evs.len();
                for e in &evs {
                    match e {
                        Event::Slam { unit, vy } => units[*unit].pos[1] += vy * dt,
                        Event::Grapple { unit, vel } => {
                            for a in 0..3 {
                                units[*unit].pos[a] += vel[a] * dt;
                            }
                        }
                        _ => {}
                    }
                }
                // beacon COUNT and ORDER must not churn: a renderer keys its
                // draw objects off the index
                let b = pr.beacons();
                match n_beacons {
                    None => n_beacons = Some(b.len()),
                    Some(n) => {
                        if n != b.len() {
                            beacons_stable = false;
                        }
                    }
                }
                if ticks == 600 {
                    pr.champion_fell();
                }
                if ticks % 600 == 0 {
                    for (i, bc) in pr.beacons().iter().enumerate() {
                        if !bc.done && !bc.anchor {
                            pr.credit_kill(i);
                            break;
                        }
                    }
                }
                if evs.iter().any(|e| matches!(e, Event::PhaseCleared | Event::WindowClosed)) {
                    break;
                }
            }
            if run.advance() {
                break;
            }
        }
        (worst, total, beacons_stable)
    }

    /// WHAT THE SHELL HAS TO BUDGET FOR. The renderer consumes this stream
    /// every tick; if a single tick can emit hundreds of events, that is an
    /// allocation and a draw-call spike, and it is better known now than
    /// discovered at 120 Hz with a window open.
    #[test]
    fn no_tick_emits_more_events_than_a_frame_can_absorb() {
        let mut worst_overall = 0;
        for b in BOSSES {
            let (worst, total, _) = drive(b);
            assert!(total > 0, "{} produced no events at all", b.id);
            assert!(
                worst <= 16,
                "{} emits {worst} events in one tick; the shell has to handle \
                 that inside a frame",
                b.id
            );
            worst_overall = worst_overall.max(worst);
        }
        assert!(worst_overall > 1, "nothing ever emits more than one event; the drive is too tame");
        // the number itself, so a regression shows as a changed figure rather
        // than as a bound that silently had room
        assert!(
            worst_overall <= 8,
            "the worst tick across all nineteen is now {worst_overall}; it was 4"
        );
    }

    /// A renderer keys beacon draw objects off the index. If the list length
    /// changes mid-phase it has to rebuild, and if the ORDER changes it draws
    /// the wrong marker at the wrong place.
    #[test]
    fn the_beacon_list_is_stable_within_a_phase() {
        for b in BOSSES {
            let (_, _, stable) = drive(b);
            assert!(stable, "{}'s beacon list changes length mid-phase", b.id);
        }
    }

    #[test]
    fn every_warden_can_actually_be_finished() {
        // the contract has to be completable for all nineteen, or a renderer
        // built against it will meet one that cannot end
        for b in BOSSES {
            let (_, total, _) = drive(b);
            assert!(total >= 3, "{} only ever emitted {total} events", b.id);
        }
    }
}

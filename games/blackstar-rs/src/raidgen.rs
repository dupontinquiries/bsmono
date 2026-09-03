//! RAID GENERATION — a boss's fiction turned into three playable phases.
//!
//! Port of the C toolchain's generator (assetkit `raids.rs` / `gen_raids.py`)
//! into the game itself, so a raid layout is something the engine makes
//! rather than something a build step made earlier. Same rules, one
//! improvement, and every rule is a test:
//!
//! * **The boss's own mechanic appears at least once per phase** — a phase
//!   of nothing but PURGE plays identically to a phase with no idea in it.
//! * **Mechanic density rises across phases.** The Approach teaches the
//!   mechanic cheaply, the Gauntlet drills it, the finale assumes it. The C
//!   generator drew every phase from the same distribution; this one makes
//!   the documented intent real.
//! * **The finale is the boss's own arena.** This replaces the C version's
//!   random finale: The Reactor happens where the walking reactor lives.
//!   Because every SYNC boss's arena is `unending` (a roster fact raidcon
//!   tests), the old "SYNC finishes at unending" rule falls out for free.
//! * **Every arena a phase references must exist** — checked at
//!   construction, because the C header once shipped with two of three
//!   phases pointing at maps that were never defined.
//!
//! Determinism: the same seed gives the same raid, byte for byte, which is
//! what makes a raid's layout a property of the raid rather than of when
//! you happened to open it — the same promise `loot::roll` makes.

use crate::raid::{Raid, Run};
use crate::raidcon::{arena, Arena, Boss, Mech, ARENAS};
use crate::sun::Orbit;
use crate::survival::{Body, Bunker, Weather};

// ----------------------------------------------------------------- rng ---

/// The C generator's xorshift, kept bit-identical so layouts can be
/// compared across the two implementations if anyone ever wants to.
pub struct Rng(u64);

impl Rng {
    pub fn new(seed: u64) -> Rng {
        Rng(seed.wrapping_mul(6364136223846793005).wrapping_add(1))
    }
    fn next(&mut self) -> u64 {
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 7;
        self.0 ^= self.0 << 17;
        self.0
    }
    pub fn f(&mut self, lo: f32, hi: f32) -> f32 {
        lo + (self.next() % 1_000_000) as f32 / 1_000_000.0 * (hi - lo)
    }
    pub fn pick<'a, T>(&mut self, v: &'a [T]) -> &'a T {
        &v[(self.next() % v.len() as u64) as usize]
    }
}

// ---------------------------------------------------------------- spec ---

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum EncKind {
    /// the phase's anchor — in the C engine, `kind 1`, cleared by the
    /// champion falling rather than by a quota
    Anchor,
    Standard,
}

/// One encounter, as designed. Coordinates are arena-local metres in the
/// C engine's ±95 convention.
#[derive(Clone, Copy, Debug)]
pub struct EncSpec {
    pub x: f32,
    pub z: f32,
    pub kind: EncKind,
    pub mech: Mech,
    pub min_units: u8,
}

pub struct PhaseSpec {
    pub name: String,
    pub arena: &'static Arena,
    pub enc: Vec<EncSpec>,
}

pub struct RaidPlan {
    pub boss: &'static Boss,
    pub seed: u64,
    pub phases: Vec<PhaseSpec>,
}

impl RaidPlan {
    /// Does the boss's mechanic show up in this phase? The invariant the
    /// generator promises; exposed so a test can ask rather than trust.
    pub fn mech_present(&self, phase: usize) -> bool {
        self.phases[phase].enc.iter().any(|e| e.mech == self.boss.mech)
    }
    /// How many encounters in a phase run the boss's own rule.
    pub fn mech_density(&self, phase: usize) -> usize {
        self.phases[phase].enc.iter().filter(|e| e.mech == self.boss.mech).count()
    }
}

// ------------------------------------------------------------ generate ---

/// Encounters for one phase. `weight` is how many of the three draw slots
/// carry the boss's mechanic — the density gradient — and the last
/// encounter always does, which is what guarantees presence.
fn encounters(rng: &mut Rng, mech: Mech, n: usize, weight: usize) -> Vec<EncSpec> {
    let w = weight.min(2);
    (0..n)
        .map(|i| {
            let m = if i == n - 1 {
                mech
            } else {
                // a slot table like the C generator's `pick(&[0,0,mech])`,
                // but with the mech's share rising per phase
                let slots: [Mech; 3] = match w {
                    0 => [Mech::Purge, Mech::Purge, mech],
                    1 => [Mech::Purge, mech, mech],
                    _ => [mech, mech, mech],
                };
                *rng.pick(&slots)
            };
            EncSpec {
                x: (rng.f(-95.0, 95.0) * 10.0).round() / 10.0,
                z: (rng.f(-95.0, 95.0) * 10.0).round() / 10.0,
                kind: if i == 0 { EncKind::Anchor } else { EncKind::Standard },
                mech: m,
                min_units: m.min_units(),
            }
        })
        .collect()
}

/// Build one boss's raid: three phases, four encounters each, finale at
/// the boss's own arena, earlier phases on distinct arenas away from it.
pub fn plan_for(boss: &'static Boss, phases: usize, seed: u64) -> RaidPlan {
    let mut rng = Rng::new(seed ^ strhash(boss.id));
    let finale = arena(boss.arena).expect("roster test guarantees the arena exists");
    let pool: Vec<&'static Arena> = ARENAS.iter().filter(|a| a.id != finale.id).collect();
    let mut chosen: Vec<&'static Arena> = Vec::new();
    while chosen.len() + 1 < phases {
        let c = *rng.pick(&pool);
        if !chosen.iter().any(|a| a.id == c.id) {
            chosen.push(c);
        }
    }
    chosen.push(finale);
    let last = boss.title.split_whitespace().last().unwrap_or("End");
    let names = ["The Approach".to_string(), "The Gauntlet".to_string(), format!("The {last}")];
    RaidPlan {
        boss,
        seed,
        phases: chosen
            .iter()
            .enumerate()
            .map(|(i, a)| PhaseSpec {
                name: names.get(i).cloned().unwrap_or_else(|| format!("Phase {}", i + 1)),
                arena: a,
                enc: encounters(&mut rng, boss.mech, 4, i),
            })
            .collect(),
    }
}

/// The whole roster for a seed.
pub fn plan_all(seed: u64) -> Vec<RaidPlan> {
    crate::raidcon::all_bosses().iter().map(|b| plan_for(b, 3, seed)).collect()
}

/// A tiny stable string hash so two bosses at the same seed do not share a
/// layout. FNV-1a, because it is eight lines and inspectable. Public: the
/// campaign seeds its wave tables off mission ids with the same hash.
pub fn strhash(s: &str) -> u64 {
    let mut h: u64 = 0xcbf29ce484222325;
    for b in s.bytes() {
        h ^= b as u64;
        h = h.wrapping_mul(0x100000001b3);
    }
    h
}

// ------------------------------------------------- the shared economy ---

/// Seconds of fighting an encounter is worth in the planning layer. ONE
/// table, used by both the coarse planner (`raid::run`) and the stepped
/// runtime's tests, so the two layers cannot drift apart silently.
///
/// The spread is the mechanic's real cost: PURGE is the baseline, GROUND
/// costs the flyers their speed, AIR costs the walkers a route, and SYNC
/// costs everyone the freedom to spread out.
pub fn enc_seconds(mech: Mech, kind: EncKind) -> f32 {
    let base = match mech {
        Mech::Purge => 90.0,
        Mech::Ground => 110.0,
        Mech::Air => 120.0,
        Mech::Sync => 140.0,
    };
    match kind {
        // the anchor holds the champion; it is the long fight of the four
        EncKind::Anchor => base * 1.4,
        EncKind::Standard => base,
    }
}

/// One phase of a plan, as a `raid::Raid` the existing planner can run.
///
/// Each phase is its own world with its own sky, so it gets its own
/// foothold choice and its own weather window — which is exactly how the C
/// engine plays it (a phase swaps the map and the clock restarts). The
/// footholds are derived from the arena and the boss tier rather than
/// authored, the same move `loot::demanded` makes for drop tables.
pub fn phase_raid(plan: &RaidPlan, phase: usize) -> Raid {
    let p = &plan.phases[phase];
    let t = plan.boss.tier as f32;
    // the finale is defended like the boss's home; earlier phases are the
    // outworks
    let hold = if phase + 1 == plan.phases.len() { 1.0 } else { 0.6 };
    Raid {
        name: plan.boss.name,
        orbit: p.arena.orbit,
        weather: p.arena.weather,
        sites: vec![
            // open ground: quick, exposed, and the ship sits there
            crate::foothold::Site {
                approach: crate::foothold::Approach::ClearLz,
                at: Bunker { distance_m: 200.0 + 40.0 * t, shielded: false },
                garrison: 0.4 * hold,
                take_s: 60.0 + 20.0 * t,
            },
            // what the wardens built: slower, dug in, and the sky stops
            // mattering once you are through the door
            crate::foothold::Site {
                approach: crate::foothold::Approach::SeizeFortress,
                at: Bunker { distance_m: 500.0 + 80.0 * t, shielded: true },
                garrison: 1.0 * hold,
                take_s: 150.0 + 40.0 * t,
            },
        ],
        encounters: p.enc.iter().map(|e| enc_seconds(e.mech, e.kind)).collect(),
        // only the finale carries the boss itself; the outwork anchors are
        // champions, priced into the anchor seconds above
        boss_tier: if phase + 1 == plan.phases.len() { plan.boss.tier } else { 0 },
        extract_m: 800.0 + 300.0 * t,
    }
}

/// Run a whole plan through the coarse planner: phases gate, and the payout
/// is the FINALE's payout — clearing outworks is how you reach the boss,
/// not how you get paid. Returns each phase's run and the shard, if won.
pub fn run_plan(plan: &RaidPlan, body: Body, speed_ms: f32, seed: u32, team: &[Body]) -> (Vec<Run>, bool) {
    let mut out = Vec::new();
    for i in 0..plan.phases.len() {
        let r = crate::raid::run(&phase_raid(plan, i), body, speed_ms, seed.wrapping_add(i as u32), team);
        let done = r.phase >= crate::raid::Phase::Extracted;
        out.push(r);
        if !done {
            return (out, false);
        }
    }
    (out, true)
}

/// Sanity used by tests and the sim: an arena with weather should still be
/// takeable by SOMETHING in the site list. Exposed so content edits get a
/// named failure instead of a mysterious Repulsed.
pub fn takeable(orbit: &Orbit, weather: Weather, sites: &[crate::foothold::Site], body: Body, speed: f32) -> bool {
    let w = crate::raid::window_s(orbit, weather);
    crate::foothold::choose(body, sites, weather, w, speed).is_some()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::raid::Phase;
    use crate::raidcon::BOSSES;

    #[test]
    fn nineteen_plans_of_three_phases() {
        let all = plan_all(7);
        assert_eq!(all.len(), 19);
        for p in &all {
            assert_eq!(p.phases.len(), 3, "{}", p.boss.id);
            for ph in &p.phases {
                assert_eq!(ph.enc.len(), 4, "{} {}", p.boss.id, ph.name);
                assert_eq!(ph.enc[0].kind, EncKind::Anchor, "{} {} has no anchor first", p.boss.id, ph.name);
                assert_eq!(ph.enc.iter().filter(|e| e.kind == EncKind::Anchor).count(), 1);
            }
        }
    }

    #[test]
    fn the_mechanic_is_present_in_every_phase() {
        for p in plan_all(3) {
            for i in 0..p.phases.len() {
                assert!(p.mech_present(i), "{} phase {} lost its own mechanic", p.boss.id, i);
            }
        }
    }

    #[test]
    fn density_rises_across_the_raid() {
        // per-raid it can plateau (a SYNC raid may roll 2-2-3); over the
        // roster the gradient must be real or the Gauntlet teaches nothing
        let all = plan_all(11);
        let sum = |i: usize| all.iter().map(|p| p.mech_density(i)).sum::<usize>();
        let (a, g, f) = (sum(0), sum(1), sum(2));
        assert!(g > a, "gauntlet ({g}) no denser than approach ({a})");
        assert!(f >= g, "finale ({f}) thinner than the gauntlet ({g})");
    }

    #[test]
    fn the_finale_is_the_bosss_own_arena_and_sync_ends_at_unending() {
        for p in plan_all(5) {
            let last = p.phases.last().unwrap();
            assert_eq!(last.arena.id, p.boss.arena, "{}", p.boss.id);
            if p.boss.mech == Mech::Sync {
                assert_eq!(last.arena.id, "unending", "{} is SYNC and ended elsewhere", p.boss.id);
            }
            // and the approach opens AWAY from the boss's rules
            assert_ne!(p.phases[0].arena.id, p.boss.arena, "{} opens at home", p.boss.id);
            // arenas are distinct: no phase re-fights the last one's ground
            let ids: Vec<&str> = p.phases.iter().map(|x| x.arena.id).collect();
            let mut d = ids.clone();
            d.dedup();
            assert_eq!(ids.len(), d.len(), "{} repeats an arena", p.boss.id);
        }
    }

    #[test]
    fn sync_plates_want_three_and_nothing_else_does() {
        for p in plan_all(9) {
            for ph in &p.phases {
                for e in &ph.enc {
                    if e.mech == Mech::Sync {
                        assert_eq!(e.min_units, 3);
                    } else {
                        assert_eq!(e.min_units, 1);
                    }
                }
            }
        }
    }

    #[test]
    fn the_same_seed_is_the_same_raid() {
        let a = plan_for(&BOSSES[4], 3, 42);
        let b = plan_for(&BOSSES[4], 3, 42);
        for (pa, pb) in a.phases.iter().zip(&b.phases) {
            assert_eq!(pa.arena.id, pb.arena.id);
            for (ea, eb) in pa.enc.iter().zip(&pb.enc) {
                assert_eq!((ea.x, ea.z, ea.mech), (eb.x, eb.z, eb.mech));
            }
        }
        // and two bosses at one seed do NOT share a layout
        let c = plan_for(&BOSSES[5], 3, 42);
        assert!(a.phases[0].enc[0].x != c.phases[0].enc[0].x || a.phases[0].arena.id != c.phases[0].arena.id);
    }

    #[test]
    fn every_phase_of_every_raid_is_plannable() {
        // the content-level promise: whatever arena and weather a phase
        // landed on, a Cybertronian team can take SOME foothold there.
        // This is the check that catches an arena whose sky refuses every
        // site — the class of failure that otherwise shows up as a player
        // staring at REPULSED.
        for p in plan_all(1) {
            for i in 0..p.phases.len() {
                let r = phase_raid(&p, i);
                assert!(
                    takeable(&r.orbit, r.weather, &r.sites, Body::Cybertronian, 14.0),
                    "{} phase {} ({}) cannot be entered at all",
                    p.boss.id, i, p.phases[i].arena.id
                );
            }
        }
    }

    #[test]
    fn a_full_plan_runs_gated_and_pays_only_at_the_end() {
        // machines, at machine speed, on a fresh clock per phase: the
        // roster should be clearable — that is what "authored" means here
        let mut cleared = 0;
        for p in plan_all(2) {
            let (runs, won) = run_plan(&p, Body::Cybertronian, 14.0, 8, &[Body::Cybertronian; 3]);
            if won {
                cleared += 1;
                assert_eq!(runs.len(), 3);
                assert!(runs.last().unwrap().drop.is_some(), "{} won and paid nothing", p.boss.id);
            } else {
                // gating: a failed phase ends the raid there
                assert!(runs.len() <= 3);
                assert!(runs.last().unwrap().phase < Phase::Extracted);
            }
        }
        assert_eq!(cleared, 19, "an authored raid was not clearable by a full machine team");
    }

    #[test]
    fn a_suited_team_loses_the_walk_out_where_a_machine_team_does_not() {
        // where body actually bites in the planner: SPEED. The reactor's
        // finale sits on frostline, whose thermal window is 900 s — a
        // machine crosses the extraction distance inside it; a suited team
        // clears the boss and gets caught by the front on the way home.
        // The flare arenas do not produce this split (their windows are
        // hours), which is itself worth knowing: the killer is the walk.
        let p = plan_for(boss_by("cherno"), 3, 4);
        let (_, machine) = run_plan(&p, Body::Cybertronian, 14.0, 4, &[Body::Cybertronian; 3]);
        let (runs, suited) = run_plan(&p, Body::HumanSuited, 2.5, 4, &[Body::HumanSuited; 3]);
        assert!(machine, "the machine team should clear the reactor's raid");
        assert!(!suited, "a suited team outran a thermal front on foot");
        // and the failure is the LATE kind: they got in, they fought, the
        // world took the win back — not a doorstep rejection
        assert!(runs.last().unwrap().phase >= Phase::Landed);
    }

    fn boss_by(id: &str) -> &'static Boss {
        crate::raidcon::boss(id).unwrap()
    }
}

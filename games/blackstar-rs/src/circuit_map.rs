//! CIRCUIT MAP — the layout, before anything is a `Shape`.
//!
//! `docs/UNWIRING.md` has already decided the shape of this problem for
//! us: the eighteen `MapStyle` arms in `world.rs::generate_at` are
//! deletable (step 3 of its own order-of-work), and the model for NEW
//! content is Kuiper's — a purpose-built generator (`space.rs`), not
//! another arm bolted onto code that is already scheduled to go. So
//! this module does not touch `world::World` at all, not even to add a
//! nineteenth style. It computes WHERE things go — station fixtures on
//! a stop, control points along a bridge — as plain `Vec3` positions
//! and a small placement table, fully testable with no renderer, no
//! GPU, and no `Shape` type in sight.
//!
//! **What is deliberately NOT here yet**: turning a `Layout` into real
//! `world::Shape`s (the `push` closure pattern `world.rs` uses
//! internally), baking a tile, or wiring `main.rs`'s map-selection path
//! to reach any of this. That is real, GPU-adjacent work and belongs
//! in its own pass once this layer is right — same order `space.rs`
//! itself used ("measure, then wire"). Doing it now, blind, in a
//! sandbox with no display, would produce code that compiles and is
//! wrong about how the renderer wants its shapes.
//!
//! Everything below is deterministic in the stop/bridge's own seed, so
//! two runs of the same moon produce the same fixture layout — a
//! player's mental map of a stop doesn't reroll under them.

use crate::moons::{self, Bridge, MoonSpec, Section};
use crate::raidgen::Rng;
use glam::Vec3;

/// A buyable/usable thing standing on a stop. Positions only — what
/// each one DOES is `escalation.rs`/`gates.rs`/`circuit.rs`'s job, not
/// this module's.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum Fixture {
    Rack,
    Perk,
    Repair,
    Forge,
    /// index into `gates::GATE_COUNT`-shaped local gate chain, or a
    /// bridge-crossing seal — `which` disambiguates without a second enum
    Seal { which: u32 },
    Crate,
    Relay,
    Bank,
}

/// One fixture, placed. `pos` is stop-LOCAL (origin at the stop's own
/// centre); `circuit_map`'s eventual `Shape`-building pass adds the
/// stop's world-space anchor on top of this, once one exists.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Placement {
    pub fixture: Fixture,
    pub pos: Vec3,
    /// facing, radians around Y — so a rack model isn't planted facing
    /// into a wall by coincidence of the ring layout
    pub yaw: f32,
}

/// Ring-layout radius, metres — (guess), sized so fixtures don't
/// crowd a 60 m starting arena (`gates::extent` before any gate opens).
const FIXTURE_RING_R: f32 = 14.0;

/// The station list for one stop, matching the design brief's table
/// (RACK/PERK/REPAIR/FORGE/SEAL/CRATE/RELAY/BANK counts per stop) —
/// counts are read from `moons::Section` rather than hardcoded per id,
/// so a new stop with an existing section automatically gets a
/// sensible fixture set instead of silently getting none.
fn fixtures_for(section: Section, is_dock: bool) -> Vec<Fixture> {
    let mut f = Vec::new();
    if is_dock {
        f.push(Fixture::Rack);
        f.push(Fixture::Repair);
        f.push(Fixture::Seal { which: 0 });
        f.push(Fixture::Seal { which: 1 });
        f.push(Fixture::Bank);
        return f;
    }
    f.push(Fixture::Rack);
    f.push(Fixture::Perk);
    match section {
        Section::ThermalSweep => {
            f.push(Fixture::Forge);
            f.push(Fixture::Seal { which: 0 });
        }
        Section::Calving => {
            f.push(Fixture::Seal { which: 0 });
        }
        Section::WreckGrapple => {
            // the Tithe: most shops, the relay, the crate spawns here
            f.push(Fixture::Rack);
            f.push(Fixture::Perk);
            f.push(Fixture::Forge);
            f.push(Fixture::Crate);
            f.push(Fixture::Relay);
        }
        Section::TidalSwell => {
            f.push(Fixture::Seal { which: 0 });
        }
        Section::SpinDeck => unreachable!("Dock is handled above"),
    }
    f
}

/// The full placement for one stop: every fixture, positioned on a
/// ring around the stop's local centre, spaced evenly and facing
/// inward. Deterministic in the stop's own seed (via `moons::MoonSpec`
/// not carrying one directly, so the fixture ORDER is what varies —
/// derived from the stop's name, which is itself fixed — meaning the
/// layout is not just deterministic but literally constant per stop,
/// which is the right answer: nothing about a fixture's position
/// should depend on when the player visits).
pub fn layout(spec: &MoonSpec) -> Vec<Placement> {
    let is_dock = spec.id == "spindle_dock";
    let fixtures = fixtures_for(spec.section, is_dock);
    let n = fixtures.len().max(1);
    fixtures
        .into_iter()
        .enumerate()
        .map(|(i, fixture)| {
            let a = (i as f32 / n as f32) * std::f32::consts::TAU;
            let pos = Vec3::new(a.cos() * FIXTURE_RING_R, 0.0, a.sin() * FIXTURE_RING_R);
            // face inward, toward the ring's centre
            let yaw = a + std::f32::consts::PI;
            Placement { fixture, pos, yaw }
        })
        .collect()
}

/// One control point along a bridge's rubble chain, in bridge-LOCAL
/// space: `t` runs 0 (the FROM stop's end) to 1 (the TO stop's end)
/// along the bridge's own axis, `lateral`/`vertical` are the seeded
/// wander off that straight line — "rubble that should not exist"
/// reads as a spline that never quite goes straight, not as a tube.
#[derive(Clone, Copy, PartialEq, Debug)]
pub struct Waypoint {
    pub t: f32,
    pub lateral_m: f32,
    pub vertical_m: f32,
}

/// (guess) — how far the bridge is allowed to wander off a straight
/// line, and how many control points a chain of `RBox`/`Capsule`/
/// `Shell` SDFs (per the design brief) would need to read as rubble
/// rather than a girder.
const BRIDGE_WANDER_M: f32 = 6.0;
const BRIDGE_SEGMENTS: usize = 12;

/// The bridge's control-point chain, deterministic in `bridge.seed` —
/// two players crossing the same bridge see the same rubble, and the
/// same seed always regenerates the same chain (no caching needed, no
/// staleness possible).
pub fn bridge_spline(bridge: &Bridge) -> Vec<Waypoint> {
    let mut rng = Rng::new(bridge.seed);
    let mut out = Vec::with_capacity(BRIDGE_SEGMENTS + 1);
    // endpoints anchor exactly at 0 and 1 with no wander, so the bridge
    // always meets both stops cleanly regardless of how wild the
    // middle gets
    out.push(Waypoint { t: 0.0, lateral_m: 0.0, vertical_m: 0.0 });
    for i in 1..BRIDGE_SEGMENTS {
        let t = i as f32 / BRIDGE_SEGMENTS as f32;
        // taper the wander toward both ends so it still reads as
        // "landing on solid ground", not a chain that swings wildly
        // right up to the doorstep
        let taper = (t * std::f32::consts::PI).sin();
        out.push(Waypoint {
            t,
            lateral_m: rng.f(-BRIDGE_WANDER_M, BRIDGE_WANDER_M) * taper,
            vertical_m: rng.f(-BRIDGE_WANDER_M * 0.4, BRIDGE_WANDER_M * 0.4) * taper,
        });
    }
    out.push(Waypoint { t: 1.0, lateral_m: 0.0, vertical_m: 0.0 });
    out
}

/// The whole ring's layout in one call — every stop's fixtures, every
/// bridge's spline — for a `--circuit-map` driver or a future baking
/// pass to consume without recomputing the seeds by hand.
pub fn full_ring() -> (Vec<Vec<Placement>>, Vec<Vec<Waypoint>>) {
    let stops = moons::stops();
    let bridges = moons::bridges();
    let placements = stops.iter().map(layout).collect();
    let splines = bridges.iter().map(bridge_spline).collect();
    (placements, splines)
}

/// `--circuit-map`: prints every stop's fixture list and each bridge's
/// waypoint count/wander extremes, so the layout can be read and
/// argued with before any of it becomes a `Shape`.
pub fn run() {
    println!("CIRCUIT MAP — layout only, no world::World involved\n");
    for spec in moons::stops() {
        let l = layout(&spec);
        println!("{} ({:?}):", spec.name, spec.section);
        for p in &l {
            println!("  {:?} at ({:.1}, {:.1}, {:.1}) yaw {:.2}", p.fixture, p.pos.x, p.pos.y, p.pos.z, p.yaw);
        }
    }
    println!();
    for b in moons::bridges() {
        let s = bridge_spline(&b);
        let max_lateral = s.iter().map(|w| w.lateral_m.abs()).fold(0.0f32, f32::max);
        println!(
            "bridge {} -> {}: {} waypoints, {:.1} m long, max lateral wander {:.1} m",
            b.from, b.to, s.len(), b.length_m, max_lateral
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_stop_produces_a_nonempty_layout() {
        for spec in moons::stops() {
            let l = layout(&spec);
            assert!(!l.is_empty(), "{} has no fixtures", spec.name);
        }
    }

    #[test]
    fn the_dock_has_exactly_the_seal_pair_and_a_bank() {
        let dock = moons::by_id("spindle_dock").unwrap();
        let l = layout(&dock);
        let seals: Vec<_> = l.iter().filter(|p| matches!(p.fixture, Fixture::Seal { .. })).collect();
        assert_eq!(seals.len(), 2);
        assert!(l.iter().any(|p| p.fixture == Fixture::Bank));
        assert!(!l.iter().any(|p| p.fixture == Fixture::Relay), "the relay is at the Tithe, not the Dock");
    }

    #[test]
    fn the_tithe_carries_the_relay_and_the_crate_spawn() {
        let tithe = moons::by_id("tithe").unwrap();
        let l = layout(&tithe);
        assert!(l.iter().any(|p| p.fixture == Fixture::Relay));
        assert!(l.iter().any(|p| p.fixture == Fixture::Crate));
    }

    #[test]
    fn only_the_tithe_carries_the_relay() {
        for spec in moons::stops() {
            let l = layout(&spec);
            let has_relay = l.iter().any(|p| p.fixture == Fixture::Relay);
            assert_eq!(has_relay, spec.id == "tithe", "{}", spec.id);
        }
    }

    #[test]
    fn fixtures_are_spaced_evenly_on_the_ring_and_face_inward() {
        for spec in moons::stops() {
            let l = layout(&spec);
            for p in &l {
                let r = (p.pos.x * p.pos.x + p.pos.z * p.pos.z).sqrt();
                assert!((r - FIXTURE_RING_R).abs() < 1e-3, "{}: off-ring radius {r}", spec.id);
                // yaw should point back toward the centre: the vector
                // (cos(yaw), sin(yaw)) should be roughly opposite pos's
                // own bearing from the centre
                let bearing = p.pos.z.atan2(p.pos.x);
                let inward = (bearing + std::f32::consts::PI).rem_euclid(std::f32::consts::TAU);
                let facing = p.yaw.rem_euclid(std::f32::consts::TAU);
                let diff = (inward - facing).abs().min(std::f32::consts::TAU - (inward - facing).abs());
                assert!(diff < 1e-3, "{}: fixture not facing inward, diff {diff}", spec.id);
            }
        }
    }

    #[test]
    fn layout_is_deterministic_across_calls() {
        for spec in moons::stops() {
            let a = layout(&spec);
            let b = layout(&spec);
            assert_eq!(a, b);
        }
    }

    #[test]
    fn bridge_endpoints_have_zero_wander() {
        for b in moons::bridges() {
            let s = bridge_spline(&b);
            let first = s.first().unwrap();
            let last = s.last().unwrap();
            assert_eq!(first.t, 0.0);
            assert_eq!(first.lateral_m, 0.0);
            assert_eq!(first.vertical_m, 0.0);
            assert_eq!(last.t, 1.0);
            assert_eq!(last.lateral_m, 0.0);
            assert_eq!(last.vertical_m, 0.0);
        }
    }

    #[test]
    fn bridge_wander_never_exceeds_its_own_budget() {
        for b in moons::bridges() {
            for w in bridge_spline(&b) {
                assert!(w.lateral_m.abs() <= BRIDGE_WANDER_M + 1e-4);
                assert!(w.vertical_m.abs() <= BRIDGE_WANDER_M * 0.4 + 1e-4);
            }
        }
    }

    #[test]
    fn bridge_spline_is_deterministic_per_seed() {
        for b in moons::bridges() {
            let a = bridge_spline(&b);
            let c = bridge_spline(&b);
            assert_eq!(a, c);
        }
    }

    #[test]
    fn different_bridges_do_not_share_a_wander_pattern() {
        let bs = moons::bridges();
        let a = bridge_spline(&bs[0]);
        let b = bridge_spline(&bs[1]);
        assert_ne!(a, b, "two different bridges must not be identical ruins");
    }

    #[test]
    fn full_ring_covers_every_stop_and_bridge_exactly_once() {
        let (placements, splines) = full_ring();
        assert_eq!(placements.len(), crate::circuit::STOP_COUNT);
        assert_eq!(splines.len(), crate::circuit::STOP_COUNT);
    }
}

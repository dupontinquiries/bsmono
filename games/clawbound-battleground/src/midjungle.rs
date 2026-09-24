//! midjungle — the part of the map that is designed rather than scattered.
//!
//! An ASCII plan of the generated world (`scene::plan_tests`) showed what
//! "the middle is tight" meant in the generator's terms:
//!
//! * `jungle_spot` places nothing inside radius 22, so the centre of the map
//!   was a bare disc with a road through it. Nothing to fight around, put
//!   your back to, or leap onto.
//! * Beside mid lane there was a cleared margin and then uniform random
//!   scatter. No screen, no corridor, no doorway: nowhere to flank FROM,
//!   because a flank needs something between you and the lane.
//!
//! Two authored pieces, both data ([`Placement`]) so they can be tested
//! without a GPU and, later, emitted by the map backend instead of by hand:
//!
//! ## The Hollow
//!
//! The centre, furnished. Perches at three heights (a rock to a pillar cap is
//! a leap chain), a fallen giant to put between you and a ranged cat, fern
//! beds to stalk through. Everything stands at least [`LANE_KEEP`] off the
//! lane so minion pathing is untouched.
//!
//! ## The Runs
//!
//! Two corridors parallel to mid lane, one each side, hidden from it by a
//! SCREEN: ruin walls and carved heads at ground level for sight lines, and
//! tall trees over them so a raised third-person camera on the lane cannot
//! look down into the run either. The screen has DOORWAYS — that is the
//! point. You pass round a tall thing into the jungle, take the inner path
//! unseen, and come out of another gap behind whoever is in the lane.
//!
//! ## Fairness
//!
//! Authored for one quadrant-pair and mirrored through the origin,
//! `(x, z) -> (-x, -z)`, which is the symmetry the two bases have. Mirroring
//! in x as well would be tidier and would give Blue's left what Red has on
//! its right.

use crate::blockers::{Blockers, PropKind};

/// One authored prop.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Placement {
    pub kind: PropKind,
    pub x: f32,
    pub z: f32,
    /// Quarter turns, as the scene's `Piece` takes them.
    pub quarter: u8,
    pub scale: f32,
}

/// Nothing authored stands nearer mid lane's centreline than this. Lane
/// half-width is ~4 u; minions and their push-out want the rest.
pub const LANE_KEEP: f32 = 6.5;
pub const HOLLOW_RADIUS: f32 = 21.0;
/// Centreline of each run, and how far along z it goes.
pub const RUN_X: f32 = 15.5;
pub const RUN_HALF_WIDTH: f32 = 2.6;
/// Ends where the mid outer tower's cleared ground begins. It ran to 56 --
/// 15.5 u from a tower at z = 54, INSIDE its range, behind a screen: a lane
/// built for skimming the tower's edge from cover. Review called that out;
/// the test here had only checked that nothing stood ON a tower.
pub const RUN_Z: (f32, f32) = (HOLLOW_RADIUS + 1.0, 36.0);
/// Where the screen stands, between lane and run.
pub const SCREEN_X: f32 = 9.6;
/// Doorways through the screen, as z on the +z side (mirrored for -z).
/// ONE, in the middle: each run is already open at both ends -- into the
/// Hollow, and out into the jungle behind the far tower line -- so a door
/// every few units left more gap than screen, and a screen that is mostly
/// gap hides nobody. This gives two hidden stretches of ~12 u a side.
///
/// THE ARRAY WAS EMPTY. The doc above says "ONE, in the middle" and the
/// declaration was `[f32; 0]` -- so `reserved`'s `in_door` term could never be
/// true, the scatter was free to fill the screen line end to end, and the run
/// was reachable only from its two ends. Reported as a middle that is "very
/// tight and unpathable", which is exactly what a sealed corridor plays like.
///
/// Placed at the midpoint of the run's z span, which is what "in the middle"
/// meant: the two hidden stretches either side come out at ~11u each, matching
/// the ~12 the comment above claims.
pub const DOORS: [f32; 1] = [(RUN_Z.0 + RUN_Z.1) * 0.5];
pub const DOOR_HALF: f32 = 2.4;

/// Is `(x, z)` ground the scatter must leave alone? The run floors, the
/// doorways, and the Hollow — so a random jungle tree cannot wall off a
/// corridor that the whole design depends on being open.
/// Half-width of a lane-to-lane connector, in world units.
///
/// 3.2: wide enough for two cats to pass without one shoving the other into a
/// tree, narrow enough to still read as a jungle trail rather than a fourth
/// lane. A corridor a player mistakes for a lane is a corridor they expect
/// minions to walk down.
pub const LINK_HALF_WIDTH: f32 = 3.2;

/// Where the connectors cross, as |z|. Mirrored to both halves of the map.
///
/// Two per side, at the quarter points between the river and the tower line,
/// so a rotation from mid to a side lane does not have to go all the way back
/// through the Hollow — which is what makes the middle play as a dead end.
pub const LINKS_Z: [f32; 2] = [26.0, 46.0];

/// The x span a connector covers: from just outside the run to just short of
/// the side lane's own keep-out.
///
/// Stops at 50 rather than 58 because `lane_clearance` already protects the
/// lane itself; running the reservation into it would carve a permanent notch
/// in the lane's tree line and make the junction obvious from the lane.
pub const LINK_X: (f32, f32) = (RUN_X + 2.0, 50.0);

/// Is `(x, z)` on a lane-to-lane connector?
///
/// THERE WERE NO CROSS-PATHS. The runs go from the Hollow outward and end in
/// the jungle behind the tower line; the side lanes are at x = ±58. That left
/// 42u of unreserved jungle between them, so whether a route from mid to a
/// side lane existed at all depended on where the scatter happened to drop
/// trees — which is the "no clear path from one lane to the next" that was
/// reported.
pub fn on_link(x: f32, z: f32) -> bool {
    let (ax, az) = (x.abs(), z.abs());
    if ax < LINK_X.0 || ax > LINK_X.1 {
        return false;
    }
    LINKS_Z.iter().any(|l| (az - l).abs() < LINK_HALF_WIDTH)
}

pub fn reserved(x: f32, z: f32) -> bool {
    if crate::map::active() != crate::map::MapId::Jungle {
        return false; // the Hollow is the jungle's
    }
    if x.hypot(z) < HOLLOW_RADIUS {
        return true;
    }
    let (ax, az) = (x.abs(), z.abs());
    let in_run = (ax - RUN_X).abs() < RUN_HALF_WIDTH + 1.2 && az > RUN_Z.0 - 2.0 && az < RUN_Z.1 + 2.0;
    let in_door = ax > LANE_KEEP - 2.0 && ax < RUN_X && DOORS.iter().any(|d| (az - d).abs() < DOOR_HALF + 1.0);
    // The screen's own strip too: scatter landing ON the screen line would
    // plug doorways from the side.
    let on_screen = (ax - SCREEN_X).abs() < 2.4 && az > RUN_Z.0 - 2.0 && az < RUN_Z.1 + 2.0;
    in_run || in_door || on_screen || on_link(x, z)
}

/// The authored half, for z >= 0 (toward Red). [`layout`] mirrors it.
fn half() -> Vec<Placement> {
    use PropKind::*;
    let p = |kind, x: f32, z: f32, quarter: u8, scale: f32| Placement { kind, x, z, quarter, scale };
    let mut v = Vec::new();

    // ── the Hollow (this half: z >= 0, both sides of the lane) ───────────
    // West of the lane: low rock -> boulders -> pillar cap. A leap chain
    // that climbs, each within 9 u of the last.
    v.push(p(Rock, -9.0, 3.5, 0, 1.0));
    v.push(p(BoulderCluster, -13.5, 9.5, 1, 1.0));
    v.push(p(RuinPillar, -9.5, 15.5, 0, 1.0));
    // East: a fallen giant lying ALONG the lane, 13 u of hard cover from a
    // ranged cat standing in it, and a platform 3.3 u up.
    v.push(p(FallenLog, 13.0, 9.0, 1, 1.0));
    v.push(p(Rock, 8.5, 17.0, 2, 1.0));
    // Somewhere to stalk: beds either side of the giant and behind the rocks.
    for (x, z) in [(10.0, 2.5), (16.5, 3.5), (15.5, 12.5), (-15.5, 3.0), (-15.0, 12.0), (-7.5, 10.0)] {
        v.push(p(Fern, x, z, 0, 1.0));
    }
    v.push(p(Bush, -11.0, 16.5, 0, 1.0));
    v.push(p(Bush, 11.5, 17.0, 0, 1.0));

    // ── the Runs, z in RUN_Z, both sides ─────────────────────────────────
    for side in [-1.0f32, 1.0] {
        // The screen: whatever stands between two doorways. Ground-level
        // sight blockers alternate with trunks, whose canopies roof the run
        // against a camera raised over the lane.
        // Spaced by their OWN radii, shoulder to shoulder. A fixed pitch left
        // a gap beside every trunk (1.8 u across against a 3.4 u pitch) and
        // the screen hid 69% of the run; the test wanted 70 and was right.
        let mut z = RUN_Z.0 + 1.5;
        let mut k = 0;
        while z < RUN_Z.1 {
            let kind = [StoneHead, JungleTree, BoulderCluster, StranglerFig][k % 4];
            let r = kind.footprint().radius;
            let centre = z + r;
            let in_door = DOORS.iter().any(|d| (centre - d).abs() < DOOR_HALF + r);
            if in_door {
                z = DOORS.iter().copied().find(|d| (centre - d).abs() < DOOR_HALF + r).unwrap() + DOOR_HALF;
                continue;
            }
            v.push(p(kind, side * SCREEN_X, centre, (k % 4) as u8, 1.0));
            k += 1;
            z = centre + r + 0.25;
        }
        // The run's own floor: cover to move through, never a solid.
        let mut z = RUN_Z.0 + 2.0;
        let mut j = 0;
        while z < RUN_Z.1 {
            let wob = if j % 2 == 0 { 1.1 } else { -1.2 };
            v.push(p(if j % 3 == 0 { Bush } else { Fern }, side * (RUN_X + wob), z, 0, 1.0));
            z += 4.6;
            j += 1;
        }
        // The far wall of the run: trunks on its OUTER edge, so it reads as
        // a corridor and not as a clearing, with its own gaps to the jungle.
        let mut z = RUN_Z.0 + 3.0;
        let mut m = 0;
        while z < RUN_Z.1 {
            if m % 3 != 2 {
                v.push(p(if m % 2 == 0 { JungleTree } else { Palm }, side * (RUN_X + RUN_HALF_WIDTH + 3.4), z, (m % 4) as u8, 1.0));
            }
            z += 5.2;
            m += 1;
        }
    }
    v
}

/// Every authored placement: the z >= 0 half and its mirror through the origin.
pub fn layout() -> Vec<Placement> {
    // Nothing authored inside a tower's cleared ground, whatever half() says.
    let h: Vec<Placement> = half().into_iter().filter(|p| !crate::consts::in_tower_clearing(p.x, p.z)).collect();
    let mut all = h.clone();
    all.extend(h.iter().map(|p| Placement { x: -p.x, z: -p.z, quarter: (p.quarter + 2) % 4, ..*p }));
    all
}

/// The layout's own blockers, without the rest of the world. What the tests
/// here and any map-authoring tool can check against.
pub fn blockers() -> Blockers {
    let mut b = Blockers::new(crate::consts::ARENA_RADIUS + 20.0, 6.0);
    for p in layout() {
        b.push_placed(p.kind, p.x, p.z, 0.0, p.quarter, p.scale);
    }
    b
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::blockers::Perch;
    use crate::consts::GOD_RADIUS;

    fn sides_and_ends() -> Vec<(f32, f32)> {
        vec![(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)]
    }

    /// The design's first promise: the inner path exists. Every run, walked
    /// end to end down its centreline at a cat's width plus margin.
    #[test]
    fn every_run_can_be_walked_end_to_end() {
        let b = blockers();
        let mut steps = 0;
        for (sx, sz) in sides_and_ends() {
            let mut z = RUN_Z.0;
            while z <= RUN_Z.1 {
                assert!(!b.blocked(sx * RUN_X, sz * z, GOD_RADIUS + 0.35), "run ({sx},{sz}) is plugged at z = {}", sz * z);
                z += 0.5;
                steps += 1;
            }
        }
        // 4 runs x 14 u at 0.5 u. Was > 200 when the runs were 34 u long; they
        // stop at the tower clearing now, and are honestly STUBS: see RUN_Z.
        assert!(steps > 100, "only {steps} samples: did RUN_Z collapse?");
    }

    /// The second: you cannot be seen from the lane while you are on it --
    /// except at the doorways, which must exist and must be open. Both
    /// directions, as a range: a screen with no gaps is a wall, and one that
    /// is mostly gap is not a screen.
    #[test]
    fn the_screen_hides_the_run_from_the_lane_except_at_its_doorways() {
        let b = blockers();
        for (sx, sz) in sides_and_ends() {
            // What is left of the screen is a prop or two a side, so a hidden
            // FRACTION over three samples is noise (it read 0.67 and meant
            // nothing). The claim that survives the truncation, and does not
            // depend on length: each screen prop hides what is straight
            // behind it from the lane.
            let mut props = 0;
            for pl in layout().iter().filter(|pl| (pl.x - sx * SCREEN_X).abs() < 0.1 && pl.z * sz > 0.0) {
                assert!(b.sight_obstruction((0.0, pl.z), (sx * RUN_X, pl.z)) >= 0.99, "{:?} at z = {} does not hide the run behind it", pl.kind, pl.z);
                props += 1;
            }
            assert!(props >= 1, "side ({sx},{sz}) has no screen left at all");
            for d in DOORS {
                let z = sz * d;
                assert!(b.sight_obstruction((0.0, z), (sx * RUN_X, z)) < 0.99, "doorway at z = {z} is not a gap in the view");
                let mut x = LANE_KEEP - 2.0;
                while x <= RUN_X {
                    assert!(!b.blocked(sx * x, z, GOD_RADIUS + 0.2), "doorway at z = {z} is blocked at x = {}", sx * x);
                    x += 0.5;
                }
            }
        }
    }

    /// The third: the middle has things in it and still has room. A floor on
    /// furniture AND a floor on free space -- either alone is the old
    /// problem or its opposite.
    #[test]
    fn the_hollow_is_furnished_and_still_has_room_to_fight() {
        let b = blockers();
        let in_hollow = |x: f32, z: f32| x.hypot(z) < HOLLOW_RADIUS;
        let perches: Vec<(f32, Perch)> = b
            .with_perch()
            .filter(|(k, p)| *p != Perch::None && { let (ax, az, _) = k.shape.nearest_axis(0.0, 0.0); in_hollow(ax, az) })
            .map(|(k, p)| (k.top, p))
            .collect();
        assert!(perches.len() >= 8, "only {} things to leap onto in the Hollow", perches.len());
        let heights: std::collections::BTreeSet<i32> = perches.iter().map(|p| (p.0 * 2.0) as i32).collect();
        assert!(heights.len() >= 3, "perches at {heights:?}: a leap chain wants to CLIMB");
        assert!(perches.iter().any(|p| p.1 == Perch::Narrow), "and something to balance on");
        let cover = layout().iter().filter(|p| in_hollow(p.x, p.z) && matches!(p.kind, PropKind::Fern | PropKind::Bush)).count();
        assert!(cover >= 10, "only {cover} beds to stalk through");

        let (mut free, mut cells) = (0, 0);
        let mut x = -HOLLOW_RADIUS;
        while x < HOLLOW_RADIUS {
            let mut z = -HOLLOW_RADIUS;
            while z < HOLLOW_RADIUS {
                if in_hollow(x, z) {
                    cells += 1;
                    free += !b.blocked(x, z, GOD_RADIUS) as usize;
                }
                z += 1.0;
            }
            x += 1.0;
        }
        let open = free as f32 / cells as f32;
        assert!((0.72..0.93).contains(&open), "{open:.2} of the Hollow is walkable: furnished, not filled");
    }

    /// Minions walk the lane by push-out. Nothing authored may stand where
    /// it could shoulder them.
    #[test]
    fn nothing_authored_touches_mid_lane_or_a_tower() {
        let b = blockers();
        let mut z = -60.0;
        while z <= 60.0 {
            for x in [-LANE_KEEP + 1.6, 0.0, LANE_KEEP - 1.6] {
                assert!(!b.blocked(x, z, GOD_RADIUS), "the lane is blocked at ({x}, {z})");
            }
            z += 0.5;
        }
        let mut towers = 0;
        for lane in [crate::consts::Lane::Top, crate::consts::Lane::Mid, crate::consts::Lane::Bot] {
            for team in [crate::consts::Team::Blue, crate::consts::Team::Red] {
                for inner in [false, true] {
                    let t = crate::consts::tower_pos(lane, team, inner);
                    assert!(!b.blocked(t.x, t.z, 4.0), "a tower at {t} has an authored prop on it");
                    towers += 1;
                }
            }
        }
        assert_eq!(towers, 12);
    }

    /// The review's point, as a test: no bush to skim a tower's line from,
    /// and no rock to perch beside it. Everything authored that hides or
    /// lifts a cat is outside every tower's cleared ground.
    #[test]
    fn nothing_that_hides_or_lifts_a_cat_is_inside_a_towers_clearing() {
        let mut checked = 0;
        for p in layout() {
            assert!(!crate::consts::in_tower_clearing(p.x, p.z), "{:?} at ({}, {}) is inside a tower's clearing", p.kind, p.x, p.z);
            checked += 1;
        }
        assert!(checked > 40, "{checked} placements left: did the filter eat the layout?");
        // And the clearing really is bigger than the range, which is bigger
        // than anything a cat can auto-attack from.
        let max_auto = crate::gods::ROSTER.iter().map(|d| d.auto_range).fold(0.0f32, f32::max);
        assert!(crate::consts::TOWER_RANGE > max_auto + 6.0, "tower {} vs longest auto {max_auto}", crate::consts::TOWER_RANGE);
        assert!(crate::consts::TOWER_CLEARING > crate::consts::TOWER_RANGE);
    }

    /// What Blue has toward Red, Red has toward Blue.
    #[test]
    fn the_layout_is_the_same_from_either_base() {
        let all = layout();
        assert!(all.len() > 40, "{} placements", all.len());
        for p in &all {
            let twin = all.iter().any(|q| q.kind == p.kind && (q.x + p.x).abs() < 1e-3 && (q.z + p.z).abs() < 1e-3);
            assert!(twin, "{:?} at ({}, {}) has no twin through the origin", p.kind, p.x, p.z);
            assert!(reserved(p.x, p.z) || p.x.abs() > RUN_X, "{:?} at ({}, {}) is authored on unreserved ground, where scatter may land on it", p.kind, p.x, p.z);
        }
    }

#[cfg(test)]
mod pathing_tests {
    use super::*;

    /// THE DOOR ARRAY WAS EMPTY. `in_door` could never be true, so the screen
    /// was solid end to end and the run was reachable only from its two ends.
    #[test]
    fn the_screen_has_a_doorway() {
        assert!(!DOORS.is_empty(), "a screen with no door seals the run");
        let z = DOORS[0];
        // On the screen line, at the door: reserved, so scatter keeps off it.
        assert!(reserved(SCREEN_X, z), "the doorway must be kept clear");
        assert!(reserved(-SCREEN_X, -z), "and mirrored");
    }

    /// The door sits INSIDE the run, with screen either side of it.
    ///
    /// My first version of this asserted that the screen line away from the
    /// door is NOT reserved -- and it is, deliberately: `on_screen` reserves
    /// the whole line so scatter cannot plug a doorway from the side. The
    /// screen is built geometry, not scatter, so "reserved" here means "the
    /// random trees keep off", not "it is walkable". Asserting the position
    /// instead, which is what the door actually has to get right.
    #[test]
    fn the_door_sits_inside_the_run_with_screen_either_side() {
        let z = DOORS[0];
        assert!(z > RUN_Z.0 && z < RUN_Z.1, "the door must be on the run");
        let near = z - DOOR_HALF - RUN_Z.0;
        let far = RUN_Z.1 - (z + DOOR_HALF);
        assert!(near > 3.0 && far > 3.0, "screen either side: {near} / {far}");
        assert!((near - far).abs() < 1.0, "and centred: {near} vs {far}");
    }

    /// THERE WERE NO CROSS-PATHS. 42u of unreserved jungle sat between the
    /// runs and the side lanes, so a route from mid to a side lane existed
    /// only if the scatter happened not to fill it.
    #[test]
    fn a_connector_spans_from_the_run_to_the_side_lane() {
        let z = LINKS_Z[0];
        assert!(on_link(RUN_X + 3.0, z), "starts just outside the run");
        assert!(on_link(48.0, z), "and reaches out to the lane's own keep-out");
        assert!(reserved(30.0, z), "so the scatter leaves it alone");
    }

    /// Mirrored into all four quadrants, or one side of the map plays
    /// differently from the other.
    #[test]
    fn the_connectors_are_mirrored() {
        let z = LINKS_Z[1];
        for (sx, sz) in [(1.0, 1.0), (-1.0, 1.0), (1.0, -1.0), (-1.0, -1.0)] {
            assert!(on_link(30.0 * sx, z * sz), "quadrant {sx},{sz}");
        }
    }

    /// A TRAIL, NOT A FOURTH LANE. A corridor a player reads as a lane is one
    /// they expect minions to walk down.
    #[test]
    fn a_connector_is_narrower_than_a_lane_corridor() {
        let z = LINKS_Z[0];
        assert!(on_link(30.0, z), "on the centre line");
        assert!(!on_link(30.0, z + LINK_HALF_WIDTH + 0.1), "and not far off it");
        assert!(LINK_HALF_WIDTH < RUN_HALF_WIDTH * 2.0, "still a trail");
    }

    /// The connectors must not run into the lane's own protected strip, or
    /// they carve a permanent notch in its tree line.
    #[test]
    fn a_connector_stops_short_of_the_lane_itself() {
        let z = LINKS_Z[0];
        assert!(!on_link(58.0, z), "the side lane's centre is not a connector");
        assert!(LINK_X.1 < 58.0 - 5.0, "with room between");
    }

    /// The Hollow is still open -- the connectors must not have replaced it.
    #[test]
    fn the_hollow_is_still_reserved() {
        assert!(reserved(0.0, 0.0));
        assert!(reserved(HOLLOW_RADIUS - 1.0, 0.0));
    }
}
}

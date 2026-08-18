//! junglemap.rs — the Battleground's **jungle layer**: the natural blockers that
//! break the big map's sightlines and make the ground between the trails worth
//! fighting in. Three kinds of feature, every one mirrored under a half-turn so
//! neither clan's side of the territory hides better:
//!
//! **The symmetry, and why it is this one.** Every feature is mirrored across the
//! **anti-diagonal** — the reflection (x, z) -> (-z, -x), which swaps the two
//! camps and fixes each lane onto itself. That is the same map the river course,
//! the causeway, the mountain tilt and (since it started granting sight
//! advantage) the terrain noise itself are folded on.
//!
//! It was a half-turn until it was measured. A half-turn is *also* a clan-swap
//! symmetry, so each layer looked fair alone — but the two conventions disagree
//! about the lanes (a half-turn sends your duo lane to their solo lane, a mirror
//! sends it to their duo lane), and a map built half from one and half from the
//! other is fair under neither. The symptom was a mirror match — identical
//! drafts, both sides, eight lands — coming back 8.0 deaths to 4.2. One
//! convention, chosen to match the river.
//!
//! * **Thickets** — stands of brush so dense a cat inside is *unseen* from outside
//!   until you are almost on top of it (the books' undergrowth ambush; a MOBA's
//!   brush). Placed beside prey camps, at the trail bends, and at the contention
//!   ground's edge — everywhere an ambush pays.
//! * **Stone walls** — old broken ridges of rock. Sight does not cross stone: a
//!   wall between you and a cat is a wall between you and *knowing about* that
//!   cat. Each has a **pass** carved through its middle where sight (and easy
//!   paws) go through; crossing anywhere else means scrambling slowly over.
//! * **Mistfalls** — where water tumbles off the crags, the spray hangs as mist.
//!   Concealment like a thicket, but it drifts over open ground near the water.
//!
//! This layer is deliberately **local to the battleground and pure**: functions of
//! `(x, z)` and nothing else. It never touches `world`'s global terrain, so the
//! other modes, the map grader, and every parallel test see the land they always
//! saw — and the sim stays deterministic for free. (A true heightfield adjustment
//! — carving these ridges into the rendered terrain itself — would mean a
//! per-mode terrain variant in `world`; that seam is noted in TODO, not smuggled
//! in here.)

/// Within this range you sense a hidden cat anyway — rustling, scent, breath.
pub const CLOSE_SENSE: f32 = 6.0;
/// Adjacent to a wall you hear claws on the far side even without sight.
pub const WALL_SENSE: f32 = 4.0;
/// Scrambling over a wall (outside its pass) slows a cat to this fraction.
pub const SCRAMBLE: f32 = 0.45;
/// How wide a wall's footprint is (for the scramble zone).
pub const WALL_W: f32 = 3.0;
/// The pass: this central fraction of each wall is open ground.
pub const PASS: (f32, f32) = (0.42, 0.58);

#[derive(Clone, Copy)]
pub struct Thicket { pub x: f32, pub z: f32, pub r: f32 }
#[derive(Clone, Copy)]
pub struct Wall { pub ax: f32, pub az: f32, pub bx: f32, pub bz: f32 }
#[derive(Clone, Copy)]
pub struct Mist { pub x: f32, pub z: f32, pub r: f32 }

// Base features on clan A's half; the mirror (a half-turn about the centre) is
// generated, so symmetry is by construction, not by hand-kept bookkeeping.
const THICKETS_BASE: [Thicket; 4] = [
    Thicket { x: -70.0, z: 28.0, r: 7.0 },    // brush beside the west prey camp
    Thicket { x: -20.0, z: -62.0, r: 7.0 },   // brush beside the south prey camp
    Thicket { x: -92.0, z: 72.0, r: 8.0 },    // the west trail bend's ambush stand
    Thicket { x: -18.0, z: 9.0, r: 6.5 },     // the contention ground's edge
];
const WALLS_BASE: [Wall; 2] = [
    // a broken ridge in the jungle between the mid and west trails, with its pass
    Wall { ax: -70.0, az: 10.0, bx: -10.0, bz: 70.0 },
    // a shorter spur guarding the approach jungle nearer clan A's camp —
    // laid wholly west of the mid trail's diagonal so no lane ever scrambles
    Wall { ax: -94.0, az: -34.0, bx: -70.0, bz: -58.0 },
];
const MISTS_BASE: [Mist; 1] = [
    Mist { x: -26.0, z: 26.0, r: 8.0 },       // the mistfall off the mid-river crags
];

fn mirrored<T: Copy, const N: usize>(base: [T; N], flip: impl Fn(T) -> T) -> Vec<T> {
    base.iter().copied().chain(base.iter().map(|&f| flip(f))).collect()
}

/// The clan-swap reflection: across the anti-diagonal, swapping the camps and
/// fixing every lane. `reflect(reflect(p)) == p`, and the trail bends are its
/// fixed points.
pub fn reflect(x: f32, z: f32) -> (f32, f32) { (-z, -x) }

pub fn thickets() -> Vec<Thicket> {
    mirrored(THICKETS_BASE, |t| {
        let (x, z) = reflect(t.x, t.z);
        Thicket { x, z, r: t.r }
    })
}
pub fn walls() -> Vec<Wall> {
    mirrored(WALLS_BASE, |w| {
        let (ax, az) = reflect(w.ax, w.az);
        let (bx, bz) = reflect(w.bx, w.bz);
        Wall { ax, az, bx, bz }
    })
}
pub fn mists() -> Vec<Mist> {
    mirrored(MISTS_BASE, |m| {
        let (x, z) = reflect(m.x, m.z);
        Mist { x, z, r: m.r }
    })
}

fn dist(ax: f32, az: f32, bx: f32, bz: f32) -> f32 { ((ax - bx).powi(2) + (az - bz).powi(2)).sqrt() }

/// Concealing cover at a point: the index of the thicket (0..) or mist (offset by
/// thicket count) whose canopy this point is under, if any. Two cats under the
/// *same* cover see each other; a cat outside sees neither.
pub fn concealment_at(x: f32, z: f32) -> Option<usize> {
    let th = thickets();
    for (i, t) in th.iter().enumerate() {
        if dist(x, z, t.x, t.z) < t.r { return Some(i); }
    }
    for (i, m) in mists().iter().enumerate() {
        if dist(x, z, m.x, m.z) < m.r { return Some(th.len() + i); }
    }
    None
}

/// Parameter t of the closest point on segment ab to p, clamped to [0,1].
fn seg_t(ax: f32, az: f32, bx: f32, bz: f32, px: f32, pz: f32) -> f32 {
    let (dx, dz) = (bx - ax, bz - az);
    let len2 = dx * dx + dz * dz;
    if len2 < 1e-6 { return 0.0; }
    (((px - ax) * dx + (pz - az) * dz) / len2).clamp(0.0, 1.0)
}

/// Is this point in a wall's scramble footprint (stone underfoot, outside the pass)?
pub fn on_wall(x: f32, z: f32) -> bool {
    walls().iter().any(|w| {
        let t = seg_t(w.ax, w.az, w.bx, w.bz, x, z);
        if t > PASS.0 && t < PASS.1 { return false; }   // the pass is open ground
        let (cx, cz) = (w.ax + (w.bx - w.ax) * t, w.az + (w.bz - w.az) * t);
        dist(x, z, cx, cz) < WALL_W
    })
}

/// Does segment p→q cross segment a→b? (Proper crossing; shared endpoints don't count.)
fn crosses(px: f32, pz: f32, qx: f32, qz: f32, ax: f32, az: f32, bx: f32, bz: f32) -> bool {
    let d = |ox: f32, oz: f32, ux: f32, uz: f32, vx: f32, vz: f32|
        (ux - ox) * (vz - oz) - (uz - oz) * (vx - ox);
    let d1 = d(px, pz, qx, qz, ax, az);
    let d2 = d(px, pz, qx, qz, bx, bz);
    let d3 = d(ax, az, bx, bz, px, pz);
    let d4 = d(ax, az, bx, bz, qx, qz);
    (d1 * d2 < 0.0) && (d3 * d4 < 0.0)
}

/// Does stone stand between two points? Each wall blocks along its solid spans
/// (either side of the pass) — sight through the pass itself is clear.
pub fn wall_between(px: f32, pz: f32, qx: f32, qz: f32) -> bool {
    walls().iter().any(|w| {
        let at = |t: f32| (w.ax + (w.bx - w.ax) * t, w.az + (w.bz - w.az) * t);
        let spans = [(0.0, PASS.0), (PASS.1, 1.0)];
        spans.iter().any(|&(t0, t1)| {
            let (ax, az) = at(t0);
            let (bx, bz) = at(t1);
            crosses(px, pz, qx, qz, ax, az, bx, bz)
        })
    })
}

/// The whole sight rule: can a cat at p see a cat at q through the jungle?
/// (Stealth is the sim's business; this is terrain only.)
pub fn sees(px: f32, pz: f32, qx: f32, qz: f32) -> bool {
    let d = dist(px, pz, qx, qz);
    match concealment_at(qx, qz) {
        Some(c) if concealment_at(px, pz) != Some(c) && d > CLOSE_SENSE => return false,
        _ => {}
    }
    if wall_between(px, pz, qx, qz) && d > WALL_SENSE { return false; }
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_jungle_is_mirrored_so_neither_clan_hides_better() {
        let same = |a: (f32, f32), b: (f32, f32)| (a.0 - b.0).abs() < 1e-3 && (a.1 - b.1).abs() < 1e-3;
        for t in thickets() {
            let r = reflect(t.x, t.z);
            assert!(
                thickets().iter().any(|u| same((u.x, u.z), r) && (u.r - t.r).abs() < 1e-4),
                "thicket at ({}, {}) has no mirror", t.x, t.z
            );
        }
        for w in walls() {
            let (ra, rb) = (reflect(w.ax, w.az), reflect(w.bx, w.bz));
            assert!(
                walls().iter().any(|u| (same((u.ax, u.az), ra) && same((u.bx, u.bz), rb))
                    || (same((u.ax, u.az), rb) && same((u.bx, u.bz), ra))),
                "wall has no mirror"
            );
        }
        for m in mists() {
            assert!(mists().iter().any(|u| same((u.x, u.z), reflect(m.x, m.z))));
        }
        // the reflection is an involution and fixes the trail bends
        for p in [(-85.0f32, 85.0f32), (0.0, 0.0), (85.0, -85.0)] {
            assert!(same(reflect(p.0, p.1), p), "{p:?} should be a fixed point");
        }
        assert!(same(reflect(reflect(12.0, -5.0).0, reflect(12.0, -5.0).1), (12.0, -5.0)));
        // and it swaps the camps
        assert!(same(reflect(-118.0, -118.0), (118.0, 118.0)));
    }

    #[test]
    fn a_cat_in_the_thicket_is_unseen_until_you_are_on_it() {
        // base[1]: its open eastern sightline crosses no wall — base[0]'s does,
        // which is itself the point of the jungle (cover compounds)
        let t = THICKETS_BASE[1];
        assert!(!sees(t.x + 30.0, t.z, t.x, t.z), "brush conceals from the open");
        assert!(sees(t.x + CLOSE_SENSE - 1.0, t.z, t.x, t.z), "but not from a whisker away");
        assert!(sees(t.x + 1.0, t.z, t.x - 1.0, t.z), "two cats in the same brush see each other");
        assert!(sees(t.x, t.z, t.x + 30.0, t.z), "and the hidden cat sees OUT just fine");
        let m = MISTS_BASE[0];
        assert!(!sees(m.x + 30.0, m.z, m.x, m.z), "mist conceals like brush");
    }

    #[test]
    fn sight_does_not_cross_stone_except_at_the_pass() {
        let w = WALLS_BASE[0];
        let (mx, mz) = ((w.ax + w.bx) * 0.5, (w.az + w.bz) * 0.5);
        // perpendicular to the wall's (1,1) run is (1,-1)
        let quarter = (w.ax + (w.bx - w.ax) * 0.2, w.az + (w.bz - w.az) * 0.2);
        assert!(!sees(quarter.0 + 12.0, quarter.1 - 12.0, quarter.0 - 12.0, quarter.1 + 12.0),
            "the solid span blocks sight");
        assert!(sees(mx + 12.0, mz - 12.0, mx - 12.0, mz + 12.0),
            "the pass through the middle does not");
        assert!(sees(quarter.0 + 1.3, quarter.1 - 1.3, quarter.0 - 1.3, quarter.1 + 1.3),
            "pressed to the stone you hear the claws on the far side");
    }

    #[test]
    fn stone_underfoot_slows_but_the_pass_is_clear() {
        let w = WALLS_BASE[0];
        let quarter = (w.ax + (w.bx - w.ax) * 0.2, w.az + (w.bz - w.az) * 0.2);
        let (mx, mz) = ((w.ax + w.bx) * 0.5, (w.az + w.bz) * 0.5);
        assert!(on_wall(quarter.0, quarter.1), "the solid span is a scramble");
        assert!(!on_wall(mx, mz), "the pass is open ground");
        assert!(!on_wall(0.0, 0.0), "the contention ground is not stone");
    }

    #[test]
    fn the_trails_themselves_stay_open() {
        // no wall's solid span sits on a trail polyline: patrols must never be
        // forced into a scramble just to walk their own lane
        let camps = [(-118.0f32, -118.0f32), (118.0, 118.0)];
        let bends = [(-85.0f32, 85.0f32), (0.0, 0.0), (85.0, -85.0)];
        for &(cx, cz) in &camps {
            for &(bx, bz) in &bends {
                for k in 0..=20 {
                    let t = k as f32 / 20.0;
                    let (x, z) = (cx + (bx - cx) * t, cz + (bz - cz) * t);
                    assert!(!on_wall(x, z), "wall on the trail at ({x:.0},{z:.0})");
                }
            }
        }
    }
}
